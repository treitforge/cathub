# Virtual serial UMDF proof of concept

## Scope and status

This branch began as the isolated proof of concept for issue #6 and now contains the integrated
development candidate. Phase 1 defined the private framing contract and conformance harness; the
subsequent work added the pure Rust UMDF data plane, CatHub adapter, Ports-class provisioning, and
development packaging.

The driver now registers one application-facing `GUID_DEVINTERFACE_COMPORT` interface and one
reference-named `daemon` instance of the private CatHub interface
`{0084BDDE-9F40-4A6A-AF84-0F4E46B70901}`. The application sees one Windows-assigned COM port while
`cathub.exe` transfers framed data and control events through the private interface.

## Reproducible inputs

| Input | Pin |
|---|---|
| Rust | `1.91.0` |
| Target | `x86_64-pc-windows-msvc` |
| `windows-drivers-rs` | `8e88dd899d9fa988df841e08cc01e9f663e5a415` |
| WDK NuGet package | `10.0.28000.2526` |
| SDK dependency | `10.0.28000.1721` |
| Driver model | UMDF 2.33 |
| Minimum Windows family | Windows 11 x64 |

The Microsoft Rust driver repository is still experimental.
CatHub therefore pins a commit instead of a branch or loose crate version.
The driver is an excluded, standalone Cargo package because the upstream build supports only one
WDK configuration in a Cargo build graph.

## Local environment audit

Audit date: 2026-08-22.

Available on the development station:

- Rust and Cargo 1.91.0 for `x86_64-pc-windows-msvc`
- LLVM/Clang 21.1.2
- Visual Studio 2026 Build Tools and Visual Studio 2022
- Windows SDK directories through 10.0.26100.0
- User-local Microsoft WDK NuGet package 10.0.28000.2526 and SDK dependency 10.0.28000.1721
- Pinned `windows-drivers-rs` checkout and Microsoft VirtualSerial2 source cache
- `cargo-make` 0.37.24 and `rust-script` 0.36.0
- N1MM Logger+ and the existing com0com pairs, which remain untouched for comparison

The WDK package supplies the WDF headers and WDK validation/packaging tools without a machine-wide
installation or administrator access. The driver compiles against that package, `Inf2Cat` reports
zero signability errors or warnings, and `InfVerif` accepts the generated INF. The package embeds
a short-lived development signature in the DLL before regenerating and signing the catalog.

With explicit operator authorization, the development package was provisioned locally as COM91
without changing or removing any com0com device. Windows staged the package and verified both
signatures. Normal boot policy correctly rejected the self-signed image with
`ERROR_INVALID_IMAGE_HASH`; after the operator explicitly enabled Windows Test Signing and
rebooted, the exact `f641c2b` package loaded successfully with Memory Integrity running and
integrity checks enabled. The full installed-driver acceptance described below then passed.

## Microsoft sample inventory

The API reference is Microsoft VirtualSerial2 at commit
`717778a20ba4dd2440fe609f69153a1f8a64f597`.
The source remains upstream; CatHub does not copy the C implementation.

### Callbacks and queues

| Area | VirtualSerial2 behavior | CatHub PoC state |
|---|---|---|
| Driver | `DriverEntry`, `EVT_WDF_DRIVER_DEVICE_ADD` | Pure Rust entry and device creation implemented |
| Device | Device context and cleanup callback | Typed per-device context and destroy cleanup implemented |
| Default queue | Parallel read, write, and device-control callbacks | Parallel read, write, and serial-control queue implemented |
| Pending reads | Manual queue | Two manual queues with cancellation and disconnect draining implemented |
| Pending event wait | Separate manual queue | One-outstanding-wait manual queue implemented |
| Cleanup | Device cleanup releases COM mapping | Application/daemon detach drains requests and clears bounded buffers |

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

CatHub adds a separate ACL-restricted private device interface and a stable endpoint ID.
Those are CatHub requirements, not behaviors supplied by VirtualSerial2.

## Unsafe and FFI inventory

The unsafe surface remains isolated in one module, `src/interop.rs`:

- exported `DriverEntry`
- `WdfDriverCreate`
- device-add callback and `WdfDeviceCreate`
- file create and cleanup callbacks
- public COM and private device-interface registration
- global COM symbolic-link and legacy `SERIALCOMM` device-map lifecycle
- default and manual queue creation
- typed device-context registration, lookup, and destroy cleanup
- request forwarding, retrieval, cancellation, and completion
- conversion of WDF-owned request buffers and file names to bounded Rust slices
- unload callback registration

Every ABI entry catches panics.
The raw slices never outlive their WDF request or file-object callback. Safe Rust owns the bounded
byte buffers, exclusive-handle state, application session sequence, and pending-read queue handles.
Each device owns those values through typed WDF context; file and queue callbacks resolve their
parent device before accessing the state.

## Development acceptance and remaining production gates

The exact package for `f641c2b028871b5ce90dc9bfcd912da393c14f76` passed the elevated local
end-to-end harness on 2026-08-22. The retained device was healthy as COM91 after reboot and after a
PnP disable/enable cycle. The run verified both interfaces, stable identity, native synchronous
and overlapped I/O, cancellation, timeouts, purge, `WaitCommEvent`, DCB and modem controls, queue
status, bounded-buffer rejection and recovery, exclusive open/reopen, .NET `SerialPort`, CatHub
TS-590 traffic, daemon failure/restart, and reconnect. Every automated compatibility profile
passed 11 of 11 checks, for 55 of 55 total.

That evidence used Windows Test Signing and the development certificate with Secure Boot disabled
under an explicit exception. Issue #6 still requires these production gates:

1. Obtain an approved public or Microsoft signing path and run the clean Windows 11 acceptance
   flow with Secure Boot and Memory Integrity enabled, Test Signing disabled, and no publisher
   certificate preinstalled.
2. Capture and document runs from the five actual supported clients: HDSDR/OmniRig, N1MM CAT,
   ARCP-590, N1MM WinKeyer, and WKTools. The automated profiles validate their expected API shapes
   but are not substitutes for those application runs.
3. Exercise sleep/resume, sustained high-rate and cancellation-race stress, and fail-safe PTT and
   keying behavior through real endpoint lifecycle changes.
4. Implement and validate installer upgrade, repair, rollback, and uninstall behavior, including
   subsequent standard-user operation.
