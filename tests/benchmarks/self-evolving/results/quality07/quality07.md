# Quality07 independent development audit

Development-only: three distinct transfer tasks, each repeated across three seeds and three arms.

This is descriptive development evidence. It does not establish a confirmatory advantage or general coding superiority.

| Arm | Verified within budget / planned | Raw grader passes | Tokens | Cost USD | Agent seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 8/9 | 8 | 1,614,621 | 0.780758 | 772.4 |
| raw | 9/9 | 9 | 1,309,488 | 0.690006 | 642.1 |
| learned | 7/9 | 7 | 1,439,992 | 0.721418 | 628.0 |

Training allocation sensitivity (tokens per verified success):

| Horizon | Off | Raw | Learned | Learned reduction vs off |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 290,846.2 | 224,626.3 | 315,747.1 | -8.56% |
| 4 | 224,082.3 | 165,280.6 | 233,221.6 | -4.08% |
| 12 | 209,245.8 | 152,092.6 | 214,882.6 | -2.69% |
| 24 | 205,536.7 | 148,795.7 | 210,297.9 | -2.32% |

All planned slots:

| Family | Phase | Seed | Arm | Execution | Task status | Grader | Qualified | Tokens |
| --- | --- | ---: | --- | --- | --- | --- | --- | ---: |
| flow | training | 17 | training | ended | completed | pass | pass | 55,583 |
| flow | development | 17 | learned | ended | completed | pass | pass | 557,467 |
| flow | development | 17 | off | ended | completed | pass | pass | 326,839 |
| flow | development | 17 | raw | ended | completed | pass | pass | 289,555 |
| flow | development | 29 | off | ended | completed | pass | pass | 329,842 |
| flow | development | 29 | raw | ended | completed | pass | pass | 119,408 |
| flow | development | 29 | learned | ended | completed | fail | fail | 126,083 |
| flow | development | 43 | raw | ended | completed | pass | pass | 191,878 |
| flow | development | 43 | learned | ended | completed | pass | pass | 128,458 |
| flow | development | 43 | off | ended | completed | pass | pass | 129,580 |
| queue | training | 17 | training | ended | completed | pass | pass | 98,973 |
| queue | development | 17 | raw | ended | completed | pass | pass | 150,048 |
| queue | development | 17 | learned | ended | completed | pass | pass | 85,188 |
| queue | development | 17 | off | ended | completed | pass | pass | 176,756 |
| queue | development | 29 | learned | ended | completed | pass | pass | 157,183 |
| queue | development | 29 | off | ended | completed | pass | pass | 242,833 |
| queue | development | 29 | raw | ended | completed | pass | pass | 139,262 |
| queue | development | 43 | off | ended | completed | pass | pass | 158,672 |
| queue | development | 43 | raw | ended | completed | pass | pass | 145,248 |
| queue | development | 43 | learned | ended | completed | pass | pass | 134,280 |
| report | training | 17 | training | ended | completed | pass | pass | 102,190 |
| report | development | 17 | off | ended | completed | pass | pass | 67,619 |
| report | development | 17 | raw | ended | completed | pass | pass | 109,395 |
| report | development | 17 | learned | ended | completed | pass | pass | 65,218 |
| report | development | 29 | raw | ended | completed | pass | pass | 62,634 |
| report | development | 29 | learned | ended | completed | fail | fail | 87,966 |
| report | development | 29 | off | ended | completed | fail | fail | 83,392 |
| report | development | 43 | learned | ended | completed | pass | pass | 98,149 |
| report | development | 43 | off | ended | completed | pass | pass | 99,088 |
| report | development | 43 | raw | ended | completed | pass | pass | 102,060 |

Audit qualification:

- No formal significance estimate; repeated seeds are not independent task identities.
- Every planned slot remains in quality denominators; missing or unaccounted totals are null.
- Lifecycle totals allocate common training equally to all arms, with reflection charged only to learned, at every listed horizon.
- This workflow trains once per family, freezes its lessons and profile, then evaluates reuse. It does not measure continuous learning or reflection after every later task.
- Lifecycle equivalents are not literal experiment spending. Agent time and provider time are separate; reflection wall time is not inferred.
- Historical and adaptively stopped rounds remain separately available and are not pooled as treatment effects.
- The prospective screen also requires known dollar costs; known tokens with unknown cost remain reported but cannot pass that screen.
- Profile admission and before/after-grading immutability use the frozen harness checks and recorded flags; no pre-grading or profile-admission snapshot was independently persisted.
- Actual distilled exposure checks exact outbound payloads against frozen lessons; it does not establish semantic correctness or causality.
- Quality07 prospectively adds eligible saved guidance in every positive family and verified injection in all nine initial learned coding requests. The older global exposure criterion is reported separately; Quality06 is not reclassified.
- Every terminal learning API record is hashed and projected to bounded mode, generation, outcome enums and counts. An absent last outcome is preserved; historical status is not a success or current eligibility claim.

Prospective H12 / quality / accounting / all-nine-initial-exposures screen: **False**. This is a development screen, not a significance result or reveal authorization.
Older aggregate screen before the new family-exposure requirement: **False**. Reliable exposure in every positive family: **True**.

Complete and auditable development round: **True**. Proceeding to a sealed holdout requires independent review; this helper does not authorize reveal.

The JSON includes per-family and paired-seed details, unknown-accounting counts, configuration checks, artifact hashes, and separate common/reflection training components.

Historical evidence is preserved in the companion historical JSON files; its hashes are recorded in this report's JSON.
