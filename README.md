# CatHub

CatHub lets radio applications safely share one transceiver and one keyer.
Clients can use CAT, Hamlib NET, virtual serial, or WinKeyer interfaces.

The daemon is the only owner of each physical device.
It puts all changes in sequence and gives each client separate permissions.
It also controls PTT.
The daemon leaves the station unkeyed after shutdown or a client failure.

## Current support

- First-class Kenwood TS-590 backend
- Broad rig support through an external `rigctld` backend
- Hamlib NET endpoints with per-endpoint permissions
- TS-590 and TS-2000 compatible virtual serial endpoints
- Single-VFO presentation for applications that cannot model the active VFO correctly
- Multi-client WinKeyer broker with typed gRPC and virtual serial endpoints
- Standalone and embedded `[cat_hub]` configuration layouts

## Install

Install the daemon from crates.io with Rust 1.88 or newer:

```powershell
cargo install cathub --version 0.2.1
```

You can also download the Windows or Linux archive from the
[GitHub Releases page](https://github.com/treitforge/cathub/releases).
Download the adjacent SHA-256 checksum.
Verify the archive with the checksum.
Extract the archive.
Put `cathub` or `cathub.exe` on `PATH`.

To build the daemon from source:

```powershell
git clone https://github.com/treitforge/cathub.git
Set-Location cathub
cargo build --release -p cathub
```

The Windows executable is `target\release\cathub.exe`.
The Linux executable is `target/release/cathub`.
A daemon operator does not need the protocol development packages.

## Build and test

The full repository gate requires PowerShell 7, a current stable Rust toolchain, the .NET
10 SDK, and Buf:

```powershell
.\build.ps1 check
```

The principal checks can also be run directly:

```powershell
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
buf lint
dotnet build CatHub.slnx
```

The Rust protocol crate vendors `protoc`, so `cargo build` does not require a separate
Protocol Buffers compiler installation.

## Configuration

CatHub's default configuration path is:

- Windows: `%APPDATA%\cathub\cathub.toml`
- Linux: `$XDG_CONFIG_HOME/cathub/cathub.toml`, or `~/.config/cathub/cathub.toml`

Set `CATHUB_CONFIG_PATH` or pass `--config` to use another file. The complete example is
[config/cathub.toml](config/cathub.toml).

A standalone document uses top-level `[radio]`, `[winkeyer]`, and endpoint tables.
CatHub also accepts these settings below `[cat_hub]` in a managed document.
Use `--section cat_hub` to require the embedded layout.

Validate and inspect configuration without opening hardware:

```powershell
cathub config validate --config config\cathub.toml
cathub config print-effective --config config\cathub.toml
cathub config print-effective --format json --config config\cathub.toml
```

Extract an embedded `[cat_hub]` section without changing the source file:

```powershell
cathub config migrate `
  --from C:\station\managed-config.toml `
  --output "$env:APPDATA\cathub\cathub.toml"
```

Migration does not overwrite an existing destination unless you supply `--force`.
Migration does not remove the source automatically.
`--remove-source-section` creates a `.bak` copy before it changes the managed document.

## Run

The repository helper builds the daemon when necessary.
On first use, it copies the sample configuration to the default location.
Then it starts CatHub:

```powershell
.\scripts\Start-CatHub.ps1
```

For an installed binary:

```powershell
cathub config validate
cathub
```

An orchestrator can request an available typed API port:

```powershell
cathub --winkeyer-api-bind 127.0.0.1:0 --runtime-info .\cathub-runtime.json
```

CatHub publishes the selected endpoint after all configured listeners bind.
The runtime file includes the CatHub process ID.
Clients must validate that process ID before they use the endpoint.

## Client interfaces

- Hamlib-aware clients connect to a configured `[[hamlib_net]]` TCP listener.
- On Windows, serial CAT clients can connect to a single CatHub-owned UMDF COM endpoint; existing
  physical and externally provisioned serial transports remain supported.
- Legacy WinKeyer clients can use their own CatHub-owned UMDF COM endpoint.
- Typed WinKeyer clients connect to the loopback gRPC address in `[winkeyer].api_bind`.

A launcher-managed client uses the endpoint in CatHub's runtime file.

Each endpoint has its own permissions. Applications never open the physical radio or keyer
port directly.

## Protocol packages

The `cathub` crate builds and installs the daemon.
`cathub-protocol` is the Rust client/server contract crate.
`CatHub.Protocol` is the .NET client package project.
Applications use these packages with the typed WinKeyer API.
You do not need them to run the daemon.

The release workflow puts both protocol artifacts in a GitHub release.
Published client packages are available as
[`cathub-protocol`](https://crates.io/crates/cathub-protocol) for Rust and
[`CatHub.Protocol`](https://www.nuget.org/packages/CatHub.Protocol) for .NET.
Registry publication is a separate authorized operation. See
[release and compatibility](docs/architecture/release-and-compatibility.md).

## Documentation

- [Documentation standard](docs/documentation-style.md)
- [Architecture report](docs/architecture/index.html)
- [Radio hub design](docs/design/multi-client-cat-hub.md)
- [WinKeyer broker design](docs/design/winkeyer-broker.md)
- [Virtual serial Phase 1](docs/design/virtual-serial-transport-phase-1.md)
- [Serial client inventory](docs/testing/serial-client-inventory.md)
- [Operator setup](docs/integration/setup.md)
- [Windows CatHub-owned virtual serial](docs/integration/windows-virtual-serial.md)
- [Virtual serial signing decision](docs/design/virtual-serial-signing-decision.md)
- [Release and compatibility](docs/architecture/release-and-compatibility.md)

## License

MIT. See [LICENSE](LICENSE).
