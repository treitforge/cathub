# Multi-client radio hub design

## Purpose

CatHub allows applications with incompatible radio-control interfaces to share one
transceiver without sharing the physical serial port. The daemon is the only radio owner.
Every application connects to a dedicated CatHub endpoint with an explicit dialect and
permission set.

The design addresses four recurring station failures:

- competing programs retargeting VFO A and VFO B during polling
- repeated or reordered frequency and mode writes
- several processes attempting to open one serial device
- conflicting PTT requests leaving transmit state ambiguous

## Scope

The current runtime manages one radio and one optional physical WinKeyer per process. It
supports:

- a native Kenwood TS-590 radio backend
- an external `rigctld` radio backend for broader rig support
- TS-590, transparent TS-590, and TS-2000 serial client dialects
- Hamlib NET client endpoints
- virtual and typed WinKeyer client endpoints
- one shared PTT ownership manager

Multi-radio orchestration, remote authorization, and unrestricted public network listeners
are outside the 0.1 scope.

## System boundary

```text
radio applications
  |-- private virtual serial endpoints --|
  `-- dedicated Hamlib NET listeners ----|--> CatHub --> radio or private rigctld

CW applications
  |-- private virtual serial endpoints --|
  `-- loopback typed gRPC ---------------|--> CatHub --> physical WinKeyer
```

The daemon owns physical transports and client-facing listeners. Applications own only the
opposite side of a virtual pair or a TCP connection. Each serial client receives its own
pair because a virtual COM port is still exclusive at either endpoint.

## Radio state model

CatHub maintains one common radio snapshot.
It contains frequency, mode, VFO, split state, RIT/XIT, PTT, and configured transmit power.
The snapshot contains a field only when the backend supplies it.

One scheduler controls all modeled writes.
CatHub serves a read from the snapshot when the field is fresh.
The native TS-590 backend enables the radio's auto-information stream.
It uses a low-rate heartbeat when push events supply a field.
Polling never sends VFO-select or VFO-retarget commands.

Client sessions receive state-change notifications translated into their configured
dialect. A session's auto-information preference is virtual and does not enable or disable
the radio's physical push stream.

Commands outside the modeled state can use the supported passthrough path.
The scheduler puts passthrough commands in sequence with modeled work.
Passthrough commands invalidate the related cached fields.
Endpoint permissions also apply to passthrough commands.

## Backends

### Native TS-590

The native backend speaks the radio protocol directly and is the first-class path for
TS-590 stations. It owns `AI2;`, normalizes VFO and split state, deduplicates modeled writes,
and reads configured power from `PC;`.

### External rigctld

The bridge backend is the sole client of a private downstream `rigctld`. Client applications
still connect to CatHub, not to that private process. Required state probes drive the common
snapshot. Optional split and power probes can return Hamlib's not-supported result without
failing baseline operation.

### Loopback

The loopback backend supports deterministic tests without radio hardware.

## Client endpoints

### Hamlib NET

Each `[[hamlib_net]]` entry creates a TCP listener with its own permissions.
Use separate listeners for different authority groups, such as read-only monitors and
digital-mode programs allowed to write and key PTT.

The endpoint implements the tested subset of the `rigctld` network protocol.
It includes ordinary and extended-response forms for supported clients.
Captured transcripts and tests define compatibility. CatHub does not claim support for every Hamlib command.

### Serial CAT

Each `[[serial_endpoint]]` either selects a CatHub-owned Windows UMDF endpoint with
`virtual_endpoint` or binds the daemon side of an externally provisioned virtual serial pair with
`transport`. The application opens `application_transport` in either case. A configured dialect
parses the client's command stream and translates modeled operations into the shared scheduler.

The transparent TS-590 dialect relays the real dual-VFO stream and therefore cannot use
single-VFO presentation. Modeled TS-590 and TS-2000 endpoints can enable `single_vfo` when a
client cannot operate correctly while VFO B is active.

### Single-VFO presentation

`single_vfo = true` presents the current operating VFO as VFO A without changing the radio.
Reads, writes, and notifications still target the real operating VFO.
This view cannot represent real A/B split. Thus, CatHub rejects split-enable requests.

## Permissions

Each endpoint has fixed permissions:

| Permission | Allows |
|---|---|
| `read` | State queries and non-mutating capability queries |
| `frequency_write` | Frequency changes without general mode or configuration writes |
| `write` | Modeled operating-state changes |
| `ptt` | Transmit and receive requests through the PTT lease |
| `config_write` | Persistent or maintenance-oriented device changes |

Unknown commands and unauthorized commands fail closed. A TCP listener is a policy group,
not a user identity boundary, so clients with different authority requirements need
different listeners.

## PTT ownership and safety

CAT PTT and WinKeyer transmit jobs use one station-wide ownership manager. Only one session
can own transmit authority at a time. A conflicting request fails deterministically.

The safety rules are:

- every transmit lease has a configured maximum duration
- disconnecting the active owner forces receive and releases the lease
- backend or keyer failure releases transmit ownership
- graceful shutdown clears queued keying, forces key-up, and returns the radio to receive
- maintenance cannot begin while transmission is active or queued

Automated tests use loopback transports and must not key physical hardware.

## Configuration

Standalone configuration uses top-level tables:

```toml
[radio]
backend = "ts590"
transport = "serial"
port = "COM4"
baud = 57600

[[hamlib_net]]
name = "logger-readonly"
bind = "127.0.0.1:4532"
perms = ["read"]

[[serial_endpoint]]
name = "contest-logger"
virtual_endpoint = "n1mm-cat"
application_transport = "COM21"
dialect = "ts590"
single_vfo = true
perms = ["read", "write", "ptt"]
```

You can put the same tables beneath `[cat_hub]` in a managed TOML document. CatHub owns
schema validation in both layouts. See [operator setup](../integration/setup.md) and the
complete [sample configuration](../../config/cathub.toml).

## Runtime lifecycle

Startup proceeds in fail-closed order:

1. Load and validate configuration.
2. Open the radio backend and establish initial state.
3. Open the optional physical WinKeyer.
4. Bind every configured client endpoint.
5. Start polling, push reconciliation, and client session tasks.

A required bind or configuration failure stops startup. Runtime transport failures use
bounded reconnect backoff while listeners and diagnostic state remain available when safe.

## Verification

The repository gate covers formatting, Clippy, tests, protobuf lint, .NET protocol generation, and dependency policy.
Fixtures and transcript tests verify protocol compatibility for supported commands.

An operator must attend hardware acceptance tests.
Validate one client at a time.
Confirm read-only rejection and PTT timeout behavior.
Then add concurrent clients while you monitor the CatHub log.

## Implementation map

- `crates/cathub/src/radio`: scheduler and physical radio link
- `crates/cathub/src/state.rs`: shared snapshot
- `crates/cathub/src/backend`: radio backends
- `crates/cathub/src/dialect`: serial client dialects
- `crates/cathub/src/hamlib_net.rs`: Hamlib NET listeners
- `crates/cathub/src/serial_endpoint.rs`: virtual serial sessions
- `crates/cathub/src/ptt.rs`: station PTT ownership
- `crates/cathub/src/winkeyer`: WinKeyer actor, broker, endpoints, and typed API
