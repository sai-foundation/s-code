---
site: true
slug: introduction
title: Introduction to S-Code
short_title: Introduction
group: Start here
order: 10
description: Safe, self-evolving, and swift coding agent.
keywords:
  - overview
  - s-code
  - agent
  - execution plane
---

# Introduction to S-Code

S-Code is a safe, self-evolving, and swift coding agent for your terminal and
browser, licensed under Apache-2.0. Both interfaces share a local service for model calls, tools,
permissions and session history.

An agent turn can inspect your repository, edit files, run tests and recover
from failures. Diffs, test output and tool activity stay together in the session
so you can review the result.

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

## Availability

`v0.1.0-preview.1` is a source-only Developer Preview for macOS and Linux.
The first public release is still in preparation. See the
[Preview details](deployment/preview-release.md) for supported features and
known limitations.

## Continue reading

- [Build, install and run](deployment/community.md)
- [Core concepts](architecture/core-concepts.md)
- [Configuration](guides/configuration.md)
- [Architecture overview](architecture/overview.md)
- [Security model](architecture/security.md)
- [Privacy and security comparison](product/privacy-security-comparison.md)
- [Testing and verification](testing/README.md)
- [Preview release readiness](deployment/preview-release.md)
