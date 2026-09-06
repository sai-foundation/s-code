# Incident report CLI

A small standard-library Python application for deterministic incident reports.
Requires Python 3.10 or later. No installation or network access is needed.

```sh
python3 -m incident_report events.jsonl --output report.json
python3 -m unittest discover -s tests -v
```

Each non-empty JSONL line contains `service`, `severity`, and `message` strings.
Severity is `info`, `warning`, or `error`. Extra fields are ignored. The report
has total, severity and service counts, and the three most frequent messages.

The package separates event parsing, pure aggregation, atomic output, and CLI
errors. See [behavior contracts](docs/contracts.md). Extend the application
without changing existing tests; add new regression files under `tests/`.
