# Security policy

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use the repository's
[private vulnerability report](https://github.com/shilongliu-iteria/opencoding-community/security/advisories/new).
Include the affected commit or version, reproduction steps, impact and any
suggested mitigation. Do not include real credentials, customer source or
production data.

The project aims to acknowledge a report within two business days and provide
an initial severity assessment within five business days. Target remediation
windows are 7 days for critical, 30 days for high, 90 days for medium and 180
days for low severity, subject to coordinated disclosure and safe deployment.

If GitHub private vulnerability reporting is unavailable, do not disclose the
issue publicly. Open a content-free issue asking a maintainer to enable the
private reporting channel.

## Supported versions

There is no public supported version yet. Invited private-RC testers receive
fixes only on the latest staging commit. After the first public release, the
latest release line will be supported unless the release notes state otherwise.

## Release security

Release candidates must pass dependency, license, secret, source-boundary,
build, test, installer and updater checks. Public artifacts, when enabled, must
include an SPDX SBOM, checksums, provenance and keyless signature material. The
installer has no unsigned fallback.
