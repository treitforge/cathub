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

Registry publication requires the release owner's authorization and must occur only after the
tagged release passes CI. Configure the `CARGO_REGISTRY_TOKEN` Actions repository secret.
On nuget.org, add a Trusted Publishing policy for repository owner `treitforge`, repository
`cathub`, and workflow file `publish-registries.yml`. Leave the policy environment empty and
set the `NUGET_USER` Actions repository variable to the policy owner's NuGet username. Then
dispatch the publication workflow:

```powershell
gh workflow run publish-registries.yml -f release_tag=v0.1.0
```

The workflow rejects a draft or prerelease, verifies every release asset checksum, and checks
that the tagged Rust and NuGet versions match. It publishes `cathub-protocol`, waits until
Cargo can resolve that exact version from the crates.io index, publishes `cathub`, and then
uses GitHub OIDC to request a short-lived NuGet API key and publish `CatHub.Protocol`.
Finally, it verifies both crates through the crates.io API and waits for the NuGet package to
become available. Existing versions are detected so a retry can safely continue after a
partial registry failure.

Do not print the crates.io token or the temporary NuGet key. Verify package ownership, the
expected version, and the tag before dispatching the workflow. The daemon crate depends on
the same version of `cathub-protocol`.

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
