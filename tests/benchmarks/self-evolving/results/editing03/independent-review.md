# Independent editing03 review

The complete, auditable development round does **not support a learning advantage**. Off passed 9/9 development attempts; learned and raw each passed 8/9. Every attempt, including both failures, remains in the results. There are three distinct development tasks, with three seeds per arm; no formal significance test was performed.

All 30 planned slots ended and were graded. All 424 provider requests settled and were assigned exactly once. The a11 source snapshots, binary copies, request settings and each development attempt’s initial source match the freeze. All 30 recorded grader verdicts match their external grading output. No unknown usage or budget rejection occurred.

| Horizon | Off tokens / verified success | Raw | Learned | Learned change versus off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 233,179.22 | 288,294.25 | 257,071.75 | +10.25% |
| 4 | 175,446.72 | 223,345.19 | 187,040.50 | +6.61% |
| 12 | 162,617.28 | 208,912.06 | 171,478.00 | +5.45% |
| 24 | 159,409.92 | 205,303.78 | 167,587.38 | +5.13% |

A positive change above means higher token cost. At H12, learned costs $0.08880 per verified success versus $0.07554 for off. The smaller unadjusted learned token total does not offset its lower success count. Its better comparison with raw does not replace the off comparison.

Both failed patches—report/raw/seed17 and report/learned/seed43—skip whitespace-only lines before checking their byte length. A line containing 12 spaces therefore passes a 10-byte limit. The frozen public requirement covers input lines without exempting whitespace, and the external grader correctly rejects both patches. Both daemon tasks reported completed; this is a real patch-contract failure, not a grader, test-protection, budget, or accounting failure. It does not establish that memory caused the error.

Physical experiment spending, including training and reflection: **4,577,961 tokens, $2.24893008**. Common training was 230,930 tokens and reflection was 18,070. Summed agent time was 2,002.75 seconds; grading separately took 232.44 seconds. Summed provider-request time was 1,783.14 seconds and overlaps agent time. These sums are not supervisor wall-clock duration.

Keep the untouched holdout sealed while the already-planned quality04 fixes undergo implementation review and a separately frozen full development round. Preserve editing03 and the earlier historical/stopped rounds; do not attribute later improvements to a particular fix without independent evidence, selectively rerun failed slots, or change this round’s denominator. A development threshold should not become a target pursued by choosing favorable runs. This one-shot, small-project design measures bounded transfer, not long-term memory accumulation.

The companion JSON contains the detailed audit, source references by hash and failure evidence. The earlier sanitized historical and stopped-round JSON files are retained byte-for-byte with their original hashes. The six-file report bundle contains no credentials, absolute user paths, profiles, generation IDs, raw model streams, private reasoning, or sealed task content. No candidate, grader outcome, or model attempt was changed or rerun during this review.
