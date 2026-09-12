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

`read_file` returns numbered content, a SHA-256 digest and a 16-character
`revision`. The model uses the revision to show which file contents it edited.
File tools traverse from a stable workspace directory handle and reject
symlink-swapped components. Revision checking is optimistic concurrency control,
not a cross-process transaction lock: a small check-to-rename race remains.

`apply_patch` supports three formats. The service selects the model-facing
schema and corresponding system instructions using the
[model editing configuration](configuration.md#model-editing-formats).
The legacy single-file `path` / `expected_revision` / `edits` or `content` input
and full `expected_sha256` remain accepted for existing integrations.

### Line edits across files

Each file carries its own revision. Line ranges are 1-based, inclusive and refer
to that file's original `numbered_content`. Ranges within one file must not
overlap. Do not include the displayed line-number prefixes in `new_text`.

```json
{
  "files": [
    {
      "path": "src/a.ts",
      "expected_revision": "0123456789abcdef",
      "edits": [{"start_line": 10, "end_line": 12, "new_text": "return result;\n"}]
    },
    {
      "path": "src/b.ts",
      "expected_revision": "fedcba9876543210",
      "edits": [{"start_line": 4, "end_line": 4, "new_text": "const enabled = true;\n"}]
    }
  ]
}
```

The example revisions are placeholders: copy actual revisions from `read_file`.
A single shared batch revision cannot describe files that were read separately.

### Exact text edits

Use the same `files` envelope, replacing each line edit with an exact text edit:

```json
{"old_text": "const enabled = false;", "new_text": "const enabled = true;"}
```

Each old block must match exactly once. Edits run in array order, so later blocks
see earlier replacements. Ambiguous or missing matches fail; the runtime never
silently guesses a nearby match. Do not mix text and line edits within one file.

### Patch edits

```json
{
  "patch": "*** Begin Patch\n*** Update File: src/a.ts\n@@\n-const enabled = false;\n+const enabled = true;\n*** Add File: note.txt\n+Enabled by default.\n*** End Patch",
  "revisions": {"src/a.ts": "0123456789abcdef", "note.txt": null}
}
```

This is an intentionally strict Add/Update subset of the patch format.
Use bare `@@` before each update hunk and prefix lines with a space for context,
`-` for removal or `+` for addition. Each update hunk must match complete lines
exactly once, including trailing newlines; add unchanged context to disambiguate
repeated lines. Hunks run sequentially. The runtime rejects fuzzy matches,
numbered/named hunk headers, Delete/Move operations and EOF markers. For files
without trailing newlines or with CRLF line endings, use a deliberate
`files` / `content` replacement instead. This is not `git apply` and requires no
Git staging, commit or repository.

All formats can create or deliberately replace a whole file with `content`
instead of `edits`. New files require `expected_revision: null`; existing files
require their current revision.

### Multi-file execution and failure handling

One call accepts up to 32 unique relative paths, 100 edits per file and 16 MiB
each of serialized request input, combined file snapshots and resulting content.
The service validates every path, revision and edit before writing anything.
A stale revision, protected path or invalid edit in any file rejects the batch
without changing the others. The scheduler reserves every target in the batch.

Writes then run sequentially, with a final revision check per write. This is
not a filesystem transaction: a concurrent external change, disk or storage
failure can stop the batch after some files were written. The error lists the
applied paths; later paths are not attempted. Re-read before retrying. Each
applied change keeps its Turn undo record, including the original content or
new-file status. Turn undo refuses to overwrite subsequent external changes.
The model's editing format never changes approval or sandbox requirements.

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
