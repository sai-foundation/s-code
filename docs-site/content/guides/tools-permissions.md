---
site: true
slug: tools-permissions
title: Tools and permissions
short_title: Tools & permissions
group: Build with S-Code
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

The OS sandbox applies to the built-in command tool. Local MCP stdio servers,
Hooks and background terminals are explicitly installed or started host
processes: they inherit the signed-in operating-system account's filesystem and
network authority. Their permission preview states this ambient authority and
must be confirmed. Configuration persists credential handles rather than
values, but the trusted executable receives each resolved value and could read,
transmit or print it. Captured output is redacted before product persistence;
install these processes with the same care as any local program.

Git tools also use a host process rather than the command sandbox. S-Code
resolves Git from an absolute `PATH` entry, ignores system and user Git
configuration, disables hooks, fsmonitor, recursive submodules, signing and
automatic maintenance, uses `--no-ext-diff --no-textconv`, and rejects local
configuration that could start filters, text converters, fsmonitor, includes
or submodule update commands. The check runs immediately before every Git
operation. This blocks Git-configuration command execution; it is not a claim
that the Git process itself runs inside an OS sandbox. Automatic push is not
exposed to the Agent in the Preview.

## File edits

`read_file` returns original UTF-8 file content, including its line endings, a
SHA-256 digest and a short revision. File tools traverse from a stable workspace
directory handle and reject symlink-swapped components. For an existing file,
`apply_patch` requires the revision and applies uniquely matching
`old_text`/`new_text` blocks. Missing or ambiguous matches reject the whole batch.
The result includes a bounded before/after excerpt and the new revision for
inspection and subsequent edits. The revision is an optimistic concurrency
check, not a cross-process transaction lock: a very small check-to-rename race
remains, so stop concurrent generators and inspect the final diff.

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

Preview approvals authorize one operation only. The approval card freezes the
server-projected command or external target, sandbox profile, filesystem scope
and network setting before the operation runs; there is no session-wide approval
shortcut. The explicit `s-code sandbox` command shows the same effective
profile and network request interactively, or requires `--yes` in automation.

## Dependency caches

Rust, Go and npm commands reuse private dependency-content caches beneath
`$XDG_CACHE_HOME/s-code/tool-dependencies` or
`~/.cache/s-code/tool-dependencies`. Set `S_CODE_TOOL_CACHE_DIR` to an
absolute base directory outside the workspace to relocate them. Each canonical
workspace receives a separate private namespace, so one repository cannot read
or poison another repository's cache. The first fetch still requires approved
network access; later commands in that same workspace can reuse downloaded
content offline.

Credential-bearing configuration remains command-local: Cargo uses an isolated
`CARGO_HOME`, npm ignores the host `.npmrc`, Go uses an isolated `GOPATH`, and
the sandbox mounts only the specific content-cache directories as writable.
Version-scoped runtime roots installed by NVM, pyenv, Conda, asdf, mise and
Volta are mounted read-only so their standard libraries and package-manager
files continue to work without exposing the rest of the home directory.
