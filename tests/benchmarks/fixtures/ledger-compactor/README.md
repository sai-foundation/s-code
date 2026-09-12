# Ledger compactor

Build a dependency-free Python command-line tool that folds an append-only
account journal into a balance snapshot without ever corrupting either file.
The public command is:

```sh
python3 -m ledger compact JOURNAL.csv --snapshot BALANCES.json
```

`JOURNAL.csv` holds one entry per line as `seq,account,delta`: `seq` is a
positive integer that strictly increases down the file, `account` matches
`[a-z][a-z0-9_]*`, and `delta` is a signed integer such as `-25` or `40`.
Blank lines are not allowed. `BALANCES.json` is an object
`{"accounts": {...}, "through_seq": S}` mapping account names to integer
balances; when the file does not exist it is treated as empty with
`through_seq` 0.

Compaction applies every journal entry, whose `seq` must be greater than
the snapshot's `through_seq`, to the account balances, writes the new
snapshot with `through_seq` set to the last applied `seq`, and rewrites the
journal as an empty file, so the state is carried entirely by the snapshot.

Requirements:

- Validate the whole journal and the existing snapshot before writing
  anything. A bad journal entry is reported to stderr as `line N: <reason>`
  (1-based), a snapshot that is not valid JSON or has the wrong shape as
  `BALANCES.json: <reason>` using the path as given; in both cases exit with
  status 2, print no traceback, and leave both files byte-for-byte unchanged.
- Write each file through a temporary file in the same directory that is
  atomically renamed over the target, the snapshot first and then the
  journal. After the command, successful or not, the directory contains no
  file other than the journal, the snapshot and whatever was there before.
- The snapshot uses sorted keys, two-space indentation and a trailing
  newline; accounts whose balance is 0 are omitted. Compacting an empty
  journal is a successful no-op that changes neither file.
- On success print `{"folded": N, "through_seq": S}` and a newline.

Keep journal parsing, folding and file replacement separate. Do not change
the tests. Run `python3 -m unittest discover -s tests -v` until every test
passes.
