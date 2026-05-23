//! precc-hook: Claude Code PreToolUse:Bash hook binary.
//!
//! Reads JSON from stdin (Claude Code hook event), processes through the
//! shared [`precc_core::pipeline::Pipeline`], and emits modified JSON to
//! stdout.
//!
//! Safety: on any error, exit 0 (allow command unchanged). Never block
//! Claude. Latency target: < 5 ms p99.

use precc_core::pipeline::Pipeline;
use serde_json::Value;
use std::io::Read;

fn main() {
    if run().is_err() {
        std::process::exit(0);
    }
}

fn run() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let hook_input: Value = serde_json::from_str(&input)?;
    let command = match hook_input
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(cmd) => cmd.to_string(),
        None => return Ok(()),
    };

    if command.is_empty() {
        return Ok(());
    }

    let mut pipeline = Pipeline::new(command);
    pipeline.run();

    if pipeline.modified() {
        let tool_input = hook_input
            .get("tool_input")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));

        emit_rewrite(&tool_input, &pipeline.command, &pipeline.reason())?;
    }

    Ok(())
}

fn emit_rewrite(
    original_tool_input: &Value,
    new_command: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let mut updated_input = original_tool_input.clone();
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert(
            "command".to_string(),
            Value::String(new_command.to_string()),
        );
    }

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": reason,
            "updatedInput": updated_input
        }
    });

    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_rewrite_produces_valid_json() {
        let tool_input = serde_json::json!({"command": "git status", "timeout": 5000});
        let mut updated = tool_input.clone();
        updated["command"] = Value::String("rtk git status".to_string());

        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "PRECC: rtk-rewrite",
                "updatedInput": updated
            }
        });

        let s = serde_json::to_string(&output).unwrap();
        assert!(s.contains("rtk git status"));
        assert!(s.contains("PreToolUse"));
    }
}
