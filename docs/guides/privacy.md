---
site: true
slug: privacy
title: See what goes to your model
short_title: Privacy
group: Guides
order: 66
description: Inspect local file coverage and model request history in Web, Mac and CLI.
keywords:
  - privacy
  - files
  - model requests
---

# See what goes to your model

Open **Privacy** in Web or Mac to cover the main conversation area while keeping
the sidebar available. Use **Back** to return to chat. Two tabs separate local
file coverage from individual model requests.

- **Files** is a collapsible local directory tree. Red means complete captured
  text was included in an accepted request; orange means partial text or delivery
  is unconfirmed; white means no transmission was identified in loaded records.
  Search checks opened folders and recorded paths, including historical files.
  Large directory listings and visible rows are bounded. Open folders as needed
  instead of loading the entire repository. Colors describe recorded versions,
  which may differ from files currently on disk.
- **Event record** lists individual dispatches, newest first. Expand an event for
  its model, endpoint, timestamp, request bytes, outcome, file sources and other
  context categories. Filter the loaded events or load earlier history. Web uses
  bounded pages for events and their source lists, so older entries stay reachable.

Web's file tree uses an authenticated, session-scoped local daemon endpoint on
macOS and Linux. It reads only names and metadata, does not follow symlinks, and
caps each directory at 3,000 scanned entries. Hidden files can appear as names;
opening this page does not load their contents into model context. A shortened
listing is labeled. In Chat mode there is no project tree; request events remain
available. The view clears when the account or conversation changes.

In the terminal, run `/privacy`. Use the arrow keys or Page Up / Page Down to
scroll, `n` for older requests, `r` to refresh and Escape to close. All views
require access to the account that owns the current conversation. No recorded
transmission is not proof that a file never left the computer.

## What a record means

S-Code records each built-in model HTTP dispatch after context packing and
provider conversion, including retries, fallback endpoints and title generation.
Reading a file locally does not by itself create a disclosure record.

- **Request started:** dispatch was about to begin; delivery is unconfirmed.
  A cancellation or process exit can leave this status behind.
- **Accepted by endpoint:** the endpoint returned a successful HTTP status.
  This does not prove that generation finished or describe provider retention.
- **Rejected by endpoint:** the endpoint returned an error. It may still have
  received the request body.
- **Connection error:** delivery is unknown; a failure does not prove that no
  bytes were sent.

The destination is the immediate endpoint origin, including a local endpoint
when used. S-Code cannot inspect forwarding done by that endpoint.

## Which sources are identified

The ledger identifies packed project instructions, editor context, skills and
knowledge sources, structured `read_file` results, and matching attachment parts.
A file read is labeled complete captured text only when numbered lines cover
all reported lines and the result is explicitly untruncated. Other reads,
including older records without that evidence, stay labeled excerpts. This
describes the captured text, not the current file or binary-byte fidelity.
Unsupported attachments that contribute only their names are labeled accordingly.

Shell output, search output, MCP results, code-mode output, pasted text, generated
text and summaries can include file contents without reliable individual file
attribution. These appear under **Other context included**. The panel is not a
network monitor for arbitrary commands, hooks, MCP servers or external harnesses.
An empty source list is not a claim that the request contains no file data.
Requests from versions before this ledger was added have no records.

## Local storage and access

Records contain metadata, not file bodies, model request bodies, headers or API
keys. Endpoint credentials, paths and query parameters are excluded from endpoint
labels. Records use the existing local audit store and its configured encryption;
they survive a service restart and follow existing audit retention and export
policies. They are not sent to the model as extra context. As with local session
history, someone controlling your OS account can access local data.

This first version is a read-only transparency view. It does not revoke data
already sent or claim that blocking one filename can remove copies from shell
output, conversation history or summaries. Existing sandbox and tool permission
controls continue to apply.
