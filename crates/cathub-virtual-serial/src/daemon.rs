//! Safe daemon-side state machine for the private virtual-serial protocol.

use crate::protocol::{
    control_field, data_field, endpoint_field, feature, hello_field, serial_field, session_field,
    Frame, FrameDecoder, FrameFlags, MessageKind, ProtocolError, ABSOLUTE_MAX_FRAME_LEN,
    CURRENT_VERSION,
};

/// Default bounded receive window advertised by the daemon.
pub const DEFAULT_RECEIVE_WINDOW: usize = 64 * 1024;

/// Version 1.0 capabilities required by the managed transport.
pub const REQUIRED_FEATURES: u64 = feature::APPLICATION_LIFECYCLE
    | feature::SERIAL_CONFIG
    | feature::MODEM_LINES
    | feature::CANCELLATION
    | feature::PURGE
    | feature::HEALTH
    | feature::RECEIVE_CREDIT;

/// One driver-reported endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointDescriptor {
    /// Driver-scoped endpoint identifier.
    pub endpoint_id: u64,
    /// Endpoint kind: 1 is CAT and 2 is WinKeyer.
    pub kind: u16,
    /// Stable configuration identifier.
    pub stable_id: String,
    /// Operator-facing name.
    pub display_name: String,
    /// Whether the driver currently permits attachment.
    pub enabled: bool,
}

/// Serial configuration reported by the application COM handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialConfiguration {
    /// Baud rate selected by the application.
    pub baud: u32,
    /// Number of data bits.
    pub data_bits: u16,
    /// Windows parity enum.
    pub parity: u16,
    /// Windows stop-bit enum.
    pub stop_bits: u16,
    /// Protocol flow-control enum.
    pub flow_control: u16,
    /// Read timeout in milliseconds.
    pub read_timeout_ms: u32,
    /// Write timeout in milliseconds.
    pub write_timeout_ms: u32,
}

/// Event produced from driver traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    /// Version negotiation completed.
    Negotiated,
    /// One endpoint was discovered.
    Endpoint(EndpointDescriptor),
    /// Endpoint enumeration completed.
    DiscoveryComplete,
    /// Attachment completed with the active application session.
    Attached {
        /// Attached driver endpoint.
        endpoint_id: u64,
        /// Current application-open sequence, or zero while closed.
        session_id: u64,
    },
    /// Application bytes arrived from the public COM handle.
    Data(Vec<u8>),
    /// The application opened its COM handle.
    ApplicationOpen(u64),
    /// The application closed its COM handle.
    ApplicationClose(u64),
    /// The application changed its serial configuration.
    SerialConfiguration(SerialConfiguration),
    /// The application changed DTR, RTS, or break.
    ModemControl(u32),
    /// The application purged one or more queues.
    Purge(u32),
    /// The application canceled an operation.
    Cancel {
        /// Driver-assigned operation identifier.
        operation_id: u64,
        /// Operation kind being canceled.
        kind: u32,
    },
    /// A health request completed.
    Health {
        /// Whether the driver reports an active attachment.
        attached: bool,
        /// Current application-open sequence.
        session_id: u64,
        /// Redacted health detail.
        detail: String,
    },
    /// The driver detached the endpoint.
    Detached,
    /// The driver rejected a request.
    Error {
        /// Stable protocol error code.
        code: u32,
        /// Redacted diagnostic detail.
        detail: String,
    },
}

/// Daemon protocol failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DaemonProtocolError {
    /// Shared frame decoder rejected input.
    #[error("invalid private transport frame: {0}")]
    Protocol(#[from] ProtocolError),
    /// Driver selected an incompatible contract.
    #[error("driver selected an incompatible protocol")]
    Version,
    /// Driver omitted a required capability.
    #[error("driver omitted a required feature")]
    Features,
    /// A frame exceeds the negotiated limit.
    #[error("frame exceeds the negotiated limit")]
    FrameLimit,
    /// A data operation was attempted before attachment.
    #[error("endpoint is not attached")]
    NotAttached,
    /// A data sequence was repeated or moved backward.
    #[error("invalid data sequence")]
    Sequence,
    /// Peer exceeded its receive credit.
    #[error("receive credit exhausted")]
    Credit,
    /// A frame addressed a different endpoint.
    #[error("frame addressed a different endpoint")]
    WrongEndpoint,
}

/// State machine shared by the Windows transport adapter and its tests.
#[derive(Debug)]
pub struct DaemonProtocol {
    decoder: FrameDecoder,
    negotiated: bool,
    attached_endpoint: Option<u64>,
    frame_limit: usize,
    send_limit: usize,
    send_credit: usize,
    receive_credit: usize,
    outbound_sequence: u64,
    inbound_sequence: u64,
}

impl DaemonProtocol {
    /// Create an unnegotiated daemon protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            decoder: FrameDecoder::default(),
            negotiated: false,
            attached_endpoint: None,
            frame_limit: ABSOLUTE_MAX_FRAME_LEN,
            send_limit: 0,
            send_credit: 0,
            receive_credit: DEFAULT_RECEIVE_WINDOW,
            outbound_sequence: 0,
            inbound_sequence: 0,
        }
    }

    /// Build the initial version and capability offer.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be represented by protocol 1.0.
    pub fn hello(request_id: u64) -> Result<Vec<u8>, DaemonProtocolError> {
        let mut frame = Frame::new(MessageKind::Hello, 0, request_id);
        frame
            .fields
            .insert_u16(hello_field::MIN_MAJOR, CURRENT_VERSION.major)?;
        frame
            .fields
            .insert_u16(hello_field::MIN_MINOR, CURRENT_VERSION.minor)?;
        frame
            .fields
            .insert_u16(hello_field::MAX_MAJOR, CURRENT_VERSION.major)?;
        frame
            .fields
            .insert_u16(hello_field::MAX_MINOR, CURRENT_VERSION.minor)?;
        frame.fields.insert_u32(
            hello_field::MAX_FRAME_BYTES,
            u32::try_from(ABSOLUTE_MAX_FRAME_LEN).map_err(|_| DaemonProtocolError::FrameLimit)?,
        )?;
        frame.fields.insert_u32(
            hello_field::RECEIVE_WINDOW_BYTES,
            u32::try_from(DEFAULT_RECEIVE_WINDOW).map_err(|_| DaemonProtocolError::Credit)?,
        )?;
        frame
            .fields
            .insert_u64(hello_field::FEATURES, REQUIRED_FEATURES)?;
        frame
            .fields
            .insert_string(hello_field::IDENTITY, "cathub-daemon")?;
        Ok(frame.encode()?)
    }

    /// Request the driver's endpoint inventory.
    ///
    /// # Errors
    ///
    /// Returns an error before negotiation or if encoding fails.
    pub fn discover(&self, request_id: u64) -> Result<Vec<u8>, DaemonProtocolError> {
        self.require_negotiated()?;
        self.encode(&Frame::new(MessageKind::Discover, 0, request_id))
    }

    /// Attach to one discovered endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error before negotiation or if encoding fails.
    pub fn attach(
        &self,
        endpoint_id: u64,
        request_id: u64,
    ) -> Result<Vec<u8>, DaemonProtocolError> {
        self.require_negotiated()?;
        let mut frame = Frame::new(MessageKind::Attach, endpoint_id, request_id);
        frame.fields.insert_u64(session_field::SESSION_ID, 0)?;
        frame.fields.insert_u32(
            session_field::RECEIVE_CREDIT,
            u32::try_from(DEFAULT_RECEIVE_WINDOW).map_err(|_| DaemonProtocolError::Credit)?,
        )?;
        self.encode(&frame)
    }

    /// Encode bytes produced by the CatHub endpoint session.
    ///
    /// # Errors
    ///
    /// Returns an error before attachment, when credit is exhausted, or if encoding fails.
    pub fn data(&mut self, bytes: &[u8]) -> Result<Vec<u8>, DaemonProtocolError> {
        let endpoint_id = self
            .attached_endpoint
            .ok_or(DaemonProtocolError::NotAttached)?;
        if bytes.len() > self.send_credit {
            return Err(DaemonProtocolError::Credit);
        }
        self.outbound_sequence = self.outbound_sequence.wrapping_add(1).max(1);
        let mut frame = Frame::new(MessageKind::Data, endpoint_id, 0);
        frame
            .fields
            .insert_u64(data_field::SEQUENCE, self.outbound_sequence)?;
        frame.fields.insert(data_field::BYTES, bytes)?;
        let encoded = self.encode(&frame)?;
        self.send_credit -= bytes.len();
        Ok(encoded)
    }

    /// Restore driver send credit after application bytes enter the endpoint session.
    ///
    /// # Errors
    ///
    /// Returns an error if the credit value or frame cannot be encoded.
    pub fn release_received(
        &mut self,
        count: usize,
    ) -> Result<Option<Vec<u8>>, DaemonProtocolError> {
        let Some(endpoint_id) = self.attached_endpoint else {
            return Ok(None);
        };
        if count == 0 {
            return Ok(None);
        }
        self.receive_credit = self
            .receive_credit
            .saturating_add(count)
            .min(DEFAULT_RECEIVE_WINDOW);
        let mut frame = Frame::new(MessageKind::WindowUpdate, endpoint_id, 0);
        frame.fields.insert_u32(
            data_field::CREDIT,
            u32::try_from(count).map_err(|_| DaemonProtocolError::Credit)?,
        )?;
        self.encode(&frame).map(Some)
    }

    /// Build a transport health request.
    ///
    /// # Errors
    ///
    /// Returns an error before negotiation or if encoding fails.
    pub fn health(&self, request_id: u64) -> Result<Vec<u8>, DaemonProtocolError> {
        self.require_negotiated()?;
        self.encode(&Frame::new(MessageKind::Health, 0, request_id))
    }

    /// Build a graceful detach frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the detach frame cannot be encoded.
    pub fn detach(&self, reason: u32) -> Result<Option<Vec<u8>>, DaemonProtocolError> {
        let Some(endpoint_id) = self.attached_endpoint else {
            return Ok(None);
        };
        let mut frame = Frame::new(MessageKind::Detach, endpoint_id, 0);
        frame
            .fields
            .insert_u32(session_field::OPTIONS_OR_REASON, reason)?;
        self.encode(&frame).map(Some)
    }

    /// Consume any private stream fragment and return decoded events.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, incompatible, oversized, out-of-sequence, or
    /// credit-violating driver traffic.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        if self.decoder.buffered_len().saturating_add(bytes.len()) > ABSOLUTE_MAX_FRAME_LEN {
            return Err(DaemonProtocolError::FrameLimit);
        }
        self.decoder.push(bytes);
        let mut events = Vec::new();
        while let Some(frame) = self.decoder.next_frame()? {
            if self.negotiated && frame.encode()?.len() > self.frame_limit {
                return Err(DaemonProtocolError::FrameLimit);
            }
            if self.negotiated && frame.version != CURRENT_VERSION {
                return Err(DaemonProtocolError::Version);
            }
            events.extend(self.process_frame(&frame)?);
        }
        Ok(events)
    }

    fn process_frame(&mut self, frame: &Frame) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        match frame.kind {
            MessageKind::HelloAck => self.process_hello_ack(frame),
            MessageKind::Endpoint => Ok(vec![DaemonEvent::Endpoint(EndpointDescriptor {
                endpoint_id: frame.endpoint_id,
                kind: frame.fields.require_u16(endpoint_field::KIND)?,
                stable_id: frame
                    .fields
                    .require_string(endpoint_field::STABLE_ID)?
                    .to_owned(),
                display_name: frame
                    .fields
                    .require_string(endpoint_field::DISPLAY_NAME)?
                    .to_owned(),
                enabled: frame.fields.require_u16(endpoint_field::ENABLED)? != 0,
            })]),
            MessageKind::DiscoverComplete => Ok(vec![DaemonEvent::DiscoveryComplete]),
            MessageKind::AttachAck => self.process_attach_ack(frame),
            MessageKind::Data => self.process_data(frame),
            MessageKind::WindowUpdate => self.process_window_update(frame),
            MessageKind::ApplicationOpen => Ok(vec![DaemonEvent::ApplicationOpen(
                frame
                    .fields
                    .require_u64(control_field::SEQUENCE_OR_OPERATION_ID)?,
            )]),
            MessageKind::ApplicationClose => Ok(vec![DaemonEvent::ApplicationClose(
                frame
                    .fields
                    .require_u64(control_field::SEQUENCE_OR_OPERATION_ID)?,
            )]),
            MessageKind::SerialConfig => Ok(vec![DaemonEvent::SerialConfiguration(
                SerialConfiguration {
                    baud: frame.fields.require_u32(serial_field::BAUD)?,
                    data_bits: frame.fields.require_u16(serial_field::DATA_BITS)?,
                    parity: frame.fields.require_u16(serial_field::PARITY)?,
                    stop_bits: frame.fields.require_u16(serial_field::STOP_BITS)?,
                    flow_control: frame.fields.require_u16(serial_field::FLOW_CONTROL)?,
                    read_timeout_ms: frame.fields.require_u32(serial_field::READ_TIMEOUT_MS)?,
                    write_timeout_ms: frame.fields.require_u32(serial_field::WRITE_TIMEOUT_MS)?,
                },
            )]),
            MessageKind::ModemControl => Ok(vec![DaemonEvent::ModemControl(
                frame.fields.require_u32(control_field::MASK)?,
            )]),
            MessageKind::Purge => Ok(vec![DaemonEvent::Purge(
                frame.fields.require_u32(control_field::MASK)?,
            )]),
            MessageKind::Cancel => Ok(vec![DaemonEvent::Cancel {
                operation_id: frame
                    .fields
                    .require_u64(control_field::SEQUENCE_OR_OPERATION_ID)?,
                kind: frame
                    .fields
                    .require_u32(control_field::KIND_OR_ERROR_CODE)?,
            }]),
            MessageKind::HealthAck => Ok(vec![DaemonEvent::Health {
                attached: frame.fields.require_u32(control_field::MASK)? != 0,
                session_id: frame
                    .fields
                    .require_u64(control_field::SEQUENCE_OR_OPERATION_ID)?,
                detail: frame
                    .fields
                    .require_string(control_field::DETAIL)?
                    .to_owned(),
            }]),
            MessageKind::Detach => {
                self.attached_endpoint = None;
                Ok(vec![DaemonEvent::Detached])
            }
            MessageKind::Error => Ok(vec![DaemonEvent::Error {
                code: frame
                    .fields
                    .require_u32(control_field::KIND_OR_ERROR_CODE)?,
                detail: frame
                    .fields
                    .require_string(control_field::DETAIL)?
                    .to_owned(),
            }]),
            _ => Ok(Vec::new()),
        }
    }

    fn process_hello_ack(
        &mut self,
        frame: &Frame,
    ) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        if !frame.flags.contains(FrameFlags::RESPONSE)
            || frame.fields.require_u16(hello_field::MIN_MAJOR)? != CURRENT_VERSION.major
            || frame.fields.require_u16(hello_field::MIN_MINOR)? != CURRENT_VERSION.minor
            || frame.fields.require_u16(hello_field::MAX_MAJOR)? != CURRENT_VERSION.major
            || frame.fields.require_u16(hello_field::MAX_MINOR)? != CURRENT_VERSION.minor
        {
            return Err(DaemonProtocolError::Version);
        }
        let features = frame.fields.require_u64(hello_field::FEATURES)?;
        if features & REQUIRED_FEATURES != REQUIRED_FEATURES {
            return Err(DaemonProtocolError::Features);
        }
        let frame_limit = usize::try_from(frame.fields.require_u32(hello_field::MAX_FRAME_BYTES)?)
            .map_err(|_| DaemonProtocolError::FrameLimit)?;
        if frame_limit < 64 {
            return Err(DaemonProtocolError::FrameLimit);
        }
        let receive_window = usize::try_from(
            frame
                .fields
                .require_u32(hello_field::RECEIVE_WINDOW_BYTES)?,
        )
        .map_err(|_| DaemonProtocolError::Credit)?;
        if receive_window == 0 {
            return Err(DaemonProtocolError::Credit);
        }
        self.negotiated = true;
        self.frame_limit = frame_limit.min(ABSOLUTE_MAX_FRAME_LEN);
        self.send_limit = receive_window;
        self.send_credit = receive_window;
        Ok(vec![DaemonEvent::Negotiated])
    }

    fn process_attach_ack(
        &mut self,
        frame: &Frame,
    ) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        let receive_window =
            usize::try_from(frame.fields.require_u32(session_field::RECEIVE_CREDIT)?)
                .map_err(|_| DaemonProtocolError::Credit)?;
        if receive_window == 0 {
            return Err(DaemonProtocolError::Credit);
        }
        self.attached_endpoint = Some(frame.endpoint_id);
        self.send_limit = receive_window;
        self.send_credit = receive_window;
        self.receive_credit = DEFAULT_RECEIVE_WINDOW;
        self.outbound_sequence = 0;
        self.inbound_sequence = 0;
        Ok(vec![DaemonEvent::Attached {
            endpoint_id: frame.endpoint_id,
            session_id: frame.fields.require_u64(session_field::SESSION_ID)?,
        }])
    }

    fn process_data(&mut self, frame: &Frame) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        self.require_endpoint(frame)?;
        let sequence = frame.fields.require_u64(data_field::SEQUENCE)?;
        if sequence <= self.inbound_sequence {
            return Err(DaemonProtocolError::Sequence);
        }
        let bytes = frame
            .fields
            .get(data_field::BYTES)
            .ok_or(ProtocolError::MissingField(data_field::BYTES))?;
        if bytes.len() > self.receive_credit {
            return Err(DaemonProtocolError::Credit);
        }
        self.inbound_sequence = sequence;
        self.receive_credit -= bytes.len();
        Ok(vec![DaemonEvent::Data(bytes.to_vec())])
    }

    fn process_window_update(
        &mut self,
        frame: &Frame,
    ) -> Result<Vec<DaemonEvent>, DaemonProtocolError> {
        self.require_endpoint(frame)?;
        let credit = usize::try_from(frame.fields.require_u32(data_field::CREDIT)?)
            .map_err(|_| DaemonProtocolError::Credit)?;
        if credit == 0 {
            return Err(DaemonProtocolError::Credit);
        }
        self.send_credit = self.send_credit.saturating_add(credit).min(self.send_limit);
        Ok(Vec::new())
    }

    fn require_negotiated(&self) -> Result<(), DaemonProtocolError> {
        if self.negotiated {
            Ok(())
        } else {
            Err(DaemonProtocolError::Version)
        }
    }

    fn require_endpoint(&self, frame: &Frame) -> Result<(), DaemonProtocolError> {
        if self.attached_endpoint == Some(frame.endpoint_id) {
            Ok(())
        } else {
            Err(DaemonProtocolError::WrongEndpoint)
        }
    }

    fn encode(&self, frame: &Frame) -> Result<Vec<u8>, DaemonProtocolError> {
        let encoded = frame.encode()?;
        if encoded.len() > self.frame_limit {
            Err(DaemonProtocolError::FrameLimit)
        } else {
            Ok(encoded)
        }
    }
}

impl Default for DaemonProtocol {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn hello_ack() -> Frame {
        let mut frame = Frame::new(MessageKind::HelloAck, 0, 1);
        frame.flags = FrameFlags::RESPONSE | FrameFlags::FINAL;
        frame
            .fields
            .insert_u16(hello_field::MIN_MAJOR, 1)
            .expect("min major");
        frame
            .fields
            .insert_u16(hello_field::MIN_MINOR, 0)
            .expect("min minor");
        frame
            .fields
            .insert_u16(hello_field::MAX_MAJOR, 1)
            .expect("max major");
        frame
            .fields
            .insert_u16(hello_field::MAX_MINOR, 0)
            .expect("max minor");
        frame
            .fields
            .insert_u32(hello_field::MAX_FRAME_BYTES, 65_536)
            .expect("frame");
        frame
            .fields
            .insert_u32(hello_field::RECEIVE_WINDOW_BYTES, 65_536)
            .expect("window");
        frame
            .fields
            .insert_u64(hello_field::FEATURES, REQUIRED_FEATURES)
            .expect("features");
        frame
            .fields
            .insert_string(hello_field::IDENTITY, "driver")
            .expect("identity");
        frame
    }

    fn attached_protocol() -> DaemonProtocol {
        let mut protocol = DaemonProtocol::new();
        protocol
            .ingest(&hello_ack().encode().expect("encode"))
            .expect("hello ack");
        let mut ack = Frame::new(MessageKind::AttachAck, 7, 2);
        ack.flags = FrameFlags::RESPONSE | FrameFlags::FINAL;
        ack.fields
            .insert_u64(session_field::SESSION_ID, 12)
            .expect("session");
        ack.fields
            .insert_u32(session_field::RECEIVE_CREDIT, 65_536)
            .expect("credit");
        protocol
            .ingest(&ack.encode().expect("encode"))
            .expect("attach ack");
        protocol
    }

    #[test]
    fn hello_offer_contains_required_contract() {
        let frame = Frame::decode(&DaemonProtocol::hello(9).expect("hello")).expect("decode");
        assert_eq!(frame.kind, MessageKind::Hello);
        assert_eq!(frame.request_id, 9);
        assert_eq!(
            frame.fields.require_u64(hello_field::FEATURES),
            Ok(REQUIRED_FEATURES)
        );
    }

    #[test]
    fn decodes_split_negotiation_and_discovery() {
        let mut protocol = DaemonProtocol::new();
        let bytes = hello_ack().encode().expect("encode");
        assert!(protocol.ingest(&bytes[..13]).expect("part").is_empty());
        assert_eq!(
            protocol.ingest(&bytes[13..]).expect("rest"),
            vec![DaemonEvent::Negotiated]
        );
        assert!(protocol.discover(2).is_ok());

        let mut endpoint = Frame::new(MessageKind::Endpoint, 7, 2);
        endpoint
            .fields
            .insert_u16(endpoint_field::KIND, 1)
            .expect("kind");
        endpoint
            .fields
            .insert_string(endpoint_field::STABLE_ID, "n1mm")
            .expect("id");
        endpoint
            .fields
            .insert_string(endpoint_field::DISPLAY_NAME, "N1MM")
            .expect("name");
        endpoint
            .fields
            .insert_u16(endpoint_field::ENABLED, 1)
            .expect("enabled");
        let events = protocol
            .ingest(&endpoint.encode().expect("encode"))
            .expect("endpoint");
        assert_eq!(
            events,
            vec![DaemonEvent::Endpoint(EndpointDescriptor {
                endpoint_id: 7,
                kind: 1,
                stable_id: "n1mm".to_owned(),
                display_name: "N1MM".to_owned(),
                enabled: true,
            })]
        );
    }

    #[test]
    fn attached_data_uses_sequences_and_credit() {
        let mut protocol = attached_protocol();
        let outbound = Frame::decode(&protocol.data(b"reply").expect("data")).expect("decode");
        assert_eq!(outbound.endpoint_id, 7);
        assert_eq!(outbound.fields.require_u64(data_field::SEQUENCE), Ok(1));

        let mut inbound = Frame::new(MessageKind::Data, 7, 0);
        inbound
            .fields
            .insert_u64(data_field::SEQUENCE, 1)
            .expect("sequence");
        inbound
            .fields
            .insert(data_field::BYTES, b"request")
            .expect("bytes");
        assert_eq!(
            protocol
                .ingest(&inbound.encode().expect("encode"))
                .expect("ingest"),
            vec![DaemonEvent::Data(b"request".to_vec())]
        );
        let update = Frame::decode(
            &protocol
                .release_received(7)
                .expect("release")
                .expect("update"),
        )
        .expect("decode");
        assert_eq!(update.kind, MessageKind::WindowUpdate);
        assert_eq!(update.fields.require_u32(data_field::CREDIT), Ok(7));
    }
}
