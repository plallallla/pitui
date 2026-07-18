## Summary

<!-- What changed and why? -->

## Safety impact

<!-- List Git commands added/changed, mutation scope, confirmations, rollback, and repository routing. Write "None" when not applicable. -->

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] `cargo test --workspace --doc`
- [ ] New behavior has focused tests, or the reason tests are unnecessary is explained.
- [ ] Documentation and hotkey help are updated.
- [ ] Git output remains terminal-sanitized and commands use argv rather than a shell string.

## Vibe-coding disclosure

<!-- Describe any AI-generated code/docs and the human review or verification performed. -->
