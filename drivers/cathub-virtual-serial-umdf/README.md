# CatHub pure Rust UMDF virtual serial driver

This isolated Cargo package implements the Windows driver side of issue #6.
It is not part of the normal CatHub workspace because `windows-drivers-rs` supports one WDK
configuration per Cargo build graph.

The current driver installs as a Ports-class WDF device, registers `GUID_DEVINTERFACE_COMPORT`,
uses the COM number assigned by the Windows Ports class installer, and creates the corresponding
global `COMx` symbolic link. It also registers the reference-named `daemon` instance of the private
CatHub interface `{0084BDDE-9F40-4A6A-AF84-0F4E46B70901}`.
Do not install it on the working station.

## Pinned inputs

- Rust `1.91.0`, selected by `rust-toolchain.toml`
- Microsoft `windows-drivers-rs` commit
  `8e88dd899d9fa988df841e08cc01e9f663e5a415`
- UMDF `2.33`
- Microsoft WDK NuGet package `10.0.28000.2526`
- Windows 11 x64 target

The upstream revision is intentionally repeated in `Cargo.toml` and `Makefile.toml` so neither the
driver build nor the packaging task follows a moving branch.

## Prerequisites

- Microsoft WDK `10.0.28000.2526`, either installed or restored as a NuGet package
- Visual Studio C++ x64 build tools compatible with that WDK
- LLVM/libclang
- `cargo-make` 0.37.16 or newer

Running the self-signed package also requires a development target with Windows Test Signing mode
enabled. Keep Memory Integrity enabled; the package embeds a signature in the driver DLL so HVCI
does not permit unsigned code. Production installation under normal boot policy requires a public
or Microsoft driver signature.

An SDK-only installation is not enough.
The check script automatically finds an installed WDK, `WDKContentRoot`, or the newest package
under `%LOCALAPPDATA%\CatHub\wdk\packages`.
The pinned package can be restored without administrator access:

```powershell
nuget install Microsoft.Windows.WDK.x64 -Version 10.0.28000.2526 `
  -OutputDirectory "$env:LOCALAPPDATA\CatHub\wdk\packages" `
  -NonInteractive -DirectDownload -Source https://api.nuget.org/v3/index.json
```

## Build without installation

From the repository root, run the driver-only check:

```powershell
.\scripts\Test-UmdfPoc.ps1
```

To build and validate a locked release package without creating a certificate:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action ValidatePackage
```

Produce a test-signed release package for an isolated driver-development target with:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action Package
```

The `Package` action generates a short-lived code-signing certificate without adding it to the
host certificate store, embeds a signature in the UMDF driver DLL, regenerates and signs the
package catalog, deletes the private-key file, and leaves the public `cathub_umdf_test.cer` in the
package for the development target. The image signature is required for UMDF code-integrity
validation; it must be applied before catalog generation so the catalog contains the signed DLL's
hash. Building a package does not authorize installing its certificate or driver. Keep the public
test certificate off normal operator machines.

## Current safety boundary

All Windows and WDF calls live in `src/interop.rs`.
Every exported or registered callback catches Rust panics before they can unwind into WDF.
The driver contains no kernel-mode CatHub code and no C or C++ shim.

The data plane currently provides:

- one exclusive handle for each reference name;
- independent 64 KiB bounded queues in both directions;
- all-or-nothing writes, with overflow reported instead of truncation;
- cancelable reads held in WDF manual queues when no data is available; and
- fail-closed cleanup that clears buffered bytes and completes both sides' pending reads when
  either handle disconnects, with new reads and writes rejected until both peers reconnect.

The public COM handle implements the Windows serial controls used by the Phase 1 conformance
harness, including baud/line settings, timeouts, flow control, special characters, modem lines,
queue status, purge, immediate characters, and `WaitCommEvent`. Unsupported IOCTLs fail explicitly.

Each WDF device owns its transport state and pending-read queues through typed object context, and
the context's destroy callback releases the Rust-owned state during device teardown.

The private daemon handle implements the shared `CHVS` 1.0 framing contract, including version
negotiation, discovery, attach/detach, lifecycle events, framed data, receive credit, serial and
modem events, purge, health checks, and deterministic protocol rejection. Its application data is
kept separate from driver control frames.

The private daemon path uses UMDF request impersonation. Provisioning stores the invoking Windows
user SID in the device instance; a daemon create request is accepted only when that SID is a member
of the impersonated caller token. The public COM path does not use this private authorization check.
The application COM path, private CatHub adapter, and driver package must still be verified
together on the isolated driver-development target.
