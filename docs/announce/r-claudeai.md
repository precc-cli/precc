# r/ClaudeAI — precc

**Title:** precc — a hook that stops Claude Code from wasting turns on `cargo build` in the wrong dir

**Flair:** `Tools` or `Showcase` (subreddit-dependent)

**Body:**

Quick share for anyone using Claude Code: I got tired of watching Claude run `cargo build` from a notes subdirectory, hit an error, then `cd` and retry — every time costing a turn. So I wrote **precc**, a `PreToolUse:Bash` hook that rewrites the command before it runs.

Repo: https://github.com/precc-cli/precc

**What it catches today** (12 built-in patterns):

- `cargo` outside a Rust project → prepends `cd <project root>`
- Same for `git`, `go`, `make`, `npm`/`yarn`/`pnpm`, `python`/`pytest`, `gradle`, `maven`, `bazel`, `docker compose`, `terraform`, `just`

The hook sees JSON on stdin, returns JSON with the rewritten command; Claude executes the corrected command instead of the wrong one. No round-trip wasted, no model context spent on the error. Hook latency is ~1 ms (measured) so you won't notice it.

**Install** (Rust toolchain required):

```bash
cargo install precc-cli precc-hook
precc init        # prints the settings.json snippet to copy
```

Then paste the printed `PreToolUse` hook block into your `~/.claude/settings.json` and you're done. No telemetry, no daemon, all data is in `~/.local/share/precc/`.

Bonus: `precc savings` shows how many tokens it estimates you've saved (counts rewrites × ~250 tok each, conservative).

Also ships shims for Cursor (`beforeShellExecution` hook) and Aider (`$SHELL=` wrapper), if you're cross-tool.

Feedback wanted on which other patterns are common enough to ship by default. The skill format is plain TOML, so you can drop your own into `~/.local/share/precc/skills/builtin/` and `precc init` to reload.
