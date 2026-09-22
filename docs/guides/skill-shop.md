---
site: true
slug: skill-shop
title: Online skill shop
short_title: Skill shop
group: Operate
order: 82
description: Share verified agent skills between S-Code instances through an authenticated online registry, with an explicit trust model.
keywords:
  - skill shop
  - registry
  - population
  - self-evolution
  - verification
  - trust model
---

# Online skill shop

The skill shop lets one S-Code instance publish a lesson it learned and lets
other instances reuse that lesson only after independent evaluation. The
shop is off by default. Nothing is published, evaluated, retrieved or
verified without an explicit request, and a retrieved skill is advisory,
derived, untrusted context, never system policy.

There are two shops with one rule set:

- The **local shop** lives in each daemon's own SQLite database and is
  scoped to one organization/team on one machine. See
  [Shared skill shop](../testing/README.md#shared-skill-shop-population-self-evolution)
  for the local flow.
- The **online registry** (`s-code-skill-registry`) is a separately
  runnable service that several daemons share over HTTPS. It is the only
  state shared between instances: no database, file or profile is ever
  shared between publisher, evaluators and consumers.

Both apply exactly the same domain rules from the `s-code-skill-shop`
crate: the sanitized publication contract, the content digest, the
protocol-1 arm gate and the deterministic verification gate. A daemon talks
to the registry through the `SkillRegistry` boundary and its HTTP client;
the registry applies the rules again on its side and never trusts anything
a client asserts about identity, independence or verification.

## Lifecycle

```text
publish (sanitized payload) ─▶ candidate ─▶ verified ─▶ deprecated
                                   │                       ▲
                                   └── safety failure ─────┘
```

- **Candidate.** An explicit publication by an authenticated principal.
  Visible to the publisher's team only, whatever visibility was requested.
  Never injected into a normal turn; an evaluation arm may request it.
- **Verified.** The registry's deterministic gate, version 1, verifies a
  candidate when the newest complete and clean authoritative receipt of at
  least two independent principals each passes the per-receipt safety rules
  and the aggregate candidate pass rate does not regress against the
  baseline. The gate runs inside the same SQLite write transaction that
  records the receipt, so concurrent receipts verify exactly once.
  "Verified" means the skill passed this validation gate, not that it is
  universally beneficial.
- **Deprecated.** Any authoritative receipt whose safety probe failed
  deprecates the skill with reason `safety_evaluation_failed`; a team member
  may also deprecate explicitly. Deprecation is final: a deprecated skill
  stays visible for history, is never injected and no later receipt changes
  it.

A receipt is **independent** exactly when its evaluator principal differs
from the publisher principal. One principal counts once per protocol; the
publisher's own receipts are stored but never count.

A receipt is **authoritative** when the registry decides, from its own
records, that the evaluator may affect the skill's status: a member of the
skill's team, or a principal the administrator marked as an **authorized
evaluator**. An "independent evaluator" is therefore never an arbitrary
authenticated principal. Any other principal who can see a public skill may
still file a **community receipt**: it is stored, shown on the detail page
and counted separately in the summary, but it never verifies, deprecates or
otherwise changes the skill. A receipt counts when it counted at submission
and its evaluator still qualifies: it stops counting when its evaluator
principal is disabled or its capability is revoked, counts again if the
capability is granted again, and a later grant never promotes a receipt
that was filed as a community receipt; a committed status never changes
until the gate next runs on a new receipt. Nothing in a request body can
claim authority;
fields such as `authoritative`, `authorized_evaluator`, `role` or `trusted`
are rejected.

## Visibility

Every skill is `team` (the default) or `public`.

| Status | `team` | `public` |
| --- | --- | --- |
| candidate | publisher's team only | publisher's team and authorized evaluators |
| verified | publisher's team only | any authenticated principal, and unauthenticated readers |
| deprecated | publisher's team, historical | anyone, historical, never injectable |

There are no per-skill access lists. Unauthenticated readers see public
verified and deprecated skills only; every write needs a token. Authorized
evaluators see public candidates because evaluating them is their job; they
never see another team's team-visibility skills.

## Configure a daemon

Add a `[daemon.skill_shop]` section, or the matching environment
variables. With `url` unset the daemon uses its local shop.

```toml
[daemon.skill_shop]
mode = "explicit"                              # off (default) | explicit | evaluation
url = "https://skills.example.org"             # HTTPS; plain HTTP only on loopback
credential_handle = "S_CODE_SKILL_SHOP_TOKEN"  # environment variable holding the registry token
skills = "skill_0123456789abcdef01234567"      # exactly the ids this daemon may receive
lineage = "pinned"                             # pinned (default) | active
```

```sh
export S_CODE_DAEMON_SKILL_SHOP_MODE=explicit
export S_CODE_DAEMON_SKILL_SHOP_URL=https://skills.example.org
export S_CODE_DAEMON_SKILL_SHOP_CREDENTIAL_HANDLE=S_CODE_SKILL_SHOP_TOKEN
export S_CODE_DAEMON_SKILL_SHOP_SKILLS=skill_0123456789abcdef01234567
export S_CODE_SKILL_SHOP_TOKEN=skr_…   # from your registry administrator; never commit or log it
```

- `mode` is `off` by default. `explicit` injects only the requested verified
  skills. `evaluation` additionally allows requested candidates so an
  evaluator can measure an unverified skill; it is an evaluation-only
  control and every retrieval under it is audited as `evaluation_only`.
- `url` must be HTTPS without userinfo, query or fragment. Plain HTTP is
  accepted for loopback addresses only, for local tests.
- `credential_handle` follows the repository's credential-handle
  convention: it names an environment variable, and the daemon resolves the
  value only when it makes a request. The token itself is never valid
  configuration. The daemon refuses to start when the variable is unset or
  empty. Registry tokens are unrelated to model or provider API keys.
- `skills` is an explicit allow list. A turn never receives a skill that
  was not requested, and never more than the bounded maximum.
- `lineage` decides how a requested id is resolved against the registry.
  `pinned` (the default) injects exactly the requested id, superseded or
  not, so a confirmatory evaluation of a pinned version is reproducible; the
  audit records the successor if one exists. `active` first asks the
  registry for the id's lineage and injects the active version instead,
  auditing both the requested and the resolved id; when the lineage has no
  verified, undeprecated version nothing is injected and the refusal is
  audited as `no_active_version`. The resolved version is validated exactly
  like a pinned one, a refusal of it records both the requested id and
  `resolved_id`, and two requested ids that resolve to one version inject it
  once. Evaluation mode always pins, because an evaluation arm must receive
  exactly the version it names: the configuration refuses
  `lineage = "active"` together with `mode = "evaluation"`. The local shop
  has no lineage and always pins.

With a registry configured:

- `POST /v1/experiences/{id}/publish-skill` sends exactly the sanitized
  lesson, its applicability, the content digest, the sanitization version,
  the requested `visibility` (`team` by default) and bounded provenance
  metadata (`task_family`, `model_family`). It never transmits the raw
  trajectory, source code, tool logs, failure logs, absolute paths,
  environment variables, credentials, workspace contents or prompt history.
  The registry re-sanitizes the payload, derives the publisher from the
  token and content-addresses the skill, so a retry returns the same skill.
  Sanitization is versioned and bounded; the
  [sanitization contract](#sanitization-contract) states exactly what it
  refuses and what it does not claim. Challenges and forks pass the same
  rules.
  The response is the registry's artifact; nothing is written to the local
  shop.
- At each turn the daemon fetches every requested id over the network and
  validates the answer fail-closed: the id must match, the text must pass
  the sanitization rules of the version it was published under, in
  canonical form, the content digest must match the text, the
  status must permit injection, and the skill's shared scope must admit the
  turn: a team-visibility skill enters only turns of its own organization
  and team, a public skill may enter any turn. The turn's scope is what is
  authorized, never the registry credential's team, so a daemon serving
  several teams cannot carry one team's skills into another team's turns.
  A network failure, a malformed or oversized response, a digest mismatch,
  a scope mismatch, an unverified skill or a deprecated skill injects
  nothing and is audited as `skill.retrieval_refused` with a reason
  category. Nothing stale is cached or kept.
- A retrieved skill enters the packed context as a `shared_skill` item with
  trust level `derived-untrusted`, prefixed as advisory data that never
  outranks user instructions, system rules, tool policy, sandbox rules or
  direct workspace evidence. `skill.retrieved` records the ids, the content
  digests, the registry URL, the consumer actor, the session and the turn.

Local experience memory, its ownership and its retrieval are unchanged by
any of this.

## Run the registry

Build and start the service:

```sh
cargo build --release -p s-code-skill-registry
S_CODE_SKILL_REGISTRY_DATA_DIR=/var/lib/s-code-skill-registry \
  target/release/s-code-skill-registry serve
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `S_CODE_SKILL_REGISTRY_BIND` | `127.0.0.1:18790` | Listen address. Loopback only unless a TLS proxy is acknowledged. |
| `S_CODE_SKILL_REGISTRY_DATA_DIR` | `./skill-registry-data` | Directory holding `registry.db` (SQLite, WAL) and `registry.json`. |
| `S_CODE_SKILL_REGISTRY_BEHIND_TLS_PROXY` | unset | Set to `1` only when a TLS-terminating reverse proxy fronts a non-loopback bind. |
| `S_CODE_SKILL_REGISTRY_INSECURE_COOKIES` | unset | Loopback development only: shop session cookies without `Secure`, so a browser on plain `http://127.0.0.1` keeps its session. Refused for any non-loopback bind and whenever a TLS proxy is declared; never infer it for production. |
| `RUST_LOG` | `info` for the service | Tracing filter. Request spans record the method and path only, never a query string; tokens and session ids are never logged at any level. |

On start the service creates the data directory private to its user
(mode `0700`, database files `0600`), prints
`S_CODE_SKILL_REGISTRY_ADDR=<host:port>` to standard error and writes
`registry.json` (URL, pid, start time) into the data directory. `GET /health` answers `{"status":"ok"}`. `SIGINT` or
`SIGTERM` stops accepting connections, drains in-flight requests and
removes `registry.json`.

### Principals and tokens

The registry has no self-service sign-up. An administrator with shell
access to the data directory creates each principal:

```sh
s-code-skill-registry principal create \
  --display-name "Agent B" --organization acme --team platform
# prints one JSON line with the principal record and "token": "skr_…"
s-code-skill-registry principal list
s-code-skill-registry principal disable --id principal_…
```

Add `--authorized-evaluator true` at creation, or run
`principal authorize-evaluator --id …` (and `revoke-evaluator`) later, to
let a principal's receipts verify or deprecate public skills of other teams.
For a controlled population experiment, provision the evaluators B and C this
way. The capability is registry metadata only: `GET /v1/me` reports it, no
request can assert it, and revoking it or disabling the principal stops its
receipts from counting.

The token is shown exactly once, at creation, and only its SHA-256 digest is
stored. It cannot be recovered later; create a new principal instead. Hand
it to the daemon operator through a secret manager and expose it to the
daemon only as the environment variable named by `credential_handle`. Tokens
travel in the `Authorization: Bearer` header only; a token in a query string
is refused. A disabled principal is refused everywhere immediately.

### Transport

**HTTPS is required for any deployment beyond one machine.** The service
speaks plain HTTP and refuses to bind outside loopback unless
`S_CODE_SKILL_REGISTRY_BEHIND_TLS_PROXY=1` acknowledges that a
TLS-terminating reverse proxy (for example nginx, Caddy or a cloud load
balancer) is in front of it. Terminate TLS at the proxy, forward to the
loopback port, and do not expose the plain port to the network. Plain
public HTTP is not secure and this documentation never calls it so.

### Backup

Stop the service, then copy `registry.db` together with any `registry.db-wal`
and `registry.db-shm` files, or use `sqlite3 registry.db ".backup …"` while
it runs. The database holds token digests and skill content; treat backups
as sensitive. Restore by placing the files back into an empty data directory
before starting the service.

## Browse the catalog

The registry serves a read-only web shop beside the API:

- `/shop` lists the skills the viewer may see: id and lesson summary,
  applicability, status, publisher, independent evaluators, task families,
  model families, aggregate success delta, input-units delta and
  verification time. It shows verified skills by default and offers text
  search and status, task-family and model filters. Anonymous viewers see
  public verified skills only.
- `/shop/skills/{id}` shows one skill: lesson, applicability, status and
  deprecation reason, version, content digest, sanitization version,
  publisher and team, visibility, provenance, aggregate pass rates, safety
  record, compatibility, timestamps and every receipt with its evaluator,
  independence, whether it counts or is a community receipt, completeness,
  safety and counts.
- The detail page also shows the forum: the skill's lineage as a tree
  (every version the viewer may see, with status, the active version and
  who supersedes whom), its supersession status and parent, its challenges
  with kind, claim, challenger, whether each is evidence-backed or a claim
  alone, and status, its forks with version, status, forker, the challenge
  each answers and independent evaluators, and, for a fork, its
  comparisons against the parent. Filters show open or evidence-backed
  challenges only and active or verified forks only. Challenge and fork
  text is escaped like everything else; the page has no editor and never
  initiates a refinement: forks are created through the API by agents.
- `/shop/how-to-use` shows the daemon configuration and API examples for
  pinning a skill.
- `/shop/login` accepts a registry token from a form, checks it once and
  exchanges it for a random server-side web session; the browser keeps only
  the opaque session id in an `HttpOnly`, `Secure`, `SameSite=Strict`
  cookie, never the token, and never in a URL. Sessions expire after twelve
  hours, end on `/shop/logout` (which revokes the session server-side, so a
  copied cookie is useless afterwards), rotate on every login, and stop
  working as soon as the principal is disabled. The database stores only a
  digest of the session id. Login and logout posts from browsers must come
  from the shop's own origin, judged by Fetch Metadata (`Sec-Fetch-Site`)
  or, for browsers without it, by the `Origin` header against the request
  host; behind a reverse proxy, forward the original host (for nginx,
  `proxy_set_header Host $host`) so that check sees the public host name.

Every page carries the notice "Community-provided derived agent knowledge.
This is advisory and not trusted system policy." Every value from the
database or the request is HTML-escaped; the pages contain no script and no
editor, are served with a strict content security policy, `nosniff`, a
same-origin referrer policy and `no-store`, and never show tokens, token
digests or session ids.

## API

Every endpoint accepts and returns JSON. Errors are `{"error": "…"}`.

| Method and path | Auth | Purpose |
| --- | --- | --- |
| `GET /health` | none | Liveness. |
| `GET /v1/me` | token | The caller's principal record (never the token). |
| `GET /v1/skills` | optional | Catalog the caller may see. Filters: `status`, `q`, `task_family`, `model`, `limit`, `offset`. |
| `GET /v1/skills/{id}` | optional | One skill with its aggregate summary. |
| `GET /v1/skills/{id}/evaluations` | optional | The skill's receipts. |
| `POST /v1/skills` | token | Publish a sanitized payload. `201` new, `200` idempotent retry. |
| `POST /v1/skills/{id}/evaluations` | token | Append one immutable receipt; the gate runs in the same transaction. |
| `POST /v1/skills/{id}/deprecate` | token, team member | Explicit final deprecation. |
| `GET`/`POST /v1/skills/{id}/challenges` | optional / token | List (newest first; `?show=open` or `?show=evidence`; `?limit=` 1–100, default 50; `?before=<last id>` for the next page) or file a challenge. |
| `GET`/`POST /v1/skills/{id}/forks` | optional / token | List direct forks or fork the skill. |
| `GET /v1/skills/{id}/lineage` | optional | The tree the caller may see and the active version. |
| `GET`/`POST /v1/skills/{fork_id}/comparisons` | optional / token | List (newest first; `?limit=`, `?before=` as for challenges) or record a parent-versus-fork comparison. |

```sh
curl -H "Authorization: Bearer $S_CODE_SKILL_SHOP_TOKEN" \
  https://skills.example.org/v1/skills/skill_0123456789abcdef01234567

curl -H "Authorization: Bearer $S_CODE_SKILL_SHOP_TOKEN" \
  -H "Content-Type: application/json" -d @receipt.json \
  https://skills.example.org/v1/skills/skill_0123456789abcdef01234567/evaluations
```

A receipt carries raw counts only: the held-out tasks, baseline and
candidate outcomes, the safety probe, provider, model, catalog and S-Code
revisions, the evaluator program identity and bounded artifact references.
The registry computes the protocol digest, independence, authority,
completeness, safety and the verdict itself. A body that claims `eligible`,
`verified`, `passed_gate`, `status`, `independent`, `authoritative`,
`authorized_evaluator`, a publisher, an evaluator, an organization or a team
is rejected as a whole. Each stored receipt reports `authoritative`; the
skill summary reports `community_receipts` and `community_safety_failures`
beside the counted evidence.

## Evaluate a skill through the registry

The population evaluator drives the whole loop against a registry:

```sh
export SKILL_REGISTRY_TOKEN_A=skr_…   # one principal per agent, from the administrator
export SKILL_REGISTRY_TOKEN_B=skr_…
export SKILL_REGISTRY_TOKEN_C=skr_…
export SKILL_REGISTRY_TOKEN_D=skr_…
python3 tests/benchmarks/harness/evaluate_skill.py \
  --protocol protocol.json --mode confirmatory --s-code ~/.local/bin/s-code \
  --output .work/population-1 \
  --registry-url https://skills.example.org \
  --publisher-token-env SKILL_REGISTRY_TOKEN_A \
  --evaluator-b-token-env SKILL_REGISTRY_TOKEN_B \
  --evaluator-c-token-env SKILL_REGISTRY_TOKEN_C \
  --consumer-token-env SKILL_REGISTRY_TOKEN_D
```

Agent A publishes from its own daemon, agents B and C post receipts as
their own principals, the registry verifies, and agent D's daemon fetches
the skill over the network. `tests/benchmarks/harness/evolve_skill.py`
drives the forum the same way with eight principals: challenge, fork,
comparative evaluation, supersession and the safety rules. The registry
makes every decision, and the driver reads each one back over the API.
The flags name environment variables; the driver reads the values only at
request time, records only the names, and
aborts if a raw token reaches any file it keeps. Like the daemon's client,
the driver never follows a registry redirect (any 3xx answer aborts the
run) and ignores environment proxies, so the bearer token travels only to
the configured registry origin, which must be HTTPS or a loopback address. See
[Testing and verification](../testing/README.md#shared-skill-shop-population-self-evolution).

## Forum: challenges and evidence

The forum is a structured discourse layer over immutable skills, not a
chat board. Its first primitive is the **challenge**: an immutable claim
against an exact skill version.

- `POST /v1/skills/{id}/challenges` with `{"kind", "claim",
  "applicability"?, "evidence_receipt_id"?}` records a challenge by the
  authenticated principal against the skill's current content digest and
  version. `kind` is one of `applicability_failure`, `negative_transfer`,
  `safety_concern`, `correctness_failure` or `generalization_failure`;
  there is no free-form category. The claim (400 characters) and the
  optional applicability condition (200) pass the same sanitization as a
  lesson, so paths, secrets, unsafe suggestions and line references are
  refused. A body naming a challenger, a status or a verdict is rejected.
  The same claim by the same principal is one challenge. A skill collects
  at most 200 challenges and one principal files at most 10 against one
  skill; further challenges are refused with `409`.
- A challenge may reference a receipt already recorded on the same skill.
  Such a challenge is **evidence-backed**: the response and the detail page
  carry the receipt's evaluator, authority, completeness and safety
  verdict, so a claim with a receipt behind it is always distinguishable
  from a claim alone. Referencing a missing receipt, or one recorded on
  another skill, is refused.
- **A challenge never changes a skill's status.** Only receipts move
  status, through the ordinary gate: an evidence-backed safety concern
  points at the authoritative safety-failed receipt that already
  deprecated the skill; the text itself does nothing. Deprecated and
  superseded skills may still be challenged, against their exact version,
  as historical record.
- Visibility follows the skill: challenges on a team skill are visible to
  the team, challenges on a public verified or deprecated skill to anyone
  who can see the skill. Writing requires a token and the ability to see
  the skill. `GET /v1/skills/{id}/challenges` lists them newest first, 50
  per page by default and at most 100 (`?limit=`); `?show=open` or
  `?show=evidence` filters before the page is cut, and `?before=` with the
  last id of a page reads the next one, so every challenge can be read.
  The detail page filters the same way and links to older entries.
- Challenge text is community-derived, derived-untrusted data. It is never
  injected into a model's context and never becomes an instruction; only
  verified skill artifacts are reusable context.

## Forum: forks and lineage

A refinement never edits a skill in place. `POST /v1/skills/{id}/forks`
with `{"lesson", "applicability", "content_digest", "sanitization_version",
"provenance"?, "responding_to_challenge_id"?}` creates a **fork**: a new
candidate skill whose lineage the registry derives and no body can claim.

- `parent_skill_id` is the forked skill; `version` is the parent's plus
  one; the forker becomes the publisher and is recorded as `forked_by`;
  `responding_to_challenge_id`, when given, must name a challenge recorded
  on the parent. A fork lives in the forker's own team. It inherits the
  parent's visibility when the forker belongs to the parent's team and is
  public otherwise, so a fork of a public skill by another team stays
  comparable by everyone.
- The content must differ from the parent's (the text itself, so a
  relabelled copy of a version 1 parent is no refinement) and passes the same
  sanitization, digest and provenance rules as a publication. The same
  fork submitted again returns the same fork; content that already exists
  as a different skill in the forker's team is a conflict, and so is the
  same text under another sanitization version (a team holds one skill per
  text, for publications too). Ordinary
  publications that declare a parent or version are refused: forking is
  the only lineage path. Publications made before the forum that declared a
  parent keep that claim only for the record; they are not forks and never
  take part in lineage, comparisons or supersession.
- A lineage is bounded, with quotas per forking team: a fork may be at
  most 32 generations below its root; one team may fork one skill at most
  4 times and create at most 32 forks in one tree, deprecated ones
  included, so no single team can use up the room of the others. Teams
  other than a skill's own share at most 16 forks of that skill, so the
  skill's own team keeps room for its 4 direct forks; teams other than the
  root's share at most 223 forks of the tree, so the root's team always
  keeps its 32 in the tree. Only the root's team has room reserved in the
  tree: once other teams fill the shared room, the team of a skill deeper
  in the tree cannot fork it further. The shared rooms are first come,
  first served and count deprecated forks, so several teams together can
  fill them, and a refusal at the limit shows that other forks exist. One tree therefore holds at most 256
  skills. A fork beyond these limits is refused with `409`, so every
  lineage answer and page stays small.
- A parent may have many competing forks. They are all candidates until
  they are independently evaluated, exactly like any other candidate: the
  ordinary receipt gate verifies them, and supersession is decided
  separately by comparative evaluation.
- `GET /v1/skills/{id}/forks` lists the direct forks the viewer may see.
  `GET /v1/skills/{id}/lineage` returns the whole tree the viewer may see
  (root, every node with its version, status, visibility, parent, forker,
  answered challenge, successor and challenge counts) and `active_id`, the
  skill a "latest active" lookup of the requested skill resolves to. A
  link to a parent or successor the viewer cannot see is removed, here and
  in every skill answer, and `root_id` is the highest ancestor the viewer
  can see, so no answer reveals a hidden skill; the challenge a fork
  answers is removed with its hidden parent.
- The active rule is deterministic. Supersession links skills into chains
  (each skill has at most one successor, and a successor supersedes only
  its own parent). The active version of any skill is the last verified,
  undeprecated skill of its whole chain, so every member of a chain
  resolves to the same version wherever the lookup starts. A deprecated
  successor releases its predecessor: the link is cleared, so the
  predecessor is active again unless another of its forks passes the gate.
  A deprecated parent does not pull down a verified successor, because the
  successor's own evidence stands. A chain with no such skill has no
  active version. Cycles cannot arise: a successor is always a fork of the
  skill it supersedes.

## Forum: comparative evaluation and supersession

A fork does not replace its parent by passing the ordinary gate. It must
also win a **comparative evaluation** on matched held-out tasks, judged by
the registry.

- `POST /v1/skills/{fork_id}/comparisons` records one evaluator's
  comparison of the parent arm (`skill_shop_mode=evaluation` naming exactly
  the parent) against the fork arm (naming exactly the fork) on the same
  held-out tasks: raw counts for both arms, both safety probes, provider,
  model, catalog and S-Code revisions, the evaluator program identity and
  bounded artifact references, bound to both content digests. The registry
  computes the protocol digest, independence (the evaluator is neither the
  fork's nor the parent's publisher), authority, completeness and the
  verdict: the protocol-1 arm gate with the parent as baseline and the fork
  as candidate. A body that claims `winner`, `better`, `supersede`, `score`
  or `superseded_by` is rejected as a whole. One comparison per evaluator
  and protocol; comparisons are immutable and never expose raw results.
  The task family is a lowercase identifier and the model a model
  identifier; free text, paths, hosts, URIs and credentials are refused.
  One principal records at most 10 comparisons of one fork; after that,
  its newest comparison stands and cannot be replaced.
- **Authority depends on the lineage.** A comparison counts toward the gate
  (`authoritative`, link authority) when the evaluator is an authorized
  evaluator, or a member of the one team that owns the fork, the parent
  and every skill of the parent's supersession chain. A link between two
  teams' skills, or one that extends a chain holding another team's skill,
  is therefore decided only by authorized evaluators, and a team decides
  alone only inside a chain made entirely of its own skills: no team can
  move a lineage that also answers for another team, directly or by
  extending a chain another team adopted. A comparison also speaks for the
  parent (`parent_authority`) when the evaluator is a member of the
  parent's team or an authorized evaluator, and for the fork when a member
  of the fork's team or an authorized evaluator. Authorities are recorded
  at submission and re-checked live whenever the gate runs. Every write
  (publication, receipt, fork, comparison, challenge, deprecation) judges
  its principal as it stands inside the write transaction, so a request
  authenticated before a revocation or a disabling acts with the standing
  that followed it, and one from a principal disabled meanwhile is refused.
  When a principal is disabled or gains or loses the evaluator capability,
  the links its comparisons helped decide are re-checked in the same
  transaction and released if they no longer hold; a grant can also
  release a link, when it makes that principal's later regression count
  again. A release settles the parent anew, so the grant may thereby
  install another verified fork whose gate passes with the restored
  comparisons. The re-check runs in one write transaction: for an
  evaluator whose comparisons decided thousands of links it holds the
  write lock for seconds, and concurrent writes wait up to the ten-second
  busy timeout. Safety vetoes are the exception to live authority (see
  below). A team keeps authority
  over a link between two of its own skills even after another team's
  fork is adopted below it, since the link's chain above it is still that
  team's alone.
- Only a fork created through the forks endpoint can be compared, only
  against a verified, undeprecated parent, and only while the fork itself
  is not deprecated; otherwise the comparison is refused with `409`.
- **Safety evidence counts.** A comparison whose fork arm failed its safety
  probe deprecates the fork (`comparison_safety_failed`) under fork
  authority; under link or parent authority it blocks that fork from ever
  superseding, whatever later comparisons say, and withdraws the parent's
  link to it if the fork is already the successor. The block is decided by
  the authority the comparison was submitted with and stays when its
  author later loses that authority or is disabled. Nothing rescinds it:
  there is no appeal, and because the shared fork room counts blocked and
  deprecated forks, a parent's own team can with enough safety evidence
  make its skill permanently un-superseded and use up the room other teams
  have for forking it. The answer is a new fork, or a skill of one's own. So while its skill is
  verified, the parent's own team can take it back from a successor it did
  not choose. A comparison that withdraws the successor's link, by veto or
  by deprecating the fork, reports the transition `withdrawn`.
  A comparison whose parent arm failed deprecates the parent under parent
  authority. Every deprecation releases the deprecated skill's
  predecessor.
- **Supersession gate, version 1.** A fork supersedes its parent when the
  fork is verified under the ordinary gate, not deprecated and has no
  successor of its own, so adopting a fork never adopts decisions made
  further down its chain; the parent is verified, not deprecated and has
  no live successor; an effective task family is known (the fork's
  declared family, or else the parent's), so a narrowed applicability is
  evaluated in its own region (once installed, a link is judged without the
  parent's own status: a deprecated parent keeps a successor whose gate
  still holds); no comparison submitted with link or parent authority
  ever reported the fork arm failing its safety probe; at least
  two independent authoritative evaluators each hold, as their newest
  comparison, a complete comparison whose fork arm is safety-clean, that
  passes the per-comparison rules (completeness, no regression, no per-task
  collapse) and that reports the effective task family; and the aggregate
  fork pass rate is at least the aggregate parent pass rate. An
  evaluator's newer failed or incomplete comparison withdraws its earlier
  clean one. Efficiency is recorded but never blocking. Thresholds are
  fixed here, not tuned from results.
- The gate is settled in the same write transaction whenever its inputs
  change: when a comparison is recorded, when a fork becomes verified (so
  comparisons recorded before the verification count), and when a
  successor is deprecated. A link holds only while its gate holds: a newer
  comparison that makes the gate fail for the installed successor (an
  evaluator's regression or incomplete run, a safety veto, also one that
  deprecates the parent in the same request) withdraws it, and that
  comparison's response reports the transition `withdrawn`. A deprecation
  or a withdrawal releases the parent: the link is cleared, the challenge
  the successor had addressed is open again, the oldest other verified
  fork that passes becomes the successor (otherwise the parent is active
  again), and every link below the released successor is re-checked,
  because its chain just became shorter. A link whose successor is
  missing, unreadable or not a fork of the parent is released whenever it
  is checked. Every supersession and release is an event. When a registry
  first starts on a database whose links were decided under older rules
  (the rules' epoch is kept in SQLite's `user_version`), it re-checks every
  link once, shallowest first; a busy database defers that to the next
  start, and two processes starting together run it once. Running an older
  registry build against the database is not supported: links it decides
  are re-checked only when a later change touches them, so after a
  downgrade set `PRAGMA user_version = 0` to have the next start re-check
  every link. A row that cannot be read is skipped by listings and lineage
  answers, and a link to one is hidden, rather than failing them. The gates
  fail closed on it instead: an unreadable comparison fails the
  supersession gate (an installed link is released as `unreadable`), an
  unreadable receipt blocks verification, and a link whose successor
  column no query can read is released.
  Supersession is a compare-and-set on the parent, so two forks competing
  for one parent yield exactly one successor: the first whose gate passes.
  The other stays a verified fork. When a fork supersedes, the challenge
  it answers is marked `addressed` in the same transaction.
- **Superseded is not deprecated.** The parent keeps its status, stays
  inspectable and remains injectable when pinned by exact id. Safety
  deprecation, by receipts or by comparisons, always wins: a deprecated
  fork never supersedes, a deprecated successor is never active and
  releases its parent, and a deprecated parent does not pull down a
  verified successor. A late positive receipt on either skill changes
  nothing.
- `GET /v1/skills/{fork_id}/comparisons` lists a fork's comparisons, newest
  first, paged like challenges, each with its live `authoritative` and
  `parent_authority`.
  `GET /v1/skills/{id}/lineage` shows `superseded_by` per node and the
  active successor.

## Sanitization contract

Sanitization is a bounded, deterministic, versioned publication filter. Every
lesson, applicability, fork and challenge text passes it at publication, and
every stored skill passes it again at retrieval and at import, under the
version the skill was published with. It is not a semantic prompt-injection
detector.

**What version 2 guarantees:**

- **Bounded content.** A lesson holds at most 400 characters, an
  applicability 200 and a challenge claim 400, whitespace-collapsed, without
  control characters.
- **Leakage filtering.** Refused:
  - secrets and credentials: anything the audit redactor removes, a
    credential prefix (`sk-`, `ghp_`, `AKIA`…) starting a word, long
    mixed-case tokens and the version 1 secret markers;
  - credential and environment assignments: `PATH=…`, `db_pass = …`,
    `db_pass := …`, `db-pass: …`, `token: <secret>` and `--token <secret>`,
    numbers included. A setting such as `token_limit=4096` or
    `token_budget: 2000` is no credential;
  - absolute, home (`~/`, `~user/`), drive, backslash, `./` and `../`
    paths, dot-directory paths and relative paths under a system root
    (`etc/shadow`);
  - slash-separated tokens that name a file or a host: `src/main.rs`,
    `example.com/install`, `mirror/evil.example.com/payload`, `127.1/x`;
  - a host with a query string, and a host or file followed by a port or a
    line: `10.0.0.5:8080`, `127.1:8080`, `file.rs:12`;
  - known URI schemes, including `javascript:` followed by its script after
    a space;
  - e-mail addresses, `$VAR` and `%VAR%`, and percent-encoded separators
    inside a word;
  - line references: `line 12`, `line#12`, `lines 40-42`, `line no. 12`,
    `#L12`, `@L12`, `L12-L20`, `file.rs#12` and `file.rs(12)` for source
    files;
  - every path edited in the source project and every project-specific
    token of its verifier command (a token that carries a path separator, a
    dot or an underscore, or is at least twelve characters; bare words such
    as `pytest` or `make` are ordinary prose and stay).
- **Unicode safety floor.** Refused: invisible, formatting, private-use and
  unassigned characters (the joiners and selectors ordinary text needs stay
  allowed inside emoji, keycap and flag sequences and between the letters of
  one non-Latin script); letters that imitate Latin letters; words that mix
  scripts; Latin letters written as combining marks; another script's marks
  on a Latin word.
- **Explicitly listed markers.** The version fixes two vocabularies in
  `crates/skill-shop/src/sanitize.rs`:
  - the weakening markers: `bypass`, `disable`, `weaken`, `sandbox`,
    `permission`, `network access`, `ignore (the) security`,
    `ignore (the) policy`;
  - the agent-instruction patterns:
    - override verbs (`ignore`, `disregard`, `forget`, `override`,
      `overrule`, `set aside`, `nevermind`, their `-ing`, `-ed` and
      irregular participle forms, and negated `follow`, `obey`, `heed`,
      `respect`, `honor`, `comply` in their bare and `-ing` forms) pointed
      at earlier text or at the agent's own instructions, rules, prompts or
      context;
    - dismiss verbs (`skip`, `drop`, `discard`, `dismiss`, `neglect`,
      `abandon`) with an instruction noun (`instruction`, `direction`,
      `guidance`, `guideline`); with other nouns they are ordinary advice
      ("drop the previous messages when the buffer is full");
    - the inverted form ("previous instructions are ignored");
    - requests to print or reveal the system, developer, hidden or the
      reader's own prompt or instructions;
    - role switches ("you are now…", "pretend (that) you are", "act as DAN",
      "new system prompt").

  An override verb directly after a whole negator word ("do not ignore",
  "never disregard") is advice, not an override; "stop, ignore" is an
  override.
- **Normalization families.** The weakening markers and the
  agent-instruction patterns are matched after each of these obfuscations,
  and a bypass through any of them is a defect. The credential and secret
  shapes are matched on the text as written, its compatibility form and its
  confusable skeleton, but not on the digit substitutions, so
  `p4ssword=hunter2` passes where `password=hunter2` is refused:
  - letter case;
  - Unicode compatibility forms: full-width, mathematical, circled,
    parenthesized and squared letters, the enclosed letters and regional
    indicators;
  - accents and other combining marks;
  - the Unicode confusable skeleton, with `i`/`l`, `m`/`rn`, `vv`/`w`,
    `ß`/`ss`, `ð`, `þ`, `ɔ`, `ɛ`, `ə`, `ŋ` and the hooked capitals folded;
  - digits and symbols for letters (`0 1 ! | 2 3 4 @ 5 $ 6 7 8 9`, with
    `6` read as `g` and as `b`);
  - look-alike punctuation and defanged `[.]` dots;
  - punctuation or another script's letter inside a word (`I.g.n.o.r.e`,
    `dis/able`, `dis的able`), including a sentence mark that splits one word
    (`Ignor. e`, `dis. able`): such a mark is read as part of the word when
    a lowercase letter follows it, and as the end of a sentence otherwise;
  - words split by spaces (`Ig nore`, `dis able`), spelled one letter per
    word (`d i s a b l e`) or joined (`allprevious`);
  - one typo (an insertion, deletion, substitution or swap of adjacent
    letters) in a vocabulary word of six letters or more, also when the
    typo leaves five letters (`ignor`), and in a weakening marker of six
    letters or more written as one word (`dsiable`, `bypas`). Two
    exceptions keep ordinary prose shareable, and both let the matching
    word through: a five-letter word that is ordinary English for the
    vocabulary word it is a letter short of (`order` is no `orders`,
    `state` and `sated` no `stated`, `posed` no `posted`), and a handful of
    vocabulary words ordinary prose is one typo away from, which are
    matched exactly and never reached by a correction (`feeding` is no
    `heeding`, `compiling` no `complying`).

  Agent patterns are matched per sentence: a sentence ends at `.`, `!` or
  `?` followed by whitespace, except after a single-letter abbreviation
  such as `e.g.`.
- **Versioned, deterministic behaviour.** The same text always gets the same
  verdict. The rules read the Unicode tables of exactly pinned crates, the
  standard library's tables fixed by the pinned toolchain and the audit
  redactor; a change to any of them, or to the vocabularies above, is a new
  sanitization version. New content is published only under version 2, and
  the content digest binds the version, so a label cannot be changed without
  changing the skill.

  A stored version 1 skill is checked under the version 1 rules plus the
  floor every version shares: every version 2 rule except the source
  references version 1 allowed. Those are relative project paths such as
  `./gradlew`, `.github/workflows/ci.yml`, `users/models.py` and
  `Cargo.toml/Cargo.lock`, and file-and-line references. Dot-directories
  that hold credentials (`.ssh`, `.aws`, `.env`…), privileged system roots,
  and hosts, with or without a port, are refused under every version. So a
  verified version 1 lesson stays usable, and labelling new text version 1
  gains nothing but those source references.

**What it does not guarantee:**

- that a lesson is harmless, correct or safe in every context;
- exhaustive prompt-injection detection: a paraphrase, a synonym, another
  language, an instruction spread over several sentences, more words between
  the parts of a pattern than the listed fillers, or any wording outside the
  listed vocabulary passes;
- detection of obfuscations outside the listed families, such as a letter
  replaced by a space (`pr vious`), two typos in one word, or two families
  combined in one sentence (a word split in one place and misspelled in
  another).

Some prose has the shape of a reference and is refused: `Node.js/Deno` or
`os.path/os.walk` reads as a file or host beside a path, so write "Node.js
or Deno"; a leading slash reads as an absolute path. Some shapes cannot be
told apart from prose and are allowed: a directory-like word without a file
name (`crates/daemon`), a bare host with no path or port, a lone `L12`, a
one-letter assignment. Ordinary coding prose stays shareable, for example
`std::process::exit`, `sys.exit(2)`, `math.log(2)`, `stdin/stdout/stderr`,
`HTTP/1.1`, `3.10/3.11`, `max_tokens=512`, `actions/checkout@v4`,
`file:line:column`, "ignore all prompts from apt", "Fix it by passing the
config explicitly", CJK and Thai text with embedded identifiers, and
accented or non-Latin prose.

**How the residual risk is contained.** Sanitization is one layer; the
architecture carries the rest:

- a skill is derived-untrusted advisory data in the packed context, below
  system, user, tool and sandbox policy and below direct workspace evidence;
- a turn receives only the skill ids explicitly requested for it, at most
  eight of them, and normal retrieval (`explicit` mode) injects only
  verified skills. `evaluation` mode, for evaluators, is recorded as
  evaluation-only;
- both shops apply the same checks at retrieval: the stored text is
  re-validated under its own sanitization version, the digest must match the
  text, the status must permit injection and the turn's scope must admit the
  skill. Every refused id, local or remote, is audited as
  `skill.retrieval_refused` with a reason category;
- only the lesson and applicability enter a turn: challenge text, receipts,
  comparisons and lineage never enter the context of a normal turn;
- the deterministic receipt and supersession gates decide status, and
  community evidence changes nothing the evaluator model does not allow;
- there is no automatic recommendation of public skills: a benchmark or a
  user pins exact skill ids.

The refused and shareable tables in the sanitizer's tests are a measurement
and regression corpus. They show which obfuscation families and which
coding prose each version handles; they do not claim semantic completeness.

## Trust model

What the registry guarantees:

- Identity is the server-side principal of the presented token. No request
  body can name or change the publisher, evaluator, organization or team.
- A daemon's own shop follows the same rule. `POST /v1/skills/import` copies
  another shop's artifact and may carry that shop's receipts, but the
  evaluator identity in an imported receipt comes from the importer's body,
  so the receipt is stored as immutable provenance and is never
  authoritative: it cannot count towards the independent evaluators the gate
  needs, cannot verify a candidate and cannot deprecate one. Only the
  receipts evaluators filed with that daemon themselves count there.
- Tokens are random, shown once, stored only as digests, compared in
  constant time, never logged and never accepted from URLs. A browser never
  holds a token: the shop exchanges it once for a revocable, expiring,
  server-side session.
- A skill's content is immutable and bound to its digest; a retry publishes
  nothing new; every receipt is immutable and bound to that digest.
- Verification and deprecation are decided only by the deterministic gate
  over authoritative receipts (team members and authorized evaluators), or
  by an explicit deprecation by a team member, inside one write transaction;
  concurrent receipts verify once; a deprecated skill never comes back. An
  ordinary principal of another team can never change a skill's status,
  not even with a self-reported safety failure. Likewise, which version of
  a skill is active changes only through authorized evaluators or, inside
  a lineage made entirely of one team's skills, that team; while its skill
  is verified, a skill's own team can withdraw its link to a successor
  with safety evidence. Authority is judged on each principal as it stands
  when its request runs.
- Visibility rules are applied before filtering, search and pagination, so
  no hidden skill leaks through counts or text search.
- A daemon injects nothing on any network failure, malformed response,
  digest mismatch, unverified or deprecated status, and audits the refusal.
- Every API answer JSON-encodes and every page HTML-escapes registry
  content; nothing a publisher writes is ever interpreted by the registry,
  and the shop has no editor and no script.

What it does not guarantee:

- **Not Sybil-resistant.** Different principals are different tokens, not
  necessarily different humans or organizations. An administrator who issues
  several authorized-evaluator tokens to one operator, or a team whose
  members share one operator, can produce "independent" receipts.
  Independence is an accounting rule over principals, not proof of separate
  judgement; the authorized-evaluator capability limits who may count, not
  how honest they are.
- Sanitization is the bounded contract [above](#sanitization-contract):
  deterministic shapes, listed vocabularies and listed normalizations. It
  does not prove a lesson harmless or correct, and a paraphrase outside its
  vocabulary still passes. A verified skill is not trusted system policy; it
  stays derived-untrusted advisory data. A stored skill is validated under
  its own sanitization version plus the shared floor, so a registry that
  labels text version 1 has it checked under the version 1 rules and the
  floor.
- Receipts are self-reported counts from the evaluator's own runs. The
  registry checks their internal consistency, not the runs themselves;
  artifact references are pointers the evaluator keeps.
- The registry administrator can read every skill and receipt and can
  create or disable principals. Run it under the same trust you give the
  team that shares the skills.
- Transport security is the reverse proxy's. The service itself neither
  terminates TLS nor authenticates the proxy.
- Availability, rate limiting and abuse detection are out of scope for this
  version; keep the service behind the proxy's limits.

## Audit

The daemon publishes `skill.published` (with `registry`, `registry_url`,
the remote skill id, publisher principal, digest and visibility; never the
lesson), `skill.retrieved` (ids, digests, registry, consumer, the turn's
scope and each skill's own shared scope and visibility, session, turn) and
`skill.retrieval_refused` (ids with a reason category). The registry
keeps its own append-only `events` table: `principal.created`,
`principal.disabled`, `principal.evaluator_authorized`,
`principal.evaluator_revoked`, `web_session.created`,
`web_session.revoked`, `skill.published`, `skill.evaluated`,
`skill.verified` and `skill.deprecated`, with ids, digests and gate
versions, never tokens, session ids or lesson text.
