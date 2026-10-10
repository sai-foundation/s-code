---
site: true
slug: self-evolving-s-code
title: Self-Evolving S-Code
short_title: Self-evolution
group: Operate
order: 81
description: How S-Code turns an agent's own corrections into verified Experience, sanitized shared Skills and collaboratively refined knowledge, with explicit trust boundaries, opt-in configuration and a runnable demo.
keywords:
  - self-evolution
  - experience
  - skill
  - skill shop
  - forum
  - verification
  - trust boundary
  - privacy
---

# Self-Evolving S-Code

S-Code implements an end-to-end, opt-in, verified self-evolution stack. An agent can turn a
corrective trajectory into a quarantined Experience, have it distilled, evaluated and
approved, reuse it on its own later work, publish it as a sanitized Skill for independent
verification, let other agents reuse the verified Skill, and let the community challenge,
refine, compare and supersede it through an auditable lineage. Everything the models
derive along that path stays advisory, derived-untrusted data: it never outranks the
system rules, the user, policy, permissions, the sandbox or a tool result.

This page is the map. The settings and the full trust model are in
[Online skill shop](skill-shop.md); the lifecycle is runnable in
[Self-evolution demo](self-evolution-demo.md); what leaves a machine is in
[Privacy](privacy.md).

## Why

Agents repeatedly rediscover the same corrections: a check that fails, an edit that makes
it pass, a lesson nobody wrote down. Keeping that knowledge is valuable, but
model-generated knowledge that persists across sessions, actors and machines needs trust
boundaries that are explicit, enforced by the product and auditable. S-Code's answer is a
lifecycle in which every promotion is a decision the product can verify, and every
artifact crosses a boundary with a stated, bounded content.

## The three loops

**Local loop: correction → Experience → verification → reuse.** While a turn runs, the
daemon keeps a bounded corrective trace of verifier runs and edits. A recovery segment is
one verifier identity that failed, at least one successful edit, and the identical
verifier passing afterwards, with no later edit. Only that segment, bounded, becomes
evidence. After the turn, the evidence is distilled into a short lesson with its
applicability through the same protected, observed provider path as the agent itself.
The result is a **candidate**: stored, quarantined, never retrievable. An explicit
decision admits it — manually, or through an evaluation on held-out work recorded as an
immutable record the approval is bound to — in one transaction with its audit event. Only
an approved Experience of the same actor and project can later be retrieved into a turn,
and then only as advisory context.

**Population loop: Experience → sanitized Skill → independent verification → sharing.** An
approved Experience can be published to a Skill Shop as a sanitized Skill candidate. The
candidate carries the lesson, its applicability and provenance, and nothing else: no
evidence, no workspace key, no session or turn id, no path from the machine it was learned
on. Independent evaluators file immutable receipts under their authenticated identity;
imported receipts are kept as provenance and count for nothing. A deterministic gate over
the receipts on record promotes the candidate to a **verified** Skill inside the same write
transaction that records the last receipt. A fresh agent in another scope may then be sent
that Skill, under the same advisory framing, and every retrieval or refusal is an audit
event. Verified means it passed the gate, not that it is universally beneficial.

**Evolution loop: challenge → refinement → comparison → supersession.** The Forum is a
structured discourse layer over immutable Skills. A **challenge** is an immutable,
evidence-backed claim against an exact Skill version, with counterexamples. A **fork** is a
refinement that never edits a Skill in place: a new candidate whose lineage the registry
derives. A fork does not replace its parent by passing the ordinary gate; it must also win
a **comparative evaluation** on matched held-out tasks judged by the registry, after which
the registry records the **supersession** and the lineage shows which version currently
stands. Deprecation for safety is separate from supersession and is never injectable.

## Trust

Experience and Skill content never becomes instruction authority. Retrieved items are
injected as derived, untrusted context with a fixed preamble and provenance, below the
system rules, the user's turn, policy, permissions, the sandbox and tool results. The
product decides, rather than accepting from a request: quarantine and approval, evaluator
identity and authority, the verification gate, lineage, and what a Skill may contain.

## Privacy and safety

- **Scope.** Experiences belong to an actor and project; Skill visibility is organization
  and team scoped; retrieval is scope-bound on the server.
- **Privacy resets.** When file protection changes for an actor, content derived before the
  change stops crossing the model boundary, whether an Experience or a shared Skill;
  distillation itself refuses to run across such a change.
- **Sanitization.** Publication is bounded by an explicit sanitizer contract; a candidate
  that fails it is not published.
- **Authenticated receipts.** Evaluator identity and independence come from the registry's
  own records; the number and independence of receipts are facts the gate reads, not
  fields a client supplies.
- **Quarantine.** Candidates are never retrievable; a poisoned candidate stays where it is.
- **Auditability.** Every decision, retrieval, refusal, receipt, challenge, fork and
  supersession is an audit event.

## Opt-in configuration

Everything is off by default. Turn on only what a deployment needs.

```toml
[daemon]
experience_mode = "verified"        # off (default) | observe | verified
experience_promotion = "manual"     # manual (default) | evaluated | automatic

[daemon.skill_shop]
mode = "explicit"                   # off (default) | explicit | evaluation
url = "https://skills.example.org"  # unset: the daemon's local shop
credential_handle = "S_CODE_SKILL_SHOP_TOKEN"
skills = "skill_0123456789abcdef01234567"
lineage = "pinned"                  # pinned (default) | active
```

The same settings are available as `S_CODE_DAEMON_EXPERIENCE_MODE`,
`S_CODE_DAEMON_EXPERIENCE_PROMOTION`, `S_CODE_DAEMON_SKILL_SHOP_MODE`,
`S_CODE_DAEMON_SKILL_SHOP_URL`, `S_CODE_DAEMON_SKILL_SHOP_CREDENTIAL_HANDLE`,
`S_CODE_DAEMON_SKILL_SHOP_SKILLS` and `S_CODE_DAEMON_SKILL_SHOP_LINEAGE`.
`observe` records candidates without ever injecting them; `verified` also retrieves
approved Experiences. `evaluation` is an evaluation-only shop mode, audited as such.

## See it run

```sh
scripts/demo-self-evolve.sh                  # deterministic replay, no provider
scripts/demo-self-evolve.sh --live \
    --organization ORG --team TEAM --actor ACTOR \
    [--registry URL --registry-token TOKEN]
```

Replay narrates a recorded run that was reduced to its lifecycle and stripped of anything
private, checked against its own digest. Live walks the same lifecycle against your own
daemon through the product's scoped endpoints and shows only state that run actually
read; a stage it cannot show stops the demo saying what is missing. Nothing is
synthesized in either mode.

