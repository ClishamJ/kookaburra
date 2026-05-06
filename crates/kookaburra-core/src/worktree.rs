//! Git worktree metadata attached to tiles.
//!
//! Phase 6 will implement the actual git subprocess calls; for now this
//! module only defines the data shapes other code refers to.

use std::path::PathBuf;

/// Persistent metadata for a worktree-mode tile.
///
/// Mirrors the spec §4 model: a tile knows its source repo, the worktree
/// path, the branch it lives on, and the base ref it was branched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Path to the source repository (the "real" repo the user started in).
    pub source_repo: PathBuf,
    /// Filesystem path to this worktree, typically under
    /// `~/.kookaburra/worktrees/<repo>-<short-id>`.
    pub worktree_path: PathBuf,
    /// Branch name, e.g. `kookaburra/auth-refactor-3f9a`.
    pub branch: String,
    /// Base ref the branch was created from (e.g. `HEAD`, `main`).
    pub base_ref: String,
    /// Cached `git status --porcelain` snapshot, refreshed periodically.
    pub status: WorktreeStatus,
}

/// Lightweight status snapshot updated on a poll loop in Phase 6.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

/// What we ask `apply_action` to create when the user opts into a worktree
/// in the new-tile dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeConfig {
    /// Source repo to branch from. Detected from CWD via
    /// `git -C <cwd> rev-parse --show-toplevel` at dialog time.
    pub source_repo: PathBuf,
    /// Branch name to create. None means "auto-generate from template".
    pub branch: Option<String>,
    /// Base ref. None means HEAD.
    pub base_ref: Option<String>,
    /// Hint used when `branch` is `None` to assemble the auto-generated
    /// branch name. Typically the workspace label so users can recognize
    /// branches like `kookaburra/auth-refactor-3f9a`. May be empty; the
    /// implementation falls back to a default in that case.
    pub label_hint: String,
}
