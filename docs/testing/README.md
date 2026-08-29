# Testing and release candidates

Run the complete Community validation from the repository root:

```sh
scripts/verify-community.sh
```

Focused entrypoints are documented in [`tests/README.md`](../../tests/README.md).
The release-candidate gate additionally runs the real CLI/daemon/model fixture:

```sh
tests/test-cli-e2e.sh
```

Automatic `ci.yml` feedback is limited to DCO, one Linux source gate and
visibility-gated CodeQL. The expensive cross-platform matrix lives in the
manual-only `rc.yml` workflow. Run it once against the exact reviewed revision
and retain its run ID before creating a candidate.

The private `release.yml` workflow builds all four supported macOS/Linux target
archives, generates an SPDX SBOM and release metadata, produces checksums,
archives third-party licenses, emits in-toto provenance, keyless-signs the
checksum file and uploads workflow artifacts. GitHub-hosted attestations are
also created once the repository is public. Manual private-candidate runs never
create a Git tag, GitHub Release or visibility change. After publication is
enabled and the repository is public, pushing an existing version tag runs the
same build and publishes its verified artifacts as a GitHub Release; the
workflow still never creates tags or changes visibility itself.

The automatic Community CI includes CodeQL with a public-visibility gate. GitHub Code
Security is not available for private repositories on GitHub Free or Pro, so
the job is intentionally skipped during private staging and starts running
automatically after publication. Maintainers must require its successful check
before accepting external changes once the repository is public.

A private candidate is addressed by its full source commit plus workflow run.
It becomes a public release only after publication is explicitly enabled by a
reviewed contract change and all external signing and repository controls have
been validated.
