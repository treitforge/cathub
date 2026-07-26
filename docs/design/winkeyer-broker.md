# CatHub multi-client WinKeyer broker

## Decision

CatHub is the sole owner of the physical WinKeyer serial port. Native clients use a typed
loopback gRPC API. Each unmodified legacy program receives a dedicated virtual WinKeyer
serial endpoint. The keyer subsystem is independent of the radio CAT actor, but both use the
station PTT ownership manager.

```text
physical WinKeyer <-- 8-N-2 --> CatHub WinKeyer actor
                                      |-- typed loopback API --> native client
                                      |-- virtual COM endpoint ----> N1MM
                                      `-- virtual COM endpoint ----> maintenance tool
```

## Protocol and session model

One incremental parser reads each WinKeyer command from a virtual endpoint.
It keeps partial fixed-length and variable-length commands between reads.
The parser limits the largest command to the 258-byte EEPROM load frame.
It sends ordinary Morse data separately.
CatHub virtualizes Host Open, Host Close, firmware revision, status requests, and speed-pot requests for each client.
A virtual close does not close the physical session during normal operation.

Buffer-pointer commands are active-stream operations.
They are not persistent client configuration.
CatHub forwards each pointer command one time under active-owner control.
It does not replay the command before later text.
CatHub tracks the profile that it applies to the physical keyer.
It replays a profile only when client ownership changes.

A stream that starts with Buffered Speed (`1C`) supplies its own job speed.

Thus, CatHub does not insert an unbuffered speed command.
Do not insert configuration or speed commands in the N1MM pointer sequence.
An inserted command corrupts keyboard CW.
It can also key the WPM byte as a false character.

CatHub keeps authorized WinKeyer command and text bytes in their original order.
N1MM's append-pointer command is `16 02 <position>`.
The position byte is part of that command.
Do not interpret it as an Admin prefix before the buffered-speed command.
The fail-closed maintenance policy applies to invalid or disruptive Admin commands.

CatHub uses the WinKeyer tag bits to classify device bytes.
The classifications are status, speed-pot, and echo events.
CatHub sends status and pot events to all applicable clients.
Only the active stream owner can see echo bytes.
The primary endpoint can also see them during physical paddle break-in.

Only the maintenance owner can see maintenance response bytes.
These bytes do not enter typed event streams.

Virtual sessions keep independent WK1/WK2/WK3 modes so one client's pushbutton/status choice does not alter another client's status format.

## Scheduling and transient state

Typed sends are atomic jobs.
A virtual client gets a raw stream lease when its first data reaches the scheduler.
The scheduler adds more data from that client to the same active stream.
Jobs use one global arrival-order FIFO.
This FIFO keeps the order for each client.
It also prevents later submissions from blocking an earlier job.

CatHub does not put client bytes or commands inside another job.

The scheduler stores the speed and transient register profile for each client.
Before a job, it applies that profile and the requested speed.
After the queue drains, it restores the primary endpoint profile.
It also restores the fixed or pot-controlled speed.

Physical paddles keep the keyer's native priority.
They can interrupt machine-sent Morse.

A speed-pot byte contains `actual WPM - MIN_WPM`.
The broker tracks the active Speed Pot Setup minimum.
It keeps the raw offset for protocol-compatible endpoints.
It publishes actual WPM through the typed status and event contract.

## Abort and safety rules

- A queued cancel removes only jobs owned by that client.
- The broker accepts Clear Buffer only from the active stream owner or primary idle controller.
- An active-client disconnect clears the physical buffer, forces key-up, cancels that client's job, and records the safety action.
- The broker owns one maximum-transmit watchdog across all clients.
- CAT PTT and WinKeyer transmit jobs acquire the same station lease. A conflicting owner receives a failed-precondition response.
- USB/read/write failure cancels queued work, releases station PTT, marks status disconnected, and reopens the physical port with bounded backoff. The radio hub and client API stay alive.
- Graceful shutdown clears the buffer, forces key-up, sends physical Host Close, and releases station PTT.

## Maintenance

Reset, calibration, EEPROM load or dump, firmware update, and other maintenance commands require `config_write`.
`config_write` also requires `status` and `control`.
CatHub grants a lease only when no transmission is active or queued.

When CatHub grants the lease, it clears the buffer and removes keying.
It closes the physical host session before it forwards the administrative command.
The WinKeyer protocol requires this sequence.
Other sends receive repeatable busy errors.
CatHub sends replies only to the owner.
After virtual Host Close or client loss, CatHub sends physical Host Open.

It waits for the firmware byte.
Then it applies safe initialization and the foreground transient profile.
Normal scheduling then continues.

Routine keying never requires EEPROM writes. Do not give normal operating endpoints
`config_write`.

## Configuration

Standalone configuration uses `[winkeyer]` and `[[winkeyer_endpoint]]`.
Managed documents put the same tables below `[cat_hub]`.
The API must bind to loopback.
Physical and virtual transports must be different.
Only one endpoint can be primary.
CatHub validates dependent permission combinations.

See [operator setup](../integration/setup.md) for the complete workflow.
