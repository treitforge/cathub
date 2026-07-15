# CatHub as an independent product

Status: accepted for the 0.1 release.

CatHub is independently versioned station infrastructure. It owns radio CAT access,
Hamlib NET and virtual serial endpoints, PTT arbitration, physical WinKeyer access,
its configuration schema, and its public broker protocol. It does not depend on a
logger, QSO storage, ADIF, QRZ, a station profile, or a specific user interface.

QsoRipper is one optional client. It consumes Hamlib NET and the versioned WinKeyer
broker protocol. QsoRipper may launch an installed or explicitly bundled CatHub, but
normal logging does not require CatHub to be installed or running.

The 0.1 protocol preserves the historical `qsoripper.services` wire package so deployed
clients remain compatible. CatHub owns that contract from this release onward. A future
CatHub-namespaced protocol will be introduced as a separate version, not as an in-place
wire rename.

CatHub's standalone configuration is `cathub.toml`. A QsoRipper unified file containing
`[cat_hub]` remains a supported managed compatibility layout. `--section cat_hub` makes
that selection explicit, while automatic detection remains available during migration.

Tagged releases produce Windows and Linux archives, SHA-256 checksum files, the
`cathub-protocol` Rust crate archive, and the `CatHub.Protocol` NuGet package. Publishing
to package registries is a separate release-authority step. Publish `cathub-protocol`
before publishing the `cathub` crate because the daemon depends on that version.
