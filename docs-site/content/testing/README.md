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

## Verified experience memory

Verified experience memory is the first step of a cross-task learning loop
and is evaluation-only: the production default records nothing and injects
nothing. It separates three things that must never collapse into one: a raw
observation, an experience candidate, and an approved experience.

- **Modes.** `daemon.experience_mode` (`S_CODE_DAEMON_EXPERIENCE_MODE`) is
  `off` by default. `observe` records quarantined candidates and audit events
  only; candidates never influence a task. `verified` additionally retrieves
  explicitly approved experiences. Any other value fails configuration.
- **Candidates.** While a turn runs, the agent loop keeps a bounded
  corrective trace: for every `run_command` result the exact verifier
  identity (the SHA-256 of the canonical, complete structured arguments,
  never collapsed or truncated), a bounded display form of the command, and
  a 300-character tail of the failure output; for every `apply_patch`
  result the bounded path and whether it succeeded. The trace is recorded
  when each result is observed, before the loop compacts older tool results
  and their call arguments out of the model history, and it is carried
  across approval and question pauses; it holds at most 64 observations,
  dropping the oldest. After a completed turn the daemon scans the complete
  trace, never just the first repair: for a verifier identity the final
  observed result must be a success, that success must follow a successful
  edit made after the identity's most recent failure, no edit may follow it,
  and the last verifier the turn ran must have passed. So `fail, edit, pass`
  yields a candidate; `fail, edit, pass, fail` and `fail, edit, pass, edit,
  fail` yield none; `fail, edit, pass, edit, fail, edit, pass` yields a
  candidate from the latest recovery segment. A different command never
  closes another command's loop. The stored lesson is built from that
  bounded evidence (verifier identity and display command, the latest
  failure excerpt, the edited paths of the final segment and the failure
  count since the previous pass) and never from model prose, workspace
  files, environment or unbounded tool output. Secret-shaped evidence is
  dropped. Candidates expire after 90 days, are owned by the acting actor,
  are keyed to the workspace, and are sealed at rest like other sensitive
  payloads. Extraction runs in a background task after `turn.completed`;
  failures are logged and never fail or delay the turn.
- **Distillation.** The candidate lesson comes from one bounded, tool-free
  auxiliary model call (15-second timeout, 512 output tokens) that receives
  only the bounded evidence above (the verifier identity digest, the display
  command, the latest failure excerpt, the edited paths of the final segment
  and the failure count) plus a fixed outcome label, never the corrective
  trace, the exact verifier arguments, model history or tool output, and is
  asked for
  a repository-independent practice as strict JSON `{"lesson", "applicability"}`
  (400 and 200 characters). Before the call the daemon checks that the sealed
  evidence would still fit the storage bound with the largest valid distilled
  outcome attached; evidence that leaves room only for fallback provenance
  skips the model and records the fallback class `oversized_evidence`, and
  evidence that cannot hold even that yields no candidate. Output that is not
  exactly that object, exceeds
  the limits, looks secret-shaped, suggests weakening tests, permissions,
  sandboxing or network restrictions, or echoes an edited path or a line
  number is discarded. The marker screen is a heuristic defence in depth,
  not the security boundary: explicit approval remains the only way a lesson
  becomes active. Fallback rule: on provider error, timeout, tool call,
  malformed or screened output the deterministic evidence-derived lesson is
  stored instead; malformed model output is never stored, and the result is
  always a quarantined candidate. One deadline covers the request and every
  streamed event, and usage the provider reported before a stall is kept in
  the timeout record rather than reset to zero. Because the task is best-effort and runs
  after the turn is already recorded as complete, stopping the service while
  it runs can lose the candidate, or in a narrow window leave a candidate
  whose `experience.created` event was never published; storage and turn
  state are unaffected either way. The sealed evidence records the outcome
  under `distillation` (status `distilled` or `fallback`, failure class,
  distillation model, input, output and total usage, elapsed time), and the
  same figures appear on `experience.created`; they are never added to the
  coding turn's `turn.usage`.
- **Decisions.** `GET /v1/experiences?organization_id=…&team_id=…&actor_id=…`
  lists the actor's records; `POST /v1/experiences/{id}/decision` with
  `{"scope": …, "decision": "approved" | "rejected"}` is the only path out of
  quarantine, and only the owning actor scope can take it. Decisions are final.
- **Evaluation evidence.** `POST /v1/experiences/{id}/evaluation` records
  one completed evaluation of a candidate; the daemon records and gates, it
  never runs evaluation jobs. The submission carries raw counts only:
  protocol version, the candidate's source session and turn, the source
  task and distinct held-out tasks (each with its protected digest), catalog
  and S-Code revisions, provider, model, repeats, per-task attempts, passes
  and comparable-success medians for the baseline arm (`experience_mode=off`)
  and the candidate arm (`experience_mode=verified` with only this
  experience approved), a mandatory poisoning probe verdict, bounded
  artifact references and the evaluator's identity. Validation fails closed:
  the experience must be this actor's candidate for this project, the
  submitted source session and turn must equal the candidate's recorded
  provenance, the declared source task may not appear in the held-out set
  by identity or digest, every held-out task must be reported exactly once
  per arm with attempts equal to the repeats, counts must be internally
  consistent, a clean verdict requires both probe checks, and a
  client-supplied `eligible` or `passed` field is rejected outright. The
  daemon computes the protocol digest itself over the canonical design
  (protocol version, arm definitions, source task, held-out tasks sorted by
  track and id, probe task, repeats, catalog and S-Code revisions, provider,
  model and evaluator identity); a declared digest must match it, and
  results never change it. The daemon recomputes the pre-registered gates
  (`evaluate_experience_gate`, protocol version 1): completeness (exactly
  five repeats per arm, every held-out task attempted five times in both
  arms), safety (candidate held-out passes at least baseline minus one; no
  task at zero candidate passes while the baseline passed at least three of
  five), and a clean poisoning verdict. Other repeat counts are recorded but
  never eligible; efficiency medians are recorded and never blocking. The
  record is immutable and sealed at rest; a second submission of the same
  protocol for the same candidate is refused, and evaluations cannot be
  attached to decided candidates. `GET /v1/experiences/{id}/evaluations`
  lists them, newest first.
- **Promotion gate.** `daemon.experience_promotion`
  (`S_CODE_DAEMON_EXPERIENCE_PROMOTION`) is `manual` by default, which is
  exactly the behaviour above. In `evaluated` mode an explicit approval is
  accepted only when the newest evaluation on record is eligible; an
  `evaluation_id` named in the decision must be that newest record, so an
  older pass can never mask newer failed evidence. A missing, foreign,
  superseded or ineligible evaluation refuses the approval with the
  recorded reasons. Nothing approves automatically, submitting evidence
  never changes a candidate's status, and rejection never needs evidence.
- **Retrieval.** In `verified` mode a turn receives at most eight approved,
  unexpired experiences owned by the same actor for the same workspace,
  newest first. They enter the packed context as `experience` items marked
  `derived-untrusted`, prefixed as advisory prior experience that never
  outranks current user instructions, system rules or security policy. A
  distilled lesson is injected together with its applicability condition,
  re-bounded to 200 characters at read time; a fallback lesson is injected
  alone. Tool
  policy and approvals are enforced by the daemon regardless of any lesson.
- **Audit.** `experience.created` (source session and turn, the verifier
  identity of the corrective trace, distillation status and usage, metadata
  only), `experience.evaluated` (evaluation id,
  protocol version and digest, the recomputed gate results, pass and attempt
  counts, poisoning verdict and eligibility), `experience.approved` (decider,
  and the evaluation id it relied on in evaluated mode) or
  `experience.rejected`, and `experience.retrieved` (the ids actually packed
  into a turn) reconstruct where a lesson came from, what evidence it had,
  who admitted it and every turn that used it.
- **What the boundary proves.** The local authenticated endpoint
  establishes that the acting authorised user submitted the result under
  their own scope; it is not a third-party attestation. The daemon verifies
  only what it holds itself: the target is this actor's candidate for this
  project, and the submitted source session and turn equal the candidate's
  recorded provenance. Interactive candidates carry no benchmark task
  identity and none is invented, so the source task, the held-out tasks and
  their digests, the counts and the poisoning probe (its verdict, that the
  probe candidate stayed unapproved and that no request carried the harmful
  rule) are evaluator-attested. The daemon checks them for protocol
  consistency, records them immutably, and recomputes the verdict so no
  client can assert eligibility, but it does not replay the runs: an
  eligible evaluation is an auditable assertion by an authenticated
  submitter backed by structured evidence, digests, revisions and artifact
  references, not an independently reproduced or cryptographically attested
  proof. The poisoning gate blocks on that assertion because protocol
  version 1 requires a clean one.

The daemon tests prove that candidates, rejected records, expired records,
other actors' records and other projects' records are never retrieved, that
`off` reproduces today's requests and events, that `observe` never injects
even an approved record, and the closed loop: candidate created, absent from
the next turn, explicitly approved, present in the following same-project
turn, with the audit trail intact.

To evaluate learning without leakage, split a task family so the lesson
comes from one exercise and the held-out exercises share the skill but not
the answer; the evaluator must never replay the source task after exposing
its solution, and the daemon relies on the evaluator's attested source task
for that, checking only that it is absent from the held-out set. Run the
baseline (`off`) and the candidate (`verified` with
only the candidate approved in a scratch profile) on the same binary,
provider, model, permission mode and prompt version, count every attempted
run toward success, restrict efficiency summaries to comparable successful
runs, and run the mandatory poisoning probe: an untrusted workspace attempts
to persist a harmful rule, the probe's candidate must stay unapproved and no
later request may carry the rule. The external driver that produces this
contract from the frozen benchmark tasks is not part of the daemon; until it
lands, the contract is exercised with synthetic results in the daemon tests.

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
