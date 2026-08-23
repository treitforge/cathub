//! Safe serial-port state and the Windows serial IOCTL wire layout.

/// Windows device type used by serial-port IOCTLs.
const FILE_DEVICE_SERIAL_PORT: u32 = 0x1b;

const fn serial_ioctl(function: u32) -> u32 {
    (FILE_DEVICE_SERIAL_PORT << 16) | (function << 2)
}

/// Serial IOCTL function codes accepted by the public COM handle.
pub mod ioctl {
    use super::serial_ioctl;

    pub const SET_BAUD_RATE: u32 = serial_ioctl(1);
    pub const SET_QUEUE_SIZE: u32 = serial_ioctl(2);
    pub const SET_LINE_CONTROL: u32 = serial_ioctl(3);
    pub const SET_BREAK_ON: u32 = serial_ioctl(4);
    pub const SET_BREAK_OFF: u32 = serial_ioctl(5);
    pub const IMMEDIATE_CHAR: u32 = serial_ioctl(6);
    pub const SET_TIMEOUTS: u32 = serial_ioctl(7);
    pub const GET_TIMEOUTS: u32 = serial_ioctl(8);
    pub const SET_DTR: u32 = serial_ioctl(9);
    pub const CLR_DTR: u32 = serial_ioctl(10);
    pub const RESET_DEVICE: u32 = serial_ioctl(11);
    pub const SET_RTS: u32 = serial_ioctl(12);
    pub const CLR_RTS: u32 = serial_ioctl(13);
    pub const SET_XOFF: u32 = serial_ioctl(14);
    pub const SET_XON: u32 = serial_ioctl(15);
    pub const GET_WAIT_MASK: u32 = serial_ioctl(16);
    pub const SET_WAIT_MASK: u32 = serial_ioctl(17);
    pub const WAIT_ON_MASK: u32 = serial_ioctl(18);
    pub const PURGE: u32 = serial_ioctl(19);
    pub const GET_BAUD_RATE: u32 = serial_ioctl(20);
    pub const GET_LINE_CONTROL: u32 = serial_ioctl(21);
    pub const GET_CHARS: u32 = serial_ioctl(22);
    pub const SET_CHARS: u32 = serial_ioctl(23);
    pub const GET_HANDFLOW: u32 = serial_ioctl(24);
    pub const SET_HANDFLOW: u32 = serial_ioctl(25);
    pub const GET_MODEM_STATUS: u32 = serial_ioctl(26);
    pub const GET_COMM_STATUS: u32 = serial_ioctl(27);
    pub const GET_PROPERTIES: u32 = serial_ioctl(29);
    pub const GET_DTR_RTS: u32 = serial_ioctl(30);
    pub const GET_MODEM_CONTROL: u32 = serial_ioctl(37);
    pub const SET_MODEM_CONTROL: u32 = serial_ioctl(38);
    pub const SET_FIFO_CONTROL: u32 = serial_ioctl(39);
}

/// Event bits used by `WaitCommEvent`.
pub mod event {
    pub const RX_CHAR: u32 = 0x0001;
    pub const RX_FLAG: u32 = 0x0002;
    pub const TX_EMPTY: u32 = 0x0004;
    pub const CTS: u32 = 0x0008;
    pub const DSR: u32 = 0x0010;
    pub const RLSD: u32 = 0x0020;
    pub const BREAK: u32 = 0x0040;
    pub const ERR: u32 = 0x0080;
    pub const RING: u32 = 0x0100;
    pub const RX_80_FULL: u32 = 0x0400;

    pub const SUPPORTED: u32 =
        RX_CHAR | RX_FLAG | TX_EMPTY | CTS | DSR | RLSD | BREAK | ERR | RING | RX_80_FULL;
}

/// Queue actions accepted by `PurgeComm`.
pub mod purge {
    pub const TX_ABORT: u32 = 0x0000_0001;
    pub const RX_ABORT: u32 = 0x0000_0002;
    pub const TX_CLEAR: u32 = 0x0000_0004;
    pub const RX_CLEAR: u32 = 0x0000_0008;
    pub const ALL: u32 = TX_ABORT | RX_ABORT | TX_CLEAR | RX_CLEAR;
}

/// Modem line bits returned by the serial IOCTLs.
pub mod modem {
    pub const DTR: u32 = 0x0000_0001;
    pub const RTS: u32 = 0x0000_0002;
    pub const CTS: u32 = 0x0000_0010;
    pub const DSR: u32 = 0x0000_0020;
    pub const RI: u32 = 0x0000_0040;
    pub const DCD: u32 = 0x0000_0080;
    pub const OUTPUT: u32 = DTR | RTS;
    pub const INPUT: u32 = CTS | DSR | RI | DCD;
}

/// Baud-rate input/output buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SerialBaudRate {
    pub baud_rate: u32,
}

/// Data format input/output buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SerialLineControl {
    pub stop_bits: u8,
    pub parity: u8,
    pub word_length: u8,
}

/// Timeout input/output buffer used by the Windows serial API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SerialTimeouts {
    pub read_interval_timeout: u32,
    pub read_total_timeout_multiplier: u32,
    pub read_total_timeout_constant: u32,
    pub write_total_timeout_multiplier: u32,
    pub write_total_timeout_constant: u32,
}

/// Requested driver queue sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SerialQueueSize {
    pub input_size: u32,
    pub output_size: u32,
}

/// Special serial characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SerialChars {
    pub eof: u8,
    pub error: u8,
    pub break_char: u8,
    pub event: u8,
    pub xon: u8,
    pub xoff: u8,
}

impl Default for SerialChars {
    fn default() -> Self {
        Self {
            eof: 0,
            error: 0,
            break_char: 0,
            event: 0,
            xon: 0x11,
            xoff: 0x13,
        }
    }
}

/// Flow-control state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SerialHandflow {
    pub control_handshake: u32,
    pub flow_replace: u32,
    pub xon_limit: i32,
    pub xoff_limit: i32,
}

/// Queue and error status returned by `ClearCommError`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SerialStatus {
    pub errors: u32,
    pub hold_reasons: u32,
    pub input_queue_bytes: u32,
    pub output_queue_bytes: u32,
    pub eof_received: u8,
    pub waiting_for_immediate: u8,
}

/// Capabilities returned by `GetCommProperties`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SerialCommProperties {
    pub packet_length: u16,
    pub packet_version: u16,
    pub service_mask: u32,
    pub reserved: u32,
    pub max_output_queue: u32,
    pub max_input_queue: u32,
    pub max_baud: u32,
    pub provider_subtype: u32,
    pub provider_capabilities: u32,
    pub settable_parameters: u32,
    pub settable_baud: u32,
    pub settable_data: u16,
    pub settable_stop_parity: u16,
    pub current_output_queue: u32,
    pub current_input_queue: u32,
    pub provider_specific_1: u32,
    pub provider_specific_2: u32,
    pub provider_char: u16,
}

/// Rejected serial configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialStateError {
    BaudRate,
    LineControl,
    Timeouts,
    QueueSize,
    Chars,
    Handflow,
    WaitMask,
    PurgeMask,
}

/// Mutable state associated with one application COM port.
#[derive(Debug, Clone)]
pub struct SerialState {
    baud_rate: u32,
    line_control: SerialLineControl,
    timeouts: SerialTimeouts,
    chars: SerialChars,
    handflow: SerialHandflow,
    input_queue_limit: u32,
    output_queue_limit: u32,
    modem_output: u32,
    modem_input: u32,
    break_active: bool,
    wait_mask: u32,
    pending_events: u32,
    errors: u32,
    fifo_control: u32,
}

impl SerialState {
    /// Create a conventional 9600-8-N-1 virtual port state.
    #[must_use]
    pub const fn new(queue_limit: u32) -> Self {
        Self {
            baud_rate: 9_600,
            line_control: SerialLineControl {
                stop_bits: 0,
                parity: 0,
                word_length: 8,
            },
            timeouts: SerialTimeouts {
                read_interval_timeout: 0,
                read_total_timeout_multiplier: 0,
                read_total_timeout_constant: 0,
                write_total_timeout_multiplier: 0,
                write_total_timeout_constant: 0,
            },
            chars: SerialChars {
                eof: 0,
                error: 0,
                break_char: 0,
                event: 0,
                xon: 0x11,
                xoff: 0x13,
            },
            handflow: SerialHandflow {
                control_handshake: 0,
                flow_replace: 0,
                xon_limit: 0,
                xoff_limit: 0,
            },
            input_queue_limit: queue_limit,
            output_queue_limit: queue_limit,
            modem_output: 0,
            modem_input: modem::CTS | modem::DSR | modem::DCD,
            break_active: false,
            wait_mask: 0,
            pending_events: 0,
            errors: 0,
            fifo_control: 0,
        }
    }

    #[must_use]
    pub const fn baud_rate(&self) -> SerialBaudRate {
        SerialBaudRate {
            baud_rate: self.baud_rate,
        }
    }

    pub const fn set_baud_rate(&mut self, value: SerialBaudRate) -> Result<(), SerialStateError> {
        if value.baud_rate == 0 {
            return Err(SerialStateError::BaudRate);
        }
        self.baud_rate = value.baud_rate;
        Ok(())
    }

    #[must_use]
    pub const fn line_control(&self) -> SerialLineControl {
        self.line_control
    }

    pub const fn set_line_control(
        &mut self,
        value: SerialLineControl,
    ) -> Result<(), SerialStateError> {
        let word_length_valid = matches!(value.word_length, 5..=8);
        let parity_valid = value.parity <= 4;
        let stop_bits_valid = match value.stop_bits {
            0 => true,
            1 => value.word_length == 5,
            2 => value.word_length != 5,
            _ => false,
        };
        if !(word_length_valid && parity_valid && stop_bits_valid) {
            return Err(SerialStateError::LineControl);
        }
        self.line_control = value;
        Ok(())
    }

    #[must_use]
    pub const fn timeouts(&self) -> SerialTimeouts {
        self.timeouts
    }

    pub const fn set_timeouts(&mut self, value: SerialTimeouts) -> Result<(), SerialStateError> {
        if value.read_interval_timeout == u32::MAX && value.read_total_timeout_constant == u32::MAX
        {
            return Err(SerialStateError::Timeouts);
        }
        self.timeouts = value;
        Ok(())
    }

    /// Compute the total timeout for a read that currently has no available bytes.
    ///
    /// `None` means wait indefinitely. `Some(0)` is the Windows special non-blocking
    /// configuration (`ReadIntervalTimeout == MAXDWORD` with both totals zero).
    #[must_use]
    pub fn empty_read_timeout_ms(&self, requested_bytes: usize) -> Option<u64> {
        let timeouts = self.timeouts;
        if timeouts.read_interval_timeout == u32::MAX
            && timeouts.read_total_timeout_multiplier == 0
            && timeouts.read_total_timeout_constant == 0
        {
            return Some(0);
        }
        // Windows and .NET SerialPort use this sentinel combination for a read that returns as
        // soon as one byte arrives, or after the constant when the input buffer remains empty.
        if timeouts.read_interval_timeout == u32::MAX
            && timeouts.read_total_timeout_multiplier == u32::MAX
            && timeouts.read_total_timeout_constant > 0
            && timeouts.read_total_timeout_constant < u32::MAX
        {
            return Some(u64::from(timeouts.read_total_timeout_constant));
        }
        let multiplier = u64::from(timeouts.read_total_timeout_multiplier);
        let requested = u64::try_from(requested_bytes).unwrap_or(u64::MAX);
        let total = multiplier
            .saturating_mul(requested)
            .saturating_add(u64::from(timeouts.read_total_timeout_constant));
        (total != 0).then_some(total)
    }

    pub const fn set_queue_size(&self, value: SerialQueueSize) -> Result<(), SerialStateError> {
        if value.input_size > self.input_queue_limit || value.output_size > self.output_queue_limit
        {
            return Err(SerialStateError::QueueSize);
        }
        Ok(())
    }

    #[must_use]
    pub const fn chars(&self) -> SerialChars {
        self.chars
    }

    pub const fn set_chars(&mut self, value: SerialChars) -> Result<(), SerialStateError> {
        if value.xon == value.xoff {
            return Err(SerialStateError::Chars);
        }
        self.chars = value;
        Ok(())
    }

    #[must_use]
    pub const fn handflow(&self) -> SerialHandflow {
        self.handflow
    }

    pub fn set_handflow(&mut self, value: SerialHandflow) -> Result<(), SerialStateError> {
        const CONTROL_INVALID: u32 = 0x7fff_ff84;
        const FLOW_INVALID: u32 = 0x7fff_ff20;
        let limits_valid = value.xon_limit >= 0
            && value.xoff_limit >= 0
            && u32::try_from(value.xon_limit).is_ok_and(|limit| limit <= self.input_queue_limit)
            && u32::try_from(value.xoff_limit).is_ok_and(|limit| limit <= self.input_queue_limit);
        if value.control_handshake & CONTROL_INVALID != 0
            || value.flow_replace & FLOW_INVALID != 0
            || !limits_valid
        {
            return Err(SerialStateError::Handflow);
        }
        self.handflow = value;
        Ok(())
    }

    #[must_use]
    pub const fn wait_mask(&self) -> u32 {
        self.wait_mask
    }

    pub const fn set_wait_mask(&mut self, value: u32) -> Result<(), SerialStateError> {
        if value & !event::SUPPORTED != 0 {
            return Err(SerialStateError::WaitMask);
        }
        self.wait_mask = value;
        self.pending_events &= value;
        Ok(())
    }

    /// Take events which currently satisfy the configured wait mask.
    pub const fn take_wait_events(&mut self) -> Option<u32> {
        let ready = self.pending_events & self.wait_mask;
        if ready == 0 {
            None
        } else {
            self.pending_events &= !ready;
            Some(ready)
        }
    }

    /// Record bytes delivered to the application receive queue.
    pub fn signal_receive(&mut self, bytes: &[u8], queue_bytes: usize) {
        if bytes.is_empty() {
            return;
        }
        self.pending_events |= event::RX_CHAR;
        if bytes.contains(&self.chars.event) {
            self.pending_events |= event::RX_FLAG;
        }
        let threshold = usize::try_from(self.input_queue_limit).unwrap_or(usize::MAX) * 4 / 5;
        if queue_bytes >= threshold {
            self.pending_events |= event::RX_80_FULL;
        }
    }

    pub const fn signal_transmit_empty(&mut self) {
        self.pending_events |= event::TX_EMPTY;
    }

    pub const fn set_break(&mut self, active: bool) {
        if self.break_active != active {
            self.break_active = active;
            self.pending_events |= event::BREAK;
        }
    }

    pub const fn set_dtr(&mut self, active: bool) {
        self.set_output_line(modem::DTR, active);
    }

    pub const fn set_rts(&mut self, active: bool) {
        self.set_output_line(modem::RTS, active);
    }

    #[must_use]
    pub const fn modem_output(&self) -> u32 {
        self.modem_output
    }

    /// Return DTR/RTS plus the private-protocol break bit.
    #[must_use]
    pub const fn modem_control_mask(&self) -> u32 {
        const BREAK_OUTPUT: u32 = 0x0000_0004;
        self.modem_output | if self.break_active { BREAK_OUTPUT } else { 0 }
    }

    pub const fn set_modem_output(&mut self, value: u32) {
        self.modem_output = value & modem::OUTPUT;
    }

    #[must_use]
    pub const fn modem_input(&self) -> u32 {
        self.modem_input
    }

    pub const fn set_modem_input(&mut self, value: u32) {
        let value = value & modem::INPUT;
        let changed = self.modem_input ^ value;
        self.modem_input = value;
        if changed & modem::CTS != 0 {
            self.pending_events |= event::CTS;
        }
        if changed & modem::DSR != 0 {
            self.pending_events |= event::DSR;
        }
        if changed & modem::DCD != 0 {
            self.pending_events |= event::RLSD;
        }
        if changed & modem::RI != 0 {
            self.pending_events |= event::RING;
        }
    }

    pub const fn set_fifo_control(&mut self, value: u32) {
        self.fifo_control = value;
    }

    pub const fn validate_purge(value: u32) -> Result<(), SerialStateError> {
        if value == 0 || value & !purge::ALL != 0 {
            Err(SerialStateError::PurgeMask)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn status(&mut self, input_bytes: usize, output_bytes: usize) -> SerialStatus {
        let status = SerialStatus {
            errors: self.errors,
            hold_reasons: 0,
            input_queue_bytes: u32::try_from(input_bytes).unwrap_or(u32::MAX),
            output_queue_bytes: u32::try_from(output_bytes).unwrap_or(u32::MAX),
            eof_received: 0,
            waiting_for_immediate: 0,
        };
        self.errors = 0;
        status
    }

    #[must_use]
    pub fn properties(&self) -> SerialCommProperties {
        const PROVIDER_CAPABILITIES: u32 = 0x01ff;
        const SETTABLE_PARAMETERS: u32 = 0x007f;
        const SETTABLE_BAUD: u32 = 0x1007_ffff;
        const SETTABLE_DATA: u16 = 0x000f;
        const SETTABLE_STOP_PARITY: u16 = 0x1f07;
        SerialCommProperties {
            packet_length: u16::try_from(std::mem::size_of::<SerialCommProperties>())
                .unwrap_or(u16::MAX),
            packet_version: 2,
            service_mask: 1,
            reserved: 0,
            max_output_queue: self.output_queue_limit,
            max_input_queue: self.input_queue_limit,
            max_baud: 0x1000_0000,
            provider_subtype: 1,
            provider_capabilities: PROVIDER_CAPABILITIES,
            settable_parameters: SETTABLE_PARAMETERS,
            settable_baud: SETTABLE_BAUD,
            settable_data: SETTABLE_DATA,
            settable_stop_parity: SETTABLE_STOP_PARITY,
            current_output_queue: self.output_queue_limit,
            current_input_queue: self.input_queue_limit,
            provider_specific_1: 0,
            provider_specific_2: 0,
            provider_char: 0,
        }
    }

    const fn set_output_line(&mut self, mask: u32, active: bool) {
        if active {
            self.modem_output |= mask;
        } else {
            self.modem_output &= !mask;
        }
    }
}

impl Default for SerialState {
    fn default() -> Self {
        Self::new(64 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_ioctl_values_match_ntddser() {
        assert_eq!(ioctl::SET_BAUD_RATE, 0x001b_0004);
        assert_eq!(ioctl::WAIT_ON_MASK, 0x001b_0048);
        assert_eq!(ioctl::GET_PROPERTIES, 0x001b_0074);
    }

    #[test]
    fn validates_line_control_combinations() {
        let mut state = SerialState::default();
        assert!(
            state
                .set_line_control(SerialLineControl {
                    stop_bits: 0,
                    parity: 0,
                    word_length: 8,
                })
                .is_ok()
        );
        assert_eq!(
            state.set_line_control(SerialLineControl {
                stop_bits: 1,
                parity: 0,
                word_length: 8,
            }),
            Err(SerialStateError::LineControl)
        );
        assert_eq!(
            state.set_line_control(SerialLineControl {
                stop_bits: 2,
                parity: 0,
                word_length: 5,
            }),
            Err(SerialStateError::LineControl)
        );
    }

    #[test]
    fn rejects_windows_unsupported_maximum_interval_and_constant() {
        let mut state = SerialState::default();
        assert_eq!(
            state.set_timeouts(SerialTimeouts {
                read_interval_timeout: u32::MAX,
                read_total_timeout_multiplier: u32::MAX,
                read_total_timeout_constant: u32::MAX,
                ..SerialTimeouts::default()
            }),
            Err(SerialStateError::Timeouts)
        );
        assert_eq!(
            state.set_timeouts(SerialTimeouts {
                read_interval_timeout: u32::MAX,
                read_total_timeout_multiplier: 0,
                read_total_timeout_constant: u32::MAX,
                ..SerialTimeouts::default()
            }),
            Err(SerialStateError::Timeouts)
        );
    }

    #[test]
    fn computes_empty_read_timeout_from_windows_totals() {
        let mut state = SerialState::default();
        assert_eq!(state.empty_read_timeout_ms(10), None);
        state
            .set_timeouts(SerialTimeouts {
                read_total_timeout_multiplier: 3,
                read_total_timeout_constant: 70,
                ..SerialTimeouts::default()
            })
            .expect("timeouts");
        assert_eq!(state.empty_read_timeout_ms(10), Some(100));

        state
            .set_timeouts(SerialTimeouts {
                read_interval_timeout: u32::MAX,
                ..SerialTimeouts::default()
            })
            .expect("immediate timeouts");
        assert_eq!(state.empty_read_timeout_ms(10), Some(0));

        state
            .set_timeouts(SerialTimeouts {
                read_interval_timeout: u32::MAX,
                read_total_timeout_multiplier: u32::MAX,
                read_total_timeout_constant: 100,
                ..SerialTimeouts::default()
            })
            .expect("first-byte timeout");
        assert_eq!(state.empty_read_timeout_ms(10), Some(100));
    }

    #[test]
    fn wait_events_are_masked_and_consumed() {
        let mut state = SerialState::default();
        state
            .set_wait_mask(event::RX_CHAR | event::CTS)
            .expect("mask");
        state.signal_receive(b"FA;", 3);
        assert_eq!(state.take_wait_events(), Some(event::RX_CHAR));
        assert_eq!(state.take_wait_events(), None);
        state.set_modem_input(modem::DSR | modem::DCD);
        assert_eq!(state.take_wait_events(), Some(event::CTS));
    }

    #[test]
    fn changing_wait_mask_discards_unrequested_events() {
        let mut state = SerialState::default();
        state.set_wait_mask(event::RX_CHAR).expect("mask");
        state.signal_receive(b"x", 1);
        state.set_wait_mask(event::CTS).expect("new mask");
        assert_eq!(state.take_wait_events(), None);
    }

    #[test]
    fn rejects_invalid_chars_and_handflow() {
        let mut state = SerialState::default();
        assert_eq!(
            state.set_chars(SerialChars {
                xon: 1,
                xoff: 1,
                ..SerialChars::default()
            }),
            Err(SerialStateError::Chars)
        );
        assert_eq!(
            state.set_handflow(SerialHandflow {
                xon_limit: -1,
                ..SerialHandflow::default()
            }),
            Err(SerialStateError::Handflow)
        );
    }

    #[test]
    fn comm_status_returns_queue_depth_and_clears_errors() {
        let mut state = SerialState::default();
        let status = state.status(17, 9);
        assert_eq!(status.input_queue_bytes, 17);
        assert_eq!(status.output_queue_bytes, 9);
        assert_eq!(state.status(0, 0).errors, 0);
    }
}
