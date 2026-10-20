# Batch and Review Templates

Publish in the PR's language. Replace placeholders with observed facts or explicit limits. Write
the UTF-8 body to a file; the publisher adds its own PR/head marker. Never imply native approval.

## Batch description

```markdown
## Why

These PRs share <contracts or integration surfaces>:

- #<source> (<author>): <behavior and ticket>; original head `<full-sha>`.
<Excluded PRs and decisions explaining exclusion.>

## Result

- Source commits retained; <conflict resolutions and integration fixes>.
- <Observable behavior and migration consequences>.

## Review and evidence

- Primary base: `<full-sha>`; candidate: `<full-sha>`, tree: `<tree>`.
- Source reviews: <PR, full reviewed head and comment URL for each>.
- Aggregate review: <reviewers, full head, scope, findings and limits>.
- Preflight: <command, exact head, environment, actual gates and E2E selection>.
- CI: <pipeline URL, full head, outcome and unexecuted checks>.
- Relevant negative witnesses: <fault, oracle, rejection and restored acceptance>.
- Proof assessment where applicable: <actual verdict and limits; no invented receipt>.

## Follow-ups and replacement

<Finding, originating PR, owner decision and impact on the batch.>
The skill does not request source-branch deletion. GitHub repository settings may delete the
merged batch branch independently. After merge verification, the authorized replacement operation will
close <source PRs>. Their reviews and separate replacement notices remain visible.
```

## Every source PR

Prepare one distinct body per original head, including PRs with no finding. Attribute aggregate
corrections and execution to the aggregate candidate, rather than the original source head.

```markdown
### Technical review

**Scope** — #<source>, head `<full-source-sha>`, base `<full-base-sha>`.
Reviewed for consolidation into <batch PR URL or proposed batch identity>.

**Findings** — <severity, file:line, concrete failure and resolution; or no defect demonstrated
within this scope>.

**Integration** — <retained behavior, fixes in the batch and their candidate SHA>.
<Outstanding source-specific decisions and follow-ups.>

**Evidence** — <checks actually executed on this source, supplied evidence, environment and limits>.
Aggregate evidence: <separately identified candidate and links>.

**Assessment** — <favorable within scope / corrections required / essential evidence missing>.
This comment is advisory; it grants no native approval or mechanical merge block.
```

```sh
python3 <installed-skill>/scripts/forge.py review-comment <source-pr> <full-source-head> <body-file> dry-run
python3 <installed-skill>/scripts/forge.py review-comment <source-pr> <full-source-head> <body-file> apply
```

`dry-run` is read-only. `apply` requires publication authority. A same-head reassessment preserves
previous conclusions and adds a dated explanation of new evidence. For an excluded PR, identify
the demonstrated problem and needed decision; do not imply a native changes-requested state.

## Aggregate review

Use the same publisher with the batch's own head and a body covering the complete base-to-candidate
diff, cross-PR interactions, independent reviewers, fixes, actual checks and limits. Include links
to every source review. Merge and replacement retain their own authorization and checks.

## Replacement plan

After merge verification pass `forge.py replace` a plan with `lot_pr` and `items`, each containing
`id`, the original expected `head`, and optional source-specific `extra` text. Preview first.
The generated ancestry/replacement notice supplements the earlier source review.
