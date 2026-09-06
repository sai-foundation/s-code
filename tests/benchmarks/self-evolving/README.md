# Project-learning benchmark

This benchmark runs the real S-Code daemon on separate training and transfer
tasks. It compares learning off, bounded retrieval of raw observations, and
automatically saved project source observations. Historical rounds below used
distilled lessons. The public development projects are a JSONL incident
reporter, a durable SQLite queue, and a workflow runner.

The [design](../../../docs/architecture/self-evolving.md) explains the feature.
The [analysis contract](ANALYSIS.md) defines the success criterion, training
overhead, missing-data treatment and limits of inference. Development results
are exploratory; they do not establish a confirmatory performance advantage.

## Observed results

The [quality07 development audit](results/quality07/README.md) at `fe5feb5`
retained all 30 slots and 411 requests with complete accounting. Learned passed
7/9 transfer attempts versus off 8/9 and raw 9/9, and used **2.69% more H12
tokens per verified success than off**. The development screen failed. All
three projects saved three lessons, and all nine learned initial requests
received verified guidance, passing the new prospective exposure-reliability
gate. Delivery alone did not establish benefit; the holdout remains sealed.

All three failing grades remain unchanged. Two report attempts missed the
limit for overlong whitespace-only lines. One flow failure was an oracle
wording limitation (`fail fast` versus `fail-fast`); later assertions were not
reached, and no counterfactual passing grade is claimed. The package preserves
all costs and twelve separate historical reports, with offline arithmetic
reproduction and the exact frozen auditor source.

The [quality06 development audit](results/quality06/README.md) at `9fbdd76`
retained all 30 slots and 531 requests with complete accounting. Learned
passed 8/9 transfer attempts versus off 7/9 and raw 7/9. Its aggregate H12
tokens per verified success were 38.35% lower than off, so the frozen
development screen passed. **Memory benefit was not demonstrated:** only
queue saved and received a lesson, and learned used 43.56% more H12 tokens
per verified success there at equal 3/3 quality. Flow and report received no
lessons; their paired initial off/learned requests were byte-identical.
Confirmation was separately deferred and the new holdout remains sealed.
The original passing screen and every failure are preserved.

The measured workflow trains once, freezes the resulting artifacts and runs
transfer attempts in reuse mode. H12 amortizes initial training/reflection;
it does not include ongoing reflection after every future learn-mode task
or establish cumulative learning over 12 tasks.

The [quality05 development audit](results/quality05/README.md) at `5dd7e34`
retained all 30 slots and 444 provider requests. Learned passed 9/9 transfer
attempts versus off 8/9 and raw 6/9. Including training and reflection at H12,
learned used 37.29% fewer tokens per verified success than off. All three
projects saved three lessons, with verified lesson payloads in 106 requests
across all nine learned attempts. This is promising development evidence, not
an independent confirmation: one raw request has unknown usage and cost, so
the frozen completeness screen failed and the new holdout remains sealed.
Raw totals and complete experiment spending remain unknown. Earlier rounds
below are retained separately; they are not pooled with this result.

The complete [quality04 development audit](results/quality04/README.md) at
`981e4b3` retained all 30 slots and 484 provider requests: off passed 9/9,
learned 8/9 and raw 6/9. Learned used 8.55% more H12 lifecycle tokens per
verified success than off. Flow saved one lesson; queue and report saved none.

The subsequent [108-attempt confirmatory audit](results/confirmatory-quality04/README.md)
at the same frozen revision is **inconclusive**. Off passed 28/36, raw 29/36,
and learned 26/36. All 1,190 provider requests were retained, including one
request with unknown usage; learned token/cost totals remain unknown. The
observed quality guardrail also failed. Audit integrity passed, but no learning
advantage was established. The exposed task set is retained as historical data;
a revised candidate needs a new untouched holdout.

The complete [editing03 audit](results/editing03/independent-review.md) at
revision `a11bc6c` did **not** show a learning advantage. With three transfer
tasks and three seeds, off succeeded 9/9 times; learned and raw each succeeded
8/9 times. At a 12-task reuse horizon, learned used 5.45% more lifecycle tokens
per verified success than off. Every planned attempt and all provider usage were
accounted for. These are three task identities, not 27 independent tasks.

[Measurements](results/editing03/editing03.json) retain all 30 training/development
slots, failures, configuration and artifact hashes, plus the earlier experiments
and adaptively stopped round. The report separates literal spending from
amortized training and keeps cost, tokens and latency distinct. Subsequent
engineering fixes cannot be counted as benefits in this earlier comparison.
The exact [audit source](results/editing03/audit.py), whose hash is recorded in the JSON,
is included for review; it operates on the locally retained full pilot artifacts.
Those private request/profile artifacts are not part of the public result package.

## Offline checks

Run from the repository root with Python 3. These commands make no model calls:

```sh
python3 tests/benchmarks/self-evolving/fixtures/check-fixtures.py
python3 -m unittest discover -s tests/benchmarks/self-evolving -p 'test_*.py' -v
```

They check the public grader against reference changes and intentionally broken
solutions, process and filesystem isolation, provider accounting, frozen
artifacts, and the statistical analysis. CI runs these in the existing benchmark
job. Linux grading requires Bubblewrap; macOS uses its native sandbox.

## Real-model development runs

Commit the implementation and benchmark helpers before building the daemon, and
supply an OpenRouter credential file outside the repository. Source snapshots
include files tracked by Git, including staged additions, and hash their current
working contents. Untracked personal files and ignored experiment outputs are
excluded. A tracked file that is missing, traverses a symlink, or exceeds the
5 MB per-file limit stops the snapshot instead of being silently omitted.
These runs incur provider charges. Each output directory must be new so previous
attempts remain available.

```sh
cargo build -p s-code-daemon
python3 tests/benchmarks/self-evolving/pilot.py \
  --key-file /absolute/path/to/openrouter-key \
  --provider z-ai/fp8 \
  --project flow \
  --seeds 17 29 43 \
  --max-cost 15 \
  --output .work/learning-flow-01
```

Repeat serially with `--project queue` and `--project report`, using separate
output directories. The default model is `z-ai/glm-5.3`; coding requests low
reasoning effort. The source-observation candidate makes no reflection call. The provider route is fixed without fallback.
Training runs once with seed 17. Each requested seed then runs all three arms in
sequence, rotating their order so that with three seeds each arm occupies each
position once. Every attempt has a fresh daemon and session; learned profiles
copy the closed training snapshot, and every arm starts with an empty tool cache.
Each daemon closes before its candidate is graded. No attempt is retried or
selected based on its result.

The dollar limit is divided equally across training and every development
attempt: `max-cost / (1 + 3 * number-of-seeds)`. The example has ten tasks with
a $1.50 cap each, including all requests made by that task. The global request
limit scales to 200 times this task count (2,000 here). Unreported usage stays
unknown even if a provider waives a charge.

Omitting `--seeds` preserves the single-seed default `[17]` and the existing
`output/off`, `output/raw`, and `output/learned` layout; for example, `--max-cost 6`
then gives four $1.50 task caps. Multiple seeds use `output/seed-17/off` and the
corresponding seed/arm paths. Each result records its seed, arm and order
position. Empty and duplicate seed lists are rejected.

Each pilot preserves the trained source, a closed training-state snapshot,
automatically generated lessons, raw observations, provider usage, task status,
patches and external grades. Learned experience is frozen before transfer; the
other arms receive the same trained source. The source, lesson/profile snapshot
and raw observations are checked before every development attempt. Failed and
incomplete-usage attempts remain in the aggregate results. This development
repetition option does not change the separate 108-attempt confirmatory protocol.
Grading runs outside the candidate
workspace and checks that existing tests and test configuration remain intact.

## Training without pilot transfers

Use `--training-only` instead of `--seeds` to run one seed-17 training task,
grade it and save the closed source/profile/experience artifacts. It makes no
development calls and does not report a comparable transfer result. Set an
explicit training budget, for example `--max-cost 1.5`. Failure and unknown
accounting remain recorded; downstream evaluation must reject incomplete or
unsuccessful training rather than selecting a replacement.

The evaluator also supports a freeze marked `phase: "exposed-development"`
for a prospectively planned comparison using previously exposed tasks. Its
metadata identifies that scope. Such a run can inform a development screen;
it cannot establish an independent confirmation or authorize opening a new
holdout. Historical data stays separate.

## Confirmatory evaluation

`evaluate.py` consumes an independently supplied task manifest, an external
grader, and a preregistration JSON. Before revealing task content, freeze the
committed implementation and binary hashes, analysis, model/provider, three training
artifacts, budgets, sample count and order rule. The exact required fields are
validated by `evaluate.py`; the statistical fields are documented in
[ANALYSIS.md](ANALYSIS.md).

```sh
python3 tests/benchmarks/self-evolving/evaluate.py \
  --freeze /absolute/path/to/freeze.json \
  --tasks /absolute/path/to/tasks.json \
  --grader /absolute/path/to/grade.py \
  --key-file /absolute/path/to/openrouter-key \
  --output .work/learning-confirmatory-01
python3 tests/benchmarks/self-evolving/analysis.py \
  .work/learning-confirmatory-01/measurements.json \
  --output .work/learning-confirmatory-01/analysis.json
```

Every attempt starts from a fresh daemon profile and restored source. The runner
checks frozen artifacts before each attempt and preserves all planned outcomes.
A revised implementation needs a new untouched final task set for a new
confirmatory claim. Reusing exposed tasks measures reproduction or development.

Output directories are private by default. Publish sanitized measurements and
reproduction metadata; do not publish credentials, local encryption keys,
profile stores, or private model reasoning from raw provider streams.
