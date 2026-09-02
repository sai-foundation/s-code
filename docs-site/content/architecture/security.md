---
site: true
slug: security
title: Security model
short_title: Security model
group: Operate
order: 80
description: Review the local trust boundary, browser credential handling and fail-closed execution rules.
keywords:
  - security
  - credentials
  - browser
  - cookie
  - network
  - sandbox
---

# Security model

## Trust boundary

The default service is local and loopback-bound. Provider credentials remain
with the independently managed model process. Workspace tools are constrained
to the authorized root, sensitive paths are denied and policy decisions are
made before execution.

## Browser session

Local Web never receives the provider key or daemon bearer token. A one-time
bootstrap becomes an HttpOnly, SameSite=Strict cookie, keeping credentials out
of browser JavaScript, storage and URLs.

## Execution boundaries

- Commands use structured executable and argument fields rather than an
  implicit shell.
- Network access is disabled unless it is explicitly required and approved.
- Workspace-write and browser-test profiles retain explicit write roots.
- Timeouts terminate and wait for the entire child-process tree.
- External integrations cannot bypass local policy and approval.

## Local state

Fresh installations keep the SQLite database outside the repository under
`~/.opencoding/state`. Sensitive fields are protected with AES-256-GCM and a
generated 32-byte managed key; the directory, database and key use private
permissions and reject unsafe symlink targets. The database and key are
separate files so a database copied alone does not disclose session content.

This is a local-at-rest boundary, not a claim to resist compromise of the
signed-in operating-system account. A process able to read the entire state
directory can obtain both files. Use full-disk encryption, protected backups
and a secure user account as the outer boundary.

## Audit

Tool requests, policy decisions, approvals, execution outcomes and usage are
projected as typed session events. Sensitive content is bounded and protected
separately from metadata needed for review and replay.
