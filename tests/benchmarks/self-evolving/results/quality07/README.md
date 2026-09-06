# Quality07: guidance delivered, readiness screen failed

At revision `fe5feb52818b11060390200eea730d76ad2de752`, all 30 planned
training/development attempts ended. All 411 physical provider requests have
complete usage and cost, valid grading and no budget denial. The independent
audit passed; the development readiness screen failed. The new holdout remains
sealed.

| Condition | Verified successes | H12 tokens per verified success |
| --- | ---: | ---: |
| Learning off | 8/9 | 209,245.84 |
| Raw experience | 9/9 | 152,092.64 |
| Distilled lessons | 7/9 | 214,882.64 |

Learned used **2.69% more tokens per verified success than off** and 41.28%
more than raw at H12. The observed success guard also failed. These are three
public development task identities repeated across three seeds and conditions,
not independent confirmation or a significance result.

## Guidance delivery and family results

Each training attempt saved three eligible lessons. All nine learned initial
coding requests contained exact verified guidance, with 120 exposed requests
in total: flow 53, queue 35 and report 32. Quality07 prospectively required
eligible saved guidance in every positive-transfer family and delivery in all
nine initial learned requests. That new reliability gate passed. The older
global exposure flag is retained separately; neither changes Quality06's
historical screen.

| Family | Off success | Learned success | Off H12 tokens/success | Learned H12 tokens/success |
| --- | ---: | ---: | ---: | ---: |
| Flow | 3/3 | 2/3 | 266,185.75 | 412,951.88 |
| Queue | 3/3 | 3/3 | 200,457.50 | 133,798.08 |
| Report | 2/3 | 2/3 | 137,018.50 | 138,440.25 |

Queue has a favorable descriptive comparison with equal success, while flow
and report do not. Every family remains included. Reliable delivery is evidence
that guidance reached the model, not proof of useful or causal learning.

## Preserved failures and an oracle limitation

All three failures retain their original grades and costs:

- **Flow, learned, seed 29:** the frozen oracle required the literal substring
  `fail-fast`; the candidate wrote `fail fast`. The task requests a concise
  fail-fast reason without prescribing an exact quoted spelling. This is a
  brittle wording check. Earlier retry/stopping/skipped-state assertions passed,
  but later assertions in that test were not reached. This does not establish
  a counterfactual passing grade or complete functional correctness.
- **Report, learned and off, seed 29:** both candidates skip whitespace-only
  lines before applying the byte limit, so an overlong physical line is accepted.
  Each failed one of seven checks.

All three tasks reached completed daemon status, and their existing public
tests passed. Nothing was regraded, replaced or selectively rerun. The twelve
companion historical files retain earlier negative, incomplete and stopped
rounds separately; none is pooled into this comparison.

## Accounting scope

Training ran once per family. Its source, lessons, profile and raw observations
were then frozen, and each learned development attempt used reuse mode with a
fresh profile. H1/H4/H12/H24 amortize the initial training/reflection; they do
not include ongoing learn-mode reflection after each later task or establish
cumulative learning over those horizons. Common training is allocated equally
to all conditions, with reflection charged only to learned.

Actual experiment totals, including training and every failure, were
**4,620,847 tokens and USD 2.34006056**. Training contributed 237,383 common
tokens and 19,363 reflection tokens. Agent time was 2,177.71 seconds; provider
request time was 1,959.45 seconds. These time measures are separate, and
lifecycle equivalents are not literal experiment spending.

## Evidence and offline reproduction

- [Measurements and audit](quality07.json), [readable report](quality07.md).
- [Independent interpretation](independent-review.md) and [bound checks](independent-review.json).
- [Exact public-file hashes](publishable-files.json).
- [Exact audit source](audit.py), [binding helper](quality07_common.py), and
  [reproduction-source hashes](reproduction-files.json).

From the repository root:

```sh
python3 -B tests/benchmarks/self-evolving/results/quality07/reproduce.py
```

This verifies the declared file hashes and recomputes aggregate counts,
accounting, lifecycle comparisons and the failed readiness screen from the
published measurements. It does not recreate private request, profile or
grading provenance, or rerun candidates. The full provenance auditor requires
the original frozen checkout and private artifacts; its unchanged source is
included for inspection. Credentials, request payloads, response streams,
generation identifiers and local profiles are excluded. Literal privacy-filter
patterns in source are not secrets.

This result does not qualify the candidate for a confirmatory freeze. Any
future changes must be prospective, preserve this exact record and pass a
separate review before a new holdout can be revealed.
