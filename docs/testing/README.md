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

Private source candidates are identified by the exact downstream commit and the
successful `Community RC` run. Private staging does not create candidate tags or
artifact bundles. After publication is enabled and the repository is public,
pushing an existing version tag runs the complete source gate and creates a
GitHub Release with GitHub's generated source archives and release notes. The
workflow never creates tags, changes visibility or uploads precompiled files.

The automatic Community CI includes CodeQL with a public-visibility gate. GitHub Code
Security is not available for private repositories on GitHub Free or Pro, so
the job is intentionally skipped during private staging and starts running
automatically after publication. Maintainers must require its successful check
before accepting external changes once the repository is public.

A private candidate becomes a public source release only after publication is
explicitly enabled by a reviewed contract change and the repository controls
have been validated.
