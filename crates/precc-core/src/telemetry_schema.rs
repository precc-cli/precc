// SPDX-License-Identifier: MIT OR Apache-2.0
// Part of the open-source `precc-core` surface (dual MIT / Apache-2.0),
// extracted so the telemetry *schema* — the exhaustive list of fields PRECC
// reports — is publicly auditable, independent of the (private) value
// derivation in `telemetry.rs`. "You can read exactly what it collects."
//
// These are pure serde data definitions: no command text, paths, usernames, or
// IPs are ever fields here. Every member is a usage count, a boolean
// capability flag, or an anonymous hash. Telemetry is opt-out (one command to
// disable); the per-command counterfactual stream is separately opt-in.

//! Telemetry payload schema for PRECC — the wire format of an anonymous report.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct TelemetryPayload {
    pub schema_version: u32,
    pub precc_version: String,
    pub os: String,
    pub arch: String,
    pub tier: String,
    /// Stable anonymous machine identifier — first 16 hex chars of
    /// SHA256(machine_id || username). Used by the server to dedup reports
    /// from the same machine across IP changes (NAT, VPN, mobile networks).
    pub machine_hash: String,
    /// Stable anonymous user identifier — first 16 hex chars of
    /// SHA256(lowercase(email)) for users who registered a license/trial.
    /// `None` for users with no email on file. Lets the server aggregate
    /// reports from multiple machines belonging to the same user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_hash: Option<String>,
    /// LLM model behind this session, when detectable from env. Lets the
    /// aggregator on precc.cc apply per-model pricing instead of
    /// universal Opus rates — necessary for accurate $-savings now that
    /// Cursor (GPT/Gemini/Llama) and other non-Anthropic surfaces are
    /// supported. `None` on older clients or when the env doesn't reveal
    /// a model; aggregator falls back to a documented default (current
    /// claude-opus-4-5) and only counts those records under "tokens
    /// saved", not "$ saved", to avoid fabricated cost numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub skills: Vec<SkillTelemetry>,
    pub pillars: PillarTelemetry,
    pub hook_latency: LatencyTelemetry,
    /// Total API-relevant tokens across all sessions (denominator for savings %).
    /// Computed from session JSONL breakdown. 0 if unavailable.
    pub total_api_tokens: u64,
    /// Real per-token-type counts pulled from each assistant message's
    /// `usage` subobject in the session JSONL files. These are summed
    /// across all sessions in scope (since metrics.db's earliest
    /// timestamp). The dollar cost is then `Σ(tokens × per-MTok rate)`
    /// at the model's rate, which is the only way to get an accurate
    /// session-cost figure — `total_api_tokens` is a byte-proxy and
    /// undercounts severely once cache reads dominate the spend.
    #[serde(default)]
    pub api_input_tokens: u64,
    #[serde(default)]
    pub api_output_tokens: u64,
    #[serde(default)]
    pub api_cache_read_tokens: u64,
    #[serde(default)]
    pub api_cache_write_tokens: u64,
    /// Number of assistant turns (i.e. count of `usage` blocks the
    /// per-token-type sums above were drawn from). Lets the public
    /// stats page show "N turns produced X output" — turns out to be
    /// the missing context for why a machine with many short sessions
    /// can look "cheaper" than one with a few long sessions, even when
    /// the former has more total bytes on disk.
    #[serde(default)]
    pub assistant_turns: u64,
    /// Earliest event timestamp in metrics.db (unix epoch seconds) — the
    /// `since` filter that bounds the session JSONL window. Lets the
    /// stats page show "metrics window: 28 days" to explain shorter
    /// totals on machines with younger DBs.
    #[serde(default)]
    pub metrics_db_earliest: u64,
    /// Measured tokens saved (ground truth from re-running original commands).
    pub compression_tokens_saved: u64,
    /// Combined savings: pillars + measured compression.
    pub combined_tokens_saved: u64,
    /// Ground-truth measurement details.
    pub measured: MeasuredTelemetry,
    /// Per-task-type token totals across recently-active sessions on
    /// this machine, derived from the abtop classifier (rule-based).
    /// Map key: task type ("coding", "refactor", "debugging", …).
    /// Map value: cumulative `input + output + cache_read + cache_write`
    /// tokens summed over sessions classified as that type.
    ///
    /// Empty `{}` until v0.3.27+ — older clients do not populate it.
    /// Lets the public stats page surface "PRECC's saving ratio is
    /// X% on debugging vs Y% on coding" once the field has been
    /// reported by enough clients to be meaningful.
    #[serde(default)]
    pub tokens_by_task_type: std::collections::BTreeMap<String, u64>,
    /// Per-command-class strata for the stratified savings estimator. Each
    /// entry pairs a class's true population frequency (observed Bash
    /// invocations) with its measured savings, so the fleet aggregator can
    /// weight measured means by class frequency instead of extrapolating a
    /// single biased mean across all invocations. Capped to the top classes
    /// by population to bound payload. Empty on clients < v0.3.82.
    #[serde(default)]
    pub class_strata: Vec<ClassStratumTelemetry>,
    /// Per-machine capability/enablement snapshot. Unlike every other field
    /// (usage counts), this records which optional features are *configured* —
    /// the signal needed to compute fleet config-adoption and, with measured
    /// savings, to suggest under-used features at the `doctor` stage. Booleans
    /// only, no PII. Empty/default on clients < v0.3.83.
    #[serde(default)]
    pub capabilities: CapabilitySnapshot,
    /// Execution environment: `"claude-sandbox"` (Cowork's document-skill
    /// sandbox, detected via the host-proxy env vars) or `"host"` (a normal
    /// Claude Code session). Lets the fleet segment document-token work
    /// Cowork-vs-Code. Empty on clients < v0.3.89.
    #[serde(default)]
    pub harness: String,
    /// Count of Bash invocations whose hook ran inside a Claude sandbox
    /// (Cowork's document-skill environment) — the `harness_claude_sandbox`
    /// metric, recorded in the execution context where the sandbox env is
    /// visible. >0 robustly identifies a Cowork machine even when telemetry is
    /// sent host-side. Empty on clients < v0.3.93.
    #[serde(default)]
    pub harness_sandbox_hooks: u64,
    /// Document operations observed (Cowork pptx/xlsx/pdf work): count of Bash
    /// commands touching an Office/PDF file, and the total output tokens they
    /// produced. Measures whether document work is a token sink before heavier
    /// compression is built. Empty on clients < v0.3.88/0.3.93.
    #[serde(default)]
    pub doc_op_count: u64,
    #[serde(default)]
    pub doc_op_tokens: u64,
    /// Document-guard advisories fired (`cat` of a binary Office/PDF, whole-PDF
    /// rasterization) — the avoidable-mistake count.
    #[serde(default)]
    pub doc_advisory_count: u64,
}

/// Telemetry view of a command-class stratum — a command class paired with
/// its population frequency and measured savings.
#[derive(Debug, Serialize, Default)]
pub struct ClassStratumTelemetry {
    pub cmd_class: String,
    pub population_count: u64,
    pub measured_count: u64,
    pub measured_savings_total: u64,
}

/// Which optional features are installed/enabled on this machine. For each
/// compression backend we report both `available` (binary present) and
/// `enabled` (present AND switched on) so the aggregator can distinguish
/// three states — not installed, installed-but-off, and on — and recommend
/// the right remedy ("install lean-ctx" vs "set `PRECC_LOWFAT=1`"). All
/// booleans; carries no commands, paths, or identifiers.
#[derive(Debug, Serialize, Default)]
pub struct CapabilitySnapshot {
    pub lean_ctx_available: bool,
    pub lean_ctx_enabled: bool,
    pub nushell_available: bool,
    pub nushell_enabled: bool,
    pub lowfat_available: bool,
    pub lowfat_enabled: bool,
    /// compression-prompt (`compress` CLI) — opt-in measured backend.
    pub compression_prompt_available: bool,
    pub compression_prompt_enabled: bool,
    /// RTK is gated only on the binary being present (no separate toggle).
    pub rtk_available: bool,
    /// diet is rule-based (no binary); on by default unless `PRECC_DIET=0`.
    pub diet_enabled: bool,
    /// Counterfactual (Pillar III) stream — affirmative opt-in.
    pub counterfactual_enabled: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct MeasuredTelemetry {
    /// Total original output tokens (what would have been without PRECC).
    pub original_output_tokens: u64,
    /// Total actual output tokens (with PRECC compression).
    pub actual_output_tokens: u64,
    /// Total tokens saved by compression (original - actual).
    pub savings_tokens: u64,
    /// Savings percentage (savings / original × 100).
    pub savings_pct: f64,
    /// Number of ground-truth measurements taken.
    pub ground_truth_count: u64,
    /// Total measurements (including skipped unsafe commands).
    pub measurement_count: u64,
    /// Per-rewrite-type breakdown.
    pub by_rewrite_type: Vec<RewriteTypeTelemetry>,
}

#[derive(Debug, Serialize)]
pub struct RewriteTypeTelemetry {
    pub rewrite_type: String,
    pub count: u64,
    pub avg_savings_pct: f64,
    pub total_savings_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct SkillTelemetry {
    pub name: String,
    pub source: String,
    pub activated: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub est_tokens_saved: f64,
}

#[derive(Debug, Serialize)]
pub struct PillarTelemetry {
    pub rtk_rewrites: i64,
    pub rtk_tokens_saved: f64,
    pub cd_prepends: i64,
    pub cd_tokens_saved: f64,
    pub skill_activations: i64,
    pub skill_tokens_saved: f64,
    pub mined_preventions: i64,
    pub mined_tokens_saved: f64,
    pub lean_ctx_wraps: i64,
    pub lean_ctx_tokens_saved: f64,
}

#[derive(Debug, Serialize)]
pub struct LatencyTelemetry {
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub count: u64,
}
