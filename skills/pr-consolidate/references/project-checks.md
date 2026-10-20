# Project Command Contract

Discover commands from the current project's instructions, documented CLI and CI. Do not infer a
database from a familiar directory, or replace a native command with a new validation framework.
Use command argument arrays; no shell interpolation is performed by the runner. A deliberate
shell command is still code whose effects must be reviewed before execution.

## Campaign configuration

`lot.json` records `base_branch`, full `base_sha`, `remote_url`, campaign-owned clone paths,
human identity and source PR heads. Populate context from current Git discovery, not from this
example. The batch branch must differ from its primary branch and every included source branch.

`commands` contains phases `prepare`, `build`, `check`, `reinstall` and `lockfile`. Each command
has `argv` and `timeout_seconds`. Build is explicitly empty when the project has no build step;
do not add a dummy successful command. Checks must exercise the changed behavior. Invoking a
required phase without a configured command is an error, not evidence that the project passed.

Declare actual `lockfiles` relative to the candidate checkout. Reconstruct them from the pinned
base and run the selected native resolver/check. Never assume a filename or package manager.
Set optional `hooks_path` only when the project's documented setup requires it; ordinary Git
hook behavior remains enabled. Project setup can install its own hooks.

## Resources and credentials

Before running preparation or checks, trace their database, filesystem, service and container
effects. Use the project's supported isolated environment and its actual configuration. There
is no generic automatic database creation, certificate copy, migration invocation or container
lifecycle. If no safe native setup is available, stop that phase and report the gap.

Commands run in the campaign candidate checkout, with process timeouts and failure propagation.
Pass credentials through the existing process environment, never configuration literals or
command arguments. The runner cannot prove a project's scripts preserve every secret or resource;
inspect those scripts and keep that distinction in the evidence report.

## Guarantees migrated from a project-specific runner

| Concern                             | Current owner and evidence                                                                       |
| ----------------------------------- | ------------------------------------------------------------------------------------------------ |
| Source/base heads and integration   | Git helpers, exact refs and temporary-repository integration tests                               |
| Build and hooks                     | Configured project build and normal Git commit; executed hook evidence                           |
| Lockfile resolution                 | Declared files and the project's native resolver/check                                           |
| Database, migrations and containers | Project-supported setup and behavioral integration checks                                        |
| Negative witnesses                  | The relevant project oracle under a representative fault, assessed with `proof-integrity-review` |
| Service publication                 | The selected forge's supported interface, current account/head and observed remote state         |

A successful local orchestration test does not prove any project service boundary. Do not retain
the former runner's environment assumptions or its historical gate count as current evidence.
