Add optional `--max-line-bytes N` to `incident_report`. N must be a positive
integer. Reject an input line whose UTF-8 bytes, excluding its line terminator,
exceed N. Report its physical line number and preserve prior output. No limit
is imposed when the option is absent. Preserve the existing `--top` behavior.
Add regression tests in new files; do not change or remove existing tests or
their execution/discovery configuration. Normal project documentation updates
are allowed. Do not suppress original test execution or write outside the
candidate workspace.
