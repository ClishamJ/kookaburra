//! User actions. `apply_action` is the only mutation site for `AppState`.
//!
//! The rule from the spec: UI code only ever sees `&AppState` and pushes
//! `Action`s into a `Vec`. After the UI pass, the action vec is drained
//! and each action passes through `apply_action`. This keeps mutations in
//! exactly one place and makes testing trivial.
//!
//! For the rough-draft pass, PTY side effects (spawn / close) are modeled
//! with a callback trait — `PtySideEffects` — so `kookaburra-core` does
//! not need to depend on `kookaburra-pty`. The `app` crate wires the real
//! PTY manager as the implementer.

use std::path::{Path, PathBuf};

use crate::ids::{PtyId, TileId, WorkspaceId};
use crate::layout::Layout;
use crate::state::{AppState, Tile, Workspace};
use crate::worktree::{Worktree, WorktreeConfig};

/// Search scope for the search dialogs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SearchScope {
    FocusedTile,
    ActiveWorkspace,
}

/// Every user or system interaction that mutates `AppState` goes through
/// one of these. Keep variants data-only; keep logic in `apply_action`.
#[derive(Clone, Debug)]
pub enum Action {
    // Workspaces
    SwitchWorkspace(WorkspaceId),
    CreateWorkspace,
    DeleteWorkspace(WorkspaceId),
    RenameWorkspace {
        id: WorkspaceId,
        new_label: String,
    },
    ReorderWorkspaces {
        from: usize,
        to: usize,
    },

    // Tiles
    /// Promote an empty slot to a live tile. `tile_id` must name an
    /// existing slot; if the slot is already live the action is a no-op.
    /// The UI should emit this when the user clicks (or focus+Enter) an
    /// empty placeholder, and during bootstrap to fill the starter tile.
    SpawnInTile {
        tile_id: TileId,
        worktree: Option<WorktreeConfig>,
    },
    CloseTile(TileId),
    FocusTile(TileId),
    MoveTile {
        tile_id: TileId,
        target_workspace: WorkspaceId,
    },
    /// Spin a tile out into its own new workspace, then focus the new
    /// workspace. Emitted when the user drags a tile onto the strip
    /// outside any existing card (spec §3 "drag tile onto empty strip").
    MoveTileToNewWorkspace {
        tile_id: TileId,
    },
    SetPrimaryTile {
        workspace: WorkspaceId,
        tile: TileId,
    },
    ToggleFollowMode(TileId),
    ForkTile(TileId),

    // Layout
    SetLayout {
        workspace: WorkspaceId,
        layout: Layout,
    },
    ToggleZenMode,

    // Misc
    OpenSearch {
        scope: SearchScope,
    },
    ClearTileDirty(TileId),
    /// Set or clear the per-workspace theme override. `theme_name = None`
    /// reverts the workspace to the config default. The name is resolved
    /// against `Theme::builtin`; unresolvable names are stored as-is and
    /// `effective_theme` falls through to the default.
    SetWorkspaceTheme {
        workspace: WorkspaceId,
        theme_name: Option<String>,
    },
    /// Set or clear the per-workspace default cwd. New tiles spawned in
    /// this workspace start in this directory unless a worktree path
    /// overrides it. The app layer is expected to refresh
    /// `Workspace::project_repo` after this lands by calling
    /// `PtySideEffects::resolve_repo_root`.
    SetWorkspaceCwd {
        id: WorkspaceId,
        cwd: Option<PathBuf>,
    },
    /// Toggle workspace-wide worktree mode. While `on`, every
    /// `SpawnInTile` in this workspace auto-creates a fresh git worktree
    /// of the workspace's `project_repo` on a new branch.
    SetWorktreeMode {
        id: WorkspaceId,
        on: bool,
    },
}

/// Result of a [`PtySideEffects::spawn`] call.
///
/// `worktree` is `Some` only when a worktree was actually created (i.e. the
/// caller supplied a `WorktreeConfig` *and* `git worktree add` succeeded).
/// `apply_action` stores it on the freshly-promoted [`Tile`] so the UI can
/// surface the branch name and so the close-tile path knows the worktree
/// exists.
#[derive(Clone, Debug)]
pub struct SpawnResult {
    pub pty_id: PtyId,
    pub worktree: Option<Worktree>,
}

/// Side effects the action handler may need to ask the PTY layer to
/// perform. Implemented by `kookaburra-pty::PtyManager` in the app crate.
///
/// Keeping this a trait lets the core crate stay free of tokio /
/// alacritty_terminal and lets us unit-test `apply_action` against a
/// stub.
pub trait PtySideEffects {
    /// Spawn a new PTY bound to `tile_id`. The `tile_id` is decided by
    /// `apply_action` before this call so the PTY's event listener can
    /// tag events with the same id the `Tile` will carry.
    ///
    /// `cwd` is the workspace's default cwd (overridden by the worktree
    /// path when one is created). `worktree` is `Some` only when the
    /// caller wants a fresh git worktree created — the implementation is
    /// expected to shell out to `git worktree add`, then spawn the PTY
    /// with cwd set to the worktree path.
    fn spawn(
        &mut self,
        tile_id: TileId,
        cwd: Option<&Path>,
        worktree: Option<&WorktreeConfig>,
    ) -> SpawnResult;
    /// Close a PTY. Best-effort; failures should be logged, not returned.
    fn close(&mut self, pty: PtyId);
    /// Resolve the git repo root that `cwd` lives inside, if any. Returns
    /// `None` when `cwd` isn't inside a repo. Used by `SetWorkspaceCwd` to
    /// refresh `Workspace::project_repo`.
    fn resolve_repo_root(&self, cwd: &Path) -> Option<PathBuf>;
}

/// Locate a tile across all workspaces by its id. Returns
/// `(workspace_index, tile_index)` if found.
fn find_tile_loc(state: &AppState, tile_id: TileId) -> Option<(usize, usize)> {
    for (wi, ws) in state.workspaces.iter().enumerate() {
        if let Some(ti) = ws.tiles.iter().position(|t| t.id == tile_id) {
            return Some((wi, ti));
        }
    }
    None
}

/// The only mutation site for `AppState`. Keep this a pure function over
/// `&mut AppState` + `&mut dyn PtySideEffects`: no other place in the
/// codebase should be reaching into `AppState` fields for writes.
pub fn apply_action(state: &mut AppState, pty: &mut dyn PtySideEffects, action: Action) {
    state.request_redraw();
    match action {
        Action::SwitchWorkspace(id) => {
            if state.workspace(id).is_some() {
                state.active_workspace = id;
                // When switching, honor primary tile if set. Otherwise
                // prefer the first *live* tile — never silently drop focus
                // onto an empty slot when a usable terminal exists, since
                // empty slots reinterpret Enter as "spawn a new window".
                // Fall back to tiles[0] only for all-empty workspaces, so
                // users who just created a workspace can Enter to spawn.
                let ws = state.active_workspace();
                let primary = ws.primary_tile;
                let first_live = ws.tiles.iter().find(|t| t.is_live()).map(|t| t.id);
                let first = ws.tiles.first().map(|t| t.id);
                state.focused_tile = primary.or(first_live).or(first);
            }
        }
        Action::CreateWorkspace => {
            let idx = state.workspaces.len() + 1;
            let ws = Workspace::new(format!("workspace {idx}"));
            let id = ws.id;
            state.workspaces.push(ws);
            state.active_workspace = id;
            state.focused_tile = None;
        }
        Action::DeleteWorkspace(id) => {
            // Close every live PTY that belonged to the workspace. Empty
            // slots have no PTY to kill.
            if let Some(ws) = state.workspace(id) {
                let pty_ids: Vec<PtyId> = ws.tiles.iter().filter_map(|t| t.pty_id).collect();
                for p in pty_ids {
                    pty.close(p);
                }
            }
            state.workspaces.retain(|w| w.id != id);
            if state.workspaces.is_empty() {
                // Always keep at least one workspace around.
                let ws = Workspace::new("workspace 1");
                state.active_workspace = ws.id;
                state.workspaces.push(ws);
                state.focused_tile = None;
            } else if state.active_workspace == id {
                let next_ws = &state.workspaces[0];
                // Mirror SwitchWorkspace: prefer a live tile over an empty
                // slot at index 0, so the first keystroke after deletion
                // doesn't land on an empty-slot Enter-promotion path.
                let first_live = next_ws.tiles.iter().find(|t| t.is_live()).map(|t| t.id);
                let first = next_ws.tiles.first().map(|t| t.id);
                state.active_workspace = next_ws.id;
                state.focused_tile = first_live.or(first);
            }
        }
        Action::RenameWorkspace { id, new_label } => {
            if let Some(ws) = state.workspace_mut(id) {
                ws.label = new_label;
            }
        }
        Action::ReorderWorkspaces { from, to } => {
            if from < state.workspaces.len() && to < state.workspaces.len() && from != to {
                let item = state.workspaces.remove(from);
                state.workspaces.insert(to, item);
            }
        }
        Action::SpawnInTile { tile_id, worktree } => {
            // Promote an empty slot to live. No-op if the tile doesn't
            // resolve or the slot is already live.
            let is_empty_slot = state.tile(tile_id).map(|t| !t.is_live()).unwrap_or(false);
            if !is_empty_slot {
                return;
            }
            // Resolve the owning workspace once: we need the cwd, the
            // worktree-mode flag, the project_repo, and the label.
            let owning = state
                .workspaces
                .iter()
                .find(|w| w.tiles.iter().any(|t| t.id == tile_id));
            let cwd: Option<PathBuf> = owning.and_then(|w| w.default_cwd.clone());
            // Auto-generate a WorktreeConfig from workspace state when:
            // - no explicit one was supplied,
            // - the workspace is in worktree mode, and
            // - it has a detected project repo to branch from.
            // Otherwise pass the explicit config (or None) through.
            let effective_worktree = match worktree {
                Some(cfg) => Some(cfg),
                None => owning.and_then(|w| {
                    if w.worktree_mode {
                        w.project_repo.as_ref().map(|repo| WorktreeConfig {
                            source_repo: repo.clone(),
                            branch: None,
                            base_ref: None,
                            label_hint: w.label.clone(),
                        })
                    } else {
                        None
                    }
                }),
            };
            let result = pty.spawn(tile_id, cwd.as_deref(), effective_worktree.as_ref());
            if let Some(tile) = state.tile_mut(tile_id) {
                tile.promote(result.pty_id);
                tile.worktree = result.worktree;
            }
            state.focused_tile = Some(tile_id);
        }
        Action::CloseTile(tile_id) => {
            // Demote a live slot back to empty. The slot itself stays —
            // the user can click / press Enter to instantiate a fresh
            // terminal in its place. No-op on slots that are already empty.
            if let Some(tile) = state.tile_mut(tile_id) {
                if let Some(pty_id) = tile.demote() {
                    pty.close(pty_id);
                }
            }
        }
        Action::FocusTile(tile_id) => {
            if state.tile(tile_id).is_some() {
                state.focused_tile = Some(tile_id);
                if let Some(tile) = state.tile_mut(tile_id) {
                    tile.has_new_output = false;
                }
            }
        }
        Action::MoveTile {
            tile_id,
            target_workspace,
        } => {
            // Find the source slot.
            let Some((src_wi, src_ti)) = find_tile_loc(state, tile_id) else {
                return;
            };
            // Moving an empty slot has no meaning.
            if !state.workspaces[src_wi].tiles[src_ti].is_live() {
                return;
            }
            // Resolve destination workspace.
            let Some(dst_wi) = state
                .workspaces
                .iter()
                .position(|w| w.id == target_workspace)
            else {
                return;
            };
            if src_wi == dst_wi {
                return;
            }
            // Find first empty slot in destination.
            let Some(dst_ti) = state.workspaces[dst_wi]
                .tiles
                .iter()
                .position(|t| !t.is_live())
            else {
                // No room in the destination; spec treats this as a no-op.
                return;
            };
            // Take the source tile out, leave a fresh empty in its place.
            let moved =
                std::mem::replace(&mut state.workspaces[src_wi].tiles[src_ti], Tile::empty());
            // Replace the destination's empty slot with the moved tile.
            // The moved Tile keeps its TileId so the PTY's event listener
            // (which tagged events with that id at spawn time) continues
            // to land on the right slot.
            state.workspaces[dst_wi].tiles[dst_ti] = moved;
            // If the user was focused on the tile we just moved, clear
            // focus so the next keystroke in the source workspace doesn't
            // traverse `state.tile()` (which resolves globally) and write
            // bytes into a terminal across the strip that the user can't
            // see. `active_tile()` will fall back to the source workspace's
            // first live tile — or to an empty slot if none remain.
            if state.focused_tile == Some(tile_id) {
                state.focused_tile = None;
            }
        }
        Action::MoveTileToNewWorkspace { tile_id } => {
            let Some((src_wi, src_ti)) = find_tile_loc(state, tile_id) else {
                return;
            };
            if !state.workspaces[src_wi].tiles[src_ti].is_live() {
                return;
            }
            let moved =
                std::mem::replace(&mut state.workspaces[src_wi].tiles[src_ti], Tile::empty());
            let idx = state.workspaces.len() + 1;
            let mut ws = Workspace::new(format!("workspace {idx}"));
            let new_ws_id = ws.id;
            // Slot 0 of the new workspace receives the moved tile.
            ws.tiles[0] = moved;
            state.workspaces.push(ws);
            state.active_workspace = new_ws_id;
            state.focused_tile = Some(tile_id);
        }
        Action::SetPrimaryTile { workspace, tile } => {
            if let Some(ws) = state.workspace_mut(workspace) {
                if ws.tile(tile).is_some() {
                    ws.primary_tile = Some(tile);
                }
            }
        }
        Action::ToggleFollowMode(tile_id) => {
            if let Some(tile) = state.tile_mut(tile_id) {
                tile.follow_mode = !tile.follow_mode;
            }
        }
        Action::ForkTile(_tile_id) => {
            // Phase 6. No-op for the rough draft.
        }
        Action::SetLayout { workspace, layout } => {
            if let Some(ws) = state.workspace_mut(workspace) {
                ws.layout = layout;
            }
        }
        Action::ToggleZenMode => {
            state.zen_mode = !state.zen_mode;
        }
        Action::OpenSearch { .. } => {
            // Phase 4. No-op for the rough draft.
        }
        Action::ClearTileDirty(tile_id) => {
            if let Some(tile) = state.tile_mut(tile_id) {
                tile.has_new_output = false;
            }
        }
        Action::SetWorkspaceTheme {
            workspace,
            theme_name,
        } => {
            if let Some(ws) = state.workspace_mut(workspace) {
                ws.theme_override = theme_name;
            }
        }
        Action::SetWorkspaceCwd { id, cwd } => {
            // Refresh the cached project_repo via the side-effects trait
            // so the UI can decide whether to enable the worktree-mode
            // toggle. Done before the assignment so a None cwd clears
            // both fields atomically.
            let new_repo = cwd.as_deref().and_then(|p| pty.resolve_repo_root(p));
            if let Some(ws) = state.workspace_mut(id) {
                ws.default_cwd = cwd;
                ws.project_repo = new_repo;
                if ws.project_repo.is_none() {
                    // Worktree mode requires a repo. Clear the flag so the
                    // UI doesn't render a "mode on, but greyed out" oddity.
                    ws.worktree_mode = false;
                }
            }
        }
        Action::SetWorktreeMode { id, on } => {
            if let Some(ws) = state.workspace_mut(id) {
                // Refuse to enable when the workspace has no detected
                // repo. The UI is expected to disable the toggle in that
                // case anyway; this is a guardrail against stale state.
                if on && ws.project_repo.is_none() {
                    return;
                }
                ws.worktree_mode = on;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// Counts pty side effects so we can assert apply_action wired them up.
    #[derive(Default)]
    struct StubPty {
        spawns: u32,
        closes: u32,
        last_spawn_tile: Option<TileId>,
        last_spawn_cwd: Option<PathBuf>,
        last_spawn_worktree_config: Option<WorktreeConfig>,
        /// When `Some`, the next `spawn` returns this `Worktree` in its
        /// `SpawnResult`. Drained on use so tests can seed one return.
        next_spawn_worktree: Option<Worktree>,
        /// When `Some`, `resolve_repo_root` always returns this. Otherwise
        /// returns `None`.
        repo_root_for_next_lookup: Option<PathBuf>,
    }

    impl PtySideEffects for StubPty {
        fn spawn(
            &mut self,
            tile_id: TileId,
            cwd: Option<&Path>,
            worktree: Option<&WorktreeConfig>,
        ) -> SpawnResult {
            self.spawns += 1;
            self.last_spawn_tile = Some(tile_id);
            self.last_spawn_cwd = cwd.map(PathBuf::from);
            self.last_spawn_worktree_config = worktree.cloned();
            SpawnResult {
                pty_id: PtyId::new(),
                worktree: self.next_spawn_worktree.take(),
            }
        }
        fn close(&mut self, _pty: PtyId) {
            self.closes += 1;
        }
        fn resolve_repo_root(&self, _cwd: &Path) -> Option<PathBuf> {
            self.repo_root_for_next_lookup.clone()
        }
    }

    #[test]
    fn spawn_in_tile_promotes_empty_slot_and_focuses() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert_eq!(pty.spawns, 1);
        assert!(state.tile(tile_id).unwrap().is_live());
        assert_eq!(state.focused_tile, Some(tile_id));
    }

    #[test]
    fn spawn_in_tile_is_noop_on_already_live_slot() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert_eq!(pty.spawns, 1, "second SpawnInTile on live slot is no-op");
    }

    #[test]
    fn spawn_in_tile_with_unknown_id_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id: TileId::new(),
                worktree: None,
            },
        );
        assert_eq!(pty.spawns, 0);
        assert_eq!(state.focused_tile, None);
    }

    #[test]
    fn close_tile_demotes_slot_to_empty_and_closes_pty() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert!(state.tile(tile_id).unwrap().is_live());

        apply_action(&mut state, &mut pty, Action::CloseTile(tile_id));
        assert_eq!(pty.closes, 1);
        // Slot stays, just the PTY is gone.
        assert!(state.tile(tile_id).is_some());
        assert!(!state.tile(tile_id).unwrap().is_live());
    }

    #[test]
    fn close_tile_on_empty_slot_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let empty_id = state.active_workspace().tiles[0].id;
        assert!(!state.tile(empty_id).unwrap().is_live());
        apply_action(&mut state, &mut pty, Action::CloseTile(empty_id));
        assert_eq!(pty.closes, 0, "no PTY to close for empty slot");
        assert!(state.tile(empty_id).is_some(), "slot still present");
    }

    #[test]
    fn spawn_in_tile_passes_tile_id_to_pty() {
        // Regression: the PTY's event proxy must be tagged with the same
        // TileId that will carry the tile in AppState. If apply_action
        // minted a fresh id after spawning, events would never find their
        // tile.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert_eq!(pty.last_spawn_tile, Some(tile_id));
    }

    #[test]
    fn toggle_zen_mode_flips_flag() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        assert!(!state.zen_mode);
        apply_action(&mut state, &mut pty, Action::ToggleZenMode);
        assert!(state.zen_mode);
        apply_action(&mut state, &mut pty, Action::ToggleZenMode);
        assert!(!state.zen_mode);
    }

    #[test]
    fn create_workspace_appends_and_switches() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        assert_eq!(state.workspaces.len(), 1);
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(state.active_workspace, state.workspaces[1].id);
    }

    #[test]
    fn create_workspace_yields_workspace_of_empty_slots() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let new_ws = state.active_workspace();
        assert_eq!(new_ws.tiles.len(), new_ws.layout.cell_count());
        assert!(new_ws.tiles.iter().all(|t| !t.is_live()));
        assert_eq!(pty.spawns, 0, "creating a workspace shouldn't spawn PTYs");
    }

    #[test]
    fn delete_last_workspace_reseeds_with_empty_slots() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let only = state.active_workspace;
        apply_action(&mut state, &mut pty, Action::DeleteWorkspace(only));
        assert_eq!(state.workspaces.len(), 1);
        let ws = state.active_workspace();
        assert_eq!(ws.tiles.len(), ws.layout.cell_count());
        assert!(ws.tiles.iter().all(|t| !t.is_live()));
    }

    #[test]
    fn move_tile_to_new_workspace_creates_and_switches() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        let before = state.workspaces.len();
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTileToNewWorkspace { tile_id },
        );
        assert_eq!(state.workspaces.len(), before + 1);
        let new_ws = state.active_workspace();
        assert_eq!(new_ws.tiles.len(), new_ws.layout.cell_count());
        assert!(new_ws.tiles[0].is_live());
        assert_eq!(new_ws.tiles[0].id, tile_id, "moved tile keeps its id");
        assert!(
            new_ws.tiles[1..].iter().all(|t| !t.is_live()),
            "remaining slots are empty"
        );
        assert_ne!(state.active_workspace, source);
        assert_eq!(state.focused_tile, Some(tile_id));
        // Source keeps its slot count; the slot we moved from is now empty.
        let src = state.workspace(source).unwrap();
        assert_eq!(src.tiles.len(), src.layout.cell_count());
        assert!(src.tiles.iter().all(|t| !t.is_live()));
    }

    #[test]
    fn move_tile_relocates_it_to_target_workspace() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let target = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTile {
                tile_id,
                target_workspace: target,
            },
        );
        // Source: all slots still there, all empty.
        let src = state.workspace(source).unwrap();
        assert_eq!(src.tiles.len(), src.layout.cell_count());
        assert!(src.tiles.iter().all(|t| !t.is_live()));
        // Target: first empty slot now live with the moved tile; rest empty.
        let tgt = state.workspace(target).unwrap();
        assert_eq!(tgt.tiles.len(), tgt.layout.cell_count());
        assert!(tgt.tiles[0].is_live());
        assert_eq!(tgt.tiles[0].id, tile_id);
        assert!(tgt.tiles[1..].iter().all(|t| !t.is_live()));
    }

    #[test]
    fn move_tile_into_full_destination_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        let source_tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id: source_tile_id,
                worktree: None,
            },
        );
        // Fill up a second workspace so it has no empty slots.
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let target = state.active_workspace;
        let target_slot_ids: Vec<TileId> = state
            .workspace(target)
            .unwrap()
            .tiles
            .iter()
            .map(|t| t.id)
            .collect();
        for id in &target_slot_ids {
            apply_action(
                &mut state,
                &mut pty,
                Action::SpawnInTile {
                    tile_id: *id,
                    worktree: None,
                },
            );
        }

        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTile {
                tile_id: source_tile_id,
                target_workspace: target,
            },
        );
        // Source still has the live tile; target unchanged.
        let src = state.workspace(source).unwrap();
        assert!(
            src.tiles
                .iter()
                .any(|t| t.id == source_tile_id && t.is_live()),
            "source tile stayed put — target had no empty slot"
        );
        assert_eq!(
            state.workspace(target).unwrap().tiles.len(),
            target_slot_ids.len()
        );
    }

    #[test]
    fn move_tile_within_same_workspace_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        let before_tiles = state.workspace(ws_id).unwrap().tiles.len();
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTile {
                tile_id,
                target_workspace: ws_id,
            },
        );
        let after = state.workspace(ws_id).unwrap();
        assert_eq!(after.tiles.len(), before_tiles);
        assert!(after.tiles[0].is_live());
        assert_eq!(after.tiles[0].id, tile_id);
    }

    #[test]
    fn switch_workspace_focuses_first_live_tile_when_slot_zero_is_empty() {
        // Regression for the "Enter spawns a new window" bug: when switching
        // into a workspace that has a live tile at a non-zero index, focus
        // must land on the live tile, not on the empty slot at index 0.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let origin_ws = state.active_workspace;
        // Make a second workspace and spawn a tile at slot index 2 (slots 0
        // and 1 stay empty).
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let target_ws = state.active_workspace;
        let live_tile_id = state.active_workspace().tiles[2].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id: live_tile_id,
                worktree: None,
            },
        );
        // Bounce back to the origin then switch into the target. Switching
        // must focus the live tile, not tiles[0] (empty).
        apply_action(&mut state, &mut pty, Action::SwitchWorkspace(origin_ws));
        apply_action(&mut state, &mut pty, Action::SwitchWorkspace(target_ws));
        assert_eq!(
            state.focused_tile,
            Some(live_tile_id),
            "SwitchWorkspace must prefer a live tile to an empty slot"
        );
    }

    #[test]
    fn delete_workspace_focus_lands_on_live_tile_in_fallback_workspace() {
        // Regression: after DeleteWorkspace re-homes focus to workspaces[0],
        // it must pick the first *live* tile there, not tiles[0] (which may
        // be empty).
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        // workspaces[0] has a live tile in slot 1, not slot 0.
        let live_tile_id = state.active_workspace().tiles[1].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id: live_tile_id,
                worktree: None,
            },
        );
        // Create a second workspace (so deleting it is meaningful) and
        // switch to it so we're deleting the *active* workspace.
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let victim = state.active_workspace;
        apply_action(&mut state, &mut pty, Action::DeleteWorkspace(victim));
        assert_eq!(
            state.focused_tile,
            Some(live_tile_id),
            "DeleteWorkspace fallback must prefer a live tile in workspaces[0]"
        );
    }

    #[test]
    fn move_tile_clears_focus_when_moved_tile_was_focused() {
        // Regression: MoveTile moves a live tile into another workspace but
        // historically left `focused_tile` pointing at the moved tile's id.
        // Because `state.tile()` resolves globally, subsequent keystrokes in
        // the *source* workspace would still hit the moved tile across the
        // strip. After the fix, focus is cleared when the moved tile was
        // the focused one; the source workspace has no focus until the
        // user picks a new tile.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source_ws = state.active_workspace;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        // Create a destination workspace then switch back to the source —
        // mirroring the real drag-to-card flow where the user is still
        // looking at the source workspace when the move lands.
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let target_ws = state.active_workspace;
        apply_action(&mut state, &mut pty, Action::SwitchWorkspace(source_ws));
        assert_eq!(state.focused_tile, Some(tile_id));
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTile {
                tile_id,
                target_workspace: target_ws,
            },
        );
        assert_ne!(
            state.focused_tile,
            Some(tile_id),
            "MoveTile must not leave focus pointing at a tile that lives \
             in a different workspace"
        );
    }

    #[test]
    fn set_workspace_theme_sets_and_clears_override() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceTheme {
                workspace: ws_id,
                theme_name: Some("Tokyo Night".into()),
            },
        );
        assert_eq!(
            state.workspace(ws_id).unwrap().theme_override.as_deref(),
            Some("Tokyo Night")
        );
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceTheme {
                workspace: ws_id,
                theme_name: None,
            },
        );
        assert!(state.workspace(ws_id).unwrap().theme_override.is_none());
    }

    #[test]
    fn set_workspace_cwd_assigns_path() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: ws_id,
                cwd: Some(std::path::PathBuf::from("/tmp/projects/foo")),
            },
        );
        assert_eq!(
            state.workspace(ws_id).unwrap().default_cwd.as_deref(),
            Some(std::path::Path::new("/tmp/projects/foo"))
        );
    }

    #[test]
    fn set_workspace_cwd_clears_with_none() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        state.workspace_mut(ws_id).unwrap().default_cwd =
            Some(std::path::PathBuf::from("/tmp/will-be-cleared"));
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: ws_id,
                cwd: None,
            },
        );
        assert!(state.workspace(ws_id).unwrap().default_cwd.is_none());
    }

    #[test]
    fn set_workspace_cwd_unknown_id_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: WorkspaceId::new(),
                cwd: Some(std::path::PathBuf::from("/tmp/anywhere")),
            },
        );
        assert!(state.active_workspace().default_cwd.is_none());
    }

    #[test]
    fn spawn_in_tile_passes_workspace_default_cwd_to_pty() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        let tile_id = state.active_workspace().tiles[0].id;
        let project_path = std::path::PathBuf::from("/tmp/projects/foo");
        state.workspace_mut(ws_id).unwrap().default_cwd = Some(project_path.clone());
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert_eq!(pty.last_spawn_cwd.as_deref(), Some(project_path.as_path()));
    }

    #[test]
    fn spawn_in_tile_passes_none_cwd_when_workspace_has_no_default() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert!(pty.last_spawn_cwd.is_none());
    }

    #[test]
    fn spawn_in_tile_auto_builds_worktree_config_when_workspace_in_worktree_mode() {
        // 6-worktrees-per-tab: the user toggled `worktree_mode` on the
        // workspace; every SpawnInTile (no explicit config) should fan out
        // a fresh WorktreeConfig to the PTY layer derived from the
        // workspace's project_repo + label.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        {
            let ws = state.workspace_mut(ws_id).unwrap();
            ws.label = "Auth Refactor".into();
            ws.project_repo = Some(PathBuf::from("/tmp/repo"));
            ws.worktree_mode = true;
        }
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        let cfg = pty
            .last_spawn_worktree_config
            .as_ref()
            .expect("worktree config must be passed through to spawn");
        assert_eq!(cfg.source_repo, PathBuf::from("/tmp/repo"));
        assert!(cfg.branch.is_none(), "branch is auto-generated downstream");
        assert!(
            cfg.base_ref.is_none(),
            "base_ref defaults to HEAD downstream"
        );
        assert_eq!(cfg.label_hint, "Auth Refactor");
    }

    #[test]
    fn spawn_in_tile_skips_worktree_config_when_worktree_mode_off() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        // Repo is set but worktree mode is not: a plain shell is wanted.
        state.workspace_mut(ws_id).unwrap().project_repo = Some(PathBuf::from("/tmp/repo"));
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert!(pty.last_spawn_worktree_config.is_none());
    }

    #[test]
    fn spawn_in_tile_skips_worktree_config_when_no_project_repo() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        // worktree_mode is true but project_repo is None — apply_action
        // refuses to invent a repo, so no config goes to spawn.
        state.workspace_mut(ws_id).unwrap().worktree_mode = true;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert!(pty.last_spawn_worktree_config.is_none());
    }

    #[test]
    fn spawn_in_tile_passes_explicit_worktree_config_through_unchanged() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        // Workspace has a different repo configured: an explicit config
        // (e.g. via Fork) wins.
        state.workspace_mut(ws_id).unwrap().project_repo =
            Some(PathBuf::from("/tmp/workspace-repo"));
        state.workspace_mut(ws_id).unwrap().worktree_mode = true;

        let tile_id = state.active_workspace().tiles[0].id;
        let explicit = crate::worktree::WorktreeConfig {
            source_repo: PathBuf::from("/tmp/explicit-repo"),
            branch: Some("feature/explicit".into()),
            base_ref: Some("main".into()),
            label_hint: "explicit".into(),
        };
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: Some(explicit.clone()),
            },
        );
        assert_eq!(pty.last_spawn_worktree_config.as_ref(), Some(&explicit));
    }

    #[test]
    fn spawn_in_tile_passes_worktree_path_as_cwd_when_pty_returns_a_worktree() {
        // Sanity: when the PTY layer reports back a created worktree,
        // apply_action stores it on the tile but leaves the cwd plumbing
        // to the PTY layer (which sets cwd = worktree_path before spawn).
        // This test verifies the tile-side storage; the cwd-vs-worktree
        // precedence lives in `git.rs::create_worktree`.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty {
            next_spawn_worktree: Some(crate::worktree::Worktree {
                source_repo: PathBuf::from("/tmp/repo"),
                worktree_path: PathBuf::from("/tmp/wt-out"),
                branch: "kookaburra/whatever-1234".into(),
                base_ref: "HEAD".into(),
                status: Default::default(),
            }),
            ..Default::default()
        };
        let ws_id = state.active_workspace;
        state.workspace_mut(ws_id).unwrap().project_repo = Some(PathBuf::from("/tmp/repo"));
        state.workspace_mut(ws_id).unwrap().worktree_mode = true;
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        let stored = state.tile(tile_id).unwrap().worktree.as_ref().unwrap();
        assert_eq!(stored.worktree_path, PathBuf::from("/tmp/wt-out"));
        assert_eq!(stored.branch, "kookaburra/whatever-1234");
    }

    #[test]
    fn spawn_in_tile_stores_worktree_metadata_returned_by_pty() {
        // The spawn callback decides the actual worktree path/branch (it
        // shells out to `git worktree add`). apply_action just stores the
        // returned metadata on the freshly promoted tile.
        let returned = crate::worktree::Worktree {
            source_repo: std::path::PathBuf::from("/tmp/repo"),
            worktree_path: std::path::PathBuf::from("/tmp/wt"),
            branch: "kookaburra/scratch-abcd".into(),
            base_ref: "HEAD".into(),
            status: Default::default(),
        };
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty {
            next_spawn_worktree: Some(returned.clone()),
            ..Default::default()
        };
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: Some(crate::worktree::WorktreeConfig {
                    source_repo: std::path::PathBuf::from("/tmp/repo"),
                    branch: None,
                    base_ref: None,
                    label_hint: "scratch".into(),
                }),
            },
        );
        assert_eq!(
            state.tile(tile_id).unwrap().worktree.as_ref(),
            Some(&returned)
        );
    }

    #[test]
    fn set_worktree_mode_turns_flag_on() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        // Worktree mode requires a detected repo.
        state.workspace_mut(ws_id).unwrap().project_repo = Some(PathBuf::from("/tmp/repo"));
        assert!(!state.workspace(ws_id).unwrap().worktree_mode);
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorktreeMode {
                id: ws_id,
                on: true,
            },
        );
        assert!(state.workspace(ws_id).unwrap().worktree_mode);
    }

    #[test]
    fn set_worktree_mode_turns_flag_off() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        state.workspace_mut(ws_id).unwrap().worktree_mode = true;
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorktreeMode {
                id: ws_id,
                on: false,
            },
        );
        assert!(!state.workspace(ws_id).unwrap().worktree_mode);
    }

    #[test]
    fn set_worktree_mode_on_unknown_workspace_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorktreeMode {
                id: WorkspaceId::new(),
                on: true,
            },
        );
        // Original workspace untouched.
        assert!(!state.active_workspace().worktree_mode);
    }

    #[test]
    fn set_worktree_mode_refused_when_no_project_repo() {
        // Guardrail: even if the UI somehow asks for worktree mode on a
        // workspace whose default_cwd isn't inside a repo, apply_action
        // refuses. (The UI is expected to disable the toggle in that
        // case, but state is authoritative.)
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        assert!(state.workspace(ws_id).unwrap().project_repo.is_none());
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorktreeMode {
                id: ws_id,
                on: true,
            },
        );
        assert!(!state.workspace(ws_id).unwrap().worktree_mode);
    }

    #[test]
    fn set_workspace_cwd_caches_project_repo_when_in_repo() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty {
            repo_root_for_next_lookup: Some(PathBuf::from("/tmp/repo")),
            ..Default::default()
        };
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: ws_id,
                cwd: Some(PathBuf::from("/tmp/repo/sub/dir")),
            },
        );
        assert_eq!(
            state.workspace(ws_id).unwrap().project_repo.as_deref(),
            Some(Path::new("/tmp/repo"))
        );
    }

    #[test]
    fn set_workspace_cwd_clears_project_repo_when_not_in_repo() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        // No repo_root_for_next_lookup set — `resolve_repo_root` returns None.
        let ws_id = state.active_workspace;
        state.workspace_mut(ws_id).unwrap().project_repo = Some(PathBuf::from("/tmp/old-repo"));
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: ws_id,
                cwd: Some(PathBuf::from("/tmp/elsewhere")),
            },
        );
        assert!(state.workspace(ws_id).unwrap().project_repo.is_none());
    }

    #[test]
    fn set_workspace_cwd_to_none_clears_project_repo_and_worktree_mode() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        {
            let ws = state.workspace_mut(ws_id).unwrap();
            ws.default_cwd = Some(PathBuf::from("/tmp/repo"));
            ws.project_repo = Some(PathBuf::from("/tmp/repo"));
            ws.worktree_mode = true;
        }
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: ws_id,
                cwd: None,
            },
        );
        let ws = state.workspace(ws_id).unwrap();
        assert!(ws.default_cwd.is_none());
        assert!(ws.project_repo.is_none());
        assert!(
            !ws.worktree_mode,
            "worktree_mode auto-clears when project_repo goes away"
        );
    }

    #[test]
    fn set_workspace_theme_on_unknown_workspace_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceTheme {
                workspace: WorkspaceId::new(),
                theme_name: Some("Tokyo Night".into()),
            },
        );
        // Original workspace still has no override.
        assert!(state.active_workspace().theme_override.is_none());
    }

    #[test]
    fn move_tile_on_empty_slot_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        let empty_id = state.active_workspace().tiles[0].id;
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        let target = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTile {
                tile_id: empty_id,
                target_workspace: target,
            },
        );
        // Nothing changed.
        assert!(state
            .workspace(source)
            .unwrap()
            .tiles
            .iter()
            .any(|t| t.id == empty_id));
        assert!(state
            .workspace(target)
            .unwrap()
            .tiles
            .iter()
            .all(|t| !t.is_live()));
    }
}
