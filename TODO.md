# Follow-ups

- Refactor the runtime helpers to Rust, removing the production Python dependency while preserving
  repository discovery, primary-branch selection, forge behavior, review ownership and recovery,
  Git safeguards, project command execution and relevant test evidence.
- Publish an exact-version npm package when distribution is ready.
- Replace the local preview with the published version through Bun and the canonical harness
  manifest, retaining the Claude, Codex and Cursor projections.
- Qualify a complete campaign with isolated project services and real forge operations;
  local fixture tests do not cover that assembled boundary.
