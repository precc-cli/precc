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

    // Observability probe (opt-in): when two or more matches tie for the top
    // confidence, the pick between them is arbitrary. Logging how often that
    // happens shows whether a smarter tie-breaker would change selection.
    // Gated on PRECC_SKILL_TIE_PROBE or PRECC_HOOK_TRACE; off by default, so
    // normal runs are unaffected.
    if tie_probe_enabled() {
        let tied = top_confidence_ties(&matches);
        if tied >= 2 {
            log_skill_tie(tied);
        }
    }

    Ok(matches)
}

/// Count how many leading matches share the top confidence (within a small
/// epsilon). Assumes `matches` is sorted by confidence descending. Returns 0
/// for fewer than two matches or when the top two differ.
fn top_confidence_ties(matches: &[SkillMatch]) -> usize {
    if matches.len() < 2 {
        return 0;
    }
    let top = matches[0].confidence;
    matches
        .iter()
        .take_while(|m| (m.confidence - top).abs() < 1e-9)
        .count()
}

/// Lazy, cached env-var read: either `PRECC_SKILL_TIE_PROBE` (narrow probe
/// opt-in) or `PRECC_HOOK_TRACE` (general trace, which implies the probe).
fn tie_probe_enabled() -> bool {
    use std::sync::OnceLock;
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        std::env::var("PRECC_SKILL_TIE_PROBE").is_ok() || std::env::var("PRECC_HOOK_TRACE").is_ok()
    })
}

/// Append one tie observation to `metrics.log` as a JSONL line. Best-effort —
/// errors are swallowed. The line uses the same shape `import_log` reads, so
/// it surfaces as the `skill_tie_at_top` metric after import.
fn log_skill_tie(tied: usize) {
    use std::io::Write;
    let Ok(data_dir) = crate::db::data_dir() else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let line = format!("{{\"ts\":{ts},\"type\":\"skill_tie_at_top\",\"value\":{tied}.0}}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("metrics.log"))
        .and_then(|mut f| f.write_all(line.as_bytes()));
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

/// Decrement the action confidence of a named skill by `delta` (clamped at 0).
/// If the resulting max confidence drops below `disable_threshold`, the skill
/// is auto-disabled (`enabled = 0`) and `Ok(true)` is returned.
///
/// Intended for a negative-feedback signal: when a skill's rewrite turns out to
/// have been unhelpful, decay its confidence so repeated misses eventually
/// retire it. Bumps the skill's `failed` counter as a side effect. A skill name
/// that doesn't exist is a no-op returning `Ok(false)`.
pub fn decay_confidence(
    conn: &Connection,
    skill_name: &str,
    delta: f64,
    disable_threshold: f64,
) -> Result<bool> {
    let skill_id: Option<i64> = conn
        .query_row("SELECT id FROM skills WHERE name = ?1", [skill_name], |r| {
            r.get(0)
        })
        .ok();
    let Some(id) = skill_id else {
        return Ok(false);
    };

    // Decrement confidence on every action row of this skill.
    conn.execute(
        "UPDATE skill_actions
         SET confidence = MAX(0.0, confidence - ?1)
         WHERE skill_id = ?2",
        rusqlite::params![delta, id],
    )?;
    // Ensure a stats row exists, then bump the failed counter.
    conn.execute(
        "INSERT OR IGNORE INTO skill_stats (skill_id, activated, succeeded, failed, last_used)
         VALUES (?1, 0, 0, 0, NULL)",
        [id],
    )?;
    conn.execute(
        "UPDATE skill_stats SET failed = failed + 1 WHERE skill_id = ?1",
        [id],
    )?;

    // If the best remaining action is below threshold, disable the skill.
    let max_conf: f64 = conn
        .query_row(
            "SELECT COALESCE(MAX(confidence), 0.0) FROM skill_actions WHERE skill_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap_or(0.0);
    if max_conf < disable_threshold {
        conn.execute("UPDATE skills SET enabled = 0 WHERE id = ?1", [id])?;
        return Ok(true);
    }
    Ok(false)
}

/// Parse a skill TOML and update an existing skill in place.
///
/// Replaces the skill's metadata, triggers, and actions; the `skill_stats` row
/// is preserved unchanged. Errors if the TOML's skill name doesn't match
/// `existing_name` (rename-via-edit is not supported) or if the skill is
/// absent.
pub fn update_skill_toml(conn: &Connection, existing_name: &str, toml_content: &str) -> Result<()> {
    let doc: SkillDoc = toml::from_str(toml_content)?;

    if doc.skill.name != existing_name {
        anyhow::bail!(
            "skill name in TOML ({:?}) does not match existing name ({:?}); \
             rename is not supported via edit",
            doc.skill.name,
            existing_name
        );
    }

    let skill_id: i64 = match conn
        .query_row(
            "SELECT id FROM skills WHERE name = ?1",
            [existing_name],
            |r| r.get(0),
        )
        .ok()
    {
        Some(id) => id,
        None => anyhow::bail!("skill '{}' not found", existing_name),
    };

    let now = chrono_now();

    // Update skill metadata.
    conn.execute(
        "UPDATE skills SET description = ?2, source = ?3, priority = ?4, updated_at = ?5
         WHERE id = ?1",
        rusqlite::params![
            skill_id,
            doc.skill.description,
            doc.skill.source,
            doc.skill.priority,
            now
        ],
    )?;

    // Replace triggers.
    conn.execute("DELETE FROM skill_triggers WHERE skill_id = ?1", [skill_id])?;
    for trigger in &doc.triggers {
        conn.execute(
            "INSERT INTO skill_triggers (skill_id, trigger_type, pattern, weight)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![skill_id, trigger.r#type, trigger.pattern, trigger.weight],
        )?;
    }

    // Replace actions.
    conn.execute("DELETE FROM skill_actions WHERE skill_id = ?1", [skill_id])?;
    for action in &doc.actions {
        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![skill_id, action.r#type, action.template, action.confidence],
        )?;
    }

    Ok(())
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

    fn sm(confidence: f64) -> SkillMatch {
        SkillMatch {
            skill_name: "s".to_string(),
            action_type: "rewrite_command".to_string(),
            template: "t".to_string(),
            confidence,
            skill_id: 1,
        }
    }

    #[test]
    fn top_ties_needs_two() {
        assert_eq!(top_confidence_ties(&[]), 0);
        assert_eq!(top_confidence_ties(&[sm(0.9)]), 0);
    }

    #[test]
    fn top_ties_clear_winner_is_one() {
        // A single top match (no tie) counts as 1 — below the log threshold.
        assert_eq!(top_confidence_ties(&[sm(0.9), sm(0.5)]), 1);
    }

    #[test]
    fn top_ties_counts_shared_top_only() {
        assert_eq!(top_confidence_ties(&[sm(0.9), sm(0.9), sm(0.5)]), 2);
        assert_eq!(
            top_confidence_ties(&[sm(0.8), sm(0.8), sm(0.8), sm(0.2)]),
            3
        );
    }

    #[test]
    fn top_ties_respects_epsilon() {
        // Within epsilon → tied.
        assert_eq!(top_confidence_ties(&[sm(0.9), sm(0.9 + 1e-12)]), 2);
        // Outside epsilon → not tied.
        assert_eq!(top_confidence_ties(&[sm(0.9), sm(0.8)]), 1);
    }

    #[test]
    fn decay_confidence_decrements_and_disables_below_threshold() {
        let conn = test_db();
        let now = chrono_now();
        conn.execute(
            "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
             VALUES ('decay-me', 'test', 'mined', 100, ?1, ?1)",
            [&now],
        )
        .unwrap();
        let skill_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, 'rewrite_command', 'rtk cargo', 0.35)",
            [skill_id],
        )
        .unwrap();

        // First decay: 0.35 → 0.30, still at threshold, not disabled.
        assert!(!decay_confidence(&conn, "decay-me", 0.05, 0.30).unwrap());
        // Second decay: 0.30 → 0.25, drops below threshold → disabled.
        assert!(decay_confidence(&conn, "decay-me", 0.05, 0.30).unwrap());

        let enabled: i64 = conn
            .query_row(
                "SELECT enabled FROM skills WHERE id = ?1",
                [skill_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);

        // Failed counter incremented once per decay.
        let failed: i64 = conn
            .query_row(
                "SELECT failed FROM skill_stats WHERE skill_id = ?1",
                [skill_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(failed, 2);
    }

    #[test]
    fn decay_confidence_unknown_skill_is_noop() {
        let conn = test_db();
        assert!(!decay_confidence(&conn, "does-not-exist", 0.10, 0.30).unwrap());
    }

    #[test]
    fn update_skill_toml_changes_metadata_and_replaces_children() {
        let conn = test_db();
        let original = r#"
[skill]
name = "my-skill"
description = "original description"
source = "mined"
priority = 100

[[triggers]]
type = "command_regex"
pattern = "^cargo"
weight = 1.0

[[actions]]
type = "rewrite_command"
template = "cd {{project_root}} && {{original_command}}"
confidence = 0.5
"#;
        load_skill_toml(&conn, original).unwrap();

        let updated = r#"
[skill]
name = "my-skill"
description = "updated description"
source = "mined"
priority = 200

[[triggers]]
type = "command_regex"
pattern = "^cargo\\s+build"
weight = 0.9

[[actions]]
type = "rewrite_command"
template = "cd {{project_root}} && {{original_command}}"
confidence = 0.7
"#;
        update_skill_toml(&conn, "my-skill", updated).unwrap();

        let (desc, pri): (String, i64) = conn
            .query_row(
                "SELECT description, priority FROM skills WHERE name = 'my-skill'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(desc, "updated description");
        assert_eq!(pri, 200);

        // Children are replaced, not duplicated.
        let trigger_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM skill_triggers WHERE skill_id = \
                 (SELECT id FROM skills WHERE name = 'my-skill')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 1);

        let conf: f64 = conn
            .query_row(
                "SELECT confidence FROM skill_actions WHERE skill_id = \
                 (SELECT id FROM skills WHERE name = 'my-skill')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((conf - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn update_skill_toml_rejects_name_change() {
        let conn = test_db();
        let original = r#"
[skill]
name = "orig-skill"
description = "desc"
source = "mined"
priority = 100
"#;
        load_skill_toml(&conn, original).unwrap();

        let renamed = r#"
[skill]
name = "renamed-skill"
description = "desc"
source = "mined"
priority = 100
"#;
        let result = update_skill_toml(&conn, "orig-skill", renamed);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("rename is not supported"));
    }

    #[test]
    fn update_skill_toml_preserves_stats() {
        let conn = test_db();
        let toml = r#"
[skill]
name = "stats-skill"
description = "desc"
source = "mined"
priority = 100

[[triggers]]
type = "command_regex"
pattern = "^npm"
weight = 1.0

[[actions]]
type = "rewrite_command"
template = "cd {{project_root}} && {{original_command}}"
confidence = 0.5
"#;
        load_skill_toml(&conn, toml).unwrap();

        let skill_id: i64 = conn
            .query_row(
                "SELECT id FROM skills WHERE name = 'stats-skill'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE skill_stats SET activated = 7 WHERE skill_id = ?1",
            [skill_id],
        )
        .unwrap();

        update_skill_toml(&conn, "stats-skill", toml).unwrap();

        // Stats survive the in-place update.
        let activated: i64 = conn
            .query_row(
                "SELECT activated FROM skill_stats WHERE skill_id = ?1",
                [skill_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(activated, 7);
    }
}
