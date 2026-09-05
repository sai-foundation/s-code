---
site: true
slug: preview-release
title: Preview release readiness
short_title: Preview release
group: Operate
order: 95
description: Understand the v0.1.0-preview.1 scope, evidence and final public launch gate.
keywords:
  - preview
  - release
  - launch
  - source
---

# Preview release readiness

## Supported Preview scope

`v0.1.0-preview.1` is a source-only Developer Preview for macOS and Linux. It
includes the CLI, Local Web, local sessions, guarded tools, approvals, audit,
Git integration and local single-user MCP. The checked-in VS Code and JetBrains prototypes are
experimental source fixtures, not supported Preview clients: they do not yet
share the installed application's automatic local connection flow. Windows
native tool execution, precompiled binaries, automatic updates and IDE
marketplace distribution are outside this Preview.

The Preview is intended for evaluation on non-production repositories. Back up
important work and inspect the proposed diff before accepting an edit.

## Evidence required before the tag

The exact candidate commit must pass:

- the complete Community source gate;
- macOS and Linux CLI end-to-end tests;
- clean-home setup, daemon autostart and first real task;
- ten executable privacy/security use cases;
- macOS and Windows Rust compilation and tests;
- Local Web and documentation-site checks;
- DCO and candidate-tree evidence binding.
- a high-confidence credential scan over the tracked tree and every blob
  reachable from the local Git refs (`python3 scripts/check-community-secrets.py --history`).

Performance comparisons admit only reviewed, trusted workspaces accepted by the
frozen outcome checks. Public samples, fixtures and derived-claim validation live under
`tests/benchmarks/`.

## Final publication gate

The repository remains private and `publication_enabled` remains `false` until
the code and evidence review is complete. After visibility changes to public,
maintainers must configure branch protection before announcing the project:

1. require a pull request, one approval, resolved conversations and a current
   branch;
2. require the fixed `source-gate` check, which includes DCO and every selected
   test, including CodeQL when applicable;
3. block force pushes and branch deletion;
4. enable private vulnerability reporting and set the documentation homepage;
5. merge the reviewed change that sets `publication_enabled` to `true`;
6. manually run **S-Code full verification** on the exact reviewed `main`
   commit and require its complete public matrix and release-ready evidence;
7. create the reviewed `v0.1.0-preview.1` tag from that qualified commit.

The tag workflow reruns the complete gate, creates a draft GitHub release,
attaches a deterministic source archive, SPDX SBOM, third-party license
information, checksums, machine-readable release evidence and GitHub artifact
attestations, then downloads and verifies the exact asset set and checksums.
Only after those checks does it publish the draft (and mark Preview versions as
prereleases), so users never see a partially uploaded release. The workflow
does not create tags or change repository visibility.

## Known limitations

- The Preview can prepare and commit an approved local diff, but it does not
  expose automatic Git push to the agent. Review the commit and push it with
  your normal Git client so remote credentials and destinations remain under
  direct user control.
- The default distribution is source installation and requires the documented
  Rust, Node.js, Python, Git and platform build prerequisites.
- Local managed encryption protects a copied database file, but not an attacker
  who controls the signed-in operating-system account and can read both the
  database and local key.
- Network access is disabled for normal tool commands unless separately
  requested and permitted. The model endpoint itself is an explicit external
  data boundary.
- Benchmark results describe the named tasks, versions and samples. They are
  not a universal ranking across repositories, models or providers.
- The Preview event cursor is globally monotonic. Filtered clients cannot read
  another actor's event, but sequence gaps can reveal that other local activity
  occurred; use separate daemons where even that metadata is sensitive.
- Team Grant mode does not load MCP runtimes or OAuth credential providers in
  the Preview. Local single-user mode retains MCP; shared per-actor runtime
  routing is required before MCP can be enabled for Team Grant deployments.
