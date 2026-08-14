#![allow(unsafe_code, clippy::borrow_as_ptr)]

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Devices::Communication::{
    BuildCommDCBW, ClearCommBreak, ClearCommError, EscapeCommFunction, GetCommState,
    GetCommTimeouts, PurgeComm, SetCommBreak, SetCommMask, SetCommState, SetCommTimeouts,
    WaitCommEvent, CLRDTR, CLRRTS, COMMTIMEOUTS, COMSTAT, DCB, EV_RXCHAR, NOPARITY, ONESTOPBIT,
    PURGE_RXABORT, PURGE_RXCLEAR, SETDTR, SETRTS, TWOSTOPBITS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use super::{
    ApplicationProfile, CaseId, CaseResult, CaseStatus, ConformanceError, ConformanceReport,
    ProfileCase,
};

const IO_WAIT_MS: u32 = 2_000;
const TEST_DATA: &[u8] = b"CatHub serial conformance";

pub(super) fn run(
    profile: &ApplicationProfile,
    application_port: &str,
    peer_port: &str,
) -> Result<ConformanceReport, ConformanceError> {
    if application_port.eq_ignore_ascii_case(peer_port) {
        return Err(ConformanceError::InvalidResult(
            "application and peer ports must be different".to_string(),
        ));
    }

    let results = profile
        .cases
        .iter()
        .map(|case| run_case(*case, profile, application_port, peer_port))
        .collect();
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(ConformanceReport {
        schema_version: 1,
        generated_at_utc: format!("unix:{seconds}"),
        profile: profile.name.to_string(),
        application_port: application_port.to_string(),
        peer_port: peer_port.to_string(),
        operating_system: "Windows".to_string(),
        results,
    })
}

fn run_case(
    profile_case: ProfileCase,
    profile: &ApplicationProfile,
    application_port: &str,
    peer_port: &str,
) -> CaseResult {
    let result = match profile_case.id {
        CaseId::SynchronousIo => synchronous_io(application_port, peer_port),
        CaseId::OverlappedIo => overlapped_io(application_port, peer_port),
        CaseId::CancelPendingRead => cancel_pending_read(application_port),
        CaseId::ReadTimeout => read_timeout(application_port),
        CaseId::PurgeReceive => purge_receive(application_port, peer_port),
        CaseId::WaitCommEvent => wait_comm_event(application_port, peer_port),
        CaseId::SerialConfiguration => serial_configuration(application_port, profile),
        CaseId::ModemControl => modem_control(application_port),
        CaseId::QueueStatus => queue_status(application_port, peer_port),
    };

    match result {
        Ok(detail) => CaseResult {
            id: profile_case.id,
            requirement: profile_case.requirement,
            status: CaseStatus::Passed,
            detail,
            win32_error: None,
        },
        Err(error) => CaseResult {
            id: profile_case.id,
            requirement: profile_case.requirement,
            status: CaseStatus::Failed,
            detail: error.to_string(),
            win32_error: match error {
                ConformanceError::Win32 { code, .. } => Some(code),
                _ => None,
            },
        },
    }
}

fn synchronous_io(application_port: &str, peer_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    let peer = Port::open(peer_port, false)?;
    set_timeout(application.handle(), 500)?;
    set_timeout(peer.handle(), 500)?;

    write_sync(application.handle(), TEST_DATA)?;
    let received = read_sync(peer.handle(), TEST_DATA.len())?;
    require_equal("synchronous application write", &received, TEST_DATA)?;

    let reply = b"CatHub synchronous reply";
    write_sync(peer.handle(), reply)?;
    let received = read_sync(application.handle(), reply.len())?;
    require_equal("synchronous application read", &received, reply)?;
    Ok("blocking reads and writes transferred bytes in both directions".to_string())
}

fn overlapped_io(application_port: &str, peer_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, true)?;
    let peer = Port::open(peer_port, true)?;

    write_overlapped(application.handle(), TEST_DATA)?;
    let received = read_overlapped(peer.handle(), TEST_DATA.len())?;
    require_equal("overlapped application write", &received, TEST_DATA)?;

    let reply = b"CatHub overlapped reply";
    write_overlapped(peer.handle(), reply)?;
    let received = read_overlapped(application.handle(), reply.len())?;
    require_equal("overlapped application read", &received, reply)?;
    Ok("overlapped reads and writes transferred bytes in both directions".to_string())
}

fn cancel_pending_read(application_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, true)?;
    let event = Event::new()?;
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    overlapped.hEvent = event.handle();
    let mut byte = 0u8;
    let mut immediate = 0u32;
    let started = unsafe {
        ReadFile(
            application.handle(),
            &mut byte,
            1,
            &mut immediate,
            &mut overlapped,
        )
    };
    if started != 0 {
        return Err(ConformanceError::InvalidResult(
            "pending read completed before cancellation".to_string(),
        ));
    }
    let error = unsafe { GetLastError() };
    if error != ERROR_IO_PENDING {
        return Err(win32("ReadFile for cancellation", error));
    }
    if unsafe { CancelIoEx(application.handle(), &overlapped) } == 0 {
        return Err(last_error("CancelIoEx"));
    }
    wait_event(event.handle(), IO_WAIT_MS, "cancelled read")?;
    let mut transferred = 0u32;
    if unsafe { GetOverlappedResult(application.handle(), &overlapped, &mut transferred, 0) } != 0 {
        return Err(ConformanceError::InvalidResult(
            "cancelled read reported success".to_string(),
        ));
    }
    let error = unsafe { GetLastError() };
    if error != ERROR_OPERATION_ABORTED {
        return Err(win32("GetOverlappedResult after cancellation", error));
    }
    Ok("CancelIoEx completed the pending read with ERROR_OPERATION_ABORTED".to_string())
}

fn read_timeout(application_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    set_timeout(application.handle(), 100)?;
    let mut byte = 0u8;
    let mut read = 0u32;
    let start = Instant::now();
    if unsafe { ReadFile(application.handle(), &mut byte, 1, &mut read, null_mut()) } == 0 {
        return Err(last_error("ReadFile timeout"));
    }
    let elapsed = start.elapsed();
    if read != 0 || elapsed < Duration::from_millis(50) || elapsed > Duration::from_secs(1) {
        return Err(ConformanceError::InvalidResult(format!(
            "empty read returned {read} bytes after {} ms",
            elapsed.as_millis()
        )));
    }
    Ok(format!(
        "empty read completed after {} ms",
        elapsed.as_millis()
    ))
}

fn purge_receive(application_port: &str, peer_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    let peer = Port::open(peer_port, false)?;
    set_timeout(application.handle(), 500)?;
    write_sync(peer.handle(), TEST_DATA)?;
    std::thread::sleep(Duration::from_millis(50));
    let before = queue_depth(application.handle())?;
    if before == 0 {
        return Err(ConformanceError::InvalidResult(
            "receive queue did not contain peer data before purge".to_string(),
        ));
    }
    if unsafe { PurgeComm(application.handle(), PURGE_RXABORT | PURGE_RXCLEAR) } == 0 {
        return Err(last_error("PurgeComm"));
    }
    let after = queue_depth(application.handle())?;
    if after != 0 {
        return Err(ConformanceError::InvalidResult(format!(
            "receive queue contains {after} bytes after purge"
        )));
    }
    Ok(format!("purge removed {before} queued bytes"))
}

fn wait_comm_event(application_port: &str, peer_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, true)?;
    let peer = Port::open(peer_port, true)?;
    if unsafe { SetCommMask(application.handle(), EV_RXCHAR) } == 0 {
        return Err(last_error("SetCommMask"));
    }
    let event = Event::new()?;
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    overlapped.hEvent = event.handle();
    let mut event_mask = 0u32;
    let started = unsafe { WaitCommEvent(application.handle(), &mut event_mask, &mut overlapped) };
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(win32("WaitCommEvent", error));
        }
    }
    write_overlapped(peer.handle(), b"E")?;
    if started == 0 {
        wait_event(event.handle(), IO_WAIT_MS, "WaitCommEvent")?;
        let mut transferred = 0u32;
        if unsafe { GetOverlappedResult(application.handle(), &overlapped, &mut transferred, 0) }
            == 0
        {
            return Err(last_error("GetOverlappedResult for WaitCommEvent"));
        }
    }
    if event_mask & EV_RXCHAR == 0 {
        return Err(ConformanceError::InvalidResult(format!(
            "WaitCommEvent returned mask 0x{event_mask:08x}"
        )));
    }
    Ok("WaitCommEvent reported EV_RXCHAR after peer data arrived".to_string())
}

fn serial_configuration(
    application_port: &str,
    profile: &ApplicationProfile,
) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    let mut original: DCB = unsafe { zeroed() };
    original.DCBlength = u32::try_from(size_of::<DCB>())
        .map_err(|_| ConformanceError::InvalidResult("DCB size overflow".to_string()))?;
    if unsafe { GetCommState(application.handle(), &mut original) } == 0 {
        return Err(last_error("GetCommState"));
    }

    let winkeyer = matches!(profile.name, "n1mm-winkeyer" | "wktools");
    let command = if winkeyer {
        "baud=1200 parity=N data=8 stop=2"
    } else {
        "baud=9600 parity=N data=8 stop=1"
    };
    let mut proposed = original;
    let wide = wide(command);
    if unsafe { BuildCommDCBW(wide.as_ptr(), &mut proposed) } == 0 {
        return Err(last_error("BuildCommDCBW"));
    }

    let result = (|| {
        if unsafe { SetCommState(application.handle(), &proposed) } == 0 {
            return Err(last_error("SetCommState"));
        }
        let mut observed: DCB = unsafe { zeroed() };
        observed.DCBlength = original.DCBlength;
        if unsafe { GetCommState(application.handle(), &mut observed) } == 0 {
            return Err(last_error("GetCommState after update"));
        }
        let expected_baud = if winkeyer { 1_200 } else { 9_600 };
        let expected_stop = if winkeyer { TWOSTOPBITS } else { ONESTOPBIT };
        if observed.BaudRate != expected_baud
            || observed.ByteSize != 8
            || observed.Parity != NOPARITY
            || observed.StopBits != expected_stop
        {
            return Err(ConformanceError::InvalidResult(format!(
                "observed format baud={} data={} parity={} stop={}",
                observed.BaudRate, observed.ByteSize, observed.Parity, observed.StopBits
            )));
        }
        Ok(format!("serial format accepted: {command}"))
    })();

    let _ = unsafe { SetCommState(application.handle(), &original) };
    result
}

fn modem_control(application_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    for (operation, code) in [
        ("SETDTR", SETDTR),
        ("CLRDTR", CLRDTR),
        ("SETRTS", SETRTS),
        ("CLRRTS", CLRRTS),
    ] {
        if unsafe { EscapeCommFunction(application.handle(), code) } == 0 {
            return Err(last_error(operation));
        }
    }
    if unsafe { SetCommBreak(application.handle()) } == 0 {
        return Err(last_error("SetCommBreak"));
    }
    if unsafe { ClearCommBreak(application.handle()) } == 0 {
        return Err(last_error("ClearCommBreak"));
    }
    Ok("DTR, RTS, and break control requests completed".to_string())
}

fn queue_status(application_port: &str, peer_port: &str) -> Result<String, ConformanceError> {
    let application = Port::open(application_port, false)?;
    let peer = Port::open(peer_port, false)?;
    set_timeout(application.handle(), 500)?;
    write_sync(peer.handle(), TEST_DATA)?;
    std::thread::sleep(Duration::from_millis(50));
    let depth = queue_depth(application.handle())?;
    if depth < u32::try_from(TEST_DATA.len()).unwrap_or(u32::MAX) {
        return Err(ConformanceError::InvalidResult(format!(
            "ClearCommError reported only {depth} queued bytes"
        )));
    }
    let received = read_sync(application.handle(), TEST_DATA.len())?;
    require_equal("queue status drain", &received, TEST_DATA)?;
    Ok(format!("ClearCommError reported {depth} queued bytes"))
}

fn set_timeout(handle: HANDLE, milliseconds: u32) -> Result<(), ConformanceError> {
    let mut original: COMMTIMEOUTS = unsafe { zeroed() };
    if unsafe { GetCommTimeouts(handle, &mut original) } == 0 {
        return Err(last_error("GetCommTimeouts"));
    }
    let timeouts = COMMTIMEOUTS {
        ReadIntervalTimeout: 0,
        ReadTotalTimeoutMultiplier: 0,
        ReadTotalTimeoutConstant: milliseconds,
        WriteTotalTimeoutMultiplier: 0,
        WriteTotalTimeoutConstant: milliseconds,
    };
    if unsafe { SetCommTimeouts(handle, &timeouts) } == 0 {
        return Err(last_error("SetCommTimeouts"));
    }
    Ok(())
}

fn queue_depth(handle: HANDLE) -> Result<u32, ConformanceError> {
    let mut errors = 0u32;
    let mut status: COMSTAT = unsafe { zeroed() };
    if unsafe { ClearCommError(handle, &mut errors, &mut status) } == 0 {
        return Err(last_error("ClearCommError"));
    }
    Ok(status.cbInQue)
}

fn write_sync(handle: HANDLE, bytes: &[u8]) -> Result<(), ConformanceError> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| ConformanceError::InvalidResult("test payload is too large".to_string()))?;
    let mut written = 0u32;
    if unsafe { WriteFile(handle, bytes.as_ptr(), length, &mut written, null_mut()) } == 0 {
        return Err(last_error("WriteFile"));
    }
    if written != length {
        return Err(ConformanceError::InvalidResult(format!(
            "WriteFile accepted {written} of {length} bytes"
        )));
    }
    Ok(())
}

fn read_sync(handle: HANDLE, length: usize) -> Result<Vec<u8>, ConformanceError> {
    let length_u32 = u32::try_from(length)
        .map_err(|_| ConformanceError::InvalidResult("test read is too large".to_string()))?;
    let mut bytes = vec![0u8; length];
    let mut read = 0u32;
    if unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr(),
            length_u32,
            &mut read,
            null_mut(),
        )
    } == 0
    {
        return Err(last_error("ReadFile"));
    }
    bytes.truncate(usize::try_from(read).unwrap_or(0));
    Ok(bytes)
}

fn write_overlapped(handle: HANDLE, bytes: &[u8]) -> Result<(), ConformanceError> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| ConformanceError::InvalidResult("test payload is too large".to_string()))?;
    let event = Event::new()?;
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    overlapped.hEvent = event.handle();
    let mut transferred = 0u32;
    let started = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr(),
            length,
            &mut transferred,
            &mut overlapped,
        )
    };
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(win32("overlapped WriteFile", error));
        }
        wait_event(event.handle(), IO_WAIT_MS, "overlapped write")?;
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 0) } == 0 {
            return Err(last_error("GetOverlappedResult for write"));
        }
    }
    if transferred != length {
        return Err(ConformanceError::InvalidResult(format!(
            "overlapped write transferred {transferred} of {length} bytes"
        )));
    }
    Ok(())
}

fn read_overlapped(handle: HANDLE, length: usize) -> Result<Vec<u8>, ConformanceError> {
    let length_u32 = u32::try_from(length)
        .map_err(|_| ConformanceError::InvalidResult("test read is too large".to_string()))?;
    let event = Event::new()?;
    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    overlapped.hEvent = event.handle();
    let mut bytes = vec![0u8; length];
    let mut transferred = 0u32;
    let started = unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr(),
            length_u32,
            &mut transferred,
            &mut overlapped,
        )
    };
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(win32("overlapped ReadFile", error));
        }
        wait_event(event.handle(), IO_WAIT_MS, "overlapped read")?;
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 0) } == 0 {
            return Err(last_error("GetOverlappedResult for read"));
        }
    }
    bytes.truncate(usize::try_from(transferred).unwrap_or(0));
    Ok(bytes)
}

fn wait_event(
    handle: HANDLE,
    timeout_ms: u32,
    operation: &'static str,
) -> Result<(), ConformanceError> {
    let result = unsafe { WaitForSingleObject(handle, timeout_ms) };
    if result != WAIT_OBJECT_0 {
        return Err(ConformanceError::InvalidResult(format!(
            "{operation} did not complete within {timeout_ms} ms, wait result {result}"
        )));
    }
    Ok(())
}

fn require_equal(operation: &str, actual: &[u8], expected: &[u8]) -> Result<(), ConformanceError> {
    if actual != expected {
        return Err(ConformanceError::InvalidResult(format!(
            "{operation} returned {} bytes, expected {}",
            actual.len(),
            expected.len()
        )));
    }
    Ok(())
}

fn last_error(operation: &'static str) -> ConformanceError {
    win32(operation, unsafe { GetLastError() })
}

const fn win32(operation: &'static str, code: u32) -> ConformanceError {
    ConformanceError::Win32 { operation, code }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Port(HANDLE);

impl Port {
    fn open(name: &str, overlapped: bool) -> Result<Self, ConformanceError> {
        let path = wide(&format!(r"\\.\{name}"));
        let flags = FILE_ATTRIBUTE_NORMAL | if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                flags,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateFileW serial port"));
        }
        Ok(Self(handle))
    }

    const fn handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self, ConformanceError> {
        let handle = unsafe { CreateEventW(null(), 1, 0, null()) };
        if handle.is_null() {
            return Err(last_error("CreateEventW"));
        }
        Ok(Self(handle))
    }

    const fn handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}
