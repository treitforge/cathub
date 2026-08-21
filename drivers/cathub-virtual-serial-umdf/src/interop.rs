//! Auditable Windows/WDF FFI boundary.

use std::{
    ffi::c_void,
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicPtr, Ordering},
    },
};

use wdk::println;
use wdk_sys::{
    _WDF_EXECUTION_LEVEL, _WDF_FILEOBJECT_CLASS, _WDF_IO_QUEUE_DISPATCH_TYPE,
    _WDF_SYNCHRONIZATION_SCOPE, _WDF_TRI_STATE, BOOLEAN, GUID, NTSTATUS, PCUNICODE_STRING,
    PDRIVER_OBJECT, PVOID, ULONG, ULONG_PTR, UNICODE_STRING, WDF_DRIVER_CONFIG,
    WDF_FILEOBJECT_CONFIG, WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE, WDF_NO_OBJECT_ATTRIBUTES,
    WDF_OBJECT_ATTRIBUTES, WDF_OBJECT_CONTEXT_TYPE_INFO, WDFDEVICE, WDFDEVICE_INIT, WDFDRIVER,
    WDFFILEOBJECT, WDFOBJECT, WDFQUEUE, WDFQUEUE__, WDFREQUEST, call_unsafe_wdf_function_binding,
};

use crate::data_plane::{ChannelRole, DEFAULT_BUFFER_CAPACITY, DataPlaneError, EndpointDataPlane};

const STATUS_SUCCESS: NTSTATUS = 0;
const STATUS_UNSUCCESSFUL: NTSTATUS = -1_073_741_823;
const STATUS_INVALID_DEVICE_REQUEST: NTSTATUS = -1_073_741_808;
const STATUS_OBJECT_NAME_INVALID: NTSTATUS = -1_073_741_773;
const STATUS_SHARING_VIOLATION: NTSTATUS = -1_073_741_757;
const STATUS_DEVICE_NOT_CONNECTED: NTSTATUS = -1_073_741_667;
const STATUS_CANCELLED: NTSTATUS = -1_073_741_536;
const STATUS_BUFFER_OVERFLOW: NTSTATUS = -2_147_483_643;
const TRUE: BOOLEAN = 1;

const APPLICATION_REFERENCE: [u16; 12] = [97, 112, 112, 108, 105, 99, 97, 116, 105, 111, 110, 0];
const DAEMON_REFERENCE: [u16; 7] = [100, 97, 101, 109, 111, 110, 0];

/// Private proof-of-concept interface, `{0084BDDE-9F40-4A6A-AF84-0F4E46B70901}`.
static CATHUB_POC_INTERFACE_GUID: GUID = GUID {
    Data1: 0x0084_BDDE,
    Data2: 0x9F40,
    Data3: 0x4A6A,
    Data4: [0xAF, 0x84, 0x0F, 0x4E, 0x46, 0xB7, 0x09, 0x01],
};

#[repr(C)]
struct DeviceContext {
    state: *mut DeviceState,
}

struct DeviceState {
    channel: Mutex<ChannelState>,
    application_reads: AtomicPtr<WDFQUEUE__>,
    daemon_reads: AtomicPtr<WDFQUEUE__>,
}

impl DeviceState {
    fn new() -> Self {
        Self {
            channel: Mutex::new(ChannelState::new()),
            application_reads: AtomicPtr::new(ptr::null_mut()),
            daemon_reads: AtomicPtr::new(ptr::null_mut()),
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
    let state = Box::into_raw(Box::new(DeviceState::new()));
    // SAFETY: The context is exclusively initialized before queues or interfaces publish device.
    unsafe { (*context).state = state };
    // SAFETY: `state` remains owned by the WDF device context until its destroy callback.
    let state = unsafe { &*state };
    // SAFETY: The WDF device was successfully created above.
    let status = unsafe { configure_queues(device, state) };
    if !nt_success(status) {
        return status;
    }
    // SAFETY: The device exists and registration consumes both counted strings synchronously.
    unsafe { register_interfaces(device) }
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
    let application = unicode_string(&APPLICATION_REFERENCE);
    // SAFETY: All pointers remain valid through this synchronous call.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &raw const CATHUB_POC_INTERFACE_GUID,
            &raw const application,
        )
    };
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
        if state.channel().close_if_owner(role, file) {
            println!("CatHub PoC {role:?} handle cleaned up");
            // SAFETY: This handle refers to a manual queue owned by this device.
            unsafe {
                drain_pending_reads(state.queue_for(ChannelRole::Application), STATUS_CANCELLED);
            };
            // SAFETY: This handle refers to a manual queue owned by this device.
            unsafe { drain_pending_reads(state.queue_for(ChannelRole::Daemon), STATUS_CANCELLED) };
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

unsafe extern "C" fn evt_io_default(_queue: WDFQUEUE, request: WDFREQUEST) {
    ffi_void(|| {
        // SAFETY: WDF transfers ownership of this unsupported request to the callback.
        unsafe { complete_request(request, STATUS_INVALID_DEVICE_REQUEST, 0) };
    });
}

unsafe extern "C" fn evt_io_canceled_on_queue(_queue: WDFQUEUE, request: WDFREQUEST) {
    ffi_void(|| {
        // SAFETY: WDF transfers ownership of the canceled request to the callback.
        unsafe { complete_request(request, STATUS_CANCELLED, 0) };
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
    let available = state.channel().plane.available_to_read(role);
    match available {
        Ok(0) => {
            let queue = state.queue_for(role);
            if queue.is_null() {
                return RequestDisposition::error(STATUS_UNSUCCESSFUL);
            }
            // SAFETY: The request is framework-owned and target is a valid manual queue.
            let status = unsafe {
                call_unsafe_wdf_function_binding!(WdfRequestForwardToIoQueue, request, queue)
            };
            if nt_success(status) {
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
    let write_result = state.channel().plane.write(role, input);
    match write_result {
        Ok(written) => {
            // SAFETY: The peer's pending queue belongs to this device.
            unsafe { service_pending_reads(state, role.peer()) };
            RequestDisposition::success(written)
        }
        Err(error) => RequestDisposition::error(status_for_error(error)),
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
    let read_result = state.channel().plane.read(role, output);
    match read_result {
        Ok(read) => RequestDisposition::success(read),
        Err(error) => RequestDisposition::error(status_for_error(error)),
    }
}

unsafe fn service_pending_reads(state: &DeviceState, role: ChannelRole) {
    let queue = state.queue_for(role);
    loop {
        if state
            .channel()
            .plane
            .available_to_read(role)
            .map_or(true, |available| available == 0)
        {
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
        // SAFETY: Retrieval transfers the queued request to this driver.
        let disposition = unsafe { read_available(state, request, role, usize::MAX) };
        // SAFETY: The retrieved request is no longer owned by the manual queue.
        unsafe { finish_request(request, disposition) };
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
    if utf16_eq_ascii_case(segment, b"application") {
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
