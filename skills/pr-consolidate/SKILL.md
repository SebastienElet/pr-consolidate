---
name: pr-consolidate
description: >
  Consolidate coupled pull requests from the current repository into a reviewed batch. Use when
  asked to combine several PRs into one batch or mega PR. Detect the repository and its primary
  branch, review every source and the aggregate, and publish head-specific comments when
  authorized. Excludes single-PR reviews and cleanup without an agreed consolidation scope.
compatibility: Requires Git and the packaged Rust executable for macOS or Linux; project tools and an authenticated forge CLI depend on the current repository.
metadata:
  category: dev
---

# PR Consolidate

## Overview

Consolidate an agreed group of PRs from the current Git repository onto its remote's primary
branch. Keep source commits, review each source and the aggregate, and publish an individual
review before closing any source as replaced. Project commands own builds, databases and migrations.

## Usage

Invoke `$pr-consolidate` or `/pr-consolidate` from the target repository. A nested working directory
selects its enclosing checkout. Never select a repository from the skill's installation directory.
Use an explicit repository path when the user names another checkout. The current feature branch
does not determine the merge target: detect the primary branch from the remote, without guessing
`main` or another conventional name. Missing or ambiguous context stops the affected operation.

Required harness skills are `code-review` for source and aggregate reviews, `tdd` for owned fixes,
and `proof-integrity-review` when verification changes. Check availability before each required
phase; never bundle these external capabilities. This workflow does not compose `pr-verdict`.
Use `<installed-skill>/scripts/forge context` from the target checkout to inspect
GitHub or Bitbucket Cloud context. The packaged entrypoints are native executables; invoke them
directly, including the compatibility names ending in `.sh`. Use the detected forge's supported CLI within the user's existing authorization. An inspection
request does not authorize publication, merge or replacement; reuse authority already given.

## Workflow

1. **Discover and freeze.** Inspect the current checkout's instructions, remotes, primary branch,
   PRs and native development commands. Refresh remote context before writing. Record repository
   identity, primary branch, full base and source heads, agreed grouping, ownership, budget and
   stopping conditions in a campaign-local plan. Separate drafts and changes-requested PRs.
   Group by actual contract coupling; a shared lockfile alone is insufficient. Moved inputs
   require reassessment rather than reuse of an old verdict.
2. **Plan project checks.** Read the project's AGENTS, ADRs in force, documented tasks and existing
   CI. Select its supported preparation, build, lockfile and verification commands. Do not invent
   a framework, database, package manager or migration layout. Fill `lot.json` using
   `references/lot.example.json` and `references/project-checks.md`. Inspect each command's real
   resource effects before execution: an isolated clone does not isolate a database or service.
   If safe project setup is unavailable, stop that phase and name the missing capability.
3. **Prepare owned clones.** The campaign owns root, source, push clone and artifacts. The bundle
   importer writes refs into source, so source must not be the shared checkout. Keep artifacts
   outside the candidate. Run the lot bundle and setup helpers with the pinned context. Preserve
   the original remote URL in campaign clones and reject repository or primary-branch mismatch.
   Use only the project's supported authentication and setup; credentials stay in the environment.
4. **Review every source.** Use `code-review` on each original full head and its actual base,
   including a stacked parent where applicable. Read the complete diff and applicable decisions.
   Prepare a source-specific review from `references/templates.md`, distinguishing checks executed
   on that source from aggregate evidence. Leave source branches unchanged. Return material
   product or architecture decisions outside delegated authority to the user; continue independent work.
5. **Integrate and correct.** Branch from the pinned primary base. Merge source heads, parents
   before stacked children, retaining author commits and describing conflict resolutions. Resolve
   unions without losing values. Rebuild only declared lockfiles with the project's command and
   verify the result. Use `tdd` for owned fixes. Build where the plan requires it, run normal
   repository hooks, and commit with the configured human identity. Refresh the base when it
   advances; a conflict outside declared lockfiles stops automatic base integration.
6. **Review and verify the aggregate.** Freeze full base, head and tree. Use independent read-only
   `code-review` reviewers on the complete aggregate; add architecture, security or privacy
   coverage where relevant. Confirm significant findings in source. After fixes review the delta
   and complete final candidate. Execute the planned project checks and report actual selections,
   results and environments. An absent required check, command failure or timeout is incomplete
   evidence. Apply `proof-integrity-review` to changed verification, following `references/proof.md`.
7. **Publish reviews and batch.** Within authorization, recheck repository, primary branch and
   current inputs, push the batch without force and create its PR against the detected target.
   Check CI against the exact head. Publish one review for every source and the batch: full
   reviewed head, findings, aggregate corrections, batch link, checks and limits. Verify the
   account and complete comments before writing; the same account/PR/head reuses its exact
   marked review, while a new head preserves the older assessment. Reconcile ambiguous writes
   before retrying. A comment alone does not establish native approval or a mechanical merge block.
8. **Verify and replace.** Queue or merge only within explicit authority. Verify the final merge
   on the primary branch, including the candidate and resulting tree. Before closing sources,
   verify their expected heads and ancestry plus their individual published reviews. Preview the
   replacement notices, then apply within existing authority. Keep reviews and replacement
   notices separate. Resume partial success only for sources whose head and exact owned
   replacement record still match; never treat an arbitrary closed PR as prior success.

Return source and batch links, reviewed heads, actual checks and CI, review links, merge
verification, replacement states and evidence limits. Execute remaining authorized steps;
ask only for an indispensable unresolved decision.

## Gotchas

- **Installed directory mistaken for context** — the wrong project is selected. Invoke helpers
  from the intended checkout or supply its explicit path; verify the discovered remote.
- **A familiar primary-branch name guessed** — a batch targets the wrong branch. Resolve the
  remote's actual primary branch and refuse missing or conflicting information.
- **Project checks inherited from another repository** — tools or shared resources are misused.
  Discover the project's own commands and resource isolation before running them.
- **A replacement notice mistaken for review** — authors lose findings and scope. Publish each
  source review before replacement, including sources with no demonstrated defect.
- **A moved head or ambiguous comment ignored** — an assessment may overwrite unrelated history.
  Re-anchor, verify ownership and exact markers, and preserve prior-head reviews.

## Constraints

- Never impose a repository, primary branch, package manager, database or migration framework.
- Never mutate source branches, force-push, self-approve or fabricate reviewer evidence.
- Never publish, merge or close outside established authority; reuse authority already given.
- Never report checks against inputs or environments different from those actually exercised.
- Never replace a source before batch merge verification and publication of its own review.
- Never infer resource isolation from a clone, a plan declaration or green fixture tests.
- Never persist or print credentials; use the project's supported environment-based authentication.

## References

- [references/project-checks.md](references/project-checks.md) — project command contract
- [references/lot.example.json](references/lot.example.json) — illustrative campaign configuration
- [references/templates.md](references/templates.md) — descriptions and individual reviews
- [references/proof.md](references/proof.md) — evidence and proof assessment
- [references/pitfalls.md](references/pitfalls.md) — recovery boundaries
