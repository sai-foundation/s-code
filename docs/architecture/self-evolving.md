# Self-evolving project experience

Status: implemented behind an opt-in setting; real-model evaluation in progress.
No confirmatory performance improvement has been established yet.

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
reflection. Setup retries consume separate agent request slots. Automatic
model-router fallback can contact multiple endpoints within one such slot;
its prior endpoint usage remains unknown. The independent benchmark meter
records every provider request, including requests that finish after the daemon
has disconnected, and remains the authority for experimental cost accounting.

Disabling/clearing/removing lessons increments a project generation. In-flight
extraction must match that generation before committing, so forgetting cannot
be undone by a late response. Source-turn idempotency prevents duplicate
learning after a resumed or repeated completion. Removal retains content-free
audit provenance, not the deleted lesson text.

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
