//! RTK compressor adapter.
//!
//! Wraps a canonical command in an `rtk <canonical>` invocation. RTK
//! (<https://github.com/rtk-ai/rtk>) is a CLI output compressor; precc
//! supports it as an optional backend but does not require it. Users
//! opt in via `compressor = "rtk"` in `~/.config/precc/config.toml`.

use super::Compressor;
use std::sync::LazyLock;

pub struct RtkCompressor;

impl Compressor for RtkCompressor {
    fn name(&self) -> &'static str {
        "rtk"
    }

    fn is_available(&self) -> bool {
        *RTK_AVAILABLE
    }

    fn is_already_wrapped(&self, command: &str) -> bool {
        command.starts_with("rtk ") || command.contains("/rtk ")
    }

    fn wrap(&self, canonical: &str, tail: &str) -> String {
        format!("rtk {canonical}{tail}")
    }
}

/// Cached `rtk` availability check. Performed once per process. Looks
/// at the marker file first, then common locations, then walks PATH.
static RTK_AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
    if let Ok(home) = std::env::var("HOME") {
        let cache = std::path::Path::new(&home).join(".local/share/precc/.rtk_path");
        if let Ok(cached_path) = std::fs::read_to_string(&cache) {
            let p = cached_path.trim();
            if !p.is_empty() && std::path::Path::new(p).is_file() {
                return true;
            }
        }
        let common = [
            format!("{home}/.cargo/bin/rtk"),
            "/usr/local/bin/rtk".to_string(),
            "/usr/bin/rtk".to_string(),
        ];
        for path in &common {
            if std::path::Path::new(path).is_file() {
                let _ = std::fs::write(&cache, path);
                return true;
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join("rtk");
            if candidate.is_file() {
                if let Ok(home) = std::env::var("HOME") {
                    let cache = std::path::Path::new(&home).join(".local/share/precc/.rtk_path");
                    let _ = std::fs::write(&cache, candidate.to_string_lossy().as_ref());
                }
                return true;
            }
        }
    }
    false
});
