# Contributing

Use a current stable Rust toolchain, the .NET 10 SDK, Buf, and PowerShell 7.

Run the complete local gate before opening a pull request:

```powershell
.\build.ps1 check
```

Changes to files under `crates\cathub-protocol\proto` are public wire-contract changes.
Keep protobuf 1-1-1 structure, use unique request and response envelopes, run `buf lint`,
and document compatibility. Do not rename the 0.1 `qsoripper.services` package in place.

Hardware transmission tests must be attended. Automated tests must not key a physical
transmitter.
