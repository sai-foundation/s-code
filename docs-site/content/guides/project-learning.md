---
site: true
slug: project-learning
title: Project learning
short_title: Project learning
group: Build with S-Code
order: 55
description: Learn verified project conventions and reuse relevant experience across local coding sessions.
keywords:
  - self-evolving
  - learning
  - memory
  - verified experience
---

# Project learning

S-Code can learn reusable project conventions from completed tasks and recall
them in later sessions. For example, after fixing one queue operation it might
remember the project's shared clock helper and transaction conventions when
working on a different queue operation.

In the interactive terminal, select a session and use:

| Command | Effect |
| --- | --- |
| `/learn on` | Learn from eligible completed tasks and reuse relevant lessons. |
| `/learn reuse` | Reuse existing lessons without generating new ones. |
| `/learn off` | Disable extraction and recall; retain stored lessons. |
| `/learn list` | Show the setting, last recorded learning result, lessons, sources and expiry. |
| `/learn remove ID` | Delete a particular lesson. |
| `/learn clear` | Delete all learned lessons for this project and identity. |

Local Web exposes the same setting and deletion controls under **Context →
Self-evolving**. The last recorded learning result explains whether learning was skipped,
attempted without a new lesson, failed, or saved lessons. It includes the time
and a short reason, such as changes after verification or incomplete provider
usage. Changing the mode or deleting lessons clears this result and cancels
pending saves. A process or storage failure can prevent a new result from being
recorded, so check its timestamp. A saved result describes a past learning step;
current relevance, source completion, expiry and file checks determine whether
the lesson can still be reused. Explicit memory saved through `/memory` is
managed separately.

## What gets learned

Learning is off by default. When enabled, an uninterrupted coding task must
complete and include an observed successful, nonempty test run. Supported
summaries include unittest, pytest, Cargo, verbose Go tests and common JavaScript
test runners. Help output, collection-only runs, failures and cancelled tasks
are ineligible. Existing recognized tests and test configuration must remain
unchanged; new regression test files are allowed. A source file containing Rust
inline tests is conservatively protected as a whole file. Large or unsupported
verification layouts may cause extraction to be skipped.

Complete necessary source and documentation changes before final verification.
If files are edited afterward, another successful test run is needed before
automatic learning. Documentation is not exempt: tests or application code may
read it. When there is not enough budget to verify the final changes, learning
is skipped.

One reflection request uses the task's configured model. It receives bounded
public task input and tool evidence, with no tools or private reasoning. It may
produce up to three short lessons about conventions, dependencies or procedures.
The daemon checks cited tool IDs and computes file hashes from observations made
before verification. A self-reported success alone cannot create a lesson.

Every lesson belongs to the organization, team, actor and canonical workspace
that produced it. New sessions in the same directory can reuse it; another
checkout or identity has separate experience. There is no automatic global or
team sharing.

## Recall and control

Before each coding-model request, S-Code selects relevant lessons and verifies
their dependency files. Changed files, expired records and incomplete source
turns are excluded. Recall is limited to four lessons and 1,200 estimated tokens.
When a different verified task produces the same guidance for updated related
file versions, S-Code can replace the old lesson with the new evidence. A repeated
result from the same source task cannot refresh it. Lesson text is untrusted
context; repository instructions, the current request and tool permissions still
apply.

Lessons are encrypted in the existing local database, capped at 64 per project,
and expire after 30 days. Removing lessons, disabling learning or switching to
reuse invalidates pending extraction. Subsequent requests use the current
settings, including after a paused task resumes. A request already sent to a
model cannot be withdrawn by deleting a local lesson.

Deletion removes the stored lesson text and leaves a content-free tombstone to
prevent a late response from recreating it. Source task transcripts follow the
normal session-history controls. Recall text is not copied into checkpoints.

## Cost and evidence

Extraction has a 30-second deadline, a bounded input and at most 1,024 output
tokens. It is skipped when the remaining task or goal budget is insufficient,
or when the completed task contains requests whose full usage was not received.
Provider-reported learning tokens are included in task usage. Learning events
identify missing usage explicitly; they do not report it as known zero cost.
Task usage notes retain this distinction after reopening a session. Displayed
token counts are the reported subtotal when completeness is unknown.
A reflection response with incomplete usage cannot create a lesson. A failed
extraction does not fail a successful coding task.

This feature adapts project context. It does not train model weights or install
self-modifying code. The [design and research](../architecture/self-evolving.md)
describes the mechanism and evaluation contract. Mechanism tests establish
isolation and lifecycle behavior; real task-transfer benchmarks are required
before making performance claims.
