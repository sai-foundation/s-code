# Community compliance baseline

`baseline.json` is a machine-readable engineering control catalog used by the
Apache-2.0 `opencoding-compliance` crate. It separates controls demonstrated by
repository tests from controls that require external organizational evidence.

Run:

```sh
cargo run --locked -p opencoding-compliance > compliance-evidence.json
```

A valid report is not a certification. External identity, personnel, vendor,
incident, production and customer evidence must never be fabricated by a local
test. Reports contain control status and evidence references, not credentials,
source code, prompts or customer data.
