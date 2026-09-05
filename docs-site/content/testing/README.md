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

The local command remains the complete, sequential release-equivalent gate.
Pull-request CI invokes its `policy`, `rust`, and `web` scopes in parallel and
combines them with the cross-platform CLI E2E job under the required
`source-gate` check. Cargo output and package-download caches use stable paths
inside the CI workspace so consecutive tests do not rebuild the same revision.

The gate checks the repository manifest, documentation, formatting, Clippy,
Rust tests, dependency policy, advisories, generated protocol bindings, Local
Web, the product documentation site and the source installation contract.

## Focused checks

Focused entrypoints are documented in [`tests/README.md`](../../tests/README.md).
The release-candidate gate additionally runs the real CLI/daemon/model fixture:

```sh
tests/test-source-install-real.sh
tests/test-cli-e2e.sh
```

The source-installation test builds the actual release through the public
installer before running the first-run black-box test. That test installs the public launcher into an isolated
home, configures a credential handle, proves the secret is not persisted,
autostarts the service, verifies managed encryption with `doctor`, completes a
real guarded file edit, stops the service, exercises offline backup and
integrity verification, and requires normal exit in under ten minutes.

Run archive verification from a fresh extraction, before build outputs and
downloaded dependencies exist. Use a Git clone for repeated development checks.

Use focused entrypoints while iterating, then run the complete gate against the
exact source revision intended for review.

## Coding harness benchmarks

The coding harness is evaluated by outcome first: a run enters a performance
comparison only after the same frozen trusted-workspace check accepts its workspace.
Comparisons must use the same task fixture, starting state, model route and
model. Latency is wall-clock elapsed time and token counts come from the same
provider accounting boundary.

The frozen tasks and outcome graders live under
[`tests/benchmarks/`](../../tests/benchmarks/). The manifest records a digest of
every protected path, including file contents, executable bits, symlinks,
missing paths and unexpected additions. Run:

```sh
python3 tests/test-harness-benchmark.py validate
tests/test-harness-grader-integrity.sh
```

The local benchmark runner is a repeatable engineering tool, not a secure
anti-cheat supervisor. Python checks import candidate code into the checker
process, while frontend checks execute candidate JavaScript in the browser. It therefore
must be used only with trusted workspaces, or inside an outer container or VM.
Its integrity checks reject the tampering patterns covered by the tests, but a
hostile candidate can still attempt self-modification, test interference or
result forgery.

For a publishable cross-harness result, reviewers must inspect the candidate
diff for test-interference code, then export the workspace to a clean supervisor.
That supervisor reruns the check, verifies protected digests both before and
after execution, and retains its raw output. Neither this clean rerun nor an
outer VM turns same-process language tests into a hostile-code boundary; a pass
from the local runner alone is not evidence for a competitive performance claim.

A future public comparison must check in the exact public Community revision,
competitor version and configuration, model identity, raw per-run artifacts,
provider usage, grader output, failures and stopped runs. Summary medians and
percentage claims are derived only from those artifacts. Historical private
Integration measurements are deliberately not presented as Community release
evidence because an external contributor cannot reproduce their source tree.

## Release candidates

Every Community pull request runs DCO, the complete Linux source gate, macOS and
Windows Rust jobs, Linux/macOS CLI E2E and both IDE clients in parallel. After
every required job succeeds, CI records the source revision, pull-request head,
workflow run and tested Git tree in a qualification artifact.

After merge, `rc.yml` performs no rebuild. It downloads the successful
qualification evidence, proves that Community `main` has the exact tested Git
tree and emits a `community-release-ready-*` artifact. Private source candidates
are identified by that evidence, the exact Community commit and the successful
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
