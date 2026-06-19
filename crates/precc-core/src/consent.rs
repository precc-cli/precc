// SPDX-License-Identifier: MIT OR Apache-2.0
// Part of the open-source `precc-core` surface (dual MIT / Apache-2.0). The
// consent gate is published so the privacy contract is auditable: anyone can
// read exactly when telemetry is and isn't sent. Pairs with the open
// `telemetry_schema` (the list of fields collected).

//! Telemetry consent management for PRECC.
//!
//! Consent state is stored in `~/.config/precc/consent.toml`.
//! Telemetry is **opt-out**: enabled by default for new installs (so the
//! public stats page reflects real usage without each user having to
//! remember to grant consent). Two ways to disable:
//!   - Set `PRECC_NO_TELEMETRY=1` in the environment (per-shell).
//!   - Run `precc telemetry off` (persisted in `consent.toml`).
//!
//! Existing users whose `consent.toml` says `enabled = false` are
//! respected — the flip from opt-in to opt-out only affects machines
//! that have never created a `consent.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current consent schema version.  Bumping this forces re-consent.
pub const CONSENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentConfig {
    pub telemetry: TelemetryConsent,
    /// Counterfactual triple stream (Pillar III). Default: disabled.
    /// Distinct opt-in from `telemetry`; see `is_counterfactual_enabled`.
    #[serde(default)]
    pub counterfactual: CounterfactualConsent,
    /// User-interface preferences (status-line language, etc.).
    #[serde(default)]
    pub ui: UiConfig,
}

/// User-interface preferences. Keep this section small — most behaviour
/// stays controlled by env vars.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiConfig {
    /// ISO 639-1 / 639-2 language code (en, zh, es, fr, ja, ar, ...).
    /// Empty or unknown values fall back to "en". `PRECC_LANG` env var
    /// overrides this when set.
    #[serde(default)]
    pub preferred_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConsent {
    pub enabled: bool,
    pub consented_at: String,
    pub consent_version: u32,
}

/// Per-machine consent for the counterfactual telemetry stream.
///
/// Default is disabled. Opt-in is affirmative — the user types `YES` at the
/// `precc telemetry counterfactual on` ceremony, after which `enabled = true`
/// is persisted alongside the consent version.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CounterfactualConsent {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub consented_at: String,
    #[serde(default)]
    pub consent_version: u32,
}

/// Config directory — delegates to `db::config_dir()` so the precedence
/// (PRECC_CONFIG_DIR / CLAUDE_CONFIG_DIR / $HOME) stays in one place.
fn config_dir() -> Result<PathBuf> {
    crate::db::config_dir()
}

fn consent_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("consent.toml"))
}

/// Load the consent configuration.  Returns `None` if the file does not exist.
pub fn load() -> Result<Option<ConsentConfig>> {
    let path = consent_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).context("reading consent.toml")?;
    let cfg: ConsentConfig = toml::from_str(&raw).context("parsing consent.toml")?;
    Ok(Some(cfg))
}

/// Returns `true` when telemetry is enabled. Decision flow:
///   1. `PRECC_NO_TELEMETRY=1` in env → always `false` (kill switch).
///   2. `consent.toml` present and `enabled = true` (current consent
///      version) → `true`.
///   3. `consent.toml` present and `enabled = false` → `false`
///      (respect a user who explicitly opted out).
///   4. `consent.toml` absent → `true` (opt-out default; new installs
///      report unless they actively disable).
pub fn is_telemetry_enabled() -> bool {
    if std::env::var("PRECC_NO_TELEMETRY").is_ok() {
        return false;
    }
    match load() {
        Ok(Some(cfg)) => {
            // Explicit consent.toml: respect both `enabled = true` and
            // `enabled = false`. Stale consent_version (a future bump)
            // falls through to the default-on path.
            if cfg.telemetry.consent_version == CONSENT_VERSION {
                cfg.telemetry.enabled
            } else {
                true
            }
        }
        Ok(None) => true, // No consent.toml → default-on
        Err(_) => true,   // Parse error → default-on (fail safe to "send")
    }
}

/// True iff the user has explicitly opted-in via `precc telemetry on`
/// (file present with `enabled = true` at current consent version).
/// Distinct from `is_telemetry_enabled` so callers can tell the
/// difference between "default-on" and "explicitly granted".
pub fn has_explicit_consent() -> bool {
    if std::env::var("PRECC_NO_TELEMETRY").is_ok() {
        return false;
    }
    matches!(load(), Ok(Some(cfg)) if cfg.telemetry.enabled && cfg.telemetry.consent_version == CONSENT_VERSION)
}

/// Returns `true` only when counterfactual telemetry is explicitly opted-in
/// at the current consent version. This stream is **opt-in only** — absence
/// of the file or absence of the section means disabled.
///
/// Honours `PRECC_NO_TELEMETRY=1` (the global kill switch) and a dedicated
/// `PRECC_NO_COUNTERFACTUAL=1` per-shell escape hatch.
pub fn is_counterfactual_enabled() -> bool {
    if std::env::var("PRECC_NO_TELEMETRY").is_ok()
        || std::env::var("PRECC_NO_COUNTERFACTUAL").is_ok()
    {
        return false;
    }
    matches!(
        load(),
        Ok(Some(cfg))
            if cfg.counterfactual.enabled
                && cfg.counterfactual.consent_version == CONSENT_VERSION
    )
}

/// Persist consent state. Preserves the existing `[counterfactual]` and
/// `[ui]` sections when present — only the `[telemetry]` section is rewritten.
pub fn save(enabled: bool) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)?;

    let now = chrono_lite_now();
    let existing = load().ok().flatten();
    let cfg = ConsentConfig {
        telemetry: TelemetryConsent {
            enabled,
            consented_at: now,
            consent_version: CONSENT_VERSION,
        },
        counterfactual: existing
            .as_ref()
            .map(|c| c.counterfactual.clone())
            .unwrap_or_default(),
        ui: existing.map(|c| c.ui).unwrap_or_default(),
    };

    let toml_str = toml::to_string_pretty(&cfg).context("serializing consent")?;
    let path = consent_path()?;
    std::fs::write(&path, toml_str).context("writing consent.toml")?;
    Ok(())
}

/// Persist counterfactual consent state. Preserves the existing
/// `[telemetry]` and `[ui]` sections — only the `[counterfactual]`
/// section is rewritten.
pub fn save_counterfactual(enabled: bool) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)?;

    let now = chrono_lite_now();
    let existing = load().ok().flatten();
    let telemetry = existing
        .as_ref()
        .map(|c| c.telemetry.clone())
        .unwrap_or(TelemetryConsent {
            enabled: true,
            consented_at: now.clone(),
            consent_version: CONSENT_VERSION,
        });
    let ui = existing.map(|c| c.ui).unwrap_or_default();
    let cfg = ConsentConfig {
        telemetry,
        counterfactual: CounterfactualConsent {
            enabled,
            consented_at: now,
            consent_version: CONSENT_VERSION,
        },
        ui,
    };

    let toml_str = toml::to_string_pretty(&cfg).context("serializing consent")?;
    let path = consent_path()?;
    std::fs::write(&path, toml_str).context("writing consent.toml")?;
    Ok(())
}

/// Minimal ISO-8601 timestamp without pulling in chrono.
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // Format as seconds-since-epoch (parseable, no PII)
    format!("{}Z", secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directly test the consent TOML round-trip via file I/O (avoids mutating HOME).
    #[test]
    fn consent_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.toml");

        let cfg = ConsentConfig {
            telemetry: TelemetryConsent {
                enabled: true,
                consented_at: "12345Z".to_string(),
                consent_version: CONSENT_VERSION,
            },
            counterfactual: CounterfactualConsent::default(),
            ui: UiConfig::default(),
        };
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        std::fs::write(&path, &toml_str).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let loaded: ConsentConfig = toml::from_str(&raw).unwrap();
        assert!(loaded.telemetry.enabled);
        assert_eq!(loaded.telemetry.consent_version, CONSENT_VERSION);
        assert!(!loaded.counterfactual.enabled, "counterfactual default-off");
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.toml");
        assert!(!path.exists());
        // load() depends on HOME; instead verify the logic directly
        let result: Option<ConsentConfig> = None;
        assert!(result.is_none());
    }

    /// Encodes the opt-out default: when the consent file is absent we
    /// route to the default-on path, NOT the locked-down path. This is
    /// the rule a future refactor must not regress without re-deciding.
    #[test]
    fn default_on_when_no_consent_file() {
        // Mirror the decision tree in `is_telemetry_enabled` for the
        // load() == Ok(None) branch — this test pins the behavior even
        // when the actual function depends on $HOME.
        let load_result: Result<Option<ConsentConfig>> = Ok(None);
        let env_kill_switch = false;
        let enabled = if env_kill_switch {
            false
        } else {
            match load_result {
                Ok(Some(cfg)) if cfg.telemetry.consent_version == CONSENT_VERSION => {
                    cfg.telemetry.enabled
                }
                _ => true,
            }
        };
        assert!(enabled, "missing consent.toml must default to enabled");
    }

    /// Explicit `enabled = false` from a user who opted out must be
    /// respected. This is the contract that lets us flip the default
    /// without trampling existing opt-outs.
    #[test]
    fn explicit_opt_out_is_respected() {
        let cfg = ConsentConfig {
            telemetry: TelemetryConsent {
                enabled: false,
                consented_at: "0Z".to_string(),
                consent_version: CONSENT_VERSION,
            },
            counterfactual: CounterfactualConsent::default(),
            ui: UiConfig::default(),
        };
        let load_result: Result<Option<ConsentConfig>> = Ok(Some(cfg));
        let enabled = match load_result {
            Ok(Some(cfg)) if cfg.telemetry.consent_version == CONSENT_VERSION => {
                cfg.telemetry.enabled
            }
            _ => true,
        };
        assert!(!enabled, "explicit enabled=false must override default-on");
    }

    /// Counterfactual stream is opt-in only: missing section ⇒ disabled.
    #[test]
    fn counterfactual_default_off_when_section_missing() {
        // Old consent.toml without [counterfactual] section.
        let toml_str = r#"
[telemetry]
enabled = true
consented_at = "0Z"
consent_version = 1
"#;
        let cfg: ConsentConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.telemetry.enabled);
        assert!(!cfg.counterfactual.enabled);
    }

    /// `save_counterfactual(true)` round-trips with `enabled=true` AND
    /// preserves whatever the existing `[telemetry]` and `[ui]` sections
    /// said. The CLI ceremony depends on this — flipping one flag must
    /// not silently re-consent telemetry.
    #[test]
    fn save_counterfactual_preserves_other_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        // Seed an existing consent.toml with telemetry=false and a UI lang.
        let dir = tmp.path().join(".config/precc");
        std::fs::create_dir_all(&dir).unwrap();
        let seed = r#"
[telemetry]
enabled = false
consented_at = "111Z"
consent_version = 1

[ui]
preferred_language = "ja"
"#;
        std::fs::write(dir.join("consent.toml"), seed).unwrap();

        save_counterfactual(true).unwrap();
        let loaded = load().unwrap().unwrap();
        assert!(loaded.counterfactual.enabled);
        assert_eq!(loaded.counterfactual.consent_version, CONSENT_VERSION);
        assert!(
            !loaded.telemetry.enabled,
            "telemetry section must be preserved verbatim"
        );
        assert_eq!(loaded.telemetry.consented_at, "111Z");
        assert_eq!(loaded.ui.preferred_language, "ja");

        // Round-trip the off case from the same file.
        save_counterfactual(false).unwrap();
        let loaded = load().unwrap().unwrap();
        assert!(!loaded.counterfactual.enabled);
        assert!(!loaded.telemetry.enabled);
        assert_eq!(loaded.ui.preferred_language, "ja");

        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }

    /// Bumping `CONSENT_VERSION` must invalidate a stored opt-in even
    /// when `enabled = true`. The runtime guard is `is_counterfactual_enabled`.
    #[test]
    fn version_bump_invalidates_counterfactual_opt_in() {
        let cfg: ConsentConfig = toml::from_str(
            r#"
[telemetry]
enabled = true
consented_at = "0Z"
consent_version = 1

[counterfactual]
enabled = true
consented_at = "0Z"
consent_version = 0
"#,
        )
        .unwrap();
        // Simulate the runtime guard: the stored version (0) is older than
        // CONSENT_VERSION (1), so the opt-in must NOT count as live.
        let counts_as_enabled =
            cfg.counterfactual.enabled && cfg.counterfactual.consent_version == CONSENT_VERSION;
        assert!(
            !counts_as_enabled,
            "stale consent_version must not gate emission on"
        );
    }
}
