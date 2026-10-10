---
site: true
slug: self-evolution-demo
title: Self-evolution demo
short_title: Self-evolution demo
group: Operate
order: 83
description: Walk the lifecycle from one agent's correction to a verified Skill another agent reuses, either as a deterministic replay or against your own running daemon.
keywords:
  - demo
  - self-evolution
  - experience
  - skill
  - verification
  - trust boundary
---

# Self-evolution demo

S-Code can remember a correction it made, have that lesson verified by others, and
reuse it later. The demo shows that path end to end, in one of two modes, and it
never blurs them:

```sh
scripts/demo-self-evolve.sh                  # deterministic replay, no provider
scripts/demo-self-evolve.sh --live \
    --organization ORG --team TEAM --actor ACTOR
```

**Replay** narrates a recorded run that was reduced to its lifecycle and stripped
of anything private. It needs no provider, no daemon and no network, its fixture
is checked against its own digest before a line is printed, and two runs print
the same bytes — which is what makes it usable in a talk or a README.

**Live** walks the same lifecycle against your own daemon, through the product's
own scoped endpoints: the Experiences it holds, the Skills that scope can see,
the receipts behind a verified Skill, and how often that Skill has actually been
injected into a turn. Add `--registry URL --registry-token TOKEN` to read the
lineage a Skill belongs to in the shared registry.

Nothing in live mode is filled in for you. Every line is state the run just read,
and a stage that cannot be shown stops the demo saying what is missing and how to
produce it — no approved Experience, no verified Skill, fewer than two
independent evaluators behind it, or a Skill nobody has reused yet. A demo that
synthesized a candidate to look successful would be worse than no demo.

## What the stages mean

| stage | what it shows |
| --- | --- |
| learned | an episode failed its own verifier, was corrected, and passed it |
| candidate | the lesson is quarantined: stored, never retrievable |
| evaluated | the candidate was measured on held-out work before anything trusted it |
| published | an approved Experience became a sanitized shared Skill candidate |
| receipts | independent evaluators filed immutable receipts on that candidate |
| verified | the deterministic gate promoted it on those receipts, not on a claim |
| reused | another actor's turn retrieved it |
| refused | a poisoned candidate stayed in quarantine and never reached a prompt |
| refined | the registry's lineage: which version currently supersedes which |

## The trust boundary

A Skill that crosses between agents carries a sanitized lesson, its
applicability and the receipts that verified it — and nothing else. It carries no
evidence, no workspace key, no session or turn id, and no path from the machine
it was learned on.

What the product decides, rather than accepting from a request:

- **Quarantine.** A new Experience is a candidate. Only an explicit, scope-checked
  decision by its owning actor can approve it, and only an approved Experience is
  ever retrievable.
- **Identity and authority.** The evaluator of a receipt is the authenticated
  principal, and independence and authority come from the server's own records. A
  receipt copied in from elsewhere is kept as provenance and counts for nothing.
- **Verification.** Promotion is a deterministic gate over the receipts on record,
  inside the same write transaction that records the last one. "Verified" means it
  passed that gate, not that it is universally beneficial.
- **Injection.** A retrieved Skill is advisory, derived, untrusted data. It never
  outranks the user, the system rules, policy or a tool result, and every
  retrieval and refusal is an audit event.
- **Privacy.** When file protection changes for an actor, content derived before
  that change stops crossing the model boundary, whether it is an Experience or a
  shared Skill.

Both the Experience memory and the shop are off unless a deployment turns them
on. See [Online skill shop](skill-shop.md) for the settings and the full trust
model, and [Privacy](privacy.md) for what leaves a machine.
