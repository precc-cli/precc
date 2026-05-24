# r/rust — precc 0.1.0

**Title:** precc 0.1.0 — a Rust pre-exec hook that rewrites an LLM agent's bash commands before they fail

**Flair:** `project` (or `tooling`)

**Body:**

I released **precc** today: a Rust workspace that ships a ~1 ms binary which Claude Code, Cursor, and Aider can call before they execute a bash command, so wrong-directory and other recoverable mistakes get fixed in-place instead of costing a model round-trip.

(Measured: 1.1 ms mean / 2.6 ms max over 2,699 hyperfine runs on the rewrite path — read stdin, query SQLite, apply skill template, emit JSON.)

Repo: https://github.com/precc-cli/precc
Release: https://github.com/precc-cli/precc/releases/tag/v0.1.0

The Rust-relevant bits:

- **Workspace layout.** `precc-core` (lib) + four binaries: `precc` (CLI), `precc-hook` (Claude Code), `precc-cursor-hook` (Cursor), `precc-shell` (`$SHELL=` wrapper for Aider). Single shared `Pipeline` struct in `precc-core`.
- **Pluggable compressors via a trait.** `Compressor::wrap(canonical, tail) -> String`. Two adapters ship: `none` (default no-op) and `rtk`. Picks one at startup from `~/.config/precc/config.toml` with a 50-line ad-hoc TOML parser, avoiding `toml` in the hook's hot path.
- **SQLite via `rusqlite` (bundled).** Three DBs: `heuristics.db` (skills), `history.db` (mined patterns), `metrics.db` (activations / wraps). Schema bootstrapped lazily on first connection.
- **Deferred writes for hot-path latency.** The hook appends activations to a single log file (one `write()` syscall); measurement commands (`precc savings`, `precc skills list`) replay the log into `skill_stats` inside a single transaction, then truncate. No background daemon, no race conditions on a system without flock.
- **Embedded skill pack.** The 12 builtin skills (cargo, git, go, npm, python, etc.) are baked into the CLI binary via `include_str!`. `precc init` writes them to the data dir; existing files are not overwritten, so user edits stick.
- **CI on GitHub Actions:** rustfmt + clippy + tests on ubuntu and macos, all with `-D warnings`.

Stats: 5 crates, ~3 KB of embedded TOML, 80 tests, dual MIT/Apache-2.0.

Things I'd be most interested in feedback on from a Rust audience:

1. Trait design for `Compressor` — is `wrap(canonical: &str, tail: &str) -> String` the right shape, or should it be something more like `Cow<str>` to avoid allocation in the noop case?
2. Whether the ad-hoc TOML parser (~30 lines, only handles the one key we need) is worth the cold-start savings vs. pulling in `toml`.
3. SQLite WAL vs. activation-log: is the "append and replay" pattern overcomplicating something that a simple write-through with WAL + `UPSERT` would handle?

Demo GIF in the README, no install required to look at.
