# Quality04 confirmatory result

**Inconclusive; no learning advantage established.** The independent integrity
audit passed, while the performance criterion was not met. All 108 planned
attempts and 1,190 physical provider requests were retained at frozen revision
`981e4b38d976d1d3bb54ceaa82a3db294d270297`.

| Condition | Verified completions | Complete run tokens |
| --- | ---: | ---: |
| Learning off | 28/36 | 3,407,027 |
| Raw experience | 29/36 | 3,713,357 |
| Distilled experience | 26/36 | Unknown |

The table reports coding-run tokens; the analysis separately includes common
training and reflection overhead. One request in Q1/17/learned has unknown
usage after a transport error. Complete learned and physical totals therefore
remain unknown. No missing usage was replaced with zero, and no failed or
unknown attempt was dropped or selectively rerun. The observed quality
guardrail also failed. Any complete-pair point estimate or interval retained in
the unchanged analysis is descriptive and cannot establish a complete primary
effect under this missing-data rule.

The [audit](audit.md), [measurements](measurements.json) and
[analysis](analysis.json) preserve the original result. The
[independent output review](final-output-review.md) passed all 31 checks.
[publishable-files.json](publishable-files.json) binds the original five output
files; [reproduction-files.json](reproduction-files.json) binds the separate
source/review supplement. Neither manifest replaces the other.

Queue and report retained zero lessons; all 24 paired initial off/learned
requests in those families were byte-identical. Only flow received distilled
experience. Empty-treatment differences cannot establish memory causality.
There are 12 task identities from three fixed projects, repeated over three
seeds and three conditions. Earlier development rounds remain
[separate](../quality04/README.md). This task set is now exposed: a tuned
candidate needs a new untouched holdout for another confirmatory claim.

## Reproduction scope

The exact frozen [analysis source](analysis.py) runs on the public measurements
without model calls. From the repository root, write to a new output path:

```sh
python3 tests/benchmarks/self-evolving/results/confirmatory-quality04/analysis.py \
  tests/benchmarks/self-evolving/results/confirmatory-quality04/measurements.json \
  --output .work/quality04-reproduced-analysis.json
```

The released [task manifest](tasks.json), [grader](grader.py), and exact
[audit source](audit.py) are included for inspection. To use the unchanged
grader, stage it at `.work/quality04-confirmatory-release/grader.py` in a
checkout of the frozen revision; its support files are the tracked
`tests/benchmarks/self-evolving/fixtures/grade.py` and `bounded_process.py`.
Keep grading outside the agent workspace and use the benchmark sandbox.

The full provenance audit depends on privately retained request records,
closed profiles, operator manifests and their frozen paths. Public hashes alone
cannot reproduce that audit end to end. Credentials, profile contents, raw
provider streams and private reasoning are excluded from this package.
