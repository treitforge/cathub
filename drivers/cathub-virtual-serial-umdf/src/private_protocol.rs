//! Safe implementation of the private `CatHub` driver protocol.

use cathub_virtual_serial::protocol::{
    ABSOLUTE_MAX_FRAME_LEN, CURRENT_VERSION, Frame, FrameDecoder, FrameFlags, MessageKind,
    ProtocolError, ProtocolVersion, control_field, data_field, endpoint_field, feature,
    hello_field, serial_field, session_field,
};

use crate::data_plane::DEFAULT_BUFFER_CAPACITY;

/// The first driver package exposes one endpoint per device instance.
pub const ENDPOINT_ID: u64 = 1;
const ENDPOINT_KIND_CAT: u16 = 1;
const ENDPOINT_STABLE_ID: &str = "cathub-default";
const ENDPOINT_DISPLAY_NAME: &str = "CatHub Virtual Serial Port";
const DRIVER_IDENTITY: &str = "cathub-umdf";
const REQUIRED_FEATURES: u64 = feature::APPLICATION_LIFECYCLE
    | feature::SERIAL_CONFIG
    | feature::MODEM_LINES
    | feature::CANCELLATION
    | feature::PURGE
    | feature::HEALTH
    | feature::RECEIVE_CREDIT;

/// One effect emitted while processing private protocol traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolOutput {
    /// Encoded frame to make available to the `CatHub` daemon.
    ToDaemon(Vec<u8>),
    /// Opaque serial bytes to make available to the application COM handle.
    ToApplication(Vec<u8>),
    /// New CTS/DSR/DCD/RI input-line bitmap.
    ModemStatus(u32),
}

/// Fatal private-channel error. The daemon must reconnect after this result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverProtocolError {
    Framing,
    Version,
    Features,
    FrameLimit,
    ReceiveWindow,
    WrongEndpoint,
    NotReady,
    NotAttached,
    Session,
    Sequence,
    Credit,
    Encoding,
}

impl From<ProtocolError> for DriverProtocolError {
    fn from(_error: ProtocolError) -> Self {
        Self::Framing
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolPhase {
    AwaitHello,
    Ready,
}

/// Provisioned identity for one public COM device instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointMetadata {
    /// Endpoint kind: 1 is CAT and 2 is `WinKeyer`.
    pub kind: u16,
    /// Stable identifier referenced by `CatHub` configuration.
    pub stable_id: String,
    /// Operator-facing endpoint description.
    pub display_name: String,
}

impl Default for EndpointMetadata {
    fn default() -> Self {
        Self {
            kind: ENDPOINT_KIND_CAT,
            stable_id: ENDPOINT_STABLE_ID.to_owned(),
            display_name: ENDPOINT_DISPLAY_NAME.to_owned(),
        }
    }
}

/// Protocol state owned by one driver device instance.
#[derive(Debug)]
pub struct DriverProtocol {
    endpoint: EndpointMetadata,
    decoder: FrameDecoder,
    phase: ProtocolPhase,
    attached: bool,
    application_open: bool,
    application_session: u64,
    negotiated_frame_limit: usize,
    daemon_receive_limit: usize,
    daemon_receive_credit: usize,
    driver_receive_credit: usize,
    outbound_sequence: u64,
    inbound_sequence: u64,
}

impl DriverProtocol {
    /// Create a disconnected protocol endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self::for_endpoint(EndpointMetadata::default())
    }

    /// Create a disconnected protocol endpoint with a provisioned identity.
    #[must_use]
    pub fn for_endpoint(endpoint: EndpointMetadata) -> Self {
        Self {
            endpoint,
            decoder: FrameDecoder::default(),
            phase: ProtocolPhase::AwaitHello,
            attached: false,
            application_open: false,
            application_session: 0,
            negotiated_frame_limit: ABSOLUTE_MAX_FRAME_LEN,
            daemon_receive_limit: 0,
            daemon_receive_credit: 0,
            driver_receive_credit: DEFAULT_BUFFER_CAPACITY,
            outbound_sequence: 0,
            inbound_sequence: 0,
        }
    }

    /// Reset negotiation when the private daemon handle changes.
    pub fn reset_daemon(&mut self) {
        let application_open = self.application_open;
        let application_session = self.application_session;
        let endpoint = self.endpoint.clone();
        *self = Self::for_endpoint(endpoint);
        self.application_open = application_open;
        self.application_session = application_session;
    }

    /// Record a new application COM session and emit its lifecycle event when attached.
    pub fn application_opened(
        &mut self,
        session: u64,
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        self.application_open = true;
        self.application_session = session;
        if self.attached {
            Ok(vec![
                self.application_lifecycle(MessageKind::ApplicationOpen)?,
            ])
        } else {
            Ok(Vec::new())
        }
    }

    /// Record application cleanup and emit a close event when attached.
    pub fn application_closed(&mut self) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        let outputs = if self.attached && self.application_open {
            vec![self.application_lifecycle(MessageKind::ApplicationClose)?]
        } else {
            Vec::new()
        };
        self.application_open = false;
        Ok(outputs)
    }

    /// Encode application bytes for the attached daemon.
    pub fn application_data(
        &mut self,
        bytes: &[u8],
    ) -> Result<ProtocolOutput, DriverProtocolError> {
        self.require_attached()?;
        if !self.application_open {
            return Err(DriverProtocolError::Session);
        }
        if bytes.len() > self.daemon_receive_credit {
            return Err(DriverProtocolError::Credit);
        }
        self.outbound_sequence = self.outbound_sequence.wrapping_add(1).max(1);
        let mut frame = Frame::new(MessageKind::Data, ENDPOINT_ID, 0);
        frame
            .fields
            .insert_u64(data_field::SEQUENCE, self.outbound_sequence)?;
        frame.fields.insert(data_field::BYTES, bytes)?;
        let encoded = encode_frame(&frame, self.negotiated_frame_limit)?;
        self.daemon_receive_credit -= bytes.len();
        Ok(encoded)
    }

    /// Restore daemon-to-driver receive credit after application bytes are released.
    pub fn application_bytes_released(
        &mut self,
        count: usize,
    ) -> Result<Option<ProtocolOutput>, DriverProtocolError> {
        if count == 0 || !self.attached {
            return Ok(None);
        }
        self.driver_receive_credit = self
            .driver_receive_credit
            .saturating_add(count)
            .min(DEFAULT_BUFFER_CAPACITY);
        let credit = u32::try_from(count).map_err(|_| DriverProtocolError::Credit)?;
        let mut frame = Frame::new(MessageKind::WindowUpdate, ENDPOINT_ID, 0);
        frame.fields.insert_u32(data_field::CREDIT, credit)?;
        encode_frame(&frame, self.negotiated_frame_limit).map(Some)
    }

    /// Consume an arbitrary stream fragment written by the daemon.
    pub fn ingest_daemon(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        if self.decoder.buffered_len().saturating_add(bytes.len()) > ABSOLUTE_MAX_FRAME_LEN {
            return Err(DriverProtocolError::FrameLimit);
        }
        self.decoder.push(bytes);
        let mut outputs = Vec::new();
        while let Some(frame) = self.decoder.next_frame()? {
            if self.phase == ProtocolPhase::Ready
                && (frame.version != CURRENT_VERSION
                    || frame
                        .encode()
                        .map_err(|_| DriverProtocolError::Encoding)?
                        .len()
                        > self.negotiated_frame_limit)
            {
                return Err(DriverProtocolError::Version);
            }
            outputs.extend(self.process_frame(&frame)?);
        }
        Ok(outputs)
    }

    /// Emit the current serial configuration to an attached daemon.
    pub fn serial_config(
        &self,
        baud: u32,
        data_bits: u8,
        parity: u8,
        stop_bits: u8,
        read_timeout_ms: u32,
        write_timeout_ms: u32,
    ) -> Result<Option<ProtocolOutput>, DriverProtocolError> {
        if !self.attached {
            return Ok(None);
        }
        let mut frame = Frame::new(MessageKind::SerialConfig, ENDPOINT_ID, 0);
        frame.fields.insert_u32(serial_field::BAUD, baud)?;
        frame
            .fields
            .insert_u16(serial_field::DATA_BITS, u16::from(data_bits))?;
        frame
            .fields
            .insert_u16(serial_field::PARITY, u16::from(parity))?;
        frame
            .fields
            .insert_u16(serial_field::STOP_BITS, u16::from(stop_bits))?;
        frame.fields.insert_u16(serial_field::FLOW_CONTROL, 0)?;
        frame
            .fields
            .insert_u32(serial_field::READ_TIMEOUT_MS, read_timeout_ms)?;
        frame
            .fields
            .insert_u32(serial_field::WRITE_TIMEOUT_MS, write_timeout_ms)?;
        encode_frame(&frame, self.negotiated_frame_limit).map(Some)
    }

    /// Emit current DTR/RTS/break state to an attached daemon.
    pub fn modem_control(&self, mask: u32) -> Result<Option<ProtocolOutput>, DriverProtocolError> {
        if !self.attached {
            return Ok(None);
        }
        let mut frame = Frame::new(MessageKind::ModemControl, ENDPOINT_ID, 0);
        frame.fields.insert_u32(control_field::MASK, mask)?;
        encode_frame(&frame, self.negotiated_frame_limit).map(Some)
    }

    /// Emit a purge request to an attached daemon.
    pub fn purge(&self, mask: u32) -> Result<Option<ProtocolOutput>, DriverProtocolError> {
        if !self.attached {
            return Ok(None);
        }
        let mut frame = Frame::new(MessageKind::Purge, ENDPOINT_ID, 0);
        frame.fields.insert_u32(control_field::MASK, mask)?;
        encode_frame(&frame, self.negotiated_frame_limit).map(Some)
    }

    fn process_frame(&mut self, frame: &Frame) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        if frame.kind != MessageKind::Hello && self.phase != ProtocolPhase::Ready {
            return Err(DriverProtocolError::NotReady);
        }
        match frame.kind {
            MessageKind::Hello => self.process_hello(frame),
            MessageKind::Discover => self.process_discover(frame),
            MessageKind::Attach => self.process_attach(frame),
            MessageKind::Detach => {
                Self::require_endpoint(frame)?;
                self.attached = false;
                Ok(Vec::new())
            }
            MessageKind::Data => self.process_data(frame),
            MessageKind::WindowUpdate => self.process_window_update(frame),
            MessageKind::ModemStatus => self.process_modem_status(frame),
            MessageKind::Health => self.process_health(frame),
            _ => self.error_response(frame, 1, "message is not valid daemon input"),
        }
    }

    fn process_hello(&mut self, frame: &Frame) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        if frame.endpoint_id != 0 {
            return Err(DriverProtocolError::WrongEndpoint);
        }
        let peer_min = ProtocolVersion {
            major: frame.fields.require_u16(hello_field::MIN_MAJOR)?,
            minor: frame.fields.require_u16(hello_field::MIN_MINOR)?,
        };
        let peer_max = ProtocolVersion {
            major: frame.fields.require_u16(hello_field::MAX_MAJOR)?,
            minor: frame.fields.require_u16(hello_field::MAX_MINOR)?,
        };
        let Some(version) =
            ProtocolVersion::negotiate(CURRENT_VERSION, CURRENT_VERSION, peer_min, peer_max)
        else {
            return Err(DriverProtocolError::Version);
        };
        let features = frame.fields.require_u64(hello_field::FEATURES)?;
        if features & REQUIRED_FEATURES != REQUIRED_FEATURES {
            return Err(DriverProtocolError::Features);
        }
        let peer_frame_limit =
            usize::try_from(frame.fields.require_u32(hello_field::MAX_FRAME_BYTES)?)
                .map_err(|_| DriverProtocolError::FrameLimit)?;
        if peer_frame_limit < 64 {
            return Err(DriverProtocolError::FrameLimit);
        }
        let receive_window = usize::try_from(
            frame
                .fields
                .require_u32(hello_field::RECEIVE_WINDOW_BYTES)?,
        )
        .map_err(|_| DriverProtocolError::ReceiveWindow)?;
        if receive_window == 0 {
            return Err(DriverProtocolError::ReceiveWindow);
        }
        self.phase = ProtocolPhase::Ready;
        self.attached = false;
        self.negotiated_frame_limit = peer_frame_limit.min(ABSOLUTE_MAX_FRAME_LEN);
        self.daemon_receive_limit = receive_window.min(DEFAULT_BUFFER_CAPACITY);
        self.daemon_receive_credit = self.daemon_receive_limit;
        self.driver_receive_credit = DEFAULT_BUFFER_CAPACITY;
        self.outbound_sequence = 0;
        self.inbound_sequence = 0;

        let mut response = response_frame(MessageKind::HelloAck, frame, true);
        response.version = version;
        response
            .fields
            .insert_u16(hello_field::MIN_MAJOR, version.major)?;
        response
            .fields
            .insert_u16(hello_field::MIN_MINOR, version.minor)?;
        response
            .fields
            .insert_u16(hello_field::MAX_MAJOR, version.major)?;
        response
            .fields
            .insert_u16(hello_field::MAX_MINOR, version.minor)?;
        response.fields.insert_u32(
            hello_field::MAX_FRAME_BYTES,
            u32::try_from(self.negotiated_frame_limit)
                .map_err(|_| DriverProtocolError::FrameLimit)?,
        )?;
        response.fields.insert_u32(
            hello_field::RECEIVE_WINDOW_BYTES,
            u32::try_from(DEFAULT_BUFFER_CAPACITY)
                .map_err(|_| DriverProtocolError::ReceiveWindow)?,
        )?;
        response
            .fields
            .insert_u64(hello_field::FEATURES, REQUIRED_FEATURES)?;
        response
            .fields
            .insert_string(hello_field::IDENTITY, DRIVER_IDENTITY)?;
        Ok(vec![encode_frame(&response, self.negotiated_frame_limit)?])
    }

    fn process_discover(&self, frame: &Frame) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        if frame.endpoint_id != 0 {
            return Err(DriverProtocolError::WrongEndpoint);
        }
        let mut endpoint = response_frame(MessageKind::Endpoint, frame, false);
        endpoint.endpoint_id = ENDPOINT_ID;
        endpoint
            .fields
            .insert_u16(endpoint_field::KIND, self.endpoint.kind)?;
        endpoint
            .fields
            .insert_string(endpoint_field::STABLE_ID, &self.endpoint.stable_id)?;
        endpoint
            .fields
            .insert_string(endpoint_field::DISPLAY_NAME, &self.endpoint.display_name)?;
        endpoint.fields.insert_u16(endpoint_field::ENABLED, 1)?;
        let complete = response_frame(MessageKind::DiscoverComplete, frame, true);
        Ok(vec![
            encode_frame(&endpoint, self.negotiated_frame_limit)?,
            encode_frame(&complete, self.negotiated_frame_limit)?,
        ])
    }

    fn process_attach(
        &mut self,
        frame: &Frame,
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        Self::require_endpoint(frame)?;
        let requested_session = frame.fields.require_u64(session_field::SESSION_ID)?;
        if requested_session != 0
            && self.application_open
            && requested_session != self.application_session
        {
            return Err(DriverProtocolError::Session);
        }
        let credit = usize::try_from(frame.fields.require_u32(session_field::RECEIVE_CREDIT)?)
            .map_err(|_| DriverProtocolError::Credit)?;
        if credit == 0 {
            return Err(DriverProtocolError::Credit);
        }
        self.daemon_receive_limit = credit.min(DEFAULT_BUFFER_CAPACITY);
        self.daemon_receive_credit = self.daemon_receive_limit;
        self.driver_receive_credit = DEFAULT_BUFFER_CAPACITY;
        self.attached = true;
        self.outbound_sequence = 0;
        self.inbound_sequence = 0;

        let mut response = response_frame(MessageKind::AttachAck, frame, true);
        response
            .fields
            .insert_u64(session_field::SESSION_ID, self.application_session)?;
        response.fields.insert_u32(
            session_field::RECEIVE_CREDIT,
            u32::try_from(DEFAULT_BUFFER_CAPACITY).map_err(|_| DriverProtocolError::Credit)?,
        )?;
        let mut output = vec![encode_frame(&response, self.negotiated_frame_limit)?];
        if self.application_open {
            output.push(self.application_lifecycle(MessageKind::ApplicationOpen)?);
        }
        Ok(output)
    }

    fn process_data(&mut self, frame: &Frame) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        self.require_attached()?;
        Self::require_endpoint(frame)?;
        if !self.application_open {
            return Err(DriverProtocolError::Session);
        }
        let sequence = frame.fields.require_u64(data_field::SEQUENCE)?;
        if sequence <= self.inbound_sequence {
            return Err(DriverProtocolError::Sequence);
        }
        let bytes = frame
            .fields
            .get(data_field::BYTES)
            .ok_or(ProtocolError::MissingField(data_field::BYTES))?;
        if bytes.len() > self.driver_receive_credit {
            return Err(DriverProtocolError::Credit);
        }
        self.inbound_sequence = sequence;
        self.driver_receive_credit -= bytes.len();
        Ok(vec![ProtocolOutput::ToApplication(bytes.to_vec())])
    }

    fn process_window_update(
        &mut self,
        frame: &Frame,
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        self.require_attached()?;
        Self::require_endpoint(frame)?;
        let credit = usize::try_from(frame.fields.require_u32(data_field::CREDIT)?)
            .map_err(|_| DriverProtocolError::Credit)?;
        if credit == 0 {
            return Err(DriverProtocolError::Credit);
        }
        self.daemon_receive_credit = self
            .daemon_receive_credit
            .saturating_add(credit)
            .min(self.daemon_receive_limit);
        Ok(Vec::new())
    }

    fn process_modem_status(
        &self,
        frame: &Frame,
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        self.require_attached()?;
        Self::require_endpoint(frame)?;
        Ok(vec![ProtocolOutput::ModemStatus(
            frame.fields.require_u32(control_field::MASK)?,
        )])
    }

    fn process_health(&self, frame: &Frame) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        if frame.endpoint_id != 0 && frame.endpoint_id != ENDPOINT_ID {
            return Err(DriverProtocolError::WrongEndpoint);
        }
        let mut response = response_frame(MessageKind::HealthAck, frame, true);
        response
            .fields
            .insert_u32(control_field::MASK, u32::from(self.attached))?;
        response.fields.insert_u64(
            control_field::SEQUENCE_OR_OPERATION_ID,
            self.application_session,
        )?;
        response.fields.insert_string(control_field::DETAIL, "ok")?;
        Ok(vec![encode_frame(&response, self.negotiated_frame_limit)?])
    }

    fn error_response(
        &self,
        frame: &Frame,
        code: u32,
        detail: &str,
    ) -> Result<Vec<ProtocolOutput>, DriverProtocolError> {
        let mut response = response_frame(MessageKind::Error, frame, true);
        response
            .fields
            .insert_u32(control_field::KIND_OR_ERROR_CODE, code)?;
        response
            .fields
            .insert_string(control_field::DETAIL, detail)?;
        Ok(vec![encode_frame(&response, self.negotiated_frame_limit)?])
    }

    fn application_lifecycle(
        &self,
        kind: MessageKind,
    ) -> Result<ProtocolOutput, DriverProtocolError> {
        let mut frame = Frame::new(kind, ENDPOINT_ID, 0);
        frame.fields.insert_u64(
            control_field::SEQUENCE_OR_OPERATION_ID,
            self.application_session,
        )?;
        encode_frame(&frame, self.negotiated_frame_limit)
    }

    const fn require_endpoint(frame: &Frame) -> Result<(), DriverProtocolError> {
        if frame.endpoint_id == ENDPOINT_ID {
            Ok(())
        } else {
            Err(DriverProtocolError::WrongEndpoint)
        }
    }

    fn require_attached(&self) -> Result<(), DriverProtocolError> {
        if self.phase != ProtocolPhase::Ready {
            Err(DriverProtocolError::NotReady)
        } else if !self.attached {
            Err(DriverProtocolError::NotAttached)
        } else {
            Ok(())
        }
    }
}

impl Default for DriverProtocol {
    fn default() -> Self {
        Self::new()
    }
}

fn response_frame(kind: MessageKind, request: &Frame, final_response: bool) -> Frame {
    let mut response = Frame::new(kind, request.endpoint_id, request.request_id);
    response.flags = if final_response {
        FrameFlags::RESPONSE | FrameFlags::FINAL
    } else {
        FrameFlags::RESPONSE
    };
    response
}

fn encode_frame(frame: &Frame, frame_limit: usize) -> Result<ProtocolOutput, DriverProtocolError> {
    let bytes = frame.encode().map_err(|_| DriverProtocolError::Encoding)?;
    if bytes.len() > frame_limit {
        return Err(DriverProtocolError::FrameLimit);
    }
    Ok(ProtocolOutput::ToDaemon(bytes))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cathub_virtual_serial::daemon::{DaemonEvent, DaemonProtocol};

    fn hello(request_id: u64) -> Frame {
        let mut frame = Frame::new(MessageKind::Hello, 0, request_id);
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
            .expect("frame limit");
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
            .insert_string(hello_field::IDENTITY, "test-daemon")
            .expect("identity");
        frame
    }

    fn attach(request_id: u64) -> Frame {
        let mut frame = Frame::new(MessageKind::Attach, ENDPOINT_ID, request_id);
        frame
            .fields
            .insert_u64(session_field::SESSION_ID, 0)
            .expect("session");
        frame
            .fields
            .insert_u32(session_field::RECEIVE_CREDIT, 65_536)
            .expect("credit");
        frame
    }

    fn ingest_frame(protocol: &mut DriverProtocol, frame: &Frame) -> Vec<ProtocolOutput> {
        protocol
            .ingest_daemon(&frame.encode().expect("encode"))
            .expect("ingest")
    }

    fn output_frame(output: &ProtocolOutput) -> Frame {
        let ProtocolOutput::ToDaemon(bytes) = output else {
            panic!("expected daemon output");
        };
        Frame::decode(bytes).expect("decode")
    }

    fn daemon_events(daemon: &mut DaemonProtocol, outputs: &[ProtocolOutput]) -> Vec<DaemonEvent> {
        let mut events = Vec::new();
        for output in outputs {
            let ProtocolOutput::ToDaemon(bytes) = output else {
                continue;
            };
            events.extend(daemon.ingest(bytes).expect("daemon ingest"));
        }
        events
    }

    #[test]
    fn negotiates_discovers_and_attaches() {
        let mut protocol = DriverProtocol::new();
        let hello_output = ingest_frame(&mut protocol, &hello(11));
        let hello_ack = output_frame(&hello_output[0]);
        assert_eq!(hello_ack.kind, MessageKind::HelloAck);
        assert_eq!(hello_ack.request_id, 11);
        assert!(hello_ack.flags.contains(FrameFlags::FINAL));

        let discover = Frame::new(MessageKind::Discover, 0, 12);
        let discovery = ingest_frame(&mut protocol, &discover);
        assert_eq!(output_frame(&discovery[0]).kind, MessageKind::Endpoint);
        assert_eq!(
            output_frame(&discovery[1]).kind,
            MessageKind::DiscoverComplete
        );

        protocol.application_opened(7).expect("open");
        let attached = ingest_frame(&mut protocol, &attach(13));
        assert_eq!(output_frame(&attached[0]).kind, MessageKind::AttachAck);
        assert_eq!(
            output_frame(&attached[1]).kind,
            MessageKind::ApplicationOpen
        );
    }

    #[test]
    fn discovery_reports_provisioned_endpoint_identity() {
        let mut protocol = DriverProtocol::for_endpoint(EndpointMetadata {
            kind: 2,
            stable_id: "n1mm-winkeyer".to_owned(),
            display_name: "CatHub N1MM WinKeyer Port".to_owned(),
        });
        ingest_frame(&mut protocol, &hello(1));
        let discovery = ingest_frame(&mut protocol, &Frame::new(MessageKind::Discover, 0, 2));
        let endpoint = output_frame(&discovery[0]);
        assert_eq!(endpoint.fields.require_u16(endpoint_field::KIND), Ok(2));
        assert_eq!(
            endpoint.fields.require_string(endpoint_field::STABLE_ID),
            Ok("n1mm-winkeyer")
        );
        assert_eq!(
            endpoint.fields.require_string(endpoint_field::DISPLAY_NAME),
            Ok("CatHub N1MM WinKeyer Port")
        );
    }

    #[test]
    fn stream_decoder_handles_split_hello() {
        let mut protocol = DriverProtocol::new();
        let encoded = hello(1).encode().expect("encode");
        assert!(
            protocol
                .ingest_daemon(&encoded[..17])
                .expect("part")
                .is_empty()
        );
        let output = protocol.ingest_daemon(&encoded[17..]).expect("rest");
        assert_eq!(output_frame(&output[0]).kind, MessageKind::HelloAck);
    }

    #[test]
    fn moves_data_in_both_directions_after_attach() {
        let mut protocol = DriverProtocol::new();
        ingest_frame(&mut protocol, &hello(1));
        protocol.application_opened(4).expect("open");
        ingest_frame(&mut protocol, &attach(2));

        let to_daemon = protocol.application_data(b"FA;").expect("application data");
        let data = output_frame(&to_daemon);
        assert_eq!(data.kind, MessageKind::Data);
        assert_eq!(data.fields.get(data_field::BYTES), Some(b"FA;".as_slice()));

        let mut from_daemon = Frame::new(MessageKind::Data, ENDPOINT_ID, 0);
        from_daemon
            .fields
            .insert_u64(data_field::SEQUENCE, 1)
            .expect("sequence");
        from_daemon
            .fields
            .insert(data_field::BYTES, b"FA00007100000;")
            .expect("bytes");
        assert_eq!(
            ingest_frame(&mut protocol, &from_daemon),
            vec![ProtocolOutput::ToApplication(b"FA00007100000;".to_vec())]
        );
        let update = protocol
            .application_bytes_released(15)
            .expect("release")
            .expect("update");
        assert_eq!(output_frame(&update).kind, MessageKind::WindowUpdate);
    }

    #[test]
    fn rejects_data_before_attach_and_duplicate_sequence() {
        let mut protocol = DriverProtocol::new();
        ingest_frame(&mut protocol, &hello(1));
        protocol.application_opened(1).expect("open");
        assert_eq!(
            protocol.application_data(b"x"),
            Err(DriverProtocolError::NotAttached)
        );
        ingest_frame(&mut protocol, &attach(2));
        let mut data = Frame::new(MessageKind::Data, ENDPOINT_ID, 0);
        data.fields
            .insert_u64(data_field::SEQUENCE, 1)
            .expect("sequence");
        data.fields.insert(data_field::BYTES, b"x").expect("bytes");
        ingest_frame(&mut protocol, &data);
        assert_eq!(
            protocol.ingest_daemon(&data.encode().expect("encode")),
            Err(DriverProtocolError::Sequence)
        );
    }

    #[test]
    fn interoperates_with_the_cathub_daemon_state_machine() {
        let mut driver = DriverProtocol::new();
        let mut daemon = DaemonProtocol::new();

        let outputs = driver
            .ingest_daemon(&DaemonProtocol::hello(1).expect("hello"))
            .expect("driver hello");
        assert_eq!(
            daemon_events(&mut daemon, &outputs),
            vec![DaemonEvent::Negotiated]
        );

        let outputs = driver
            .ingest_daemon(&daemon.discover(2).expect("discover"))
            .expect("driver discovery");
        let events = daemon_events(&mut daemon, &outputs);
        assert!(events.iter().any(|event| matches!(
            event,
            DaemonEvent::Endpoint(endpoint)
                if endpoint.stable_id == "cathub-default" && endpoint.kind == 1
        )));
        assert!(events.contains(&DaemonEvent::DiscoveryComplete));

        driver.application_opened(41).expect("application open");
        let outputs = driver
            .ingest_daemon(&daemon.attach(ENDPOINT_ID, 3).expect("attach"))
            .expect("driver attach");
        assert_eq!(
            daemon_events(&mut daemon, &outputs),
            vec![
                DaemonEvent::Attached {
                    endpoint_id: ENDPOINT_ID,
                    session_id: 41,
                },
                DaemonEvent::ApplicationOpen(41),
            ]
        );

        let to_daemon = driver.application_data(b"ID;").expect("application data");
        assert_eq!(
            daemon_events(&mut daemon, &[to_daemon]),
            vec![DaemonEvent::Data(b"ID;".to_vec())]
        );
        let credit = daemon
            .release_received(3)
            .expect("release")
            .expect("credit frame");
        assert!(
            driver
                .ingest_daemon(&credit)
                .expect("driver credit")
                .is_empty()
        );

        let to_application = daemon.data(b"ID021;").expect("daemon data");
        assert_eq!(
            driver.ingest_daemon(&to_application).expect("driver data"),
            vec![ProtocolOutput::ToApplication(b"ID021;".to_vec())]
        );
        let update = driver
            .application_bytes_released(6)
            .expect("application release")
            .expect("window update");
        assert!(daemon_events(&mut daemon, &[update]).is_empty());
    }
}
