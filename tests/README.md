# Community test entrypoints

Repository tests keep temporary state under `.work/` and clean it after a
successful run. Native desktop integration checks use an isolated system temporary
directory outside the checkout, removed after the run.

- `test-macos-desktop.py`: native Mac bundle, Swift protocol/regression checks,
  real-engine coding flow, provider failures and owned-parent/terminal cleanup.
- `test-cli-e2e.sh`: real CLI, daemon, API Server and model fixture.
- `test-ci-workflow.sh` / `test-ci-plan.py`: affected-area routing, complete Git
  diffs, fail-closed aggregation and PR/full/release workflow contracts.
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
- `test-community-candidate.sh`: full-profile enforcement and candidate Git
  tree, workflow-run and public CodeQL evidence binding.
- `test-dco.sh`: author-bound sign-off enforcement and bot-bypass rejection.
- `test-protocol-bindings.sh`: Rust protocol, TypeScript and OpenAPI drift.
- `test-source-install.sh`: source build/install and failed-build preservation.
- `test-web-build.sh`: Local Web types, tests, style and checked-in bundle.
- `test-supply-chain.sh`: active dependency advisory gate.

Run the complete local source gate with `scripts/verify-community.sh`. PRs run
checks for affected areas under `source-gate`. Weekly/manual full verification
adds the complete platform/client matrix; only a manual full run on `main`
produces release evidence for the exact tested commit. See the
[CI routing and release policy](../docs/testing/README.md).
