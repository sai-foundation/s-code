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

The private `release.yml` workflow builds all four supported macOS/Linux target
archives, generates an SPDX SBOM and release metadata, produces checksums,
archives third-party licenses, emits in-toto provenance, keyless-signs the
checksum file and uploads workflow artifacts. GitHub-hosted attestations are
also created once the repository is public. Manual private-candidate runs never
create a Git tag, GitHub Release or visibility change. After publication is
enabled and the repository is public, pushing an existing version tag runs the
same build and publishes its verified artifacts as a GitHub Release; the
workflow still never creates tags or changes visibility itself.

A private candidate is addressed by its full source commit plus workflow run.
It becomes a public release only after publication is explicitly enabled by a
reviewed contract change and all external signing and repository controls have
been validated.
