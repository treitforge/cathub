# CatHub pure Rust UMDF proof of concept

This isolated Cargo package proves the first build and ABI boundary for issue #6.
It is not part of the normal CatHub workspace because `windows-drivers-rs` supports one WDK
configuration per Cargo build graph.

The current driver creates a private proof-of-concept-class WDF device only.
It does not register `GUID_DEVINTERFACE_COMPORT`, claim a COM number, or expose the private CatHub
interface.
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

The next implementation step is the application COM interface and its bounded read/write queues.
That work must land with cancellation and cleanup behavior; the INF stays in the Sample class until
those behaviors exist.
