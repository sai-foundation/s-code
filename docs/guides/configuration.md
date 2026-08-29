# Configuration

The default application runs locally with SQLite and a loopback listener. Start
with [`config/opencoding.example.toml`](../../config/opencoding.example.toml).

Validate or print the redacted effective daemon configuration:

```sh
cargo run --locked -p opencoding-config -- \
  validate --component daemon --config config/opencoding.example.toml
cargo run --locked -p opencoding-config -- \
  print-effective --component daemon --config config/opencoding.example.toml
```

Configuration precedence is defaults, an explicit configuration file, allowed
development environment variables and explicit command overrides. Production
profiles reject ordinary environment and command overrides. Secret values must
come from explicitly named `secret_references`; effective configuration output
redacts them.

The browser is not a configuration or secret store. Model provider credentials
belong in the independently managed model API process or an external secret
manager.

MCP is disabled by default. When enabled, use absolute executable paths,
bounded arguments and environment-variable handles rather than secret values.
Every MCP tool still enters the ordinary policy, approval and audit path.
