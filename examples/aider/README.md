# Aider integration via `precc-shell`

Aider does not expose a pre-exec hook, so PRECC integrates by replacing
the `$SHELL` Aider uses for `/run` and tool invocations.

## Setup

```bash
cargo install --path crates/precc-shell
export SHELL=$(which precc-shell)
aider
```

`precc-shell` only intercepts the `-c "<cmd>"` form (which is what
Aider uses); interactive sessions still pass through to `/bin/bash`
unchanged.

## What it does

When PRECC would rewrite a command (e.g. you ran `cargo build` from a
notes subdirectory), `precc-shell` prints

```
[PRECC: skill:cargo-wrong-dir (conf=0.9)] rewrote: cd /repo && cargo build
```

to stderr and then `exec`s `bash -c` with the rewritten command. Aider
captures stdout as command output and stderr as Aider-side context,
so the corrected command runs without breaking Aider's flow.
