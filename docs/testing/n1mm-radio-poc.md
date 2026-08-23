# N1MM radio CAT proof-of-concept runbook

## Purpose

This runbook is the first client evidence path for issue #6.
It exercises N1MM Logger+ through an isolated TS-590 loopback and never connects to a physical
radio or a keyer.

COM21 is N1MM's application side and COM20 is CatHub's side on the current development station.
Confirm the pair before every application run; do not assume the numbers on another system.

## Baseline before installing a CatHub driver

1. Choose an unused isolated com0com pair and record both ports.
   COM20/COM21 cannot be used while the station CatHub and N1MM processes own them.
2. Confirm both selected ports are com0com devices and no physical radio is selected.
3. Run the Phase 1 conformance profile:

   ```powershell
   cargo run -p cathub-virtual-serial --bin serial-conformance -- run `
     --application-port <APPLICATION-COM> `
     --peer-port <PEER-COM> `
     --profile n1mm-radio `
     --output docs\testing\evidence\n1mm-radio-com0com-baseline.json
   ```

4. Preserve the JSON report with the branch evidence.
5. Stop the normal station CatHub process through its normal shutdown procedure.
6. Configure N1MM's radio as a TS-590 on COM21 with PTT and keying disabled.
7. Start a CatHub read-only TS-590 loopback configuration on COM20.
8. Capture serial API/IOCTL activity with the approved Windows tracing method.
9. In N1MM, connect, read frequency and mode, disconnect, reconnect, then close N1MM.
10. Redact raw CAT payloads from the saved application trace.
11. Record the trace tool/version, N1MM version, Windows build, port settings, and evidence path in
    `serial-client-inventory.md`.
12. Stop the loopback instance and restore the normal station CatHub process.

The conformance harness and N1MM cannot own COM21 at the same time.
The harness baseline and application trace are separate runs.

## CatHub UMDF run

Do not begin this section until the driver exposes a real Ports-class device and the private test
channel has bounded queues and deterministic cleanup.

1. Use the isolated driver test target, not the working station setup.
2. Keep the existing com0com devices installed and unchanged for rollback.
3. Confirm the CatHub test process has no path to a physical radio or transmitter.
4. Start the CatHub-side test channel and record its endpoint identity.
5. Select the CatHub-owned COM port in N1MM as a TS-590 with PTT and keying disabled.
6. Exercise read-only frequency and mode queries with synchronous and overlapped traffic.
7. Stop and restart the CatHub-side process while a read is pending.
8. Restart the UMDF host in the isolated environment and verify bounded failure and reconnection.
9. Run the `n1mm-radio` conformance profile against the CatHub port and preserve its JSON report.
10. Capture UMDF diagnostics and a user-mode dump, then update the evidence inventory.

## Pass conditions

- N1MM never hangs during open, read, close, cancellation, or reconnect.
- Only the isolated endpoint receives the test bytes.
- A peer or UMDF-host failure completes outstanding work with a documented error.
- Reconnection does not require rebooting Windows.
- PTT and keying remain disabled for the entire run.
- The trace and conformance report are preserved before inventory states change.
