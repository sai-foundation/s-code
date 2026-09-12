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
launcher and records the result. It prepares the task with the tooling above,
gives the run its own S-Code service namespace, runs
`s-code exec --stream-json --ephemeral` inside the prepared workspace, keeps
the raw event stream, stops the service it started, grades the final
workspace with the unchanged protected grader and writes one `run.json`
record beside the raw artifacts:

```sh
python3 tests/benchmarks/harness/run.py \
  --track project --task durable-task-queue \
  --s-code "$HOME/.local/bin/s-code" --model <model> \
  --output .work/benchmark-runs/durable-task-queue-1
```

The output directory must be new and beneath `.work/`. It receives the
prepared `workspace/`, the exact `prompt.txt`, the raw `events.jsonl` rows the
CLI printed, `s-code.stderr.log`, the grader's `grade.json` and `grade.log`,
`run.json`, and `service/` with the home, runtime and state directories of
the daemon that served the run. Pass `--polyglot-root` for algorithm tasks
and `--playwright-browsers` for frontend tasks, exactly as for `prepare` and
`grade`. The exit status is 0 only for a comparable run.

`--ephemeral` isolates the session, not the daemon: a long-lived local
service would otherwise execute the measured turn with whatever build it was
started from. The runner therefore points `S_CODE_HOME`, `S_CODE_RUNTIME_DIR`
and `S_CODE_STATE_DIR` at fresh directories under `service/`, drops
`S_CODE_URL`, `S_CODE_TOKEN` and `S_CODE_NO_AUTOSTART`, binds the service to
an ephemeral loopback port, and lets the supplied launcher start a daemon
there; only a daemon started for this run can be discovered, and the runner
stops it afterwards. The isolated service reads the caller's configuration
file in place (`--service-config`, else `S_CODE_CONFIG`, else the S-Code home
`config.toml`) so the provider, endpoint and credential handle are the usual
ones; nothing from it is copied, and the record keeps only its digest. Pass
the launcher script as `--s-code`: the bare CLI binary starts no service and
the run stays non-comparable.

`run.json` (schema version 2) records the S-Code version and the source
revision of the checkout, the task and its protected digest, the requested
model and permission mode, the prompt digest, the wall-clock elapsed time of
the S-Code process from launch to exit, the process exit status and timeout
flag, the turn status and error code, the configured model, the effective
model the daemon routed to with any fallback, context compactions, the usage
totals below, the per-call `accounting`, the event `lifecycle` verdict, the
`service` identity block, per-kind event counts, the unmodified grader result
and relative references to every raw artifact. It never contains environment
variables, credentials or paths outside the run directory. The raw artifacts
do contain the agent's tool traffic for the workspace, and `service/state`
holds the run's own daemon database and logs, so review them before sharing.

A run is `comparable` only when every one of these holds:

- the protected grader accepted the final workspace;
- the event lifecycle is bound to one turn: the stream opens with exactly
  one `turn.started` anchor naming the session and turn, every later row
  carries those ids, and exactly one terminal `turn.completed`,
  `turn.failed` or `turn.cancelled` row closes it (rows from any other
  session or turn are counted and ignored, never folded into the evidence);
- the turn completed, the process exited normally within its timeout, and
  no event evidence was truncated or malformed;
- the daemon that executed the turn is the run's isolated service: the
  `daemon` identity the daemon stamps into `turn.created` names the instance
  published in the run's own connection file, and its version is the version
  the measured launcher reports;
- every model call the daemon counted has complete, valid usage evidence
  (see the accounting below) and the turn total equals the sum of the
  per-call usage events;
- the model route never changed;
- when the run requested a tool-history arm, the policy and budget the
  daemon reported in `turn.created` equal the request (see below).

Every other run keeps its record and artifacts with an `exclusion_reason`.
Compare only records that share the same task, S-Code revision, effective
model, permission mode and prompt version, and derive any summary from the
retained records.

#### Comparing tool-history policies

Before every model call the agent loop replaces older tool results with a
compacted stub and keeps only the most recent ones detailed. The production
default is `fixed-count`: exactly the six most recent results stay detailed
regardless of size. The evaluation-only `token-budget` policy keeps recent
results detailed only while both that count and a soft estimated budget
allow. The budget applies to the retained prior detailed history, counting
each result together with its call arguments; the newest tool-call batch is
guaranteed one observation opportunity regardless of the budget, up to the
same count of six, and the latest failed `run_command` result stays pinned
under both policies. The budget is therefore not a hard bound on the request,
but the budgeted window is never larger than the fixed-count window. The
policy is selected through daemon configuration,
`S_CODE_DAEMON_TOOL_HISTORY_POLICY` and
`S_CODE_DAEMON_TOOL_HISTORY_BUDGET_TOKENS`; invalid values stop the service
with an error rather than falling back.

The daemon reports the policy it actually applied, with its budget or `null`,
in every `turn.created` event. The runner records that report under
`turn.tool_history_policy` and `turn.tool_history_budget_tokens`, beside the
requested values under `configuration`. Pass `--tool-history-policy
fixed-count`, or `--tool-history-policy token-budget` together with a
required `--tool-history-budget-tokens`; a budget with any other policy is
rejected, and an omitted policy is recorded as `null` and means the service
default. A run with a requested policy is `comparable` only when the reported
policy and budget equal the request, so a service that ran under another
policy is excluded rather than miscounted. The request is placed in the
environment the run's isolated service starts from, so each arm's daemon is
started under its own policy and no daemon is shared between arms; the
report in `turn.created` sits beside the daemon identity that binds the turn
to that service.

Run the comparison in two stages on the six tasks with a sound frozen
contract (the four algorithm tasks, `durable-task-queue` and
`dependency-flow-runner`), holding the binary, provider, model, permission
mode and prompt version fixed. The pilot (two arms, three repeats per task,
36 runs) is diagnostic only: confirm that the candidate reduces input units,
that no task regresses catastrophically, that model calls do not balloon and
that every record's reported policy matches its arm. The confirmatory run
(two arms, five repeats per task, 60 runs) applies the pre-registered gates:
safety, candidate total passes at most one below the baseline and no task at
zero of five candidate passes while the baseline passes three or more;
efficiency, candidate median input units lower on at least four of six tasks
and the pooled median at least 15% lower; guard, candidate median model calls
at most 20% above the baseline. The 12,000-token budget stays frozen across
both stages; 8,000 and 16,000 are possible pre-registered ablations
afterwards, never post-hoc tuning.

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

#### Per-call accounting

A matching turn total is not enough: a call whose provider stream carried no
usage object leaves no trace in the sum, and the total then agrees with a
partial subtotal. The daemon therefore attributes every `model.usage` row to
its model call (`model_call`, counted from 1 within the turn) and publishes
one `model.call.completed` row per counted call with the number of usage
objects it streamed, their sums, whether any usage was observed
(`accounted`) and how the call ended. The runner's `accounting` block is
`verified` only when the records cover exactly the calls `turn.usage`
counts, each call has at least one usage object, the streamed rows of each
call reproduce its record, the records sum to the turn total, and no counter
was missing or malformed. A missing or malformed counter is counted as
invalid, never as zero. Several usage objects per call are normal for the
provider streams above; a call without any makes the run non-comparable
while keeping the subtotal in the record.

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
- The recorded source revision is that of the checkout containing the runner;
  no S-Code build embeds one. The runner verifies that the turn ran on the
  daemon it started (instance identity) and that this daemon reports the
  launcher's crate version, which is the strongest identity the build
  carries. Build the launcher's binaries from the recorded revision, and
  treat the crate version as a build family, not a commit.
- A caller configuration that pins `daemon.listen` is overridden to an
  ephemeral loopback port for the isolated service, so two runs and the
  caller's own daemon can coexist.
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
