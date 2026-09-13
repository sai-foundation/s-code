---
site: true
slug: code-mode
title: Code Mode
short_title: Code Mode
group: Build with S-Code
order: 65
description: Orchestrate read-only tools with JavaScript while keeping each call visible.
keywords:
  - code mode
  - JavaScript
  - tool orchestration
---

# Code Mode

Code Mode lets the model use one JavaScript program to read several files,
search a workspace, and return only the useful parts of the results. Each tool
still appears in the conversation, with its own status under a **Code Mode**
parent. A caught tool error remains visible even when the program succeeds.

## Enable it

Code Mode is optional and disabled by default. Add this to your S-Code
configuration and restart the service:

```toml
[model]
code_mode = true
```

The model receives an additional `execute` tool in coding, planning and review
sessions. Direct tools stay available. Code Mode works with providers that can
call ordinary JSON tools; it does not require a provider-specific code execution
API. Whether a particular model uses it effectively still needs evaluation.

## What the model sends

The `execute` arguments contain a `code` string. Its body is JavaScript inside an
async function. For example:

```javascript
const files = await Promise.all([
  tools.read_file({ path: "src/main.rs", max_bytes: 12000 }),
  tools.read_file({ path: "Cargo.toml", max_bytes: 12000 }),
]);
text(files);
```

The available functions are `tools.read_file`, `tools.list_files`,
`tools.search_text`, `tools.git_status` and `tools.git_diff`. They use the same
argument objects and result shapes as the direct tools. `text(value)` selects
what returns to the model; otherwise raw child results stay out of the model's
conversation. Rejected tools reject their promises. Always await every call.

Edits, commands, MCP tools and operations requiring approval use direct tool
calls. Code Mode cannot suspend for approval. It returns a direct-call hint and
creates no approval request. The host checks policy before hooks and again
after hooks modify arguments; normal sensitive-path checks still apply.

## Execution limits

Each program runs in a fresh QuickJS process with a 64 MiB JavaScript heap limit,
a 512 KiB JavaScript stack limit, a 30-second deadline, and no filesystem,
network, environment, process, module-loader or timer bindings. Source and
selected output are each limited to 32 KiB. Tool arguments are limited to 64 KiB
and each result transfer to 1 MiB. At most four child tools run concurrently;
there are at most 32 child calls per turn and four active workers per daemon.
These heap limits do not represent a total operating-system process RSS cap.

Stopping a turn cancels the program and prevents further child calls. Blocking
host Git reads may finish under their existing timeout; their late results are
discarded. Installed hooks continue to be trusted host code under their existing
execution rules. A daemon restart marks interrupted programs, children and
associated active turns cancelled. Scripts are never automatically replayed.

## Tool visibility

The host assigns each program and child a stable tool ID. It persists the child
before validation, hooks or execution, then publishes lifecycle events. The
parent ID is stored with the child, so a history page containing only a child
still identifies it as a Code Mode call. Events contain tool names and redacted
display details, rather than file contents or the complete script output.

This follows the parent/child tracing approach used in
[OpenAI programmatic tool calling](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
and [Anthropic programmatic tool calling](https://platform.claude.com/docs/en/agents-and-tools/tool-use/programmatic-tool-calling).
[OpenCode's implementation](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/tool/code-mode.ts)
also reports nested tool start/end activity from its host bridge. S-Code keeps
these lifecycle records in its normal tool history so live UI and refreshed
history share the same identities.

Code Mode can reduce model round trips and returned context for batch reads.
It is not inherently faster for a single tool, and this release does not claim
a measured quality or token advantage across models.
