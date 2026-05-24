# Benchmarks

Hook latency measured with [`hyperfine`](https://github.com/sharkdp/hyperfine)
v1.20.0 on a Linux 5.15 / x86_64 machine. Numbers are the mean ± σ of the
runs reported by hyperfine; max is the worst observed sample.

## Hook latency

Reading JSON from stdin, querying `heuristics.db`, applying the matched
skill, and emitting the rewrite JSON to stdout. All runs against a fresh
`precc init` data dir with the 12 builtin skills loaded.

| Scenario | Mean | Max | Runs |
|---|---|---|---|
| Rewrite path (`cargo build` from a Rust-project subdir, stdin via fd) | **1.1 ms** | 2.6 ms | 2,699 |
| Rewrite path, invoked via `bash -c 'echo … \| precc-hook'` | 3.5 ms | 4.3 ms | 823 |
| Passthrough (`echo hello`, no rule matches), via bash | 3.5 ms | 4.5 ms | 860 |
| Cargo build from project root (no skill, just context resolve), via bash | 3.5 ms | 4.5 ms | 833 |

The bash-wrapped numbers include bash startup (~1.5 ms) plus the precc
hook itself; the standalone-stdin number isolates the hook process.
Either way, well under the < 5 ms p99 target the design budgets for.

## Reproducing

```bash
cargo install hyperfine
cargo build --release --workspace

# Set up a fresh data dir with the 12 builtin skills.
export HOME=$(mktemp -d)
target/release/precc init >/dev/null
mkdir -p $HOME/myproj/notes && cd $HOME/myproj && cargo init --lib -q
cd $HOME/myproj/notes

# Standalone hook (rewrite path):
hyperfine --warmup 50 --min-runs 300 --shell=none \
    --input <(echo '{"tool_input":{"command":"cargo build"}}') \
    "../../target/release/precc-hook"
```

## Why the hot path is fast

- One small Rust binary, no runtime / VM startup.
- `heuristics.db` opened **read-only** with WAL, then closed. No
  schema migration in the hot path (`precc init` handles it).
- Skill activations are appended to `activations.log` with a single
  `write()` syscall; the SQLite `UPSERT` against `skill_stats` is
  deferred to measurement commands (`precc savings`, `precc skills
  list`) which replay the log in a single transaction.
- No subprocess spawns. RTK / gdb availability checks are PATH scans
  with a cached marker file, and they only run if the user opted in
  via `~/.config/precc/config.toml`.
- Builtin skills are baked into the binary via `include_str!`, so
  there is no I/O to load them after init.
