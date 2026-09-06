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

## Decision

Add opt-in, actor-owned project learning alongside explicit user memory.
Completed tasks with observed successful verification commands may produce up
to three short lessons through one bounded model reflection. A lesson records
its applicability, procedure, source session/turn, supporting tool calls and
the hashes of files it depends on. Only observed file paths can be dependencies.
Model self-reported success is insufficient for eligibility.

The lifecycle is `off`, `learn` (extract and reuse), or `reuse` (frozen lessons,
no extraction). CLI and Local Web expose the setting, lesson content, sources,
individual removal and clearing. Settings are per organization, team, actor
and canonical workspace, defaulting to off. No automatic global/team sharing.
A content-free last learning outcome distinguishes a skipped reflection from
an empty, failed or successful reflection. Its reason, timestamp, source turn
and saved count are encrypted with the same project scope as the settings;
no prompt, tool output or provider error text is stored in this status.

Lessons are encrypted using the existing local store, capped at 64 per project,
expire after 30 days, and are omitted if a dependency hash changes. Selection
uses bounded lexical relevance and at most four lessons/1,200 estimated tokens.
Explicit user instructions retain precedence. Retrieved lessons are separate
untrusted user-context data, never system instructions or executable skills.
They cannot grant permissions, change tools, enable network access or edit
repository instruction files.

Reflection prioritizes observed project interfaces, required call ordering and
verification setup. Structured evidence retains tool IDs, file versions and
exit codes; long values keep explicitly marked beginning/end excerpts. Recent
dependency reads take precedence over repetitive repair logs. Generic editing
words do not trigger recall, and ordinary tool schemas or one-off platform
warnings should not consume lesson slots.

Each extraction has a 30-second deadline, a bounded transcript and at most 1,024
output tokens. It uses the configured task provider/model without tool access
and without automatic retries. Only public task input and tool evidence are
eligible; private reasoning summaries are excluded. Secret-like content is
rejected before extraction and before persistence. Reflection failure cannot
turn a successful coding task into a failure. Learning usage is visible and
included in task totals when supplied by the provider; missing usage remains
explicitly unknown in learning events and benchmark accounting.
An eligible task must also have complete observed usage before spending on
reflection; the reflection itself must report complete usage before any lesson
can be saved. The common coding prompt calls for final applicable verification
after all necessary source and documentation changes, across all learning modes.
It does not exempt documentation or run additional unmetered verification. Setup retries consume separate agent request slots. Automatic
model-router fallback can contact multiple endpoints within one such slot;
its prior endpoint usage remains unknown. The independent benchmark meter
records every provider request, including requests that finish after the daemon
has disconnected, and remains the authority for experimental cost accounting.

Disabling/clearing/removing lessons increments a project generation. In-flight
extraction and outcome writes must match the generation captured at task start,
so forgetting cannot be undone by a late response. These controls also clear
the last outcome. Source-turn idempotency prevents duplicate
learning after a resumed or repeated completion. Removal retains content-free
audit provenance, not the deleted lesson text.
Same-text deduplication does not prevent a different verified source turn from
refreshing changed dependency versions. Replacement is atomic under the captured
generation: it retires the previous record to a content-free tombstone and saves
a new lesson, while rejecting old IDs, same-source replays and expired proposals.
Unchanged dependency sets remain duplicates. Recall does not delete records.

## Measured evidence

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
identities, not independent confirmation. The next complete development
replication includes the reviewed transport diagnostics and user-control fixes;
the learning engine and the screen's thresholds remain unchanged. Keep the new
holdout sealed until the existing prospective conditions are satisfied.

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
rejection of earlier proposals. Reflection asks for distinct, narrowly supported
observations and only eligible file dependencies. Those changes were included
in quality04 above. The exposed confirmatory set is now historical evaluation
data and cannot validate a tuned revision again.

The evaluated candidate adds final-verification guidance, visible learning
outcomes and verified replacement of stale dependency versions. These changes
come from development evidence and lifecycle review; quality05 shows a
descriptive gain but has not established a confirmatory advantage. Before a new holdout is revealed, run a complete development
round with fresh training. Require at least 20% lower H12 lifecycle tokens per
verified success than off, no observed success loss, complete accounting, and evidence that verified
lessons actually reached a learned coding request. This is an adaptive development
screen, not independent proof. Preserve unsuccessful rounds and keep a new
independently designed holdout sealed until the candidate and its actual training
artifacts are frozen and reviewed.

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

The new experiment has disjoint training, development and final holdout tasks.
Training teaches reusable repository conventions and workflows, not holdout
answers. Each task starts in a new session and clean workspace; learned records
are frozen before holdout tasks run. Hidden grading lives outside the agent
workspace and is applied to the resulting patch in a fresh supervisor. Include
unrelated tasks and changed-dependency tasks to measure negative transfer.

Compare three conditions with the same provider, exact model, tool budget and
task prompts: learning off; equally bounded raw experience retrieval; distilled
project lessons. Randomize condition order. Store every failure, request usage,
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
