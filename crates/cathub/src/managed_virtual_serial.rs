//! Bridge between CatHub endpoint sessions and the private UMDF transport.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

const BRIDGE_CAPACITY: usize = 64 * 1024;
const MESSAGE_CAPACITY: usize = 128;

/// Open and attach to one driver-managed virtual serial endpoint.
pub(crate) async fn open(stable_id: &str, expected_kind: u16) -> io::Result<DuplexStream> {
    let stable_id = stable_id.to_owned();
    let worker = tokio::task::spawn_blocking(move || platform::connect(&stable_id, expected_kind))
        .await
        .map_err(|error| io::Error::other(format!("managed transport worker failed: {error}")))??;
    Ok(spawn_bridge(worker))
}

fn spawn_bridge(mut worker: platform::Worker) -> DuplexStream {
    let (session, mut bridge) = tokio::io::duplex(BRIDGE_CAPACITY);
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 16 * 1024];
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                read = bridge.read(&mut buffer) => match read {
                    Ok(0) => break,
                    Ok(count) => {
                        let bytes = buffer.get(..count).unwrap_or(&[]).to_vec();
                        if worker.commands.send(platform::Command::Data(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "managed virtual serial session read failed");
                        break;
                    }
                },
                event = worker.events.recv() => match event {
                    Some(platform::Event::Data(bytes)) => {
                        let count = bytes.len();
                        if bridge.write_all(&bytes).await.is_err() {
                            break;
                        }
                        if worker.commands.send(platform::Command::Release(count)).await.is_err() {
                            break;
                        }
                    }
                    Some(platform::Event::ApplicationOpen(sequence)) => {
                        tracing::debug!(sequence, "application opened managed COM endpoint");
                    }
                    Some(platform::Event::ApplicationClose(sequence)) => {
                        tracing::debug!(sequence, "application closed managed COM endpoint");
                        break;
                    }
                    Some(platform::Event::SerialConfiguration(config)) => {
                        tracing::debug!(?config, "application updated managed COM settings");
                    }
                    Some(platform::Event::ModemControl(lines)) => {
                        tracing::debug!(lines, "application updated managed COM modem lines");
                    }
                    Some(platform::Event::Closed) | None => break,
                    Some(platform::Event::Failed(error)) => {
                        tracing::warn!(%error, "managed virtual serial transport failed");
                        break;
                    }
                },
                _ = heartbeat.tick() => {
                    if worker.commands.send(platform::Command::Health).await.is_err() {
                        break;
                    }
                }
            }
        }

        let _ = worker.commands.send(platform::Command::Shutdown).await;
    });
    session
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    use cathub_virtual_serial::daemon::SerialConfiguration;
    use tokio::sync::mpsc;

    pub(super) enum Command {
        Data(Vec<u8>),
        Release(usize),
        Health,
        Shutdown,
    }

    pub(super) enum Event {
        Data(Vec<u8>),
        ApplicationOpen(u64),
        ApplicationClose(u64),
        SerialConfiguration(SerialConfiguration),
        ModemControl(u32),
        Closed,
        Failed(String),
    }

    pub(super) struct Worker {
        pub(super) commands: mpsc::Sender<Command>,
        pub(super) events: mpsc::Receiver<Event>,
    }

    pub(super) fn connect(_stable_id: &str, _expected_kind: u16) -> io::Result<Worker> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "managed virtual serial endpoints require Windows",
        ))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::mem::{offset_of, size_of};
    use std::os::windows::fs::OpenOptionsExt;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use cathub_virtual_serial::daemon::{
        DaemonEvent, DaemonProtocol, EndpointDescriptor, SerialConfiguration,
    };
    use tokio::sync::mpsc;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IMPERSONATION, SECURITY_SQOS_PRESENT};

    const PRIVATE_INTERFACE: GUID = GUID {
        data1: 0x0084_BDDE,
        data2: 0x9F40,
        data3: 0x4A6A,
        data4: [0xAF, 0x84, 0x0F, 0x4E, 0x46, 0xB7, 0x09, 0x01],
    };

    pub(super) enum Command {
        Data(Vec<u8>),
        Release(usize),
        Health,
        Shutdown,
    }

    pub(super) enum Event {
        Data(Vec<u8>),
        ApplicationOpen(u64),
        ApplicationClose(u64),
        SerialConfiguration(SerialConfiguration),
        ModemControl(u32),
        Closed,
        Failed(String),
    }

    pub(super) struct Worker {
        pub(super) commands: mpsc::Sender<Command>,
        pub(super) events: mpsc::Receiver<Event>,
    }

    pub(super) fn connect(stable_id: &str, expected_kind: u16) -> io::Result<Worker> {
        let paths = enumerate_interfaces()?;
        let mut last_error = None;
        for path in paths {
            match connect_path(&path, stable_id, expected_kind) {
                Ok(worker) => return Ok(worker),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "CatHub UMDF private interface was not found",
            )
        }))
    }

    fn connect_path(path: &str, stable_id: &str, expected_kind: u16) -> io::Result<Worker> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION)
            .open(path)?;
        let mut protocol = DaemonProtocol::new();

        write_frame(
            &mut file,
            &DaemonProtocol::hello(1).map_err(protocol_error)?,
        )?;
        read_until(&mut file, &mut protocol, |event| {
            matches!(event, DaemonEvent::Negotiated)
        })?;

        write_frame(&mut file, &protocol.discover(2).map_err(protocol_error)?)?;
        let discovery = read_until(&mut file, &mut protocol, |event| {
            matches!(event, DaemonEvent::DiscoveryComplete)
        })?;
        let endpoint = discovery
            .iter()
            .find_map(|event| match event {
                DaemonEvent::Endpoint(endpoint) if endpoint.stable_id == stable_id => {
                    Some(endpoint.clone())
                }
                _ => None,
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("managed virtual endpoint `{stable_id}` was not found"),
                )
            })?;
        validate_endpoint(&endpoint, expected_kind)?;

        write_frame(
            &mut file,
            &protocol
                .attach(endpoint.endpoint_id, 3)
                .map_err(protocol_error)?,
        )?;
        read_until(&mut file, &mut protocol, |event| {
            matches!(event, DaemonEvent::Attached { .. })
        })?;

        spawn_workers(file, protocol)
    }

    fn validate_endpoint(endpoint: &EndpointDescriptor, expected_kind: u16) -> io::Result<()> {
        if !endpoint.enabled {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "managed virtual endpoint `{}` is disabled",
                    endpoint.stable_id
                ),
            ));
        }
        if endpoint.kind != expected_kind {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "managed virtual endpoint `{}` has kind {}, expected {expected_kind}",
                    endpoint.stable_id, endpoint.kind
                ),
            ));
        }
        Ok(())
    }

    fn spawn_workers(file: File, protocol: DaemonProtocol) -> io::Result<Worker> {
        let reader_file = file.try_clone()?;
        let protocol = Arc::new(Mutex::new(protocol));
        let stopping = Arc::new(AtomicBool::new(false));
        let request_id = Arc::new(AtomicU64::new(10));
        let (commands_tx, commands_rx) = mpsc::channel(super::MESSAGE_CAPACITY);
        let (events_tx, events_rx) = mpsc::channel(super::MESSAGE_CAPACITY);

        {
            let protocol = protocol.clone();
            let stopping = stopping.clone();
            let events = events_tx.clone();
            std::thread::Builder::new()
                .name("cathub-umdf-reader".to_owned())
                .spawn(move || reader_loop(reader_file, protocol, stopping, events))?;
        }
        {
            let protocol = protocol.clone();
            let stopping = stopping.clone();
            std::thread::Builder::new()
                .name("cathub-umdf-writer".to_owned())
                .spawn(move || writer_loop(file, protocol, stopping, request_id, commands_rx))?;
        }

        Ok(Worker {
            commands: commands_tx,
            events: events_rx,
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn reader_loop(
        mut file: File,
        protocol: Arc<Mutex<DaemonProtocol>>,
        stopping: Arc<AtomicBool>,
        events: mpsc::Sender<Event>,
    ) {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let count = match file.read(&mut buffer) {
                Ok(0) => {
                    let _ = events.blocking_send(Event::Closed);
                    return;
                }
                Ok(count) => count,
                Err(error) => {
                    let _ = events.blocking_send(Event::Failed(error.to_string()));
                    return;
                }
            };
            let decoded = match protocol.lock() {
                Ok(mut protocol) => protocol.ingest(buffer.get(..count).unwrap_or(&[])),
                Err(error) => {
                    let _ = events.blocking_send(Event::Failed(error.to_string()));
                    return;
                }
            };
            match decoded {
                Ok(decoded) => {
                    for event in decoded {
                        if let Some(event) = translate_event(event) {
                            if events.blocking_send(event).is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = events.blocking_send(Event::Failed(error.to_string()));
                    return;
                }
            }
            if stopping.load(Ordering::Acquire) {
                let _ = events.blocking_send(Event::Closed);
                return;
            }
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn writer_loop(
        mut file: File,
        protocol: Arc<Mutex<DaemonProtocol>>,
        stopping: Arc<AtomicBool>,
        request_id: Arc<AtomicU64>,
        mut commands: mpsc::Receiver<Command>,
    ) {
        while let Some(command) = commands.blocking_recv() {
            let result = match command {
                Command::Data(bytes) => with_protocol(&protocol, |protocol| protocol.data(&bytes))
                    .and_then(|frame| write_frame(&mut file, &frame)),
                Command::Release(count) => {
                    with_protocol(&protocol, |protocol| protocol.release_received(count))
                        .and_then(|frame| write_optional_frame(&mut file, frame))
                }
                Command::Health => with_protocol(&protocol, |protocol| {
                    protocol.health(request_id.fetch_add(1, Ordering::Relaxed))
                })
                .and_then(|frame| write_frame(&mut file, &frame)),
                Command::Shutdown => {
                    stopping.store(true, Ordering::Release);
                    let detach = with_protocol(&protocol, |protocol| protocol.detach(0));
                    if let Ok(frame) = detach {
                        let _ = write_optional_frame(&mut file, frame);
                    }
                    with_protocol(&protocol, |protocol| {
                        protocol.health(request_id.fetch_add(1, Ordering::Relaxed))
                    })
                    .and_then(|frame| write_frame(&mut file, &frame))
                }
            };
            if result.is_err() || stopping.load(Ordering::Acquire) {
                return;
            }
        }
    }

    fn with_protocol<T>(
        protocol: &Mutex<DaemonProtocol>,
        operation: impl FnOnce(
            &mut DaemonProtocol,
        ) -> Result<T, cathub_virtual_serial::daemon::DaemonProtocolError>,
    ) -> io::Result<T> {
        let mut guard = protocol
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        operation(&mut guard).map_err(protocol_error)
    }

    fn translate_event(event: DaemonEvent) -> Option<Event> {
        match event {
            DaemonEvent::Data(bytes) => Some(Event::Data(bytes)),
            DaemonEvent::ApplicationOpen(sequence) => Some(Event::ApplicationOpen(sequence)),
            DaemonEvent::ApplicationClose(sequence) => Some(Event::ApplicationClose(sequence)),
            DaemonEvent::SerialConfiguration(config) => Some(Event::SerialConfiguration(config)),
            DaemonEvent::ModemControl(lines) => Some(Event::ModemControl(lines)),
            _ => None,
        }
    }

    fn read_until(
        file: &mut File,
        protocol: &mut DaemonProtocol,
        done: impl Fn(&DaemonEvent) -> bool,
    ) -> io::Result<Vec<DaemonEvent>> {
        let mut all_events = Vec::new();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "UMDF private channel closed during handshake",
                ));
            }
            let events = protocol
                .ingest(buffer.get(..count).unwrap_or(&[]))
                .map_err(protocol_error)?;
            let complete = events.iter().any(&done);
            all_events.extend(events);
            if complete {
                return Ok(all_events);
            }
        }
    }

    fn write_frame(file: &mut File, frame: &[u8]) -> io::Result<()> {
        file.write_all(frame)
    }

    fn write_optional_frame(file: &mut File, frame: Option<Vec<u8>>) -> io::Result<()> {
        frame.map_or(Ok(()), |frame| write_frame(file, &frame))
    }

    fn protocol_error(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, error.to_string())
    }

    fn enumerate_interfaces() -> io::Result<Vec<String>> {
        let interface_guid = PRIVATE_INTERFACE;
        let handle = unsafe {
            SetupDiGetClassDevsW(
                &raw const interface_guid,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if handle == INVALID_HANDLE_VALUE as HDEVINFO {
            return Err(io::Error::last_os_error());
        }
        let set = DeviceInfoSet(handle);
        let mut paths = Vec::new();
        let mut index = 0;
        loop {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: u32::try_from(size_of::<SP_DEVICE_INTERFACE_DATA>()).unwrap_or(u32::MAX),
                InterfaceClassGuid: zero_guid(),
                Flags: 0,
                Reserved: 0,
            };
            let found = unsafe {
                SetupDiEnumDeviceInterfaces(
                    set.0,
                    null(),
                    &raw const interface_guid,
                    index,
                    &raw mut interface,
                )
            };
            if found == 0 {
                let error = unsafe { GetLastError() };
                if error == ERROR_NO_MORE_ITEMS {
                    break;
                }
                return Err(io::Error::from_raw_os_error(error.cast_signed()));
            }
            paths.push(interface_path(set.0, &mut interface)?);
            index += 1;
        }
        Ok(paths)
    }

    fn interface_path(
        set: HDEVINFO,
        interface: &mut SP_DEVICE_INTERFACE_DATA,
    ) -> io::Result<String> {
        let mut required = 0;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set,
                interface,
                null_mut(),
                0,
                &raw mut required,
                null_mut(),
            );
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_INSUFFICIENT_BUFFER {
            return Err(io::Error::from_raw_os_error(error.cast_signed()));
        }

        let byte_count = usize::try_from(required).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "interface path is too large")
        })?;
        let words = byte_count.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let detail = storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail).cbSize =
                u32::try_from(size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>()).unwrap_or(u32::MAX);
        }
        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set,
                interface,
                detail,
                required,
                null_mut(),
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        let path = unsafe {
            let offset = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
            let start = detail.cast::<u16>().add(offset / size_of::<u16>());
            let units = byte_count.saturating_sub(offset) / size_of::<u16>();
            std::slice::from_raw_parts(start, units)
        };
        let length = path
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(path.len());
        String::from_utf16(path.get(..length).unwrap_or(&[]))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    const fn zero_guid() -> GUID {
        GUID {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        }
    }

    struct DeviceInfoSet(HDEVINFO);

    impl Drop for DeviceInfoSet {
        fn drop(&mut self) {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
}
