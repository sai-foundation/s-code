---
site: true
slug: verification
title: Verification and releases
short_title: Verification
group: Operate
order: 90
description: Run the Community gate and understand private release-candidate evidence.
keywords:
  - testing
  - verification
  - release
  - CI
  - source
---

# Verification and releases

## Complete gate

Run the complete Community validation from the repository root:

```sh
scripts/verify-community.sh
```

The gate checks the repository manifest, documentation, formatting, Clippy,
Rust tests, dependency policy, advisories, generated protocol bindings, Local
Web, the product documentation site and the source installation contract.

## Focused checks

Focused entrypoints are documented in [`tests/README.md`](../../tests/README.md).
The release-candidate gate additionally runs the real CLI/daemon/model fixture:

```sh
tests/test-cli-e2e.sh
```

Use focused entrypoints while iterating, then run the complete gate against the
exact source revision intended for review.

## Performance evidence

The coding harness is evaluated by outcome first: a run enters the performance
comparison only after the same frozen external grader accepts its workspace.
Selected comparisons use the same task fixture, starting state, model endpoint
and model. Latency is wall-clock elapsed time and token counts are the totals
reported by each harness's provider path.

The 2026-08-31 hard-task follow-up used `z-ai/glm-5.3`:

| Task and harness | Passing samples | Elapsed seconds | Total tokens |
| --- | ---: | --- | --- |
| Durable Task Queue · Community | 3/3 | 39.522, **46.741**, 102.896 | 101,578, **121,904**, 254,040 |
| Durable Task Queue · OpenCode | 3/3 | 48.219, **50.020**, 55.949 | 179,394, **186,791**, 211,140 |
| Dependency Flow Runner · Community | 3/3 | 51.063, **58.820**, 67.964 | 114,547, **128,873**, 147,091 |
| Dependency Flow Runner · OpenCode current completed sample | 1/1 | 105.875 | 351,329 |

Bold values are medians where three completed samples exist. A second current
OpenCode Flow run was stopped after approximately 390 seconds with only three
of four frozen checks passing and at least 635,810 reported tokens; it is
disclosed but excluded from completed-run latency statistics. The Community
Queue maximum is also disclosed because median leadership does not mean every
individual run is faster.

On the repeated Queue comparison, Community's median was 6.6% faster and used
34.7% fewer tokens. Against the current completed OpenCode Flow sample,
Community's three-run median was 44.4% faster and used 63.3% fewer tokens.
These results support the named tasks and model only; they are not a universal
ranking of all repositories, models or workloads.

## Release candidates

The promotion pull request runs DCO, the complete Linux source gate, macOS and
Windows Rust jobs, Linux/macOS CLI E2E and both IDE clients in parallel. After
every required job succeeds, CI records the source revision, pull-request head,
workflow run and tested Git tree in a qualification artifact.

After merge, `rc.yml` performs no rebuild. It downloads the successful
qualification evidence, proves that downstream `main` has the exact tested Git
tree and emits a `community-release-ready-*` artifact. Private source candidates
are identified by that evidence, the exact downstream commit and the successful
qualification run. Private staging does not create candidate tags or artifact
bundles. After publication is enabled and the repository is public, pushing an
existing version tag runs the complete source gate and creates a GitHub Release
with GitHub's generated source archives and release notes. The workflow never
creates tags, changes visibility or uploads precompiled files.

The automatic Community CI includes CodeQL with a public-visibility gate. GitHub Code
Security is not available for private repositories on GitHub Free or Pro, so
the job is intentionally skipped during private staging and starts running
automatically after publication. Maintainers must require its successful check
before accepting external changes once the repository is public.

## Current status

A private candidate becomes a public source release only after publication is
explicitly enabled by a reviewed contract change and the repository controls
have been validated.
