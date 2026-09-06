Add an optional `--top N` argument to `incident_report`. Preserve the existing
default of three. N is a non-negative integer; zero produces an empty
`top_messages` list. Invalid values are user errors and must preserve an existing
output report. Keep the remaining output schema and existing CLI behavior.
Add regression tests in new test files and run the relevant test suite. Do not
change or remove the existing tests or their execution/discovery configuration.
Normal project documentation updates are allowed. Do not suppress original
test execution or write outside the candidate workspace.
