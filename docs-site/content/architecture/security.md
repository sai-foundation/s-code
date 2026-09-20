---
site: true
slug: security
title: Security model
short_title: Security model
group: Operate
order: 80
description: Review the local trust boundary, browser credential handling and fail-closed execution rules.
keywords:
  - security
  - credentials
  - browser
  - cookie
  - network
  - sandbox
---

# Security model

## Trust boundary

The default service is local and loopback-bound. In direct-provider mode the
daemon resolves a named environment handle and therefore can access the
provider credential value. In independent-proxy mode the proxy owns that value
and the daemon has no provider key. Workspace tools are constrained to the
authorized root, sensitive paths are denied and policy decisions are made
before execution.

## Browser session

Local Web never receives the daemon bearer token. An
unauthenticated request to the loopback homepage cannot mint a browser session.
The `s-code web` launcher reads the private connection file and
authenticates to mint a one-time bootstrap; another authenticated local client
may do the same. Only that short-lived value is passed in a URL
fragment. The page removes the fragment before its first network request and
exchanges the bootstrap for an HttpOnly, SameSite=Strict cookie. The daemon
bearer token stays out of browser JavaScript, storage and URLs.

Guided Web setup temporarily holds the provider key in a password input and
JavaScript request bodies. It sends the key through the authenticated local
setup API; the daemon validates it against the selected provider endpoint and
saves it in a private `0600` credential file. Closing or completing setup clears
the input. The key is never placed in Web Storage or URLs, and setup responses
do not return the saved key. This file is not OS-keychain encryption. Browser
extensions or code controlling the page can observe a key entered into Web setup;
use terminal setup or an independently configured proxy to avoid that boundary.
Environment-managed and Team Grant installations do not expose guided setup.

## External data boundaries

The selected model provider can receive prompts, selected code/editor context,
relevant transcript history, selected attachments, tool schemas and tool or
command results. Optional remote MCP servers and configured connectors receive
the arguments or content required for their explicitly configured actions.
Their retention is governed by those external services. Local-first means the
execution, policy, approvals and persistence are local; it does not mean every
model or integration payload stays on the machine.

## Execution boundaries

- Commands use structured executable and argument fields rather than an
  implicit shell.
- Network access is disabled unless it is explicitly required and approved.
- Workspace-write and browser-test profiles retain explicit write roots.
- Timeouts terminate the command process group and wait for the direct child
  and output drains. A deliberately
  detached host process can escape that group, so untrusted work should also
  use an outer container or VM when complete machine isolation is required.
- External tool calls enter local policy and approval. Installing a local MCP
  server or Hook, or starting a background terminal, separately grants that
  host executable the signed-in account's ambient filesystem and network
  authority; the permission preview discloses this boundary. Approval is bound
  to the command specification plus the executable, direct shebang interpreter
  and absolute file-argument identities and is revalidated before reuse.
  Transitive files loaded by trusted host code are not frozen by this mechanism.
- Git runs as a hardened host process outside the command sandbox. System and
  user Git configuration is ignored, executable local configuration is rejected
  before every operation, and hooks, fsmonitor, external diff/text conversion,
  recursive submodules, signing and automatic maintenance are disabled. The
  authorized workspace must be the repository root, so Git cannot inspect a
  parent repository outside a selected subdirectory. The Preview does not
  expose automatic push to the Agent.
- Actor-scoped MCP runtime loading is available only in local development-token
  mode. Team Grant mode advertises MCP runtime capabilities as unavailable and
  does not load stdio, HTTP, Plugin-provided or OAuth-backed MCP registries. This
  fail-closed Preview boundary prevents one actor from reaching another actor's
  MCP resources or credential provider while per-actor runtime routing is built.
- A timed-out, malformed or cancelled local stdio MCP request kills its entire
  process group, waits for the direct process asynchronously when cancellation
  interrupts the request, permanently invalidates the connection and never
  consumes a later response. Remote HTTP MCP cancellation cannot revoke work
  that the remote service already accepted; treat that as an external-service
  residual risk.

## Local state

Fresh installations keep the SQLite database outside the repository under
`~/.s-code/state`. Sensitive transcript, attachment, installed-extension
and local audit payloads are protected with AES-256-GCM and a generated 32-byte
managed key; the directory, database and key use private permissions and reject
unsafe symlink targets. Operational IDs, timestamps, statuses and indexes
remain plaintext so the daemon can schedule and filter work. Team planning
fields also remain plaintext: goal/task titles, outcome and PR URLs, evidence
and blockers, ownership URIs and on-call labels, knowledge titles/source URIs,
model labels and budgets. The database and key are separate files, so a
database copied alone does not disclose encrypted content, but it can disclose
those operational and planning fields.

This is a local-at-rest boundary, not a claim to resist compromise of the
signed-in operating-system account. A process able to read the entire state
directory can obtain both files. Use full-disk encryption, protected backups
and a secure user account as the outer boundary.

## Audit

Tool requests, policy decisions, approvals, execution outcomes and usage are
projected as typed session events. Sensitive content is bounded and protected
separately from metadata needed for review and replay.

The Preview event cursor is a daemon-global monotonic sequence. Actor-filtered
streams never return another actor's event content or identity, but gaps can
reveal that other local activity occurred and approximately when. Deploy a
separate daemon per trust boundary when that metadata matters; opaque
principal-scoped cursors are planned beyond the Preview.
