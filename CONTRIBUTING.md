# Contributing to S-Code

Thank you for improving S-Code. Contributions are accepted under
the Apache License, Version 2.0, and must preserve the Community/Enterprise
dependency boundary.

Community development happens in this repository. Company maintainers and
outside contributors use the same issue, pull-request, review and CI process.
Private Enterprise checks are not required to contribute here.

## Before opening a change

- Use an issue for a substantial feature, protocol change, new dependency or
  behavior that changes security, compatibility or release artifacts.
- Report vulnerabilities privately through
  [GitHub Security Advisories](https://github.com/sl-7qx/s-code/security/advisories/new),
  never in a public issue.
- Keep Community code independent of private Enterprise packages, services and
  build inputs.
- Design Enterprise-requested execution features as general Community
  capabilities; do not add product-specific backdoors or private-only branches.
- Do not include credentials, customer data, proprietary source or generated
  local state.

## Development

Install Rust 1.89.0 (including rustfmt and Clippy), Python 3.9 or newer,
Node.js 22, npm, Git, curl, Ruby, OpenSSL and the platform C/C++ build tools.
The Rust toolchain is pinned by `rust-toolchain.toml`. On Linux, install
Bubblewrap for command isolation:

```sh
# Debian / Ubuntu (use your distribution's equivalent on other Linux systems)
sudo apt-get install bubblewrap
```

The complete gate also needs these pinned verification tools. Build them with
the stable Rust toolchain; the project itself continues to use Rust 1.89.0:

```sh
rustup toolchain install stable --profile minimal
cargo +stable install --locked cargo-deny --version 0.20.2
cargo +stable install --locked cargo-audit --version 0.22.2
cargo +stable install --locked cargo-about --version 0.9.2 --features cli
```

Ensure Cargo's binary directory (normally `$HOME/.cargo/bin`) is on `PATH`.
From the repository root, run:

```sh
scripts/verify-community.sh
```

The gate installs the locked Web/documentation dependencies, builds the actual
source-installed release in an isolated directory, and exercises its first
run. Use focused [test entrypoints](tests/README.md) while editing. Commit your
changes before running the complete gate: release verification requires a
clean checkout. Tests keep transient state under `.work/` and must not use or
delete a developer's normal S-Code runtime state.

For a downloaded source archive, run the gate from a fresh extraction before
building or installing dependencies there. Archive validation intentionally
rejects generated/integration files; use a Git clone for repeated development
and verification.

IDE clients have additional checks outside the local source gate:

```sh
npm ci --prefix clients/vscode --no-audit --no-fund
npm run check --prefix clients/vscode
npm run test:extension --prefix clients/vscode
```

The VS Code extension-host test needs a graphical session; headless Linux CI
uses `xvfb-run -a npm run test:extension --prefix clients/vscode`.
The [JetBrains client](clients/jetbrains/README.md) requires JDK 17 and Gradle
9.1.0 for its tests, packaging and compatibility verification.

## Pull requests

All changes to `main` must use the pull-request workflow, including maintainer
changes, documentation-only edits and urgent fixes:

1. Create a separate branch from the current `main`.
2. Commit signed-off changes and push that branch.
3. Open a pull request targeting `main`, with review and validation evidence.
4. Complete the reviews and checks required by [Governance](GOVERNANCE.md).
5. Merge using GitHub's pull-request merge operation.

Do not push commits or locally created merges directly to `main`, force-push
it, delete it or use administrator privileges to bypass this workflow.

CI runs basic source checks on every PR and selects additional tests by the
changed area. A README-only edit does not trigger cross-platform compilation;
runtime changes still exercise Linux and macOS. Require the single fixed
`source-gate` status. Full compatibility and dependency checks run weekly and
on manual dispatch; only a successful manual full run can qualify a release.
See [Verification and releases](docs/testing/README.md) for the routing policy.

Each pull request should contain one coherent change and explain:

- the user-visible outcome;
- security and compatibility impact;
- tests run and any test that could not be run;
- documentation or migration work;
- whether AI-assisted output was used and how it was reviewed.

Generated code must be regenerated from its checked-in source of truth. New
dependencies need a clear purpose and must pass the license and advisory gates.
Maintainers may request smaller commits or additional evidence before merging.
If Enterprise needs the change, it adopts the reviewed Community commit only
after this pull request merges; contributors do not need access to Enterprise.

## Developer Certificate of Origin

Every commit must carry a Developer Certificate of Origin 1.1 sign-off:

```sh
git commit -s
```

The sign-off certifies the statements in [`DCO`](DCO). Use a name and email
address you are authorized to associate with the contribution. The project does
not require a separate Contributor License Agreement at this stage.

## License

Unless explicitly and validly identified otherwise, accepted contributions are
licensed under Apache-2.0. Third-party work must retain its required copyright,
license and attribution notices.
