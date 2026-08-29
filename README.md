# Opencoding Community

Opencoding Community is the Apache-2.0 local execution foundation for an AI
coding agent. It provides the `opencoding` CLI, Local Web, sessions, model
gateway, tools, policy, approval, audit, storage, Git integration, MCP and IDE
clients on one local execution service.

> **Release status:** private release-candidate development. There is no public
> supported release or signed download yet. Staging artifacts are identified by
> a full source commit and workflow run and must not be presented as a public
> release.

Opencoding Community is independent and is not affiliated with, sponsored by,
or endorsed by the OpenCode project or its maintainers.

## Architecture

```text
model provider or local model API
              |
              v
      Community execution service
        |                    |
        v                    v
  opencoding CLI         Local Web
```

The daemon owns sessions, execution, tools, approval, audit and local
persistence. The CLI and Local Web are clients of the same loopback service and
observe the same state. Provider credentials never belong in browser storage,
URLs or checked-in configuration.

See [the architecture overview](docs/architecture/overview.md) for trust and
process boundaries.

## Build and install from source

Prerequisites:

- macOS or Linux;
- Rust 1.89;
- Node.js 22 and npm;
- Python 3, Git, ripgrep (`rg`) and the platform build toolchain.

Clone the repository, then install the Community application:

```sh
git clone https://github.com/shilongliu-iteria/opencoding-community.git
cd opencoding-community
scripts/install-from-source.sh
```

The default destination is `$HOME/.local/bin`. Override it with
`OPENCODING_INSTALL_DIR`. Add that directory to `PATH`, then run:

```sh
opencoding
# or
opencoding web
```

The normal installed command is always `opencoding`. The packaged daemon and
CLI binaries are internal runtime helpers.

## Model endpoint for development

The application expects an OpenAI-compatible model API. The repository contains
an independent development API Server; it is not part of the installed
application. Build and run it in a separate terminal:

```sh
cargo build --locked --release -p opencoding-api-server
OPENROUTER_API_KEY='your-key' \
  target/release/opencoding-api-server
```

Keep credentials in the API Server process environment or an external secret
manager. Never commit them or enter them in Local Web. The default development
endpoint is `http://127.0.0.1:18787/v1`.

## Verify a checkout

Install `cargo-deny` and `cargo-audit`, then run:

```sh
scripts/verify-community.sh
```

The gate checks the generated repository manifest, documentation, formatting,
Clippy, Rust tests, dependency policy, advisories, generated protocol bindings,
Local Web, signed installer behavior and updater rollback.

## Documentation

- [Documentation index](docs/README.md)
- [Configuration](docs/guides/configuration.md)
- [Community deployment](docs/deployment/community.md)
- [Testing and release candidates](docs/testing/README.md)
- [Security policy](SECURITY.md)
- [Contributing](CONTRIBUTING.md)

## License

Opencoding Community is licensed under the
[Apache License, Version 2.0](LICENSE). Contributions require
[Developer Certificate of Origin](DCO) sign-off. Project names and marks follow
[`TRADEMARKS.md`](TRADEMARKS.md).
