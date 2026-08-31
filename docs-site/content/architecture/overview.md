---
site: true
slug: architecture
title: Architecture
short_title: Architecture
group: Operate
order: 70
description: Follow the runtime path from model API through the execution service to every client.
keywords:
  - architecture
  - daemon
  - CLI
  - web
  - SQLite
  - runtime
---

# Architecture

## Runtime flow

Opencoding Community uses one local execution service with multiple clients.

```text
Independent model API (OpenAI-compatible)
                  |
                  v
        Local execution service
        | sessions and turns
        | tools and approvals
        | policy and audit
        | SQLite persistence
        | Git and MCP
        |
        +---- opencoding CLI
        `---- Local Web
```

## Service ownership

The local execution service owns session state, turns, model calls, tools,
approvals, audit events, Git integration, MCP and persistence. This makes every
client a view onto one authoritative execution history.

The model API and the Community application remain independently managed
processes. Keeping them separate lets application restarts, model evaluations
and multiple clients use one stable model endpoint.

## Client connection

The service binds loopback by default. It publishes a private runtime connection
file beneath the user's Opencoding runtime directory. The CLI discovers that
file automatically. Local Web exchanges a single-use bootstrap for an HttpOnly,
SameSite=Strict cookie.

Provider credentials belong to the independent model API or a configured
credential handle. Daemon bearer credentials and provider secrets must never be
placed in browser JavaScript, browser storage, URLs, checked-in configuration or
logs.

## Edition boundary

Community contains public protocols that allow separately distributed systems
to compose additional identity and governance capabilities. Those private
systems are not required to build or run the local Community product, and
Community packages must never depend on their implementation.
