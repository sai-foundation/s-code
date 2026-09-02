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
Model API → Community execution service → CLI · Local Web
```

The model API and the Community application are independently managed
processes. Keeping them separate makes application restarts, model evaluation
and multiple clients use the same stable endpoint.

## First-use setup

Use the interactive assistant:

```sh
opencoding setup
opencoding doctor
```

Presets are available for OpenRouter, OpenAI, Anthropic, Gemini and a local
OpenAI-compatible endpoint. Custom endpoints can be configured without a
prompt:

```sh
export MODEL_API_KEY='your-key'
opencoding setup \
  --provider openai-compatible \
  --base-url https://models.example/v1 \
  --credential-handle MODEL_API_KEY \
  --model provider/model \
  --yes
```

Remote endpoints must use HTTPS; plain HTTP is accepted only for loopback.
Configuration stores `MODEL_API_KEY` as a handle, not the value. If a running
service predates a configuration change, restart it so the new endpoint and
environment are loaded.

## Development server

The Community repository includes a development API Server that exposes an
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
checked-in configuration, product logs or benchmark artifacts. The local
service resolves a configured environment handle only when calling the model;
it never exposes the value to either client.
