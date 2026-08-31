---
site: true
slug: model-endpoints
title: Model endpoints
short_title: Model endpoints
group: Build with Opencoding
order: 50
description: Connect the execution service to an independently managed OpenAI-compatible model API.
keywords:
  - model
  - OpenAI compatible
  - API server
  - OpenRouter
  - credential
---

# Model endpoints

## Topology

```text
OpenAI-compatible model API → Community execution service → CLI · Local Web
```

The model API and the Community application are independently managed
processes. Keeping them separate makes application restarts, model evaluation
and multiple clients use the same stable endpoint.

## Development server

The integration repository includes a development API Server that exposes an
OpenAI-compatible `/v1` API on loopback. It is not installed as part of the
Community application.

```sh
cargo build --locked --release -p opencoding-api-server
OPENROUTER_API_KEY='your-key' \
  target/release/opencoding-api-server
```

The default development endpoint is `http://127.0.0.1:18787/v1`.

## Credential boundary

Provider keys must not enter browser JavaScript, browser storage, URLs,
checked-in configuration, product logs or benchmark artifacts. The execution
service owns sessions, Agent execution, tools, approvals and persistence; it
does not need to expose the provider key to either client.
