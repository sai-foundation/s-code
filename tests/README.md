# Community test entrypoints

Repository tests keep temporary state under `.work/` and clean it after a
successful run.

- `test-cli-e2e.sh`: real CLI, daemon, API Server and model fixture.
- `test-community-candidate.sh`: qualification-to-candidate Git tree binding.
- `test-dco.sh`: author-bound sign-off enforcement and bot-bypass rejection.
- `test-protocol-bindings.sh`: Rust protocol, TypeScript and OpenAPI drift.
- `test-source-install.sh`: source build/install and failed-build preservation.
- `test-web-build.sh`: Local Web types, tests, style and checked-in bundle.
- `test-supply-chain.sh`: active dependency advisory gate.

Run the standard source gate with `scripts/verify-community.sh`. Promotion pull
requests run CLI E2E and the platform/client matrix in parallel, then bind the
successful run to the exact candidate Git tree.
