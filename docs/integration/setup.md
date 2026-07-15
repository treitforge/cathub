# CatHub operator setup

This guide configures one CatHub process to own a radio and optional WinKeyer while several
applications connect through dedicated endpoints. The examples use Windows, a Kenwood
TS-590, com0com virtual serial pairs, and common amateur-radio applications. Substitute the
ports and clients used by your station.

## 1. Install CatHub

Download the matching platform archive from the
[GitHub Releases page](https://github.com/treitforge/cathub/releases), verify the adjacent
SHA-256 checksum, extract it, and place `cathub` or `cathub.exe` on `PATH`.

If the daemon crate has been published for that version and Rust is already installed, the
equivalent installation is:

```powershell
cargo install cathub --version <version>
```

No public release has been tagged yet. Until the first release exists, build from source:

```powershell
git clone https://github.com/treitforge/cathub.git
Set-Location cathub
cargo build --release -p cathub
```

The installed daemon does not require the `cathub-protocol` Rust crate, the
`CatHub.Protocol` NuGet package, the .NET SDK, Buf, or a separate `protoc` installation.
Those tools and packages are for source builds or client development.

Confirm the executable:

```powershell
cathub --version
cathub --help
```

## 2. Inventory physical devices

Record:

- the radio serial port and its configured CAT baud rate
- the physical WinKeyer port, if present
- every application that needs radio state, radio writes, PTT, or keying
- whether each application supports Hamlib NET, a vendor serial dialect, or WinKeyer serial

Only CatHub may open the physical radio and keyer ports. Stop any `rigctld`, serial bridge,
logger, digital-mode program, or manufacturer utility that currently owns either device.
Remove startup tasks that would relaunch an old bridge.

Do not proceed until the physical ports are free.

## 3. Create virtual serial pairs

Applications that require a COM port need one dedicated null-modem pair each. CatHub opens
one side and the application opens the other. Hamlib NET and typed gRPC clients use TCP and
do not need a pair.

Example com0com pairs:

```text
COM10 <-> COM11    SDR software through OmniRig
COM20 <-> COM21    contest logger radio CAT
COM30 <-> COM31    manufacturer control panel
COM40 <-> COM41    contest logger WinKeyer
COM42 <-> COM43    WinKeyer maintenance tool
```

With com0com's `setupc` utility:

```text
install PortName=COM10 PortName=COM11
install PortName=COM20 PortName=COM21
install PortName=COM30 PortName=COM31
install PortName=COM40 PortName=COM41
install PortName=COM42 PortName=COM43
```

The lower, even port in this example is CatHub's `transport`. The other port is
`application_transport`. Never point an application at CatHub's side of the pair.

On Linux, use stable PTY or virtual-serial endpoints managed by the host. Ensure the CatHub
service account can open the radio, keyer, and hub-side paths.

## 4. Create cathub.toml

Copy the repository's [sample configuration](../../config/cathub.toml) to the platform
default:

- Windows: `%APPDATA%\cathub\cathub.toml`
- Linux: `$XDG_CONFIG_HOME/cathub/cathub.toml`, or `~/.config/cathub/cathub.toml`

Set `CATHUB_CONFIG_PATH` or pass `--config` to use another location.

The standalone file uses top-level `[radio]`, `[poll]`, `[ptt]`, `[events]`,
`[[serial_endpoint]]`, `[[hamlib_net]]`, `[winkeyer]`, and `[[winkeyer_endpoint]]` tables.
Delete unused example endpoints and replace every port with the station's actual values.

The radio baud must match the radio's menu setting. Virtual serial endpoint baud values
describe the client-facing protocol and do not replace the physical radio baud.

CatHub also accepts the same tables beneath `[cat_hub]` in a larger managed TOML document.
Require that layout with `--section cat_hub`. To extract it into a standalone file:

```powershell
cathub config migrate `
  --from C:\station\managed-config.toml `
  --output "$env:APPDATA\cathub\cathub.toml"
```

## 5. Validate without hardware

Validation and effective-config printing do not open the radio or keyer:

```powershell
cathub config validate
cathub config print-effective
cathub config print-effective --format json
```

When running from a source checkout with the sample file:

```powershell
cargo run -p cathub -- config validate --config config\cathub.toml
cargo run -p cathub -- config print-effective --config config\cathub.toml
```

Correct every validation error before starting the daemon.

## 6. Start and inspect CatHub

Start an installed daemon:

```powershell
cathub
```

From a source checkout, the helper builds and starts it:

```powershell
.\scripts\Start-CatHub.ps1
```

The rolling log is under `%LOCALAPPDATA%\cathub\logs` on Windows and
`$XDG_STATE_HOME/cathub` or `~/.local/state/cathub` on Linux. From a source checkout:

```powershell
.\scripts\Get-CatHubLog.ps1 -Follow
```

Stop with Ctrl+C. The repository stop helper requests confirmation before terminating a
background CatHub process:

```powershell
.\scripts\Stop-CatHub.ps1
```

## 7. Connect applications

Add clients one at a time. Confirm read behavior before enabling writes or PTT.

### Read-only Hamlib NET client

Point any logger or monitor that supports Hamlib NET rigctl at the configured read-only
listener, such as `127.0.0.1:4532`. It should receive frequency, mode, VFO, split, and power
data when the backend exposes them. Set commands must fail.

### WSJT-X

- Rig: `Hamlib NET rigctl`
- Network Server: the dedicated write/PTT listener, such as `127.0.0.1:4533`
- PTT method: `CAT`
- Split Operation: `Fake It` when the endpoint uses `single_vfo = true`
- Mode: `Data/Pkt` for normal digital operation

Give every simultaneously running digital-mode program its own listener. Separate listeners
prevent one program's mode or PTT policy from affecting another.

### Log4OM

Configure Hamlib NET rigctl with the dedicated Log4OM address, such as
`127.0.0.1:4534`. The sample enables `single_vfo` so Log4OM's VFO A polling follows the real
operating VFO.

### N1MM Logger+

- Radio: `Kenwood`, application port `COM21`, 115200 baud, 8-N-1
- WinKeyer: application port `COM41`, 1200 baud

The sample radio endpoint uses TS-590 dialect, read/write/PTT permissions, and single-VFO
presentation for SO1V operation. The normal WinKeyer endpoint has status, send, control, and
PTT permissions but no `config_write`.

### HDSDR through OmniRig

Configure OmniRig as Kenwood TS-2000 on application port `COM11`. The sample grants read and
frequency-write only, allowing click-to-tune while preventing mode and PTT changes.

### ARCP-590

Configure application port `COM31`. The sample grants full TS-590 read, write, PTT, and
configuration-write access because the manufacturer control panel needs commands outside the
modeled operating state.

### WinKeyer maintenance

Configure the maintenance utility on application port `COM43`. Keep maintenance permissions
on a separate endpoint that is normally unused. Close the utility when maintenance ends so
it releases the virtual port and lease.

### Typed WinKeyer client

Use the loopback gRPC address in `[winkeyer].api_bind`, normally
`http://127.0.0.1:50071`. Supply a stable `client_name` so cancellation and telemetry remain
scoped to that client.

## 8. Safety acceptance

Perform transmit tests while attended and into a suitable load:

1. Verify read-only endpoints reject frequency, mode, and PTT writes.
2. Key and unkey from one authorized CAT client.
3. Confirm a second client cannot acquire PTT while the first owns it.
4. Disconnect the active owner and confirm the station returns to receive.
5. Send a short typed or virtual WinKeyer job and verify completion.
6. Test scoped cancel, active-owner disconnect, and the transmit watchdog.
7. Confirm graceful shutdown leaves both radio and keyer unkeyed.

## Troubleshooting

| Symptom | Check |
|---|---|
| Physical port is busy | Stop every direct radio/keyer client and old bridge. CatHub must be the only physical owner. |
| Radio opens but does not answer | Match `[radio].baud` to the radio menu and verify the physical port. |
| Serial client cannot open a port | The client must open `application_transport`, not CatHub's `transport`. |
| Hamlib NET client cannot connect | Confirm the listener address, firewall policy, and that CatHub completed startup. |
| Writes return not supported | Check endpoint permissions and whether the command is modeled for that dialect. |
| Digital mode stops on VFO B | Enable `single_vfo` for that client and use fake split. |
| PTT is busy | Find the current lease owner in the log and confirm the previous client unkeyed. |
| Typed keyer API is unavailable | Verify `[winkeyer].api_bind` is loopback and not already in use. |
| Maintenance is rejected | Stop active and queued sends, then connect through the `config_write` endpoint. |

Set `CATHUB_LOG=debug` for verbose tracing. Narrow it to a module, such as
`CATHUB_LOG=cathub::serial_endpoint=trace`, when diagnosing one interface.
