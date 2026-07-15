# cathub operator setup

`cathub` is a single daemon that owns the radio's CAT serial port and fans it out
to every application over that application's native protocol. Because only the daemon talks
to the radio, the classic multi-app failures disappear:

- no VFO A/B oscillation (baseline polling never emits VFO-select/retarget commands),
- no frequency drift (one serialized writer, ordered writes, native-push reconciliation),
- no PTT contention (single-owner PTT lease with a hard transmit-time ceiling),
- no auto-info stomping (each app's auto-information is virtualized per connection).

This runbook brings up the hub for six applications sharing one Kenwood TS-590:
HDSDR (via OmniRig), QsoRipper TUI and GUI, ARCP-590, N1MM Logger+, WSJT-X, and Log4OM.

See `docs/design/multi-client-cat-hub.md` for the architecture and behavior contracts.

## 1. Retire the legacy chain

The old stack was rigctld + a Python safe-bridge + rigctlcom. Stop all of it before starting
the hub. Only one process may own the radio's COM port (COM4 on this station - the
Silicon Labs CP210x USB-UART bridge that fronts the TS-590's USB CAT port).

    Get-Process rigctld, rigctlcom -ErrorAction SilentlyContinue | Stop-Process
    # also stop any safe-bridge Python process and any app still bound directly to COM4

Remove or disable any legacy startup hooks that relaunch `rigctld.exe`; otherwise it can
reclaim the radio COM port after you stop CatHub. Check scheduled tasks first:

    Get-ScheduledTask -TaskName QsoRipper-rigctld -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName QsoRipper-rigctld -Confirm:$false

Confirm nothing else holds COM4 before continuing.

## 2. Create virtual serial pairs (com0com)

Serial clients connect through a virtual null-modem pair. The daemon binds the first port of
each pair; the application binds the second. Using the com0com "setupc" tool, create:

    install PortName=COM10 PortName=COM11    # HDSDR / OmniRig  (daemon COM10, app COM11)
    install PortName=COM20 PortName=COM21    # N1MM Logger+     (daemon COM20, app COM21)
    install PortName=COM30 PortName=COM31    # ARCP-590         (daemon COM30, app COM31)
    install PortName=COM40 PortName=COM41    # N1MM WinKeyer    (daemon COM40, app COM41)
    install PortName=COM42 PortName=COM43    # WKTools          (daemon COM42, app COM43)

WSJT-X, Log4OM, and the QsoRipper engine use the Hamlib NET (TCP) endpoints instead and need
no serial pair.

The WinKeyer pairs are separate from N1MM's radio-CAT pair. N1MM uses COM21 for its TS-590 radio and COM41 for WinKeyer. WKTools uses COM43 only for maintenance. CatHub owns physical WinKeyer COM3 and the hub sides COM40 and COM42. This permits N1MM and QsoRipper to remain connected to one keyer without attempting an unsafe shared open of COM3, while device maintenance receives a separately permissioned endpoint.

Each pair has two COM numbers: a **daemon side** (the lower, even number COM10/20/30 that the
hub opens via `transport`) and an **application side** (the partner COM11/21/31). Point each
application at its application-side port. The daemon-side port is held open by the hub, so it
typically will **not** appear in an application's COM-port dropdown at all -- that is expected,
and the partner port (COM11/21/31) is the one to select.

Record the application side as `application_transport` on each `[[cat_hub.serial_endpoint]]` and
`[[cat_hub.winkeyer_endpoint]]`. CatHub does not open this endpoint. It uses the value in setup
status and dry-run guidance so the application port remains discoverable after comments are
removed from the active configuration.

## 3. Configure the daemon

CatHub's authoritative settings live in its own `cathub.toml` file:

- Windows: `%APPDATA%\cathub\cathub.toml`
- Linux/macOS: `$XDG_CONFIG_HOME/cathub/cathub.toml` (or `~/.config/cathub/cathub.toml`)
- Override the location with `CATHUB_CONFIG_PATH` or `--config`.

The standalone file uses top-level `[radio]`, `[poll]`, `[ptt]`, `[events]`,
`[[serial_endpoint]]`, `[[hamlib_net]]`, `[winkeyer]`, and `[[winkeyer_endpoint]]` tables.
The repository ships a complete example at `config\cathub.toml`. Validate it without
touching hardware:

    cargo run -p cathub -- config validate --config config\cathub.toml
    cargo run -p cathub -- config print-effective --config config\cathub.toml

CatHub also accepts an existing QsoRipper unified file with settings nested under `[cat_hub]`.
Managed launchers should pass `--section cat_hub` to require that layout explicitly.
To move that section into CatHub's standalone file without changing the source:

    cargo run -p cathub -- config migrate `
      --from "$env:APPDATA\qsoripper\config.toml" `
      --output "$env:APPDATA\cathub\cathub.toml"

Add `--remove-source-section` only when you are ready to stop using managed compatibility
mode. CatHub creates a `.bak` copy before changing the source file.

To move in the other direction, copy the standalone tables beneath a new `[cat_hub]`
namespace in QsoRipper's unified file, validate with `cathub --section cat_hub config
validate --config <path>`, and only then point the managed launcher at that file. Preserve
the standalone file until the managed launch has been verified.

The `[radio].baud` value **must match the radio's own PC/CAT port speed** (TS-590 menu 62;
e.g. 57600). If they differ, the daemon opens COM4 but cannot talk to the radio. The
`[[serial_endpoint]].baud` values are nominal only -- com0com virtual pairs pass data regardless of baud,
so a client app can use any baud on its side of the pair.

## 4. Start the hub

    .\scripts\Start-CatHub.ps1

The daemon opens COM4, enables and owns the TS-590 `AI2;` native push stream, starts the
baseline poller (which backs off to heartbeat once push covers a field), opens each serial
endpoint, and binds each Hamlib NET endpoint. Watch the log in another terminal:

    .\scripts\Get-CatHubLog.ps1 -Follow

Stop the hub with Ctrl+C in its window, or:

    .\scripts\Stop-CatHub.ps1

Build and test CatHub independently with `.\build.ps1` and `.\test.ps1`. QsoRipper's launcher
can also manage an installed CatHub binary through `CATHUB_EXECUTABLE`, `PATH`, or its optional
bundled-artifact location. Managed mode may continue to pass QsoRipper's unified file while it
contains a valid `[cat_hub]` table.

## 5. Point each application at the hub

### HDSDR (via OmniRig)
- OmniRig: Rig type `Kenwood TS-2000`, port **COM11**, baud 115200, 8-N-1.
- The `hdsdr-omnirig` endpoint is `dialect = "ts2000"`,
  `perms = ["read", "frequency_write"]`. The panadapter follows the radio and click-to-tune
  on the waterfall sets frequency. OmniRig mode writes are denied, preventing its band-plan
  mode selection from changing WSJT-X's USB+Data operation to CW after a frequency update.
  VFO-target writes (`FR`/`FT`) remain rejected by design.

### N1MM Logger+
- Configurer > Hardware: radio `Kenwood`, port **COM21**, 115200, 8-N-1, no flow control.
- Configurer > Hardware: enable WinKeyer on **COM41**, 1200 baud. COM41 is the application
  side of the dedicated COM40/COM41 keyer pair; never select physical COM3 or CatHub's COM40.
- Configure WKTools for **COM43**, 1200 baud. COM43 is the application side of the maintenance
  COM42/COM43 pair. Close WKTools after maintenance so the port is released.
- The `n1mm` endpoint is `dialect = "ts590"`, `single_vfo = true`, `perms = ["read", "write", "ptt"]`.
- **`single_vfo = true` is required for SO1V.** N1MM in single-VFO (SO1V) mode refuses VFO B
  ("You should not use VFO B when configured for SO1V") and freezes its frequency display when
  the radio is on VFO B. With `single_vfo = true` the hub presents whichever VFO the radio is
  on as VFO A, so N1MM tracks the radio across A/B switches with no warning. If you run N1MM in
  SO2V instead, you may set `single_vfo = false`; for SO1V leave it on (the shipped default for
  this endpoint). See design §8.4.2.
- Keep the everyday `n1mm-cw` WinKeyer endpoint limited to `status`, `send`, `control`, and `ptt`.
  Persistent EEPROM/reset access belongs on a separate, normally disabled maintenance endpoint.
- CatHub preserves N1MM's authorized runtime stream byte-for-byte. In particular, the observed
  `16 02 <position> 1C <wpm> <text>` sequence remains an append-pointer command followed by a
  buffered-speed command and the intended text. CatHub does not reinterpret the position byte as
  an Admin prefix or inject configuration between those bytes.

### ARCP-590
- Set ARCP-590's COM port to **COM31**, 115200, 8-N-1.
- The `arcp590` endpoint is `dialect = "ts590"`, `perms = ["read", "write", "ptt", "config_write"]`
  so the full control-panel (including EX-menu writes) works.

### WSJT-X
- Settings > Radio: Rig `Hamlib NET rigctl`, Network Server **127.0.0.1:4533**.
- PTT method `CAT`. The `wsjtx` endpoint is `perms = ["read", "write", "ptt"]`, `single_vfo = true`.
- **`single_vfo = true` keeps WSJT-X decoding on either VFO.** WSJT-X fundamentally expects to
  receive on VFO A and stops decoding if the hub reports the operator is on VFO B. With
  `single_vfo = true` the endpoint always presents whichever VFO the radio is actually on as
  VFO A (`get_vfo` -> `VFOA`, with the live operating frequency/mode), so WSJT-X decodes
  whether the rig is on A or B. The frequency is always real; only the VFO *letter* is
  virtualized.
- **Mode:** set the WSJT-X *Mode* selector to **Data/Pkt** (the default for FT8/WSPR). The
  hub maps Hamlib's `PKTUSB`/`PKTLSB` to the TS-590's DATA sub-mode by composing the base
  mode (`MD`) with the radio's independent DATA flag (`DA`): `PKTUSB` -> `MD2;`+`DA1;`,
  `PKTLSB` -> `MD1;`+`DA1;`, and a plain `USB`/`LSB` clears it with `DA0;`. The mode
  read-back recomposes the token, so WSJT-X sees `PKTUSB`/`PKTLSB` echoed back and the radio
  shows its DATA indicator lit. *Mode = None* (operator selects DATA on the front panel) and
  *Mode = USB* also round-trip cleanly if you prefer to manage the sub-mode yourself.
- **Split Operation:** use **`Fake It`** (not `Rig`). A `single_vfo` endpoint presents a
  single operating VFO and cannot model a real A/B split, so it **rejects** rig split-enable
  (`RPRT -11`); "Fake It" QSYs the single VFO at TX time and works correctly on either VFO.
  If you genuinely need real `Rig` split, point WSJT-X at a non-virtualized endpoint instead.
- **PTT:** WSJT-X keys with Hamlib `RIG_PTT_ON_DATA` (`T 3`). The hub maps the Hamlib PTT
  family faithfully to the TS-590 - `T 1` -> `TX;`, `T 2` (mic) -> `TX0;`, `T 3` (data) ->
  `TX1;`, `T 0` -> `RX;`. `TX1;` keys with modulation from the DATA/USB audio path, which is
  what digital modes want.
- Frequencies are sent by Hamlib as a `%f` value (e.g. `14074000.000000`); the hub
  accepts both that decimal form and a plain integer, so `Test CAT` and band changes
  set the dial correctly.

### JS8Call

- Settings > Radio: Rig `Hamlib NET rigctl`, Network Server **127.0.0.1:4535**.
- PTT method `CAT`, Split Operation **Fake It**.
- Give JS8Call its own writable/PTT endpoint. Do not point it at WSJT-X's port 4533. Both
  applications set mode and PTT, and sharing a port allows one application's plain-USB mode
  selection to clear the TS-590 DATA flag required by the other.

### TS-590 PC-control beep (fixed in the hub)

Earlier builds made the TS-590 emit a short Morse **"U"** (di-di-dah) tone during WSJT-X
operation, most noticeably when switching between modes that share a radio mode (for example
FT8 and WSPR, which are both DATA-USB). The TS-590 beeps on **every** mode (`MD`) command it
receives over CAT - frequency sets are silent. WSJT-X re-asserts its mode on every poll and on
each FT8/WSPR switch, and the hub forwarded each `MD` set to the radio even when the value was
unchanged, so the radio chirped. A native Hamlib driver never beeps because it caches state and
sends a mode command only when the mode actually changes.

The hub now does the same: a modeled write (frequency, mode, DATA sub-mode, split, RIT/XIT)
is sent to the radio only when it would change the radio. Mode comparison uses the **value
written to the radio** (the `MD` digit), and the DATA flag (`DA`) is deduped independently, so
switching between two modes that share the same wire state - for example FT8 and WSPR, which
are both `PKTUSB` (`MD2`+`DA1`) - is recognized as a no-op and suppressed after the first set.
A genuine mode change (for example switching to CW, or toggling DATA on/off) still sends one
command and the radio beeps once, which matches native Hamlib behavior. PTT is never
suppressed - keying and unkeying always reach the radio.

No radio-menu change is needed. Leave **Beep Volume** at your normal setting.

### Log4OM
- CAT interface: Hamlib `NET rigctl`, host **127.0.0.1**, port **4534**.
- The `log4om` endpoint is `perms = ["read", "write"]`, `single_vfo = true`.
- Log4OM-NG uses Hamlib's **Extended Response Protocol** (ERP): it opens every session with
  `;V ?` (list supported VFOs) and then polls with `+\get_vfo_info VFOA` (~2 Hz). The hub's
  `hamlib_net` endpoint parses the ERP separator prefix (`+ ; | ,`) and answers both shapes in the
  exact byte format real `rigctld` produces, so Log4OM connects and stays **online**. Plain
  clients (WSJT-X, N1MM, the engine) are unaffected - they never send the ERP prefix.
- **`single_vfo = true` makes Log4OM log the live frequency on either VFO.** Because Log4OM
  polls the fixed VFO `VFOA`, without virtualization it would read the *inactive* VFO A's
  stale frequency whenever the operator works on VFO B. With `single_vfo = true` the endpoint
  resolves the `VFOA` poll to whichever VFO is actually operating, so Log4OM tracks the real
  dial on both VFOs (and `;V ?` advertises only `VFOA`).

### QsoRipper engine (TUI and GUI)
- The engine's `RigctldProvider` points at the read-only endpoint **127.0.0.1:4532**
  (`single_vfo = false`, so the engine sees and logs the true operating VFO letter).
- The TUI and GUI both consume the engine over gRPC; neither talks to the radio directly, so
  both get a consistent view fed by the same hub. TCP allows the engine and any other NET
  client to share an endpoint simultaneously.
- The read-only endpoint exposes split TX frequency/mode and configured transmitter power when
  the radio backend supplies them. During split operation the engine logs the transmit side as
  `FREQ`/`BAND`, the receive side as `FREQ_RX`/`BAND_RX`, and configured power as `TX_PWR`.
  Unsupported optional values remain blank and do not make rig control appear disconnected.
- Keep `[rig_control].stale_threshold_ms` low (e.g. **100**) when reading through cathub. The hub
  serves reads from its in-memory state cache (kept current by the radio's native AI2 push), so a
  short freshness window is cheap and makes the GUI/TUI frequency display follow knob turns almost
  immediately. A large value such as 5000 makes the engine reuse a stale snapshot for that many
  milliseconds and the UI lags behind the radio by up to that interval.

## 6. Verify (bench)

With the hub running and all six apps connected:

1. Turn the physical VFO knob. Every app's displayed frequency tracks it within a poll/push
   cycle, and the cathub log shows `NativePush` (not `PollDiff`) events once `AI2;` is active.
2. Change band in N1MM. HDSDR's waterfall recenters without HDSDR ever talking to N1MM, and
   the TS-590 never bounces between VFO A and B.
3. Set frequency from WSJT-X. N1MM, Log4OM, ARCP-590, and the engine all converge.
4. Key WSJT-X (Tune). The PTT lease is granted; while held, an attempt to key from N1MM is
   rejected (Hamlib `RPRT` busy) and logged. Releasing WSJT-X frees the lease.
5. Leave one over running longer than expected: confirm a transmit never exceeds
   `ptt_max_tx_ms` (the safety ceiling force-releases and logs loudly).

Live transmit verification requires the operator and real hardware; do not key the
transmitter from automation. Watch `Get-CatHubLog.ps1 -Follow` throughout.

## 7. Troubleshooting

- "Access denied" / port busy on COM4: the legacy chain or another app still owns the radio
  port (step 1).
- Daemon starts but never reads valid data from the radio (timeouts / stale): the `[radio].baud`
  does not match the radio's PC/CAT port speed. Check TS-590 menu 62 and set `[radio].baud` to
  the same value (e.g. 57600). A client app proves the rig's real baud quickly via a direct
  connection.
- Daemon starts, the port opens, but every poll times out at the *correct* baud: some radios
  (notably the Kenwood TS-590) only reply when the **RTS modem-control line is asserted**. The
  hub now asserts RTS and DTR automatically when it opens a serial radio port, so this should
  not occur with current builds. If you see it on an older build, update the daemon.
- An app's COM dropdown does not list the daemon-side port (COM10/20/30): expected. The hub
  holds that port open, so applications only see the partner port. Select COM11/21/31 instead.
- An app sees no data on its serial port: the com0com pair is reversed or the app is on the
  daemon's half of the pair. The app binds the **second** port of each pair (COM11/21/31).
- N1MM cannot open the WinKeyer: select COM41, not COM3 or COM40, and confirm the hub log says
  the `n1mm-cw` endpoint opened COM40 at 1200 baud, 8-N-2.
- A physical WinKeyer USB disconnect cancels queued keying, releases station PTT, and starts
  capped reconnect attempts without stopping the radio hub. N1MM may need to reopen its
  logical keyer session after the physical device reconnects.
- A NET client cannot connect: confirm the bind address/port in `config\cathub.toml` matches
  the app, and that the hub log shows the endpoint listening.
- An app that relies on Kenwood auto-information (notably **ARCP-590**) connects but never
  tracks the dial / shows "BUSY": such apps poll `AI;` as a keepalive and depend entirely on
  the radio pushing `FA;`/`IF;` frames. The hub virtualizes auto-info per connection - an `AI;`
  read reports the endpoint's current state (`AI0;`/`AI2;`) without disabling it, and the hub fans
  out native-push frames to any endpoint that has enabled `AI2;`. This works in current builds; if
  an older build froze ARCP-590 on connect, update the daemon.
- Set `CATHUB_LOG=debug` before starting for verbose tracing. Use
  `CATHUB_LOG=cathub::serial_endpoint=trace` to see each endpoint's request/reply/notify
  frames, which is the fastest way to diagnose a client handshake.

## 8. Known v1 limitations

- **Automatic radio reconnect.** If the radio transport drops mid-session (USB unplugged,
  radio powered off, cable hiccup, or a write error), the daemon now reopens the serial/TCP
  link automatically with capped exponential backoff (0.5 s up to 5 s) and resumes serving the
  same client command queue - you no longer need to restart the hub. On each reconnect it also
  re-arms the radio's native push (auto-info) state, which a power-cycled radio forgets (design
  §8.4/§8.7). Client endpoints and NET endpoints stay up throughout. Note: clients that hold their
  own CAT session above the hub (e.g. HDSDR via OmniRig) may still need their own
  OmniRig/session restart if they latched onto the dead link before the hub recovered.
- **Hamlib NET bind errors surface in the log, not at startup.** A serial endpoint that fails to
  open aborts startup with a clear error, but a `[[hamlib_net]]` endpoint whose bind address
  is already in use logs the error from its listener task rather than failing the whole
  daemon. If a NET client cannot connect, check the log for that endpoint.
- On shutdown (Ctrl+C) the daemon makes a best-effort `RX;` to unkey the transmitter. A hard
  crash cannot run that path; the `ptt_max_tx_ms` ceiling and the radio's own TX timeout are
  the ultimate stuck-transmitter backstops.
