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

use crate::ids::{PtyId, TileId, WorkspaceId};
use crate::layout::Layout;
use crate::state::{AppState, Tile, Workspace};
use crate::worktree::WorktreeConfig;

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
    CreateTile {
        workspace: WorkspaceId,
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
}

/// Side effects the action handler may need to ask the PTY layer to
/// perform. Implemented by `kookaburra-pty::PtyManager` in the app crate.
///
/// Keeping this a trait lets the core crate stay free of tokio /
/// alacritty_terminal and lets us unit-test `apply_action` against a
/// stub.
pub trait PtySideEffects {
    /// Spawn a new PTY bound to `tile_id` and return its id. The `tile_id`
    /// is decided by `apply_action` before this call so the PTY's event
    /// listener can tag events with the same id the `Tile` will carry.
    /// The implementation chooses CWD and shell from the worktree config +
    /// app defaults.
    fn spawn(&mut self, tile_id: TileId, worktree: Option<&WorktreeConfig>) -> PtyId;
    /// Close a PTY. Best-effort; failures should be logged, not returned.
    fn close(&mut self, pty: PtyId);
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
                // When switching, honor primary tile if set, else focus
                // the first tile in the workspace.
                let primary = state.active_workspace().primary_tile;
                let first = state.active_workspace().tiles.first().map(|t| t.id);
                state.focused_tile = primary.or(first);
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
            // Close every PTY that belonged to the workspace.
            if let Some(ws) = state.workspace(id) {
                let pty_ids: Vec<PtyId> = ws.tiles.iter().map(|t| t.pty_id).collect();
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
                let next_id = state.workspaces[0].id;
                let first_tile = state.workspaces[0].tiles.first().map(|t| t.id);
                state.active_workspace = next_id;
                state.focused_tile = first_tile;
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
        Action::CreateTile {
            workspace,
            worktree,
        } => {
            if state.workspace(workspace).is_some() {
                let tile_id = TileId::new();
                let pty_id = pty.spawn(tile_id, worktree.as_ref());
                let tile = Tile::with_id(tile_id, pty_id);
                if let Some(ws) = state.workspace_mut(workspace) {
                    ws.push_tile(tile);
                }
                state.focused_tile = Some(tile_id);
            }
        }
        Action::CloseTile(tile_id) => {
            // Find the pty + workspace for this tile.
            let mut found: Option<(WorkspaceId, PtyId)> = None;
            for ws in &state.workspaces {
                if let Some(t) = ws.tile(tile_id) {
                    found = Some((ws.id, t.pty_id));
                    break;
                }
            }
            if let Some((ws_id, pty_id)) = found {
                pty.close(pty_id);
                if let Some(ws) = state.workspace_mut(ws_id) {
                    ws.remove_tile(tile_id);
                }
                if state.focused_tile == Some(tile_id) {
                    let first = state.active_workspace().tiles.first().map(|t| t.id);
                    state.focused_tile = first;
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
            // Pull the tile out of its current workspace.
            let mut extracted: Option<Tile> = None;
            for ws in state.workspaces.iter_mut() {
                if ws.tile(tile_id).is_some() {
                    extracted = ws.remove_tile(tile_id);
                    break;
                }
            }
            if let (Some(tile), Some(ws)) = (extracted, state.workspace_mut(target_workspace)) {
                ws.push_tile(tile);
            }
        }
        Action::MoveTileToNewWorkspace { tile_id } => {
            // Extract the tile from whichever workspace holds it.
            let mut extracted: Option<Tile> = None;
            for ws in state.workspaces.iter_mut() {
                if ws.tile(tile_id).is_some() {
                    extracted = ws.remove_tile(tile_id);
                    break;
                }
            }
            if let Some(tile) = extracted {
                let idx = state.workspaces.len() + 1;
                let mut ws = Workspace::new(format!("workspace {idx}"));
                let new_ws_id = ws.id;
                ws.push_tile(tile);
                state.workspaces.push(ws);
                state.active_workspace = new_ws_id;
                state.focused_tile = Some(tile_id);
            }
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
    }

    impl PtySideEffects for StubPty {
        fn spawn(&mut self, tile_id: TileId, _worktree: Option<&WorktreeConfig>) -> PtyId {
            self.spawns += 1;
            self.last_spawn_tile = Some(tile_id);
            PtyId::new()
        }
        fn close(&mut self, _pty: PtyId) {
            self.closes += 1;
        }
    }

    #[test]
    fn create_tile_spawns_pty_and_inserts_tile() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::CreateTile {
                workspace: ws_id,
                worktree: None,
            },
        );
        assert_eq!(pty.spawns, 1);
        assert_eq!(state.active_workspace().tiles.len(), 1);
        assert!(state.focused_tile.is_some());
    }

    #[test]
    fn close_tile_closes_pty_and_removes_tile() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::CreateTile {
                workspace: ws_id,
                worktree: None,
            },
        );
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(&mut state, &mut pty, Action::CloseTile(tile_id));
        assert_eq!(pty.closes, 1);
        assert!(state.active_workspace().tiles.is_empty());
    }

    #[test]
    fn create_tile_passes_same_tile_id_to_spawn_and_tile() {
        // Regression: if apply_action generates the TileId *after* calling
        // spawn, the PTY's event proxy is tagged with a stale id and no
        // PtyEvent::OutputReceived will ever find its tile.
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::CreateTile {
                workspace: ws_id,
                worktree: None,
            },
        );
        let tile_id = state.active_workspace().tiles[0].id;
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
    fn delete_last_workspace_reseeds_with_empty_one() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let only = state.active_workspace;
        apply_action(&mut state, &mut pty, Action::DeleteWorkspace(only));
        assert_eq!(state.workspaces.len(), 1);
    }

    #[test]
    fn move_tile_to_new_workspace_creates_and_switches() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::CreateTile {
                workspace: source,
                worktree: None,
            },
        );
        let tile_id = state.active_workspace().tiles[0].id;
        let before = state.workspaces.len();
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTileToNewWorkspace { tile_id },
        );
        assert_eq!(state.workspaces.len(), before + 1);
        assert_eq!(state.active_workspace().tiles.len(), 1);
        assert_eq!(state.active_workspace().tiles[0].id, tile_id);
        assert_ne!(state.active_workspace, source);
        assert_eq!(state.focused_tile, Some(tile_id));
        // Source workspace is now empty.
        assert!(state.workspace(source).unwrap().tiles.is_empty());
    }

    #[test]
    fn move_tile_relocates_it_to_target_workspace() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::CreateTile {
                workspace: source,
                worktree: None,
            },
        );
        let tile_id = state.active_workspace().tiles[0].id;
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
        assert_eq!(
            state
                .workspace(source)
                .expect("source workspace still there")
                .tiles
                .len(),
            0
        );
        assert_eq!(
            state.workspace(target).expect("target exists").tiles.len(),
            1
        );
    }
}
