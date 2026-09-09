# Changelog

All notable changes to S-Code are recorded here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases
use [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first planned public source release is `v0.1.0-preview.1` for macOS and
Linux. It is a source-only Developer Preview.

### Changed

- Renamed the project to S-Code across the CLI, Local Web, IDE clients,
  packages, configuration, documentation and repository links. The command is
  `s-code`, environment variables use `S_CODE_*`, and local state uses
  `~/.s-code`.
- This prepublication rename requires a fresh profile. Old Opencoding Community
  databases and backups cannot be reused, including previous candidates with
  the same Preview version. Preserve the old state and binary for access to old
  history; see the [transition instructions](docs/deployment/community.md#moving-from-opencoding-community).

### Added

- Apache-2.0 local coding agent with the `s-code` CLI and Local Web.
- Local sessions, model gateway, tools, policy, approval, audit, storage, Git,
  and MCP; VS Code and JetBrains sources are experimental fixtures outside the
  supported Preview client surface.
- Reproducible source installation and release-candidate evidence bound to an
  exact reviewed S-Code revision.
- First-use model setup, secure daemon autostart, managed local-state
  encryption and diagnostic checks.
- Frozen benchmark fixtures and methodology, plus executable privacy/security
  use cases.
- A benchmark run collector that measures an S-Code binary on a frozen task
  and records the grader outcome, provider-reported usage and timing.
