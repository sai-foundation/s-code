# Community test entrypoints

Repository tests keep temporary state under `.work/` and clean it after a
successful run.

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
- `benchmarks/harness/run.py` and `benchmarks/harness/test_run.py`: run one
  frozen task through an S-Code launcher inside an isolated service
  namespace, keep the raw `--stream-json` events, verify the turn's
  lifecycle, daemon identity and per-call usage accounting, grade the final
  workspace and write one run record; the test drives the collector with a
  fake binary and passes on a fresh checkout:

  ```sh
  python3 -m unittest discover -s tests/benchmarks/harness -p test_run.py -v
  ```
- `benchmarks/harness/evaluate_experience.py` and
  `benchmarks/harness/test_evaluate_experience.py`: evaluate one experience
  candidate on held-out tasks with the runner above (baseline against
  candidate, interleaved repeats, poisoning probe) and submit the raw counts
  to the daemon's evaluation API, which recomputes eligibility; the test
  drives the driver against a fake launcher and a fake daemon:

  ```sh
  python3 tests/benchmarks/harness/test_evaluate_experience.py
  ```
- `benchmarks/harness/test_transfer_tasks.py`: construction audit and grader
  validation for the paired experience-transfer task families (distinct
  ids, digests and vocabulary; starter packages without implementation;
  graders that accept a known-good solution and reject a wrong one):

  ```sh
  python3 tests/benchmarks/harness/test_transfer_tasks.py
  ```
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
