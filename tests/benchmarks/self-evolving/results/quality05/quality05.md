# Quality05 independent development audit

Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.

This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.

| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 8/9 | 8 | 1,835,859 | 0.870470 | 919.3 |
| raw | 6/9 | 6 | unknown | unknown | 930.7 |
| learned | 9/9 | 9 | 1,273,644 | 0.681955 | 814.5 |

Training allocation sensitivity (tokens per verified success):

| Horizon | Off | Raw | Learned | Learned reduction vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 315,198.8 | unknown | 223,917.3 | 28.96% |
| 4 | 250,911.5 | unknown | 162,116.3 | 35.39% |
| 12 | 236,625.4 | unknown | 148,382.8 | 37.29% |
| 24 | 233,053.9 | unknown | 144,949.4 | 37.80% |

All planned slots:

| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |
| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |
| flow | training | 17 | training | ended | completed | pass | pass | 44,827 |
| flow | development | 17 | learned | ended | completed | pass | pass | 74,532 |
| flow | development | 17 | off | ended | completed | pass | pass | 208,673 |
| flow | development | 17 | raw | ended | completed | pass | pass | unknown |
| flow | development | 29 | off | ended | completed | pass | pass | 371,490 |
| flow | development | 29 | raw | ended | completed | pass | pass | 649,390 |
| flow | development | 29 | learned | ended | completed | pass | pass | 210,096 |
| flow | development | 43 | raw | ended | completed | pass | pass | 115,462 |
| flow | development | 43 | learned | ended | completed | pass | pass | 316,539 |
| flow | development | 43 | off | ended | completed | pass | pass | 227,892 |
| queue | training | 17 | training | ended | completed | pass | pass | 52,717 |
| queue | development | 17 | raw | ended | completed | pass | pass | 140,748 |
| queue | development | 17 | learned | ended | completed | pass | pass | 242,544 |
| queue | development | 17 | off | ended | completed | pass | pass | 271,672 |
| queue | development | 29 | learned | ended | completed | pass | pass | 145,960 |
| queue | development | 29 | off | ended | completed | pass | pass | 293,349 |
| queue | development | 29 | raw | ended | completed | pass | pass | 167,341 |
| queue | development | 43 | off | ended | completed | pass | pass | 108,600 |
| queue | development | 43 | raw | ended | completed | pass | pass | 228,379 |
| queue | development | 43 | learned | ended | completed | pass | pass | 118,329 |
| report | training | 17 | training | ended | completed | pass | pass | 149,660 |
| report | development | 17 | off | ended | completed | pass | pass | 154,788 |
| report | development | 17 | raw | ended | completed | fail | fail | 145,166 |
| report | development | 17 | learned | ended | completed | pass | pass | 38,697 |
| report | development | 29 | raw | ended | completed | fail | fail | 212,065 |
| report | development | 29 | learned | ended | completed | pass | pass | 47,087 |
| report | development | 29 | off | ended | completed | fail | fail | 70,268 |
| report | development | 43 | learned | ended | completed | pass | pass | 79,860 |
| report | development | 43 | off | ended | completed | pass | pass | 129,127 |
| report | development | 43 | raw | ended | completed | fail | fail | 113,902 |

Audit qualification:

- No formal significance estimate; repeated seeds are not independent task identities.
- Every planned slot remains in quality denominators; missing or unaccounted totals are null.
- Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.
- Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.
- Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects.
- The prospective screen also requires known dollar costs; known tokens with unknown cost remain reported but cannot pass that screen.
- Profile admission and before/after-grading immutability use the frozen harness checks and recorded flags; no pre-grading or profile-admission snapshot was independently persisted.
- Actual distilled exposure checks exact outbound payloads against frozen lessons; it does not establish semantic correctness or causality.

Prospective H12 / quality / accounting / real-exposure screen: **False**. This is a development screen, not a significance result or reveal authorization.

Complete and auditable development round: **False**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.

The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.

Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.
