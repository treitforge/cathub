# Virtual serial transport Phase 1

## Scope

This document defines the private transport between the CatHub daemon and the UMDF driver.
It also defines the first serial conformance process.

The private transport is not a public client API.
Do not expose it through the application COM port.
Do not move CAT, WinKeyer, PTT, or permission policy into the driver.

## Shared implementation

The `cathub-virtual-serial` crate contains the shared contract.
The future UMDF driver and `cathub.exe` must use this crate.
The crate also contains the `serial-conformance` command.

The crate is not published to crates.io.
The driver package and daemon release must contain compatible crate revisions.

## Device channel

The driver exposes one private device interface.
Windows restricts this interface to the CatHub service identity and local administrators.
An application COM handle cannot send private frames.

The channel is a bidirectional byte stream.
Each write can contain part of a frame or more than one frame.
The receiver must retain an incomplete frame.
The receiver must reject a frame larger than the negotiated limit.

## Frame header

All multi-byte integers use little-endian byte order.
The fixed header has 40 bytes.

| Offset | Size | Field | Rule |
|---:|---:|---|---|
| 0 | 4 | magic | Exact bytes `CHVS` |
| 4 | 2 | header length | `40` for header version 1 |
| 6 | 2 | message kind | Value from the message table |
| 8 | 2 | protocol major | Incompatible contract generation |
| 10 | 2 | protocol minor | Compatible feature generation |
| 12 | 4 | flags | Response, final, and acknowledgment flags |
| 16 | 4 | payload length | Bytes after the header |
| 20 | 8 | endpoint ID | Zero for device operations |
| 28 | 8 | request ID | Zero for unsolicited events |
| 36 | 4 | reserved | Sender sets zero. Receiver ignores the value. |

The maximum frame size before negotiation is 1 MiB.
The `Hello` exchange can select a smaller value.

## Payload fields

Each payload uses type-length-value fields.
A field contains a two-byte tag, a four-byte length, and the field bytes.
Tag zero is reserved.
A message cannot contain the same tag more than once.

A receiver rejects a missing required field.
A receiver ignores an unknown optional field.
A sender does not change the meaning of an existing tag.

## Version rules

Protocol version 1.0 is the first contract.
A major version change can remove a field or change a field meaning.
A minor version change can add a message or an optional field.

The daemon sends `Hello` first.
It supplies its lowest version, highest version, frame limit, receive window, and features.
The driver returns `HelloAck` with one selected version.
The peers disconnect when they cannot select one major version.

A peer can use a feature only when both peers advertise its bit.
The first implementation requires all version 1.0 feature bits.

## Message table

| ID | Message | Direction | Purpose |
|---:|---|---|---|
| 1 | `Hello` | Daemon to driver | Offer versions and features. |
| 2 | `HelloAck` | Driver to daemon | Select a version and features. |
| 3 | `Discover` | Daemon to driver | Request the endpoint set. |
| 4 | `Endpoint` | Driver to daemon | Describe one endpoint. |
| 5 | `DiscoverComplete` | Driver to daemon | Complete endpoint discovery. |
| 6 | `Attach` | Daemon to driver | Attach to one endpoint. |
| 7 | `AttachAck` | Driver to daemon | Confirm the attachment. |
| 8 | `Detach` | Either direction | End an endpoint session. |
| 9 | `Data` | Either direction | Transfer application bytes. |
| 10 | `WindowUpdate` | Either direction | Add receive credit. |
| 11 | `SerialConfig` | Driver to daemon | Report serial settings. |
| 12 | `ModemControl` | Driver to daemon | Report DTR, RTS, and break state. |
| 13 | `ModemStatus` | Daemon to driver | Report CTS, DSR, DCD, and RI state. |
| 14 | `ApplicationOpen` | Driver to daemon | Report an application open. |
| 15 | `ApplicationClose` | Driver to daemon | Report an application close. |
| 16 | `Cancel` | Driver to daemon | Cancel a serial operation. |
| 17 | `Purge` | Driver to daemon | Purge or reset a queue. |
| 18 | `Health` | Daemon to driver | Request transport health. |
| 19 | `HealthAck` | Driver to daemon | Return transport health. |
| 20 | `Error` | Either direction | Reject a request or report a fault. |

The Rust constants in `protocol.rs` define the field tags.
The header `request_id` correlates a response with a request.
The header `endpoint_id` selects one configured endpoint.

## Endpoint identity

The driver assigns one 64-bit endpoint ID for each device instance.
The ID remains stable while the device instance exists.
The discovery payload also contains the stable CatHub configuration ID.

Endpoint kind 1 is CAT.
Endpoint kind 2 is WinKeyer.
The daemon rejects an attachment when the configured kind does not match.

## Session sequence

The driver sends `ApplicationOpen` after an application opens the COM port.
It gives the open a new session sequence.
The driver includes this sequence in later session events.

The daemon sends `Attach` before it accepts application data.
The driver returns `AttachAck` before it sends `Data`.
The peers send `Detach` before a normal stop.

An application close revokes its PTT and keying state.
A private channel failure revokes all endpoint-owned PTT and keying state.
A UMDF host failure has the same result.

## Flow control

Each attachment starts with a byte credit value.
A sender subtracts each `Data` payload size from its available credit.
A sender stops data frames when the credit reaches zero.

The receiver sends `WindowUpdate` after it releases buffer space.
The receiver does not add credit above its configured buffer limit.
Control messages do not use data credit.

The driver and daemon must use bounded queues.
A full queue does not discard a control message.
A peer can detach the endpoint when control progress stops.

## Error behavior

The receiver rejects bad magic, an unsupported header, and a duplicate field.
The receiver also rejects an oversized frame and a missing required field.

An `Error` response uses the request ID from the rejected request.
An unsolicited fatal error uses request ID zero.
The peer does not include raw CAT or WinKeyer data in an error detail.

## Conformance command

Use an isolated virtual serial pair for Phase 1 tests.
Do not connect the harness to a physical radio or keyer.

List profiles:

```powershell
cargo run -p cathub-virtual-serial --bin serial-conformance -- profiles
```

Run one profile:

```powershell
cargo run -p cathub-virtual-serial --bin serial-conformance -- run `
  --application-port COM21 `
  --peer-port COM20 `
  --profile n1mm-radio `
  --output artifacts\serial-conformance\n1mm-radio.json
```

The command tests blocking I/O, overlapped I/O, cancellation, timeouts, purge, and event masks.
It also tests serial settings, modem control, and queue status.
The command returns an error when a required case fails.

## Phase 1 exit criteria

- The shared crate compiles on Windows and Linux.
- Contract unit tests cover framing, stream splits, field errors, and version selection.
- The Windows harness writes a versioned JSON report.
- Each supported client has a profile.
- An operator records one client trace for each supported client interface.
- The inventory links each requirement to a trace or test report.

The last two criteria require the supported Windows applications.
Do not mark the Phase 1 application inventory complete before those runs.
