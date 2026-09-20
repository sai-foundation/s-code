# S-Code for VS Code

This extension is a thin client of the local S-Code daemon. It does not run
models, shell commands, policy, or approvals inside the extension host.

For development, open this directory in VS Code and run the `Extension`
launch configuration, or package it with `vsce package`. Configure the loopback
daemon URL, run **S-Code: Connect to Daemon**, and paste the daemon startup
token. The token is stored in VS Code SecretStorage rather than settings JSON.

Connecting reuses a Work session for the current account and project, or creates
one when none exists. Saved sessions are revalidated before use. **Select Shared
Session** lists only this project's Work sessions and shows their mode and folder.
Web Chat sessions stay in Local Web; promoting a Chat into a managed folder does
not attach it to the IDE's open project. In a multi-root workspace, the active
editor's folder determines the project.

The extension sends a bounded, versioned editor context containing the active
document, selection, document revision, and diagnostics. The daemon treats this
as untrusted context with provenance and applies its normal context budget.

**Review Changes by Hunk** parses the approved Git diff, lets the user choose
specific hunks to reject, verifies that the working file still matches the
reviewed hunk, and submits a hash-preconditioned `apply_patch` to the daemon.
The plugin never writes the rejection directly: normal approval, Turn undo and
audit controls remain in force.
