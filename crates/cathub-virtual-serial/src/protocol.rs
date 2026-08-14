//! Versioned frame contract for the private CatHub device interface.
//!
//! Each frame starts with a fixed 40-byte header. Multi-byte integers use little-endian
//! byte order. Message payloads use type-length-value fields so a newer peer can add optional
//! fields without a major protocol change.

use std::collections::BTreeMap;

use thiserror::Error;

/// Four-byte marker at the start of each private transport frame.
pub const MAGIC: [u8; 4] = *b"CHVS";

/// Size of the version 1 frame header.
pub const HEADER_LEN: usize = 40;

const HEADER_LEN_WIRE: u16 = 40;

/// Largest frame accepted by the shared decoder before negotiation.
pub const ABSOLUTE_MAX_FRAME_LEN: usize = 1024 * 1024;

/// Current private transport protocol version.
pub const CURRENT_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

/// Feature bits exchanged in `Hello` and `HelloAck` field 7.
pub mod feature {
    /// The peer reports application open and close events.
    pub const APPLICATION_LIFECYCLE: u64 = 1 << 0;
    /// The peer reports serial format changes.
    pub const SERIAL_CONFIG: u64 = 1 << 1;
    /// The peer reports modem control and status changes.
    pub const MODEM_LINES: u64 = 1 << 2;
    /// The peer reports cancellation requests.
    pub const CANCELLATION: u64 = 1 << 3;
    /// The peer reports purge and queue reset requests.
    pub const PURGE: u64 = 1 << 4;
    /// The peer supports health requests.
    pub const HEALTH: u64 = 1 << 5;
    /// The device contains more than one endpoint.
    pub const MULTI_ENDPOINT: u64 = 1 << 6;
    /// The peer applies explicit receive credit to data frames.
    pub const RECEIVE_CREDIT: u64 = 1 << 7;
}

/// Field tags for `Hello` and `HelloAck` messages.
pub mod hello_field {
    /// Lowest supported protocol major version.
    pub const MIN_MAJOR: u16 = 1;
    /// Lowest supported protocol minor version.
    pub const MIN_MINOR: u16 = 2;
    /// Highest supported protocol major version.
    pub const MAX_MAJOR: u16 = 3;
    /// Highest supported protocol minor version.
    pub const MAX_MINOR: u16 = 4;
    /// Largest frame in bytes that the peer accepts.
    pub const MAX_FRAME_BYTES: u16 = 5;
    /// Initial receive credit in bytes.
    pub const RECEIVE_WINDOW_BYTES: u16 = 6;
    /// Supported feature bitmap.
    pub const FEATURES: u16 = 7;
    /// Daemon or driver identity string.
    pub const IDENTITY: u16 = 8;
}

/// Field tags for endpoint discovery messages.
pub mod endpoint_field {
    /// Endpoint kind. Value 1 is CAT and value 2 is WinKeyer.
    pub const KIND: u16 = 1;
    /// Stable endpoint identifier from CatHub configuration.
    pub const STABLE_ID: u16 = 2;
    /// Operator-facing endpoint name.
    pub const DISPLAY_NAME: u16 = 3;
    /// Enabled state as zero or one.
    pub const ENABLED: u16 = 4;
}

/// Field tags for attach and detach messages.
pub mod session_field {
    /// Application session sequence.
    pub const SESSION_ID: u16 = 1;
    /// Initial receive credit in bytes.
    pub const RECEIVE_CREDIT: u16 = 2;
    /// Attach options or detach reason.
    pub const OPTIONS_OR_REASON: u16 = 3;
    /// Optional diagnostic detail.
    pub const DETAIL: u16 = 4;
}

/// Field tags for data and flow-control messages.
pub mod data_field {
    /// Monotonic data sequence for one endpoint session.
    pub const SEQUENCE: u16 = 1;
    /// Opaque application bytes.
    pub const BYTES: u16 = 2;
    /// Additional receive credit in bytes.
    pub const CREDIT: u16 = 3;
}

/// Field tags for serial configuration messages.
pub mod serial_field {
    /// Baud rate.
    pub const BAUD: u16 = 1;
    /// Data bit count.
    pub const DATA_BITS: u16 = 2;
    /// Parity enum.
    pub const PARITY: u16 = 3;
    /// Stop bit enum.
    pub const STOP_BITS: u16 = 4;
    /// Flow-control enum.
    pub const FLOW_CONTROL: u16 = 5;
    /// Read timeout in milliseconds.
    pub const READ_TIMEOUT_MS: u16 = 6;
    /// Write timeout in milliseconds.
    pub const WRITE_TIMEOUT_MS: u16 = 7;
}

/// Field tags for control, lifecycle, purge, and error messages.
pub mod control_field {
    /// Modem line or purge bitmap.
    pub const MASK: u16 = 1;
    /// Application process ID.
    pub const PROCESS_ID: u16 = 2;
    /// Application open sequence or operation ID.
    pub const SEQUENCE_OR_OPERATION_ID: u16 = 3;
    /// Operation kind or error code.
    pub const KIND_OR_ERROR_CODE: u16 = 4;
    /// Optional diagnostic detail.
    pub const DETAIL: u16 = 5;
}

/// A private transport protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtocolVersion {
    /// Incompatible contract generation.
    pub major: u16,
    /// Backward-compatible feature generation.
    pub minor: u16,
}

impl ProtocolVersion {
    /// Select the highest mutually compatible version.
    #[must_use]
    pub fn negotiate(
        local_min: Self,
        local_max: Self,
        peer_min: Self,
        peer_max: Self,
    ) -> Option<Self> {
        if local_min.major != local_max.major
            || peer_min.major != peer_max.major
            || local_max.major != peer_max.major
        {
            return None;
        }

        let minimum = local_min.max(peer_min);
        let maximum = local_max.min(peer_max);
        (minimum <= maximum).then_some(maximum)
    }
}

/// Message identifiers in the private device contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MessageKind {
    /// Daemon version and capability offer.
    Hello = 1,
    /// Driver version and capability selection.
    HelloAck = 2,
    /// Request the configured endpoint set.
    Discover = 3,
    /// One endpoint description in a discovery response.
    Endpoint = 4,
    /// End of a discovery response.
    DiscoverComplete = 5,
    /// Attach the daemon to one endpoint.
    Attach = 6,
    /// Confirm an endpoint attachment.
    AttachAck = 7,
    /// Detach the daemon from one endpoint.
    Detach = 8,
    /// Bytes for one application session.
    Data = 9,
    /// Add receive credit for bounded flow control.
    WindowUpdate = 10,
    /// Report the application serial configuration.
    SerialConfig = 11,
    /// Report DTR, RTS, or break state.
    ModemControl = 12,
    /// Report CTS, DSR, DCD, or RI state.
    ModemStatus = 13,
    /// Report an application handle open.
    ApplicationOpen = 14,
    /// Report an application handle close.
    ApplicationClose = 15,
    /// Cancel an outstanding serial operation.
    Cancel = 16,
    /// Purge or reset serial queues.
    Purge = 17,
    /// Request transport health information.
    Health = 18,
    /// Return transport health information.
    HealthAck = 19,
    /// Report a rejected request or protocol fault.
    Error = 20,
}

impl TryFrom<u16> for MessageKind {
    type Error = ProtocolError;

    fn try_from(value: u16) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::HelloAck),
            3 => Ok(Self::Discover),
            4 => Ok(Self::Endpoint),
            5 => Ok(Self::DiscoverComplete),
            6 => Ok(Self::Attach),
            7 => Ok(Self::AttachAck),
            8 => Ok(Self::Detach),
            9 => Ok(Self::Data),
            10 => Ok(Self::WindowUpdate),
            11 => Ok(Self::SerialConfig),
            12 => Ok(Self::ModemControl),
            13 => Ok(Self::ModemStatus),
            14 => Ok(Self::ApplicationOpen),
            15 => Ok(Self::ApplicationClose),
            16 => Ok(Self::Cancel),
            17 => Ok(Self::Purge),
            18 => Ok(Self::Health),
            19 => Ok(Self::HealthAck),
            20 => Ok(Self::Error),
            other => Err(ProtocolError::UnknownMessageKind(other)),
        }
    }
}

/// Header flags for a private transport frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameFlags(u32);

impl FrameFlags {
    /// The frame is a response to the request with the same request ID.
    pub const RESPONSE: Self = Self(1 << 0);
    /// The frame completes a multi-frame response.
    pub const FINAL: Self = Self(1 << 1);
    /// The sender requires an explicit response.
    pub const ACK_REQUIRED: Self = Self(1 << 2);

    /// Construct flags from their wire bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the wire bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Test whether all supplied flags are set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for FrameFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// A decoded private transport frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Protocol version used for this frame.
    pub version: ProtocolVersion,
    /// Message operation.
    pub kind: MessageKind,
    /// Operation flags.
    pub flags: FrameFlags,
    /// Stable endpoint ID, or zero for device-wide operations.
    pub endpoint_id: u64,
    /// Request correlation ID, or zero for unsolicited events.
    pub request_id: u64,
    /// Type-length-value payload.
    pub fields: Fields,
}

impl Frame {
    /// Create a frame with the current protocol version.
    #[must_use]
    pub fn new(kind: MessageKind, endpoint_id: u64, request_id: u64) -> Self {
        Self {
            version: CURRENT_VERSION,
            kind,
            flags: FrameFlags::default(),
            endpoint_id,
            request_id,
            fields: Fields::default(),
        }
    }

    /// Encode this frame into the stable wire form.
    ///
    /// # Errors
    ///
    /// Returns an error if a field or frame exceeds the protocol size limits.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let payload = self.fields.encode()?;
        let total = HEADER_LEN
            .checked_add(payload.len())
            .ok_or(ProtocolError::FrameTooLarge(usize::MAX))?;
        if total > ABSOLUTE_MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(total));
        }

        let payload_len = u32::try_from(payload.len())
            .map_err(|_| ProtocolError::FrameTooLarge(payload.len()))?;
        let mut output = Vec::with_capacity(total);
        output.extend_from_slice(&MAGIC);
        output.extend_from_slice(&HEADER_LEN_WIRE.to_le_bytes());
        output.extend_from_slice(&(self.kind as u16).to_le_bytes());
        output.extend_from_slice(&self.version.major.to_le_bytes());
        output.extend_from_slice(&self.version.minor.to_le_bytes());
        output.extend_from_slice(&self.flags.bits().to_le_bytes());
        output.extend_from_slice(&payload_len.to_le_bytes());
        output.extend_from_slice(&self.endpoint_id.to_le_bytes());
        output.extend_from_slice(&self.request_id.to_le_bytes());
        output.extend_from_slice(&0u32.to_le_bytes());
        output.extend_from_slice(&payload);
        Ok(output)
    }

    /// Decode one complete frame.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid framing, unknown message types, or invalid fields.
    pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
        if input.len() < HEADER_LEN {
            return Err(ProtocolError::Incomplete {
                needed: HEADER_LEN,
                available: input.len(),
            });
        }
        if input.get(0..4) != Some(MAGIC.as_slice()) {
            return Err(ProtocolError::BadMagic);
        }

        let header_len = usize::from(read_u16(input, 4)?);
        if header_len != HEADER_LEN {
            return Err(ProtocolError::UnsupportedHeader(header_len));
        }
        let kind = MessageKind::try_from(read_u16(input, 6)?)?;
        let version = ProtocolVersion {
            major: read_u16(input, 8)?,
            minor: read_u16(input, 10)?,
        };
        let flags = FrameFlags::from_bits(read_u32(input, 12)?);
        let payload_len = usize::try_from(read_u32(input, 16)?)
            .map_err(|_| ProtocolError::FrameTooLarge(usize::MAX))?;
        let total = header_len
            .checked_add(payload_len)
            .ok_or(ProtocolError::FrameTooLarge(usize::MAX))?;
        if total > ABSOLUTE_MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(total));
        }
        if input.len() < total {
            return Err(ProtocolError::Incomplete {
                needed: total,
                available: input.len(),
            });
        }
        if input.len() != total {
            return Err(ProtocolError::TrailingBytes(input.len() - total));
        }

        Ok(Self {
            version,
            kind,
            flags,
            endpoint_id: read_u64(input, 20)?,
            request_id: read_u64(input, 28)?,
            fields: Fields::decode(input.get(header_len..total).unwrap_or_default())?,
        })
    }
}

/// Ordered type-length-value payload fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields(BTreeMap<u16, Vec<u8>>);

impl Fields {
    /// Insert a raw field.
    ///
    /// # Errors
    ///
    /// Returns an error when the reserved tag zero is used.
    pub fn insert(&mut self, tag: u16, value: impl Into<Vec<u8>>) -> Result<(), ProtocolError> {
        if tag == 0 {
            return Err(ProtocolError::ReservedFieldTag);
        }
        self.0.insert(tag, value.into());
        Ok(())
    }

    /// Insert an unsigned 16-bit value.
    ///
    /// # Errors
    ///
    /// Returns an error when the reserved tag zero is used.
    pub fn insert_u16(&mut self, tag: u16, value: u16) -> Result<(), ProtocolError> {
        self.insert(tag, value.to_le_bytes())
    }

    /// Insert an unsigned 32-bit value.
    ///
    /// # Errors
    ///
    /// Returns an error when the reserved tag zero is used.
    pub fn insert_u32(&mut self, tag: u16, value: u32) -> Result<(), ProtocolError> {
        self.insert(tag, value.to_le_bytes())
    }

    /// Insert an unsigned 64-bit value.
    ///
    /// # Errors
    ///
    /// Returns an error when the reserved tag zero is used.
    pub fn insert_u64(&mut self, tag: u16, value: u64) -> Result<(), ProtocolError> {
        self.insert(tag, value.to_le_bytes())
    }

    /// Insert a UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns an error when the reserved tag zero is used.
    pub fn insert_string(
        &mut self,
        tag: u16,
        value: impl Into<String>,
    ) -> Result<(), ProtocolError> {
        self.insert(tag, value.into().into_bytes())
    }

    /// Get a raw field.
    #[must_use]
    pub fn get(&self, tag: u16) -> Option<&[u8]> {
        self.0.get(&tag).map(Vec::as_slice)
    }

    /// Read an unsigned 16-bit field.
    ///
    /// # Errors
    ///
    /// Returns an error if the field is absent or has the wrong size.
    pub fn require_u16(&self, tag: u16) -> Result<u16, ProtocolError> {
        let value = self.require(tag)?;
        let bytes: [u8; 2] = value
            .try_into()
            .map_err(|_| ProtocolError::InvalidFieldSize { tag, expected: 2 })?;
        Ok(u16::from_le_bytes(bytes))
    }

    /// Read an unsigned 32-bit field.
    ///
    /// # Errors
    ///
    /// Returns an error if the field is absent or has the wrong size.
    pub fn require_u32(&self, tag: u16) -> Result<u32, ProtocolError> {
        let value = self.require(tag)?;
        let bytes: [u8; 4] = value
            .try_into()
            .map_err(|_| ProtocolError::InvalidFieldSize { tag, expected: 4 })?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// Read an unsigned 64-bit field.
    ///
    /// # Errors
    ///
    /// Returns an error if the field is absent or has the wrong size.
    pub fn require_u64(&self, tag: u16) -> Result<u64, ProtocolError> {
        let value = self.require(tag)?;
        let bytes: [u8; 8] = value
            .try_into()
            .map_err(|_| ProtocolError::InvalidFieldSize { tag, expected: 8 })?;
        Ok(u64::from_le_bytes(bytes))
    }

    /// Read a UTF-8 field.
    ///
    /// # Errors
    ///
    /// Returns an error if the field is absent or is not valid UTF-8.
    pub fn require_string(&self, tag: u16) -> Result<&str, ProtocolError> {
        std::str::from_utf8(self.require(tag)?).map_err(|_| ProtocolError::InvalidUtf8Field(tag))
    }

    /// Return the number of fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Test whether the payload has no fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn require(&self, tag: u16) -> Result<&[u8], ProtocolError> {
        self.get(tag).ok_or(ProtocolError::MissingField(tag))
    }

    fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = Vec::new();
        for (&tag, value) in &self.0 {
            let len = u32::try_from(value.len()).map_err(|_| ProtocolError::FieldTooLarge {
                tag,
                size: value.len(),
            })?;
            output.extend_from_slice(&tag.to_le_bytes());
            output.extend_from_slice(&len.to_le_bytes());
            output.extend_from_slice(value);
        }
        Ok(output)
    }

    fn decode(mut input: &[u8]) -> Result<Self, ProtocolError> {
        let mut fields = Self::default();
        while !input.is_empty() {
            if input.len() < 6 {
                return Err(ProtocolError::MalformedFields);
            }
            let tag = read_u16(input, 0)?;
            if tag == 0 {
                return Err(ProtocolError::ReservedFieldTag);
            }
            let len =
                usize::try_from(read_u32(input, 2)?).map_err(|_| ProtocolError::FieldTooLarge {
                    tag,
                    size: usize::MAX,
                })?;
            let end = 6usize
                .checked_add(len)
                .ok_or(ProtocolError::FieldTooLarge { tag, size: len })?;
            let value = input.get(6..end).ok_or(ProtocolError::MalformedFields)?;
            if fields.0.insert(tag, value.to_vec()).is_some() {
                return Err(ProtocolError::DuplicateField(tag));
            }
            input = input.get(end..).ok_or(ProtocolError::MalformedFields)?;
        }
        Ok(fields)
    }
}

/// Incremental decoder for a byte stream from the private device interface.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    /// Add bytes received from the device.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Decode the next complete frame when it is available.
    ///
    /// # Errors
    ///
    /// Returns an error when the buffered header or frame is invalid.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }
        if self.buffer.get(0..4) != Some(MAGIC.as_slice()) {
            return Err(ProtocolError::BadMagic);
        }
        let header_len = usize::from(read_u16(&self.buffer, 4)?);
        if header_len != HEADER_LEN {
            return Err(ProtocolError::UnsupportedHeader(header_len));
        }
        let payload_len = usize::try_from(read_u32(&self.buffer, 16)?)
            .map_err(|_| ProtocolError::FrameTooLarge(usize::MAX))?;
        let total = header_len
            .checked_add(payload_len)
            .ok_or(ProtocolError::FrameTooLarge(usize::MAX))?;
        if total > ABSOLUTE_MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(total));
        }
        if self.buffer.len() < total {
            return Ok(None);
        }
        let bytes: Vec<u8> = self.buffer.drain(..total).collect();
        Frame::decode(&bytes).map(Some)
    }

    /// Return the number of bytes that wait for a complete frame.
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }
}

/// Errors from private transport encoding or decoding.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    /// The input does not contain a full frame.
    #[error("incomplete frame: need {needed} bytes, have {available}")]
    Incomplete {
        /// Total bytes required.
        needed: usize,
        /// Bytes currently available.
        available: usize,
    },
    /// The frame marker is not `CHVS`.
    #[error("invalid private transport frame magic")]
    BadMagic,
    /// The fixed header size is not supported.
    #[error("unsupported private transport header length {0}")]
    UnsupportedHeader(usize),
    /// The message identifier is not defined.
    #[error("unknown private transport message kind {0}")]
    UnknownMessageKind(u16),
    /// The frame exceeds the hard decoder limit.
    #[error("private transport frame is too large: {0} bytes")]
    FrameTooLarge(usize),
    /// The caller supplied more than one frame.
    #[error("private transport frame has {0} trailing bytes")]
    TrailingBytes(usize),
    /// Field tag zero is reserved.
    #[error("private transport field tag zero is reserved")]
    ReservedFieldTag,
    /// A field exceeds the representable size.
    #[error("private transport field {tag} is too large: {size} bytes")]
    FieldTooLarge {
        /// Field tag.
        tag: u16,
        /// Field size.
        size: usize,
    },
    /// A field appears more than once.
    #[error("duplicate private transport field {0}")]
    DuplicateField(u16),
    /// The payload field sequence is truncated or invalid.
    #[error("malformed private transport fields")]
    MalformedFields,
    /// A required field is absent.
    #[error("missing private transport field {0}")]
    MissingField(u16),
    /// A fixed-size field has the wrong size.
    #[error("private transport field {tag} must contain {expected} bytes")]
    InvalidFieldSize {
        /// Field tag.
        tag: u16,
        /// Required size.
        expected: usize,
    },
    /// A text field is not valid UTF-8.
    #[error("private transport field {0} is not valid UTF-8")]
    InvalidUtf8Field(u16),
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16, ProtocolError> {
    let bytes: [u8; 2] = input
        .get(offset..offset.saturating_add(2))
        .ok_or(ProtocolError::MalformedFields)?
        .try_into()
        .map_err(|_| ProtocolError::MalformedFields)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32, ProtocolError> {
    let bytes: [u8; 4] = input
        .get(offset..offset.saturating_add(4))
        .ok_or(ProtocolError::MalformedFields)?
        .try_into()
        .map_err(|_| ProtocolError::MalformedFields)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize) -> Result<u64, ProtocolError> {
    let bytes: [u8; 8] = input
        .get(offset..offset.saturating_add(8))
        .ok_or(ProtocolError::MalformedFields)?
        .try_into()
        .map_err(|_| ProtocolError::MalformedFields)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_preserves_header_and_fields() {
        let mut frame = Frame::new(MessageKind::Hello, 42, 9);
        frame.flags = FrameFlags::ACK_REQUIRED;
        frame.fields.insert_u16(1, 1).expect("min major");
        frame.fields.insert_u16(2, 0).expect("min minor");
        frame
            .fields
            .insert_string(3, "cathub.exe")
            .expect("identity");

        let encoded = frame.encode().expect("encode");
        let decoded = Frame::decode(&encoded).expect("decode");

        assert_eq!(decoded, frame);
        assert_eq!(decoded.fields.require_u16(1).expect("major"), 1);
        assert_eq!(
            decoded.fields.require_string(3).expect("identity"),
            "cathub.exe"
        );
    }

    #[test]
    fn stream_decoder_handles_split_and_multiple_frames() {
        let first = Frame::new(MessageKind::Health, 0, 1)
            .encode()
            .expect("first");
        let second = Frame::new(MessageKind::Discover, 0, 2)
            .encode()
            .expect("second");
        let split = first.len() / 2;
        let mut decoder = FrameDecoder::default();

        decoder.push(&first[..split]);
        assert!(decoder.next_frame().expect("partial").is_none());
        decoder.push(&first[split..]);
        decoder.push(&second);

        assert_eq!(
            decoder.next_frame().expect("first").expect("frame").kind,
            MessageKind::Health
        );
        assert_eq!(
            decoder.next_frame().expect("second").expect("frame").kind,
            MessageKind::Discover
        );
        assert!(decoder.next_frame().expect("empty").is_none());
    }

    #[test]
    fn decoder_rejects_duplicate_fields() {
        let mut bytes = Frame::new(MessageKind::Health, 0, 1)
            .encode()
            .expect("frame");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&12u32.to_le_bytes());

        assert_eq!(Frame::decode(&bytes), Err(ProtocolError::DuplicateField(1)));
    }

    #[test]
    fn version_negotiation_requires_one_major_generation() {
        assert_eq!(
            ProtocolVersion::negotiate(
                ProtocolVersion { major: 1, minor: 0 },
                ProtocolVersion { major: 1, minor: 3 },
                ProtocolVersion { major: 1, minor: 1 },
                ProtocolVersion { major: 1, minor: 2 },
            ),
            Some(ProtocolVersion { major: 1, minor: 2 })
        );
        assert_eq!(
            ProtocolVersion::negotiate(
                ProtocolVersion { major: 1, minor: 0 },
                ProtocolVersion { major: 1, minor: 3 },
                ProtocolVersion { major: 2, minor: 0 },
                ProtocolVersion { major: 2, minor: 0 },
            ),
            None
        );
    }
}
