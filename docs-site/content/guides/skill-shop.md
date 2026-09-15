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
  candidate when the newest complete and clean receipt of at least two
  independent principals each passes the per-receipt safety rules and the
  aggregate candidate pass rate does not regress against the baseline. The
  gate runs inside the same SQLite write transaction that records the
  receipt, so concurrent receipts verify exactly once. "Verified" means the
  skill passed this validation gate, not that it is universally beneficial.
- **Deprecated.** Any receipt whose safety probe failed deprecates the skill
  with reason `safety_evaluation_failed`; a team member may also deprecate
  explicitly. Deprecation is final: a deprecated skill stays visible for
  history, is never injected and no later receipt changes it.

A receipt is **independent** exactly when its evaluator principal differs
from the publisher principal. One principal counts once per protocol; the
publisher's own receipts are stored but never count.

## Visibility

Every skill is `team` (the default) or `public`.

| Status | `team` | `public` |
| --- | --- | --- |
| candidate | publisher's team only | publisher's team only |
| verified | publisher's team only | any authenticated principal, and unauthenticated readers |
| deprecated | publisher's team, historical | anyone, historical, never injectable |

There are no per-skill access lists. Unauthenticated readers see public
verified and deprecated skills only; every write needs a token.

## Configure a daemon

Add a `[daemon.skill_shop]` section, or the matching environment
variables. With `url` unset the daemon uses its local shop.

```toml
[daemon.skill_shop]
mode = "explicit"                              # off (default) | explicit | evaluation
url = "https://skills.example.org"             # HTTPS; plain HTTP only on loopback
credential_handle = "S_CODE_SKILL_SHOP_TOKEN"  # environment variable holding the registry token
skills = "skill_0123456789abcdef01234567"      # exactly the ids this daemon may receive
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

With a registry configured:

- `POST /v1/experiences/{id}/publish-skill` sends exactly the sanitized
  lesson, its applicability, the content digest, the sanitization version,
  the requested `visibility` (`team` by default) and bounded provenance
  metadata (`task_family`, `model_family`). It never transmits the raw
  trajectory, source code, tool logs, failure logs, absolute paths,
  environment variables, credentials, workspace contents or prompt history.
  The registry re-sanitizes the payload, derives the publisher from the
  token and content-addresses the skill, so a retry returns the same skill.
  The response is the registry's artifact; nothing is written to the local
  shop.
- At each turn the daemon fetches every requested id over the network and
  validates the answer fail-closed: the id must match, the text must be in
  canonical sanitized form, the content digest must match the text, and the
  status must permit injection. A network failure, a malformed or oversized
  response, a digest mismatch, an unverified skill or a deprecated skill
  injects nothing and is audited as `skill.retrieval_refused` with a reason
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
| `RUST_LOG` | `info` for the service | Tracing filter. Tokens are never logged at any level. |

On start the service prints `S_CODE_SKILL_REGISTRY_ADDR=<host:port>` to
standard error and writes `registry.json` (URL, pid, start time) into the
data directory. `GET /health` answers `{"status":"ok"}`. `SIGINT` or
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
  independence, completeness, safety and counts.
- `/shop/how-to-use` shows the daemon configuration and API examples for
  pinning a skill.
- `/shop/login` accepts a registry token from a form and keeps it in an
  `HttpOnly`, `SameSite=Strict` session cookie for the shop pages only;
  `/shop/logout` clears it. Tokens never appear in URLs.

Every page carries the notice "Community-provided derived agent knowledge.
This is advisory and not trusted system policy." Every value from the
database or the request is HTML-escaped; the pages contain no script and no
editor, and they never show tokens or token digests.

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
The registry computes the protocol digest, independence, completeness,
safety and the verdict itself. A body that claims `eligible`, `verified`,
`passed_gate`, `status`, `independent`, a publisher, an evaluator, an
organization or a team is rejected as a whole.

## Trust model

What the registry guarantees:

- Identity is the server-side principal of the presented token. No request
  body can name or change the publisher, evaluator, organization or team.
- Tokens are random, shown once, stored only as digests, compared in
  constant time, never logged and never accepted from URLs.
- A skill's content is immutable and bound to its digest; a retry publishes
  nothing new; every receipt is immutable and bound to that digest.
- Verification and deprecation are decided only by the deterministic gate,
  or by an explicit deprecation, inside one write transaction; concurrent
  receipts verify once; a deprecated skill never comes back.
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
  several tokens to one operator, or an operator holding several tokens,
  can produce "independent" receipts. Independence is an accounting rule
  over principals, not proof of separate judgement.
- Sanitization refuses paths, secrets, unsafe suggestions and source-specific
  text; it does not prove a lesson harmless or correct. Verified skills are
  advisory data, never trusted system policy.
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
lesson), `skill.retrieved` (ids, digests, registry, consumer, session, turn)
and `skill.retrieval_refused` (ids with a reason category). The registry
keeps its own append-only `events` table: `principal.created`,
`principal.disabled`, `skill.published`, `skill.evaluated`,
`skill.verified` and `skill.deprecated`, with ids, digests and gate
versions, never tokens and never lesson text.
