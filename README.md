# precc

**Predictive error correction for LLM-driven shells.**

`precc` is a `PreToolUse:Bash` hook for Claude Code (and similar agentic
shells) that rewrites commands *before* they run, turning what would have
been a failed turn into a successful one. It is the missing input-shaping
layer that sits next to output-compressing tools.

## What it does

Four pillars, one hook:

1. **Context resolution** — `cargo build` outside a Rust project? `precc`
   prepends the right `cd`.
2. **GDB opportunities** — repeated failures on the same command get a
   one-line diagnostic hint.
3. **Pattern mining** — failure → fix pairs in your shell history are
   distilled into preventions for next time.
4. **Skills** — high-confidence patterns auto-apply: `git status` in a
   `jj`-colocated repo becomes `jj status`; `asciinema rec` becomes
   `precc gif`; …

Optional fifth: pluggable **output compressors** (e.g. `rtk`) — opt-in
via `~/.config/precc/config.toml`.

## Status

`v0.1` — early release. The hook, skills system, and metrics are
operational; the compressor layer ships with two adapters (`none`,
`rtk`) and is extensible.

## Install

```bash
cargo install --path crates/precc-cli
cargo install --path crates/precc-hook
precc init
```

Then add to your Claude Code `settings.json`:

```jsonc
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "precc-hook" }] }
    ]
  }
}
```

## License

Dual-licensed under MIT or Apache-2.0, at your option.
