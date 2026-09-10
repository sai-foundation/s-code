---
site: true
slug: safety-comparison
title: Safety comparison and evidence
short_title: Safety comparison
group: Verify
order: 92
description: Compare secret-file protections in local coding agents, with primary sources and reproducible S-Code tests.
keywords:
  - safety
  - sandbox
  - secrets
  - comparison
---

# Safety comparison and evidence

S-Code ships sensitive-path rules for its built-in file tools and sandboxed
commands. The concrete benefit is that a task cannot switch from a file read
to `cat .env.production` to obtain that file's contents through those tools.

## What is being compared

Other Agent A, B and C are consistent labels across the safety and efficiency
comparisons. Primary-source links remain available for verification.

Reviewed **9 September 2026**. This is a comparison of documented local CLI
controls on macOS/Linux, including the configuration needed to protect
credential files. It is not a vulnerability ranking or a head-to-head attack
benchmark. Cloud environments, administrator policies, containers and custom
permission rules can materially change the comparison.

| Protection | S-Code | Other Agent A | Other Agent B | Other Agent C |
| --- | --- | --- | --- | --- |
| OS isolation for commands | Built into the supported command runtime | Use an external container or VM [5] | Built into local sandbox modes [1] | Built-in Bash sandbox, enabled through configuration or `/sandbox` [3] |
| Sensitive files through command execution | Built-in known-sensitive-path denies, including `.env.production` | Read-tool `.env` denies are separate from Bash permissions; no built-in OS sandbox [4, 5] | Configure filesystem `deny` entries [2] | Configure sandbox credential/file read rules; no built-in credential deny list [3] |
| File reads | Known-sensitive-path rules also apply to reads and edits | `.env` and `.env.*` denied by default in the read tool; example files allowed [4] | File-specific sandbox rules can deny reads [2] | File-tool permission rules are separately configurable [6] |

Other Agent B already provides OS isolation. Other Agent C supports OS-enforced file
restrictions and configurable credential masking. Other Agent A already protects
`.env` in its read tool. S-Code's narrower advantage in this comparison is
shipping sensitive-path denies across its built-in file and command tools,
without asking users to write a project deny list first.

### Primary sources

1. [Other Agent B: agent approvals and security](https://learn.chatgpt.com/docs/agent-approvals-security). Local sandboxing and defaults; users can deliberately grant broader access.
2. [Other Agent B: filesystem permission profiles](https://learn.chatgpt.com/docs/permissions#filesystem-permissions). Explicit path/glob deny rules, inheritance, and platform caveats. Permission profiles are documented as beta; older sandbox configuration takes precedence when present.
3. [Other Agent C: sandbox configuration and credential protection](https://code.claude.com/docs/en/sandboxing#protect-credentials). Credential deny/mask settings apply to configured sandboxed commands; the document states that there is no built-in credential deny list. It also describes enabling the sandbox and configuring stricter fallback behavior.
4. [Other Agent A: permission defaults](https://opencode.ai/docs/permissions/#defaults). The `read` deny rules cover environment files; most permissions otherwise start as allow.
5. [Other Agent A: security model](https://github.com/anomalyco/opencode/blob/b6914b39db86e196ebcc95e92a0188cdf58ef67a/SECURITY.md#no-sandbox), pinned to the source revision reviewed. Its permission system is not OS isolation; the project recommends a container or VM for that boundary.
6. [Other Agent C: permission rules](https://code.claude.com/docs/en/permissions). Tool permissions and OS sandbox restrictions are separate layers.

## What we actually ran

S-Code runtime revision: `4b33aa4c3d129aad63561e8a3a99cf8f41af0e15`.
Local platform: macOS arm64. The runs below passed on 9 September 2026.
Competitor behavior was checked against official documentation and source,
not a live attack run. No comparative pass rate is claimed for any competitor.

| S-Code behavior check | Result |
| --- | --- |
| Ten existing privacy/security scenarios | 10/10 passed |
| Command reads of `.env.production`, nested `.env` and credential-bearing Git config; rename/link attempts | Passed |

Run the same checks from a source checkout:

```sh
tests/test-privacy-security-use-cases.sh
cargo test --locked -p s-code-tool-runtime \
  tests::commands_cannot_read_sensitive_workspace_files -- --exact --nocapture
```

The [scenario manifest](../../tests/cases/privacy-security-use-cases.jsonl)
maps every scenario to its platform-specific behavior test. It includes
ordinary workspace writes succeeding while external writes fail, sensitive
file and symlink denial, browser session controls, private storage and timeout
cleanup. The additional [command-read test](../../crates/tool-runtime/src/lib.rs)
uses synthetic secrets and verifies that file contents are not returned.
These exercise real tool/runtime enforcement, without a model call.

The runner selects Seatbelt tests on macOS and Bubblewrap tests on Linux.
This local run establishes the macOS result; it does not substitute for a
Linux run or an independent security audit.

## Boundaries of this claim

Sensitive-path protection recognizes known names and patterns. It cannot
identify every secret stored under an arbitrary filename, and `.env.example`
files intentionally remain usable. OS command writes are limited to the
authorized workspace plus explicit scratch roots.

Approved local MCP servers, hooks and background terminals run as trusted
host code outside this command sandbox. Git has a separate hardened execution
path. External model providers still receive the context supplied to them.
Local encryption does not protect against compromise of the same OS account.
See the [security model](../architecture/security.md) for the complete boundary.
