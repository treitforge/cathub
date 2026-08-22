//! Auditable Windows/WDF FFI boundary.

use std::{
    collections::VecDeque,
    ffi::c_void,
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicPtr, Ordering},
    },
    time::{Duration, Instant},
};

use wdk::println;
use wdk_sys::{
    _WDF_EXECUTION_LEVEL, _WDF_FILEOBJECT_CLASS, _WDF_IO_QUEUE_DISPATCH_TYPE,
    _WDF_SYNCHRONIZATION_SCOPE, _WDF_TRI_STATE, BOOLEAN, GUID, KEY_QUERY_VALUE, NTSTATUS,
    PCUNICODE_STRING, PDRIVER_OBJECT, PLUGPLAY_REGKEY_DEVICE, PVOID, ULONG, ULONG_PTR,
    UNICODE_STRING, WDF_DRIVER_CONFIG, WDF_FILEOBJECT_CONFIG, WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE,
    WDF_NO_OBJECT_ATTRIBUTES, WDF_OBJECT_ATTRIBUTES, WDF_OBJECT_CONTEXT_TYPE_INFO,
    WDF_TIMER_CONFIG, WDFDEVICE, WDFDEVICE_INIT, WDFDRIVER, WDFFILEOBJECT, WDFKEY, WDFOBJECT,
    WDFQUEUE, WDFQUEUE__, WDFREQUEST, WDFTIMER, call_unsafe_wdf_function_binding,
};

use crate::data_plane::{ChannelRole, DEFAULT_BUFFER_CAPACITY, DataPlaneError, EndpointDataPlane};
use crate::private_protocol::{
    DriverProtocol, DriverProtocolError, EndpointMetadata, ProtocolOutput,
};
use crate::serial::{
    SerialBaudRate, SerialChars, SerialCommProperties, SerialHandflow, SerialLineControl,
    SerialQueueSize, SerialState, SerialStateError, SerialStatus, SerialTimeouts, ioctl, purge,
};

const STATUS_SUCCESS: NTSTATUS = 0;
const STATUS_UNSUCCESSFUL: NTSTATUS = -1_073_741_823;
const STATUS_INVALID_DEVICE_REQUEST: NTSTATUS = -1_073_741_808;
const STATUS_INVALID_PARAMETER: NTSTATUS = -1_073_741_811;
const STATUS_OBJECT_NAME_INVALID: NTSTATUS = -1_073_741_773;
const STATUS_SHARING_VIOLATION: NTSTATUS = -1_073_741_757;
const STATUS_DEVICE_NOT_CONNECTED: NTSTATUS = -1_073_741_667;
const STATUS_CANCELLED: NTSTATUS = -1_073_741_536;
const STATUS_BUFFER_OVERFLOW: NTSTATUS = -2_147_483_643;
const TRUE: BOOLEAN = 1;

const DAEMON_REFERENCE: [u16; 7] = [100, 97, 101, 109, 111, 110, 0];
const PORT_NAME_VALUE: [u16; 9] = [80, 111, 114, 116, 78, 97, 109, 101, 0];
const ENDPOINT_KIND_VALUE: [u16; 19] = [
    67, 97, 116, 72, 117, 98, 69, 110, 100, 112, 111, 105, 110, 116, 75, 105, 110, 100, 0,
];
const STABLE_ID_VALUE: [u16; 15] = [
    67, 97, 116, 72, 117, 98, 83, 116, 97, 98, 108, 101, 73, 100, 0,
];
const DISPLAY_NAME_VALUE: [u16; 18] = [
    67, 97, 116, 72, 117, 98, 68, 105, 115, 112, 108, 97, 121, 78, 97, 109, 101, 0,
];
const DOS_DEVICE_PREFIX: &[u16] = &[
    92, 68, 111, 115, 68, 101, 118, 105, 99, 101, 115, 92, 71, 108, 111, 98, 97, 108, 92,
];

/// Private proof-of-concept interface, `{0084BDDE-9F40-4A6A-AF84-0F4E46B70901}`.
static CATHUB_POC_INTERFACE_GUID: GUID = GUID {
    Data1: 0x0084_BDDE,
    Data2: 0x9F40,
    Data3: 0x4A6A,
    Data4: [0xAF, 0x84, 0x0F, 0x4E, 0x46, 0xB7, 0x09, 0x01],
};

/// Standard Windows COM-port interface, `{86E0D1E0-8089-11D0-9CE4-08003E301F73}`.
static GUID_DEVINTERFACE_COMPORT: GUID = GUID {
    Data1: 0x86E0_D1E0,
    Data2: 0x8089,
    Data3: 0x11D0,
    Data4: [0x9C, 0xE4, 0x08, 0x00, 0x3E, 0x30, 0x1F, 0x73],
};

#[repr(C)]
struct DeviceContext {
    state: *mut DeviceState,
}

struct DeviceState {
    channel: Mutex<ChannelState>,
    protocol: Mutex<DriverProtocol>,
    serial: Mutex<SerialState>,
    application_reads: AtomicPtr<WDFQUEUE__>,
    daemon_reads: AtomicPtr<WDFQUEUE__>,
    wait_requests: AtomicPtr<WDFQUEUE__>,
    pending_application_reads: Mutex<VecDeque<PendingRead>>,
}

#[derive(Debug, Clone, Copy)]
struct PendingRead {
    request: usize,
    deadline: Option<Instant>,
}

impl DeviceState {
    #[cfg(test)]
    fn new() -> Self {
        Self::for_endpoint(EndpointMetadata::default())
    }

    fn for_endpoint(endpoint: EndpointMetadata) -> Self {
        let mut serial = SerialState::default();
        serial.set_modem_input(serial.modem_input());
        Self {
            channel: Mutex::new(ChannelState::new()),
            protocol: Mutex::new(DriverProtocol::for_endpoint(endpoint)),
            serial: Mutex::new(serial),
            application_reads: AtomicPtr::new(ptr::null_mut()),
            daemon_reads: AtomicPtr::new(ptr::null_mut()),
            wait_requests: AtomicPtr::new(ptr::null_mut()),
            pending_application_reads: Mutex::new(VecDeque::new()),
        }
    }

    fn channel(&self) -> MutexGuard<'_, ChannelState> {
        self.channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn queue_for(&self, role: ChannelRole) -> WDFQUEUE {
        match role {
            ChannelRole::Application => self.application_reads.load(Ordering::Acquire),
            ChannelRole::Daemon => self.daemon_reads.load(Ordering::Acquire),
        }
    }

    fn serial(&self) -> MutexGuard<'_, SerialState> {
        self.serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn protocol(&self) -> MutexGuard<'_, DriverProtocol> {
        self.protocol
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait_queue(&self) -> WDFQUEUE {
        self.wait_requests.load(Ordering::Acquire)
    }

    fn pending_application_reads(&self) -> MutexGuard<'_, VecDeque<PendingRead>> {
        self.pending_application_reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

static mut DEVICE_CONTEXT_TYPE_INFO: WDF_OBJECT_CONTEXT_TYPE_INFO = WDF_OBJECT_CONTEXT_TYPE_INFO {
    Size: 0,
    ContextName: c"CatHubDeviceContext".as_ptr(),
    ContextSize: size_of::<DeviceContext>(),
    UniqueType: ptr::null(),
    EvtDriverGetUniqueContextType: None,
};

struct ChannelState {
    plane: EndpointDataPlane,
    application_owner: usize,
    daemon_owner: usize,
}

impl ChannelState {
    fn new() -> Self {
        Self {
            plane: EndpointDataPlane::new(DEFAULT_BUFFER_CAPACITY),
            application_owner: 0,
            daemon_owner: 0,
        }
    }

    fn open(&mut self, role: ChannelRole, file: WDFFILEOBJECT) -> Result<u64, DataPlaneError> {
        let session = self.plane.open(role)?;
        *self.owner_mut(role) = file.addr();
        Ok(session)
    }

    fn close_if_owner(&mut self, role: ChannelRole, file: WDFFILEOBJECT) -> bool {
        let owner = self.owner_mut(role);
        if *owner != file.addr() {
            return false;
        }
        *owner = 0;
        self.plane.close(role);
        true
    }

    fn owns(&self, role: ChannelRole, file: WDFFILEOBJECT) -> bool {
        let owner = match role {
            ChannelRole::Application => self.application_owner,
            ChannelRole::Daemon => self.daemon_owner,
        };
        owner != 0 && owner == file.addr()
    }

    const fn owner_mut(&mut self, role: ChannelRole) -> &mut usize {
        match role {
            ChannelRole::Application => &mut self.application_owner,
            ChannelRole::Daemon => &mut self.daemon_owner,
        }
    }
}

#[derive(Clone, Copy)]
enum RequestDisposition {
    Complete {
        status: NTSTATUS,
        information: usize,
    },
    Pending,
}

impl RequestDisposition {
    const fn success(information: usize) -> Self {
        Self::Complete {
            status: STATUS_SUCCESS,
            information,
        }
    }

    const fn error(status: NTSTATUS) -> Self {
        Self::Complete {
            status,
            information: 0,
        }
    }
}

/// WDF driver entry point.
///
/// No panic is allowed to unwind across this ABI boundary.
///
/// # Safety
///
/// `driver` and `registry_path` must be the valid pointers supplied by WDF.
// SAFETY: WDF requires this exported symbol, and this crate exports it exactly once.
#[cfg_attr(not(test), unsafe(export_name = "DriverEntry"))]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    ffi_status(|| {
        // SAFETY: The caller is WDF and supplies both pointers under the DriverEntry contract.
        unsafe { driver_entry_inner(driver, registry_path) }
    })
}

unsafe fn driver_entry_inner(driver: PDRIVER_OBJECT, registry_path: PCUNICODE_STRING) -> NTSTATUS {
    println!("CatHub UMDF proof-of-concept DriverEntry");
    // SAFETY: DriverEntry runs before WDF can create device objects or invoke callbacks.
    unsafe { initialize_device_context_type() };
    let mut driver_config = WDF_DRIVER_CONFIG {
        Size: struct_size::<WDF_DRIVER_CONFIG>(),
        EvtDriverDeviceAdd: Some(evt_driver_device_add),
        EvtDriverUnload: Some(evt_driver_unload),
        ..WDF_DRIVER_CONFIG::default()
    };
    // SAFETY: WDF owns both input pointers. Null attributes/output are allowed and the config
    // remains valid for the synchronous call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut driver_config,
            WDF_NO_HANDLE.cast::<WDFDRIVER>(),
        )
    }
}

unsafe extern "C" fn evt_driver_device_add(
    _driver: WDFDRIVER,
    device_init: *mut WDFDEVICE_INIT,
) -> NTSTATUS {
    ffi_status(|| {
        // SAFETY: WDF supplies a valid, exclusively owned device-init pointer.
        unsafe { create_device(device_init) }
    })
}

unsafe fn create_device(mut device_init: *mut WDFDEVICE_INIT) -> NTSTATUS {
    let mut file_config = WDF_FILEOBJECT_CONFIG {
        Size: struct_size::<WDF_FILEOBJECT_CONFIG>(),
        EvtDeviceFileCreate: Some(evt_device_file_create),
        EvtFileCleanup: Some(evt_file_cleanup),
        AutoForwardCleanupClose: _WDF_TRI_STATE::WdfUseDefault,
        FileObjectClass: _WDF_FILEOBJECT_CLASS::WdfFileObjectWdfCannotUseFsContexts,
        ..WDF_FILEOBJECT_CONFIG::default()
    };
    // SAFETY: WDF copies this configuration synchronously. Null file attributes are allowed.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetFileObjectConfig,
            device_init,
            &raw mut file_config,
            WDF_NO_OBJECT_ATTRIBUTES,
        );
    }

    let mut device_attributes = WDF_OBJECT_ATTRIBUTES {
        Size: struct_size::<WDF_OBJECT_ATTRIBUTES>(),
        EvtDestroyCallback: Some(evt_device_context_destroy),
        ExecutionLevel: _WDF_EXECUTION_LEVEL::WdfExecutionLevelPassive,
        SynchronizationScope: _WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeNone,
        ContextTypeInfo: device_context_type_info(),
        ..WDF_OBJECT_ATTRIBUTES::default()
    };
    let mut device: WDFDEVICE = WDF_NO_HANDLE.cast();
    // SAFETY: This consumes WDF's device-init pointer exactly once on success.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreate,
            &raw mut device_init,
            &raw mut device_attributes,
            &raw mut device,
        )
    };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: Device creation allocated and zeroed the registered context space.
    let Some(context) = (unsafe { device_context(device) }) else {
        return STATUS_UNSUCCESSFUL;
    };
    // SAFETY: The live WDF device owns a queryable PnP instance registry key.
    let endpoint = unsafe { read_endpoint_metadata(device) };
    let state = Box::into_raw(Box::new(DeviceState::for_endpoint(endpoint)));
    // SAFETY: The context is exclusively initialized before queues or interfaces publish device.
    unsafe { (*context).state = state };
    // SAFETY: `state` remains owned by the WDF device context until its destroy callback.
    let state = unsafe { &*state };
    // SAFETY: The WDF device was successfully created above.
    let status = unsafe { configure_queues(device, state) };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: The device is live and owns the periodic timer for its full lifetime.
    let status = unsafe { configure_read_timeout_timer(device) };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: The device exists and registration consumes both counted strings synchronously.
    unsafe { register_interfaces(device) }
}

unsafe fn configure_read_timeout_timer(device: WDFDEVICE) -> NTSTATUS {
    const TIMER_PERIOD_MS: u32 = 10;
    const FIRST_DUE_TIME_100NS: i64 = -100_000;
    let mut config = WDF_TIMER_CONFIG {
        Size: struct_size::<WDF_TIMER_CONFIG>(),
        EvtTimerFunc: Some(evt_read_timeout_timer),
        Period: TIMER_PERIOD_MS,
        ..WDF_TIMER_CONFIG::default()
    };
    let mut attributes = WDF_OBJECT_ATTRIBUTES {
        Size: struct_size::<WDF_OBJECT_ATTRIBUTES>(),
        ParentObject: device.cast(),
        ExecutionLevel: _WDF_EXECUTION_LEVEL::WdfExecutionLevelPassive,
        SynchronizationScope: _WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeNone,
        ..WDF_OBJECT_ATTRIBUTES::default()
    };
    let mut timer: WDFTIMER = ptr::null_mut();
    // SAFETY: WDF copies both configurations and returns a device-owned timer handle.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfTimerCreate,
            &raw mut config,
            &raw mut attributes,
            &raw mut timer,
        )
    };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: The timer was created successfully and accepts a relative 100ns due time.
    let _was_queued =
        unsafe { call_unsafe_wdf_function_binding!(WdfTimerStart, timer, FIRST_DUE_TIME_100NS) };
    STATUS_SUCCESS
}

unsafe fn configure_queues(device: WDFDEVICE, state: &DeviceState) -> NTSTATUS {
    let mut application_reads = ptr::null_mut();
    // SAFETY: The device and output storage are valid for synchronous queue creation.
    let status = unsafe { create_manual_read_queue(device, &raw mut application_reads) };
    if !nt_success(status) {
        return status;
    }
    state
        .application_reads
        .store(application_reads, Ordering::Release);

    let mut daemon_reads = ptr::null_mut();
    // SAFETY: The device and output storage are valid for synchronous queue creation.
    let status = unsafe { create_manual_read_queue(device, &raw mut daemon_reads) };
    if !nt_success(status) {
        return status;
    }
    state.daemon_reads.store(daemon_reads, Ordering::Release);

    let mut wait_requests = ptr::null_mut();
    // SAFETY: The device and output storage are valid for synchronous queue creation.
    let status = unsafe { create_manual_read_queue(device, &raw mut wait_requests) };
    if !nt_success(status) {
        return status;
    }
    state.wait_requests.store(wait_requests, Ordering::Release);

    let mut default_queue: WDFQUEUE = ptr::null_mut();
    let mut config = WDF_IO_QUEUE_CONFIG {
        Size: struct_size::<WDF_IO_QUEUE_CONFIG>(),
        DispatchType: _WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchSequential,
        PowerManaged: _WDF_TRI_STATE::WdfUseDefault,
        AllowZeroLengthRequests: TRUE,
        DefaultQueue: TRUE,
        EvtIoDefault: Some(evt_io_default),
        EvtIoRead: Some(evt_io_read),
        EvtIoWrite: Some(evt_io_write),
        EvtIoDeviceControl: Some(evt_io_device_control),
        ..WDF_IO_QUEUE_CONFIG::default()
    };
    // SAFETY: WDF copies the config; null attributes are allowed and output storage is valid.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &raw mut config,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut default_queue,
        )
    }
}

unsafe fn create_manual_read_queue(device: WDFDEVICE, queue: *mut WDFQUEUE) -> NTSTATUS {
    let mut config = WDF_IO_QUEUE_CONFIG {
        Size: struct_size::<WDF_IO_QUEUE_CONFIG>(),
        DispatchType: _WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchManual,
        PowerManaged: _WDF_TRI_STATE::WdfUseDefault,
        EvtIoCanceledOnQueue: Some(evt_io_canceled_on_queue),
        ..WDF_IO_QUEUE_CONFIG::default()
    };
    // SAFETY: WDF copies the config; null attributes are allowed and `queue` is valid output.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &raw mut config,
            WDF_NO_OBJECT_ATTRIBUTES,
            queue,
        )
    }
}

unsafe fn register_interfaces(device: WDFDEVICE) -> NTSTATUS {
    // SAFETY: The device is live and the interface GUID has static storage.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &raw const GUID_DEVINTERFACE_COMPORT,
            ptr::null(),
        )
    };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: The device instance key and PortName value are owned by Windows Ports setup.
    let status = unsafe { create_com_symbolic_link(device) };
    if !nt_success(status) {
        return status;
    }
    let daemon = unicode_string(&DAEMON_REFERENCE);
    // SAFETY: All pointers remain valid through this synchronous call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &raw const CATHUB_POC_INTERFACE_GUID,
            &raw const daemon,
        )
    }
}

unsafe fn read_endpoint_metadata(device: WDFDEVICE) -> EndpointMetadata {
    let mut metadata = EndpointMetadata::default();
    let mut key: WDFKEY = ptr::null_mut();
    // SAFETY: The device is live, null attributes are allowed, and output storage is valid.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceOpenRegistryKey,
            device,
            PLUGPLAY_REGKEY_DEVICE,
            KEY_QUERY_VALUE,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut key,
        )
    };
    if !nt_success(status) {
        return metadata;
    }

    let mut kind = 0_u32;
    let kind_name = unicode_string(&ENDPOINT_KIND_VALUE);
    // SAFETY: The key is open and both the value name and output remain valid synchronously.
    let kind_status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRegistryQueryULong,
            key,
            &raw const kind_name,
            &raw mut kind,
        )
    };
    if nt_success(kind_status) && matches!(kind, 1 | 2) {
        metadata.kind = u16::try_from(kind).unwrap_or(1);
    }
    // SAFETY: The key remains open through both synchronous string queries.
    if let Some(stable_id) = unsafe { query_registry_string(key, &STABLE_ID_VALUE) } {
        metadata.stable_id = stable_id;
    }
    // SAFETY: The key remains open through this synchronous string query.
    if let Some(display_name) = unsafe { query_registry_string(key, &DISPLAY_NAME_VALUE) } {
        metadata.display_name = display_name;
    }
    // SAFETY: This driver owns the WDF registry-key handle returned above.
    unsafe { call_unsafe_wdf_function_binding!(WdfRegistryClose, key) };
    metadata
}

unsafe fn query_registry_string(key: WDFKEY, value: &[u16]) -> Option<String> {
    let value_name = unicode_string(value);
    let mut buffer = [0_u16; 128];
    let mut output = unicode_string_buffer(&mut buffer);
    // SAFETY: The key is open and both counted strings remain valid through the call.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRegistryQueryUnicodeString,
            key,
            &raw const value_name,
            ptr::null_mut(),
            &raw mut output,
        )
    };
    if !nt_success(status) {
        return None;
    }
    let units = usize::from(output.Length) / size_of::<u16>();
    let value = String::from_utf16(buffer.get(..units)?)
        .ok()?
        .trim_matches('\0')
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

unsafe fn create_com_symbolic_link(device: WDFDEVICE) -> NTSTATUS {
    let mut key: WDFKEY = ptr::null_mut();
    // SAFETY: The device is live, null attributes are allowed, and output storage is valid.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceOpenRegistryKey,
            device,
            PLUGPLAY_REGKEY_DEVICE,
            KEY_QUERY_VALUE,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut key,
        )
    };
    if !nt_success(status) {
        return status;
    }

    let value_name = unicode_string(&PORT_NAME_VALUE);
    let mut port_buffer = [0_u16; 64];
    let mut port_name = unicode_string_buffer(&mut port_buffer);
    // SAFETY: `key` is open, and both counted strings remain valid through the call.
    let query_status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRegistryQueryUnicodeString,
            key,
            &raw const value_name,
            ptr::null_mut(),
            &raw mut port_name,
        )
    };
    // SAFETY: This driver owns the WDF registry-key handle returned above.
    unsafe { call_unsafe_wdf_function_binding!(WdfRegistryClose, key) };
    if !nt_success(query_status) {
        return query_status;
    }
    let port_units = usize::from(port_name.Length) / size_of::<u16>();
    if port_units == 0 || port_units >= port_buffer.len() {
        return STATUS_OBJECT_NAME_INVALID;
    }
    let mut link = Vec::with_capacity(DOS_DEVICE_PREFIX.len() + port_units + 1);
    link.extend_from_slice(DOS_DEVICE_PREFIX);
    link.extend_from_slice(port_buffer.get(..port_units).unwrap_or_default());
    link.push(0);
    let symbolic_link = unicode_string(&link);
    // SAFETY: The device is live and the counted link string remains valid synchronously.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateSymbolicLink,
            device,
            &raw const symbolic_link,
        )
    }
}

unsafe extern "C" fn evt_device_file_create(
    device: WDFDEVICE,
    request: WDFREQUEST,
    file: WDFFILEOBJECT,
) {
    let disposition = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: WDF keeps the device and its context alive for this callback.
        let Some(state) = (unsafe { device_state(device) }) else {
            return RequestDisposition::error(STATUS_UNSUCCESSFUL);
        };
        // SAFETY: WDF supplies this file object to its create callback.
        let Some(role) = (unsafe { role_from_file(file) }) else {
            return RequestDisposition::error(STATUS_OBJECT_NAME_INVALID);
        };
        let open_result = state.channel().open(role, file);
        match open_result {
            Ok(session) => {
                let outputs = match role {
                    ChannelRole::Application => state.protocol().application_opened(session),
                    ChannelRole::Daemon => {
                        state.protocol().reset_daemon();
                        Ok(Vec::new())
                    }
                };
                let outputs = match outputs {
                    Ok(outputs) => outputs,
                    Err(error) => {
                        state.channel().close_if_owner(role, file);
                        return RequestDisposition::error(status_for_protocol_error(error));
                    }
                };
                // SAFETY: This callback owns the create request and all device queues are live.
                if let Err(error) = unsafe { apply_protocol_outputs(state, outputs) } {
                    state.channel().close_if_owner(role, file);
                    return RequestDisposition::error(status_for_error(error));
                }
                println!("CatHub PoC {role:?} handle opened (session {session})");
                RequestDisposition::success(0)
            }
            Err(error) => RequestDisposition::error(status_for_error(error)),
        }
    }))
    .unwrap_or_else(|_| RequestDisposition::error(STATUS_UNSUCCESSFUL));
    // SAFETY: WDF transfers ownership of the create request to this callback.
    unsafe { finish_request(request, disposition) };
}

unsafe extern "C" fn evt_file_cleanup(file: WDFFILEOBJECT) {
    ffi_void(|| {
        // SAFETY: WDF keeps the file's parent device alive for this cleanup callback.
        let Some(state) = (unsafe { device_state_from_file(file) }) else {
            return;
        };
        // SAFETY: WDF supplies this file object to its cleanup callback.
        let Some(role) = (unsafe { role_from_file(file) }) else {
            return;
        };
        if !state.channel().owns(role, file) {
            return;
        }
        let outputs = match role {
            ChannelRole::Application => state.protocol().application_closed().unwrap_or_default(),
            ChannelRole::Daemon => {
                state.protocol().reset_daemon();
                Vec::new()
            }
        };
        if state.channel().close_if_owner(role, file) {
            println!("CatHub PoC {role:?} handle cleaned up");
            state.pending_application_reads().clear();
            // SAFETY: Cleanup runs while the device and its child queues are alive.
            let _ = unsafe { apply_protocol_outputs(state, outputs) };
            // SAFETY: This handle refers to a manual queue owned by this device.
            unsafe {
                drain_pending_reads(state.queue_for(ChannelRole::Application), STATUS_CANCELLED);
            };
            // SAFETY: This handle refers to a manual queue owned by this device.
            unsafe { drain_pending_reads(state.queue_for(ChannelRole::Daemon), STATUS_CANCELLED) };
            // SAFETY: This handle refers to a manual queue owned by this device.
            unsafe { drain_pending_reads(state.wait_queue(), STATUS_CANCELLED) };
        }
    });
}

unsafe extern "C" fn evt_io_read(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    dispatch_request(request, || {
        // SAFETY: WDF keeps the queue's parent device alive for this callback.
        let Some(state) = (unsafe { device_state_from_queue(queue) }) else {
            return RequestDisposition::error(STATUS_UNSUCCESSFUL);
        };
        // SAFETY: WDF transfers the valid request to this read callback.
        unsafe { handle_read(state, request, length) }
    });
}

unsafe extern "C" fn evt_io_write(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    dispatch_request(request, || {
        // SAFETY: WDF keeps the queue's parent device alive for this callback.
        let Some(state) = (unsafe { device_state_from_queue(queue) }) else {
            return RequestDisposition::error(STATUS_UNSUCCESSFUL);
        };
        // SAFETY: WDF transfers the valid request to this write callback.
        unsafe { handle_write(state, request, length) }
    });
}

unsafe extern "C" fn evt_io_device_control(
    queue: WDFQUEUE,
    request: WDFREQUEST,
    _output_length: usize,
    _input_length: usize,
    control_code: ULONG,
) {
    dispatch_request(request, || {
        // SAFETY: WDF keeps the queue's parent device alive for this callback.
        let Some(state) = (unsafe { device_state_from_queue(queue) }) else {
            return RequestDisposition::error(STATUS_UNSUCCESSFUL);
        };
        // SAFETY: WDF transfers the valid request to this device-control callback.
        unsafe { handle_device_control(state, request, control_code) }
    });
}

unsafe extern "C" fn evt_io_default(_queue: WDFQUEUE, request: WDFREQUEST) {
    ffi_void(|| {
        // SAFETY: WDF transfers ownership of this unsupported request to the callback.
        unsafe { complete_request(request, STATUS_INVALID_DEVICE_REQUEST, 0) };
    });
}

unsafe extern "C" fn evt_io_canceled_on_queue(queue: WDFQUEUE, request: WDFREQUEST) {
    ffi_void(|| {
        // SAFETY: WDF keeps the queue's parent device alive for this callback.
        if let Some(state) = unsafe { device_state_from_queue(queue) } {
            state
                .pending_application_reads()
                .retain(|pending| pending.request != request.addr());
        }
        // SAFETY: WDF transfers ownership of the canceled request to the callback.
        unsafe { complete_request(request, STATUS_CANCELLED, 0) };
    });
}

unsafe extern "C" fn evt_read_timeout_timer(timer: WDFTIMER) {
    ffi_void(|| {
        // SAFETY: WDF supplies a live timer whose parent is the device selected at creation.
        let parent = unsafe { call_unsafe_wdf_function_binding!(WdfTimerGetParentObject, timer) };
        if parent.is_null() {
            return;
        }
        // SAFETY: The timer is parented directly to this driver's WDFDEVICE.
        let Some(state) = (unsafe { device_state(parent.cast()) }) else {
            return;
        };
        // SAFETY: The application read queue remains live while its parent device timer runs.
        unsafe { service_expired_application_reads(state) };
    });
}

unsafe fn handle_read(
    state: &DeviceState,
    request: WDFREQUEST,
    length: usize,
) -> RequestDisposition {
    // SAFETY: The invoking WDF callback owns this request.
    let Some((role, file)) = (unsafe { request_identity(request) }) else {
        return RequestDisposition::error(STATUS_OBJECT_NAME_INVALID);
    };
    if !state.channel().owns(role, file) {
        return RequestDisposition::error(STATUS_INVALID_DEVICE_REQUEST);
    }
    if length == 0 {
        return RequestDisposition::success(0);
    }
    let available = match role {
        ChannelRole::Application => state.channel().plane.available_to_read(role),
        ChannelRole::Daemon => state.channel().plane.available_for_daemon(),
    };
    match available {
        Ok(0) => {
            let timeout_ms = if role == ChannelRole::Application {
                state.serial().empty_read_timeout_ms(length)
            } else {
                None
            };
            if timeout_ms == Some(0) {
                return RequestDisposition::success(0);
            }
            let queue = state.queue_for(role);
            if queue.is_null() {
                return RequestDisposition::error(STATUS_UNSUCCESSFUL);
            }
            let mut pending =
                (role == ChannelRole::Application).then(|| state.pending_application_reads());
            // SAFETY: The request is framework-owned and target is a valid manual queue.
            let status = unsafe {
                call_unsafe_wdf_function_binding!(WdfRequestForwardToIoQueue, request, queue)
            };
            if nt_success(status) {
                if let Some(pending) = pending.as_mut() {
                    let deadline = timeout_ms.and_then(|milliseconds| {
                        Instant::now().checked_add(Duration::from_millis(milliseconds))
                    });
                    pending.push_back(PendingRead {
                        request: request.addr(),
                        deadline,
                    });
                }
                RequestDisposition::Pending
            } else {
                RequestDisposition::error(status)
            }
        }
        // SAFETY: The invoking WDF callback still owns this request.
        Ok(_) => unsafe { read_available(state, request, role, length) },
        Err(error) => RequestDisposition::error(status_for_error(error)),
    }
}

unsafe fn handle_write(
    state: &DeviceState,
    request: WDFREQUEST,
    length: usize,
) -> RequestDisposition {
    // SAFETY: The invoking WDF callback owns this request.
    let Some((role, file)) = (unsafe { request_identity(request) }) else {
        return RequestDisposition::error(STATUS_OBJECT_NAME_INVALID);
    };
    if !state.channel().owns(role, file) {
        return RequestDisposition::error(STATUS_INVALID_DEVICE_REQUEST);
    }
    if length == 0 {
        return RequestDisposition::success(0);
    }
    let mut buffer: PVOID = ptr::null_mut();
    let mut buffer_length = 0;
    // SAFETY: WDF owns the request and returns a buffer valid until completion.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveInputBuffer,
            request,
            1,
            &raw mut buffer,
            &raw mut buffer_length,
        )
    };
    if !nt_success(status) {
        return RequestDisposition::error(status);
    }
    let usable = length.min(buffer_length);
    // SAFETY: WDF returned at least `buffer_length` readable bytes.
    let input = unsafe { slice::from_raw_parts(buffer.cast_const().cast::<u8>(), usable) };
    let outputs = match role {
        ChannelRole::Application => state
            .protocol()
            .application_data(input)
            .map(|output| vec![output]),
        ChannelRole::Daemon => state.protocol().ingest_daemon(input),
    };
    let outputs = match outputs {
        Ok(outputs) => outputs,
        Err(error) => return RequestDisposition::error(status_for_protocol_error(error)),
    };
    // SAFETY: The invoking callback owns the request and device child queues are live.
    match unsafe { apply_protocol_outputs(state, outputs) } {
        Ok(()) => {
            if role == ChannelRole::Application {
                state.serial().signal_transmit_empty();
                // SAFETY: The wait-request queue belongs to this device.
                unsafe { service_wait_request(state) };
            }
            RequestDisposition::success(usable)
        }
        Err(error) => RequestDisposition::error(status_for_error(error)),
    }
}

// An exhaustive IOCTL table is easier to audit when kept as one dispatch function.
#[allow(clippy::too_many_lines)]
unsafe fn handle_device_control(
    state: &DeviceState,
    request: WDFREQUEST,
    control_code: u32,
) -> RequestDisposition {
    // SAFETY: The invoking WDF callback owns this request.
    let Some((role, file)) = (unsafe { request_identity(request) }) else {
        return RequestDisposition::error(STATUS_OBJECT_NAME_INVALID);
    };
    if role != ChannelRole::Application || !state.channel().owns(role, file) {
        return RequestDisposition::error(STATUS_INVALID_DEVICE_REQUEST);
    }

    match control_code {
        ioctl::SET_BAUD_RATE => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialBaudRate>(request) };
            serial_set(value, SerialState::set_baud_rate, state)
        }
        ioctl::GET_BAUD_RATE => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            unsafe { request_output(request, &state.serial().baud_rate()) }
        }
        ioctl::SET_QUEUE_SIZE => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialQueueSize>(request) };
            serial_set(value, |serial, value| serial.set_queue_size(value), state)
        }
        ioctl::SET_LINE_CONTROL => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialLineControl>(request) };
            serial_set(value, SerialState::set_line_control, state)
        }
        ioctl::GET_LINE_CONTROL => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            unsafe { request_output(request, &state.serial().line_control()) }
        }
        ioctl::SET_TIMEOUTS => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialTimeouts>(request) };
            serial_set(value, SerialState::set_timeouts, state)
        }
        ioctl::GET_TIMEOUTS => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            unsafe { request_output(request, &state.serial().timeouts()) }
        }
        ioctl::SET_CHARS => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialChars>(request) };
            serial_set(value, SerialState::set_chars, state)
        }
        ioctl::GET_CHARS => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            unsafe { request_output(request, &state.serial().chars()) }
        }
        ioctl::SET_HANDFLOW => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            let value = unsafe { request_input::<SerialHandflow>(request) };
            serial_set(value, SerialState::set_handflow, state)
        }
        ioctl::GET_HANDFLOW => {
            // SAFETY: The request is a buffered serial IOCTL with this documented layout.
            unsafe { request_output(request, &state.serial().handflow()) }
        }
        ioctl::SET_BREAK_ON => {
            state.serial().set_break(true);
            emit_modem_control(state)
        }
        ioctl::SET_BREAK_OFF => {
            state.serial().set_break(false);
            emit_modem_control(state)
        }
        ioctl::SET_DTR => {
            state.serial().set_dtr(true);
            emit_modem_control(state)
        }
        ioctl::CLR_DTR => {
            state.serial().set_dtr(false);
            emit_modem_control(state)
        }
        ioctl::SET_RTS => {
            state.serial().set_rts(true);
            emit_modem_control(state)
        }
        ioctl::CLR_RTS => {
            state.serial().set_rts(false);
            emit_modem_control(state)
        }
        ioctl::GET_DTR_RTS | ioctl::GET_MODEM_CONTROL => {
            // SAFETY: The output buffer receives one documented 32-bit serial line bitmap.
            unsafe { request_output(request, &state.serial().modem_output()) }
        }
        ioctl::SET_MODEM_CONTROL => {
            // SAFETY: The input buffer contains one documented 32-bit serial line bitmap.
            match unsafe { request_input::<u32>(request) } {
                Ok(value) => {
                    state.serial().set_modem_output(value);
                    emit_modem_control(state)
                }
                Err(status) => RequestDisposition::error(status),
            }
        }
        ioctl::GET_MODEM_STATUS => {
            // SAFETY: The output buffer receives one documented 32-bit modem status bitmap.
            unsafe { request_output(request, &state.serial().modem_input()) }
        }
        ioctl::SET_FIFO_CONTROL => {
            // SAFETY: The input buffer contains one documented 32-bit FIFO value.
            match unsafe { request_input::<u32>(request) } {
                Ok(value) => {
                    state.serial().set_fifo_control(value);
                    RequestDisposition::success(0)
                }
                Err(status) => RequestDisposition::error(status),
            }
        }
        ioctl::GET_WAIT_MASK => {
            // SAFETY: The output buffer receives one documented 32-bit wait mask.
            unsafe { request_output(request, &state.serial().wait_mask()) }
        }
        ioctl::SET_WAIT_MASK => {
            // SAFETY: `set_wait_mask` owns any request retrieved from the manual wait queue.
            unsafe { set_wait_mask(state, request) }
        }
        ioctl::WAIT_ON_MASK => {
            // SAFETY: The request remains owned by this callback or is forwarded exactly once.
            unsafe { wait_on_mask(state, request) }
        }
        ioctl::PURGE => {
            // SAFETY: `purge_queues` reads the documented mask and owns affected queued requests.
            unsafe { purge_queues(state, request) }
        }
        ioctl::GET_COMM_STATUS => {
            let (input, output) = {
                let channel = state.channel();
                (
                    channel.plane.incoming_len(ChannelRole::Application),
                    channel.plane.outgoing_len(ChannelRole::Application),
                )
            };
            let status: SerialStatus = state.serial().status(input, output);
            // SAFETY: The output buffer receives the documented serial status layout.
            unsafe { request_output(request, &status) }
        }
        ioctl::GET_PROPERTIES => {
            let properties: SerialCommProperties = state.serial().properties();
            // SAFETY: The output buffer receives the documented serial properties layout.
            unsafe { request_output(request, &properties) }
        }
        ioctl::IMMEDIATE_CHAR => {
            // SAFETY: The input buffer contains the immediate byte.
            unsafe { immediate_char(state, request) }
        }
        ioctl::RESET_DEVICE | ioctl::SET_XON | ioctl::SET_XOFF => RequestDisposition::success(0),
        _ => RequestDisposition::error(STATUS_INVALID_DEVICE_REQUEST),
    }
}

fn serial_set<T>(
    value: Result<T, NTSTATUS>,
    setter: impl FnOnce(&mut SerialState, T) -> Result<(), SerialStateError>,
    state: &DeviceState,
) -> RequestDisposition {
    let value = match value {
        Ok(value) => value,
        Err(status) => return RequestDisposition::error(status),
    };
    let set_result = setter(&mut state.serial(), value);
    match set_result {
        Ok(()) => emit_serial_config(state),
        Err(error) => RequestDisposition::error(status_for_serial_error(error)),
    }
}

fn emit_serial_config(state: &DeviceState) -> RequestDisposition {
    let (baud, line, timeouts) = {
        let serial = state.serial();
        (serial.baud_rate(), serial.line_control(), serial.timeouts())
    };
    let output = state.protocol().serial_config(
        baud.baud_rate,
        line.word_length,
        line.parity,
        line.stop_bits,
        timeouts.read_total_timeout_constant,
        timeouts.write_total_timeout_constant,
    );
    apply_optional_protocol_output(state, output)
}

fn emit_modem_control(state: &DeviceState) -> RequestDisposition {
    let mask = state.serial().modem_control_mask();
    let output = state.protocol().modem_control(mask);
    apply_optional_protocol_output(state, output)
}

fn apply_optional_protocol_output(
    state: &DeviceState,
    output: Result<Option<ProtocolOutput>, DriverProtocolError>,
) -> RequestDisposition {
    match output {
        Ok(Some(output)) => {
            // SAFETY: Every caller runs inside a live WDF device callback.
            match unsafe { apply_protocol_outputs(state, vec![output]) } {
                Ok(()) => RequestDisposition::success(0),
                Err(error) => RequestDisposition::error(status_for_error(error)),
            }
        }
        Ok(None) => RequestDisposition::success(0),
        Err(error) => RequestDisposition::error(status_for_protocol_error(error)),
    }
}

unsafe fn request_input<T: Copy>(request: WDFREQUEST) -> Result<T, NTSTATUS> {
    let mut buffer: PVOID = ptr::null_mut();
    let mut length = 0;
    // SAFETY: WDF owns the request and returns a buffer valid until request completion.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveInputBuffer,
            request,
            size_of::<T>(),
            &raw mut buffer,
            &raw mut length,
        )
    };
    if !nt_success(status) {
        return Err(status);
    }
    if buffer.is_null() || length < size_of::<T>() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    // SAFETY: WDF returned at least `size_of::<T>()` readable bytes; unaligned handles any layout.
    Ok(unsafe { ptr::read_unaligned(buffer.cast::<T>()) })
}

unsafe fn request_output<T: Copy>(request: WDFREQUEST, value: &T) -> RequestDisposition {
    let mut buffer: PVOID = ptr::null_mut();
    let mut length = 0;
    // SAFETY: WDF owns the request and returns a buffer valid until request completion.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveOutputBuffer,
            request,
            size_of::<T>(),
            &raw mut buffer,
            &raw mut length,
        )
    };
    if !nt_success(status) {
        return RequestDisposition::error(status);
    }
    if buffer.is_null() || length < size_of::<T>() {
        return RequestDisposition::error(STATUS_INVALID_PARAMETER);
    }
    // SAFETY: WDF returned at least `size_of::<T>()` writable bytes; unaligned handles any layout.
    unsafe { ptr::write_unaligned(buffer.cast::<T>(), *value) };
    RequestDisposition::success(size_of::<T>())
}

unsafe fn set_wait_mask(state: &DeviceState, request: WDFREQUEST) -> RequestDisposition {
    // SAFETY: The input buffer contains one documented 32-bit wait mask.
    let mask = match unsafe { request_input::<u32>(request) } {
        Ok(mask) => mask,
        Err(status) => return RequestDisposition::error(status),
    };
    let set_result = state.serial().set_wait_mask(mask);
    if let Err(error) = set_result {
        return RequestDisposition::error(status_for_serial_error(error));
    }
    let mut pending: WDFREQUEST = ptr::null_mut();
    // SAFETY: The manual queue belongs to this device and output storage is valid.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueRetrieveNextRequest,
            state.wait_queue(),
            &raw mut pending,
        )
    };
    if nt_success(status) {
        // SAFETY: Retrieval transferred ownership of the pending wait request.
        let disposition = unsafe { request_output(pending, &0_u32) };
        // SAFETY: The retrieved request is no longer owned by the manual queue.
        unsafe { finish_request(pending, disposition) };
    }
    RequestDisposition::success(0)
}

unsafe fn wait_on_mask(state: &DeviceState, request: WDFREQUEST) -> RequestDisposition {
    if state.serial().wait_mask() == 0 {
        return RequestDisposition::error(STATUS_INVALID_PARAMETER);
    }
    let ready_events = state.serial().take_wait_events();
    if let Some(events) = ready_events {
        // SAFETY: The output buffer receives one documented 32-bit event mask.
        return unsafe { request_output(request, &events) };
    }
    let mut previous: WDFREQUEST = ptr::null_mut();
    // SAFETY: The manual queue belongs to this device and output storage is valid.
    let retrieved = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueRetrieveNextRequest,
            state.wait_queue(),
            &raw mut previous,
        )
    };
    if nt_success(retrieved) {
        // SAFETY: Retrieval transferred ownership of the superseded request.
        unsafe { complete_request(previous, STATUS_UNSUCCESSFUL, 0) };
    }
    // SAFETY: The request is framework-owned and target is a valid manual queue.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfRequestForwardToIoQueue, request, state.wait_queue(),)
    };
    if nt_success(status) {
        RequestDisposition::Pending
    } else {
        RequestDisposition::error(status)
    }
}

unsafe fn service_wait_request(state: &DeviceState) {
    let Some(events) = state.serial().take_wait_events() else {
        return;
    };
    let mut request: WDFREQUEST = ptr::null_mut();
    // SAFETY: The manual queue belongs to this device and output storage is valid.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueRetrieveNextRequest,
            state.wait_queue(),
            &raw mut request,
        )
    };
    if nt_success(status) {
        // SAFETY: Retrieval transferred the queued request and its buffer remains valid.
        let disposition = unsafe { request_output(request, &events) };
        // SAFETY: The retrieved request is no longer owned by the queue.
        unsafe { finish_request(request, disposition) };
    }
}

unsafe fn purge_queues(state: &DeviceState, request: WDFREQUEST) -> RequestDisposition {
    // SAFETY: The input buffer contains one documented 32-bit purge mask.
    let mask = match unsafe { request_input::<u32>(request) } {
        Ok(mask) => mask,
        Err(status) => return RequestDisposition::error(status),
    };
    if let Err(error) = SerialState::validate_purge(mask) {
        return RequestDisposition::error(status_for_serial_error(error));
    }
    {
        let mut channel = state.channel();
        if mask & purge::RX_CLEAR != 0 {
            channel.plane.clear_incoming(ChannelRole::Application);
        }
        if mask & purge::TX_CLEAR != 0 {
            channel.plane.clear_outgoing(ChannelRole::Application);
        }
    }
    if mask & purge::RX_ABORT != 0 {
        // SAFETY: This manual queue belongs to the application side of this device.
        unsafe { drain_pending_reads(state.queue_for(ChannelRole::Application), STATUS_CANCELLED) };
    }
    let purge_output = state.protocol().purge(mask);
    match purge_output {
        Ok(Some(output)) => {
            // SAFETY: The daemon queue belongs to this live device.
            if let Err(error) = unsafe { apply_protocol_outputs(state, vec![output]) } {
                return RequestDisposition::error(status_for_error(error));
            }
        }
        Ok(None) => {}
        Err(error) => return RequestDisposition::error(status_for_protocol_error(error)),
    }
    RequestDisposition::success(0)
}

unsafe fn immediate_char(state: &DeviceState, request: WDFREQUEST) -> RequestDisposition {
    // SAFETY: The input buffer contains the documented immediate byte.
    let byte = match unsafe { request_input::<u8>(request) } {
        Ok(byte) => byte,
        Err(status) => return RequestDisposition::error(status),
    };
    let output = state.protocol().application_data(&[byte]);
    match output {
        Ok(output) => {
            state.serial().signal_transmit_empty();
            // SAFETY: The daemon queue belongs to this live device.
            match unsafe { apply_protocol_outputs(state, vec![output]) } {
                Ok(()) => RequestDisposition::success(0),
                Err(error) => RequestDisposition::error(status_for_error(error)),
            }
        }
        Err(error) => RequestDisposition::error(status_for_protocol_error(error)),
    }
}

unsafe fn read_available(
    state: &DeviceState,
    request: WDFREQUEST,
    role: ChannelRole,
    requested: usize,
) -> RequestDisposition {
    let mut buffer: PVOID = ptr::null_mut();
    let mut buffer_length = 0;
    // SAFETY: WDF owns the request and returns a buffer valid until completion.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveOutputBuffer,
            request,
            1,
            &raw mut buffer,
            &raw mut buffer_length,
        )
    };
    if !nt_success(status) {
        return RequestDisposition::error(status);
    }
    let usable = requested.min(buffer_length);
    // SAFETY: WDF returned at least `buffer_length` writable bytes.
    let output = unsafe { slice::from_raw_parts_mut(buffer.cast::<u8>(), usable) };
    let read_result = match role {
        ChannelRole::Application => state.channel().plane.read(role, output),
        ChannelRole::Daemon => state.channel().plane.read_for_daemon(output),
    };
    match read_result {
        Ok(read) => {
            if role == ChannelRole::Application {
                let update = state.protocol().application_bytes_released(read);
                if let Ok(Some(update)) = update {
                    // SAFETY: Device child queues remain live for the duration of this callback.
                    let _ = unsafe { apply_protocol_outputs(state, vec![update]) };
                }
            }
            RequestDisposition::success(read)
        }
        Err(error) => RequestDisposition::error(status_for_error(error)),
    }
}

unsafe fn apply_protocol_outputs(
    state: &DeviceState,
    outputs: Vec<ProtocolOutput>,
) -> Result<(), DataPlaneError> {
    for output in outputs {
        match output {
            ProtocolOutput::ToDaemon(bytes) => {
                state.channel().plane.write_to_daemon(&bytes)?;
                // SAFETY: The daemon read queue belongs to this device.
                unsafe { service_pending_reads(state, ChannelRole::Daemon) };
            }
            ProtocolOutput::ToApplication(bytes) => {
                let queued = {
                    let mut channel = state.channel();
                    channel.plane.write(ChannelRole::Daemon, &bytes)?;
                    channel.plane.incoming_len(ChannelRole::Application)
                };
                state.serial().signal_receive(&bytes, queued);
                // SAFETY: The application read queue belongs to this device.
                unsafe { service_pending_reads(state, ChannelRole::Application) };
                // SAFETY: The wait-request queue belongs to this device.
                unsafe { service_wait_request(state) };
            }
            ProtocolOutput::ModemStatus(mask) => {
                state.serial().set_modem_input(mask);
                // SAFETY: The wait-request queue belongs to this device.
                unsafe { service_wait_request(state) };
            }
        }
    }
    Ok(())
}

unsafe fn service_pending_reads(state: &DeviceState, role: ChannelRole) {
    let queue = state.queue_for(role);
    let mut pending_application =
        (role == ChannelRole::Application).then(|| state.pending_application_reads());
    loop {
        let available = match role {
            ChannelRole::Application => state.channel().plane.available_to_read(role),
            ChannelRole::Daemon => state.channel().plane.available_for_daemon(),
        };
        if available.map_or(true, |available| available == 0) {
            break;
        }
        let mut request: WDFREQUEST = ptr::null_mut();
        // SAFETY: Queue is a valid manual queue and request is valid output storage.
        let status = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfIoQueueRetrieveNextRequest,
                queue,
                &raw mut request,
            )
        };
        if !nt_success(status) {
            break;
        }
        if let Some(pending) = pending_application.as_mut() {
            pending.retain(|entry| entry.request != request.addr());
        }
        // SAFETY: Retrieval transfers the queued request to this driver.
        let disposition = unsafe { read_available(state, request, role, usize::MAX) };
        // SAFETY: The retrieved request is no longer owned by the manual queue.
        unsafe { finish_request(request, disposition) };
    }
}

unsafe fn service_expired_application_reads(state: &DeviceState) {
    let queue = state.queue_for(ChannelRole::Application);
    if queue.is_null() {
        return;
    }
    let mut pending = state.pending_application_reads();
    loop {
        let expired = pending
            .front()
            .and_then(|entry| entry.deadline)
            .is_some_and(|deadline| deadline <= Instant::now());
        if !expired {
            break;
        }
        let expected = pending.front().map(|entry| entry.request);
        let mut request: WDFREQUEST = ptr::null_mut();
        // SAFETY: Queue is a valid manual queue and request is valid output storage.
        let status = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfIoQueueRetrieveNextRequest,
                queue,
                &raw mut request,
            )
        };
        if !nt_success(status) {
            pending.clear();
            break;
        }
        pending.pop_front();
        if expected != Some(request.addr()) {
            pending.retain(|entry| entry.request != request.addr());
        }
        drop(pending);
        // SAFETY: Retrieval transfers ownership of this expired request to the driver.
        unsafe { complete_request(request, STATUS_SUCCESS, 0) };
        pending = state.pending_application_reads();
    }
}

unsafe fn drain_pending_reads(queue: WDFQUEUE, status: NTSTATUS) {
    if queue.is_null() {
        return;
    }
    loop {
        let mut request: WDFREQUEST = ptr::null_mut();
        // SAFETY: Queue is a valid manual queue and request is valid output storage.
        let retrieved = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfIoQueueRetrieveNextRequest,
                queue,
                &raw mut request,
            )
        };
        if !nt_success(retrieved) {
            break;
        }
        // SAFETY: Retrieval transfers ownership of the request to this driver.
        unsafe { complete_request(request, status, 0) };
    }
}

unsafe fn request_identity(request: WDFREQUEST) -> Option<(ChannelRole, WDFFILEOBJECT)> {
    // SAFETY: Caller supplies a valid WDF request.
    let file = unsafe { call_unsafe_wdf_function_binding!(WdfRequestGetFileObject, request) };
    if file.is_null() {
        return None;
    }
    // SAFETY: WDF associates this file object with the current request.
    unsafe { role_from_file(file) }.map(|role| (role, file))
}

unsafe fn role_from_file(file: WDFFILEOBJECT) -> Option<ChannelRole> {
    // SAFETY: Caller supplies a valid WDF file object.
    let name = unsafe { call_unsafe_wdf_function_binding!(WdfFileObjectGetFileName, file) };
    if name.is_null() {
        return None;
    }
    // SAFETY: WDF returns a valid counted string for the file object's lifetime.
    let name = unsafe { &*name };
    if name.Buffer.is_null() || name.Length % 2 != 0 {
        return None;
    }
    // SAFETY: Length is bytes and was checked for complete UTF-16 code units.
    let units =
        unsafe { slice::from_raw_parts(name.Buffer.cast_const(), usize::from(name.Length / 2)) };
    role_from_name(units)
}

fn role_from_name(name: &[u16]) -> Option<ChannelRole> {
    let segment = name
        .rsplit(|unit| *unit == u16::from(b'\\') || *unit == u16::from(b'/'))
        .next()?;
    if segment.is_empty() || utf16_eq_ascii_case(segment, b"application") {
        Some(ChannelRole::Application)
    } else if utf16_eq_ascii_case(segment, b"daemon") {
        Some(ChannelRole::Daemon)
    } else {
        None
    }
}

fn utf16_eq_ascii_case(units: &[u16], ascii: &[u8]) -> bool {
    units.len() == ascii.len()
        && units.iter().zip(ascii).all(|(left, right)| {
            u8::try_from(*left).is_ok_and(|value| value.eq_ignore_ascii_case(right))
        })
}

fn dispatch_request(request: WDFREQUEST, action: impl FnOnce() -> RequestDisposition) {
    let disposition = catch_unwind(AssertUnwindSafe(action))
        .unwrap_or_else(|_| RequestDisposition::error(STATUS_UNSUCCESSFUL));
    // SAFETY: Invoking callback owns the request unless disposition says it was forwarded.
    unsafe { finish_request(request, disposition) };
}

unsafe fn finish_request(request: WDFREQUEST, disposition: RequestDisposition) {
    if let RequestDisposition::Complete {
        status,
        information,
    } = disposition
    {
        // SAFETY: Caller owns this request and completes it exactly once.
        unsafe { complete_request(request, status, information) };
    }
}

unsafe fn complete_request(request: WDFREQUEST, status: NTSTATUS, information: usize) {
    let information = ULONG_PTR::try_from(information).unwrap_or(ULONG_PTR::MAX);
    // SAFETY: Caller owns this request. WDF accepts status and byte count by value.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestCompleteWithInformation,
            request,
            status,
            information,
        );
    }
}

unsafe fn initialize_device_context_type() {
    let type_info = &raw mut DEVICE_CONTEXT_TYPE_INFO;
    let size = struct_size::<WDF_OBJECT_CONTEXT_TYPE_INFO>();
    // SAFETY: DriverEntry is the only writer and initializes this field before publishing it.
    unsafe { (*type_info).Size = size };
    // SAFETY: DriverEntry is the only writer and publishes the immutable self-pointer before WDF
    // receives the type information.
    unsafe { (*type_info).UniqueType = type_info.cast_const() };
}

fn device_context_type_info() -> *const WDF_OBJECT_CONTEXT_TYPE_INFO {
    &raw const DEVICE_CONTEXT_TYPE_INFO
}

unsafe fn device_context(device: WDFDEVICE) -> Option<*mut DeviceContext> {
    // SAFETY: Caller supplies a live WDF device. The type-info pointer is initialized before WDF
    // creates any device objects and remains process-static.
    let context = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfObjectGetTypedContextWorker,
            device.cast::<c_void>(),
            device_context_type_info(),
        )
    };
    (!context.is_null()).then(|| context.cast::<DeviceContext>())
}

unsafe fn device_state<'device>(device: WDFDEVICE) -> Option<&'device DeviceState> {
    // SAFETY: Caller guarantees that `device` remains alive for the returned borrow.
    let context = unsafe { device_context(device) }?;
    // SAFETY: The context is initialized before interfaces and queues can invoke callbacks.
    let state = unsafe { (*context).state };
    // SAFETY: Device context destroy owns and frees this pointer after all child callbacks finish.
    unsafe { state.as_ref() }
}

unsafe fn device_state_from_file<'device>(file: WDFFILEOBJECT) -> Option<&'device DeviceState> {
    // SAFETY: Caller supplies a live WDF file object.
    let device = unsafe { call_unsafe_wdf_function_binding!(WdfFileObjectGetDevice, file) };
    if device.is_null() {
        return None;
    }
    // SAFETY: The file object keeps its parent device alive for the callback's duration.
    unsafe { device_state(device) }
}

unsafe fn device_state_from_queue<'device>(queue: WDFQUEUE) -> Option<&'device DeviceState> {
    // SAFETY: Caller supplies a live WDF queue.
    let device = unsafe { call_unsafe_wdf_function_binding!(WdfIoQueueGetDevice, queue) };
    if device.is_null() {
        return None;
    }
    // SAFETY: The queue keeps its parent device alive for the callback's duration.
    unsafe { device_state(device) }
}

unsafe extern "C" fn evt_device_context_destroy(object: WDFOBJECT) {
    ffi_void(|| {
        // SAFETY: WDF calls this for the device object whose context is being destroyed.
        let Some(context) = (unsafe { device_context(object.cast()) }) else {
            return;
        };
        // SAFETY: WDF supplies this context pointer for the object being destroyed.
        let state_slot = unsafe { &raw mut (*context).state };
        // SAFETY: This destroy callback is the sole consumer of the state pointer.
        let state = unsafe { ptr::replace(state_slot, ptr::null_mut()) };
        if !state.is_null() {
            // SAFETY: `state` originated from exactly one `Box::into_raw` during device creation.
            unsafe { drop(Box::from_raw(state)) };
        }
    });
}

const fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

const fn status_for_error(error: DataPlaneError) -> NTSTATUS {
    match error {
        DataPlaneError::AlreadyOpen(_) => STATUS_SHARING_VIOLATION,
        DataPlaneError::NotOpen(_) => STATUS_INVALID_DEVICE_REQUEST,
        DataPlaneError::PeerNotOpen(_) => STATUS_DEVICE_NOT_CONNECTED,
        DataPlaneError::BufferFull { .. } => STATUS_BUFFER_OVERFLOW,
    }
}

const fn status_for_serial_error(_error: SerialStateError) -> NTSTATUS {
    STATUS_INVALID_PARAMETER
}

const fn status_for_protocol_error(error: DriverProtocolError) -> NTSTATUS {
    match error {
        DriverProtocolError::NotReady
        | DriverProtocolError::NotAttached
        | DriverProtocolError::Session => STATUS_DEVICE_NOT_CONNECTED,
        DriverProtocolError::Credit | DriverProtocolError::ReceiveWindow => STATUS_BUFFER_OVERFLOW,
        DriverProtocolError::Framing
        | DriverProtocolError::Version
        | DriverProtocolError::Features
        | DriverProtocolError::FrameLimit
        | DriverProtocolError::WrongEndpoint
        | DriverProtocolError::Sequence
        | DriverProtocolError::Encoding => STATUS_INVALID_PARAMETER,
    }
}

fn struct_size<T>() -> ULONG {
    u32::try_from(size_of::<T>()).unwrap_or(ULONG::MAX)
}

fn unicode_string(units_with_null: &[u16]) -> UNICODE_STRING {
    let content_units = units_with_null.len().saturating_sub(1);
    let length = content_units.saturating_mul(size_of::<u16>());
    let maximum_length = units_with_null.len().saturating_mul(size_of::<u16>());
    UNICODE_STRING {
        Length: u16::try_from(length).unwrap_or(u16::MAX),
        MaximumLength: u16::try_from(maximum_length).unwrap_or(u16::MAX),
        Buffer: units_with_null.as_ptr().cast_mut(),
    }
}

fn unicode_string_buffer(buffer: &mut [u16]) -> UNICODE_STRING {
    UNICODE_STRING {
        Length: 0,
        MaximumLength: u16::try_from(buffer.len().saturating_mul(size_of::<u16>()))
            .unwrap_or(u16::MAX),
        Buffer: buffer.as_mut_ptr(),
    }
}

unsafe extern "C" fn evt_driver_unload(_driver: WDFDRIVER) {
    ffi_void(|| println!("CatHub UMDF proof-of-concept unloaded"));
}

fn ffi_status(action: impl FnOnce() -> NTSTATUS) -> NTSTATUS {
    catch_unwind(AssertUnwindSafe(action)).unwrap_or(STATUS_UNSUCCESSFUL)
}

fn ffi_void(action: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(action));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_is_contained_at_status_boundary() {
        let status = ffi_status(|| panic!("test panic"));
        assert_eq!(status, STATUS_UNSUCCESSFUL);
    }

    #[test]
    fn successful_status_crosses_boundary() {
        assert_eq!(ffi_status(|| 0), 0);
    }

    #[test]
    fn role_is_parsed_from_reference_name() {
        let application: Vec<u16> = "\\application".encode_utf16().collect();
        let daemon: Vec<u16> = "DAEMON".encode_utf16().collect();
        let unknown: Vec<u16> = "control".encode_utf16().collect();

        assert_eq!(role_from_name(&[]), Some(ChannelRole::Application));
        assert_eq!(role_from_name(&application), Some(ChannelRole::Application));
        assert_eq!(role_from_name(&daemon), Some(ChannelRole::Daemon));
        assert_eq!(role_from_name(&unknown), None);
    }

    #[test]
    fn device_states_keep_transport_and_queues_independent() {
        let first = DeviceState::new();
        let second = DeviceState::new();

        {
            let mut channel = first.channel();
            assert_eq!(channel.plane.open(ChannelRole::Application), Ok(1));
            assert_eq!(channel.plane.open(ChannelRole::Daemon), Ok(1));
            assert_eq!(channel.plane.write(ChannelRole::Application, b"one"), Ok(3));
            drop(channel);
        }
        {
            let mut channel = second.channel();
            assert_eq!(channel.plane.open(ChannelRole::Application), Ok(1));
            assert_eq!(channel.plane.open(ChannelRole::Daemon), Ok(1));
            drop(channel);
        }

        assert_eq!(
            first.channel().plane.available_to_read(ChannelRole::Daemon),
            Ok(3)
        );
        assert_eq!(
            second
                .channel()
                .plane
                .available_to_read(ChannelRole::Daemon),
            Ok(0)
        );
        assert!(first.queue_for(ChannelRole::Application).is_null());
        assert!(second.queue_for(ChannelRole::Daemon).is_null());
    }
}
