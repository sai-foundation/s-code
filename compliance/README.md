# Community compliance baseline

`baseline.json` is a machine-readable engineering control catalog used by the
Apache-2.0 `s-code-compliance` crate. It separates controls demonstrated by
repository tests from controls that require external organizational evidence.

Run:

```sh
cargo run --locked -p s-code-compliance > compliance-evidence.json
```

A valid report is not a certification. Maintainer access, vendor review and
security-response exercises require external evidence and must never be
fabricated by a local test. A release exercise may be marked passed only with
the exact revision, command output and retained digest. Reports contain control
status and evidence references, not credentials, source code, prompts or user
data.
