# CatHub

CatHub lets radio applications that use CAT, Hamlib NET, virtual serial, and WinKeyer
interfaces safely share one transceiver and keyer.

The daemon is the single owner of each physical device. It serializes mutations, serves
separately permissioned client endpoints, arbitrates PTT, and leaves the station unkeyed
after shutdown or a failed client session.

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
cargo install cathub --version 0.1.1
```

Alternatively, download the Windows or Linux archive and adjacent SHA-256 checksum from the
[GitHub Releases page](https://github.com/treitforge/cathub/releases). Extract the archive
and place `cathub` or `cathub.exe` on `PATH`.

To build the daemon from source:

```powershell
git clone https://github.com/treitforge/cathub.git
Set-Location cathub
cargo build --release -p cathub
```

The source-built executable is `target\release\cathub.exe` on Windows and
`target/release/cathub` on Linux. A daemon operator does not need either
protocol-development package described below.

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

A standalone document uses top-level `[radio]`, `[winkeyer]`, and endpoint tables. CatHub
also accepts those settings beneath `[cat_hub]` in a larger managed document. Use
`--section cat_hub` when a launcher must require the embedded layout.

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

Migration refuses to overwrite an existing destination unless `--force` is supplied.
Source removal is never automatic. `--remove-source-section` creates a `.bak` copy before
changing the managed document.

## Run

The repository helper builds the daemon when necessary, copies the sample configuration to
the platform default on first use, and starts CatHub:

```powershell
.\scripts\Start-CatHub.ps1
```

For an installed binary:

```powershell
cathub config validate
cathub
```

## Client interfaces

- Hamlib-aware clients connect to a configured `[[hamlib_net]]` TCP listener.
- Serial CAT clients connect to the application side of a dedicated virtual serial pair.
- Legacy WinKeyer clients connect to their own virtual serial pair.
- Typed WinKeyer clients connect to the loopback gRPC address in `[winkeyer].api_bind`.

Each endpoint has its own permissions. Applications never open the physical radio or keyer
port directly.

## Protocol packages

The `cathub` crate builds and installs the daemon. `cathub-protocol` is the Rust
client/server contract crate. `CatHub.Protocol` is the .NET client package project. The
protocol packages are for applications that use the typed WinKeyer API and are not required
to run the daemon.

The release workflow packages both protocol artifacts into a GitHub release. Published
client packages are available as
[`cathub-protocol`](https://crates.io/crates/cathub-protocol) for Rust and
[`CatHub.Protocol`](https://www.nuget.org/packages/CatHub.Protocol) for .NET. Registry
publication remains a separate release-authority operation. See
[release and compatibility](docs/architecture/release-and-compatibility.md).

## Documentation

- [Architecture report](docs/architecture/index.html)
- [Radio hub design](docs/design/multi-client-cat-hub.md)
- [WinKeyer broker design](docs/design/winkeyer-broker.md)
- [Operator setup](docs/integration/setup.md)
- [Release and compatibility](docs/architecture/release-and-compatibility.md)

## License

MIT. See [LICENSE](LICENSE).
