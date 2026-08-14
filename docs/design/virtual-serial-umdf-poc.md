# Virtual serial UMDF proof of concept

## Scope and status

This is the isolated proof-of-concept branch for issue #6.
Phase 1 defined the private framing contract and conformance harness.
This milestone pins and scaffolds the pure Rust UMDF 2 binary before any device is installed.

The scaffold is not a virtual COM driver yet.
Its INF uses the Windows Sample class and the driver only calls `WdfDriverCreate` and
`WdfDeviceCreate`.
Keeping the device non-serial prevents an incomplete driver from appearing usable to N1MM or
another station application.

## Reproducible inputs

| Input | Pin |
|---|---|
| Rust | `1.91.0` |
| Target | `x86_64-pc-windows-msvc` |
| `windows-drivers-rs` | `8e88dd899d9fa988df841e08cc01e9f663e5a415` |
| Driver model | UMDF 2.33 |
| Minimum Windows family | Windows 11 x64 |

The Microsoft Rust driver repository is still experimental.
CatHub therefore pins a commit instead of a branch or loose crate version.
The driver is a separate workspace because the upstream build supports only one WDK configuration
in a Cargo build graph.

## Local environment audit

Audit date: 2026-08-14.

Available on the development station:

- Rust and Cargo 1.91.0 for `x86_64-pc-windows-msvc`
- LLVM/Clang 21.1.2
- Visual Studio 2026 Build Tools and Visual Studio 2022
- Windows SDK directories through 10.0.26100.0
- N1MM Logger+ and isolated com0com pairs including COM20/COM21

Missing from the development station:

- WDF headers such as `wdf.h`
- WDK packaging tools `inf2cat`, `infverif`, and `stampinf`
- `cargo-make`

The SDK does provide `signtool`, but that does not make it a WDK environment.
No driver, certificate, or additional tool was installed during this audit.

## Microsoft sample inventory

The API reference is Microsoft VirtualSerial2 at commit
`717778a20ba4dd2440fe609f69153a1f8a64f597`.
The source remains upstream; CatHub does not copy the C implementation.

### Callbacks and queues

| Area | VirtualSerial2 behavior | CatHub PoC state |
|---|---|---|
| Driver | `DriverEntry`, `EVT_WDF_DRIVER_DEVICE_ADD` | Entry and device-add skeleton |
| Device | Device context and cleanup callback | Planned with endpoint state |
| Default queue | Parallel read, write, and device-control callbacks | Planned |
| Pending reads | Manual queue | Planned with bounded buffering and cancellation |
| Pending event wait | Separate manual queue | Planned, one outstanding wait policy required |
| Cleanup | Device cleanup releases COM mapping | Planned with daemon detach and fail-safe revocation |

### Serial controls implemented by the sample

VirtualSerial2 handles these controls in its device-control switch:

- baud rate get/set
- line control get/set
- timeout get/set
- modem-control get/set and FIFO control
- wait-mask set and wait-on-mask
- queue-size set
- DTR set
- RTS set/clear
- XON/XOFF set
- serial characters get/set
- handflow get/set
- device reset

The sample header defines more serial IOCTLs than its switch handles.
CatHub must not infer support from a definition alone.
The Phase 1 conformance profile additionally requires purge, queue/error status, cancellation,
overlapped I/O, and predictable unsupported-operation errors.

### Device metadata used by the sample

VirtualSerial2 uses the Ports class, `FILE_DEVICE_SERIAL_PORT`, `GUID_DEVINTERFACE_COMPORT`, a COM
name stored in the device map, and a symbolic link.
Its Windows 11 INF includes `WUDFRD.inf` and configures a UMDF service hosted through the reflector.

CatHub will add a separate ACL-restricted private device interface and a stable endpoint ID.
Those are CatHub requirements, not behaviors supplied by VirtualSerial2.

## Unsafe and FFI inventory

The initial unsafe surface is one module, `src/interop.rs`:

- exported `DriverEntry`
- `WdfDriverCreate`
- device-add callback and `WdfDeviceCreate`
- unload callback registration

Every ABI entry catches panics.
There are no raw request buffers, context casts, ownership transfers, or asynchronous request races
yet.
Each of those categories must be added to this inventory when introduced.

## Gates before the INF becomes a Ports-class package

1. Install a supported WDK in the isolated development environment.
2. Build and package this sample-class driver with warnings treated as errors.
3. Add device and queue contexts with typed accessors.
4. Implement bounded application read/write queues, cancellation, cleanup, and timeout state.
5. Register `GUID_DEVINTERFACE_COMPORT` and a private CatHub interface.
6. Add the COM mapping only after restart and removal are deterministic.
7. Install only on the isolated test target with development-signing policy documented.
8. Run the `n1mm-radio` sequence in `docs/testing/n1mm-radio-poc.md`.
