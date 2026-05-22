use super::Compressor;

/// The no-op compressor. The default — precc ships zero assumptions
/// about external tooling and will only wrap commands once the user
/// opts in by configuring a different compressor.
pub struct NoneCompressor;

impl Compressor for NoneCompressor {
    fn name(&self) -> &'static str {
        "none"
    }
    fn is_noop(&self) -> bool {
        true
    }
    fn is_available(&self) -> bool {
        true
    }
    fn is_already_wrapped(&self, _command: &str) -> bool {
        false
    }
    fn wrap(&self, canonical: &str, tail: &str) -> String {
        format!("{canonical}{tail}")
    }
}
