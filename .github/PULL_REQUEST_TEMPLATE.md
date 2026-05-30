## Summary

<!-- What does this change do, and why? -->

## Type

<!-- delete those that don't apply -->
- New builtin skill
- Bug fix
- Feature / enhancement
- Docs / chore

## Checklist

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] If a new builtin skill: TOML added under `crates/precc-cli/skills/builtin/` **and** registered in `BUILTIN_SKILLS` (`crates/precc-cli/src/embedded_skills.rs`)
- [ ] New/changed matching logic has a test
