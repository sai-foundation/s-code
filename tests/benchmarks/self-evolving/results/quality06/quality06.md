# Quality05 independent development audit

Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.

This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.

| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 7/9 | 7 | 2,451,123 | 1.077883 | 1,229.9 |
| raw | 7/9 | 7 | 1,800,902 | 0.914428 | 832.8 |
| learned | 8/9 | 8 | 1,691,115 | 0.787274 | 902.0 |

Training allocation sensitivity (tokens per verified success):

| Horizon | Off | Raw | Learned | Learned reduction vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 529,275.4 | 436,386.7 | 375,762.4 | 29.00% |
| 4 | 394,939.2 | 302,050.5 | 252,482.6 | 36.07% |
| 12 | 365,086.7 | 272,198.0 | 225,087.1 | 38.35% |
| 24 | 357,623.6 | 264,734.8 | 218,238.2 | 38.98% |

All planned slots:

| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |
| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |
| flow | training | 17 | training | ended | completed | pass | pass | 162,412 |
| flow | development | 17 | learned | ended | completed | pass | pass | 200,231 |
| flow | development | 17 | off | ended | completed | pass | pass | 177,925 |
| flow | development | 17 | raw | ended | completed | pass | pass | 260,398 |
| flow | development | 29 | off | ended | completed | pass | pass | 567,580 |
| flow | development | 29 | raw | ended | completed | pass | pass | 467,312 |
| flow | development | 29 | learned | ended | completed | pass | pass | 263,396 |
| flow | development | 43 | raw | ended | completed | pass | pass | 132,632 |
| flow | development | 43 | learned | ended | completed | pass | pass | 250,094 |
| flow | development | 43 | off | ended | completed | pass | pass | 215,223 |
| queue | training | 17 | training | ended | completed | pass | pass | 81,982 |
| queue | development | 17 | raw | ended | completed | pass | pass | 294,081 |
| queue | development | 17 | learned | ended | completed | pass | pass | 115,170 |
| queue | development | 17 | off | ended | completed | pass | pass | 155,063 |
| queue | development | 29 | learned | ended | completed | pass | pass | 251,495 |
| queue | development | 29 | off | ended | completed | pass | pass | 179,246 |
| queue | development | 29 | raw | ended | completed | pass | pass | 157,191 |
| queue | development | 43 | off | ended | completed | pass | pass | 170,154 |
| queue | development | 43 | raw | ended | completed | pass | pass | 170,529 |
| queue | development | 43 | learned | ended | completed | pass | pass | 364,110 |
| report | training | 17 | training | ended | completed | pass | pass | 193,934 |
| report | development | 17 | off | ended | completed | pass | pass | 128,187 |
| report | development | 17 | raw | ended | completed | pass | pass | 93,605 |
| report | development | 17 | learned | ended | completed | fail | fail | 53,895 |
| report | development | 29 | raw | ended | completed | fail | fail | 58,996 |
| report | development | 29 | learned | ended | completed | pass | pass | 92,967 |
| report | development | 29 | off | ended | completed | fail | fail | 111,692 |
| report | development | 43 | learned | ended | completed | pass | pass | 99,757 |
| report | development | 43 | off | ended | failed | fail | fail | 746,053 |
| report | development | 43 | raw | ended | completed | fail | fail | 166,158 |

Audit qualification:

- No formal significance estimate; repeated seeds are not independent task identities.
- Every planned slot remains in quality denominators; missing or unaccounted totals are null.
- Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.
- Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.
- Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects.
- The prospective screen also requires known dollar costs; known tokens with unknown cost remain reported but cannot pass that screen.
- Profile admission and before/after-grading immutability use the frozen harness checks and recorded flags; no pre-grading or profile-admission snapshot was independently persisted.
- Actual distilled exposure checks exact outbound payloads against frozen lessons; it does not establish semantic correctness or causality.

Prospective H12 / quality / accounting / real-exposure screen: **True**. This is a development screen, not a significance result or reveal authorization.

Complete and auditable development round: **True**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.

The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.

Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.
