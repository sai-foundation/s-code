# Selective project recall

Status: experimental candidate after the failed Quality09 development screen.
End-to-end quality and efficiency improvements have not been established.

Quality09 verified that retained changes reached the model, but learned tasks
succeeded 27/36 times versus 30/36 with learning off. Every negative-control
initial request also received experience. Shared words in a task and an old
fragment are too weak a reason to add context repeatedly.

This candidate changes only recall. Collection, storage, maximum payload,
verification requirements, expiry, project ownership and revocation stay the
same. It adds no reflection request or executable behavior.

Recall can use an exact, explicitly mentioned relative file path. It does not
match a basename inside another path, split identifiers, expand synonyms or
infer intent from a shared word. A leading `./` is accepted; case and path
boundaries are preserved.
This initial candidate expects the path itself rather than a `path:line` citation.

Without an explicit path, the current turn's latest built-in lookup can supply
a location if it succeeded. A failed lookup does not fall back to an older
successful one. A read must identify the same full-file hash and a
returned line matching the retained fragment. A search must use a complete
literal identifier, return one unambiguous path, and identify the same line
and actual text in a retained fragment. This is a location match, not symbol
definition resolution. Regex, incomplete, malformed and ambiguous search
results supply no focus. A later or overlapping potentially mutating tool
invalidates that focus. An earlier turn's tools never establish a new focus.

Before sending each request, recall still checks current file hashes and the
learning generation. If all saved fragments are already present at their exact
positions and hash in visible read-file tool results, no duplicate is appended.
A successful, complete search result also suppresses a fragment when it contains
all of its exact path, line number and literal text pairs. Partial searches still
allow recall of the unseen lines; conflicting duplicate line numbers do not
suppress recall. Search visibility does not establish a new retrieval focus or
replace the live source-hash check.
This considers the actual outgoing content, so a compacted-away lookup does not
suppress relevant recall indefinitely. Experience is expanded only in the
provider's outgoing copy and never stored as a persistent tool result or
checkpoint message.

The first validation stage uses unit, stored-runtime and real-daemon/local-
provider cases covering path boundaries, absent anchors, stale locations,
ambiguous search, current-turn isolation, duplicate suppression and revocation.
These prove selection behavior only. A subsequent end-to-end experiment needs
a separately fixed protocol that charges all learning and retrieval costs,
keeps failed attempts, and does not require irrelevant initial injections as a
success criterion. The existing sealed holdout remains closed.
