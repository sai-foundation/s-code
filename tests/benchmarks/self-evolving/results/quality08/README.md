# Quality08 — failed development screen, with a post-hoc parser diagnosis

**This round did not demonstrate a benefit.** Learned achieved **25/36** verified
transfers, versus **26/36** for both off and raw. One off request has unknown
usage and cost. The quality and accounting requirements fail independently of
the audit-parser issues described below. There is no confirmation or permission
to reveal the new holdout.

This package preserves the **19 original frozen-audit files byte for byte**,
including 14 separate historical reports. The original audit has known false
negatives. Its zero verified-exposure count **must not be read as zero actual
memory delivery**. Read the labelled [post-hoc diagnosis](posthoc/parser-diagnosis.md)
and [final independent review](independent-review.md) alongside the original
[report](quality08.md), not as replacements for it.

## Recorded outcomes

| Measure | Result |
| --- | --- |
| Planned / retained attempts | 3 training + 108 transfer = 111 |
| Transfer task identities | 12 previously exposed tasks in 3 small Python projects; 3 seeds per arm |
| Off verified successes | 26/36 |
| Raw verified successes | 26/36 |
| Learned verified successes | 25/36 |
| Settled physical requests | 1,255: 31 training + 1,224 transfer |
| Requests with known usage | 1,254 |
| Known physical token subtotal | 11,477,438 |
| Known physical cost subtotal | $6.00861112 |
| Complete token / cost totals | **Unknown**, retained as null |
| Budget denials / selective reruns | 0 / 0 |
| Frozen development screen | **Failed** |

The subtotals include known physical requests within the incompletely accounted
attempt. They are not complete experiment totals or totals restricted to wholly
known attempts. The unknown record is off/Q3/seed17, request 549, with a transport
open timeout and no usable usage/cost or recovery identifier. It was not assigned
zero, dropped, replaced, or retrospectively reconciled.

All **31 unsuccessful transfers** remain: 15 protected-existing-test changes
(5 per arm), 9 F3 timeout/signal-contract failures, and 7 R4 FIFO-emission-before-
EOF failures. No grader, candidate patch, or acceptance rule was changed to make
these outcomes pass. The [independent review](independent-review.json) records
the classifications and source/ledger checks.

## What the parser diagnosis does and does not establish

The [separate diagnosis](posthoc/parser-diagnosis.json) was produced after the
run and is explicitly post hoc. It preserves the original criteria and output:

- All 111 actual session scopes match after accepting only the API's optional
  `goal_id: null` and `task_id: null` fields. The frozen synthetic fixture omitted
  these normal serialized fields.
- All 108 ordered attempt measurements and 12 training components match. The
  protocol discrepancy is only the original R/Q/F task-list metadata order
  versus the analyzer's synthesized F/Q/R order; execution order is unchanged.
- Internal lesson/transcript evidence uses daemon `tool_*` IDs, while outbound
  tool calls use provider `call_*` IDs. The retained public evidence has no
  explicit correlation and closed checkpoints are absent. **Exact selected
  read/verifier identity linkage remains unknown.** Matching order or display
  text is not accepted as proof.

Five saved observations (flow 2, queue 2, report 1) match frozen full-file hashes
and literal complete-line fragments. Matching public read content and internal
read-before-verifier chronology are separately observable, but do not bridge
that missing identity link. Faithful frozen literal payloads appeared in **389
requests** (120/136/133 by family), including **all 27 positive-task learned
initial requests**. All **36 initial triplets** match in full after removing only
exact frozen treatment envelopes. These are limited delivery/isolation findings,
not fully verified provenance, usefulness, or a causal performance gain.

The independent reviewer reproduced all 19 original output bytes and verified
the supplemental findings. The failed screen is unchanged. The original frozen
parser and its source are preserved for inspection rather than silently patched.

## Experimental scope

Each family trained once, then its source, observations and closed profile were
frozen. Transfer attempts used fresh profiles/sessions/cache; learned used reuse
mode and off/raw used fresh off-mode state. The candidate had no reflection
model call. Raw retained its existing algorithm and different evidence-selection
and payload boundary; this is not a representation-only ablation.

H1/4/12/24 amortize initial training followed by frozen reuse, **not continuous
learning after each new task**. The twelve tasks were already exposed to the
developers. Repeated seeds do not create new task identities. No result here is
independent held-out confirmation or general coding superiority. Numerical
bootstrap/point fields in [numerical-diagnostics.json](numerical-diagnostics.json)
remain descriptive; any complete-pair calculation under incomplete accounting
is not a complete lifecycle effect estimate.

## Offline reproduction and package contents

From the repository root, using Python 3.10 or later:

```sh
python3 -B tests/benchmarks/self-evolving/results/quality08/reproduce.py
python3 -B tests/benchmarks/self-evolving/results/quality08/test_parser_diagnosis.py
```

The first command verifies the package inventory/hashes and recomputes all public
aggregate fields and the original numerical diagnostics. It needs no model,
network, account key, daemon, encrypted profile, or private experimental output.
A narrow packaging adapter resolves the exact copied schedule implementation
locally instead of the private checkout path; the schedule, metrics and data are
unchanged. This checks arithmetic from published measurements, **not** independent
reconstruction of the private physical ledger or the missing tool-ID link.

The second command exercises the exact copied post-hoc functions with bounded
public examples: optional-null scope handling, task order, distinct ID namespaces,
literal line positions, and preservation of the failed screen/unknown totals.
The original seven-test private diagnostic source and passing log are retained
for inspection under `posthoc/`; those original commands require the historical
private layout and are not the public entry point.

- [public-files.json](public-files.json) is the unchanged original 18-file digest
  manifest; together with itself it forms the original 19-file output.
- [reproduction-files.json](reproduction-files.json) binds every other package file,
  including that original manifest, exact copied source, separately labelled
  post-hoc material, final review and public reproduction wrappers.
- `audit.py`, `quality08_common.py` and `analysis.py` are exact source copies.
  Their private full-run entry points are inspection material, not public replay
  commands. `posthoc/parser-supplement.py` is also an exact copy.

Request bodies, source fragments, provider IDs, private reasoning/SSE, profiles,
credentials, credential locations, private freeze/launcher files and raw private
logs are excluded. Opaque hashes in the reports identify evidence not included
here. Historical negative and interrupted rounds stay separate and are never
pooled into Quality08's arithmetic.
