# CatHub

CatHub lets programs that use different radio CAT, Hamlib NET, virtual serial, and
WinKeyer interfaces safely share one transceiver and keyer.

CatHub is a standalone station service. It does not require QsoRipper, and QsoRipper is
only one optional client. The daemon owns each physical device, serializes mutations,
serves compatible client endpoints, arbitrates PTT, and leaves the station unkeyed after
shutdown or a failed client session.

## Current support

- First-class Kenwood TS-590 backend
- Broad rig support through an external `rigctld` backend
- Hamlib NET endpoints with per-endpoint permissions
- TS-590 and TS-2000 compatible virtual serial endpoints
- Single-VFO presentation for applications that cannot model the active VFO correctly
- Multi-client WinKeyer broker with typed gRPC and virtual serial endpoints
- QsoRipper unified configuration compatibility

## Build and test

```powershell
.\build.ps1
.\build.ps1 check
```

The direct Rust commands are:

```powershell
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
buf lint
dotnet build CatHub.slnx
```

## Configuration

CatHub's standalone default configuration is:

- Windows: `%APPDATA%\cathub\cathub.toml`
- Linux: `$XDG_CONFIG_HOME/cathub/cathub.toml`, or `~/.config/cathub/cathub.toml`

Set `CATHUB_CONFIG_PATH` or pass `--config` to use another file. The canonical example is
[config/cathub.toml](config/cathub.toml).

CatHub accepts both a standalone document with top-level `[radio]`, `[winkeyer]`, and
endpoint tables, and a QsoRipper unified document with the same content nested under
`[cat_hub]`.

Validate and inspect configuration without opening hardware:

```powershell
cargo run -p cathub -- config validate --config config\cathub.toml
cargo run -p cathub -- config print-effective --config config\cathub.toml
cargo run -p cathub -- config print-effective --format json --config config\cathub.toml
```

Extract an existing QsoRipper `[cat_hub]` section without changing the source file:

```powershell
cargo run -p cathub -- config migrate `
  --from "$env:APPDATA\qsoripper\config.toml" `
  --output "$env:APPDATA\cathub\cathub.toml"
```

Migration refuses to overwrite an existing destination unless `--force` is supplied.
Source removal is never automatic. The optional `--remove-source-section` switch creates a
`.bak` copy before changing the unified file.

For a managed unified file, pass `--section cat_hub` to require that section explicitly.
All effective CatHub settings require a daemon restart in the 0.1 release. Unknown keys
are rejected by CatHub validation. Migration never rewrites unrelated unified sections.

## Run

```powershell
.\scripts\Start-CatHub.ps1
```

See [the setup guide](docs/integration/setup.md),
[the multi-client CAT design](docs/design/multi-client-cat-hub.md), and
[the WinKeyer broker design](docs/design/winkeyer-broker.md).

## Protocol compatibility

The first standalone release preserves the historical `qsoripper.services` WinKeyer broker
wire package. This lets existing QsoRipper clients connect without a flag day. CatHub owns
the contract from this release forward through the `cathub-protocol` Rust crate and the
`CatHub.Protocol` .NET package project.

See [the independent-product decision](docs/architecture/independent-product.md) for the
ownership, compatibility, and release policy.

## License

MIT. See [LICENSE](LICENSE).
