# Incident report CLI

Build a small Python command-line project that turns newline-delimited incident
events into a deterministic JSON report.

The public command is:

```sh
python3 -m incident_report INPUT.jsonl --output REPORT.json
```

Each non-empty input line is a JSON object with `service`, `severity`, and
`message` strings. Supported severities are `info`, `warning`, and `error`.
The report must contain:

- `total`: number of valid events;
- `by_severity`: all three severity counts, including zeros;
- `by_service`: counts sorted by service name;
- `top_messages`: up to three `{message, count}` objects, ordered by decreasing
  count and then message text.

Create the `incident_report` package and keep aggregation logic separate from
the command-line entry point. Write JSON with a trailing newline. Empty lines
are ignored. A malformed event must print a useful `line N:` error to stderr,
return exit code 2, and leave an existing output file untouched.

Do not change the tests. Run `python3 -m unittest discover -s tests -v` and keep
working until every test passes.
