# Harness Integration

## Versioned acquisition

The canonical source is this repository. User installations consume the packaged distribution,
not a symlink into a development checkout. Build a local review artifact with Bun:

```sh
bun run build
bun pm pack --ignore-scripts --destination /tmp/pr-consolidate-package
```

Inspect and test the tarball before publication. It contains the skill, references, native entrypoints, tests, Rust source, Cargo lockfile and
the current host executable; it has no npm runtime dependencies or installation scripts.
`bun run build` requires Rust 1.85+ and Cargo registry access for uncached locked dependencies.
The Rust build utility compiles the runtime and launcher for its host, prepares all executables,
and replaces each executable atomically. The complete installation is not a single transaction.
It writes `skills/pr-consolidate/bin/<platform>/pr-consolidate` and the native launcher, where platform is
`darwin-arm64`, `darwin-x64`, `linux-arm64` or `linux-x64` (glibc). Build separately on each
platform before packaging a matching artifact, or explicitly build inside the acquired package
when its platform executable is absent. It generates executable aliases under `skills/pr-consolidate/scripts/` for the existing command names,
including `lot-*.sh`. These are native binaries, not shell scripts. The launcher refuses a missing
runtime; an archive must be built for the consuming platform before its entrypoints can run. Local tarball installation in
a temporary consumer exercises package layout without publishing or changing the active harness.

For persistent dotfiles integration, the exact package version must first be available from its
authorized distribution. Publication is a separate externally visible action. Once available,
add it through Bun in the consumer checkout:

```sh
bun add --exact @sebastienelet/pr-consolidate@0.0.2
```

Commit the consumer's updated `package.json` and `bun.lock` together. Do not substitute a path to
`~/Code/pr-consolidate` or deploy a link from a temporary worktree.

## Arnes declaration

Add this entry under `skills` in the consumer's `home/.arnes.yaml`:

```yaml
- slug: pr-consolidate
  source: node_modules/@sebastienelet/pr-consolidate/skills/pr-consolidate
  installations:
    - { agent: claude, scope: user }
    - { agent: cursor, scope: user }
    - { agent: codex, scope: user }
```

The existing `harness:claude-skills`, `harness:cursor-skills` and `harness:codex-skills` tasks own
projections. Run them from the canonical consumer checkout with its documented deployment
procedure. Reject divergent existing destinations; inspect them before any explicit repair.

Validate first with the native `tooling/install-agent-skills.ts` installer, a temporary consumer
manifest and isolated destination directories. Run twice for each host and inspect each leaf link
and its resolved `SKILL.md`. This proves projection mechanics and package contents, not actual
model activation or a successful consolidation campaign.

## Local packaged preview

When installation is needed before publication, retain the tarball and a dedicated Bun consumer
under `~/.local/share/pr-consolidate/<version-and-source-tree>/`. Acquire that local tarball with
Bun, keeping its archive and lockfile there. This is a distribution artifact, not a link to the
development checkout or a temporary worktree.

Give this consumer its own `home/.arnes.yaml` containing the skill entry above and use the existing
installer with that consumer root and the actual home directory for each selected agent. Inspect
resolved leaf links after deployment. Future npm integration replaces these links deliberately;
the collision check must not be bypassed. Keep the local consumer while any live link targets it.

## Dependencies and limits

Confirm that the host discovers `code-review`, `tdd` and `proof-integrity-review` for their phases.
Forge access uses its authenticated native CLI; credentials remain with that CLI. `pr-verdict`
retains its explicit invocation contract and is not composed by this workflow.

The build utility, launchers and helpers are implemented in Rust; invocation uses native executables and Git. Python 3.11+ is used only by development
fixtures. Bun acquires and packages this skill; target repositories choose their own runtimes and checks. Database, migration and container setup remain with their
supported project commands. Local fixtures do not qualify a real service write or assembled
project environment.
