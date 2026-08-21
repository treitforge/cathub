# CatHub pure Rust UMDF proof of concept

This isolated Cargo package proves the first build and ABI boundary for issue #6.
It is not part of the normal CatHub workspace because `windows-drivers-rs` supports one WDK
configuration per Cargo build graph.

The current driver creates a private proof-of-concept-class WDF device and registers two
reference-named instances of private interface
`{0084BDDE-9F40-4A6A-AF84-0F4E46B70901}`: `application` and `daemon`.
It does not register `GUID_DEVINTERFACE_COMPORT` or claim a COM number.
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

To build and validate an unsigned package without creating a certificate:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action ValidatePackage
```

On an isolated driver-development target, produce the upstream test-signed package with:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action Package
```

The `Package` action generates a certificate in the upstream workflow's private test store and
uses it to sign the package. It requires `makecert` and `signtool`.
Building a package does not authorize installing its certificate or driver.
Keep any test certificate off normal operator machines.

## Current safety boundary

All Windows and WDF calls live in `src/interop.rs`.
Every exported or registered callback catches Rust panics before they can unwind into WDF.
The driver contains no kernel-mode CatHub code and no C or C++ shim.

The proof-of-concept data plane currently provides:

- one exclusive handle for each reference name;
- independent 64 KiB bounded queues in both directions;
- all-or-nothing writes, with overflow reported instead of truncation;
- cancelable reads held in WDF manual queues when no data is available; and
- fail-closed cleanup that clears buffered bytes and completes both sides' pending reads when
  either handle disconnects, with new reads and writes rejected until both peers reconnect.

This is deliberately a single-device raw-byte proof of concept. It does not yet decode the shared
private framing contract, implement serial timeouts or controls, restrict the daemon interface to a
service SID, or expose a real application COM port. The INF stays in the private proof-of-concept
class until those behaviors are implemented and verified on an isolated driver-development target.
