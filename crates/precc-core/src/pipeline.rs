//! Shared rewrite pipeline used by every front-end (Claude Code hook,
//! Cursor hook, Aider shell shim).
//!
//! Front-ends parse their own input wire format, hand the raw command
//! to [`Pipeline::run`], and then translate [`Pipeline::command`] and
//! [`Pipeline::reason`] back into whatever their host expects.
//!
//! Stages (run in order):
//! 1. Skill matching (Pillar 4) — read-only DB lookup, skipped if no DB
//! 2. Context resolution (Pillar 1) — prepend `cd <root>` if cwd is wrong
//! 3. Compressor wrapping (opt-in) — wraps recognised commands via
//!    the configured [`crate::compressors::Compressor`]
//!
//! Safety: any DB / fs error is swallowed silently so callers can stay
//! fail-open. The pipeline never panics on bad input.

use crate::{compressors, context, db, rewrites, skills};
use std::io::Write;

/// Confidence threshold for auto-applying a skill rewrite.
pub const AUTO_APPLY_THRESHOLD: f64 = 0.7;

/// Minimum confidence to consider a skill match at all.
pub const SUGGEST_THRESHOLD: f64 = 0.3;

/// Run the full PRECC rewrite pipeline against a raw command.
///
/// Returns a [`Pipeline`] whose `command` field is the (possibly
/// rewritten) result and whose `reason` describes what stages fired.
pub struct Pipeline {
    original: String,
    pub command: String,
    reasons: Vec<String>,
}

impl Pipeline {
    pub fn new(command: String) -> Self {
        Self {
            original: command.clone(),
            command,
            reasons: Vec::new(),
        }
    }

    /// True if any stage changed the command.
    pub fn modified(&self) -> bool {
        self.command != self.original
    }

    /// Human-readable reason string (used in hook decision messages).
    pub fn reason(&self) -> String {
        if self.reasons.is_empty() {
            "PRECC".to_string()
        } else {
            format!("PRECC: {}", self.reasons.join("; "))
        }
    }

    /// Run all stages.
    pub fn run(&mut self) {
        self.stage_skills();
        self.stage_context();
        self.stage_compressor();
    }

    fn stage_skills(&mut self) {
        let data_dir = match db::data_dir() {
            Ok(d) => d,
            Err(_) => return,
        };

        let conn = match db::open_heuristics_readonly(&data_dir) {
            Ok(Some(c)) => c,
            _ => return,
        };

        let matches = match skills::find_matches(&conn, &self.command, SUGGEST_THRESHOLD) {
            Ok(m) => m,
            Err(_) => return,
        };

        for m in &matches {
            if m.confidence >= AUTO_APPLY_THRESHOLD {
                let project_root = self.resolve_project_root();
                let rewritten = skills::apply_template(&m.template, &self.command, &project_root);
                self.command = rewritten;
                self.reasons
                    .push(format!("skill:{} (conf={:.1})", m.skill_name, m.confidence));
                append_activation_log(m.skill_id, &m.skill_name, m.confidence);
                break;
            }
        }
    }

    fn stage_context(&mut self) {
        let ctx = context::resolve(&self.command);

        if let Some(rewritten) = context::apply(&self.command, &ctx) {
            if !self.command.starts_with("cd ") {
                self.command = rewritten;
                self.reasons.push(format!(
                    "cd:{} (conf={:.1})",
                    ctx.marker.as_deref().unwrap_or("?"),
                    ctx.confidence
                ));
            }
        }
    }

    fn stage_compressor(&mut self) {
        let comp = compressors::active();
        if comp.is_noop() {
            return;
        }
        let (prefix, cmd_part) = split_cd_prefix(&self.command);
        if let Some(rewritten) = rewrites::rewrite(cmd_part, comp) {
            self.command = if prefix.is_empty() {
                rewritten
            } else {
                format!("{}{}", prefix, rewritten)
            };
            self.reasons.push(format!("compressor:{}", comp.name()));
        }
    }

    fn resolve_project_root(&self) -> String {
        let ctx = context::resolve(&self.command);
        ctx.project_root
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            })
    }
}

/// Split a `cd /path && <rest>` prefix from a command. Returns
/// `("", command)` if the command doesn't start with such a prefix.
pub fn split_cd_prefix(command: &str) -> (&str, &str) {
    if let Some(pos) = command.find(" && ") {
        if command.starts_with("cd ") {
            let prefix_end = pos + 4;
            (&command[..prefix_end], &command[prefix_end..])
        } else {
            ("", command)
        }
    } else {
        ("", command)
    }
}

/// Append a skill activation record to the activations log. Single
/// `write()` syscall; fail-open on any error.
fn append_activation_log(skill_id: i64, skill_name: &str, conf: f64) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let line = format!(
        "{{\"ts\":{},\"skill_id\":{},\"skill_name\":\"{}\",\"conf\":{:.3}}}\n",
        ts, skill_id, skill_name, conf
    );

    if let Ok(home) = std::env::var("HOME") {
        let log_path = std::path::Path::new(&home).join(".local/share/precc/activations.log");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cd_prefix_with_cd() {
        let (prefix, cmd) = split_cd_prefix("cd /home/user/myapp && cargo build");
        assert_eq!(prefix, "cd /home/user/myapp && ");
        assert_eq!(cmd, "cargo build");
    }

    #[test]
    fn split_cd_prefix_without_cd() {
        let (prefix, cmd) = split_cd_prefix("cargo build --release");
        assert_eq!(prefix, "");
        assert_eq!(cmd, "cargo build --release");
    }

    #[test]
    fn pipeline_reason_empty() {
        let p = Pipeline::new("echo hello".to_string());
        assert_eq!(p.reason(), "PRECC");
    }

    #[test]
    fn pipeline_reason_with_entries() {
        let mut p = Pipeline::new("echo hello".to_string());
        p.reasons.push("rtk-rewrite".to_string());
        p.reasons.push("cd:Cargo.toml (conf=0.9)".to_string());
        assert_eq!(p.reason(), "PRECC: rtk-rewrite; cd:Cargo.toml (conf=0.9)");
    }
}
