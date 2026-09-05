# Community test entrypoints

Repository tests keep temporary state under `.work/` and clean it after a
successful run.

- `test-cli-e2e.sh`: real CLI, daemon, API Server and model fixture.
- `test-ci-workflow.sh`: parallel CI source-gate and shared-cache contract.
- `test-source-install-real.sh`: builds the real release with the public source
  installer, then exercises the installed binaries through the first-run test.
- `test-verification.py`: source-archive preflight and documentation checks after
  IDE dependency installation.
- `test-first-run.sh`: clean-home setup, secure daemon autostart, encrypted
  storage, Doctor diagnostics and one real guarded workspace edit.
- `test-privacy-security-use-cases.sh`: ten executable controls covering the
  command sandbox, network and workspace boundaries, secret protection,
  browser isolation, approval scope, audit minimization and private storage.
- `test-harness-benchmark.py`: validates, prepares and grades the frozen
  algorithm, project and frontend benchmark tasks under `tests/benchmarks/`.
- `test-harness-grader-integrity.sh`: rejects a defined set of accidental or
  common grader-tampering patterns. It is a regression check, not an
  adversarial anti-cheat boundary.
- `test-community-candidate.sh`: qualification-to-candidate Git tree binding.
- `test-dco.sh`: author-bound sign-off enforcement and bot-bypass rejection.
- `test-protocol-bindings.sh`: Rust protocol, TypeScript and OpenAPI drift.
- `test-source-install.sh`: source build/install and failed-build preservation.
- `test-web-build.sh`: Local Web types, tests, style and checked-in bundle.
- `test-supply-chain.sh`: active dependency advisory gate.

Run the standard source gate with `scripts/verify-community.sh`. Promotion pull
requests run CLI E2E and the platform/client matrix in parallel, then bind the
successful run to the exact candidate Git tree.
