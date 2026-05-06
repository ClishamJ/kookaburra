//! Git subprocess helpers for worktree mode.
//!
//! Per the spec (§4) we shell out to the user's `git` binary rather than
//! linking `git2`: it handles every edge case, respects user config, and
//! runs hooks correctly. These helpers are deliberately small wrappers so
//! the rest of the app stays out of the subprocess weeds.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use kookaburra_core::worktree::{Worktree, WorktreeConfig, WorktreeStatus};

/// Run `git -C <path> rev-parse --show-toplevel`. Returns the repo root on
/// success or `None` when `path` isn't inside a git repository (or git
/// isn't installed).
#[must_use]
pub fn resolve_repo_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Create a fresh git worktree from `config`, branching from `base_ref`
/// (defaults to `HEAD`).
///
/// The worktree directory lands under `<base_dir>/<repo>-<short_id>`.
/// `branch_template` is the format string used when `config.branch` is
/// `None`. Two `{}` placeholders are expected: the slugified
/// `config.label_hint` and a short id, in that order
/// (e.g. `kookaburra/{}-{}`).
pub fn create_worktree(
    config: &WorktreeConfig,
    base_dir: &Path,
    branch_template: &str,
) -> Result<Worktree, String> {
    std::fs::create_dir_all(base_dir)
        .map_err(|e| format!("create_dir_all {}: {}", base_dir.display(), e))?;

    let short_id = short_random_id();
    let repo_name = config
        .source_repo
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");
    let wt_dir = base_dir.join(format!("{repo_name}-{short_id}"));

    let slug = workspace_slug(&config.label_hint);
    let branch = config.branch.clone().unwrap_or_else(|| {
        branch_template
            .replacen("{}", &slug, 1)
            .replacen("{}", &short_id, 1)
    });
    let base_ref = config
        .base_ref
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());

    let output = Command::new("git")
        .arg("-C")
        .arg(&config.source_repo)
        .args(["worktree", "add"])
        .arg(&wt_dir)
        .args(["-b", &branch])
        .arg(&base_ref)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(Worktree {
        source_repo: config.source_repo.clone(),
        worktree_path: wt_dir,
        branch,
        base_ref,
        status: WorktreeStatus::default(),
    })
}

/// Default base directory for kookaburra-managed worktrees:
/// `$HOME/.kookaburra/worktrees`. Falls back to `./.kookaburra/worktrees`
/// in the (unlikely) absence of `$HOME`.
#[must_use]
pub fn default_worktrees_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".kookaburra").join("worktrees")
    } else {
        PathBuf::from(".kookaburra").join("worktrees")
    }
}

/// Lowercase, hyphen-separated workspace slug used inside auto-generated
/// branch names. Anything that isn't `[a-z0-9]` collapses to `-`; runs of
/// `-` get squashed; leading/trailing `-` are trimmed; empty results fall
/// back to `"workspace"`.
#[must_use]
pub fn workspace_slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut last_was_hyphen = true; // suppresses leading hyphen
    for ch in label.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            out.push('-');
            last_was_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

/// 4-character lowercase-hex id derived from process-wide nanos + a
/// monotonic counter. Not cryptographically random — just enough to keep
/// branch names from colliding when a user fills the grid in a few
/// seconds. `git worktree add -b` will refuse a duplicate branch, so
/// occasional collisions are loud failures, not silent corruption.
fn short_random_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = nanos
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(seq.wrapping_mul(0x85EB_CA77));
    format!("{:04x}", mixed & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_slug_lowercases_and_separates() {
        assert_eq!(workspace_slug("Auth Refactor"), "auth-refactor");
    }

    #[test]
    fn workspace_slug_collapses_repeated_separators() {
        assert_eq!(workspace_slug("Try   This!! 2"), "try-this-2");
    }

    #[test]
    fn workspace_slug_trims_edge_separators() {
        assert_eq!(workspace_slug("--scratch--"), "scratch");
    }

    #[test]
    fn workspace_slug_empty_label_falls_back() {
        assert_eq!(workspace_slug("---"), "workspace");
        assert_eq!(workspace_slug(""), "workspace");
    }

    #[test]
    fn short_random_id_is_four_hex_chars() {
        for _ in 0..16 {
            let id = short_random_id();
            assert_eq!(id.len(), 4, "expected 4 chars, got {id:?}");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "non-hex chars in {id:?}"
            );
        }
    }

    #[test]
    fn short_random_id_changes_across_calls() {
        // The counter portion guarantees uniqueness within a process even
        // when nanos collide on the same nanosecond.
        let a = short_random_id();
        let b = short_random_id();
        assert_ne!(a, b);
    }
}
