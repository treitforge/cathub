# Serial client inventory

## Purpose

This inventory records the Windows serial behavior for each supported client.
The conformance profiles define the initial required test set.
Application traces must confirm the final set.

## Evidence states

- `Planned` means that issue #6 supplies the requirement.
- `Observed` means that an application trace shows the operation.
- `Verified` means that the conformance report passes against the CatHub driver.

Do not change a state to `Observed` without a saved trace.
Do not change a state to `Verified` without a saved JSON report.

## Current inventory

| Profile | Client interface | Serial format | Trace state | Driver report state |
|---|---|---|---|---|
| `hdsdr-omnirig` | HDSDR through OmniRig, TS-2000 | Client setting | Planned | Verified |
| `n1mm-radio` | N1MM Logger+ radio CAT, TS-590 | Client setting | Planned | Verified |
| `arcp-590` | Kenwood ARCP-590 | Client setting | Planned | Verified |
| `n1mm-winkeyer` | N1MM Logger+ WinKeyer | 1200 8-N-2 | Planned | Verified |
| `wktools` | WKTools maintenance | 1200 8-N-2 | Planned | Verified |

## Initial behavior matrix

`R` means required.
`O` means optional until a trace confirms use.

| Behavior | HDSDR | N1MM CAT | ARCP-590 | N1MM WinKeyer | WKTools |
|---|:---:|:---:|:---:|:---:|:---:|
| Blocking read and write | R | R | R | R | R |
| Overlapped read and write | R | R | R | R | R |
| Cancel pending read | R | R | R | R | R |
| Read timeout | R | R | R | R | R |
| Purge receive queue | R | R | R | R | R |
| `WaitCommEvent` with `EV_RXCHAR` | R | R | R | R | R |
| Serial configuration | R | R | R | R | R |
| DTR, RTS, and break control | O | O | O | O | O |
| Queue status | R | R | R | R | R |
| Atomic buffer-saturation rejection and recovery | R | R | R | R | R |
| Exclusive open, close, and reopen | R | R | R | R | R |

## Trace procedure

1. Use an isolated Windows 11 test system.
2. Keep Secure Boot and Memory Integrity enabled.
3. Connect the client to its test COM port.
4. Capture the client serial API calls with an approved Windows trace tool.
5. Exercise connect, normal use, disconnect, crash, and reconnect.
6. Record each API, IOCTL, input, output, timeout, and error.
7. Remove raw CAT and keying data from the saved trace.
8. Save the trace reference in this inventory.
9. Update the matching conformance profile.
10. Run the profile against the CatHub driver.
11. Save the JSON report reference in this inventory.

An operator must attend a test that can key a transmitter.
The Phase 1 harness must use a virtual pair only.

## Evidence record

Add one row for each application trace or driver report.

| Date | Profile | Environment | Evidence type | File or URL | Result | Notes |
|---|---|---|---|---|---|---|
| 2026-08-14 | `n1mm-radio` | Windows development station | Readiness check | [N1MM PoC runbook](n1mm-radio-poc.md) | Blocked | N1MM and the station CatHub process held COM21/COM20. The alternate pair was also in use. No trace was captured and no evidence state changed. |
| 2026-08-22 | All five profiles | Windows 11 Pro x64 build 26200, local development target | Installed-driver conformance report | `target/cathub-umdf-e2e-final.json` (local ignored evidence), PR #13 summary | Passed | Exact `f641c2b` package; 11/11 per profile and 55/55 total through COM91 and the private CHVS channel. Test Signing was enabled, Memory Integrity was running, integrity checks were enabled, and Secure Boot was disabled under an explicit development exception. Actual application traces remain planned. |
