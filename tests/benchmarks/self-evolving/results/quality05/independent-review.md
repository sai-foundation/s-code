# Quality05 final independent review

The evidence-integrity and publication checks **pass**. The frozen development screen **does not pass**, and the future holdout must remain sealed.

All 30 planned slots ended and were graded. All 444 physical requests were settled and accounted for exactly once, in the frozen order. The source, binary, helper, freeze and approval hashes remained unchanged. The independent read-only cross-check passed 899 checks.

| Development arm | Verified success | Complete total tokens | H12 tokens per verified success |
| --- | ---: | ---: | ---: |
| Off | 8/9 | 1,835,859 | 236,625.41 |
| Raw | 6/9 | unknown | unknown |
| Learned | 9/9 | 1,273,644 | 148,382.78 |

Learned used **37.29% fewer H12 tokens per verified success than off** in these public development tasks. The corresponding reductions at H1/H4/H24 were 28.96%/35.39%/37.80%. All nine learned attempts received verified frozen lesson context: flow 43, queue 42 and report 21 outbound requests, with no invalid exposure. Each training produced three saved lessons. Delivery of context is not proof that memory caused the observed difference.

The successful flow raw seed17 attempt contains one URLError request (physical index 32, 18.2708 seconds) with unknown usage and cost. That request remains unknown and the entire attempt remains in quality denominators. Thus the whole-round completeness and known-cost requirements fail despite favorable learned/off descriptive numbers. No missing value was filled with zero, no attempt was dropped or rerun, and no criterion changed.

The four unsuccessful development patches—report off seed29 and report raw seeds17/29/43—all accepted an oversized empty physical line instead of returning exit 2. Each failed one of seven external checks, with zero grader errors; existing baseline/training tests passed and protected files remained intact.

Training used 228,577 common tokens plus 18,627 reflection tokens, totaling 247,204 tokens and $0.14725568. Common training is allocated equally to all arms and reflection only to learned. Across the actual experiment, the observed physical subtotal was 5,250,958 tokens and $2.60175056; **exact experiment totals remain unknown**. The 5,129,160-token complete-slot subtotal excludes the whole unknown slot and must not be mistaken for the physical subtotal. Agent duration totaled 2,801.29 seconds; provider request timing is reported separately in JSON.

There are only three distinct transfer tasks, repeated across three seeds and three arms. No significance calculation, confirmatory success, general coding superiority, or causal claim follows from this development round. Older negative and inconclusive reports remain separate in the eight unchanged history files.

The publication review enumerated every string-valued field in the new JSON and checked all ten generated files. They contain bounded configuration, measurements, hashes, fixed explanations and already-reviewed history—not private paths, credentials, generation IDs, profiles, raw requests, SSE, or private reasoning. The exact publication file list and digests are in `publishable-files.json`.

The audit uses the frozen harness's recorded admission/grading integrity flags; intermediate profile-admission and pre-grading snapshots were not independently persisted. Physical indices and timestamps support recorded serial order, not an adversarial-host proof. No model calls, grader reruns, profile decryption, SSE decoding, or new sealed-design reads occurred during this review.
