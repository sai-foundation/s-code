# S-Code for Mac

A native Mac workspace for Chat and Work, built with SwiftUI, AppKit and the
S-Code Rust engine. The app includes its own engine. Running the built app does
not require a Terminal session, a separate service or a browser.

## Build and open

Requires macOS 14 or newer, Swift 6 or newer (Xcode or Command Line Tools), and
the repository's pinned Rust toolchain. Apple Silicon is the first tested target.
The build script uses the host architecture; this is not a universal binary.

```sh
scripts/build-macos-app.sh
open .work/macos-dist/S-Code.app
```

Use `--debug` for faster development builds. `SWIFT` can select a compatible
Swift toolchain and `CARGO_TARGET_DIR` can reuse an existing Rust build cache.

Connect your model, then **New chat** for conversation or **Open project** for
work on a folder. Provider discovery is optional: you can enter an exact model
ID. Local endpoints may omit the API key. Remote endpoints require HTTPS and a
key. Each saved connection has its own history and managed workspaces. Update
its saved key to rotate credentials; use **New connection** for another account.

Keys use macOS Keychain. App data is stored under
`~/Library/Application Support/SAI/S-Code/`. The desktop app neither reads nor
stops your existing CLI/Web service. Closing the window keeps the app running;
quitting stops its own engine and confirms when tasks are active.

## Keyboard

| Shortcut | Action |
| --- | --- |
| Command-N | New Chat |
| Command-O | Open project |
| Command-comma | Connections |
| Return or Command-Return | Send |
| Shift-Return | New line |
| Command-period | Stop task |
| Command-Shift-D | Working changes |
| Command-R | Refresh conversation |

Approval cards show the target, risk and policy reason. **Allow once** and
**Reject** act on the daemon's pending request. Tool cards expose details, and
**Changes** displays the project's Git diff. Streaming text remains selectable;
scrolling back reveals **Latest** instead of forcing you to follow new output.

## Test

```sh
python3 tests/test-macos-desktop.py
```

This builds an ad-hoc signed debug bundle and runs native protocol checks plus
a real-daemon loopback model fixture. It exercises Chat, Work, approvals, file
edits, questions, cancellation, history, authentication, and owned-engine
shutdown. No paid API key is required. The runner has bounded timeouts and
cleans up the test processes. Native UI interaction is checked separately; see
the [design and acceptance criteria](../../docs/design/macos-desktop.md) and
[validation record](../../docs/design/macos-desktop-validation.md).

## Distribution

Local builds are ad-hoc signed. They are **not notarized public releases**.
For distribution, build with `S_CODE_CODESIGN_IDENTITY` set to a Developer ID
Application identity, then notarize and staple the app through Apple's release
process. The bundle contains the matching daemon from the same source checkout.
There is no automatic updater in this version. Replacing the bundle preserves
connection settings and history. Project-specific tools (Git, compilers and
runtimes) still need to be available on the user's Mac.

## Conversation permissions

Use the Permissions button below the composer to select Manual, Accept edits,
Workspace or Plan for the current conversation. The choice is saved by the local
engine. Each option explains which operations may proceed automatically; all
modes respect the active security policy. Finish or stop the current task before
switching modes. Existing pending approvals still require an explicit decision.

## Privacy history

Open **Privacy** in a conversation header for a large page covering the main
conversation area while keeping the sidebar available. **Back** returns to chat.

- **Files** shows the local directory tree. Expand folders to load their names;
  no file contents are read by this browser. Red means a complete captured text
  was included in an accepted request; orange means a fragment or unconfirmed
  delivery; white means no transmission was identified in the loaded history.
  The recorded version can differ from today's file. Search covers opened folders
  and recorded paths, including files that have since been removed. Listings cap
  each folder at 3,000 entries and show 200 rows at a time with **Show more files**.
- **Event record** lists individual model dispatches, newest first. Expand an
  event for its endpoint, model, time, request size, outcome, identified file
  sources and other context categories. Sources are expanded in batches.

The page updates as requests start and finish. **Refresh** reloads opened folders
and loaded history. **Load earlier records** expands historical coverage. Account
and conversation changes clear the previous view and cancel pending loads.
Symbolic links are shown but never followed. The client does not retain another
copy of the ledger or store request bodies, file contents or credentials.

An empty history is not proof that no data left the Mac: older activity and
traffic outside recorded model requests may be absent. Attribution can be
incomplete, and endpoint acceptance does not establish provider retention.
