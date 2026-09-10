---
site: true
slug: efficiency-comparison
title: Historical efficiency comparison
short_title: Efficiency comparison
group: Verify
order: 93
description: Historical same-model results for S-Code and Other Agent A, with every observation and explicit measurement scope.
keywords:
  - efficiency
  - benchmark
  - tokens
  - latency
---

# Historical efficiency comparison

On **31 August 2026**, a S-Code development build and **Other Agent A** each
completed three Durable Task Queue runs using `z-ai/glm-5.3`. Both passed all
three frozen-grader checks. The medians were **34.7% fewer reported tokens**
and **6.6% less elapsed time** for S-Code.

Other Agent A is the same comparison label used in the
[safety comparison](safety-comparison.md).

## Selected repeated cohort

| Metric | S-Code development build | Other Agent A |
| --- | ---: | ---: |
| Frozen grader passed | 3/3 | 3/3 |
| Median reported total tokens | 121,904 | 186,791 |
| Median elapsed seconds | 46.741 | 50.020 |
| Median model calls | 15 | 15 |
| Median tool calls | 16 | 15 |
| Slowest elapsed seconds | 102.896 | 55.949 |

The source snapshot records these observation arrays in order. Medians are
computed independently per metric; do not assume a single run attained every
median at once.

| Agent | Elapsed seconds | Reported total tokens |
| --- | --- | --- |
| S-Code | 39.522, 46.741, 102.896 | 101,578, 121,904, 254,040 |
| Other Agent A | 50.020, 48.219, 55.949 | 179,394, 186,791, 211,140 |

Reduction is `(1 - S-Code median / Other Agent A median) × 100`, rounded to
one decimal place. Both chart metrics use zero-based bars with the same scale
for both agents within each metric.

## Conditions and provenance

The archived experiment report records the same model, prompt, independent
API server and frozen task grader for both agents, with `low` reasoning.
The host was an Apple M4 Pro with 48 GiB RAM, macOS 26.6.1, arm64. The task
implements a durable Python/SQLite queue with concurrency, leases, retries
and idempotent enqueue behavior.

The [anonymized metric snapshot](data/efficiency-2026-08-31.json) preserves all
observations from the historical follow-up, including the second task's
incomplete comparator run. It also records both queue binary hashes and a
SHA-256 digest of the original summary. S-Code's queue binary was measured
before a later storage fix; these numbers must not be attributed to the
current release or the later binary. The full raw event streams and exact
historical source state are not bundled with this summary.

## Accounting and scope

The two harnesses report token subfields differently, including cache and
reasoning accounting. This comparison retains each recorded provider total
without reconstructing it. **Reported-token reduction is not a normalized
token, cost or billing reduction.** No billed-cost conclusion is claimed.

These are three repetitions on one visible task selected from a tuning
follow-up, not an unseen evaluation set or an overall speed ranking. S-Code
had the slower worst run. Earlier algorithm and frontend cohorts also took
longer despite reporting fewer tokens; this snapshot does not overturn those
results or establish a general advantage across models.

## Other task retained in the snapshot

| Dependency Flow Runner cohort | Grader result | Elapsed seconds | Reported total tokens |
| --- | --- | ---: | ---: |
| S-Code development build, 3 runs | 3/3 | 58.820 median | 128,873 median |
| Other Agent A, earlier completed run | Passed | 59.376 | 182,794 |
| Other Agent A, follow-up completed run | Passed | 105.875 | 351,329 |
| Other Agent A, interrupted follow-up | 3/4 tests | Stopped at approximately 390 | At least 635,810 |

The completed comparator samples are single runs, not three-run medians.
The interrupted sample has no completed latency and stays out of completed
latency statistics. This unequal cohort is retained for context and is not
used for the README headline.

## Recalculate or run a fresh comparison

Recalculate the displayed medians from the checked-in snapshot:

```sh
python3 - <<'PY'
import json
from pathlib import Path
from statistics import median

data = json.loads(Path('docs/testing/data/efficiency-2026-08-31.json').read_text())
task = data['tasks']['durable-task-queue']
for metric in ('total_tokens', 'elapsed_seconds'):
    s_code = median(task['s_code'][metric])
    other = median(task['other_agent_a'][metric])
    print(f'{metric}: {s_code} vs {other}; reduction {(1 - s_code / other) * 100:.1f}%')
PY
```

For a new measurement, follow the [frozen-task benchmark method](README.md#coding-harness-benchmarks).
Record the actual executing binary, source state, model route, usage semantics,
raw events and grader results. A fresh run measures that build; it does not
reproduce these historical numbers merely by using the same task name.
