# CatHub virtual serial

This internal crate contains the private CatHub virtual serial contract.
It also contains the Windows serial conformance tool.

The contract is not a public CatHub client API.
The UMDF driver and `cathub.exe` use the contract through a private device interface.

List the conformance profiles:

```powershell
cargo run -p cathub-virtual-serial --bin serial-conformance -- profiles
```

Run the harness with an isolated virtual serial pair:

```powershell
cargo run -p cathub-virtual-serial --bin serial-conformance -- run `
  --application-port COM21 `
  --peer-port COM20 `
  --profile n1mm-radio `
  --output artifacts\serial-conformance\n1mm-radio.json
```

Do not use a physical radio or a physical WinKeyer for this test.

For an installed CatHub-owned UMDF endpoint, start CatHub's hidden loopback-only test peer and use
the private-channel conformance mode instead of creating a second COM port:

```powershell
cathub virtual-serial test-peer --endpoint cathub-default `
  --kind cat --listen 127.0.0.1:39116

serial-conformance run --application-port COM91 `
  --peer-tcp 127.0.0.1:39116 --profile n1mm-radio `
  --output artifacts\serial-conformance\managed-n1mm-radio.json
```

The test peer binds only the explicitly supplied address; the isolated-target harness always uses
IPv4 loopback and terminates it before starting the production daemon path.
