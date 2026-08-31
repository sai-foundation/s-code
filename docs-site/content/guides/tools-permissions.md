---
site: true
slug: tools-permissions
title: Tools and permissions
short_title: Tools & permissions
group: Build with Opencoding
order: 60
description: Understand guarded edits, command sandboxes, permission modes and approvals.
keywords:
  - tools
  - permissions
  - sandbox
  - apply patch
  - approval
---

# Tools and permissions

## Core tools

The default coding profile sends a compact set of core tools for planning,
workspace discovery, file reading, editing, commands and Git inspection.
Specialized built-in and MCP schemas load through tool search only when needed.

## File edits

`read_file` returns file content and a SHA-256 digest. For an existing file,
`apply_patch` requires that exact digest and can apply uniquely matching
`old_text`/`new_text` blocks. A stale or ambiguous edit fails without
overwriting the file.

## Command profiles

| Profile | Use | Boundary |
| --- | --- | --- |
| `read-only` | Inspection and non-mutating checks | No workspace writes |
| `workspace-write` | Builds, tests and local generators | Approved workspace writes; network off by default |
| `browser-test` | Local Playwright or Chromium suites | Browser IPC compatibility with workspace and network restrictions reapplied |

## Permission modes

Manual mode requests approval. Accept-edits can approve policy-allowed file
changes. Workspace mode can approve local, sandboxed, no-network commands and
workspace writes, while network access and writes outside the workspace remain
governed.
