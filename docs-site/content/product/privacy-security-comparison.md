---
site: true
slug: privacy-security-comparison
title: Privacy and security boundaries in S-Code
short_title: Privacy and security
group: Why S-Code
order: 85
description: Understand credential protection, command isolation and guarded edits through concrete scenarios, source code and documented comparisons.
keywords:
  - privacy
  - security
  - sandbox
  - Other Agent A
  - Other Agent B
  - Other Agent C
---

# Privacy and security boundaries in S-Code

## What S-Code protects

S-Code separates **model credentials, browser sessions, tool permissions,
operating-system isolation, concurrent file edits and audit evidence** into
boundaries that can be inspected in the source.

Its commands have an OS-enforced boundary. Other Agent A's published threat
model explicitly distinguishes its permission system from security isolation.
On supported platforms, S-Code fails when its command sandbox is unavailable;
Other Agent C's documented configuration allows an unsandboxed fallback unless
configured otherwise. S-Code and Other Agent B use similar published patterns
for workspace writes, default command network restrictions and OS sandboxing.
S-Code's specific design features include an optional separate model-credential
process, Local Web key isolation, file-version preconditions for edits and
shared events covering tools, approvals, usage and diffs.

> **Historical scope:** S-Code source and public product documentation reviewed
> on 31 August 2026. An undocumented guarantee is not proof that another agent
> lacks a capability. See the [secret-file comparison](../testing/safety-comparison.md)
> for the narrower review updated on 9 September 2026.

## Five concrete scenarios

### 1. A README injection asks the agent to upload an SSH key

Malicious instructions might ask the agent to run `cat ~/.ssh/id_rsa` and
upload the result with `curl`. S-Code's structured commands start without
network access. macOS Seatbelt or Linux Bubblewrap enforces the declared read
and write roots. A model request cannot expand those OS permissions; network
access must pass through policy and approval.

This is not a virtual machine or absolute isolation. It cannot protect every
secret a user explicitly places within the permitted scope. File access,
network access and approval provide separate checks on the attempted action.

### 2. A dependency installer tries to change shell configuration

A project dependency might try to write to `~/.zshrc`, startup files or an
executable directory outside the workspace. S-Code's `run_command` grants the
command tree only its declared write roots. The OS sandbox rejects writes
outside those roots. Default network restrictions also prevent downloading a
second-stage payload unless the user explicitly expands network access.

### 3. A person and the agent edit the same file

`read_file` returns a content digest, and `apply_patch` must supply the matching
version. The runtime checks SHA-256 again before writing and fails if the
content changed. This reduces stale overwrites, but it is not a filesystem
transaction or a cross-process lock: a small race window remains between the
digest check and atomic replacement. Stop concurrent writers and inspect the
final diff when an external editor or generator is changing the same file.

### 4. Browser code tries to obtain a provider key

Local Web receives neither the provider key nor the daemon bearer token in
direct-provider or independent-proxy mode. In direct mode, the daemon can
access the credential value. Only independent-proxy mode keeps the provider
key exclusively in the proxy process.

The authenticated `s-code web` launcher reads the private connection file to
request a single-use bootstrap. Visiting the loopback page alone does not
authorize a session. The page clears the bootstrap URL fragment before
exchanging it for an `HttpOnly; SameSite=Strict` cookie. Long-lived credentials
stay out of browser JavaScript, Local Storage and URLs. Origin checks, CSRF
signals, CSP, framing restrictions and `no-store` response headers provide
additional controls.

### 5. The agent says it tested a change, but evidence is needed

Tool requests, policy decisions, approvals, execution results, model usage and
the final diff belong to one session event chain. A reviewer can inspect actual
commands, outcomes and approvals alongside the agent's summary. These records
make unexpected actions easier to investigate; they do not eliminate risk.

## How each tool is constrained

Different tools use different enforcement boundaries:

| Tool | Current enforcement | Purpose |
| --- | --- | --- |
| `run_command` and its child processes | macOS Seatbelt / Linux Bubblewrap; explicit read/write roots; network off by default | Constrain arbitrary programs and dependency scripts at the OS level |
| `read_file`, `list_files`, `apply_patch` | The daemon opens paths from a stable workspace directory handle, rejecting symlink substitution, parent traversal and sensitive paths; edits also require a SHA-256 precondition | Keep native file operations narrow without launching arbitrary processes |
| Policy, approvals and audit | Operational tool calls pass through policy and produce events | Record authorization separately from OS enforcement |
| Git, MCP and local extensions | Git runs on the host with system/user configuration ignored, hooks/fsmonitor/textconv disabled and executable repository configuration rejected before each operation; approved local MCP and extensions remain host processes | This is not universal OS sandboxing; Preview Team Grant mode disables the actor-scoped MCP runtime |

File-tool authorization and process sandboxing serve different purposes. Git's
known implicit execution paths fail closed, while broader OS isolation for
Git, local MCP and extension hooks remains future work.

## Comparison of published designs

This table retains the historical review date above. Agent labels are
consistent with the [safety comparison](../testing/safety-comparison.md).

| Design point | S-Code | Other Agent A | Other Agent B | Other Agent C |
| --- | --- | --- | --- | --- |
| Local command OS isolation | Built in: macOS Seatbelt / Linux Bubblewrap | Published threat model states no sandbox; permissions provide prompts and visibility | Built-in OS sandbox | Built-in Bash sandbox, enabled through configuration |
| Default command network access | Off; explicit request through policy/approval | Permission rules can ask or deny, without an OS sandbox egress boundary | Off in `workspace-write` | Sandbox must be enabled; then network access is governed by domain; missing sandbox can fall back to unsandboxed execution |
| Sandbox unavailable | Fails without unsandboxed fallback | Not applicable: no built-in sandbox | Selected sandbox mode defines the boundary; dangerous full access is an explicit option | Warns and runs unsandboxed by default; `failIfUnavailable` can require failure |
| Stale file write protection | Read digest plus `apply_patch` version precondition | Equivalent guarantee not stated in cited pages | Equivalent guarantee not stated in cited pages | Equivalent guarantee not stated in cited pages |
| Long-lived keys in Local Web | Provider key and daemon bearer token stay out of browser JS, storage and URLs | Different architecture; public documentation describes local code/context handling and asks users to secure server mode | Different architecture; no direct equivalent | Different architecture; no direct equivalent |
| Native Windows sandbox | Unsupported; command execution fails closed | Runs on Windows, but published threat model still states no agent sandbox | WSL2 and native Windows sandbox support | WSL2 support; no native Windows sandbox |

Other Agent A also emphasizes local operation and its code/context privacy
policy. Other Agent B provides OS sandboxing and broader Windows support.
Other Agent C can provide strong file and network isolation when sandboxing
and fail-closed behavior are configured. This comparison concerns specific
published controls, not a claim that other products are unsafe.

## Design principles

1. **Capabilities and approvals are separate.** Approval records a human decision; the sandbox limits what a process can do.
2. **Start with limited access.** Commands start without networking and receive explicit write roots; timeouts terminate their process groups and wait for direct children.
3. **Separate credentials from the interface.** Model credentials, the execution service and browser sessions have distinct process and authentication boundaries.
4. **Check file versions before edits.** The agent must provide the digest it read; this is optimistic concurrency checking, not a cross-process transaction lock.
5. **Keep evidence inspectable.** Tests, diffs, policy, approvals, usage and audit events can be reviewed together.
6. **Document uncovered cases.** Users need to know when an outer container, VM or manual review is still required.

## Limitations

- macOS and Linux are supported. Native Windows command sandboxing is unavailable, so execution fails closed.
- The command sandbox is not a VM or microVM. It cannot defend against kernel exploits or revoke broad permissions the user deliberately grants.
- Timeouts terminate the command process group, but host processes that deliberately detach into a new session may escape that cleanup. Use an outer container or VM for hostile code.
- Git, local MCP and extension hooks do not all share the same OS isolation path. Git's known configuration execution paths are disabled; Team Grant mode does not load the MCP runtime.
- File reads and edits use capability checks inside the daemon, without launching a separate OS sandbox for each operation.
- Prompts, code snippets and tool results sent to a model remain subject to the configured provider's data policy. Keeping provider keys out of the browser does not keep task data away from the provider.
- Any agent can produce vulnerable code. Execution isolation does not replace code review and real tests.

## Sources and verification

- [S-Code: platform sandbox implementation](https://github.com/sl-7qx/s-code/blob/main/crates/platform-runtime/src/lib.rs)
- [S-Code: tool execution and version preconditions](https://github.com/sl-7qx/s-code/blob/main/crates/execution/src/lib.rs)
- [S-Code: Local Web bootstrap and security headers](https://github.com/sl-7qx/s-code/blob/main/crates/daemon/src/lib.rs)
- [Other Agent A: published threat model](https://github.com/anomalyco/opencode/security)
- [Other Agent A: permission rules](https://opencode.ai/v2/docs/permissions)
- [Other Agent A: privacy information](https://opencode.ai/)
- [Other Agent B: agent approvals and security](https://developers.openai.com/codex/agent-approvals-security)
- [Other Agent C: sandboxing](https://code.claude.com/docs/en/sandboxing)
- [Other Agent C: settings and sandbox defaults](https://code.claude.com/docs/en/configuration)

These are documented designs and defaults at the stated review date. Recheck
primary sources after product updates and validate implementation behavior
with repeatable security tests. This table is not a permanent product ranking.
