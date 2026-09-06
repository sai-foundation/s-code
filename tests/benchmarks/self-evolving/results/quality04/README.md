# Quality04 development result

This completed development round at `981e4b3` did not show a learning advantage.
Off passed 9/9 transfer attempts, learned 8/9 and raw 6/9. Including common
training and reflection at the fixed 12-task horizon, learned used **8.55% more
tokens per verified success** than off. All 30 planned slots and 484 provider
requests were retained, with complete usage, valid grading and no budget denial.

The [independent review](independent-review.md), [measurements](quality04.json)
and [process diagnostics](process-diagnostics.json) are preserved unchanged.
The review's readiness language records the decision at that time; the resulting
[confirmatory run has now closed](../confirmatory-quality04/README.md).
Readiness was an integrity decision, not evidence of an efficiency gain.

Flow saved one lesson. Queue and report saved none because documentation was
edited after final verification. All six initial off/learned request pairs in
those empty-treatment families were byte-identical. Their observed differences
cannot establish an effect from memory.

The companion earlier-result JSON files preserve negative and stopped history.
They are separate experiments and are not pooled. This development set has
three distinct task identities repeated over three seeds; it does not establish
general or long-term learning performance.
