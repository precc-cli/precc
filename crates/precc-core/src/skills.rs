//! Pillar 4: Skill matching engine.
//!
//! Queries heuristics.db for skills that match the current command,
//! evaluates triggers, and returns applicable actions sorted by confidence.

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use std::path::Path;

/// A matched skill with its action to apply.
#[derive(Debug)]
pub struct SkillMatch {
    pub skill_name: String,
    pub action_type: String,
    pub template: String,
    pub confidence: f64,
    pub skill_id: i64,
}

/// Query heuristics.db for skills matching the given command.
///
/// Returns matches sorted by confidence (highest first).
/// Only returns enabled skills with confidence >= `min_confidence`.
pub fn find_matches(
    conn: &Connection,
    command: &str,
    min_confidence: f64,
) -> Result<Vec<SkillMatch>> {
    // Get all enabled skills with command_regex triggers
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.name, t.pattern, t.weight, a.action_type, a.template, a.confidence
         FROM skills s
         JOIN skill_triggers t ON t.skill_id = s.id
         JOIN skill_actions a ON a.skill_id = s.id
         WHERE s.enabled = 1
           AND t.trigger_type = 'command_regex'
           AND a.confidence >= ?1
         ORDER BY s.priority ASC, a.confidence DESC",
    )?;

    let rows = stmt.query_map([min_confidence], |row| {
        Ok(CandidateRow {
            skill_id: row.get(0)?,
            skill_name: row.get(1)?,
            pattern: row.get(2)?,
            weight: row.get(3)?,
            action_type: row.get(4)?,
            template: row.get(5)?,
            confidence: row.get(6)?,
        })
    })?;

    // First word of the command, used by the fast-path skip below. A
    // leading `cd <dir> && <verb> …` is special-cased so we look at the
    // verb after the `cd`, not the literal `cd`.
    let first_word = command_first_word_for_match(command);

    let mut matches = Vec::new();
    for row in rows {
        let row = row?;
        // Fast-path: skip Regex::new entirely when the pattern reduces to a
        // first-word literal (or small alternation) that the command's first
        // word cannot match. Compiling ~16 builtin-skill regexes on every
        // invocation is pure waste otherwise. Truly unanalysable patterns
        // (e.g. `^\s*#`) fall through to the regex engine as before.
        if let Some(literals) = pattern_first_word_literals(&row.pattern) {
            if !literals.iter().any(|w| w == first_word) {
                continue;
            }
        }
        // Compile and test the regex trigger
        if let Ok(re) = Regex::new(&row.pattern) {
            if re.is_match(command) {
                // Check file_exists triggers for this skill
                let file_check_ok = check_file_triggers(conn, row.skill_id)?;
                if file_check_ok {
                    matches.push(SkillMatch {
                        skill_name: row.skill_name,
                        action_type: row.action_type,
                        template: row.template,
                        confidence: row.confidence * row.weight,
                        skill_id: row.skill_id,
                    });
                }
            }
        }
    }

    // Sort by confidence descending
    matches.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    Ok(matches)
}

/// First word of a command, used to drive the [`find_matches`] fast-path.
///
/// Wrong-dir rewriters target the verb, not the `cd` wrapper, so a command
/// of the form `cd <dir> && <verb> …` is reduced to `<verb>`. This mirrors
/// the behaviour of the cd-prepend rewrites.
fn command_first_word_for_match(command: &str) -> &str {
    let trimmed = command.trim_start();
    if let Some(rest) = trimmed.strip_prefix("cd ") {
        if let Some(idx) = rest.find(" && ") {
            return rest[idx + 4..].split_whitespace().next().unwrap_or("");
        }
    }
    trimmed.split_whitespace().next().unwrap_or("")
}

/// Extract the set of first-word literals from a `command_regex` pattern.
///
/// `None` means "unanalysable — caller must run the regex" (e.g. `.`,
/// `^\s*#`, anchorless patterns). `Some(set)` means the command's first
/// word must be one of `set` for the regex to have any chance of matching,
/// so the caller can skip `Regex::new()` otherwise.
fn pattern_first_word_literals(pat: &str) -> Option<Vec<String>> {
    let stripped = pat.strip_prefix('^')?;
    if stripped.starts_with('(') {
        let end = stripped.find(')')?;
        let inner = &stripped[1..end];
        let mut out = Vec::new();
        for word in inner.split('|') {
            let w = word.trim();
            if w.is_empty()
                || !w
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return None; // alternation contains a complex element
            }
            out.push(w.to_string());
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    } else {
        let word: String = stripped
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if word.is_empty() {
            None
        } else {
            Some(vec![word])
        }
    }
}

struct CandidateRow {
    skill_id: i64,
    skill_name: String,
    pattern: String,
    weight: f64,
    action_type: String,
    template: String,
    confidence: f64,
}

/// Check file_exists triggers for a skill.
/// Returns true if all file_exists conditions are satisfied.
fn check_file_triggers(conn: &Connection, skill_id: i64) -> Result<bool> {
    let mut stmt = conn.prepare_cached(
        "SELECT pattern FROM skill_triggers
         WHERE skill_id = ?1 AND trigger_type = 'file_exists'",
    )?;

    let patterns: Vec<String> = stmt
        .query_map([skill_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    if patterns.is_empty() {
        return Ok(true);
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    for pattern in &patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            // '!' prefix means the file must NOT exist
            if cwd.join(negated).exists() {
                return Ok(false);
            }
        } else {
            // File must exist
            if !cwd.join(pattern).exists() {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

/// Apply a skill action template to produce a modified command.
///
/// Supported placeholders:
/// - `{{original_command}}` — the original command string
/// - `{{project_root}}` — the resolved project root directory
pub fn apply_template(template: &str, original_command: &str, project_root: &str) -> String {
    template
        .replace("{{original_command}}", original_command)
        .replace("{{project_root}}", project_root)
}

/// Record that a skill was activated (for stats tracking).
pub fn record_activation(conn: &Connection, skill_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO skill_stats (skill_id, activated, succeeded, failed, last_used)
         VALUES (?1, 1, 0, 0, datetime('now'))
         ON CONFLICT(skill_id) DO UPDATE SET
           activated = activated + 1,
           last_used = datetime('now')",
        [skill_id],
    )?;
    Ok(())
}

/// Replay any pending activations from `~/.local/share/precc/activations.log`
/// into `skill_stats`, then truncate the log.
///
/// The hook writes one JSON-per-line entry per activation (cheap append,
/// no DB write in the hot path). Measurement commands call this at start
/// so counters reflect reality.
///
/// Fail-open: returns Ok(0) on any I/O error and leaves the log untouched.
pub fn replay_activations(conn: &Connection) -> Result<usize> {
    use std::io::Read;

    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Ok(0),
    };
    let log_path = std::path::Path::new(&home).join(".local/share/precc/activations.log");
    if !log_path.exists() {
        return Ok(0);
    }

    let mut content = String::new();
    {
        let mut f = match std::fs::File::open(&log_path) {
            Ok(f) => f,
            Err(_) => return Ok(0),
        };
        if f.read_to_string(&mut content).is_err() {
            return Ok(0);
        }
    }
    if content.is_empty() {
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let mut applied = 0usize;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Parse the tiny JSON written by precc_core::pipeline::append_activation_log.
        // Format: {"ts":<u64>,"skill_id":<i64>,"skill_name":"...","conf":<f64>}
        let skill_id = match extract_int_field(line, "\"skill_id\":") {
            Some(id) => id,
            None => continue,
        };
        if record_activation(&tx, skill_id).is_ok() {
            applied += 1;
        }
    }
    tx.commit()?;

    // Truncate the log only after a successful commit so we never lose data
    // on partial replay.
    let _ = std::fs::File::create(&log_path);

    Ok(applied)
}

/// Extract an integer field from a JSON-ish line, e.g.
/// `extract_int_field(r#"{"skill_id":42,...}"#, "\"skill_id\":")` => Some(42).
/// This is a tiny zero-dependency parser tailored to the exact line format
/// written by the hook; it does not handle whitespace or escaped strings.
fn extract_int_field(line: &str, key: &str) -> Option<i64> {
    let start = line.find(key)? + key.len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Load built-in skills from TOML files into heuristics.db (if not already present).
pub fn load_builtin_skills(conn: &Connection, skills_dir: &Path) -> Result<usize> {
    let mut loaded = 0;

    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;
        if load_skill_toml(conn, &content)? {
            loaded += 1;
        }
    }

    Ok(loaded)
}

/// Parse a skill TOML and insert into heuristics.db.
/// Returns true if a new skill was inserted, false if it already existed.
fn load_skill_toml(conn: &Connection, toml_content: &str) -> Result<bool> {
    let doc: SkillDoc = toml::from_str(toml_content)?;

    // Check if skill already exists
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM skills WHERE name = ?1",
        [&doc.skill.name],
        |r| r.get(0),
    )?;

    if exists {
        return Ok(false);
    }

    let now = chrono_now();

    conn.execute(
        "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![
            doc.skill.name,
            doc.skill.description,
            doc.skill.source,
            doc.skill.priority,
            now
        ],
    )?;

    let skill_id = conn.last_insert_rowid();

    for trigger in &doc.triggers {
        conn.execute(
            "INSERT INTO skill_triggers (skill_id, trigger_type, pattern, weight)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![skill_id, trigger.r#type, trigger.pattern, trigger.weight],
        )?;
    }

    for action in &doc.actions {
        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![skill_id, action.r#type, action.template, action.confidence],
        )?;
    }

    // Initialize stats row
    conn.execute(
        "INSERT INTO skill_stats (skill_id, activated, succeeded, failed) VALUES (?1, 0, 0, 0)",
        [skill_id],
    )?;

    Ok(true)
}

pub fn chrono_now() -> String {
    // Simple UTC timestamp without pulling in chrono crate
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", d.as_secs())
}

/// TOML deserialization structures for skill files.
#[derive(serde::Deserialize)]
struct SkillDoc {
    skill: SkillMeta,
    #[serde(default)]
    triggers: Vec<TriggerDef>,
    #[serde(default)]
    actions: Vec<ActionDef>,
}

#[derive(serde::Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    source: String,
    #[serde(default = "default_priority")]
    priority: i32,
}

fn default_priority() -> i32 {
    100
}

#[derive(serde::Deserialize)]
struct TriggerDef {
    r#type: String,
    pattern: String,
    #[serde(default = "default_weight")]
    weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

#[derive(serde::Deserialize)]
struct ActionDef {
    r#type: String,
    template: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

fn default_confidence() -> f64 {
    0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_db() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        db::open_heuristics(dir.path()).unwrap()
    }

    #[test]
    fn load_cargo_wrong_dir_skill() {
        let conn = test_db();
        let toml = r#"
[skill]
name = "cargo-wrong-dir"
description = "Fix cargo commands in wrong directory"
source = "builtin"
priority = 50

[[triggers]]
type = "command_regex"
pattern = "^cargo\\s+(build|test|clippy|check)"
weight = 1.0

[[triggers]]
type = "file_exists"
pattern = "!Cargo.toml"
weight = 0.8

[[actions]]
type = "prepend_cd"
template = "cd {{project_root}} && {{original_command}}"
confidence = 0.9
"#;
        assert!(load_skill_toml(&conn, toml).unwrap());
        // Second load should return false (already exists)
        assert!(!load_skill_toml(&conn, toml).unwrap());
    }

    #[test]
    fn find_matches_returns_matching_skills() {
        let conn = test_db();
        let now = chrono_now();

        // Insert a test skill manually
        conn.execute(
            "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
             VALUES ('test-skill', 'test', 'builtin', 100, ?1, ?1)",
            [&now],
        )
        .unwrap();
        let skill_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO skill_triggers (skill_id, trigger_type, pattern, weight)
             VALUES (?1, 'command_regex', '^cargo\\s+build', 1.0)",
            [skill_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, 'rewrite_command', 'rtk cargo build', 0.9)",
            [skill_id],
        )
        .unwrap();

        let matches = find_matches(&conn, "cargo build --release", 0.3).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].skill_name, "test-skill");
        assert_eq!(matches[0].confidence, 0.9);
    }

    #[test]
    fn find_matches_filters_by_confidence() {
        let conn = test_db();
        let now = chrono_now();

        conn.execute(
            "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
             VALUES ('low-conf', 'test', 'mined', 100, ?1, ?1)",
            [&now],
        )
        .unwrap();
        let skill_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO skill_triggers (skill_id, trigger_type, pattern, weight)
             VALUES (?1, 'command_regex', '^cargo', 1.0)",
            [skill_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, 'rewrite_command', 'test', 0.2)",
            [skill_id],
        )
        .unwrap();

        // Should be excluded with min_confidence = 0.3
        let matches = find_matches(&conn, "cargo build", 0.3).unwrap();
        assert!(matches.is_empty());

        // Should be included with min_confidence = 0.1
        let matches = find_matches(&conn, "cargo build", 0.1).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn apply_template_replaces_placeholders() {
        let result = apply_template(
            "cd {{project_root}} && {{original_command}}",
            "cargo build",
            "/home/user/myapp",
        );
        assert_eq!(result, "cd /home/user/myapp && cargo build");
    }

    #[test]
    fn no_matches_for_unrelated_command() {
        let conn = test_db();
        let matches = find_matches(&conn, "echo hello", 0.0).unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn extract_int_field_parses() {
        let line = r#"{"ts":1779625618,"skill_id":42,"skill_name":"x","conf":0.900}"#;
        assert_eq!(extract_int_field(line, "\"skill_id\":"), Some(42));
        assert_eq!(extract_int_field(line, "\"ts\":"), Some(1779625618));
        assert_eq!(extract_int_field(line, "\"missing\":"), None);
    }

    #[test]
    fn replay_activations_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let data_dir = home.path().join(".local/share/precc");
        std::fs::create_dir_all(&data_dir).unwrap();
        let conn = db::open_heuristics(&data_dir).unwrap();

        // Insert a skill so the FK in skill_stats has something to point at.
        conn.execute(
            "INSERT INTO skills (id, name, description, source, priority, created_at, updated_at)
             VALUES (7, 'demo', 'd', 'builtin', 50, '0', '0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_stats (skill_id, activated, succeeded, failed) VALUES (7, 0, 0, 0)",
            [],
        )
        .unwrap();

        // Drop two activation lines into the log.
        let log = data_dir.join("activations.log");
        std::fs::write(
            &log,
            "{\"ts\":1,\"skill_id\":7,\"skill_name\":\"demo\",\"conf\":0.900}\n\
             {\"ts\":2,\"skill_id\":7,\"skill_name\":\"demo\",\"conf\":0.900}\n",
        )
        .unwrap();

        // Replay needs HOME=tempdir to find the log.
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home.path());
        let applied = replay_activations(&conn).unwrap();
        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(applied, 2);

        let count: i64 = conn
            .query_row(
                "SELECT activated FROM skill_stats WHERE skill_id = 7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Log should now be empty (truncated).
        let post = std::fs::read_to_string(&log).unwrap();
        assert!(post.is_empty(), "log should be truncated, got: {post:?}");
    }

    #[test]
    fn first_word_literals_single() {
        assert_eq!(
            pattern_first_word_literals(r"^cargo\s+build"),
            Some(vec!["cargo".to_string()])
        );
        assert_eq!(
            pattern_first_word_literals(r"^go\s+"),
            Some(vec!["go".to_string()])
        );
    }

    #[test]
    fn first_word_literals_alternation() {
        let got = pattern_first_word_literals(r"^(npm|npx|pnpm|yarn)\s+").unwrap();
        assert_eq!(got, vec!["npm", "npx", "pnpm", "yarn"]);
    }

    #[test]
    fn first_word_literals_unanalysable() {
        // No leading caret → can't infer first word
        assert_eq!(pattern_first_word_literals(r"cargo"), None);
        // Leading whitespace class → first char isn't a word char
        assert_eq!(pattern_first_word_literals(r"^\s*#"), None);
        // True catch-all
        assert_eq!(pattern_first_word_literals(r"."), None);
        // Alternation containing a regex metachar
        assert_eq!(pattern_first_word_literals(r"^(npm|\w+)"), None);
    }

    #[test]
    fn cmd_first_word_skips_cd_prepend() {
        assert_eq!(
            command_first_word_for_match("cd /repo && cargo build"),
            "cargo"
        );
        assert_eq!(command_first_word_for_match("cargo build"), "cargo");
        assert_eq!(command_first_word_for_match("  cargo build"), "cargo");
        assert_eq!(command_first_word_for_match("ls"), "ls");
        assert_eq!(command_first_word_for_match(""), "");
    }
}
