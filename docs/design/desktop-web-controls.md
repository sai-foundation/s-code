# Desktop and Web task controls

The main task view should answer four questions before a message is sent:
which project is open, which model will receive the request, which actions need
approval, and which files are protected. Desktop and Web use the same concepts
while retaining platform-native controls.

## Layout

- Sidebar: new chat, projects/open project, search, recent conversations, account
  and settings. Refresh and diagnostics are secondary actions.
- Header: conversation title, Chat/Work context, short workspace name, Changes,
  Privacy, and secondary conversation actions. A workspace menu exposes the full
  path and copying; native desktop also offers Show in Finder.
- Composer: attachment/context actions where supported, model picker, permission
  picker, and send/stop controls. Running-task input must say whether it queues a
  follow-up or redirects the current task.
- Conversation inspector: a compact protected-file summary, grouped into project
  and external paths, with an add action and access to the full privacy view.
- Privacy: the existing wide view retains Files and Events. Historical transfer
  coverage and present protection are independent properties.

Chat/Work describes workspace access. Permission modes describe approvals within
the current policy. Hard file protection remains an account-scoped restriction;
changing permission modes never overrides it. Display server-confirmed settings,
invalidate stale responses after account/session switches, and explain controls
disabled by missing capabilities or policy.

## Delivery

1. Primary controls: visible project and Changes entry points, compact workspace
   menus, readable model/permission controls, and a native session model picker.
2. Task feedback and privacy: actionable tool/approval feedback, consistent
   terminal states, and grouped protection summaries with quick management.
3. Working efficiency: readable native diffs, explicit running-task input,
   keyboard access, and restrained motion with reduced-motion support.

Each part is a separately reviewed pull request. No new permission or model
capability is implied by a label. Unavailable actions remain unavailable, with
an explanation and a relevant recovery action. Future automatic file counts or
diff previews must use guarded backend reads, not bypass protected-file policy.

## Interaction requirements

- Model selection is searchable and scoped to the current account. Changing a
  model persists only after the server accepts it; a stale response cannot
  change a newly selected conversation. Providers without reasoning controls
  do not show a nonfunctional effort picker.
- Permission descriptions come from the server catalog. A running task cannot
  silently change approval policy. Locked choices explain why they are locked.
- Adding a protected path uses the existing local protection API. Paths refer
  to the daemon's machine, which may differ from the browser's machine.
- A protected path can still have historical full/partial transfer records.
  Lock badges must not erase those records or imply past disclosure was undone.
- Successful tool steps are compact. Failures show a human-readable reason and
  retain access to exact details. Approvals show the action being approved.
  Pending approvals carry an optional exact tool-call reference so inspection
  still works when their transcript page is not loaded. Older servers may omit
  it; clients can use an exact snapshot detail link or explain that details are
  unavailable. Clients never infer an action by matching its tool name.
- Failed/completed/stopped states do not retain a working spinner or active Stop
  button. Local protection commands continue to work during active model turns.
- A disabled action has a visible or keyboard-accessible explanation. Icons have
  accessible names, menus support keyboard navigation, and focus returns to the
  invoking control. Narrow windows preserve composer access.
- Queue submits a follow-up for the next available step, which may belong to the
  active task or a later turn. Steer sends guidance to the active runtime. Both
  controls require the server's turn-input capability and retain the draft until
  acknowledgement; uncertain retries reuse an idempotency key. Local protection
  commands remain separate from model input.
- Changes renders only the guarded tool's returned diff, with file selection,
  line numbers and bounded pages. Truncation is explicit; the viewer never reads
  file contents directly or implies a partial response is complete.
- Brand orange marks primary actions and selection; errors and protection status
  are also expressed in text. Transitions should be brief and respect reduced
  motion. Token streaming must not animate or repeatedly relayout the page.

## Acceptance

Verify Chat, Work, empty workspace, unavailable provider, running/failed task,
pending approval, protected-file policy loading/error/active states, account and
session switches, long paths, and a narrow window. Run Web types/tests/style and
bundle synchronization, native DesktopChecks and the packaged real-engine tests.
Use isolated local fixture accounts for screenshots and interaction checks.

Reference interaction patterns: [Codex review](https://learn.chatgpt.com/docs/code-review),
[Codex permissions](https://learn.chatgpt.com/docs/sandboxing), and
[Claude Code desktop](https://code.claude.com/docs/en/desktop).
