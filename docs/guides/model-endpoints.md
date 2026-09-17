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

On first interactive launch, or with `s-code setup`, a three-step guide lets
you choose a provider, paste a hidden API key, and filter the provider's model
list. Local Web has the same guide on first connection and under
**Settings → Connect a provider**. SAI appears first, followed by OpenAI,
Claude, Gemini, DeepSeek, OpenRouter, local and custom endpoints. OpenRouter's
public catalog is paired with a separate authenticated key check. Discovery
never sends a generation request or spends model tokens.

SAI's current offers come from `https://api.sai.foundation/api/public/promotions`.
The server controls amounts, terms, links and campaign dates. Expired offers,
invalid responses and unavailable campaigns are hidden; client releases do not
contain a default credit amount. Offer retrieval sends no model API key.

Interactive connections are stored beside the selected configuration file as
`config.provider-credentials.json`, with permissions `0600` on macOS/Linux.
New directories are `0700`; existing shared directory permissions are preserved.
This is a private local file, not OS keychain encryption. It must not be committed
or shared. The effective configuration contains only the endpoint and model,
never the saved key. The local daemon picks up saved connections without a
manual restart; a running model response keeps its existing connection.
Managed Team Grant services cannot use the local setup API. Explicit
`S_CODE_MODEL_PROVIDER`, `S_CODE_MODEL_BASE_URL` or
`S_CODE_MODEL_CREDENTIAL_HANDLE` environment settings take precedence and disable
guided setup until they are unset. This prevents saving a connection that the
service would ignore.

Scripted setup continues to support environment-variable handles and replaces
any previous interactive connection. Custom endpoints can be configured without
a prompt:

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

The S-Code repository includes a development API Server that exposes an
OpenAI-compatible `/v1` API on loopback. It is not installed as part of the
S-Code application.

```sh
cargo build --locked --release -p s-code-api-server
OPENROUTER_API_KEY='your-key' \
  target/release/s-code-api-server
```

The default development endpoint is `http://127.0.0.1:18787/v1`.

## Credential boundary

Provider keys must not enter browser storage, URLs, checked-in configuration,
product logs or benchmark artifacts. Web setup holds the entered key only in
its dialog and sends it to the authenticated local service in a request body;
closing setup clears the field. The service never returns saved keys to the
browser. File tools and sandboxed commands deny the credentials-file suffix.
In direct mode the service resolves either a saved local connection or the
configured environment handle when calling the model. In proxy mode the daemon
does not receive the provider key.

Model inference is an external data boundary, not an offline operation. A turn
can send the user prompt, selected code and editor context, relevant transcript
history, selected attachments, tool schemas, and tool or command results to the
configured provider. Provider retention and training behavior are controlled
by that provider's contract and account settings, not by S-Code.
