# Rust Runtime Parity

The reference implementation is commit `39e717742a11f55a1495517dbcc37ea2eaf7b2ec`.
Its 119 Python tests passed on macOS ARM64 before migration. The reference commit's Ubuntu and
macOS CI jobs also passed. Those results describe that commit, not the Rust candidate.

## Runtime and CLI

The Rust runtime executable owns repository discovery, campaign Git operations, native utilities,
project command execution and forge orchestration. GitHub translation and native CLI transport
remain separate from account ownership, review publication and replacement sequencing.

| Previous entrypoint                          | Native entrypoint                            |
| -------------------------------------------- | -------------------------------------------- |
| `python3 scripts/forge.py ...`               | `scripts/forge ...`                          |
| `scripts/lot-<operation>.sh ...`             | Same names, native executables               |
| `python3 scripts/lot_cli.py <operation> ...` | `scripts/pr-consolidate lot <operation> ...` |
| `python3 scripts/lotenv.py ...`              | `scripts/lotenv ...`                         |
| `python3 scripts/project_commands.py ...`    | `scripts/project-commands ...`               |
| `python3 scripts/native.py ...`              | `scripts/native ...`                         |

Forge options and operations, including `--repository`, `--out`, `--check`, explicit review and
replacement modes and the `decline` alias retain their meaning. Exit codes remain success `0`,
forge/tool failure `1`, refusal/usage `2`, forge timeout `3`; project commands preserve their own
nonzero status and use `124` for timeout. Console diagnostics describe the same operations;
JSON context and inventory fields, review markers and replacement notice bodies retain their
contracts. Python imports were internal interfaces and are removed.

The package build and all launcher logic are also Rust. `bun run build` runs the Rust build utility,
which installs a native launcher for each retained entrypoint name. Even `lot-*.sh` executes
directly without Bash. Generated executables are build artifacts rather than tracked shell files.
The archive contains regular native files because Bun does not pack symbolic-link entrypoints.

## Evidence migration

The original reference tests included 28 subprocess cases and 91 cases that imported or patched Python
modules. All test suites are now Rust, with native substitutes for the retired Python fixtures.
Existing POSIX shell and native Git hook scenarios remain. Campaign fixtures use an isolated user configuration with
Git automatic maintenance in the foreground, so temporary repository cleanup does not race a
detached maintenance process; production Git configuration remains project-owned. The internal cases are replaced with Rust tests
and CLI integration tests at the boundaries below, rather than keeping a second Python runtime.

| Boundary and former suites                             | Current oracle                                                                                   | Retained negative witnesses                                                                                                                                                                  |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Git context, `test_git_context.py`                     | `runtime/tests/context_cli.rs`, Rust context/transport tests                                              | Ambiguous or credential-bearing origin, foreign push URL, padded/encoded/traversing URL, unresolved remote HEAD, Git environment redirection                                                 |
| Campaign, native, preflight, primary and process tests | `runtime/tests/campaign_cli.rs` and `distribution_cli.rs` through native entrypoints                                                   | Moved refs, primary change, shared Git metadata, staged conflict including newline path, dirty tree, failing build/hook, agent identity, non-fast-forward push, undeclared lockfile conflict |
| Project commands, `test_project_commands.py`           | `runtime/tests/project_cli.rs` and `campaign_cli.rs`, Rust project/process tests                                                    | Literal argv, missing/malformed phase, boolean timeout, absent executable, nonzero propagation, registry secret in argv/logs, escaped child retaining stdout                                 |
| Forge transport and GitHub adapter                     | Rust transport/adapter tests, `runtime/tests/forge_cli.rs` with native executable substitutes                    | Foreign pagination, incomplete counts/files, repeated IDs, malformed payloads, wrong check SHA, private CLI output, missing CLI, bounded timeout                                             |
| Review publication                                     | Rust review tests and native forge CLI cases                                                     | Wrong account/head/surface, misplaced or duplicate marker, inline/reply, deleted content, pending/dismissed review, empty/approval body, unobserved write, post-write head movement          |
| Replacement                                            | Rust replacement tests and native GitHub CLI replacement                                         | Missing source review, wrong target/ancestry/full planned head, unqualified closed source, unobserved notice, moved head, foreign or duplicate notice                                        |
| Inventory, description and creation                    | Rust forge tests, native GitHub/Bitbucket CLI cases                                              | Incomplete listing, existing PR, changed expected description, draft/changes-requested grouping, lockfile-only coupling                                                                      |
| CI and merge verification                              | Rust CI tests with deterministic clock, `runtime/tests/merge_cli.rs` with real temporary Git histories | Empty/all-skipped or failed CI, merge-queue confusion, wrong SHA/destination, foreign clone, incorrect squash tree, agent merge identity, candidate mismatch                                 |

New recovery witnesses also simulate accepted writes followed by transport failure or timeout,
then reconcile before replay. They exercise owned decisions against substitutes; they do not
establish a real forge's acceptance or eventual consistency behavior.

## Native fixture and test entrypoints

`bun run test` builds the packaged aliases and runs Cargo with `test-fixtures`. The six integration
suites in `runtime/tests/` execute those aliases. Cargo compiles a single development-only fixture
executable from `runtime/tests/support/fixture.rs`; it is not installed among production helpers.
The fixture supplies literal project commands, descendants with inherited output, a substitute
Cargo exit status and native `gh`/`bkt` executables. Forge state consists of explicit JSON resources
and recorded requests, rather than importing the production implementation. Git repositories,
Git hooks, merge trees, process groups and installed native images remain real at these boundaries.

The last Python CLI suites at merge commit `46fcd31706919b95e41b36345662b866d6104e5d` had 57 tests.
Their scenarios are regrouped by boundary rather than preserving test counts or Python fixture
structure. All 53 existing Rust tests remain. Context cases also change the local branch to prove
that it does not choose the remote primary. Distribution coverage includes absent runtimes,
unknown aliases, empty-PATH invocation, evidence tampering, incomplete builds preserving existing
files and replacement of a running native image. Process coverage retains escaped output-holding
children, late candidate mutation, complete child logs and inherited stdin. An independent outer
watchdog bounds fixture invocations; the production process collector is not its own timeout oracle.

The Cargo feature is enabled by both the development test command and Clippy. The package includes
the integration test and fixture sources, so an isolated acquired consumer can rebuild and run the
same checks. Neither development commands, CI nor the acquired package execute Python. Historical
Python names in this map identify retired evidence only; project-owned commands and Git hooks
still choose their own external tools.

## Limits

Commands and native Git hooks can affect resources outside a clone; campaign configuration is
not proof of service isolation. The executable preserves the existing supported environment
contract and redaction of the inherited registry token, rather than claiming generic secret
sanitization of arbitrary project commands. Review prose is advisory and never native approval.

Run `bun run test` and `bun run check` on each host under assessment. The existing CI matrix
continues to cover macOS and Ubuntu. Packaging and isolated projection checks establish archive
contents and resolved links, not actual model invocation, real forge writes or an assembled
project campaign. Keep those qualifications separate in the delivery note.
