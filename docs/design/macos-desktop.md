# S-Code Desktop for macOS

Status: implemented; validation record accompanies this change. Target: macOS 14+, Apple Silicon first.

## Product

A standalone Mac application for a Safe, Speedy, Self-evolving coding agent.
Users double-click S-Code.app, connect a model, and start a Chat or choose a
folder for Work. No Terminal, separately installed daemon, browser tab, Rust,
or Node installation is required to run the delivered app. Project tools such
as Git, compilers and language runtimes remain project-specific prerequisites.

The first release includes model connection setup, a searchable conversation
sidebar, native folder selection, persistent Chat/Work history, streaming
responses, visible tool activity, explicit approval and question cards,
cancellation, copying responses, loading older messages, and recoverable errors.
New connections create isolated profiles; a visible profile chooser retains
access to older histories. Replacing credentials within a saved profile preserves
its history (key rotation). Use New connection for another account. Each profile
owns its database, runtime and managed work directories.
The app must never use or stop an existing CLI/Web daemon.

## Technology decision

| Option | Advantages | Cost for this release |
| --- | --- | --- |
| SwiftUI + selected AppKit controls + Rust sidecar | Native windows, accessibility, menus, text input, system rendering; no browser runtime | Native client implementation; other operating systems require another UI |
| Tauri + existing Web UI + Rust sidecar | High UI reuse; system WebView; cross-platform path | WebView layout and native integration still need optimization and testing |
| Electron + existing Web UI | Mature cross-platform ecosystem; consistent Chromium | Bundles Chromium and Node, plus several process types |

Choose SwiftUI for this Mac-first release. AppKit provides the multiline composer
so IME composition, Return/Shift-Return, selection and paste use native behavior.
The Rust daemon remains the sole execution, policy, audit and persistence owner.
Swift code must not execute model-proposed commands or edit project files itself.
No third-party Swift dependency is needed. Package Manager builds the client;
a script assembles and signs a .app containing the matching Rust daemon.

Sources consulted:
- https://developer.apple.com/documentation/swiftui/performance-analysis
- https://developer.apple.com/videos/play/wwdc2025/306/
- https://v2.tauri.app/reference/webview-versions/
- https://www.electronjs.org/docs/latest/

## Architecture

Native views -> MainActor application state -> asynchronous API client ->
loopback HTTP/SSE -> bundled s-code-daemon -> existing Rust services and SQLite.

The app launches only its bundled executable using Foundation Process, an
allowlisted environment, an ephemeral loopback port, and private per-profile
state. A random bearer token is provided through the child environment; it is
not a command argument or URL. Connection-file PID must match the owned child.
A health/capability handshake completes startup. The app terminates only that
owned process, waits asynchronously, and handles unexpected exit with an explicit
reconnect action. A parent-owned stdin pipe triggers the daemon’s existing bounded
shutdown when the app crashes or is force-quit; normal shutdown closes the pipe
and escalates only its still-owned child after the grace period. Closing a window keeps the application available; quitting
asks for confirmation if a task is active and stops the owned service.

Provider credentials live in macOS Keychain. Connection settings contain a
credential identifier, endpoint and model, never the key. A key is supplied only
to its configured provider and to the owned daemon. Provider discovery rejects
redirects; remote endpoints require HTTPS; plain HTTP is allowed only for local
loopback services. Errors do not display response bodies that may echo credentials.
Profiles are identified by UUID, not raw account names or email addresses.

The daemon API is authoritative. Scope is attached to every scoped request.
Snapshots are validated against the selected session and profile. An operation
epoch prevents delayed results from replacing state after selecting another
session/profile. Approvals always use the original request ID and once scope;
a double click cannot submit twice. Questions support options and typed answers.
The composer permission button selects the existing daemon modes: Manual,
Accept edits, Workspace or Plan. Defaults remain Manual. Labels, descriptions
and locks are loaded from the authenticated daemon; only a confirmed saved mode
is shown as current. Modes never override policy denials. Preferences belong to
the conversation and survive restart. Running tasks cannot change mode in this
client; pending approvals still require a decision. Unknown/load-failed states
disable sending until permissions can be read. In-flight writes remain tracked
per account/session across selection changes, and a final read reconciles the
saved mode before sending is enabled again.

## Streaming and responsiveness

An off-main URLSession stream decodes SSE frames with bounded buffering. Events
are batched before UI delivery. Per-item UTF-8 offsets reject missing/duplicate
message chunks; a snapshot repairs gaps. Cursor reconnect uses the server replay
protocol. Only the selected session is rendered. Sidebar indicators retain running and
approval state for background sessions; quit and profile changes check all active
sessions. Diff inspection and tool/artifact detail sheets complete the native
code-review surface. Session/profile changes cancel
subscriptions and invalidate in-flight operations. Reconnection backs off and
shows its status; authentication failure is not retried indefinitely.

Transcript snapshots and pagination bound initial loading. Transcript rows use stable IDs and equatable views; the initial snapshot is
limited to 100 items. A regular stack avoids a macOS lazy-layout/scroll-to loop
observed with growing text. Sidebar and model catalogs remain lazy. Streaming updates avoid reparsing the entire transcript and publish
at most 20 times per second. The composer remains responsive during streaming.
Auto-scroll follows the latest content only while the user is at the bottom;
otherwise an explicit Jump to latest action appears. Reduced motion is respected.

## Visual and interaction design

A quiet sidebar, generous transcript, and anchored composer. Restrained orange
accent with system semantic backgrounds; light/dark appearance follows macOS.
Use SF Symbols and the existing S-Code mark. Empty state explains Chat versus
Work without displaying implementation details. A visible status describes
Connecting, Ready, Working, Waiting for approval, Reconnecting or Failed.

Keyboard: Command-N new Chat, Command-O open project, Command-comma settings,
Command-Return send (Return also sends; Shift-Return inserts a newline), Escape
closes transient UI. Stop is explicit and available while a task runs. Native
text selection and copy work across streamed and completed messages. Content
and provider errors remain visible until dismissed or corrected.

## Packaging and distribution

scripts/build-macos-app.sh builds matching Swift and Rust revisions, bundles
s-code-daemon and icons, creates Info.plist, signs nested code before the app,
and emits .work/macos-dist/S-Code.app. Local testing uses ad-hoc signing.
Public distribution requires a Developer ID certificate and Apple notarization;
these are a release requirement, not something an unsigned local build claims.
No updater runs in this first version. Replacing the app preserves profile data.
Minimum OS and binary architecture must be checked in the bundle validation.

## Acceptance and evidence

- Launch a real .app from Finder/open on this Mac; onboarding is usable without CLI.
- Connect a fixture provider; create Chat and Work, send/stream/cancel, approve or
  reject a real daemon tool request, answer a question, reopen persisted history.
- Provider failure, daemon exit, session changes during requests, stream disconnect,
  and missing bundled daemon all produce recoverable states.
- Unit/contract tests cover scope, URL validation, SSE framing and Unicode offsets,
  stale results, approvals, persistence and profile isolation. Real daemon smoke
  tests use a loopback fixture provider, never a paid key.
- Release build target: responsive window within 1 second; owned daemon ready
  within 3 seconds on this development Mac after warm filesystem caches. Report
  actual timing with hardware/build context; model latency is measured separately.
- Stream benchmark: 10,000 deltas, bounded batch count and no lost text. Inspect
  idle CPU/RSS and a long conversation interactively. Do not claim measured FPS
  unless a frame profiling tool was actually run.
- Every review is a new agent with no conversation history. The reviewer reads
  current files and evidence independently, and evaluates whether this is a
  usable standalone Mac app. Blocking findings are fixed before another review.

## Boundaries of this release

No App Store submission, notarized download, Windows/Linux UI, auto-update,
multi-window simultaneous conversations, or feature-parity claim with every
Web administration screen. The delivered artifact must still support the core
coding loop independently. Privacy history covers model requests recorded by the
built-in HTTP transports; it is not a monitor of all network connections.

### Privacy history

A Privacy button opens a native panel beside the conversation. It reads the same
local request records as Web and CLI: time, model, endpoint origin, request size,
delivery status and attributed file sources. Partial context is labeled; file
contents and API keys are not included in the history response. An empty history
does not claim that no data was sent before recording was available.

Refresh and older-history controls keep requests bounded. Live model activity
refreshes an open panel without resetting its older-page cursor. Changing the
connection or conversation clears displayed records, cancels pending reads and
rejects late responses from the previous selection. Errors remain retryable.

### Conversation titles

New conversations receive a local first-message title while the first response is
running. This uses existing credential redaction and bounded text; title generation
does not block the response. The model may refine the name in the background.
Failures preserve the local title and permit a later successful turn to retry.
Manual names and completed summaries end automatic refinement. A small additive
storage migration records naming provenance atomically with title writes.
Its landed version is 0050, leaving 0048 for IM and 0049 for privacy indexes.
Earlier Mac preview databases using the identical naming migration at 0048 are
recognized by its exact checksum and description, then upgraded without resetting
history. Unknown, changed or incomplete migrations are not relabeled.
