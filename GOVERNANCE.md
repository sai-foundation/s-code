# Governance

S-Code is maintained in the open through technical review,
documented decisions and reproducible release evidence.

This repository is the sole source authority for Community code, issues, pull
requests, CI and releases. Enterprise products consume immutable Community
versions and may not require private checks or private source in order to merge
a Community contribution.

## Roles

- **Contributors** file issues, review changes and submit signed-off commits.
- **Maintainers** triage issues, review and merge changes, maintain release
  infrastructure and enforce project policies.
- **Security maintainers** receive private vulnerability reports and coordinate
  disclosure. Access is limited to people who need it.
- **Project lead** owns product direction and resolves a documented deadlock
  when maintainers cannot reach rough consensus.

The canonical maintainer assignment is the repository `CODEOWNERS` file and
GitHub access configuration. Becoming a maintainer requires sustained,
constructive public contributions, sound security judgment and approval from
the existing maintainers. Nominations and decisions are recorded publicly.
Inactive access may be removed after notice.

## Decisions

Every change to `main` must go through a pull request, including documentation,
generated files, emergency fixes and changes authored by the repository owner
or a maintainer. Contributors work on separate branches and merge through the
source host's pull-request merge operation. Direct pushes, force pushes and
deletion of `main` are prohibited. Administrators must not bypass this process.
Repository protection must enforce these requirements for administrators too
where the hosting plan supports it; lack of host enforcement does not waive
the contribution policy.

Routine changes use pull-request review and required CI. Maintainers seek rough
consensus and document material tradeoffs. Architecture, compatibility,
licensing, security-boundary and governance changes require an ADR or equivalent
design record.

Routine changes require at least one maintainer approval. Security-boundary,
protocol, licensing, release-contract and governance changes require two
maintainer approvals once two maintainers exist. If consensus cannot be reached,
the maintainers record the alternatives and the project lead makes the
decision. Until a second maintainer is added, the initial maintainer may merge
their own change only after all required checks pass and the rationale and test
evidence are present in the pull request.

Community decisions are made from public evidence. A private Enterprise test
may inform a follow-up proposal, but it cannot be a required or unexplained veto
on a Community pull request. Contributors from the sponsoring company and
outside contributors follow the same public review and CI requirements.

## Releases

Release artifacts must be built from a clean reviewed Community commit by the
checked-in workflow. Community tags, source archives, release notes and public
CI evidence originate in this repository. Maintainers never assemble a release
by copying selected files from a private repository.

Public publication is currently disabled while the repository is prepared for
preview. Private candidates are identified by their full Community commit and
workflow run, not by a private integration revision. Enabling public
publication requires a reviewed change to the release contract and a complete
pre-publication review.

## Conduct and security

All project spaces follow [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). Security
reports follow [`SECURITY.md`](SECURITY.md). Maintainers must disclose conflicts
that could materially affect a decision and recuse themselves when appropriate.

## Changing governance

Governance changes use a pull request with at least seven calendar days for
review once the project has more than one maintainer. Emergency security changes
may merge sooner but must receive retrospective review.
