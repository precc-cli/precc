# Show HN: precc — rewrite an LLM agent's bash commands before they fail

**Title (≤80 chars):** Show HN: Precc – rewrite an LLM agent's bash commands before they fail

**URL:** https://github.com/precc-cli/precc

**Text (the optional body; Show HN posts can include one):**

I kept watching Claude Code burn turns on the same kind of mistakes: it would run `cargo build` from a notes subdirectory, get an error, read the error, run `cd ../.. && cargo build`. Same loop for `git status` outside a repo, `make` without a Makefile, `npm test` outside a Node project. Every miss costs a round trip — model latency, output tokens, attention budget.

Precc is a pre-execution hook that rewrites the command before it runs. It plugs into Claude Code's `PreToolUse:Bash`, Cursor's `beforeShellExecution`, and (because Aider has no hook surface) a thin `$SHELL=` wrapper. Same pipeline for all three:

1. Match against built-in skills (12 wrong-dir patterns: cargo, git, go, make, npm/yarn/pnpm, python, gradle, maven, bazel, docker-compose, terraform, just).
2. Resolve project root by walking up from cwd, prepend the right `cd`.
3. Optionally hand the command to a configured output compressor (one adapter ships: `rtk`, opt-in).

So `cargo build` from a subdirectory becomes `cd <project-root> && cargo build`, and Claude's turn succeeds instead of failing.

A few design notes that might be of interest:

- **Hot-path latency.** Measured with `hyperfine` over 2,699 runs on the rewrite path: **1.1 ms mean / 2.6 ms max**. DB writes for activation counts are deferred — the hook just appends a JSON line to `activations.log`; measurement commands replay the log into SQLite on read. No background daemon.
- **Cursor's hook protocol allows/denies but cannot rewrite**, so for Cursor we deny + surface the corrected command via `agent_message`, and the agent re-runs it. Documented in the README.
- **Compressors are pluggable** via a `Compressor` trait. Default is `none` (no wrapping). RTK is one adapter. The hook stays compressor-agnostic.
- Pure Rust workspace, dual MIT/Apache, no telemetry, all data stays in `~/.local/share/precc/`.

Install:

```bash
cargo install precc-cli precc-hook
precc init
# then add the hook to ~/.claude/settings.json (precc init prints the snippet)
```

I'd love feedback on: (a) which trigger patterns I missed, (b) whether the "deny + suggest" workaround for Cursor is acceptable or whether I should lobby for a real rewrite field, (c) whether the input-shaping framing (as opposed to output-compressing tools like Aider's `--map-tokens` or RTK itself) clicks.

Repo: https://github.com/precc-cli/precc
Release notes: https://github.com/precc-cli/precc/releases/tag/v0.1.0
