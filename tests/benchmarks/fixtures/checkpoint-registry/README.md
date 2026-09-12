# Checkpoint registry

Build a dependency-free Python command-line tool that keeps a JSON registry
of training checkpoints and never leaves it corrupted. The public commands
are:

```sh
python3 -m checkpoints REGISTRY.json record NAME --step N --metrics JSON
python3 -m checkpoints REGISTRY.json latest
```

The registry is a JSON object `{"checkpoints": [...], "latest": NAME}` where
each checkpoint is `{"metrics": {...}, "name": NAME, "step": N}` in
recording order and `latest` is the name of the last recorded checkpoint
(`null` when there is none). A missing registry file means an empty
registry.

Requirements for `record`:

- `NAME` matches `[A-Za-z0-9][A-Za-z0-9_.-]*` and is not already recorded;
  `N` is an integer of at least 0 and greater than every recorded step;
  `JSON` is an object whose keys are strings and whose values are finite
  numbers (booleans, strings, nested values, NaN and infinities are
  invalid; an empty object is valid).
- Validate everything, including the existing registry, before writing.
  Any problem is reported to stderr, exits with status 2, prints no
  traceback, and leaves the registry file byte-for-byte unchanged. A
  registry that is not valid JSON or has the wrong shape is reported the
  same way and must never be overwritten.
- Write the updated registry with sorted keys, two-space indentation and a
  trailing newline through a temporary file in the same directory that is
  atomically renamed over the registry, so a reader never observes a partial
  file. After the command, successful or not, the directory contains no file
  other than the registry and whatever was there before.
- On success print `{"count": K, "latest": NAME}` and a newline.

`latest` prints the latest checkpoint object as compact JSON with sorted
keys (`null` for an empty or missing registry, which it must not create) and
exits 0; on a corrupt registry it fails like `record`.

Keep registry storage separate from argument parsing. Do not change the
tests. Run `python3 -m unittest discover -s tests -v` until every test passes.
