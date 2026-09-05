<div align="center">

<img src="assets/s-code-mark.svg" width="64" height="64" alt="" />

# S-Code

**Private by default. Efficient by design.**

A local-first coding agent for your terminal and browser.

<p>
  <a href="LICENSE"><img alt="Apache 2.0 license" src="https://img.shields.io/badge/license-Apache--2.0-31865b?style=flat-square&amp;labelColor=26332b"></a>
  <a href="docs/deployment/community.md"><img alt="macOS and Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-31865b?style=flat-square&amp;labelColor=26332b"></a>
  <a href="docs/deployment/preview-release.md"><img alt="Source-only Developer Preview" src="https://img.shields.io/badge/status-source%20preview-31865b?style=flat-square&amp;labelColor=26332b"></a>
</p>

[**Get started**](#get-started) · [Documentation](https://s-code-docs.shilong86.chatgpt.site) · [Security](#privacy-and-control) · [Benchmarks](#efficiency-and-evidence)

</div>

> **Developer Preview:** `v0.1.0-preview.1` candidate development. The repository
> remains private; there is no supported public release yet. Candidates are
> identified by an exact Community commit and workflow run, not a public release.

## Get started

Requires macOS or Linux, Rust 1.89, Node.js 22, npm, Python 3, Git and the platform
build toolchain. Linux also requires Bubblewrap (`sudo apt install bubblewrap`,
`sudo dnf install bubblewrap`, or `sudo pacman -S bubblewrap`).

**1. Install from source**

```sh
git clone https://github.com/sl-7qx/s-code.git
cd s-code
scripts/install-from-source.sh
```

Installs into `$HOME/.local/bin`; make sure it is on `PATH`.

**2. Connect your model**

```sh
s-code setup
s-code doctor
```

Choose OpenRouter, OpenAI, Anthropic, Gemini, a local model or a custom
OpenAI-compatible endpoint. Setup stores the credential's environment-variable
handle, never the provider secret.

**3. Start coding**

```sh
s-code       # Terminal
s-code web   # Browser
```

Both interfaces use the same local sessions, tools, approvals and events.
The CLI starts the loopback service automatically.

<details>
<summary><strong>Setup checks and credentials</strong></summary>

`setup` supports OpenRouter, OpenAI, Anthropic, Gemini, local and custom
OpenAI-compatible endpoints. It stores only the environment-variable handle,
never the provider secret. `doctor` verifies the local service, encrypted
storage, credential-handle availability and bounded model-catalog reachability
before the first task. Because some providers expose a public model catalog,
only the first real task can prove that a provider accepted the credential.

</details>

<details>
<summary><strong>Installation options, updates and moving from Opencoding Community</strong></summary>

Set `S_CODE_INSTALL_DIR` to choose a different installation directory.
`s-code` is the public command; packaged CLI and daemon helpers are internal
implementation details.

Community currently publishes no precompiled archive, binary installer or
automatic updater. To update, stop S-Code, pull a reviewed revision or
version tag, run `scripts/install-from-source.sh`, then start `s-code`
again. The installer never kills active work; if you installed while the old
service was still active, run `s-code restart`.

**Moving from Opencoding Community:** S-Code starts a fresh installation and
profile, including when the old candidate used `v0.1.0-preview.1`. Stop the old
daemon using its original launcher, preserve `~/.opencoding` and the old binary
if you need its history, then run `s-code setup` to create `~/.s-code`.
Do not reuse old databases, backups or configuration with S-Code: encryption
identifiers, database migrations and client namespaces changed. The rename
leaves your old state untouched; no in-place migration is provided.

</details>

## Why S-Code

| Capability | What it gives you |
| --- | --- |
| **Local execution** | One service for the CLI and Local Web, with shared sessions and state. |
| **Controlled tools** | OS sandboxing, scoped writes and network access disabled for tool commands by default. |
| **Your model endpoint** | Connect a hosted or local provider without replacing the coding harness. |
| **Less repeated work** | Precise file operations, bounded context and one shared agent loop. |
| **Checkable results** | Tests, diffs, approvals and audit evidence that you can inspect. |

## Privacy and control

Tool commands start without network access. Model requests go to the endpoint
you configure, which may be external. Provider keys stay out of browser
JavaScript, browser storage and URLs.

[Read the security architecture →](docs/architecture/security.md)

<details>
<summary><strong>The six enforced controls</strong></summary>

- **Command sandbox:** macOS Seatbelt and Linux sandbox profiles enforce the
  selected read-only or workspace-write boundary.
- **Network off:** tool commands start without network access; enabling it is a
  separately governed capability.
- **Secret protection:** well-known sensitive workspace paths, parent traversal
  and high-confidence credential-shaped process output are blocked or redacted.
- **Browser isolation:** Provider keys and the daemon bearer token never enter
  browser JavaScript, Web Storage, or URLs. Only authenticated local clients
  (the installed launcher or experimental IDE client) may mint a single-use
  browser bootstrap. It hands
  that value through a URL fragment, which the page erases immediately before
  exchanging it for an HttpOnly, SameSite=Strict cookie.
- **Private local state:** fresh installs keep state under
  `~/.s-code/state`, use private filesystem permissions and encrypt
  sensitive transcript, attachment, extension and audit payloads with a locally
  generated managed key. Operational indexes remain plaintext.
- **Protected audit:** local audit payloads and transcript content are encrypted;
  content-free metadata can be exported separately for verification.

</details>

<details>
<summary><strong>Why the execution boundary matters</strong></summary>

This is a stronger built-in boundary than a permission-dialog-only workflow.
OpenCode's own [security policy](https://github.com/anomalyco/opencode/security)
states that its agent is not sandboxed and recommends Docker or a VM when
isolation is required. S-Code makes isolation part of the normal local
execution path and backs the controls with executable privacy and security
tests.

</details>

## One local execution plane

`CLI + Local Web` → `Local execution service` → `Your model endpoint`

The local service owns sessions, tools, policy, approvals, audit and persistence.
The browser uses those same sessions and events.

<details>
<summary><strong>Process and credential boundaries</strong></summary>

The execution service owns sessions, Agent execution, tools, approvals, audit
and local persistence. In direct-provider mode the daemon resolves the named
environment handle and sends the request, so the daemon process can access that
credential value. With an independent local model proxy, only the proxy holds
the provider key and the daemon needs no provider secret. Both modes keep the
browser thin and outside the provider-credential boundary.

Read the [architecture overview](docs/architecture/overview.md) for the process
and trust boundaries.

</details>

## Efficiency and evidence

S-Code includes frozen algorithm, repository and frontend tasks with repeatable
outcome checks. The Preview does not publish a universal speed or token ranking.

[Read the benchmark method →](docs/testing/README.md#coding-harness-benchmarks)

<details>
<summary><strong>How the agent reduces repeated work</strong></summary>

- **Precise file operations** reduce malformed patches, stale writes and
  recovery turns.
- **Bounded history** compacts older payloads without discarding recent
  evidence.
- **No hidden model work** means ephemeral runs do not make a second provider
  request just to generate a title.
- **Fast local approvals** avoid unnecessary round trips while retaining an audit
  decision.
- **One execution plane** keeps the CLI and Local Web on the same sessions and
  events.

</details>

<details>
<summary><strong>What a publishable comparison must include</strong></summary>

Community ships frozen algorithm, repository and frontend tasks with repeatable
trusted-workspace outcome checks. A comparison is publishable only when the exact public source
revision, raw run artifacts, model route, harness configuration and passing
check result can all be reproduced. These local checks are not an adversarial
anti-cheat boundary, and the preview does not publish a universal
speed or token ranking. See the [benchmark method](docs/testing/README.md#coding-harness-benchmarks)
and run the graders against the harnesses and models you care about.

</details>

<details>
<summary><strong>Run the complete local verification</strong></summary>

Install `cargo-deny`, `cargo-audit` and `cargo-about` using the pinned commands
in [contributor setup](CONTRIBUTING.md#development), then run the complete
Community gate from a clean checkout (or a fresh source-archive extraction):

```sh
scripts/verify-community.sh
```

The gate validates the repository manifest, docs, formatting, Clippy, Rust
tests, dependency policy, advisories, generated protocol bindings, Local Web,
the documentation site and the source-installation contract.

</details>

## Model endpoints

Use `s-code setup` for normal installation. The repository also includes an
independent development API Server for a separate proxy process.

<details>
<summary><strong>Provider configuration and the development API Server</strong></summary>

Connect a provider without replacing the coding harness:

```sh
export OPENROUTER_API_KEY='your-key'
s-code setup --provider openrouter --model z-ai/glm-5.3 --yes
s-code doctor
```

The repository also contains an independent development API Server; it is not
part of the installed application:

```sh
cargo build --locked --release -p s-code-api-server
OPENROUTER_API_KEY='your-key' \
  target/release/s-code-api-server
```

When using the independent API Server, keep credentials in that process
environment or an external secret manager. In direct-provider mode, export the
credential handle to the daemon's environment. Never commit a value or enter it
in Local Web. The default development proxy endpoint is
`http://127.0.0.1:18787/v1`.

</details>

## Explore

| Resource | Start here for |
| --- | --- |
| [Product documentation](https://s-code-docs.shilong86.chatgpt.site) | Guides and architecture reference |
| [Security architecture](docs/architecture/security.md) | Sandbox, credentials, browser and audit boundaries |
| [Tools and permissions](docs/guides/tools-permissions.md) | What the agent may do |
| [Model endpoints](docs/guides/model-endpoints.md) | Provider and local model setup |
| [Benchmark method](docs/testing/README.md#coding-harness-benchmarks) | Frozen tasks, outcome graders and publication rules |
| [Contributing](CONTRIBUTING.md) | Development workflow and DCO requirements |

## Open foundation

S-Code is independent and is not affiliated with, sponsored by,
or endorsed by the OpenCode project or its maintainers.

The source is licensed under the [Apache License, Version 2.0](LICENSE).
Contributions require [Developer Certificate of Origin](DCO) sign-off. Project
names and marks follow [`TRADEMARKS.md`](TRADEMARKS.md).
