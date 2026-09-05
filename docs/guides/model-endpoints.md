---
site: true
slug: model-endpoints
title: Model endpoints
short_title: Model endpoints
group: Build with S-Code
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

## Topologies

```text
Direct provider ← credential handle resolved by daemon ← CLI · Local Web

Provider ← credential held by local proxy ← daemon without provider key ← CLI · Local Web
```

Direct-provider setup is simpler, but the daemon process can access the
credential value. An independent loopback proxy creates a narrower boundary:
the proxy owns the provider key and the daemon uses a stable local endpoint
without that key.

## First-use setup

Use the interactive assistant:

```sh
s-code setup
s-code doctor
```

`doctor` sends the configured credential with a bounded `models` readiness
request and verifies that the endpoint catalog is reachable. Some providers
publish that catalog without authenticating the request, so this check confirms
credential-handle presence but does not claim that the provider accepted the
credential. The first real task is the end-to-end authentication check. The
readiness request does not generate model tokens.

Presets are available for OpenRouter, OpenAI, Anthropic, Gemini and a local
OpenAI-compatible endpoint. Custom endpoints can be configured without a
prompt:

```sh
export MODEL_API_KEY='your-key'
s-code setup \
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
cargo build --locked --release -p s-code-api-server
OPENROUTER_API_KEY='your-key' \
  target/release/s-code-api-server
```

The default development endpoint is `http://127.0.0.1:18787/v1`.

## Credential boundary

Provider keys must not enter browser JavaScript, browser storage, URLs,
checked-in configuration, product logs or benchmark artifacts. In direct mode
the local service resolves the configured environment handle when calling the
model; it never exposes the value to either client. In proxy mode the daemon
does not receive the provider key.

Model inference is an external data boundary, not an offline operation. A turn
can send the user prompt, selected code and editor context, relevant transcript
history, selected attachments, tool schemas, and tool or command results to the
configured provider. Provider retention and training behavior are controlled
by that provider's contract and account settings, not by S-Code.
