# Self-evolving project experience

Status: implemented behind an opt-in setting; evaluation has not established
a performance advantage. The first complete confirmatory run is inconclusive.

## Research, 2026-09-05

| Approach | Mechanism | Implication for S-Code |
| --- | --- | --- |
| [Claude Code auto memory](https://code.claude.com/docs/en/memory) | Writes project-specific notes during work; a bounded index loads across sessions and topic files load on demand. Users can inspect, edit and disable it. | Make learning visible and reversible; do not load the entire history. |
| [Codex memory configuration](https://learn.chatgpt.com/docs/config-file/config-reference) and [skills](https://learn.chatgpt.com/docs/build-skills) | Separates memory generation from use, bounds extraction/consolidation, and loads skill instructions progressively. | Separate learning from reuse, bound cost and expire unused information. |
| [OpenHands skills](https://docs.openhands.dev/sdk/guides/skill) | Repository instructions, keyword/path triggers and progressive skill loading supply reusable context. | Retrieve experience by relevance and affected files. Skill loading alone does not establish automatic learning. |
| [Letta skill learning](https://www.letta.com/blog/skill-learning/) | Reflects on task trajectories, then generates skills containing approaches, pitfalls and verification strategies. | Distill general procedures rather than replay solved answers. Account for reflection cost. |
| [Reflexion](https://arxiv.org/abs/2303.11366) | Uses textual feedback and persistent episodic reflection without updating model weights. | Tool outcomes can ground lessons, but same-task retries do not prove transfer to new tasks. |
| [ACE](https://arxiv.org/abs/2510.04618) | Incrementally generates, reflects on and curates an evolving context playbook. | Keep small independent records with provenance rather than repeatedly rewriting one growing prompt. |
| [GEPA](https://arxiv.org/abs/2507.19457) | Evaluates reflected prompt revisions and combines complementary candidates. | Use separate development and final evaluation sets. Global prompt optimization is a later layer, after reliable experience storage and measurement. |

These are distinct implementations and research settings. Their published
results are not S-Code results. The first S-Code implementation adapts context;
it does not train model weights or modify its own executable.

## Current candidate: verified source changes

Quality08 tested deterministic memory of source reads without reflection and
failed the quality screen. Its pre-verification reads often described unchanged
files: an edit invalidated the hash of an earlier read. The next candidate
selects actual edits followed by successful verification instead. This addresses
a collection limitation; a performance advantage remains unproven.

Learning remains opt-in, actor-owned and scoped to the canonical workspace.
The lifecycle is `off`, `learn` (save and reuse), or `reuse` (frozen experience,
no extraction). CLI and Local Web expose content, provenance and deletion.
There is no automatic global/team sharing and no model-weight or executable
modification.

An eligible uninterrupted task must complete with fully observed usage and a
successful, nonempty verification command. Existing verification files and
configuration must remain unchanged; supported new regression files are
allowed. Necessary source and documentation edits must precede final
verification. The existing verification fingerprint, cancellation and resumed
turn guards remain in force. Verification is a task-level eligibility signal;
it does not prove that tests cover every saved line.

Only completed built-in `apply_patch` results preceding the final verifier can
supply a record. The current full UTF-8 file must match the edit result's SHA-256
and byte length. The producer's full changed-range metadata must match current
line bounds, total lines, span bytes and excerpt. Truncated tool previews are
validated as prefixes, never mistaken for the complete changed span. Redacted
or unavailable previews, no-op edits and empty after-spans are ineligible.
The range can contain unchanged lines between edits; verification does not
establish line-level test coverage. Shell writes and read-only tasks do not
supply new records, and overwritten intermediate edits are not reconstructed.

Whole source and metadata are checked for secrets before clipping. Processing
stops at a 512 KiB aggregate limit (one runtime snapshot can be larger before
rejection). Bounded fragments retain actual line numbers and mark omitted
changed-span content. Command output, private reasoning and generated advice
are not saved. The task prompt informs selection transiently. Selection uses
lexical relevance and deterministic path/range tie-breaking without fixture or
language-specific rules.

Typed `source_observation.change` evidence distinguishes new records from old
read observations and generated summaries in the existing encrypted store.
Records carry the source turn, edit and verification tool IDs, current file hash,
optional previous hash and line fragments. A new file has change evidence with
no previous hash. Up to three files can be saved per task, each record bounded
to 3,200 serialized bytes and two fragments. A separate versioned identity
namespace prevents an old read record from blocking a new change record.
Different eligible turns may refresh changed file versions; identical path/hash
records remain duplicates. Same-version range expansion is outside this candidate.
Both legacy formats remain inspectable and removable but excluded from recall.
All formats share the 64-record capacity and 30-day expiry.

Before every coding request, recall selects source observations by the current
request's relevance to paths and source text, checks source completion and
current file hashes, and respects the existing four-record/1,200-estimated-token
budget. Changed, expired, irrelevant or revoked observations are omitted.
Recalled text is separate untrusted user context. It cannot change instructions,
grant permissions, enable network access or authorize commands.

Disabling, clearing or removing experience increments the project generation.
In-flight saves and outcome writes must match the generation captured at task
start. Source-turn idempotency and atomic version replacement prevent replays
from restoring deleted or superseded content. Deletion retains only content-free
audit provenance; normal source-session history follows its own controls.
Recall is not persisted into checkpoints.

Selection makes no model request. Missing coding usage remains unknown and
ineligible; no reflection cost is invented. Recalled context still consumes
coding input tokens. Extraction or outcome-storage failures cannot fail the
completed coding task. Last-outcome metadata distinguishes no usable source,
saved observations and extraction failure without storing provider error text.

[Anthropic's context-engineering guidance](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
describes the trade-off between compact prior context and on-demand exploration.
[Letta's context repositories](https://www.letta.com/blog/context-repositories/)
provide another example of persistent, inspectable context. These motivate
investigating source memory; neither establishes S-Code's performance. The next
comparison must retain all failures, use fresh profiles and charge all coding
and training costs before an untouched confirmation set can be opened.

## Measured evidence

The [quality08 development audit](../../tests/benchmarks/self-evolving/results/quality08/README.md)
retained all 111 training/transfer attempts and 1,255 physical requests. Off and
raw each passed 26/36 transfer attempts; learned passed 25/36. One request has
unknown usage, so the $6.00861112 known subtotal is not a complete cost total.
The original analyzer had API-shape errors, documented in a separate post-hoc
diagnosis without altering its outputs. Literal observation delivery occurred
389 times, but exact saved-tool to provider-call provenance remains unknown.
Neither that diagnosis nor this failed screen supports advancement to the sealed
holdout. The next harness must prove explicit ID linkage using a real daemon
and local fake provider before further paid evaluation.


The [quality07 development audit](../../tests/benchmarks/self-evolving/results/quality07/README.md)
at `fe5feb5` retained all 30 slots and 411 physical requests with complete
accounting. Learned passed 7/9 transfer attempts, off 8/9 and raw 9/9. H12
tokens per verified success were 214,882.64 for learned versus 209,245.84 for
off, **2.69% higher**. The quality and efficiency requirements failed, so the
candidate did not qualify for confirmation and the new holdout remains sealed.

All three projects saved three eligible lessons, and all nine initial learned
requests received exact verified guidance (120 requests overall). This passed
Quality07's new prospective per-family exposure gate. Delivery was reliable,
but aggregate benefit was not demonstrated. Queue had a favorable descriptive
comparison at equal 3/3 success; flow and report did not. Every family remains
included. Actual totals were 4,620,847 tokens and USD 2.34006056.

Two report failures missed the limit for overlong whitespace-only physical
lines. The flow learned seed 29 failure was a brittle oracle wording check:
the reason contained `fail fast` rather than the expected `fail-fast`. Earlier
behavioral assertions passed, but later assertions in that test were not
reached. This does not imply a counterfactual passing grade. All original
failures, costs and thresholds remain unchanged; nothing was regraded.

The [quality06 development audit](../../tests/benchmarks/self-evolving/results/quality06/README.md)
at `9fbdd76` retained all 30 slots and 531 physical requests with complete
usage and cost. Learned passed 8/9 transfer attempts versus off 7/9 and raw
7/9. Aggregate H12 tokens per verified success were 225,087.13 for learned
versus 365,086.68 for off, a 38.35% reduction. The frozen development screen
passed, but this did not demonstrate memory benefit.

Flow and report saved no lessons and received no distilled context; all six
paired initial off/learned requests in those families were byte-identical.
Queue saved one lesson and received 54 verified context injections. At equal
3/3 quality, its learned H12 tokens per verified success were 43.56% higher
than off. The separate conservative post-result decision was to defer
confirmation, preserving the original passing screen and all five failures.
The new holdout remains sealed.

These development rounds train once per family and freeze its artifacts,
then run every learned transfer attempt in reuse mode with a fresh profile.
H1/H4/H12/H24 amortize the initial training and reflection. They do not measure
ongoing per-task learn-mode reflection costs or cumulative learning over the
amortization horizon. Actual quality06 spending was 6,381,468 tokens and
USD 2.99770392, including training and every failed attempt.

The [quality05 development audit](../../tests/benchmarks/self-evolving/results/quality05/README.md)
at `5dd7e34` retained all 30 slots and 444 physical provider requests. Learned
passed 9/9 transfer attempts, off 8/9 and raw 6/9. H12 lifecycle tokens per
verified success were 148,382.8 for learned versus 236,625.4 for off, a 37.29%
reduction including training and reflection. Each project saved three lessons;
106 outbound requests across all nine learned attempts contained verified
distilled experience. All four transfer failures were functional failures in
the report task, with preserved grading files.

One raw-arm request failed without usage or cost. Its totals and complete
experiment spending remain unknown, so the frozen completeness screen failed
despite the favorable off/learned comparison. These are three development task
identities, not independent confirmation. The quality06 replication above
included reviewed transport diagnostics and user-control fixes while preserving
the learning engine and that screen's thresholds.

The complete [quality04 development round](../../tests/benchmarks/self-evolving/results/quality04/README.md)
at `981e4b3` retained all 30 training/development slots and all 484 provider
requests. Off passed 9/9 transfer attempts, learned 8/9, and raw 6/9. At H12,
learned used 8.55% more lifecycle tokens per verified success than off. Queue and
report saved no lessons because documentation changed after final verification;
flow saved one. These are three development task identities, not independent
evidence of general performance.

The subsequent [confirmatory audit](../../tests/benchmarks/self-evolving/results/confirmatory-quality04/README.md)
at the same frozen revision retained all 108 planned attempts and 1,190 provider
requests. Off passed 28/36, raw 29/36, and learned 26/36. One learned request
has unknown usage, so its complete token/cost totals and the primary conclusion
remain inconclusive. The observed quality guardrail also failed. Audit integrity
passed; that does not mean the feature met its performance criterion. Queue and
report still had empty lesson sets, and all 24 paired initial off/learned requests
in those families were byte-identical. No failed or unknown attempt was dropped,
replaced or selectively rerun.

The complete development round at `a11bc6c` did not establish an advantage:
off passed 9/9 attempts; learned and raw each passed 8/9. At the fixed 12-task
reuse horizon, learned consumed 5.45% more tokens per verified success after
including training and reflection. Both failures were real omissions in handling
whitespace-only physical lines, with complete usage and valid external grading.
The [independent audit and measurements](../../tests/benchmarks/self-evolving/results/editing03/independent-review.md)
retain every planned slot and previous unsuccessful or stopped experiments.
This development set has three distinct task identities, repeated across seeds;
it does not establish long-term learning or performance on other repositories.

Review of the development traces also found avoidable evidence loss. Credential
prefix checks now respect ordinary identifier boundaries, source excerpts fit
their actual serialized byte budget, and valid later proposals can survive
rejection of earlier proposals. The reflection-based candidates requested distinct, narrowly supported
observations and only eligible file dependencies. Those changes were included
in quality04 above. The exposed confirmatory set is now historical evaluation
data and cannot independently confirm a tuned revision. It may be reused only
as explicitly exposed development data.

The candidates evaluated in quality05/quality06 added final-verification
guidance, visible learning outcomes and verified replacement of stale dependency
versions. These changes came from development evidence and lifecycle review.
Quality05 had incomplete accounting; quality06 passed its aggregate screen but
did not demonstrate memory benefit. Quality07 preserved strict individual
proposal validation while retaining valid siblings, recorded terminal learning
outcomes and prospectively required exposure in every learned initial request.
It passed that exposure gate but failed the quality/efficiency screen. None of
these rounds established a confirmatory advantage.

Before a new holdout is revealed, run a complete development round with fresh
training. The screening requirements include at least 20% lower H12 lifecycle
tokens per verified success than off, no observed success loss, complete
accounting, and actual eligible source context in every positive-transfer family's
learned initial requests. These are adaptive development rules, not independent
proof. Preserve unsuccessful rounds and keep a new independently designed
holdout sealed until the candidate and its actual training artifacts are frozen
and reviewed.

## Evaluation contract

All controls use the same coding engine. Development exposed shared editing
and accounting problems: the public tool interface now uses exact source-text
anchors and bounded change previews, and preserves original line endings.
Retries consume agent request slots, malformed tool batches retain their usage,
and provider cumulative usage is normalized before aggregation. These are
baseline corrections applied to every condition, not gains attributed to
learning. Results from different engine revisions must not be pooled as a
controlled learning comparison.

The existing nine frozen coding tasks remain regression coverage. Their grader
duration and scripted evaluation provider are not measurements of real agent
latency or capability.

Training, development and final holdout task identities remain disjoint.
The next development comparison reuses all 12 previously exposed quality04
tasks, including its three negative-transfer controls. This expands task
coverage but is not a fresh confirmation; it must not be pooled with earlier
rounds or used to claim independence. The new holdout stays sealed.
Training teaches reusable repository conventions and workflows, not holdout
answers. Each task starts in a new session and clean workspace; learned records
are frozen before holdout tasks run. Hidden grading lives outside the agent
workspace and is applied to the resulting patch in a fresh supervisor. Include
unrelated tasks and changed-dependency tasks to measure negative transfer.

Compare three conditions with the same provider, exact model, tool budget and
task prompts: learning off; bounded raw experience retrieval; the current
source-observation candidate. Raw retrieval retains its original mechanism
and can include reads after verification and the prior task prompt as a
retrieval trigger; the production candidate is stricter. Report this difference
rather than attributing raw-arm results to the new implementation. Balance
condition order. Store every failure, request usage,
elapsed time, patch, grade, retrieval decision and learning artifact. Include
training/extraction overhead, amortized cost and break-even task count. A run
with missing provider usage cannot establish a token-cost advantage.

Before final holdout execution, freeze fixture hashes, implementation revision,
model settings, training sources and lessons, sample count, order and analysis.
The public pilot uses three Python project families. The independent final set
has 12 task identities (including three negative-transfer controls), each run
with three fixed seeds and all three conditions. Order is balanced within
paired task/seed blocks. Repetitions are not independent task identities.

The preregistered primary objective is quality-preserving token efficiency:
no observed loss in task-level success, at least 20% lower token consumption
per verified success including equal-lifecycle training overhead at a fixed
12-task horizon per project, and a paired task-cluster 95% bootstrap interval
excluding zero reduction. The 20% threshold applies to the point estimate, not
the confidence bound. Zero observed loss is a sample quality guardrail, not
proof of statistical non-inferiority. Bootstrap intervals are conditional on
these three fixed projects and their trained experience.

Report all planned runs, failures, incomplete accounting, raw-retrieval controls
and negative transfer separately. Any missing run, unknown token accounting or
budget rejection makes the confirmatory result inconclusive; do not discard
it or selectively rerun a favorable subset. Missing dollar cost remains
unknown even if token accounting is complete. Also report learning-overhead
sensitivity at 1, 4, 12 and 24 subsequent tasks, provider cache usage, and latency
separately from grading. An independent reviewer checks implementation,
protocol and the underlying results. After final task exposure, changes require
a new untouched holdout before another confirmatory claim.
