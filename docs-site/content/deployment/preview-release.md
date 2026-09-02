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
Git integration, MCP and the source-tested IDE clients. Windows native tool
execution, precompiled binaries, automatic updates and IDE marketplace
distribution are outside this Preview.

The Preview is intended for evaluation on non-production repositories. Back up
important work and inspect the proposed diff before accepting an edit.

## Evidence required before the tag

The exact candidate commit must pass:

- the complete Community source gate;
- macOS and Linux CLI end-to-end tests;
- clean-home setup, daemon autostart and first real task;
- ten executable privacy/security use cases;
- macOS and Windows Rust compilation and tests;
- Local Web, documentation site, VS Code and JetBrains checks;
- DCO and candidate-tree evidence binding.

Performance comparisons admit only workspaces accepted by the frozen external
grader. Public samples, fixtures and derived-claim validation live under
`tests/benchmarks/`.

## Final publication gate

The repository remains private and `publication_enabled` remains `false` until
the code and evidence review is complete. After visibility changes to public,
maintainers must configure branch protection before announcing the project:

1. require a pull request, one approval, resolved conversations and a current
   branch;
2. require DCO, source gate, macOS/Windows Rust, macOS/Linux CLI E2E, VS Code,
   JetBrains, qualification evidence and public CodeQL checks;
3. block force pushes and branch deletion;
4. enable private vulnerability reporting and set the documentation homepage;
5. merge the reviewed change that sets `publication_enabled` to `true`;
6. create the reviewed `v0.1.0-preview.1` tag from the qualified commit.

The tag workflow reruns the complete gate, marks prerelease versions as GitHub
prereleases and attaches third-party license information, checksums and a
machine-readable release-evidence record. The workflow does not create tags or
change repository visibility.

## Known limitations

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
