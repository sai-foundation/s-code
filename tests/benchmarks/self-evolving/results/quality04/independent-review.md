# Independent quality04 review

Quality04 is **ready for preparation and independent review of the actual formal freeze**, under the decision plan recorded before report-family training began. It is **not yet authorized for holdout reveal or model calls**. Readiness reflects complete and reliable evidence, not a favorable development result.

This complete development round does **not support a learning advantage**: off passed 9/9 attempts, learned 8/9, and raw 6/9. Every failure remains in the totals. All 30 planned slots ended and were graded; all 484 provider requests settled and were assigned exactly once. No unknown accounting or budget denial occurred. Source/binary/configuration, initial-source and external-grader verdict checks passed.

| Horizon | Off tokens / verified success | Raw | Learned | Learned change vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 283,274.78 | 483,147.50 | 313,312.50 | +10.60% |
| 4 | 209,773.03 | 372,894.88 | 228,772.97 | +9.06% |
| 12 | 193,439.31 | 348,394.29 | 209,986.41 | +8.55% |
| 24 | 189,355.87 | 342,269.15 | 205,289.77 | +8.41% |

A positive change means higher token cost. At H12, learned uses 8.55% more tokens per verified success and costs $0.09670 versus $0.09110 for off. Better performance relative to raw does not replace the off comparison. There are only three distinct public development tasks; no formal significance test was performed.

Actual mechanism exposure is sharply limited:

| Family | Coding training calls | Reflection calls | Stored lessons | Distilled memory in learned development requests |
| --- | ---: | ---: | ---: | ---: |
| flow | 14 | 1 | 1 | 57/57 |
| queue | 13 | 0 | 0 | 0/45 |
| report | 11 | 0 | 0 | 0/38 |

Queue passed 10 tests and then modified README.md. Report passed 8 tests and then modified docs/contracts.md and README.md. Neither reran verification after those mutations. This correctly triggers the frozen no-post-verification-mutation eligibility rule. These empty experience sets are natural retained outcomes, not provider failures or reasons to retrain selectively.

All six queue/report off-versus-learned first-request comparisons are byte-identical: queue requests are 15,392 bytes and report requests 15,384 bytes. No distilled experience was injected anywhere in either family's off or learned traces. The full hashes are in process-diagnostics.json and independent-review.json. Their arm differences cannot separately establish a memory mechanism, even if a later aggregate criterion passes. Do not remove or reweight these families.

The four valid patch failures are:

- flow/raw/seed29: the scheduler ignores the fail_fast parameter when recording a failed task and skipping pending work. It incorrectly suppresses independent work in ordinary mode, violating the explicit unchanged-default requirement.
- report/raw/seed17, report/learned/seed29 and report/raw/seed43: whitespace-only lines are skipped before applying the byte limit. A 12-space input line incorrectly passes a 10-byte limit.

All four daemon tasks reported completed and received valid external failure verdicts. None involved grader infrastructure failure, test/configuration tampering, unknown accounting or a budget denial. The evidence proves patch defects; it does not prove memory caused them.

Physical spending including training: **5,589,646 tokens, $2.65026560**. Common training used 294,007 tokens; reflection used 6,578. Summed agent elapsed time was 2,288.41 seconds, external grading 234.25 seconds, and provider requests 2,024.76 seconds. Provider time overlaps agent time; these are not whole-supervisor wall-clock duration.

The actual formal freeze must bind this readiness review, the decision plan, implementation/analysis/launcher hashes, and each family's manifest, aggregate results, training result, frozen lessons/settings, trained source, profile, raw corpus and request tree. The JSON supplies those exact bindings. Preserve the fixed $45 global and $1.50 per-attempt limits: budget denial makes the primary inconclusive without post-result top-up or selective replacement. A separate release approval must bind the reviewed freeze and subsequently materialized task/grader/support-file hashes before paid execution. The holdout remains sealed during this review.

The earlier historical, stopped quality02 and negative editing03 results remain as separate byte-preserved companion JSON. Changes across rounds and their freshly generated training trajectories are not controlled estimates of individual engineering fixes. This one-task training design measures bounded transfer, not long-term project-memory value. No candidate, grade or attempt was changed or rerun.

The public bundle contains allowlisted measurements, configuration and provenance hashes, with no credentials, absolute user paths, raw request messages, streams, private reasoning or profile contents. Opaque directory hashes were computed for artifact binding without decoding those payloads. The source commit has a DCO sign-off; it is not represented as cryptographically signed.
