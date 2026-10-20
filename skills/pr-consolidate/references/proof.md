# Proof Assessment

Apply installed `proof-integrity-review` when verification mechanisms change. Inspect the semantic
diff and actual invocation paths; filenames or a classifier do not establish applicability alone.
Use the current project's relevant native checks and an independent reviewer on the exact inputs.

For modified oracles and material guarantees, retain the representative fault, the designated
check's observed rejection, restored acceptance and execution environment. A compile error,
timeout, crash, missing discovery or unrelated failure does not demonstrate target detection.
A surviving fault leaves that guarantee unproven. Distinguish supplied evidence from execution
observed in this session.

The skill does not ship a runtime-specific witness executor. Use the project's supported tests,
database and environment, or the maintained proof capability's qualified interfaces. This avoids
making one package manager or test runner a requirement for every target repository. Missing
essential evidence remains a blocking limit; a generic command plan is not a substitute oracle.

## Optional structured record

The default audit needs no receipt. When the user or established workflow explicitly requires a
structured record, follow the installed skill's `references/cli.md`. Resolve its real package and
trusted policy root and keep outputs outside candidate and policy roots:

```sh
bun "$PROOF_PACKAGE_ROOT/bin/proof-integrity.ts" epoch --repository "$REPOSITORY" --base "$BASE" --head "$HEAD" --policy-root "$PROOF_SKILL_ROOT" --output "$EPOCH"
```

Changed inputs require fresh bindings. Never hand-author a current epoch or rename an old
receipt's fields. A missing required companion or rejected receipt leaves the structured path
incomplete. Acceptance validates its declared bindings, not execution authenticity, reviewer
independence or permission to merge.
