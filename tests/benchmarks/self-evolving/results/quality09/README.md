# Quality09: verified changes, failed development screen

This directory preserves the complete reviewed output from candidate `704f086`.
The original 20 data/history files and their manifest are byte-identical. Three
independent review sidecars and a publication manifest identify the additional
files. The manifest's `source` entries describe the original audit layout; the
files here use the manifest keys as their names.

All 3 training and 108 transfer attempts finished. Verified successes were
**off 30/36, raw 25/36, learned 27/36**. One learned request has unknown usage and
cost: complete totals remain null. The known physical subtotal across 1,264 of
1,265 requests is **11,495,610 tokens / $5.95418244**, not a complete total.

The independent audit verified all 111 explicit tool-link records, 420 source
deliveries, 27/27 required positive initial exposures and 36/36 initial request
triplets. This establishes delivery and provenance, not usefulness. The fully
accounted queue/report families used 21.38%/51.29% more H12 tokens per successful
task with learning. All failures, retries and unknown accounting remain.

These are twelve previously exposed task identities repeated across seeds, not
108 independent tasks or new confirmation. H12 amortizes one initial training
run; it is not continuous learning after every future task. No holdout was
opened, and this result does not support an efficiency claim.

See [the report](quality09.md) and [independent review](quality09-independent-final-review.md).
The separately developed selective-recall candidate is not part of this result.

Run `python3 -B reproduce.py` from this directory to verify the published file
hashes, success arithmetic and numerical diagnostics. The copied `analysis.py`
is the exact frozen numerical implementation. This reproduction uses no model
or private inputs; it does not independently re-establish private tool provenance.
