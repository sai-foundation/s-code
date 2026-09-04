# Contributing to Opencoding Community

Thank you for improving Opencoding Community. Contributions are accepted under
the Apache License, Version 2.0, and must preserve the Community/Enterprise
dependency boundary.

Community development happens in this repository. Company maintainers and
outside contributors use the same issue, pull-request, review and CI process.
Private Enterprise checks are not required to contribute here.

## Before opening a change

- Use an issue for a substantial feature, protocol change, new dependency or
  behavior that changes security, compatibility or release artifacts.
- Report vulnerabilities privately through
  [GitHub Security Advisories](https://github.com/sl-7qx/opencoding-community/security/advisories/new),
  never in a public issue.
- Keep Community code independent of private Enterprise packages, services and
  build inputs.
- Design Enterprise-requested execution features as general Community
  capabilities; do not add product-specific backdoors or private-only branches.
- Do not include credentials, customer data, proprietary source or generated
  local state.

## Development

Install Rust 1.89, Python 3, Node.js 22, npm, Git and the platform build tools.
From the repository root:

```sh
npm ci --prefix web
scripts/verify-community.sh
```

Run a focused test while iterating, then run the complete verification script
before requesting review. Tests keep transient state under `.work/` and must
not use or delete a developer's normal Opencoding runtime state.

## Pull requests

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
