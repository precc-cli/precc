//! Pluggable command rewriting.
//!
//! precc recognises commands whose output is verbose enough to be worth
//! compressing, then asks a configured [`Compressor`] to wrap them. The
//! recognition layer (this module) is independent of the wrapper tool;
//! adapters live in [`crate::compressors`]. Default target is `None`,
//! which means no wrapping is performed.

use crate::compressors::Compressor;

/// A wrappable invocation: if a raw command starts with `from`, it is a
/// candidate for compression and the canonical form to wrap is `canonical`.
pub struct RewriteRule {
    /// Prefix that flags the command as wrappable.
    pub from: &'static str,
    /// Canonical invocation the compressor will receive (without binary prefix).
    /// Often equal to `from`, but some rules canonicalise (e.g.
    /// `eslint src/` → `lint src/`, `pnpm vitest` → `vitest run`).
    pub canonical: &'static str,
    /// Estimated tokens saved per wrap (rough output-compression heuristic).
    pub est_tokens_saved: u32,
}

/// Standard rule table. Checked in order; first match wins.
pub static RULES: &[RewriteRule] = &[
    // --- Git ---
    RewriteRule {
        from: "git status",
        canonical: "git status",
        est_tokens_saved: 160,
    },
    RewriteRule {
        from: "git diff",
        canonical: "git diff",
        est_tokens_saved: 160,
    },
    RewriteRule {
        from: "git log",
        canonical: "git log",
        est_tokens_saved: 160,
    },
    RewriteRule {
        from: "git add",
        canonical: "git add",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git commit",
        canonical: "git commit",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git push",
        canonical: "git push",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git pull",
        canonical: "git pull",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git branch",
        canonical: "git branch",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git fetch",
        canonical: "git fetch",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git stash",
        canonical: "git stash",
        est_tokens_saved: 60,
    },
    RewriteRule {
        from: "git show",
        canonical: "git show",
        est_tokens_saved: 60,
    },
    // --- GitHub CLI ---
    RewriteRule {
        from: "gh pr",
        canonical: "gh pr",
        est_tokens_saved: 120,
    },
    RewriteRule {
        from: "gh issue",
        canonical: "gh issue",
        est_tokens_saved: 120,
    },
    RewriteRule {
        from: "gh run",
        canonical: "gh run",
        est_tokens_saved: 120,
    },
    // --- Cargo ---
    RewriteRule {
        from: "cargo test",
        canonical: "cargo test",
        est_tokens_saved: 420,
    },
    RewriteRule {
        from: "cargo build",
        canonical: "cargo build",
        est_tokens_saved: 420,
    },
    RewriteRule {
        from: "cargo clippy",
        canonical: "cargo clippy",
        est_tokens_saved: 420,
    },
    RewriteRule {
        from: "cargo check",
        canonical: "cargo check",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "cargo run",
        canonical: "cargo run",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "cargo fmt",
        canonical: "cargo fmt",
        est_tokens_saved: 60,
    },
    // --- npm / yarn / pnpm ---
    RewriteRule {
        from: "npm test",
        canonical: "vitest run",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "npm run",
        canonical: "npm run",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "npm install",
        canonical: "npm install",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "yarn test",
        canonical: "vitest run",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "yarn add",
        canonical: "yarn add",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "pnpm test",
        canonical: "vitest run",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "pnpm vitest",
        canonical: "vitest run",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "vitest",
        canonical: "vitest run",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "pnpm tsc",
        canonical: "tsc",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "npx tsc",
        canonical: "tsc",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "tsc",
        canonical: "tsc",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "pnpm lint",
        canonical: "lint",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "npx eslint",
        canonical: "lint",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "eslint",
        canonical: "lint",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "npx prettier",
        canonical: "prettier",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "prettier",
        canonical: "prettier",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "pnpm playwright",
        canonical: "playwright",
        est_tokens_saved: 400,
    },
    RewriteRule {
        from: "npx playwright",
        canonical: "playwright",
        est_tokens_saved: 400,
    },
    RewriteRule {
        from: "playwright",
        canonical: "playwright",
        est_tokens_saved: 400,
    },
    RewriteRule {
        from: "npx prisma",
        canonical: "prisma",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "prisma",
        canonical: "prisma",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "pnpm list",
        canonical: "pnpm list",
        est_tokens_saved: 100,
    },
    RewriteRule {
        from: "pnpm ls",
        canonical: "pnpm ls",
        est_tokens_saved: 100,
    },
    RewriteRule {
        from: "pnpm outdated",
        canonical: "pnpm outdated",
        est_tokens_saved: 100,
    },
    // --- Python ---
    RewriteRule {
        from: "python -m pytest",
        canonical: "pytest",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "pytest",
        canonical: "pytest",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "pip install",
        canonical: "pip install",
        est_tokens_saved: 200,
    },
    // --- Go ---
    RewriteRule {
        from: "go test",
        canonical: "go test",
        est_tokens_saved: 380,
    },
    RewriteRule {
        from: "go build",
        canonical: "go build",
        est_tokens_saved: 300,
    },
    // --- Files / search ---
    RewriteRule {
        from: "cat",
        canonical: "read",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "ls",
        canonical: "ls",
        est_tokens_saved: 100,
    },
    RewriteRule {
        from: "rg",
        canonical: "grep",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "grep",
        canonical: "grep",
        est_tokens_saved: 200,
    },
    // --- Docker / kubernetes ---
    RewriteRule {
        from: "docker build",
        canonical: "docker build",
        est_tokens_saved: 500,
    },
    RewriteRule {
        from: "docker run",
        canonical: "docker run",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "docker ps",
        canonical: "docker ps",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "docker images",
        canonical: "docker images",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "docker logs",
        canonical: "docker logs",
        est_tokens_saved: 400,
    },
    RewriteRule {
        from: "kubectl describe",
        canonical: "kubectl describe",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "kubectl apply",
        canonical: "kubectl apply",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "kubectl get",
        canonical: "kubectl get",
        est_tokens_saved: 300,
    },
    RewriteRule {
        from: "kubectl logs",
        canonical: "kubectl logs",
        est_tokens_saved: 400,
    },
    // --- HTTP / build ---
    RewriteRule {
        from: "curl",
        canonical: "curl",
        est_tokens_saved: 200,
    },
    RewriteRule {
        from: "make",
        canonical: "make",
        est_tokens_saved: 400,
    },
];

/// Find a matching rule for `command`, ignoring a leading `cd … && ` prefix.
pub fn match_rule(command: &str) -> Option<&'static RewriteRule> {
    let cmd = strip_cd_prefix(command);
    RULES.iter().find(|r| matches_prefix(cmd, r.from))
}

/// Estimate tokens saved by wrapping `command` with any compressor.
/// Returns 0 if no rule matches.
pub fn tokens_saved(command: &str) -> u32 {
    match_rule(command).map(|r| r.est_tokens_saved).unwrap_or(0)
}

/// Wrap a command using the configured compressor. Returns `None` when
/// no rule matches, when the compressor is unavailable, or when the
/// command is already wrapped / contains a heredoc.
pub fn rewrite<C: Compressor + ?Sized>(command: &str, compressor: &C) -> Option<String> {
    if compressor.is_noop() {
        return None;
    }
    if command.contains("<<") {
        return None;
    }
    if compressor.is_already_wrapped(command) {
        return None;
    }
    if !compressor.is_available() {
        return None;
    }
    let rule = match_rule(command)?;
    // Preserve the leading `cd … && ` if present.
    let (prefix, tail) = split_cd_prefix(command);
    let wrapped = compressor.wrap(rule.canonical, &tail[rule.from.len()..]);
    Some(format!("{prefix}{wrapped}"))
}

fn matches_prefix(command: &str, prefix: &str) -> bool {
    if command == prefix {
        return true;
    }
    if command.starts_with(prefix) {
        matches!(
            command.as_bytes().get(prefix.len()),
            Some(b' ') | Some(b'\t') | Some(b'\n')
        )
    } else {
        false
    }
}

fn strip_cd_prefix(command: &str) -> &str {
    if command.starts_with("cd ") {
        if let Some(pos) = command.find(" && ") {
            return &command[pos + 4..];
        }
    }
    command
}

/// Split `cd … && rest` into (`"cd … && "`, `"rest"`); for non-cd commands
/// returns (`""`, `command`).
fn split_cd_prefix(command: &str) -> (&str, &str) {
    if command.starts_with("cd ") {
        if let Some(pos) = command.find(" && ") {
            return (&command[..pos + 4], &command[pos + 4..]);
        }
    }
    ("", command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compressors::NoneCompressor;

    #[test]
    fn match_rule_basic() {
        assert!(match_rule("git status").is_some());
        assert!(match_rule("git status --short").is_some());
        assert!(match_rule("python script.py").is_none());
    }

    #[test]
    fn match_rule_no_partial() {
        assert!(match_rule("lsof").is_none());
    }

    #[test]
    fn match_rule_with_cd_prefix() {
        let r = match_rule("cd /foo && cargo build").unwrap();
        assert_eq!(r.from, "cargo build");
    }

    #[test]
    fn tokens_saved_known() {
        assert_eq!(tokens_saved("cargo build"), 420);
        assert_eq!(tokens_saved("git status"), 160);
        assert_eq!(tokens_saved("cd /foo && make"), 400);
    }

    #[test]
    fn tokens_saved_unknown() {
        assert_eq!(tokens_saved("python script.py"), 0);
    }

    #[test]
    fn none_compressor_is_noop() {
        assert_eq!(rewrite("cargo build", &NoneCompressor), None);
    }
}
