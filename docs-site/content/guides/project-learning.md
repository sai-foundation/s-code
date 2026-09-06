---
site: true
slug: project-learning
title: Project learning
short_title: Project learning
group: Build with S-Code
order: 55
description: Remember verified project changes and reuse relevant excerpts across local coding sessions.
keywords:
  - self-evolving
  - learning
  - memory
  - verified experience
---

# Project learning

S-Code can remember source excerpts changed during completed tasks and recall
relevant excerpts in later sessions. For example, a later task that names the
shared clock helper's file can receive an eligible excerpt. These are historical
observations, not model-generated procedures or proof that the tests covered
every remembered line. A performance advantage has not yet been established.

In the interactive terminal, select a session and use:

| Command | Effect |
| --- | --- |
| `/learn on` | Learn from eligible completed tasks and reuse relevant lessons. |
| `/learn reuse` | Reuse existing lessons without generating new ones. |
| `/learn off` | Disable extraction and recall; retain stored lessons. |
| `/learn list` | Show the setting, last recorded learning result, stored lessons, recorded dependency paths, sources and expiry. |
| `/learn remove ID` | Delete a particular lesson. |
| `/learn clear` | Delete all learned lessons for this project and identity. |

In Local Web, select a session, then open **More options (⋯) → Show context →
Self-evolving** to change the mode. You can also search More options for
**learning** or **self-evolving**. The same context panel lists stored lessons,
recorded dependency paths and deletion controls. Clear remains available when
no lessons are listed: it also stops pending saves and resets the last learning
result, while keeping the selected mode.

The last recorded learning result explains whether learning was skipped,
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

Saving experience uses no additional model request. It selects bounded excerpts
from actual `apply_patch` edits completed before the successful verification began.
Each file must still match the edit result’s full hash and byte length. The saved
range encloses the edits and may include unchanged lines between them. Shell
writes, read-only tasks and pure deletions do not supply new records. At most three file observations
are saved per task, each with up to two bounded source fragments. Command output, private reasoning,
and the old task prompt are not saved as source observations. A self-reported
success alone cannot create a record.

Every lesson belongs to the organization, team, actor and canonical workspace
that produced it. New sessions in the same directory can reuse it; another
checkout or identity has separate experience. There is no automatic global or
team sharing.

## Recall and control

The CLI and Web lists show stored, unexpired lessons, including ones whose
related files have changed. Recorded dependency paths explain what each lesson
was based on; displaying a path does not mean its current contents have been
checked. The lists do not report current applicability.

Before each coding-model request, S-Code uses an explicitly named relative file
path or a matching location from the current task's latest file lookup to select
source observations. Shared words alone do not trigger recall. Ambiguous or
outdated lookup results are ignored, and excerpts already fully visible in the
current read-file results are not repeated. S-Code verifies dependency files;
changed files, expired records and incomplete source
turns are excluded. Recall is limited to four lessons and 1,200 estimated tokens.
A different eligible task can refresh a recorded source range after its file
changes. Replaying the same source task cannot refresh it. Source text is
untrusted context; repository instructions, the current request and tool
permissions still apply.

Older generated lessons and read-only source observations remain visible and removable, but are no longer
automatically recalled. Enabling learning does not silently delete them.

Lessons are encrypted in the existing local database, capped at 64 per project,
and expire after 30 days. Removing lessons, disabling learning or switching to
reuse invalidates pending extraction. Subsequent requests use the current
settings, including after a paused task resumes. A request already sent to a
model cannot be withdrawn by deleting a local lesson.

Deletion removes the stored lesson text and leaves a content-free tombstone to
prevent a late response from recreating it. Source task transcripts follow the
normal session-history controls. Recall text is not copied into checkpoints.

## Cost and evidence

Source selection is local and bounded; it does not call a reflection model.
Reusing excerpts still adds input tokens to coding requests. Tasks with missing
provider usage remain ineligible, and unknown usage is never reported as zero.
A failed extraction does not fail a successful coding task.

This feature adapts project context. It does not train model weights or install
self-modifying code. The [design and research](../architecture/self-evolving.md)
describes the mechanism and evaluation contract. Mechanism tests establish
isolation and lifecycle behavior; real task-transfer benchmarks are required
before making performance claims.
