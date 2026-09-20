# macOS desktop validation

Development validation on 20 September 2026: Apple M4 Pro, 48 GiB memory,
macOS 26.6.1, arm64, Swift 6.0.3, repository Rust toolchain. The local CLT has
inconsistent private Swift module interfaces; a disposable copied-module/VFS
overlay under `.work` was used for this machine. No system toolchain files were
modified. CI uses its normal Xcode toolchain.

## Automated checks

- `cargo test --locked -p s-code-daemon`: 151 library, 19 binary and 6 worker
  integration tests pass. Includes the new tool-detail endpoint's owning-scope,
  wrong-session, wrong-actor, missing-auth and redaction regression.
- `python3 tests/test-macos-desktop.py`: builds and signs the debug bundle;
  15 native core checks and coordination checks pass. Real engine checks cover
  Chat, Work, SSE, approvals, file mutation, questions, cancellation without
  archiving, provider failure, proposed patch inspection, restart/history and
  invalid authentication. Tests use a deterministic loopback model, no paid key.
- Lifetime checks start an owned daemon, background terminal and child process.
  Both stdin EOF and forced death of the owning parent stop all three without
  test cleanup assistance.
- The optimized `DesktopChecks` executable also passed against the bundled
  release engine, including the provider-failure and tool-detail paths.
- CI routing/workflow checks: 18 pass. Native-only changes select the existing
  macOS job. Runtime and protocol changes include the native client checks.
- Rust formatting, local documentation links and credential scans pass.

## Native UI checks

The actual `.app` was operated through macOS accessibility and inspected visually:

- First connection via Local provider, model discovery and model selection.
- Persistent saved connection and conversation history after normal quit/reopen.
- Command-N new Chat; Command-O native folder picker and Work creation.
- Return sends; native composer accepts text during generation.
- First streamed assistant text appears before completion; Unicode survives.
- Switching away from a running task and back restores its entire streamed prefix.
- Stopping a task leaves the session visible; another message succeeds there.
- Actual file write waits for Allow once, then completes; Changes displays its
  Git diff. Tool cards show full redacted arguments and results.
- Interactive question options and Continue work.
- Provider failure produces a visible persistent failure message.
- A 150-paragraph reply streams while the user scrolls back and types. The reading
  position stays in history, and Latest resumes following. No layout hang after
  replacing the growing transcript's lazy stack with a regular stack.
- Main application and its owned engine disappear after normal quit. Force-kill
  recovery was also exercised during development, separately from the automation.

These are manual UI checks, not an automated frame-rate benchmark. Light mode,
VoiceOver navigation, non-Latin IME composition and macOS 14 hardware still need
broader release qualification; native controls and semantic colors provide the
baseline but do not substitute for those checks.

## Measurements

Single-machine development observations, not comparative marketing claims:

| Measurement | Observation |
| --- | --- |
| Release bundle disk size (`du -sh`) | 30 MiB |
| Release engine launch to health/capability handshake | 490 ms on one warm-cache integration run |
| 10,000 Unicode deltas, exact 40,000 UTF-8 bytes retained | 14 ms in the optimized reducer check |
| Idle UI process after long transcript interaction | 0.0% CPU in a `ps` sample; approximately 136 MiB RSS |
| Idle owned engine in that sample | 0.1% CPU; approximately 57 MiB RSS |

Reducer time is not rendering FPS or provider latency. Initial-window latency has
not been instrumented; the design's one-second window target remains a target.

## Distribution status

The delivered local bundle is ad-hoc signed and verified with `codesign --verify
--deep --strict`. Its executable minimum OS is macOS 14, architecture arm64;
linked libraries are Apple system/runtime libraries. It includes its own Rust
engine and runs without a separately installed CLI or Node/Rust runtime.
A public downloadable release still requires Developer ID signing, Apple
notarization and a release pipeline. This change does not claim App Store or
notarized distribution readiness.

## Independent review

A fresh reviewer with no conversation history approved this as a good, usable
standalone local Mac preview after inspecting the current implementation and
operating the app. It independently reran optimized core and real-engine checks,
verified the bundle signature, exercised question/approval/failure recovery,
and confirmed normal quit removes the owned engine and relaunch restores history.
Its separate warm-cache run observed 116 ms engine readiness and 13.7 ms for
10,000 deltas. Those observations do not establish an FPS claim.

Earlier independent design and implementation reviews resulted in fixes for
parent-death cleanup, saved credential endpoint binding, real wire message kinds,
first-delta rendering/replay, snapshot races, draft preservation and visible
provider failures. Every review used a new agent context. Native UI validation
also fixed a layout loop and cancelling a session instead of its active turn.

## Empty-workspace and concurrent-tool regression

A reported session had three failed manual Git-diff turns while its coding turn
was waiting for approval. The desktop previously treated the last started turn
as the whole session outcome. The corrected activity reducer tracks every turn,
rejects status replay through the snapshot revision, and orders terminal feedback
by the latest state transition. Waiting has a persistent composer label and a
Show request action. Changes checks for a Git root before creating a manual turn;
non-repository and nested workspaces show an explanation. Failed tool detail
sheets now include the daemon-redacted error.

Debug checks and the real bundled release engine passed the mixed-turn regression:
a failed manual diff preserves the pending approval, the approved file edit
completes, and its completion clears stale failure feedback. Unit checks cover
model completion/failure/cancellation after a manual failure, stale status replay,
linked Git worktrees and rejection of nested directories.

The rebuilt release app was also tested through native accessibility with an
isolated profile and a loopback model. Empty-folder Changes created no tool turn.
Three legacy manual failures were injected while an edit awaited approval: the UI
showed the approval and waiting label without a working spinner or task-failed
message. Show request returned from history to the approval card; tool details
showed the Git error; Allow once completed the test edit and cleared waiting state.
No real provider account or existing user task was used for these checks.

A new independent reviewer with no conversation history approved this correction
for the standalone local preview after the first review's completion-ordering
and nested-directory findings were fixed.

The macOS CI fixture startup timeout now has bounded partial-output handling and
stderr diagnostics, and avoids an unnecessary loopback DNS lookup. Ten portable
startup regressions and eighteen CI-routing checks pass. The old timeout's exact
cause was not observable because its fixture diagnostics were discarded.
