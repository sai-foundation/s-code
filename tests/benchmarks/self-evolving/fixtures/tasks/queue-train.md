Fix the queue CLI so `nan`, `inf`, and `-inf` cannot be accepted for `--now` or
`--lease-seconds`. Invalid numeric input must return the normal user-error status
before initializing or mutating a database. Preserve finite boundary behavior,
including existing finite negative, zero, and fractional timestamps. Add
regression tests in new files and run the relevant suite. Do not change or
remove the existing tests or their execution/discovery configuration. Normal
project documentation updates are allowed. Do not suppress original test
execution or write outside the candidate workspace.
