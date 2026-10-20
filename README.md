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

Read [the workflow](skills/pr-consolidate/SKILL.md). The canonical source, helpers and tests live
under `skills/pr-consolidate/`. The harness supplies `code-review`, `tdd` and
`proof-integrity-review` for their respective phases; they remain external capabilities.

Git and Python 3.11+ run the local helpers. Project runtimes, database setup, migration commands
and forge access are discovered from that repository. GitHub and Bitbucket Cloud are supported
through their authenticated `gh` and `bkt` CLIs. Bun 1.4.3 manages this skill's distribution;
it is not imposed on target projects.

## Development and installation

```sh
bun run test
```

Tests use temporary Git repositories, project commands and service substitutes. They do not
qualify a real assembled application, remote write or unexercised host. See
[harness integration](integration/README.md) for versioned local preview installation and later
npm consumption. Follow-ups are tracked in [TODO.md](TODO.md).
