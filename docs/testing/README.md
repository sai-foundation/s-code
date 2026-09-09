---
site: true
slug: verification
title: Verification and releases
short_title: Verification
group: Operate
order: 90
description: Run the Community gate and understand private release-candidate evidence.
keywords:
  - testing
  - verification
  - release
  - CI
  - source
---

# Verification and releases

## Complete gate

Run the complete Community validation from the repository root:

```sh
scripts/verify-community.sh
```

The local command runs the complete sequential source gate. Hosted full
verification adds the cross-platform matrix and IDE checks described below.
Use focused checks while editing; a README correction does not require a local
release build.

The gate checks the repository manifest, documentation, formatting, Clippy,
Rust tests, dependency policy, advisories, generated protocol bindings, Local
Web, the product documentation site and the source installation contract.

## Pull-request CI

Every PR runs source-boundary, secret-history, documentation-link, DCO and CI
regression checks. Extra jobs are selected from the complete Git diff:

| Changed area | Additional checks |
| --- | --- |
| Root README, governance and issue templates | None |
| Documentation or documentation site | Documentation generation, lint and build |
| Rust code | Linux lint/tests and macOS tests; runtime crates also run CLI E2E and first-run checks on both systems |
| Local Web | Web tests/build and Linux/macOS CLI and first-run integration |
| VS Code or JetBrains client | That client's tests and packaging; VS Code also runs the real extension-host test |
| Shared protocol | Rust, generated bindings, Web and both IDE clients |
| Installer or launcher | Actual optimized source installation and first run on Linux/macOS |
| Benchmark fixtures/runner | Frozen-task and grader-integrity checks |
| Privacy/security use-case runner or cases | The explicit use-case suite on Linux/macOS |
| Dependency files, toolchain, workflows or an unclassified area | Affected dependency audits, or all supported-platform checks for shared/unknown inputs |

The fixed `source-gate` check aggregates the results. A selected job must
succeed; a failed, cancelled, missing or unexpectedly skipped job blocks it.
Do not require each conditional job separately in branch protection. The
workflow itself always starts, including for documentation-only changes.
When public, JavaScript/TypeScript changes also select CodeQL, which is included
in `source-gate`.

Linux tests, policy checks and CLI integration share one Cargo build directory.
Caches persist package downloads and build outputs, superseded runs on the same
PR are cancelled, and formatting runs once. Ordinary runtime changes use
development binaries for first-run coverage; installer changes and full runs
exercise the actual optimized installation.

Every week, and on manual dispatch, `rc.yml` reuses the same workflow with all
checks enabled. This adds Windows Rust compatibility, platform-specific lint,
JetBrains compatibility verification, actual installation, explicit privacy
evidence and online dependency audits. Dependency-changing PRs also run the
affected audits immediately. Windows native execution remains outside the
supported Preview; its compatibility matrix is not required on every PR.

## Focused checks

Focused entrypoints are documented in [`tests/README.md`](../../tests/README.md).
The release-candidate gate additionally runs the real CLI/daemon/model fixture:

```sh
tests/test-source-install-real.sh
tests/test-cli-e2e.sh
```

The source-installation test builds the actual release through the public
installer before running the first-run black-box test. That test installs the public launcher into an isolated
home, configures a credential handle, proves the secret is not persisted,
autostarts the service, verifies managed encryption with `doctor`, completes a
real guarded file edit, stops the service, exercises offline backup and
integrity verification, and requires normal exit in under ten minutes.

Run archive verification from a fresh extraction, before build outputs and
downloaded dependencies exist. Use a Git clone for repeated development checks.

Use focused entrypoints while iterating, then run the complete gate against the
exact source revision intended for release qualification.

## Coding harness benchmarks

The coding harness is evaluated by outcome first: a run enters a performance
comparison only after the same frozen trusted-workspace check accepts its workspace.
Comparisons must use the same task fixture, starting state, model route and
model. Latency is wall-clock elapsed time and token counts come from the same
provider accounting boundary.

The frozen tasks and outcome graders live under
[`tests/benchmarks/`](../../tests/benchmarks/). The manifest records a digest of
every protected path, including file contents, executable bits, symlinks,
missing paths and unexpected additions. Run:

```sh
python3 tests/test-harness-benchmark.py validate
tests/test-harness-grader-integrity.sh
python3 tests/benchmarks/harness/test_run.py
```

The local benchmark runner is a repeatable engineering tool, not a secure
anti-cheat supervisor. Python checks import candidate code into the checker
process, while frontend checks execute candidate JavaScript in the browser. It therefore
must be used only with trusted workspaces, or inside an outer container or VM.
Its integrity checks reject the tampering patterns covered by the tests, but a
hostile candidate can still attempt self-modification, test interference or
result forgery.

For a publishable cross-harness result, reviewers must inspect the candidate
diff for test-interference code, then export the workspace to a clean supervisor.
That supervisor reruns the check, verifies protected digests both before and
after execution, and retains its raw output. Neither this clean rerun nor an
outer VM turns same-process language tests into a hostile-code boundary; a pass
from the local runner alone is not evidence for a competitive performance claim.

A public comparison must check in the exact public S-Code revision,
competitor version and configuration, model identity, raw per-run artifacts,
provider usage, grader output, failures and stopped runs. Summary medians and
percentage claims are derived only from those artifacts.

### Measuring an S-Code run

`tests/benchmarks/harness/run.py` runs one frozen task through an S-Code
binary and records the result. It prepares the task with the tooling above,
runs `s-code exec --stream-json --ephemeral` inside the prepared workspace,
keeps the raw event stream, grades the final workspace with the unchanged
protected grader and writes one `run.json` record beside the raw artifacts:

```sh
python3 tests/benchmarks/harness/run.py \
  --track project --task durable-task-queue \
  --s-code "$HOME/.local/bin/s-code" --model <model> \
  --output .work/benchmark-runs/durable-task-queue-1
```

The output directory must be new and beneath `.work/`. It receives the
prepared `workspace/`, the exact `prompt.txt`, the raw `events.jsonl` rows the
CLI printed, `s-code.stderr.log`, the grader's `grade.json` and `grade.log`,
and `run.json`. Pass `--polyglot-root` for algorithm tasks and
`--playwright-browsers` for frontend tasks, exactly as for `prepare` and
`grade`. The exit status is 0 only for a comparable run.

`run.json` (schema version 1) records the S-Code version and the source
revision of the checkout, the task and its protected digest, the requested
model and permission mode, the prompt digest, the wall-clock elapsed time of
the S-Code process from launch to exit, the process exit status and timeout
flag, the turn status and error code, the configured model, the effective
model the daemon routed to with any fallback, context compactions, the usage
totals below, per-kind event counts, the unmodified grader result and relative
references to every raw artifact. It never contains environment variables,
credentials or paths outside the run directory. The raw artifacts do contain
the agent's tool traffic for the workspace, so review them before sharing.

A run is `comparable` only when the protected grader accepted the final
workspace, the turn completed, the process exited normally within its timeout,
the event evidence is complete, the daemon reported non-zero turn usage that
matches the sum of the per-call usage events, and the model route never
changed. Every other run keeps its record and artifacts with an
`exclusion_reason`. Compare only records that share the same task, S-Code
revision, effective model, permission mode and prompt version, and derive any
summary from the retained records.

#### Usage semantics

The `usage` block reports provider usage units, not an audited token bill,
under the accounting `sum_of_provider_usage_events`: the model gateway turns
every usage object a provider streams into one usage event, and the agent
loop adds them all up for the turn. `turn.usage` is the daemon's total,
`model.usage` rows are the individual events, and the record keeps both so the
total can be checked against its parts. What one unit means depends on the
provider stream:

| Provider stream | What is summed | Consequence |
| --- | --- | --- |
| OpenAI-compatible | `usage.prompt_tokens` and `completion_tokens` of every chunk that carries `usage`; the request asks for a single final usage chunk | Exact per call when the endpoint honours `stream_options.include_usage` and reports usage once; an endpoint that repeats cumulative usage in every chunk is over-counted. Cached prompt tokens are included in the input count. |
| Anthropic Messages | `message_start` input and output counts plus `message_delta` output count | `message_delta` carries the cumulative output count, so any initial output count in `message_start` is added on top. Cache-read and cache-creation tokens are not part of `input_tokens` and are not represented. |
| Gemini | `usageMetadata` of every chunk that carries it | Exact for a stream that reports usage once; a stream that repeats cumulative `usageMetadata` per chunk is over-counted. |

These are properties of the current gateway normalisation, not of this
runner. Keep the provider and model constant across compared runs, and treat
differences smaller than the per-call over-count above as noise. Normalising
each provider stream to one final usage per model call, with cache tokens
carried separately, is gateway follow-up work.

#### Known limitations

- The agent's own verification command compiles protected test modules into
  `__pycache__` directories, which the grader rejects as undeclared paths. As
  benchmark hygiene, not success logic, the runner removes directories named
  `__pycache__` that contain only regular `.pyc` files before grading and
  lists them under `workspace_normalization`; no other path is touched.
- The frozen `incident-report-cli` task declares only `tests` as protected
  and `incident_report` as editable, so its own `README.md` is rejected as an
  undeclared path before any candidate is graded. Until the catalog is
  corrected in a separate change, that task cannot produce a passing run.
- Frontend tasks need Playwright for the agent's own `npm test`, which the
  network-off sandbox cannot install, so expect agent self-verification to
  fail on that track unless you provide it.
- The recorded source revision is that of the checkout containing the runner.
  Build the measured binary from the same revision.
- Interrupting the runner keeps the artifacts written so far but produces no
  `run.json`; a run directory without a record must not enter any summary.

## Release candidates

After merging a prospective release, manually run **S-Code full verification**
(`rc.yml`) on `main`. It verifies the merged PR's sign-offs and runs the complete
matrix against that exact commit. Only a successful manual run produces a
`community-release-ready-*` artifact; regular PRs and weekly runs do not produce
release qualification evidence.

Evidence uses schema version 3 and the explicit `full` profile, recording the
complete required check set, source revision, PR head, workflow run and tested
Git tree. Partial or legacy evidence cannot qualify a release. The tag workflow
requires a successful manual `rc.yml` run from this repository for the exact
tagged commit and checks that the evidence belongs to that run and attempt. Public release
qualification also requires CodeQL. CodeQL is skipped during private staging.

Private staging does not create tags or public release bundles. After
publication is enabled and the repository is public, pushing an existing
version tag reruns the source gate and creates the verified, attested source
release described in [Preview release readiness](../deployment/preview-release.md).
The workflow never creates tags, changes visibility or uploads precompiled files.

## Current status

A private candidate becomes a public source release only after publication is
explicitly enabled by a reviewed contract change and the repository controls
have been validated.
