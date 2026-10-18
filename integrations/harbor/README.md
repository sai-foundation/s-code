# Harbor adapter

This integration lets Harbor run S-Code reproducibly on managed tasks such as
TBLite and Terminal-Bench.

The initial adapter targets Harbor 0.17.0 and installs a checksum-verified
Linux S-Code runtime bundle into the task container. Each Harbor trial runs one
complete unattended S-Code agent attempt with isolated state and preserves its
raw events, standard error, final output and provenance in Harbor's agent logs.

## Initial scope

- Linux x86-64 with glibc.
- One complete S-Code run per Harbor trial.
- Explicit model endpoint configuration through an environment credential
  handle.
- Full S-Code permission mode inside Harbor's disposable task container;
  Harbor remains the outer isolation and network boundary.
- Adapter, bundle-validation and deterministic smoke tests.

Retries, session resumption, persistent memory and ATIF conversion are outside
this initial integration.
