---
site: true
slug: privacy
title: See what goes to your model
short_title: Privacy
group: Guides
order: 66
description: Inspect file sources and model request history in Web and CLI.
keywords:
  - privacy
  - files
  - model requests
---

# See what goes to your model

Open **Privacy** in the Web header to see a panel on the right. In the terminal,
run `/privacy`. Both views show the current conversation's recorded model
requests and require access to the account that owns the conversation.

The Web panel starts with a deduplicated source list. Expand a source to see
its destinations and request outcomes, or expand a request for its timestamp,
model, endpoint origin, payload size and included sources. **Load older requests**
continues through the history. In the CLI, use the arrow keys or Page Up / Page
Down to scroll, `n` for older requests, `r` to refresh and Escape to close.

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
It reports file reads as excerpts rather than assuming a whole-file upload.
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
