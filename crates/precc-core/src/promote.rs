//! Pattern-to-skill promotion engine.
//!
//! Scans history.db for failure-fix pairs with high occurrence counts
//! and generates candidate skills in heuristics.db. This bridges
//! Pillar 3 (failure learning) with Pillar 4 (heuristics DB).

use anyhow::Result;
use rusqlite::Connection;

/// Minimum occurrences before a pattern is promoted to a skill.
const DEFAULT_MIN_OCCURRENCES: i64 = 3;

/// Result of a promotion run.
#[derive(Debug, Default)]
pub struct PromotionSummary {
    pub candidates_found: usize,
    pub skills_created: usize,
    pub already_exists: usize,
    pub skipped: usize,
}

/// Result of a lifecycle tick (see [`tick_skill_lifecycle`]).
#[derive(Debug, Default)]
pub struct LifecycleSummary {
    /// Skills promoted from candidate (0.3) to active (0.7).
    pub promoted_to_active: usize,
    /// Skills promoted from active (0.7) to trusted (0.9).
    pub promoted_to_trusted: usize,
    /// Skills auto-disabled due to high failure rate.
    pub auto_disabled: usize,
}

/// A candidate pattern for promotion.
#[derive(Debug)]
struct Candidate {
    failure_command: String,
    fix_command: String,
    occurrences: i64,
}

/// Scan history.db for recurring patterns and promote them to skills in heuristics.db.
pub fn promote_patterns(
    history_conn: &Connection,
    heuristics_conn: &Connection,
    min_occurrences: Option<i64>,
) -> Result<PromotionSummary> {
    let threshold = min_occurrences.unwrap_or(DEFAULT_MIN_OCCURRENCES);
    let mut summary = PromotionSummary::default();

    let candidates = find_candidates(history_conn, threshold)?;
    summary.candidates_found = candidates.len();

    for candidate in &candidates {
        match promote_one(heuristics_conn, candidate) {
            PromoteResult::Created => summary.skills_created += 1,
            PromoteResult::AlreadyExists => summary.already_exists += 1,
            PromoteResult::Skipped => summary.skipped += 1,
        }
    }

    Ok(summary)
}

enum PromoteResult {
    Created,
    AlreadyExists,
    Skipped,
}

/// Find failure-fix pairs that meet the occurrence threshold.
fn find_candidates(conn: &Connection, min_occurrences: i64) -> Result<Vec<Candidate>> {
    let mut stmt = conn.prepare(
        "SELECT failure_command, fix_command, occurrences
         FROM failure_fix_pairs
         WHERE occurrences >= ?1
         ORDER BY occurrences DESC",
    )?;

    let rows = stmt.query_map([min_occurrences], |row: &rusqlite::Row| {
        Ok(Candidate {
            failure_command: row.get(0)?,
            fix_command: row.get(1)?,
            occurrences: row.get(2)?,
        })
    })?;

    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

/// Attempt to promote a single candidate to a skill.
fn promote_one(conn: &Connection, candidate: &Candidate) -> PromoteResult {
    // Skip edit-based fixes — they indicate code changes, not command rewrites
    if candidate.fix_command.starts_with("edit:") {
        return PromoteResult::Skipped;
    }

    // Generate skill components
    let skill_name = generate_skill_name(&candidate.failure_command, &candidate.fix_command);
    let trigger_regex = generate_trigger_regex(&candidate.failure_command);
    let (action_type, template) =
        generate_action(&candidate.failure_command, &candidate.fix_command);

    // Check if a skill with this name already exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM skills WHERE name = ?1",
            [&skill_name],
            |r| r.get(0),
        )
        .unwrap_or(false);

    if exists {
        return PromoteResult::AlreadyExists;
    }

    // Insert the skill
    let now = crate::skills::chrono_now();
    let result = (|| -> Result<()> {
        conn.execute(
            "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
             VALUES (?1, ?2, 'mined', 200, ?3, ?3)",
            rusqlite::params![
                skill_name,
                format!(
                    "Auto-mined: {} -> {} ({}x)",
                    candidate.failure_command, candidate.fix_command, candidate.occurrences
                ),
                now,
            ],
        )?;
        let skill_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO skill_triggers (skill_id, trigger_type, pattern, weight)
             VALUES (?1, 'command_regex', ?2, 1.0)",
            rusqlite::params![skill_id, trigger_regex],
        )?;

        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, ?2, ?3, 0.3)",
            rusqlite::params![skill_id, action_type, template],
        )?;

        conn.execute(
            "INSERT OR IGNORE INTO skill_stats (skill_id, activated, succeeded, failed)
             VALUES (?1, 0, 0, 0)",
            [skill_id],
        )?;

        Ok(())
    })();

    match result {
        Ok(()) => PromoteResult::Created,
        Err(_) => PromoteResult::Skipped,
    }
}

/// Generate a unique skill name from the failure/fix pattern.
fn generate_skill_name(failure_cmd: &str, fix_cmd: &str) -> String {
    let fail_words: Vec<&str> = failure_cmd.split_whitespace().take(2).collect();
    let fix_words: Vec<&str> = fix_cmd.split_whitespace().take(2).collect();

    let fail_part = fail_words.join("-");
    let fix_part = fix_words.join("-");

    // Create a short hash suffix to avoid collisions
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    failure_cmd.hash(&mut hasher);
    fix_cmd.hash(&mut hasher);
    let hash = format!("{:04x}", hasher.finish() & 0xFFFF);

    format!("mined-{fail_part}-to-{fix_part}-{hash}")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Generate a regex trigger pattern from a failure command.
fn generate_trigger_regex(failure_cmd: &str) -> String {
    let words: Vec<&str> = failure_cmd.split_whitespace().take(2).collect();
    match words.len() {
        0 => "^$".to_string(),
        1 => format!(r"^{}\s*", regex::escape(words[0])),
        _ => format!(
            r"^{}\s+{}",
            regex::escape(words[0]),
            regex::escape(words[1])
        ),
    }
}

/// Generate the action type and template from the fix pattern.
fn generate_action(failure_cmd: &str, fix_cmd: &str) -> (String, String) {
    // If fix starts with "cd " and contains the failure command, it's a prepend_cd
    if fix_cmd.starts_with("cd ") && fix_cmd.contains("&&") {
        return (
            "prepend_cd".to_string(),
            "cd {{project_root}} && {{original_command}}".to_string(),
        );
    }

    let fail_tool = failure_cmd.split_whitespace().next().unwrap_or("");
    let fix_tool = fix_cmd.split_whitespace().next().unwrap_or("");

    // Same tool but different args — might be adding a flag
    if fail_tool == fix_tool && fix_cmd.len() > failure_cmd.len() {
        // The fix adds something to the command
        let extra = fix_cmd
            .strip_prefix(failure_cmd)
            .unwrap_or("")
            .trim()
            .to_string();
        if !extra.is_empty() && extra.starts_with('-') {
            return (
                "add_flag".to_string(),
                format!("{{{{original_command}}}} {extra}"),
            );
        }
    }

    // Default: full command rewrite
    ("rewrite_command".to_string(), fix_cmd.to_string())
}

/// Skill confidence lifecycle thresholds (mirror ARCHITECTURE.md):
/// CANDIDATE 0.3 (created, not auto-applied) → ACTIVE 0.7 (auto-applied) →
/// TRUSTED 0.9 (well-validated).
const CONF_ACTIVE: f64 = 0.7;
const CONF_TRUSTED: f64 = 0.9;
/// Minimum activations before CANDIDATE → ACTIVE.
const ACTIVATIONS_FOR_ACTIVE: i64 = 5;
/// Minimum activations before ACTIVE → TRUSTED.
const ACTIVATIONS_FOR_TRUSTED: i64 = 20;
/// Maximum failure rate (0.0–1.0) tolerated before auto-disabling.
const MAX_FAILURE_RATE: f64 = 0.20;
/// Minimum activations before auto-disable can fire (avoids disabling on 1/1).
const MIN_ACTIVATIONS_FOR_DISABLE: i64 = 5;

/// Evaluate mined skills against their activation stats and advance (or retire)
/// their confidence level. Intended to run on a schedule after activation stats
/// are imported.
///
/// Only `source = 'mined'` skills are affected — built-in skills are never
/// auto-demoted. Transitions:
/// - `activated >= 5` → confidence 0.3 → 0.7 (CANDIDATE → ACTIVE)
/// - `activated >= 20` and failure_rate < 5% → 0.7 → 0.9 (ACTIVE → TRUSTED)
/// - failure_rate > 20% (with >= 5 activations) → `enabled = 0` (auto-disabled)
pub fn tick_skill_lifecycle(conn: &Connection) -> Result<LifecycleSummary> {
    let mut summary = LifecycleSummary::default();

    let mut stmt = conn.prepare(
        "SELECT s.id, sa.confidence, ss.activated, ss.succeeded, ss.failed
         FROM skills s
         JOIN skill_actions sa ON sa.skill_id = s.id
         LEFT JOIN skill_stats ss ON ss.skill_id = s.id
         WHERE s.source = 'mined' AND s.enabled = 1",
    )?;

    struct SkillState {
        id: i64,
        confidence: f64,
        activated: i64,
        failed: i64,
    }

    let states: Vec<SkillState> = stmt
        .query_map([], |row| {
            Ok(SkillState {
                id: row.get(0)?,
                confidence: row.get(1)?,
                activated: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                failed: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    let now = crate::skills::chrono_now();

    for state in &states {
        let failure_rate = if state.activated > 0 {
            state.failed as f64 / state.activated as f64
        } else {
            0.0
        };

        // Auto-disable: high failure rate with enough data.
        if state.activated >= MIN_ACTIVATIONS_FOR_DISABLE && failure_rate > MAX_FAILURE_RATE {
            conn.execute(
                "UPDATE skills SET enabled = 0, updated_at = ?2 WHERE id = ?1",
                rusqlite::params![state.id, now],
            )?;
            summary.auto_disabled += 1;
            continue;
        }

        // Target confidence based on activation count and current level.
        let target_conf = if state.activated >= ACTIVATIONS_FOR_TRUSTED
            && failure_rate < 0.05
            && state.confidence >= CONF_ACTIVE
            && state.confidence < CONF_TRUSTED
        {
            Some(CONF_TRUSTED)
        } else if state.activated >= ACTIVATIONS_FOR_ACTIVE && state.confidence < CONF_ACTIVE {
            Some(CONF_ACTIVE)
        } else {
            None
        };

        if let Some(new_conf) = target_conf {
            conn.execute(
                "UPDATE skill_actions SET confidence = ?2 WHERE skill_id = ?1",
                rusqlite::params![state.id, new_conf],
            )?;
            conn.execute(
                "UPDATE skills SET updated_at = ?2 WHERE id = ?1",
                rusqlite::params![state.id, now],
            )?;

            if (new_conf - CONF_TRUSTED).abs() < f64::EPSILON {
                summary.promoted_to_trusted += 1;
            } else {
                summary.promoted_to_active += 1;
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_dbs() -> (Connection, Connection) {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let history = db::open_history(dir1.path()).unwrap();
        let heuristics = db::open_heuristics(dir2.path()).unwrap();
        // Leak the tempdirs so they live long enough
        std::mem::forget(dir1);
        std::mem::forget(dir2);
        (history, heuristics)
    }

    #[test]
    fn generate_skill_name_basic() {
        let name = generate_skill_name("cargo build", "cargo build --release");
        assert!(name.starts_with("mined-cargo-build-to-cargo-build"));
        assert!(name.len() < 80);
    }

    #[test]
    fn generate_trigger_regex_two_words() {
        let re = generate_trigger_regex("cargo build --release");
        assert!(re.starts_with(r"^cargo\s+build"));
    }

    #[test]
    fn generate_trigger_regex_one_word() {
        let re = generate_trigger_regex("make");
        assert!(re.starts_with(r"^make\s*"));
    }

    #[test]
    fn generate_action_cd_prefix() {
        let (atype, template) = generate_action("cargo build", "cd /home/user/proj && cargo build");
        assert_eq!(atype, "prepend_cd");
        assert!(template.contains("{{project_root}}"));
    }

    #[test]
    fn generate_action_add_flag() {
        let (atype, template) = generate_action("cargo build", "cargo build --release");
        assert_eq!(atype, "add_flag");
        assert!(template.contains("--release"));
    }

    #[test]
    fn generate_action_rewrite() {
        let (atype, template) = generate_action("npm test", "pnpm test");
        assert_eq!(atype, "rewrite_command");
        assert_eq!(template, "pnpm test");
    }

    #[test]
    fn promote_no_candidates() {
        let (history, heuristics) = test_dbs();
        let summary = promote_patterns(&history, &heuristics, None).unwrap();
        assert_eq!(summary.candidates_found, 0);
        assert_eq!(summary.skills_created, 0);
    }

    #[test]
    fn promote_creates_skill() {
        let (history, heuristics) = test_dbs();
        let now = crate::skills::chrono_now();

        // Insert a failure-fix pair with enough occurrences
        history
            .execute(
                "INSERT INTO sessions (session_id, project, started_at, mined_at)
                 VALUES ('test', 'test', ?1, ?1)",
                [&now],
            )
            .unwrap();

        history
            .execute(
                "INSERT INTO failure_fix_pairs
                 (failure_event, fix_event, pattern_hash, failure_command, failure_output,
                  fix_command, project_type, occurrences, created_at, updated_at)
                 VALUES (NULL, NULL, 'hash1', 'cargo build', 'error: not found',
                         'cargo build --release', 'rust', 5, ?1, ?1)",
                [&now],
            )
            .unwrap();

        let summary = promote_patterns(&history, &heuristics, Some(3)).unwrap();
        assert_eq!(summary.candidates_found, 1);
        assert_eq!(summary.skills_created, 1);

        // Verify skill was created
        let count: i64 = heuristics
            .query_row(
                "SELECT COUNT(*) FROM skills WHERE source = 'mined'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn promote_skips_edit_fixes() {
        let (history, heuristics) = test_dbs();
        let now = crate::skills::chrono_now();

        history
            .execute(
                "INSERT INTO sessions (session_id, project, started_at, mined_at)
                 VALUES ('test', 'test', ?1, ?1)",
                [&now],
            )
            .unwrap();

        history
            .execute(
                "INSERT INTO failure_fix_pairs
                 (failure_event, fix_event, pattern_hash, failure_command, failure_output,
                  fix_command, project_type, occurrences, created_at, updated_at)
                 VALUES (NULL, NULL, 'hash2', 'cargo build', 'error',
                         'edit:src/main.rs', 'rust', 10, ?1, ?1)",
                [&now],
            )
            .unwrap();

        let summary = promote_patterns(&history, &heuristics, Some(3)).unwrap();
        assert_eq!(summary.candidates_found, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.skills_created, 0);
    }

    #[test]
    fn promote_idempotent() {
        let (history, heuristics) = test_dbs();
        let now = crate::skills::chrono_now();

        history
            .execute(
                "INSERT INTO sessions (session_id, project, started_at, mined_at)
                 VALUES ('test', 'test', ?1, ?1)",
                [&now],
            )
            .unwrap();

        history
            .execute(
                "INSERT INTO failure_fix_pairs
                 (failure_event, fix_event, pattern_hash, failure_command, failure_output,
                  fix_command, project_type, occurrences, created_at, updated_at)
                 VALUES (NULL, NULL, 'hash3', 'npm test', 'fail',
                         'pnpm test', 'node', 5, ?1, ?1)",
                [&now],
            )
            .unwrap();

        let s1 = promote_patterns(&history, &heuristics, Some(3)).unwrap();
        assert_eq!(s1.skills_created, 1);

        let s2 = promote_patterns(&history, &heuristics, Some(3)).unwrap();
        assert_eq!(s2.skills_created, 0);
        assert_eq!(s2.already_exists, 1);
    }

    /// Insert a mined skill with one action and a stats row, returning its id.
    fn seed_mined_skill(
        conn: &Connection,
        name: &str,
        source: &str,
        confidence: f64,
        activated: i64,
        failed: i64,
    ) -> i64 {
        let now = crate::skills::chrono_now();
        conn.execute(
            "INSERT INTO skills (name, description, source, priority, created_at, updated_at)
             VALUES (?1, 'd', ?2, 100, ?3, ?3)",
            rusqlite::params![name, source, now],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO skill_actions (skill_id, action_type, template, confidence)
             VALUES (?1, 'rewrite_command', 'x', ?2)",
            rusqlite::params![id, confidence],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_stats (skill_id, activated, succeeded, failed)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, activated, activated - failed, failed],
        )
        .unwrap();
        id
    }

    fn action_conf(conn: &Connection, id: i64) -> f64 {
        conn.query_row(
            "SELECT confidence FROM skill_actions WHERE skill_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn is_enabled(conn: &Connection, id: i64) -> bool {
        conn.query_row("SELECT enabled FROM skills WHERE id = ?1", [id], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
            == 1
    }

    #[test]
    fn tick_promotes_candidate_to_active() {
        let (_h, conn) = test_dbs();
        let id = seed_mined_skill(&conn, "cand", "mined", 0.3, 5, 0);
        let s = tick_skill_lifecycle(&conn).unwrap();
        assert_eq!(s.promoted_to_active, 1);
        assert!((action_conf(&conn, id) - CONF_ACTIVE).abs() < f64::EPSILON);
    }

    #[test]
    fn tick_promotes_active_to_trusted() {
        let (_h, conn) = test_dbs();
        let id = seed_mined_skill(&conn, "act", "mined", 0.7, 20, 0);
        let s = tick_skill_lifecycle(&conn).unwrap();
        assert_eq!(s.promoted_to_trusted, 1);
        assert!((action_conf(&conn, id) - CONF_TRUSTED).abs() < f64::EPSILON);
    }

    #[test]
    fn tick_auto_disables_high_failure_rate() {
        let (_h, conn) = test_dbs();
        // 5 of 10 failed = 50% > 20%, with >= 5 activations.
        let id = seed_mined_skill(&conn, "bad", "mined", 0.7, 10, 5);
        let s = tick_skill_lifecycle(&conn).unwrap();
        assert_eq!(s.auto_disabled, 1);
        assert!(!is_enabled(&conn, id));
    }

    #[test]
    fn tick_leaves_low_activation_untouched() {
        let (_h, conn) = test_dbs();
        let id = seed_mined_skill(&conn, "young", "mined", 0.3, 2, 0);
        let s = tick_skill_lifecycle(&conn).unwrap();
        assert_eq!(s.promoted_to_active, 0);
        assert_eq!(s.promoted_to_trusted, 0);
        assert_eq!(s.auto_disabled, 0);
        assert!((action_conf(&conn, id) - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn tick_ignores_builtin_skills() {
        let (_h, conn) = test_dbs();
        // A builtin with stats that would otherwise promote — must be untouched.
        let id = seed_mined_skill(&conn, "builtin", "builtin", 0.3, 50, 0);
        let s = tick_skill_lifecycle(&conn).unwrap();
        assert_eq!(s.promoted_to_active, 0);
        assert!((action_conf(&conn, id) - 0.3).abs() < f64::EPSILON);
    }
}
