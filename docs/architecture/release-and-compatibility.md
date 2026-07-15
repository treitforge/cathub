# Release and compatibility

CatHub versions the daemon, configuration schema, and typed WinKeyer protocol together.
The initial compatibility line is `0.1.x`.

## Release artifacts

Pushing a `v*` tag runs `.github/workflows/release.yml` and creates a GitHub release with:

- Windows x86-64 daemon archive and SHA-256 checksum
- Linux x86-64 daemon archive and SHA-256 checksum
- `cathub-protocol` Rust crate archive and SHA-256 checksum
- `CatHub.Protocol` NuGet package and SHA-256 checksum

GitHub release assets and registry packages are separate distribution channels. Creating a
tag does not publish to crates.io or NuGet.

## Registry publication order

Registry publication requires the release owner's credentials and must occur only after the
tagged release passes CI:

```powershell
cargo publish -p cathub-protocol
cargo search cathub-protocol --limit 1
cargo publish -p cathub
dotnet nuget push artifacts\release\CatHub.Protocol.<version>.nupkg `
  --source https://api.nuget.org/v3/index.json `
  --api-key $env:NUGET_API_KEY
```

Do not print either registry token. Verify package ownership, the expected version, and the
tag before publishing. The daemon crate depends on the same version of `cathub-protocol`.
Publish the protocol crate first, wait until the crates.io index exposes that version, and
only then publish `cathub`.

The current workflow does not perform registry publication. A release owner must run these
commands or add separately authorized trusted-publishing jobs.

## Wire compatibility

The 0.1 typed WinKeyer API retains the legacy `qsoripper.services` protobuf package string
so existing pre-release clients can connect. That string is a wire identifier, not a source
or runtime dependency. It must not be renamed in place within the 0.1 line.

A future wire namespace must be introduced as a new protocol version with an explicit
migration window. Request and response envelopes remain unique per RPC, and protobuf
1-1-1 remains the default file layout.

## Configuration compatibility

The authoritative standalone layout is `cathub.toml` with top-level CatHub tables. The
parser also accepts the same tables under a top-level `[cat_hub]` section in a managed TOML
document. Other top-level tables in that document are ignored.

`--section cat_hub` requires the managed layout explicitly. `config migrate` extracts that
section to a standalone file and never removes it from the source unless
`--remove-source-section` is requested.
