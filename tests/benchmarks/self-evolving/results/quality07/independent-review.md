# Quality07 independent review

**Audit integrity passed; the development readiness screen failed. The holdout remains sealed.**

All 30 planned attempts ended and were graded. All 411 physical provider requests have complete usage and costs, with no request errors or budget denials. The frozen auditor ran once and exited successfully; 2,834 additional independent checks passed.

| Arm | Verified success | Development tokens | Recorded cost | H12 tokens per verified success |
| --- | ---: | ---: | ---: | ---: |
| off | 8/9 | 1,614,621 | $0.78075840 | 209,245.84 |
| raw | 9/9 | 1,309,488 | $0.69000636 | 152,092.64 |
| learned | 7/9 | 1,439,992 | $0.72141752 | 214,882.64 |

At H12, learned uses **2.69% more tokens per verified success than off**, and 41.28% more than raw. The observed success guard also fails: learned 7/9 versus off 8/9. H1/H4/H24 comparisons remain unfavorable. These are descriptive development results, not significance estimates.

All three training attempts passed, saved three lessons each, and recorded a saved outcome. Every one of the nine learned initial coding requests received exact eligible guidance; 120 requests carried verified lessons in total (flow 53, queue 35, report 32). The new exposure-reliability gate passes. Guidance delivery was real, but aggregate useful learning is not demonstrated.

| Family | Off success | Learned success | Off H12 tokens/success | Learned H12 tokens/success |
| --- | ---: | ---: | ---: | ---: |
| flow | 3/3 | 2/3 | 266,185.75 | 412,951.88 |
| queue | 3/3 | 3/3 | 200,457.50 | 133,798.08 |
| report | 2/3 | 2/3 | 137,018.50 | 138,440.25 |

Queue has a favorable comparison with equal success. Flow and report do not; none is excluded or used alone to advance the candidate.

The three failures remain unchanged:

- **Flow, learned, seed 29:** the frozen oracle required the literal substring `fail-fast`; the candidate wrote `fail fast`. The prompt requests a concise fail-fast reason without prescribing that exact spelling. This is a brittle wording check, not proof that stopping behavior failed. Earlier retry/stopping/skipped-state checks passed, but later assertions in that test were not reached. No regrade occurred.
- **Report, off and learned, seed 29:** both skip whitespace-only lines before enforcing the byte limit, so an overlong physical line is accepted. Each failed one of seven checks.

All tasks reached daemon status completed, all grader outcomes are known, and existing public tests passed for these failed candidates. Their costs and failing grades remain in the totals.

Physical totals are **4,620,847 tokens and $2.34006056**. Initial common training uses 237,383 tokens and learned-only reflection uses 19,363, for 256,746 training tokens. Agent time totals 2,177.71 seconds; provider request time totals 1,959.45 seconds. These time measures are separate.

**Scope:** train once, freeze source/lessons/profile, then evaluate reuse. H1/4/12/24 allocate the initial training/reflection over hypothetical horizons; they do not measure ongoing reflection after each new learn-mode task. Three public task identities repeated across seeds are not 27 independent tasks. Exposure proves delivery, not semantic usefulness or causality.

The exact source, binary, helpers, approved freeze and all twelve historical files remain bound and unchanged. Source profiles were hashed, not decrypted; no model call, grader replay, candidate edit or private-reasoning/SSE inspection was performed during this audit. Profile admission and grading immutability retain the previously disclosed reliance on recorded producer checks.

The public package retains the frozen report and all previous separate negative, interrupted and incomplete rounds. Copy only the explicit manifest allowlist. Current results do not qualify the candidate for a confirmatory freeze; any future changes must be prospective and preserve this record.

Frozen report SHA256: `891417809c43298223f02b70b9f6656bfb53ab61298414917b393a9e82ef9b63`.
Independent review SHA256: `503dab004669b13992ef5484086618276162f50e7618cb60faf993798f363958`.
