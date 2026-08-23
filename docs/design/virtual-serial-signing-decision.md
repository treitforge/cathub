# Virtual serial signing decision gate

Status: research complete; provider approval and clean-system evidence pending.

## What is established

- Windows Plug and Play treats a signed catalog as the signature for the complete driver package.
  Microsoft documents Authenticode as a supported way to sign a catalog after package contents are
  finalized: [Catalog files and digital signatures](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/catalog-files).
- SignPath's current artifact schema explicitly supports Authenticode signing of `.cat` files with
  the `catalog-file` element: [Artifact Configuration Reference](https://docs.signpath.io/artifact-configuration/reference).
- SignPath Foundation currently publishes at least one accepted virtual-driver project, the
  [Virtual Display Driver](https://signpath.org/projects/virtual-display-driver/). This is useful
  policy precedent, but it is not approval of CatHub or proof that the resulting certificate will
  satisfy CatHub's PnP installation scenario.
- Microsoft's Hardware Dev Center path still requires an EV certificate to establish the hardware
  dashboard account for attestation or WHCP submission:
  [Driver code signing requirements](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-reqs).

The repository's ephemeral self-signed DLL and catalog are development-test material only. The
development target must trust the public test certificate and run with Windows Test Signing mode
enabled. This is not a production signing path.

## SignPath request

Before purchasing an EV certificate, apply to SignPath Foundation and request an explicit written
answer to all of these questions:

1. Will the Foundation program approve CatHub and allow its certificate to sign the
   `cathub_virtual_serial_umdf.cat` driver-package catalog?
2. Is that certificate intended to satisfy direct PnP installation of a pure UMDF 2 package on
   supported x64 Windows 11 systems?
3. Are driver catalogs subject to additional review, build-origin, release-approval, or timestamp
   requirements beyond ordinary executable signing?
4. May the signed catalog be distributed with the INF, UMDF DLL, symbols, provenance manifest, and
   installer outside the Microsoft Store and Windows Update?

A minimal SignPath artifact configuration for technical validation is:

```xml
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <catalog-file path="driver/cathub_virtual_serial_umdf.cat">
      <authenticode-sign />
    </catalog-file>
  </zip-file>
</artifact-configuration>
```

Provider approval must precede wiring this configuration into release CI.

## Clean-system acceptance

After obtaining a public signature, create a fresh Windows 11 x64 VM with Secure Boot and Memory
Integrity enabled. Do not import a CatHub or provider certificate and do not enable test signing.
Use the proposed installer flow and preserve:

- the exact elevation and publisher prompts;
- `signtool verify /pa /v` and certificate-chain output for the catalog;
- PnPUtil, Device Manager, Code Integrity, and UMDF host diagnostics;
- standard-user COM access after installation and reboot;
- repair, upgrade, rollback, device restart, and full uninstall results.

Direct distribution passes only if the package stages, the UMDF host loads it, and the complete
end-to-end harness passes without weakening Secure Boot, Memory Integrity, or certificate policy.
If clean Windows rejects the package, preserve the exact failure before selecting EV-backed
Hardware Dev Center attestation or WHCP/HLK as the required production path.
