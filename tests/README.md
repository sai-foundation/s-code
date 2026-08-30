# Community test entrypoints

Repository tests keep temporary state under `.work/` and clean it after a
successful run.

- `test-cli-e2e.sh`: real CLI, daemon, API Server and model fixture.
- `test-protocol-bindings.sh`: Rust protocol, TypeScript and OpenAPI drift.
- `test-source-install.sh`: source build/install and failed-build preservation.
- `test-web-build.sh`: Local Web types, tests, style and checked-in bundle.
- `test-supply-chain.sh`: active dependency advisory gate.

Run the standard source gate with `scripts/verify-community.sh`. Run
`tests/test-cli-e2e.sh` for a private release candidate.
