# Public cross-task transfer pilot fixtures

These are complete, small standard-library Python applications, with public
training/development tasks and a separate black-box grader. They are benchmark
fixtures, not production applications. No final held-out task appears here.
No model calls are made by these files.

| Project directory | Training | Development |
| --- | --- | --- |
| `incident-report-cli` | Configurable top-message count | UTF-8 input-line byte limit |
| `durable-task-queue` | Reject non-finite time before DB effects | Purge completed tasks |
| `dependency-flow-runner` | Validate duplicate dependencies before execution | Fail-fast scheduling |

The queue's starting revision deliberately omits some finite-number checks; the
workflow starting revision deliberately omits duplicate-dependency rejection.
These are stated contracts missing regression coverage, not secret requirements.
The reporting training task adds a feature. All starting public suites pass.
The training graders must fail on these original seeds.

1. Read `pilot.json` and copy only its selected project directory into an isolated
   candidate workspace. Keep `grade.py`, `tasks/`, this manifest, any run state,
   and final holdout material outside the candidate. Record source-file hashes.
2. Run the project suite: `python3 -m unittest discover -s tests -v`.
3. Give the agent the corresponding `tasks/*-train.md` prompt and the editing
   policy below. Existing tests and test execution/discovery configuration are
   protected; adding new regression test files and updating ordinary project
   documentation are permitted. The harness checks protected-file hashes and
   original test execution; documentation cannot redefine acceptance criteria.
4. Grade from this directory: `python3 grade.py --workspace /absolute/candidate
   --task report-train` (substitute task ID). Exit 0 means every baseline and
   feature check passed; the final stdout line is a compact JSON result. Detailed
   test output goes to stderr. A valid feature must pass its behavior checks,
   not just exit successfully or print a canned success message.
5. Review/freeze the successful post-training source tree and copy that same
   tree into every condition, including the no-memory baseline. Run development
   tasks in new sessions with isolated state snapshots. A dev grader also reruns
   its training checks to detect regression. Never carry one dev task's changes
   or learned records into another condition or final held-out attempt.

The grader launches candidate CLI subprocesses and reads their outputs; it does
not import candidate Python into the supervisor. Its fixtures live in a separate
temporary directory, and candidate children receive a reduced environment with
a temporary TMPDIR (HOME is not forwarded or reset). This does not sandbox deliberately hostile Python; execute
the whole evaluation in the benchmark's normal isolation boundary and keep
credentials inaccessible. The grader never loads model configuration.

Protected-file hash checks, daemon isolation, model metering, training/retrieval
costs, frozen sampling, and the three comparison conditions belong to the outer
runner. Passing this pilot is functional verification, not a claim of learning
advantage. Only the sealed final protocol can supply confirmatory results.

Shared task editing policy: update project source and documentation as needed;
preserve existing tests and test execution/discovery configuration; add tests in
new regression files without suppressing or modifying original test execution;
do not write outside the candidate workspace. Keep the original specification
snapshot and immutable task/grader contracts outside the workspace. Test-path
deletions, symlink replacements, or new configuration/bootstrap files that
change the original suite are prohibited. Apply this declared policy equally to
every arm and retain any earlier pilot rejection under its original policy.

`check-fixtures.py` additionally checks that its own narrow reference edits leave
the seed documentation unchanged. This is a self-test of those reference edits,
not a prohibition on candidate documentation updates.
