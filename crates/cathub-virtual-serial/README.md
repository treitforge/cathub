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
