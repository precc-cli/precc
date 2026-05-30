//! Measurement framework for tracking PRECC effectiveness.
//!
//! Records hook latency, skill activations, and pipeline decisions
//! into metrics.db for later analysis via `precc report`.

use anyhow::Result;
use rusqlite::Connection;

/// Metric types recorded by the hook.
pub enum MetricType {
    HookLatency,
    SkillActivation,
    CdPrepend,
    GdbSuggestion,
    CompressorWrap,
    MinerTick,
}

impl MetricType {
    fn as_str(&self) -> &'static str {
        match self {
            MetricType::HookLatency => "hook_latency",
            MetricType::SkillActivation => "skill_activation",
            MetricType::CdPrepend => "cd_prepend",
            MetricType::GdbSuggestion => "gdb_suggestion",
            MetricType::CompressorWrap => "compressor_wrap",
            MetricType::MinerTick => "miner_tick",
        }
    }
}

/// Record a metric into metrics.db.
pub fn record(
    conn: &Connection,
    metric_type: MetricType,
    value: f64,
    metadata: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO metrics (timestamp, metric_type, value, metadata)
         VALUES (datetime('now'), ?1, ?2, ?3)",
        rusqlite::params![metric_type.as_str(), value, metadata],
    )?;
    Ok(())
}

/// Record hook latency in milliseconds.
pub fn record_latency(conn: &Connection, latency_ms: f64) -> Result<()> {
    record(conn, MetricType::HookLatency, latency_ms, None)
}

/// Summary stats for a metric type.
#[derive(Debug)]
pub struct MetricSummary {
    pub count: u64,
    pub total: f64,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
}

/// Get summary statistics for a metric type.
pub fn summary(conn: &Connection, metric_type: MetricType) -> Result<Option<MetricSummary>> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*), COALESCE(SUM(value), 0), COALESCE(AVG(value), 0),
                COALESCE(MIN(value), 0), COALESCE(MAX(value), 0)
         FROM metrics WHERE metric_type = ?1",
    )?;

    let result = stmt.query_row([metric_type.as_str()], |row| {
        Ok(MetricSummary {
            count: row.get(0)?,
            total: row.get(1)?,
            avg: row.get(2)?,
            min: row.get(3)?,
            max: row.get(4)?,
        })
    })?;

    if result.count == 0 {
        Ok(None)
    } else {
        Ok(Some(result))
    }
}

// ─── Savings measurements import + queries ─────────────────────────────────
//
// A compression measurement records how many output tokens a rewrite saved on
// a given command. The hook (or a probe) appends measurements as JSONL to
// `savings_measurements.jsonl`; `import_savings_log` folds them into the
// `savings_measurements` table, and the query helpers below feed `precc
// report` / statusline / re-measurement caching.

/// Import pending savings measurements from the JSONL log into metrics.db.
///
/// Uses an atomic rename so the writer can keep appending a fresh log while we
/// import, avoiding double-counting. Returns the number of rows imported.
pub fn import_savings_log(conn: &Connection, data_dir: &std::path::Path) -> Result<usize> {
    let log_path = data_dir.join("savings_measurements.jsonl");
    if !log_path.exists() {
        return Ok(0);
    }

    let processing_path = data_dir.join("savings_measurements.jsonl.processing");
    if std::fs::rename(&log_path, &processing_path).is_err() {
        // Another importer grabbed it first — skip silently.
        return Ok(0);
    }

    let content = match std::fs::read_to_string(&processing_path) {
        Ok(c) => c,
        Err(_) => {
            let _ = std::fs::remove_file(&processing_path);
            return Ok(0);
        }
    };

    let mut count = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let _ = conn.execute(
            "INSERT INTO savings_measurements (timestamp, cmd_class, rewrite_type, compression_mode, probe_kind, session_id, original_output_tokens, actual_output_tokens, savings_tokens, savings_pct, measurement_method)
             VALUES (datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                v.get("cmd_class").and_then(|s| s.as_str()).unwrap_or(""),
                v.get("rewrite_type").and_then(|s| s.as_str()).unwrap_or(""),
                v.get("compression_mode").and_then(|s| s.as_str()).unwrap_or("basic"),
                v.get("probe_kind").and_then(|s| s.as_str()).unwrap_or("live"),
                v.get("session_id").and_then(|s| s.as_str()).unwrap_or(""),
                v.get("original_output_tokens").and_then(|n| n.as_i64()).unwrap_or(0),
                v.get("actual_output_tokens").and_then(|n| n.as_i64()).unwrap_or(0),
                v.get("savings_tokens").and_then(|n| n.as_i64()).unwrap_or(0),
                v.get("savings_pct").and_then(|n| n.as_f64()).unwrap_or(0.0),
                v.get("measurement_method").and_then(|s| s.as_str()).unwrap_or(""),
            ],
        );
        count += 1;
    }

    let _ = std::fs::remove_file(&processing_path);
    Ok(count)
}

/// Measured savings totals from the `savings_measurements` table.
#[derive(Debug, Default)]
pub struct MeasuredSavings {
    pub original_total: u64,
    pub actual_total: u64,
    pub savings_total: u64,
    pub savings_pct: f64,
    pub measurement_count: u64,
    pub ground_truth_count: u64,
}

/// Query total measured savings across all recorded measurements.
pub fn total_measured_savings(conn: &Connection) -> Result<MeasuredSavings> {
    let result = conn.query_row(
        "SELECT COALESCE(SUM(original_output_tokens), 0),
                COALESCE(SUM(actual_output_tokens), 0),
                COALESCE(SUM(savings_tokens), 0),
                COUNT(*),
                COALESCE(SUM(CASE WHEN measurement_method = 'ground_truth' THEN 1 ELSE 0 END), 0)
         FROM savings_measurements",
        [],
        |r| {
            let orig: i64 = r.get(0)?;
            let actual: i64 = r.get(1)?;
            let savings: i64 = r.get(2)?;
            let count: i64 = r.get(3)?;
            let gt_count: i64 = r.get(4)?;
            Ok(MeasuredSavings {
                original_total: orig as u64,
                actual_total: actual as u64,
                savings_total: savings as u64,
                savings_pct: if orig > 0 {
                    savings as f64 / orig as f64 * 100.0
                } else {
                    0.0
                },
                measurement_count: count as u64,
                ground_truth_count: gt_count as u64,
            })
        },
    )?;
    Ok(result)
}

/// Most recent measurement with non-zero savings. Used by the statusline to
/// surface the latest interaction's compression result. Returns
/// `(savings_tokens, savings_pct, compression_mode)` or `None`.
pub fn latest_measurement_with_savings(conn: &Connection) -> Result<Option<(u64, f64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT savings_tokens, savings_pct, compression_mode
         FROM savings_measurements
         WHERE savings_tokens > 0
         ORDER BY id DESC
         LIMIT 1",
    )?;
    let row = stmt
        .query_row([], |r| {
            let saved: i64 = r.get(0)?;
            let pct: f64 = r.get(1)?;
            let mode: String = r.get(2).unwrap_or_else(|_| "?".to_string());
            Ok((saved as u64, pct, mode))
        })
        .ok();
    Ok(row)
}

/// Per-rewrite-type savings breakdown.
#[derive(Debug)]
pub struct RewriteTypeSavings {
    pub rewrite_type: String,
    pub count: u64,
    pub avg_savings_pct: f64,
    pub total_savings_tokens: u64,
}

/// Check for a recent live measurement of this `cmd_class` + `compression_mode`.
///
/// Returns `Some((original_tokens, age_seconds))` if one exists within
/// `ttl_seconds`, so callers can skip re-measuring expensive commands (e.g.
/// `cargo build`) on every invocation.
pub fn cached_measurement(
    conn: &Connection,
    cmd_class: &str,
    compression_mode: &str,
    ttl_seconds: i64,
) -> Result<Option<(u64, i64)>> {
    let row: Option<(i64, i64)> = conn
        .query_row(
            "SELECT original_output_tokens,
                    CAST((julianday('now') - julianday(timestamp)) * 86400 AS INTEGER) AS age_secs
             FROM savings_measurements
             WHERE cmd_class = ?1
               AND compression_mode = ?2
               AND probe_kind = 'live'
             ORDER BY timestamp DESC
             LIMIT 1",
            rusqlite::params![cmd_class, compression_mode],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();

    match row {
        Some((tokens, age)) if age <= ttl_seconds => Ok(Some((tokens as u64, age))),
        _ => Ok(None),
    }
}

/// Historical baseline (avg `original_output_tokens`, count) for a `cmd_class`
/// over live measurements. Used for output-too-small detection.
pub fn historical_baseline(conn: &Connection, cmd_class: &str) -> Result<(f64, i64)> {
    let row = conn.query_row(
        "SELECT COALESCE(AVG(original_output_tokens), 0), COUNT(*)
         FROM savings_measurements
         WHERE cmd_class = ?1 AND probe_kind = 'live'",
        rusqlite::params![cmd_class],
        |r| Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(row)
}

/// Savings grouped by rewrite type, ordered by total tokens saved (desc).
pub fn savings_by_rewrite_type(conn: &Connection) -> Result<Vec<RewriteTypeSavings>> {
    let mut stmt = conn.prepare(
        "SELECT rewrite_type, COUNT(*), AVG(savings_pct), SUM(savings_tokens)
         FROM savings_measurements
         GROUP BY rewrite_type
         ORDER BY SUM(savings_tokens) DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RewriteTypeSavings {
                rewrite_type: r.get(0)?,
                count: r.get::<_, i64>(1)? as u64,
                avg_savings_pct: r.get(2)?,
                total_savings_tokens: r.get::<_, i64>(3)? as u64,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_db() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        db::open_metrics(dir.path()).unwrap()
    }

    #[test]
    fn record_and_query_metric() {
        let conn = test_db();
        record(&conn, MetricType::HookLatency, 2.5, None).unwrap();
        record(&conn, MetricType::HookLatency, 3.5, None).unwrap();

        let s = summary(&conn, MetricType::HookLatency).unwrap().unwrap();
        assert_eq!(s.count, 2);
        assert!((s.avg - 3.0).abs() < 0.01);
        assert!((s.min - 2.5).abs() < 0.01);
        assert!((s.max - 3.5).abs() < 0.01);
    }

    #[test]
    fn no_metrics_returns_none() {
        let conn = test_db();
        let s = summary(&conn, MetricType::SkillActivation).unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn record_with_metadata() {
        let conn = test_db();
        record(
            &conn,
            MetricType::SkillActivation,
            1.0,
            Some(r#"{"skill":"cargo-wrong-dir"}"#),
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metrics WHERE metadata IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // ─── Savings measurements ──────────────────────────────────────────────

    /// Two-row JSONL fixture: a ground-truth `cargo build`/rtk saving 300, and
    /// an estimated `git status`/diet saving 100.
    const SAVINGS_FIXTURE: &str = concat!(
        r#"{"cmd_class":"cargo build","rewrite_type":"rtk","compression_mode":"rtk","probe_kind":"live","session_id":"s1","original_output_tokens":1000,"actual_output_tokens":700,"savings_tokens":300,"savings_pct":30.0,"measurement_method":"ground_truth"}"#,
        "\n",
        r#"{"cmd_class":"git status","rewrite_type":"diet","compression_mode":"diet","probe_kind":"live","session_id":"s1","original_output_tokens":500,"actual_output_tokens":400,"savings_tokens":100,"savings_pct":20.0,"measurement_method":"estimate"}"#,
        "\n",
    );

    fn seed_savings(dir: &std::path::Path, conn: &Connection) -> usize {
        std::fs::write(dir.join("savings_measurements.jsonl"), SAVINGS_FIXTURE).unwrap();
        import_savings_log(conn, dir).unwrap()
    }

    #[test]
    fn import_savings_log_roundtrip_and_total() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();

        assert_eq!(seed_savings(dir.path(), &conn), 2);
        // Log is consumed by the atomic rename.
        assert!(!dir.path().join("savings_measurements.jsonl").exists());

        let s = total_measured_savings(&conn).unwrap();
        assert_eq!(s.original_total, 1500);
        assert_eq!(s.actual_total, 1100);
        assert_eq!(s.savings_total, 400);
        assert_eq!(s.measurement_count, 2);
        assert_eq!(s.ground_truth_count, 1);
        assert!((s.savings_pct - 400.0 / 1500.0 * 100.0).abs() < 0.01);
    }

    #[test]
    fn total_measured_savings_empty() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        let s = total_measured_savings(&conn).unwrap();
        assert_eq!(s.measurement_count, 0);
        assert_eq!(s.savings_total, 0);
        assert_eq!(s.savings_pct, 0.0);
    }

    #[test]
    fn import_savings_log_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        assert_eq!(import_savings_log(&conn, dir.path()).unwrap(), 0);
    }

    #[test]
    fn savings_by_rewrite_type_groups_desc() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        seed_savings(dir.path(), &conn);

        let groups = savings_by_rewrite_type(&conn).unwrap();
        assert_eq!(groups.len(), 2);
        // Ordered by total tokens saved descending: rtk (300) before diet (100).
        assert_eq!(groups[0].rewrite_type, "rtk");
        assert_eq!(groups[0].total_savings_tokens, 300);
        assert_eq!(groups[1].rewrite_type, "diet");
        assert_eq!(groups[1].total_savings_tokens, 100);
    }

    #[test]
    fn latest_measurement_with_savings_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        seed_savings(dir.path(), &conn);

        // Highest id (last imported) with savings > 0 is the diet row.
        let (saved, pct, mode) = latest_measurement_with_savings(&conn).unwrap().unwrap();
        assert_eq!(saved, 100);
        assert!((pct - 20.0).abs() < 0.01);
        assert_eq!(mode, "diet");
    }

    #[test]
    fn cached_measurement_respects_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        seed_savings(dir.path(), &conn);

        // Fresh row (age ~0) is within a generous TTL.
        let hit = cached_measurement(&conn, "cargo build", "rtk", 3600).unwrap();
        assert_eq!(hit.map(|(tokens, _age)| tokens), Some(1000));

        // Negative TTL can never be satisfied (age 0 > -1).
        assert!(cached_measurement(&conn, "cargo build", "rtk", -1)
            .unwrap()
            .is_none());

        // Unknown cmd_class / mode misses.
        assert!(cached_measurement(&conn, "cargo build", "diet", 3600)
            .unwrap()
            .is_none());
    }

    #[test]
    fn historical_baseline_averages_live_only() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_metrics(dir.path()).unwrap();
        seed_savings(dir.path(), &conn);

        let (avg, count) = historical_baseline(&conn, "cargo build").unwrap();
        assert_eq!(count, 1);
        assert!((avg - 1000.0).abs() < 0.01);

        // No measurements for an unseen class.
        let (avg0, count0) = historical_baseline(&conn, "npm install").unwrap();
        assert_eq!(count0, 0);
        assert_eq!(avg0, 0.0);
    }
}
