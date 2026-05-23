//! precc-cursor-hook: Cursor `beforeShellExecution` hook binary.
//!
//! Cursor's hook protocol is allow/deny/ask — it does **not** support
//! command rewrites at the hook layer (unlike Claude Code). So when
//! PRECC would rewrite a command, we deny it and surface the corrected
//! invocation through `user_message` / `agent_message`. The agent's
//! next turn then issues the rewritten command directly.
//!
//! Wire format (per Cursor docs):
//!
//! stdin:
//!   {"command":"...", "cwd":"...", "conversation_id":"...",
//!    "generation_id":"...", "workspace_roots":[...], "sandbox":bool}
//!
//! stdout:
//!   {"permission":"allow|deny|ask",
//!    "user_message":"...", "agent_message":"..."}
//!
//! Safety: any error => exit 0 with `{"permission":"allow"}`.

use precc_core::pipeline::Pipeline;
use serde_json::Value;
use std::io::Read;

fn main() {
    if run().is_err() {
        emit_allow();
    }
}

fn run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let hook_input: Value = serde_json::from_str(&input)?;
    let command = match hook_input.get("command").and_then(|c| c.as_str()) {
        Some(cmd) if !cmd.is_empty() => cmd.to_string(),
        _ => {
            emit_allow();
            return Ok(());
        }
    };

    let mut pipeline = Pipeline::new(command);
    pipeline.run();

    if pipeline.modified() {
        emit_deny(&pipeline.command, &pipeline.reason());
    } else {
        emit_allow();
    }

    Ok(())
}

fn emit_allow() {
    println!("{}", serde_json::json!({ "permission": "allow" }));
}

fn emit_deny(corrected: &str, reason: &str) {
    let user_msg = format!("{reason}. Run instead: {corrected}");
    let agent_msg = format!("PRECC suggests the corrected command: `{corrected}`");
    let out = serde_json::json!({
        "permission": "deny",
        "user_message": user_msg,
        "agent_message": agent_msg,
    });
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
}
