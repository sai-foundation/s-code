# Dependency flow runner

A Python 3.10+ workflow executor using only the standard library.

```sh
python3 -m flow_runner plan.json --jobs 3 --output report.json
python3 -m unittest discover -s tests -v
```

Plans contain a `tasks` array. Each task has an `id` and a non-empty argv-array
`command`, with optional `depends_on`, `resource`, `retries`, and
`timeout_seconds`. For example:

```json
{"tasks":[{"id":"hello","command":["python3","-c","print('hello')"]}]}
```

The application validates the whole graph, schedules ready tasks up to the job
limit, and writes a structured report. See [behavior contracts](docs/contracts.md).
Add regression files under `tests/`; preserve the existing tests.
