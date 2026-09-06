# Quality05: promising development evidence, incomplete accounting

At revision `5dd7e34722600eed2ccc8a2ba106626075309905`, all 30 planned
training/development attempts ended. The independent audit retained all 444
physical provider requests and found no source, order, grading or budget
integrity issues. This is three development task identities, each repeated
across three seeds and three conditions; it is not an independent confirmation.

| Condition | Verified successes | H12 tokens per verified success |
| --- | ---: | ---: |
| Learning off | 8/9 | 236,625.4 |
| Raw experience | 6/9 | Unknown |
| Distilled lessons | 9/9 | 148,382.8 |

The **37.29% reduction** for distilled lessons includes equal common-training
allocation and the additional reflection cost, amortized over a fixed 12-task
reuse horizon. It is a descriptive result from this development set, not a
guarantee for other tasks. Each project saved three lessons. The audit verified
actual distilled payloads in 106 requests across all nine learned attempts.

Flow's raw/seed-17 request 32 failed without usage or cost. That task passed its
functional tests, but its token/cost totals remain unknown, as do complete
experiment totals. The known physical subtotal is 5,250,958 tokens and
USD 2.60175056; this is **not** complete spending. No missing value was replaced
with zero and no attempt was dropped or replaced. All four development task
failures were the report task's empty-physical-line byte-limit check.

**The frozen development screen failed.** Favorable off/learned numbers do not
override its complete-accounting requirement. The next holdout remains sealed.
Earlier negative and inconclusive rounds are preserved separately in the
companion historical files; they are not pooled into this result.

## Evidence and reproduction

- [Measurements and audit](quality05.json), [readable report](quality05.md).
- [Independent review](independent-review.md) and its [checks](independent-review.json).
- [Exact public-file hashes](publishable-files.json).
- [Exact audit source](audit.py), [binding helper](quality05_common.py), and
  [reproduction-source hashes](reproduction-files.json).

From the repository root, recompute the published aggregate arithmetic offline:

```sh
python3 -B tests/benchmarks/self-evolving/results/quality05/reproduce.py
```

This verifies file hashes and reproduces aggregate counts, accounting,
lifecycle comparisons and the screen from the public measurements. It does
not independently recreate private request, profile or grading evidence.
Running the full provenance auditor requires the original frozen checkout and
its retained private artifacts. The audit/binding source is included unchanged
for inspection; its private staging paths are not a public runnable setup.
No credentials, request payloads, response streams or local profiles are
included. The two literal privacy-filter lists in the source are not secrets.
