# Quality06: screen passed, memory benefit not demonstrated

At revision `9fbdd76ce9bde1c3c21b701012466069a066626a`, all 30 planned
training/development attempts ended. The audit retained all 531 physical
provider requests with complete usage and cost, valid grading and no budget
denial. These are three public development task identities, each repeated
across three seeds and three conditions, not independent confirmation.

| Condition | Verified successes | H12 tokens per verified success |
| --- | ---: | ---: |
| Learning off | 7/9 | 365,086.68 |
| Raw experience | 7/9 | 272,197.96 |
| Distilled lessons | 8/9 | 225,087.13 |

The aggregate **38.35% reduction** for learned versus off satisfies the frozen
development screen. However, it does **not demonstrate a memory-driven
improvement**. Training saved zero lessons for flow, one for queue and zero for
report. Only queue received verified distilled context: 54 requests across its
three learned attempts. All six initial off/learned request pairs in the two
empty families were byte-identical, and neither family received later lessons.

Queue passed 3/3 in every condition. Its learned H12 tokens per verified
success were 250,423.50 versus off 174,440.42—**43.56% higher**. Favorable
differences in the untreated families cannot establish memory benefit. The
separate conservative decision is therefore **not to advance this candidate
to confirmation**. The numerical screen remains marked as passed; this
post-result decision does not rewrite it. The new holdout remains sealed.

## What the accounting covers

Training ran once per family. Its source, lessons, profile and raw observations
were then frozen, and each learned development attempt used reuse mode with a
fresh profile. H12 amortizes the initial training/reflection over a fixed
12-task reuse horizon. It does not measure ongoing learn-mode reflection after
every new task or cumulative learning across 12 tasks. Common training is
allocated equally to all conditions, with reflection charged only to learned.

Actual experiment totals, including training and all failures, were
**6,381,468 tokens and USD 2.99770392**. Training contributed 417,935 common
tokens and 20,393 reflection tokens. Lifecycle equivalents are amortization
scenarios, not literal experiment spending.

All five failed attempts remain. Four report patches missed the byte limit
for oversized empty physical lines. Report off seed43 ended failed after
48 calls and had four failing candidate-added tests, while its original tests
and fixed external feature checks passed. Nothing was regraded, replaced or
selectively rerun. Earlier negative, incomplete and stopped rounds remain
separate in the ten companion historical reports.

## Evidence and reproduction

- [Measurements and audit](quality06.json), [readable report](quality06.md).
- [Independent interpretation](independent-review.md) and [bound checks](independent-review.json).
- [Exact public-file hashes](publishable-files.json).
- [Exact audit source](audit.py), [binding helper](quality06_common.py), and
  [reproduction-source hashes](reproduction-files.json).

The unchanged generated `quality06.md` retains an inherited “Quality05”
heading. Its measurements are from quality06, as identified by the JSON round,
revision and hashes; the original artifact is preserved rather than rewritten.

From the repository root, recompute the published aggregate arithmetic offline:

```sh
python3 -B tests/benchmarks/self-evolving/results/quality06/reproduce.py
```

This verifies declared file hashes and reproduces aggregate counts,
accounting, lifecycle comparisons and the original passing screen. It does
not recreate private request, profile or grading provenance, or turn the
result into a causal claim. The full provenance auditor needs the original
frozen checkout and retained private artifacts; its unchanged source is
included for inspection. No credentials, request payloads, response streams
or local profiles are published. Literal privacy-filter patterns in the
source are not secrets.
