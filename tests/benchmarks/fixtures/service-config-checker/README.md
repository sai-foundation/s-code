# Service config checker

Build a dependency-free Python command-line tool that validates a service
configuration written in INI format (as read by the standard library's
`configparser`) and writes a typed JSON summary. The public command is:

```sh
python3 -m confcheck CONFIG.ini --summary SUMMARY.json
```

The configuration must contain exactly these sections and keys:

- `[service]`: `name` matching `[a-z][a-z0-9-]*`, `port` an integer from
  1024 to 65535, `workers` an integer from 1 to 64.
- `[limits]`: `request_timeout_seconds` a decimal number greater than 0 and
  at most 300, `max_body_kib` an integer from 1 to 65536.
- `[flags]`: any number of keys whose values are exactly `on` or `off`.

Requirements:

- Validate the whole file before writing anything: missing sections or
  keys, unknown sections, unknown keys in `[service]` or `[limits]`, and
  values of the wrong type or out of range are all errors.
- Report the first problem to stderr as `CONFIG.ini: [section] key: <reason>`
  (or `CONFIG.ini: [section]: <reason>` for a section-level problem) using
  the path as given, exit with status 2, print no traceback, and leave an
  existing summary file byte-for-byte unchanged; do not create a missing one.
- An unreadable or syntactically invalid file is reported as
  `CONFIG.ini: <reason>` with exit status 2 and no traceback.
- On success write the summary as JSON with sorted keys, two-space
  indentation and a trailing newline: `service` holding `name` (string),
  `port` and `workers` (integers); `limits` holding `request_timeout_seconds`
  (number) and `max_body_kib` (integer); `flags` mapping each key to `true`
  or `false`. Print `{"flags": N}` and a newline to stdout and exit 0.

Keep validation separate from the command-line entry point. Do not change
the tests. Run `python3 -m unittest discover -s tests -v` until every test
passes.
