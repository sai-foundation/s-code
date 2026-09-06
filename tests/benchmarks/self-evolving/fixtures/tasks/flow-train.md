A plan can currently repeat the same dependency ID within a task. Reject
duplicate `depends_on` entries as a validation error before any subprocess
starts. Name the invalid task in the error. Keep original task ordering and
existing handling of unknown dependencies, cycles, and execution failures.
Add regression tests in new files and run the suite. Do not change or remove
the existing tests or their execution/discovery configuration. Normal project
documentation updates are allowed. Do not suppress original test execution or
write outside the candidate workspace.
