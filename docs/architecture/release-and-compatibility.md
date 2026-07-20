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

The release owner must authorize registry publication.
Publish only after the tagged release passes CI.
Configure the `CARGO_REGISTRY_TOKEN` Actions repository secret.
On nuget.org, add a Trusted Publishing policy.
Set the repository owner to `treitforge`.
Set the repository to `cathub`.

Set the workflow file to `publish-registries.yml`.

Leave the policy environment empty.
Set the `NUGET_USER` Actions repository variable to the policy owner's NuGet username.
Then dispatch the publication workflow:

```powershell
gh workflow run publish-registries.yml -f release_tag=v0.1.1
```

The workflow rejects a draft or prerelease.
It verifies each release asset checksum.
It also makes sure that the tagged Rust and NuGet versions match.
The workflow publishes `cathub-protocol`.
Then it waits until Cargo can resolve that version from the crates.io index.
Next, it publishes `cathub`.

The workflow uses GitHub OIDC to request a temporary NuGet API key.
It uses this key to publish `CatHub.Protocol`.

Finally, it verifies both crates through the crates.io API.
It also waits for the NuGet package to become available.
The workflow detects existing versions.
Thus, a retry can continue after a partial registry failure.

Do not print the crates.io token or the temporary NuGet key.
Before dispatch, verify the package owner.
Verify the expected version and the tag.

The daemon crate depends on the same version of `cathub-protocol`.

## Wire compatibility

The 0.1 typed WinKeyer API retains the legacy `qsoripper.services` protobuf package string
so existing pre-release clients can connect. That string is a wire identifier, not a source
or runtime dependency. Do not rename it in the 0.1 line.

Introduce a future wire namespace as a new protocol version.
Give clients an explicit migration period.
Request and response envelopes remain unique for each RPC.
Protobuf 1-1-1 remains the default file layout.

## Configuration compatibility

The authoritative standalone layout is `cathub.toml` with top-level CatHub tables. The
parser also accepts the same tables under a top-level `[cat_hub]` section in a managed TOML
document. The parser ignores other top-level tables.

`--section cat_hub` requires the managed layout explicitly. `config migrate` extracts that
section to a standalone file. It removes the source section only when the user
specifies `--remove-source-section`.
