# Logline normalizer

Build a dependency-free Python command-line tool that turns a plain-text
service log into newline-delimited JSON events. The public command is:

```sh
python3 -m logline INPUT.log --output EVENTS.jsonl
```

Each non-empty input line has the form

```text
2026-09-12T10:03:07Z WARN scheduler: queue depth 12
```

that is, a UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`), one space, a level, one
space, a component name followed by a colon, one space, and the message.
Blank and whitespace-only lines are ignored. Levels are matched
case-insensitively and must be one of `debug`, `info`, `warn` or `error`.
Components match `[a-z][a-z0-9_-]*`. The message is the remainder of the
line with surrounding whitespace removed and must not be empty.

Requirements:

- Write one JSON object per event, in input order, as
  `json.dumps(event, sort_keys=True)` followed by a newline, with the keys
  `timestamp` (as given), `level` (lowercase), `component` and `message`.
- Validate the whole input before writing anything. The first invalid line
  is reported to stderr as `line N: <reason>` using the 1-based physical line
  number, the exit status is 2, no traceback is printed, and an existing
  output file is left byte-for-byte unchanged; a missing one is not created.
- An unreadable input path is reported to stderr as `<path>: cannot read
  input`, again with exit status 2 and no traceback.
- On success print `{"events": N}` and a newline to stdout and exit 0.

Keep parsing separate from the command-line entry point. Do not change the
tests. Run `python3 -m unittest discover -s tests -v` until every test passes.
