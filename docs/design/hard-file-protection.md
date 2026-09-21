# User-controlled file protection

S-Code provides account-scoped file protection from the Web and macOS Privacy page and from local chat commands (`/protect <path>`, `保护文件 <path>`, `保护目录 <path>`). The normal conversation shows a compact protected-files sidebar. Rules cover project-relative paths and absolute paths outside the project. Directory rules cover descendants. The list is paged, and the sidebar shows up to eight paths.

## What protection means

After activation succeeds, managed file tools deny protected paths and known filesystem aliases. Model requests cannot reuse context from before the protection change. This is enforced by the daemon and file runtime, independently of prompts, permission presets or model choice. The model has no tool to remove protection. Removal is an authenticated user action in Privacy.

Protection applies to the current organization/team/account identity across its tasks. It is not an OS-wide file permission change, and another OS process or another S-Code account does not inherit these rules. Previously transmitted requests cannot be recalled. The feature does not recognize arbitrary copies, screenshots or text deliberately pasted by a user as the same protected file.

## Deliberate restrictions

A live filesystem sandbox deny on a filename is insufficient: a shell can read a pre-existing hardlink or Git object containing the same bytes. Therefore active hard protection disables shell commands, Git tools, MCP and external tools, background terminals, hooks, attachment forwarding and undo. File reads, bounded search/list, guarded edits and local planning/questions remain available. Code Mode only has access to the same permitted child tools; it gains no host access.

Attachments currently have a filename and bytes, not a verified local source identity. The UI and daemon reject forwarding them while protection is active. Protected directories conservatively exclude multiply linked files from managed tools. On platforms without descriptor-based checks, protected file operations fail closed.

Every policy change also isolates automatic repository instructions, installed skills, memories and editor context. These stores do not yet retain sufficient acquisition provenance to prove that their contents remain safe. They remain isolated after the last rule is removed; the agent can read permitted files afresh. Historical conversation remains visible locally, but only messages from turns admitted after the latest reset may be reused as model context.

## Persistence and activation

Rules are encrypted in local storage with authenticated owner identity. Each policy has a compare-and-swap revision and a durable context cutoff, retained even when the rule list becomes empty. Rules store both the lexical absolute path and resolved path, plus filesystem device/inode identity where supported. A missing file can be protected before creation. Existing and replacement files at a protected path stay blocked.

Activation cancels account work and stops background terminals while waiting for the policy's exclusive gate. Model dispatch, managed tools, hooks and background terminal lifetimes hold shared guards. Only a committed policy is reported as protected. Failed or interrupted activation does not display an optimistic success. A request already being transmitted may finish before activation can commit.

The provider wrapper holds its guard across actual request setup, HTTP dispatch and routed fallback attempts. Each subsequent model request checks the originating turn against the current cutoff. Old approval/question checkpoints fail closed. History filtering uses originating turn time rather than message persistence time, so a late response from an old request cannot become fresh context. Forks, retries, queued input, durable work and manual compaction preserve this boundary.

## Managed file access

The file runtime compares lexical/resolved paths before access, then checks pinned, no-follow file and directory descriptors before reading content, constructing edit snapshots or traversing search results. Identity checks cover existing hardlinks, renamed files/directories, changed protected-path targets and workspace root ancestry. Case aliases are conservatively matched on macOS. Protected search does not parse ignore files, because those files can themselves be protected.

The implementation is deliberately conservative. Preserving arbitrary commands would require a separately designed isolated filesystem view without protected identities, historical Git objects or old caches, plus trusted writeback. Passing a filename deny to the existing shell sandbox is not sufficient.

## API and UI

- `GET /v1/sessions/{id}/privacy/protections`: authenticated owner policy.
- `POST` on the same route: `{scope, path, expected_revision}`.
- `DELETE /v1/sessions/{id}/privacy/protections/{rule_id}`: scope and expected revision in the query.
- `privacy.protection.updated`: refresh the account policy even when another session is selected.

Mutations use revision checks. A conflict refreshes the current policy and requires a deliberate retry. Session changes invalidate stale client responses. Controls show pending activation, errors and capability restrictions. File-tree protection badges are path-based hints; the backend remains authoritative for filesystem aliases.

## Verification

Regression coverage targets protected direct reads, search, edits, aliases, replacement paths, renamed ancestors, stale tool execution, encrypted persistence, account isolation, revision conflicts, empty-policy barriers, outbound dispatch serialization, old history/checkpoints, forks/retries, compaction and UI response races. Tests use synthetic secrets and local fixture providers, never real credentials or production endpoints.
