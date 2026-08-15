//! Auditable Windows/WDF FFI boundary.

use std::panic::{AssertUnwindSafe, catch_unwind};

use wdk::println;
use wdk_sys::{
    NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, ULONG, WDF_DRIVER_CONFIG, WDF_NO_HANDLE,
    WDF_NO_OBJECT_ATTRIBUTES, WDFDEVICE, WDFDEVICE_INIT, WDFDRIVER,
    call_unsafe_wdf_function_binding,
};

const STATUS_UNSUCCESSFUL: NTSTATUS = -1_073_741_823;

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

    let config_size =
        u32::try_from(core::mem::size_of::<WDF_DRIVER_CONFIG>()).unwrap_or(ULONG::MAX);
    let mut driver_config = WDF_DRIVER_CONFIG {
        Size: config_size,
        EvtDriverDeviceAdd: Some(evt_driver_device_add),
        EvtDriverUnload: Some(evt_driver_unload),
        ..WDF_DRIVER_CONFIG::default()
    };
    let driver_attributes = WDF_NO_OBJECT_ATTRIBUTES;
    let driver_handle_output = WDF_NO_HANDLE.cast::<WDFDRIVER>();

    // SAFETY: WDF owns the two input pointers. The attributes and output handle may be null,
    // and `driver_config` remains valid for the duration of the call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            driver_attributes,
            &raw mut driver_config,
            driver_handle_output,
        )
    }
}

extern "C" fn evt_driver_device_add(
    _driver: WDFDRIVER,
    device_init: *mut WDFDEVICE_INIT,
) -> NTSTATUS {
    ffi_status(|| {
        // SAFETY: WDF supplies a valid, exclusively owned device-init pointer to this callback.
        unsafe { create_device(device_init) }
    })
}

unsafe fn create_device(mut device_init: *mut WDFDEVICE_INIT) -> NTSTATUS {
    let mut device: WDFDEVICE = WDF_NO_HANDLE.cast();

    // SAFETY: WDF supplies `device_init`; this callback consumes it exactly once on success.
    // Null object attributes are allowed and `device` is a valid output location.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreate,
            &raw mut device_init,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut device,
        )
    }
}

extern "C" fn evt_driver_unload(_driver: WDFDRIVER) {
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
}
