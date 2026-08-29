# Governance

Opencoding Community is maintained in the open through technical review,
documented decisions and reproducible release evidence.

## Roles

- **Contributors** file issues, review changes and submit signed-off commits.
- **Maintainers** triage issues, review and merge changes, maintain release
  infrastructure and enforce project policies.
- **Security maintainers** receive private vulnerability reports and coordinate
  disclosure. Access is limited to people who need it.

The canonical maintainer assignment is the repository `CODEOWNERS` file and
GitHub access configuration. Becoming a maintainer requires sustained,
constructive contributions, sound security judgment and approval from the
existing maintainers. Inactive access may be removed after notice.

## Decisions

Routine changes use pull-request review and required CI. Maintainers seek rough
consensus and document material tradeoffs. Architecture, compatibility,
licensing, security-boundary and governance changes require an ADR or equivalent
design record.

If consensus cannot be reached, the maintainers record the alternatives and the
responsible maintainer makes the decision. Until a second maintainer is added,
the initial maintainer may merge their own change only after all required checks
pass and the rationale and test evidence are present in the pull request.

## Releases

Release artifacts must be built from a clean reviewed commit by the checked-in
workflow. The workflow produces archives, an SPDX SBOM, checksums, provenance
and signatures. Maintainers never assemble a release by copying selected local
files.

Public publication is currently disabled. Private staging candidates are
identified by their full source commit and workflow run, not a public release
tag. Enabling public publication requires a reviewed change to the release
contract and a complete pre-publication review.

## Conduct and security

All project spaces follow [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). Security
reports follow [`SECURITY.md`](SECURITY.md). Maintainers must disclose conflicts
that could materially affect a decision and recuse themselves when appropriate.

## Changing governance

Governance changes use a pull request with at least seven calendar days for
review once the project has more than one maintainer. Emergency security changes
may merge sooner but must receive retrospective review.
