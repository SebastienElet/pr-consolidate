# Harness Integration

## Versioned acquisition

The canonical source is this repository. User installations consume the packaged distribution,
not a symlink into a development checkout. Build a local review artifact with Bun:

```sh
bun pm pack --ignore-scripts --destination /tmp/pr-consolidate-package
```

Inspect and test the tarball before publication. It contains the skill, references, scripts and
tests; it has no npm runtime dependencies or installation scripts. Local tarball installation in
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

The current helpers use Python and Git. Bun acquires and packages this skill; target repositories
choose their own runtimes and checks. Database, migration and container setup remain with their
supported project commands. Local fixtures do not qualify a real service write or assembled
project environment.
