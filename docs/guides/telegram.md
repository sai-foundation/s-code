---
site: true
slug: telegram
title: Telegram remote tasks
short_title: Telegram
group: Build with S-Code
order: 67
description: Start and follow local coding tasks from your phone with Telegram.
keywords:
  - telegram
  - phone
  - messaging
  - remote tasks
---

# Telegram remote tasks

Connect a Telegram bot to S-Code to start tasks, check progress, approve an
operation or stop work from your phone. Tasks use the same sessions, workspace,
model and permission settings as the Web and CLI interfaces.

Your computer must stay awake, online and running S-Code. The daemon connects
outbound to Telegram using long polling; no public listener or inbound port is
required. This initial channel supports private text conversations with one
paired Telegram user per S-Code account. Group chats, forwarded messages,
attachments and voice are ignored.

## Connect your phone

1. Open [BotFather](https://t.me/BotFather) in Telegram and create a dedicated bot.
2. On your computer, run:

   ```sh
   s-code im telegram connect
   ```

   Paste the bot token at the hidden prompt. S-Code validates the token and
   stores it in its encrypted local database. The token is never included in
   status output. A legacy plaintext database must be upgraded before connecting.
3. Open the private pairing link printed by S-Code on your phone and press Start.
   It expires after ten minutes. Do not share it.
4. Back on your computer, inspect the pending user and confirm your numeric
   Telegram user ID:

   ```sh
   s-code im telegram status
   s-code im telegram approve <telegram-user-id>
   ```

   Opening the link alone does not grant access. Confirmation on the computer is
   required. Generate a replacement link with `s-code im telegram pair`.

For unattended configuration, put the token in an environment variable and use
`s-code im telegram connect --credential-handle TELEGRAM_BOT_TOKEN`.
Do not put the token itself in command arguments.

## Authorize a session

Create a session in S-Code's Web or CLI interface first. Then list and authorize
it on your computer:

```sh
s-code im telegram sessions
s-code im telegram allow <session-id>
```

Authorization includes access to that session's conversation and workspace.
Only sessions owned by the active S-Code account can be authorized. Up to twenty
sessions can be shared with the paired phone. Telegram cannot authorize a new
workspace or change the account's identity.

In Telegram, use `/sessions`, then `/use <session-id>` to select one. The first
authorized session is selected automatically when possible. Send a task in plain
text. Progress updates edit a single status message; final replies contain an
excerpt, with the full response available in S-Code. After a task finishes, send
another message to continue the same conversation. Only one phone-started task
is tracked at a time.

| Telegram command | Action |
| --- | --- |
| `/help` | Show available commands |
| `/sessions` | List authorized sessions |
| `/use <session-id>` | Select a session when no tracked task is active |
| `/status` | Refresh the current task's progress |
| `/stop` | Stop the current task; completed file changes remain |

When an operation needs permission, its status message shows an approval summary
and **Approve once** / **Reject** buttons. A button is bound to the paired user,
chat, message, task and exact operation, expires after ten minutes, and grants
one operation only. Phone approval is available for file edits only when the complete operation
fits in the preview without redaction. Commands, large edits, hidden values and
other tool types show a summary with a Reject button; approve those operations
in the Web or CLI. Command approvals currently execute synchronously, so keeping
them local prevents a long command from blocking phone controls. Commands
already allowed by the session policy still run normally. Ordinary messages such as
“yes” never authorize an operation.
Agent clarification questions currently need an answer in the Web or CLI.

## Remove access

```sh
s-code im telegram disallow <session-id>
s-code im telegram revoke
s-code im telegram disconnect
```

`disallow` removes one session. `revoke` removes the paired phone and pending
pairing. `disconnect` also removes the stored bot token and session authorizations.
These actions stop future remote commands and notifications; they do not undo or
cancel already started work. Stop that work in S-Code if needed. Revoke a leaked
bot token with BotFather too.

Bindings and credentials are stored under the complete organization, team and
actor identity. Only the currently active account's channel runs. Switching
accounts pauses the previous account's channel; it does not transfer access.

## Delivery and recovery

S-Code persists a Telegram update receipt before starting an operation. Repeated
updates do not start the same task again. If the daemon stops in the brief gap
between accepting and recording an operation's result, it reports an interrupted
delivery rather than automatically replaying a potentially mutating task. Check
the session before sending it again. There is no exactly-once delivery guarantee.

Known task IDs and pending notifications survive restarts. Telegram throttling
uses its retry delay; temporary network errors retry with backoff. A progress
message deleted on Telegram is replaced. Outbound notifications may be duplicated
if a response is lost after Telegram accepts them. Text messages older than ten
minutes are ignored rather than unexpectedly starting work after a long absence.

The IM provider receives the messages and summaries sent through this channel.
Tool execution still uses the existing S-Code policy and sandbox boundary.

## Implementation and verification

The channel transport is separate from the daemon's identity, session and
approval logic. A normalized `ImChannel` interface allows additional transports
without adding another tool execution path. State uses the existing encrypted
SQLite store, with revision checks for concurrent updates.

Automated tests use a real encrypted store, daemon routes, agent execution and
file approvals, with controlled Telegram/model responses. Transport tests use
HTTP fixtures to cover Telegram errors, rate limits, redirects and response
bounds. A real BotFather token is needed for a live Telegram smoke test.
