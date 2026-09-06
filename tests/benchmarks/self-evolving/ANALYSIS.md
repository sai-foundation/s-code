# Frozen analysis contract

This module analyzes measurements, not task prompts or source solutions. It does
not read the sealed holdout document or make any model calls. The confirmatory
experiment has 12 distinct task identities, three fixed project families, three
arms, and three repetitions (seeds 17, 29, 43): 108 planned attempts. Each family
has four task identities, including one negative-transfer control. Final task
IDs and their group metadata are supplied only after the reveal procedure.

The proposed primary endpoint is **quality-guarded lifecycle total-token
efficiency, learned versus off**, at a fixed horizon of 12 future tasks per
family. The zero observed success-regression condition is a sample guardrail,
not proof of population noninferiority. Learned versus raw is secondary and
cannot replace the primary comparison after results are known.

## Decision rule and accounting

An attempt succeeds only if the external grader verifies all required behavior
and no budget denial occurred. The report also preserves raw grader-pass counts
separately. All 36 planned slots per arm remain in the success-rate denominator;
missing runs and ungraded attempts are unverified, not silently removed.

For an arm, family and future-task horizon H:

    allocated training tokens per attempted task
        = (common family training tokens + arm-specific preparation tokens) / H
    lifecycle tokens per verified success
        = sum(run tokens + allocated training tokens for ALL attempted slots)
          / number of verified successes
    improvement
        = 1 - learned lifecycle tokens per success / off lifecycle tokens per success

Common training includes the same full coding trajectory, failures and retries
for every arm because all arms receive its post-training source. Arm-specific
preparation is additional work: for example, learned reflection/extraction and
raw retrieval preparation. Do not put reflection in both common and learned
components. No-memory and raw components with zero model tokens must explicitly
record zero, not omit their records. A failed or irrelevant extraction still
incurs its full preparation tokens. Ordinary retrieved prompt tokens are already
part of run input tokens and must not be added twice.

H is a deployment amortization assumption, not the number of distinct evaluated
features and not the number of model repetitions. The report presents H=1,4,12,24
without selecting the most favorable one; only H=12 controls the primary rule.
These lifecycle-equivalent totals are distinct from literal experiment spending,
where a shared training trajectory was physically executed once.

The primary decision is `met_on_fixed_benchmark` only when all conditions hold:

1. Every planned attempt and all 12 family/training-component records have
   complete token accounting, known grading outcomes, the planned arm order, and
   no budget denials. No missing records, selective reruns or duplicate keys.
2. Learned verified successes are at least off verified successes across all
   planned slots (zero observed aggregate regression).
3. The point estimate of token reduction is at least 20%.
4. The lower endpoint of the two-sided 95% paired task-cluster bootstrap interval
   is strictly greater than zero improvement.

Conditions 3 and 4 **do not mean the interval proves at least 20% improvement**.
A complete experiment failing the guardrail or effect thresholds is `not_met`.
Missing runs/grades, unknown token usage, any budget denial, or an invalid order
make the primary `inconclusive`. Undefined ratio/interval estimates also prevent
passing. Cost or latency missingness alone does not invalidate a fully observed
token endpoint; those quantities are reported as unknown separately.

Incomplete token blocks are excluded as a whole across all three arms only from
the explicitly labeled complete-pair token description. Their outcomes stay in
the all-planned quality report, and the primary remains inconclusive. A budget
failure with known usage remains in token totals and success denominators.
No automatic replacement runs are selected. Any revised experiment needs a new
preregistered schedule; the old experiment and all attempted runs remain visible.

## Bootstrap and limits of inference

Use 10,000 draws with seed 20260905. Within each fixed family, sample its task
identities with replacement, keeping **all repetitions and all arms of each
selected task together**. Recompute aggregate tokens per verified success and
the relative reduction on every draw. Use interpolated 2.5/97.5 percentiles.
Do not bootstrap the 36 repeated blocks as 36 independent task identities.

If any resampled ratio is undefined because an arm has zero successes, report
the number of undefined draws and no token interval. Never discard those draws
and silently calculate a narrower conditional interval. A family with no
complete task cluster likewise has no interval. Success-difference intervals,
paired wins/losses, per-family/control results, and leave-one-family-out effects
are descriptive diagnostics; they do not constitute a second route to primary
success.

The estimate conditions on these three project seeds and their frozen training
artifacts. There are only four task identities per family. The bootstrap does
not capture uncertainty from selecting other repositories or rerunning training,
and nominal 95% intervals are approximate in a small cluster sample. Identical
observed outcomes can produce a degenerate interval without proving zero future
regression. Do not claim general coding superiority or statistically established
population noninferiority from this experiment.

This limitation follows the usual requirement to define an inferiority margin
and interpret uncertainty, rather than equating equal observed rates with a
population claim; see the methodological discussion in the
[FDA noninferiority guidance](https://www.fda.gov/regulatory-information/search-fda-guidance-documents/non-inferiority-clinical-trials).
For cautions about bootstrap inference with few clusters, see
[Cameron and Miller, A Practitioner's Guide to Cluster-Robust Inference](https://cameron.econ.ucdavis.edu/research/Cameron_Miller_JHR_2015.pdf).
These references inform the statistical cautions, not a claim that a clinical or
regression-model test applies unchanged to this coding benchmark.

## Input JSON

The top-level object contains `schema_version: 1`, `protocol`, `training`, and
`attempts`. `protocol` must have the following fixed values:

```json
{
  "task_manifest": [
    {"task_id": "opaque-task-id", "family": "report", "negative_control": false}
  ],
  "seeds": [17, 29, 43],
  "arms": ["off", "raw", "learned"],
  "primary_horizon": 12,
  "bootstrap_samples": 10000,
  "bootstrap_seed": 20260905,
  "target_reduction": 0.20
}
```

The single manifest item illustrates the shape only: actual input requires the
frozen 12 IDs, four each in `report`, `queue`, `flow`, one control per family.
No task text or expected answers belong in analysis input.

One training record is required per `(family, component)`; component is `common`,
`off`, `raw` or `learned`. Each records exclusive preparation work:

```json
{
  "family": "report",
  "component": "common",
  "usage_complete": true,
  "input_tokens": 123,
  "output_tokens": 45,
  "cost_usd": 0.01,
  "elapsed_seconds": 2.0,
  "budget_denied": false
}
```

One attempt is permitted per `(task_id, seed, arm)`:

```json
{
  "task_id": "opaque-task-id",
  "seed": 17,
  "arm": "off",
  "order_position": 0,
  "verified_success": true,
  "usage_complete": true,
  "input_tokens": 1000,
  "output_tokens": 100,
  "cost_usd": 0.03,
  "elapsed_seconds": 15.0,
  "budget_denied": false
}
```

`usage_complete` means every provider call belonging to that record, including
failures/retries, has accounted input and output tokens. Use false for incomplete
accounting and null for unknown counts. Never substitute a reservation estimate
or zero for missing provider usage. `verified_success` is null if grading did
not finish. Missing runs are absent records. Cost and elapsed time may be null
independently of token completeness. Elapsed time covers the agent attempt,
not external grading. Preparation elapsed times must be exclusive components,
not overlapping totals. Booleans, negative counts and nonfinite numbers are
rejected in numeric fields. Duplicate attempts are rejected rather than selected.

Optional `cached_input_tokens` and `reasoning_output_tokens` are displayed as
reported details; they are not added to input+output totals. Providers commonly
include these as subsets, and the runner must normalize its specific provider's
accounting consistently. Freeze the model/provider/reasoning configuration
outside this file and archive raw usage in separate credential-free run evidence.

`--schedule` emits the exact arm order from the opaque manifest: sort tasks by
`(family, task_id)`, rotate the three arms across the three fixed seeds, and
alternate forward/reverse cycles across adjacent tasks. Every task sees each arm
in every position, and the full 12-task experiment has each of the six orders
exactly six times. Run blocks serially and preserve the measured `order_position`.
This also balances immediate order direction across the fixed task set.

## Commands and evidence

```sh
python3 tests/benchmarks/self-evolving/analysis.py measurements.json --schedule
python3 tests/benchmarks/self-evolving/analysis.py measurements.json --output analysis.json
python3 -m unittest discover -s tests/benchmarks/self-evolving -p test_analysis.py -v
```

For schedule generation, empty measurement arrays are allowed; the complete
frozen task manifest and fixed protocol fields are still required. Do not
materialize final task metadata before the approved reveal step. The JSON result
includes the canonical input hash, analysis source hash, all quality denominators,
known versus unknown accounting, horizon sensitivity and the precise decision.
Always retain input, output, raw run evidence and preregistration together.

The synthetic tests exercise arithmetic, expensive failures, censoring, missing
training/usage, budget failures, zero-success bootstrap draws, balanced order,
duplicate runs, token double-counting and deterministic resampling. Synthetic
results test the analysis machinery; they are not evidence of model advantage.
