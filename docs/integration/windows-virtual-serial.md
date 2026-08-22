# CatHub-owned Windows virtual serial endpoints

CatHub's UMDF transport replaces each com0com null-modem pair with one application-facing COM
port. `cathub.exe` attaches through a separate private device interface, so the daemon does not
consume another COM number.

| Stable endpoint | Kind | Default application port | Intended client |
|---|---|---:|---|
| `hdsdr-cat` | CAT | COM11 | HDSDR through OmniRig |
| `n1mm-cat` | CAT | COM21 | N1MM radio CAT |
| `arcp590-cat` | CAT | COM31 | ARCP-590 |
| `n1mm-winkeyer` | WinKeyer | COM41 | N1MM WinKeyer |
| `wktools` | WinKeyer | COM43 | WKTools maintenance |

The compatibility alias `cathub-default` is also available for the isolated test harness. A
configuration selects a managed endpoint explicitly:

```toml
[[serial_endpoint]]
name = "n1mm"
virtual_endpoint = "n1mm-cat"
application_transport = "COM21"
dialect = "ts590"
single_vfo = true
perms = ["read", "write", "ptt"]
```

`application_transport` records and provisions the public COM name. When omitted, CatHub uses the
endpoint's default from the table above.

## Inspect and plan

Status is read-only and does not require elevation:

```powershell
cathub virtual-serial status
cathub virtual-serial status --format json
```

Plan loads the selected CatHub configuration and compares it with PnP devices and the COM Name
Arbiter. It reports create, retain, or reassign actions and blocks any port already owned by a
physical device, com0com, another virtual driver, or a stale arbiter reservation.

```powershell
cathub --config C:\ProgramData\CatHub\cathub.toml virtual-serial plan
```

The planner never removes or repurposes a non-CatHub device. Migrate or remove old com0com pairs
explicitly before asking CatHub to reuse their application-side COM numbers.

## Apply and remove

Run device-changing operations from an elevated Administrator terminal. `apply` requires the INF
from a complete, signed CatHub driver package; keep the INF, catalog, and UMDF DLL together.

```powershell
cathub --config C:\ProgramData\CatHub\cathub.toml virtual-serial apply `
  --inf C:\ProgramData\CatHub\driver\cathub_virtual_serial_umdf.inf
```

Apply stages the package, creates only hardware IDs from CatHub's fixed allow-list, claims the
requested COM numbers, installs or restarts the device, and verifies the resulting PnP state. It is
idempotent: a second successful run reports retained endpoints and makes no changes.

Provisioning also records the invoking Windows user's SID as the private-channel owner. The COM
port remains usable by ordinary desktop serial clients, but the driver accepts the private
`cathub.exe` channel only when UMDF can impersonate a request from that owner. Run `apply` again as
the intended service user to transfer private-channel ownership; the plan reports this as an
`AUTHORIZE` action. Normal daemon and application operation does not require elevation.

Remove every CatHub-owned endpoint, or select stable IDs individually:

```powershell
cathub virtual-serial remove
cathub virtual-serial remove --endpoint n1mm-cat --endpoint n1mm-winkeyer
```

Removal checks the same compiled ownership allow-list and cannot target physical ports or com0com
devices. It removes endpoint device instances but leaves the driver package staged so a later
repair/apply does not depend on network access.

## Development package

The repository packaging command creates an ephemeral development certificate and signs the
catalog without installing that certificate on the build workstation:

```powershell
.\scripts\Test-UmdfPoc.ps1 -Action Package
```

The resulting package is suitable only for the isolated VM procedure in
`scripts/Test-UmdfEndToEnd.ps1`. Production distribution still requires the approved public
catalog-signing path and clean-system Secure Boot/Memory Integrity acceptance evidence. See the
[signing decision gate](../design/virtual-serial-signing-decision.md) for the current provider
research and required acceptance record.

Run the development acceptance harness only from an elevated PowerShell session inside an isolated
Hyper-V VM:

```powershell
cd C:\CatHubUmdfTest
.\Test-UmdfEndToEnd.ps1 -IUnderstandThisInstallsATestDriver
```

The harness fails before installation unless Secure Boot and Memory Integrity are running and the
boot configuration has neither test-signing mode nor integrity checks disabled. Its JSON evidence
records the OS and boot policy, package manifest, catalog trust before and after importing the
ephemeral test certificate, PnP driver metadata, serial I/O and recovery cases, and System, Code
Integrity, and UMDF event logs. After a successful run it removes the endpoint, staged OEM driver
package, and test certificate and verifies the resulting CatHub inventory. Pass `-KeepInstalled`
only when retaining that isolated VM state is necessary for debugging; failed runs retain state so
the original failure can be inspected before reverting the VM checkpoint.
