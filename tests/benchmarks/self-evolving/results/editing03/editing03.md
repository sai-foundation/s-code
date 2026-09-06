# Editing03 independent development audit

Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.

This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.

| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 9/9 | 9 | 1,405,823 | 0.650307 | 645.3 |
| raw | 8/9 | 8 | 1,613,564 | 0.776959 | 677.0 |
| learned | 8/9 | 8 | 1,309,574 | 0.673281 | 547.5 |

Training allocation sensitivity (tokens per verified success):

| Horizon | Off | Raw | Learned | Learned reduction vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 233,179.2 | 288,294.2 | 257,071.8 | -10.25% |
| 4 | 175,446.7 | 223,345.2 | 187,040.5 | -6.61% |
| 12 | 162,617.3 | 208,912.1 | 171,478.0 | -5.45% |
| 24 | 159,409.9 | 205,303.8 | 167,587.4 | -5.13% |

All planned slots:

| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |
| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |
| flow | training | 17 | training | ended | completed | pass | pass | 59,918 |
| flow | development | 17 | learned | ended | completed | pass | pass | 304,133 |
| flow | development | 17 | off | ended | completed | pass | pass | 373,760 |
| flow | development | 17 | raw | ended | completed | pass | pass | 89,223 |
| flow | development | 29 | off | ended | completed | pass | pass | 215,355 |
| flow | development | 29 | raw | ended | completed | pass | pass | 314,511 |
| flow | development | 29 | learned | ended | completed | pass | pass | 87,361 |
| flow | development | 43 | raw | ended | completed | pass | pass | 163,664 |
| flow | development | 43 | learned | ended | completed | pass | pass | 215,769 |
| flow | development | 43 | off | ended | completed | pass | pass | 151,309 |
| queue | training | 17 | training | ended | completed | pass | pass | 75,017 |
| queue | development | 17 | raw | ended | completed | pass | pass | 260,609 |
| queue | development | 17 | learned | ended | completed | pass | pass | 123,652 |
| queue | development | 17 | off | ended | completed | pass | pass | 158,544 |
| queue | development | 29 | learned | ended | completed | pass | pass | 180,753 |
| queue | development | 29 | off | ended | completed | pass | pass | 49,142 |
| queue | development | 29 | raw | ended | completed | pass | pass | 267,458 |
| queue | development | 43 | off | ended | completed | pass | pass | 228,733 |
| queue | development | 43 | raw | ended | completed | pass | pass | 134,264 |
| queue | development | 43 | learned | ended | completed | pass | pass | 113,394 |
| report | training | 17 | training | ended | completed | pass | pass | 114,065 |
| report | development | 17 | off | ended | completed | pass | pass | 62,197 |
| report | development | 17 | raw | ended | completed | fail | fail | 109,433 |
| report | development | 17 | learned | ended | completed | pass | pass | 70,972 |
| report | development | 29 | raw | ended | completed | pass | pass | 95,717 |
| report | development | 29 | learned | ended | completed | pass | pass | 57,427 |
| report | development | 29 | off | ended | completed | pass | pass | 75,934 |
| report | development | 43 | learned | ended | completed | fail | fail | 156,113 |
| report | development | 43 | off | ended | completed | pass | pass | 90,849 |
| report | development | 43 | raw | ended | completed | pass | pass | 178,685 |

Audit qualification:

- No formal significance estimate; repeated seeds are not independent task identities.
- Every planned slot remains in quality denominators; missing or unaccounted totals are null.
- Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.
- Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.
- Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects.

Complete and auditable development round: **True**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.

The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.

Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.
