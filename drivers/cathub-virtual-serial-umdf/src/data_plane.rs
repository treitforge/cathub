//! Safe, bounded byte transport between the application COM and daemon handles.

use std::collections::VecDeque;

/// Maximum bytes retained in each direction.
pub const DEFAULT_BUFFER_CAPACITY: usize = 64 * 1024;

/// Side of one managed endpoint's device channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    /// Public application COM handle.
    Application,
    /// Private handle owned by the `CatHub` daemon.
    Daemon,
}

impl ChannelRole {
    /// Return the role at the other end of this channel.
    #[must_use]
    pub const fn peer(self) -> Self {
        match self {
            Self::Application => Self::Daemon,
            Self::Daemon => Self::Application,
        }
    }
}

/// Rejected data-plane operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlaneError {
    /// This role already has an open handle.
    AlreadyOpen(ChannelRole),
    /// The role does not currently have an open handle.
    NotOpen(ChannelRole),
    /// The peer must attach before bytes can be accepted.
    PeerNotOpen(ChannelRole),
    /// The entire write would exceed the bounded directional buffer.
    BufferFull {
        /// Number of bytes requested by the caller.
        requested: usize,
        /// Number of bytes that could currently be accepted.
        available: usize,
    },
}

/// One endpoint's in-memory transport state.
#[derive(Debug)]
pub struct EndpointDataPlane {
    application_open: bool,
    daemon_open: bool,
    session_sequence: u64,
    application_to_daemon: BoundedByteQueue,
    daemon_to_application: BoundedByteQueue,
}

impl EndpointDataPlane {
    /// Create a data plane with equal limits in both directions.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            application_open: false,
            daemon_open: false,
            session_sequence: 0,
            application_to_daemon: BoundedByteQueue::new(capacity),
            daemon_to_application: BoundedByteQueue::new(capacity),
        }
    }

    /// Open one exclusive role and return the current application session sequence.
    ///
    /// # Errors
    ///
    /// Returns [`DataPlaneError::AlreadyOpen`] if the role is already occupied.
    pub fn open(&mut self, role: ChannelRole) -> Result<u64, DataPlaneError> {
        let is_open = match role {
            ChannelRole::Application => &mut self.application_open,
            ChannelRole::Daemon => &mut self.daemon_open,
        };
        if *is_open {
            return Err(DataPlaneError::AlreadyOpen(role));
        }
        *is_open = true;
        if role == ChannelRole::Application {
            self.session_sequence = self.session_sequence.wrapping_add(1).max(1);
        }
        Ok(self.session_sequence)
    }

    /// Close one role and discard all bytes in both directions.
    pub fn close(&mut self, role: ChannelRole) {
        match role {
            ChannelRole::Application => self.application_open = false,
            ChannelRole::Daemon => self.daemon_open = false,
        }
        self.application_to_daemon.clear();
        self.daemon_to_application.clear();
    }

    /// Return bytes available for the supplied role to read.
    ///
    /// # Errors
    ///
    /// Returns an error if either side is closed.
    pub fn available_to_read(&self, role: ChannelRole) -> Result<usize, DataPlaneError> {
        self.require_open(role)?;
        self.require_peer_open(role)?;
        Ok(self.incoming(role).len())
    }

    /// Atomically append one entire write to the peer's bounded buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if either side is closed or the complete write does not fit.
    pub fn write(&mut self, role: ChannelRole, bytes: &[u8]) -> Result<usize, DataPlaneError> {
        self.require_open(role)?;
        self.require_peer_open(role)?;
        self.outgoing_mut(role).try_push(bytes)?;
        Ok(bytes.len())
    }

    /// Append one driver-generated private frame for the connected daemon.
    pub fn write_to_daemon(&mut self, bytes: &[u8]) -> Result<usize, DataPlaneError> {
        self.require_open(ChannelRole::Daemon)?;
        self.application_to_daemon.try_push(bytes)?;
        Ok(bytes.len())
    }

    /// Read private driver frames without requiring an application COM handle.
    pub fn read_for_daemon(&mut self, output: &mut [u8]) -> Result<usize, DataPlaneError> {
        self.require_open(ChannelRole::Daemon)?;
        Ok(self.application_to_daemon.pop_into(output))
    }

    /// Return private bytes waiting for the daemon.
    pub fn available_for_daemon(&self) -> Result<usize, DataPlaneError> {
        self.require_open(ChannelRole::Daemon)?;
        Ok(self.application_to_daemon.len())
    }

    /// Remove up to `output.len()` bytes waiting for the supplied role.
    ///
    /// # Errors
    ///
    /// Returns an error if either side is closed.
    pub fn read(&mut self, role: ChannelRole, output: &mut [u8]) -> Result<usize, DataPlaneError> {
        self.require_open(role)?;
        self.require_peer_open(role)?;
        Ok(self.incoming_mut(role).pop_into(output))
    }

    /// Discard bytes waiting to be read by `role`.
    pub fn clear_incoming(&mut self, role: ChannelRole) {
        self.incoming_mut(role).clear();
    }

    /// Discard bytes written by `role` but not yet read by its peer.
    pub fn clear_outgoing(&mut self, role: ChannelRole) {
        self.outgoing_mut(role).clear();
    }

    /// Return queued bytes waiting for `role`, even while its peer is disconnected.
    #[must_use]
    pub fn incoming_len(&self, role: ChannelRole) -> usize {
        self.incoming(role).len()
    }

    /// Return bytes written by `role` and waiting for its peer.
    #[must_use]
    pub fn outgoing_len(&self, role: ChannelRole) -> usize {
        self.incoming(role.peer()).len()
    }

    const fn require_open(&self, role: ChannelRole) -> Result<(), DataPlaneError> {
        let open = match role {
            ChannelRole::Application => self.application_open,
            ChannelRole::Daemon => self.daemon_open,
        };
        if open {
            Ok(())
        } else {
            Err(DataPlaneError::NotOpen(role))
        }
    }

    fn require_peer_open(&self, role: ChannelRole) -> Result<(), DataPlaneError> {
        self.require_open(role.peer())
            .map_err(|_| DataPlaneError::PeerNotOpen(role.peer()))
    }

    const fn incoming(&self, role: ChannelRole) -> &BoundedByteQueue {
        match role {
            ChannelRole::Application => &self.daemon_to_application,
            ChannelRole::Daemon => &self.application_to_daemon,
        }
    }

    const fn incoming_mut(&mut self, role: ChannelRole) -> &mut BoundedByteQueue {
        match role {
            ChannelRole::Application => &mut self.daemon_to_application,
            ChannelRole::Daemon => &mut self.application_to_daemon,
        }
    }

    const fn outgoing_mut(&mut self, role: ChannelRole) -> &mut BoundedByteQueue {
        match role {
            ChannelRole::Application => &mut self.application_to_daemon,
            ChannelRole::Daemon => &mut self.daemon_to_application,
        }
    }
}

#[derive(Debug)]
struct BoundedByteQueue {
    bytes: VecDeque<u8>,
    capacity: usize,
}

impl BoundedByteQueue {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn try_push(&mut self, bytes: &[u8]) -> Result<(), DataPlaneError> {
        let available = self.capacity.saturating_sub(self.bytes.len());
        if bytes.len() > available {
            return Err(DataPlaneError::BufferFull {
                requested: bytes.len(),
                available,
            });
        }
        self.bytes.extend(bytes.iter().copied());
        Ok(())
    }

    fn pop_into(&mut self, output: &mut [u8]) -> usize {
        let count = output.len().min(self.bytes.len());
        for (slot, byte) in output.iter_mut().zip(self.bytes.drain(..count)) {
            *slot = byte;
        }
        count
    }

    fn clear(&mut self) {
        self.bytes.clear();
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn connected(capacity: usize) -> EndpointDataPlane {
        let mut plane = EndpointDataPlane::new(capacity);
        plane.open(ChannelRole::Application).expect("application");
        plane.open(ChannelRole::Daemon).expect("daemon");
        plane
    }

    #[test]
    fn roles_are_exclusive() {
        let mut plane = EndpointDataPlane::new(8);
        assert_eq!(plane.open(ChannelRole::Application), Ok(1));
        assert_eq!(
            plane.open(ChannelRole::Application),
            Err(DataPlaneError::AlreadyOpen(ChannelRole::Application))
        );
    }

    #[test]
    fn writes_fail_closed_until_peer_attaches() {
        let mut plane = EndpointDataPlane::new(8);
        plane.open(ChannelRole::Application).expect("application");
        assert_eq!(
            plane.write(ChannelRole::Application, b"FA;"),
            Err(DataPlaneError::PeerNotOpen(ChannelRole::Daemon))
        );
    }

    #[test]
    fn directions_do_not_leak_into_each_other() {
        let mut plane = connected(16);
        plane
            .write(ChannelRole::Application, b"application")
            .expect("application write");
        plane
            .write(ChannelRole::Daemon, b"daemon")
            .expect("daemon write");

        let mut application = [0; 16];
        let mut daemon = [0; 16];
        let application_len = plane
            .read(ChannelRole::Application, &mut application)
            .expect("application read");
        let daemon_len = plane
            .read(ChannelRole::Daemon, &mut daemon)
            .expect("daemon read");

        assert_eq!(&application[..application_len], b"daemon");
        assert_eq!(&daemon[..daemon_len], b"application");
    }

    #[test]
    fn overflowing_write_is_rejected_without_partial_data() {
        let mut plane = connected(4);
        assert_eq!(
            plane.write(ChannelRole::Application, b"12345"),
            Err(DataPlaneError::BufferFull {
                requested: 5,
                available: 4,
            })
        );
        assert_eq!(plane.available_to_read(ChannelRole::Daemon), Ok(0));
    }

    #[test]
    fn partial_read_preserves_remaining_bytes() {
        let mut plane = connected(8);
        plane
            .write(ChannelRole::Application, b"123456")
            .expect("write");
        let mut first = [0; 2];
        let mut second = [0; 8];

        assert_eq!(plane.read(ChannelRole::Daemon, &mut first), Ok(2));
        assert_eq!(&first, b"12");
        assert_eq!(plane.read(ChannelRole::Daemon, &mut second), Ok(4));
        assert_eq!(&second[..4], b"3456");
    }

    #[test]
    fn either_close_discards_both_directions_and_advances_next_session() {
        let mut plane = connected(8);
        plane
            .write(ChannelRole::Application, b"app")
            .expect("application write");
        plane
            .write(ChannelRole::Daemon, b"daemon")
            .expect("daemon write");

        plane.close(ChannelRole::Application);
        assert_eq!(
            plane.available_to_read(ChannelRole::Application),
            Err(DataPlaneError::NotOpen(ChannelRole::Application))
        );
        assert_eq!(
            plane.available_to_read(ChannelRole::Daemon),
            Err(DataPlaneError::PeerNotOpen(ChannelRole::Application))
        );
        assert_eq!(plane.open(ChannelRole::Application), Ok(2));
        assert_eq!(plane.available_to_read(ChannelRole::Application), Ok(0));
    }

    #[test]
    fn purge_directions_are_independent() {
        let mut plane = connected(8);
        plane
            .write(ChannelRole::Application, b"out")
            .expect("application write");
        plane
            .write(ChannelRole::Daemon, b"in")
            .expect("daemon write");

        plane.clear_incoming(ChannelRole::Application);
        assert_eq!(plane.incoming_len(ChannelRole::Application), 0);
        assert_eq!(plane.outgoing_len(ChannelRole::Application), 3);
        plane.clear_outgoing(ChannelRole::Application);
        assert_eq!(plane.outgoing_len(ChannelRole::Application), 0);
    }

    #[test]
    fn driver_control_frames_do_not_require_application_open() {
        let mut plane = EndpointDataPlane::new(32);
        plane.open(ChannelRole::Daemon).expect("daemon");
        plane.write_to_daemon(b"hello").expect("control frame");
        assert_eq!(plane.available_for_daemon(), Ok(5));
        let mut output = [0_u8; 8];
        assert_eq!(plane.read_for_daemon(&mut output), Ok(5));
        assert_eq!(&output[..5], b"hello");
    }
}
