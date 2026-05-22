//! Compressor adapters: pluggable backends that wrap a recognised
//! command into a token-efficient invocation.
//!
//! A compressor is identified by name in the user's config; precc picks
//! one at startup and the hook stays oblivious to which one it is.
//! Default is [`NoneCompressor`] (no wrapping).

mod none;
mod rtk;

pub use none::NoneCompressor;
pub use rtk::RtkCompressor;

/// Backend that wraps a canonical invocation into a token-saving call.
pub trait Compressor {
    /// Short name (e.g. `"rtk"`, `"none"`). Used in config and telemetry.
    fn name(&self) -> &'static str;

    /// Whether this compressor performs no work. Default implementations
    /// of [`crate::rewrites::rewrite`] short-circuit when this is true.
    fn is_noop(&self) -> bool {
        false
    }

    /// Whether the backing tool is installed and runnable. Hot path —
    /// implementations should cache.
    fn is_available(&self) -> bool;

    /// True if `command` is already a wrapped invocation of this
    /// compressor (avoids double-wrapping).
    fn is_already_wrapped(&self, command: &str) -> bool;

    /// Produce the wrapped invocation. `canonical` is the rule's
    /// canonical form (without binary prefix); `tail` is everything
    /// that followed the matched prefix in the raw command (typically
    /// flags / args), beginning with a space if non-empty.
    fn wrap(&self, canonical: &str, tail: &str) -> String;
}

/// Pick a compressor by name. Unknown names fall back to `None`.
pub fn resolve(name: &str) -> Box<dyn Compressor + Send + Sync> {
    match name {
        "rtk" => Box::new(RtkCompressor),
        _ => Box::new(NoneCompressor),
    }
}

/// Process-wide active compressor, picked from `~/.config/precc/config.toml`
/// (key `compressor`). Defaults to [`NoneCompressor`]. Evaluated once.
pub fn active() -> &'static (dyn Compressor + Send + Sync) {
    use std::sync::LazyLock;
    static ACTIVE: LazyLock<Box<dyn Compressor + Send + Sync>> = LazyLock::new(|| {
        let name = read_configured_name().unwrap_or_else(|| "none".to_string());
        resolve(&name)
    });
    ACTIVE.as_ref()
}

fn read_configured_name() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::Path::new(&home).join(".config/precc/config.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    // Tiny parser: look for `compressor = "..."` on its own line.
    // Avoids dragging the full toml crate into the hook's hot path.
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("compressor") {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('=')?.trim();
            let rest = rest.strip_prefix('"')?;
            let end = rest.find('"')?;
            return Some(rest[..end].to_string());
        }
    }
    None
}
