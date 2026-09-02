---
site: true
slug: core-concepts
title: Core concepts
short_title: Core concepts
group: Start here
order: 30
description: Learn how sessions, turns, tools, policy and evidence fit together.
keywords:
  - sessions
  - turns
  - tools
  - approval
  - audit
---

# Core concepts

## Session model

A session owns the durable conversation and workspace identity. Each user
request creates a turn inside that session. CLI and Local Web project the same
typed transcript instead of maintaining separate client histories.

## Agent loop

The loop prepares context, calls the selected model, schedules tool calls,
records results and continues until the task completes, pauses for approval,
fails with a structured reason or is cancelled. Independent reads may run
concurrently; conflicting workspace writes remain ordered.

## Policy path

```text
Model request → Tool preflight → Policy decision → Approval when required → Sandboxed execution
```

Every tool call passes through the same policy and approval path. Extensions
cannot bypass it merely because they came from MCP, a plugin or another client.

## Evidence

Completed work should carry observable evidence: the test command that ran,
its result, the final diff, model usage and any approval or audit events. The
harness optimizes for frozen checks passing before comparing latency or token
use.

Developer Preview event delivery is durable once an audit event has committed,
but a business mutation and its corresponding event are not yet one database
transaction. A process crash in that narrow interval can leave a snapshot
change without its live notification; CLI and Local Web reconcile from the
authoritative Session snapshot after reconnect. Exactly-once mutation-to-event
delivery requires the planned transactional outbox and is not a Preview claim.
