# Behavior contracts

- Read UTF-8. Empty or whitespace-only lines are ignored, but errors use the
  physical line number. The three required event fields are strings; unrelated
  keys are ignored. Invalid JSON, event types, or severities are user errors.
- JSON output contains exactly `total`, `by_severity`, `by_service`, and
  `top_messages`. Include all three severities even when zero. Sort services by
  ordinary Python string order; messages by decreasing count, then string order.
  Every successful report ends in a newline, with no success output on stdout.
- Validate the whole input before publishing. On a bad late line, invalid
  option, or I/O error, return 2 with a concise stderr message and no traceback.
  An existing output must remain byte-for-byte unchanged on validation failure.
- `events.read_events` owns parsing, `reports.summarize` owns aggregation, and
  `io.atomic_write` owns same-directory temporary-file publication. These are
  ordinary contributor helpers, not mandatory implementation details.
