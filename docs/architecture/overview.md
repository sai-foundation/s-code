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

S-Code uses one local execution service with multiple clients.

```text
Model endpoint (direct or independent proxy)
                  |
                  v
        Local execution service
        | sessions and turns
        | tools and approvals
        | policy and audit
        | SQLite persistence
        | Git and MCP
        |
        +---- s-code CLI
        `---- Local Web
```

## Service ownership

The local execution service owns session state, turns, model calls, tools,
approvals, audit events, Git integration, MCP and persistence. This makes every
client a view onto one authoritative execution history.

Direct-provider mode connects the execution service to the configured remote
endpoint. Independent-proxy mode instead keeps a separately managed local model
API in front of the provider, which lets application restarts and model
evaluations reuse one stable loopback endpoint without giving the provider key
to the daemon.

## Client connection

The service binds loopback by default. It publishes a private runtime connection
file beneath the user's S-Code runtime directory. The CLI discovers that
file automatically. Only the authenticated `s-code web` launcher can mint
a single-use bootstrap; Local Web erases its URL fragment and exchanges it for
an HttpOnly, SameSite=Strict cookie.

In direct-provider mode the daemon resolves the configured credential handle;
in independent-proxy mode the model API alone owns the provider key. Daemon
bearer credentials and provider secrets must never be
placed in browser JavaScript, browser storage, URLs, checked-in configuration or
logs.

## Edition boundary

Community contains public protocols that allow separately distributed systems
to compose additional identity and governance capabilities. Those private
systems are not required to build or run the local Community product, and
Community packages must never depend on their implementation.
