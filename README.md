# PR Consolidate

A skill for consolidating coupled pull requests from the current repository. It detects the
remote's primary branch, reviews each original PR and the aggregate, and uses the project's own
preparation and verification commands. Every source receives an individual head-specific review
before authorized replacement after merge verification.

## Usage

From the target checkout:

```text
$pr-consolidate Consolidate PRs <IDs> into one reviewed batch.
```

Read [the workflow](skills/pr-consolidate/SKILL.md). Git and the native Rust executable run the
helpers; production requires neither Python nor a shell interpreter. GitHub and Bitbucket Cloud use their authenticated
`gh` and `bkt` CLIs. Credentials remain with those CLIs and the existing environment. Project
runtimes, services and checks are discovered from the target repository.

The harness supplies `code-review`, `tdd` and `proof-integrity-review` for their respective phases;
they remain external capabilities.

## Development

Requires Rust 1.85+, Git and Bun 1.4.3:

```sh
bun run test
bun run check
```

`test` builds the native executables, runs Rust behavioral tests and exercises the public CLI
against temporary repositories and native Rust substitute forge CLIs. Cargo builds the fixture
executable only with the `test-fixtures` feature, enabled by `bun run test`; Python is not
required by development, CI or the distribution. Tests still exercise POSIX shell behavior and
native Git hooks. `check` runs native Rust formatting and
Clippy. No test performs real forge writes. See [the parity map](integration/rust-parity.md) for
retained boundaries and negative witnesses.

## Build and installation

```sh
bun run build
bun pm pack --ignore-scripts --destination /tmp/pr-consolidate-package
```

Build on macOS or Linux for the current host. The archive contains the built platform runtime and native entrypoints,
the Rust source and locked dependencies; another platform can explicitly run `bun run build` in
the installed package root. No installation script downloads or runs code automatically. After
building, helpers need neither Rust, Python nor a shell interpreter. The package has no npm runtime dependencies.

Use `<installed-skill>/scripts/forge` for forge operations and the existing `lot-*.sh` entrypoints
for campaign operations. The `.sh` names are retained for compatibility but are native binaries; invoke them directly. See [harness integration](integration/README.md) for isolated consumer
validation and Claude, Codex and Cursor projections. Local previews can be activated independently of npm publication. Remaining distribution work
is listed in [TODO.md](TODO.md).

Supported build hosts are macOS ARM64/x64 and Linux ARM64/x64 with glibc. A platform-specific build
or fixture run does not establish another host's behavior. Fixtures do not qualify real forge
writes, model activation or a complete campaign with assembled project services.
