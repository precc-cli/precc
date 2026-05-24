//! Builtin skills embedded into the precc binary at compile time.
//!
//! The skill TOML files live in `skills/builtin/` at the repo root.
//! `precc init` materializes them into `$XDG_DATA_HOME/precc/skills/builtin/`
//! (or `~/.local/share/precc/skills/builtin/`) on first run, then loads
//! them into `heuristics.db` via the normal `skills::load_builtin_skills`
//! path. This means a `cargo install`-only user gets a working install
//! with zero extra setup.
//!
//! Adding a new builtin skill is a 2-step process:
//! 1. Drop the TOML in `skills/builtin/`.
//! 2. Add an entry to `BUILTIN_SKILLS` below.

/// `(filename, contents)` pairs for every shipped builtin skill.
pub const BUILTIN_SKILLS: &[(&str, &str)] = &[
    (
        "cargo-wrong-dir.toml",
        include_str!("../../../skills/builtin/cargo-wrong-dir.toml"),
    ),
    (
        "git-wrong-dir.toml",
        include_str!("../../../skills/builtin/git-wrong-dir.toml"),
    ),
    (
        "go-wrong-dir.toml",
        include_str!("../../../skills/builtin/go-wrong-dir.toml"),
    ),
    (
        "make-wrong-dir.toml",
        include_str!("../../../skills/builtin/make-wrong-dir.toml"),
    ),
    (
        "npm-wrong-dir.toml",
        include_str!("../../../skills/builtin/npm-wrong-dir.toml"),
    ),
    (
        "python-wrong-dir.toml",
        include_str!("../../../skills/builtin/python-wrong-dir.toml"),
    ),
    (
        "gradle-wrong-dir.toml",
        include_str!("../../../skills/builtin/gradle-wrong-dir.toml"),
    ),
    (
        "maven-wrong-dir.toml",
        include_str!("../../../skills/builtin/maven-wrong-dir.toml"),
    ),
    (
        "bazel-wrong-dir.toml",
        include_str!("../../../skills/builtin/bazel-wrong-dir.toml"),
    ),
    (
        "docker-compose-wrong-dir.toml",
        include_str!("../../../skills/builtin/docker-compose-wrong-dir.toml"),
    ),
    (
        "terraform-wrong-dir.toml",
        include_str!("../../../skills/builtin/terraform-wrong-dir.toml"),
    ),
    (
        "just-wrong-dir.toml",
        include_str!("../../../skills/builtin/just-wrong-dir.toml"),
    ),
];

/// Write any missing embedded skills into `dest_dir`. Existing files
/// are left alone so user edits survive a re-run of `precc init`.
/// Returns the number of files written.
pub fn materialize(dest_dir: &std::path::Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dest_dir)?;
    let mut written = 0;
    for (name, contents) in BUILTIN_SKILLS {
        let path = dest_dir.join(name);
        if !path.exists() {
            std::fs::write(&path, contents)?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_parses() {
        for (name, contents) in BUILTIN_SKILLS {
            let parsed: Result<toml::Value, _> = toml::from_str(contents);
            assert!(parsed.is_ok(), "{name} failed to parse: {:?}", parsed.err());
        }
    }

    #[test]
    fn materialize_writes_files() {
        let tmp = tempfile::tempdir().unwrap();
        let written = materialize(tmp.path()).unwrap();
        assert_eq!(written, BUILTIN_SKILLS.len());
        // Re-running should write zero (no overwrite).
        let written2 = materialize(tmp.path()).unwrap();
        assert_eq!(written2, 0);
    }

    #[test]
    fn we_have_at_least_twelve_skills() {
        assert!(BUILTIN_SKILLS.len() >= 12, "expected ≥12 builtin skills");
    }
}
