# Contributing

Use a current stable Rust toolchain, the .NET 10 SDK, Buf, and PowerShell 7.

Run the complete local gate before opening a pull request:

```powershell
.\build.ps1 check
```

Changes under `crates\cathub-protocol\proto` change the public wire contract.
Keep the protobuf 1-1-1 structure.
Use a unique request envelope and response envelope for each RPC.
Run `buf lint`.
Document compatibility.

The 0.1 `qsoripper.services` wire-package identifier is a compatibility
boundary. Do not rename it in place. Do not interpret it as a source
dependency.

An operator must attend all hardware transmission tests.
Automated tests must not key a physical transmitter.

Follow the [documentation standard](docs/documentation-style.md) for
documentation, reports, issues, and pull requests.
