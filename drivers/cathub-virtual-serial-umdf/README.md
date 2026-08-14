# CatHub pure Rust UMDF proof of concept

This isolated Cargo workspace proves the first build and ABI boundary for issue #6.
It is not part of the normal CatHub workspace because `windows-drivers-rs` supports one WDK
configuration per Cargo build graph.

The current driver creates a sample-class WDF device only.
It does not register `GUID_DEVINTERFACE_COMPORT`, claim a COM number, or expose the private CatHub
interface.
Do not install it on the working station.

## Pinned inputs

- Rust `1.91.0`, selected by `rust-toolchain.toml`
- Microsoft `windows-drivers-rs` commit
  `8e88dd899d9fa988df841e08cc01e9f663e5a415`
- UMDF `2.33`
- Windows 11 x64 target

The upstream revision is intentionally repeated in `Cargo.toml` and `Makefile.toml` so neither the
driver build nor the packaging task follows a moving branch.

## Prerequisites

- A supported WDK environment with WDF headers, `inf2cat`, `infverif`, and `stampinf`
- Visual Studio C++ x64 build tools compatible with that WDK
- LLVM/libclang
- `cargo-make` 0.37.16 or newer

An SDK-only installation is not enough.
Use a WDK or Enterprise WDK developer prompt so the matching tools are on `PATH`.

## Build without installation

From the repository root, run the driver-only check:

```powershell
.\scripts\Test-UmdfPoc.ps1
```

To produce the package without installing it:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action Package
```

This command generates a test-signed package as part of the upstream sample workflow.
Building a package does not authorize installing its certificate or driver.
Keep any test certificate off normal operator machines.

## Current safety boundary

All Windows and WDF calls live in `src/interop.rs`.
Every exported or registered callback catches Rust panics before they can unwind into WDF.
The driver contains no kernel-mode CatHub code and no C or C++ shim.

The next implementation step is the application COM interface and its bounded read/write queues.
That work must land with cancellation and cleanup behavior; the INF stays in the Sample class until
those behaviors exist.
