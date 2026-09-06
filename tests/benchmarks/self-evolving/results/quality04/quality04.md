# Quality04 independent development audit

Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.

This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.

| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 9/9 | 9 | 1,667,452 | 0.785044 | 688.8 |
| raw | 6/9 | 6 | 2,016,864 | 0.978985 | 823.8 |
| learned | 8/9 | 8 | 1,604,745 | 0.736055 | 640.3 |

Training allocation sensitivity (tokens per verified success):

| Horizon | Off | Raw | Learned | Learned reduction vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 283,274.8 | 483,147.5 | 313,312.5 | -10.60% |
| 4 | 209,773.0 | 372,894.9 | 228,773.0 | -9.06% |
| 12 | 193,439.3 | 348,394.3 | 209,986.4 | -8.55% |
| 24 | 189,355.9 | 342,269.1 | 205,289.8 | -8.41% |

All planned slots:

| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |
| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |
| flow | training | 17 | training | ended | completed | pass | pass | 119,525 |
| flow | development | 17 | learned | ended | completed | pass | pass | 251,444 |
| flow | development | 17 | off | ended | completed | pass | pass | 189,092 |
| flow | development | 17 | raw | ended | completed | pass | pass | 103,685 |
| flow | development | 29 | off | ended | completed | pass | pass | 271,119 |
| flow | development | 29 | raw | ended | completed | fail | fail | 332,731 |
| flow | development | 29 | learned | ended | completed | pass | pass | 261,176 |
| flow | development | 43 | raw | ended | completed | pass | pass | 627,759 |
| flow | development | 43 | learned | ended | completed | pass | pass | 241,103 |
| flow | development | 43 | off | ended | completed | pass | pass | 394,178 |
| queue | training | 17 | training | ended | completed | pass | pass | 116,078 |
| queue | development | 17 | raw | ended | completed | pass | pass | 84,476 |
| queue | development | 17 | learned | ended | completed | pass | pass | 49,991 |
| queue | development | 17 | off | ended | completed | pass | pass | 162,608 |
| queue | development | 29 | learned | ended | completed | pass | pass | 126,295 |
| queue | development | 29 | off | ended | completed | pass | pass | 213,486 |
| queue | development | 29 | raw | ended | completed | pass | pass | 357,281 |
| queue | development | 43 | off | ended | completed | pass | pass | 135,242 |
| queue | development | 43 | raw | ended | completed | pass | pass | 256,340 |
| queue | development | 43 | learned | ended | completed | pass | pass | 367,782 |
| report | training | 17 | training | ended | completed | pass | pass | 64,982 |
| report | development | 17 | off | ended | completed | pass | pass | 86,992 |
| report | development | 17 | raw | ended | completed | fail | fail | 62,604 |
| report | development | 17 | learned | ended | completed | pass | pass | 136,363 |
| report | development | 29 | raw | ended | completed | pass | pass | 103,661 |
| report | development | 29 | learned | ended | completed | fail | fail | 99,024 |
| report | development | 29 | off | ended | completed | pass | pass | 108,362 |
| report | development | 43 | learned | ended | completed | pass | pass | 71,567 |
| report | development | 43 | off | ended | completed | pass | pass | 106,373 |
| report | development | 43 | raw | ended | completed | fail | fail | 88,327 |

Audit qualification:

- No formal significance estimate; repeated seeds are not independent task identities.
- Every planned slot remains in quality denominators; missing or unaccounted totals are null.
- Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.
- Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.
- Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects.

Complete and auditable development round: **True**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.

The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.

Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.
