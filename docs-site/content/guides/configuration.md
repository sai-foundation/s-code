---
site: true
slug: configuration
title: Configuration
short_title: Configuration
group: Build with S-Code
order: 40
description: Configure the local service without placing credentials in browser or checked-in state.
keywords:
  - toml
  - settings
  - environment
  - secrets
  - SQLite
---

# Configuration

## Configuration file

The default application runs locally with SQLite and a loopback listener. Start
with [`config/s-code.example.toml`](../../config/s-code.example.toml).

For an installed application, the normal first-use path is:

```sh
s-code setup
s-code doctor
```

This creates a private `~/.s-code/config.toml` and changes only model
settings when updating an existing valid file.

Validate or print the redacted effective daemon configuration:

```sh
cargo run --locked -p s-code-config -- \
  validate --component daemon --config config/s-code.example.toml
cargo run --locked -p s-code-config -- \
  print-effective --component daemon --config config/s-code.example.toml
```

## Precedence

Configuration precedence is defaults, an explicit configuration file, allowed
development environment variables and explicit command overrides. Production
profiles reject ordinary environment and command overrides. Secret values must
come from explicitly named `secret_references`; effective configuration output
redacts them.

## Secrets

The browser is not a configuration or secret store. In direct-provider mode,
the local daemon resolves the configured environment handle and can access that
credential value. In independent-proxy mode, the proxy process owns the
provider credential and the daemon connects without it. Use an external secret
manager where appropriate in either topology.

## Local state

The default database is `~/.s-code/state/s-code.db`, not the current
working directory. A fresh file automatically receives a private managed key
at `~/.s-code/state/.s-code.db.storage-key`; sensitive fields are
stored as AES-256-GCM envelopes. Existing non-empty plaintext databases are
detected and never silently rewritten. `s-code doctor` reports their
legacy status and fails before they are treated as suitable for sensitive
work.

Managed encryption limits disclosure when a database file is copied alone; it
does not protect against an attacker who already controls the same operating-
system account and can read the complete state directory. Use full-disk
encryption and normal account security for that threat.

## MCP

MCP is disabled by default. When enabled, use absolute executable paths,
bounded arguments and environment-variable handles rather than secret values.
Every MCP tool still enters the ordinary policy, approval and audit path.

The Preview loads actor-scoped MCP runtimes only in the local
`development_token` authentication mode. In `team_grant` mode the daemon fails
closed: MCP runtime capabilities are unavailable and stored, configured,
Plugin-provided and OAuth-backed MCP servers are not connected. Deploy separate
local daemons for actors who need MCP until the shared runtime can route every
registry and authorization provider by full Organization, Team and Actor scope.
