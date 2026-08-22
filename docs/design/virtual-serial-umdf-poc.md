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
- `cargo-make` 0.37.24 and `rust-script` 0.36.0
- N1MM Logger+ and the existing com0com pairs, which remain untouched for comparison

The WDK package supplies the WDF headers and WDK validation/packaging tools without a machine-wide
installation or administrator access. The driver compiles against that package, `Inf2Cat` reports
zero signability errors or warnings, and `InfVerif` accepts the generated INF. The package embeds
a short-lived development signature in the DLL before regenerating and signing the catalog.

With explicit operator authorization, the development package was provisioned locally as COM91
without changing or removing any com0com device. Windows staged the package and verified both
signatures, but normal boot policy rejected the self-signed image with
`ERROR_INVALID_IMAGE_HASH`. Installed functional validation therefore awaits an explicitly
authorized Test Signing reboot or a publicly/Microsoft-signed package.

## Microsoft sample inventory

The API reference is Microsoft VirtualSerial2 at commit
`717778a20ba4dd2440fe609f69153a1f8a64f597`.
The source remains upstream; CatHub does not copy the C implementation.

### Callbacks and queues

| Area | VirtualSerial2 behavior | CatHub PoC state |
|---|---|---|
| Driver | `DriverEntry`, `EVT_WDF_DRIVER_DEVICE_ADD` | Pure Rust entry and device creation implemented |
| Device | Device context and cleanup callback | Typed per-device context and destroy cleanup implemented |
| Default queue | Parallel read, write, and device-control callbacks | Sequential read, write, and serial-control queue implemented |
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

CatHub will add a separate ACL-restricted private device interface and a stable endpoint ID.
Those are CatHub requirements, not behaviors supplied by VirtualSerial2.

## Unsafe and FFI inventory

The unsafe surface remains isolated in one module, `src/interop.rs`:

- exported `DriverEntry`
- `WdfDriverCreate`
- device-add callback and `WdfDeviceCreate`
- file create and cleanup callbacks
- private device-interface registration
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

## Remaining installed-validation gates

1. Load the self-signed development image under explicitly authorized Windows Test Signing policy,
   or obtain a public/Microsoft signature suitable for normal policy.
2. Confirm the device starts, both interfaces enumerate, and the stable COM mapping survives a
   clean device restart.
3. Run every native serial conformance profile through the private CatHub adapter.
4. Run the `n1mm-radio` sequence in `docs/testing/n1mm-radio-poc.md` and repeat the applicable
   flows for the other four legacy clients.
5. Exercise daemon failure, UMDF-host restart, repair, upgrade, rollback, removal, sleep, and resume
   while preserving fail-safe transmit behavior.
