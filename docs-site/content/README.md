---
site: true
slug: introduction
title: Introduction to Opencoding
short_title: Introduction
group: Start here
order: 10
description: Understand the Community execution plane and the contract it provides to coding agents.
keywords:
  - overview
  - community
  - agent
  - execution plane
---

# Introduction to Opencoding

Opencoding Community is the Apache-2.0 local execution foundation for an AI
coding agent. It combines the Agent loop, CLI, Local Web, model gateway, tools,
policy, approvals, audit, storage, Git integration, MCP and local configuration
in one execution plane.

The product is built for repository work that must finish with evidence, not
for isolated code completion. A turn can inspect files, make guarded edits, run
the real test suite, recover from model or process failures and leave a
replayable record.

## Request to evidence

```text
Understand → Change safely → Run for real → Recover cleanly → Prove
```

- **Understand:** bounded reads, search, repository instructions and Git context
  ground the model.
- **Change safely:** exact text edits use SHA-256 preconditions so stale writes
  fail before overwriting newer work.
- **Run for real:** structured commands execute in explicit sandboxes with
  network disabled by default.
- **Recover cleanly:** safe model-stream retries avoid replaying completed tool
  side effects, and command timeouts reclaim the command process group.
- **Prove:** test output, diffs, usage, approvals and audit events belong to the
  same session.

## Community boundary

Community contains the shared local execution plane. Separately distributed
systems may compose additional identity and governance capabilities through
public protocols, but Community does not depend on their implementation.

> **Release status:** `v0.1.0-preview.1` is being qualified as a source-only
> Developer Preview for macOS and Linux. There is no public supported release
> until the publication gate completes.

## Continue reading

- [Build, install and run](deployment/community.md)
- [Core concepts](architecture/core-concepts.md)
- [Configuration](guides/configuration.md)
- [Architecture overview](architecture/overview.md)
- [Security model](architecture/security.md)
- [隐私与安全设计对比（中文）](product/privacy-security-comparison-zh.md)
- [Testing and private release candidates](testing/README.md)
- [Preview release readiness](deployment/preview-release.md)
