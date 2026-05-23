//! precc-shell: a thin `$SHELL` wrapper for tools that don't expose a
//! pre-exec hook (e.g. Aider's `/run` flow).
//!
//! Usage:
//!   export SHELL=$(which precc-shell)   # or set in aider config
//!
//! Aider then invokes `precc-shell -c "<cmd>"`. precc-shell runs the
//! command through the PRECC pipeline, then exec's `bash -c "<rewritten>"`.
//! For any invocation that's not `-c`, we transparently exec bash with
//! the original args so interactive use still works.
//!
//! Fail-open: any pipeline error falls through to the original command.

use precc_core::pipeline::Pipeline;
use std::os::unix::process::CommandExt;
use std::process::Command;

const REAL_SHELL: &str = "/bin/bash";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // Only intercept the `bash -c "<cmd>"` form. Everything else
    // (interactive, login, --version, etc.) gets passed through.
    if args.len() >= 2 && args[0] == "-c" {
        let original = args[1].clone();
        let mut pipeline = Pipeline::new(original.clone());
        pipeline.run();

        if pipeline.modified() {
            // Stderr is visible in Aider's `/run` output; stdout is
            // captured as command output, so the notice goes to stderr.
            eprintln!("[{}] rewrote: {}", pipeline.reason(), pipeline.command);
            args[1] = pipeline.command;
        }
    }

    // exec — this process is replaced by bash, preserving exit code,
    // signal handling, and tty state.
    let err = Command::new(REAL_SHELL).args(&args).exec();
    eprintln!("precc-shell: failed to exec {REAL_SHELL}: {err}");
    std::process::exit(127);
}
