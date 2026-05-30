# Contributing to PRECC

Thanks for helping improve **PRECC** — predictive error correction that keeps
coding agents in flow. `precc-core` and the CLI/hook crates are the open shared
layer; contributions here benefit every PRECC user.

## Dev setup

Stable Rust (see `rust-toolchain` if present). Then:

```bash
cargo build --workspace
```

## Checks (must pass — CI enforces all four)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # warnings are errors
cargo test --workspace --all-targets
```

Run them before opening a PR; CI runs the same on Linux + macOS.

## Great first contribution: a "wrong-dir" skill

The highest-leverage, lowest-friction PR is teaching PRECC a new build tool's
"you're in the wrong directory" fix. It's two steps:

1. **Add the skill TOML** at `crates/precc-cli/skills/builtin/<tool>-wrong-dir.toml`:

   ```toml
   [skill]
   name = "<tool>-wrong-dir"
   description = "Prepend cd to <tool> commands run outside the project root"
   source = "builtin"
   priority = 100

   [[triggers]]
   type = "command_regex"
   pattern = '^<tool>\s'        # e.g. ^bazel\s
   weight = 1.0

   [[triggers]]
   type = "file_exists"
   pattern = "<marker-file>"    # e.g. WORKSPACE, pom.xml, package.json

   [[actions]]
   type = "prepend_cd"
   template = "cd {{project_root}} && {{original_command}}"
   confidence = 0.9
   ```

2. **Register it** in `crates/precc-cli/src/embedded_skills.rs` — add a
   `(filename, include_str!(...))` entry to `BUILTIN_SKILLS`.

Add a test if you touch matching logic. Look for issues labelled
**`good first issue`** — most are exactly this shape.

## PR guidelines

- Keep PRs small and focused; one concern each.
- Conventional-commit-style titles (`feat:`, `fix:`, `perf:`, `docs:`) are appreciated.
- Green CI is required to merge.

## License

By contributing, you agree your work is dual-licensed under **MIT OR
Apache-2.0**, matching the project.
