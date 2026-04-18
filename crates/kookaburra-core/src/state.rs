//! Application, workspace, and tile state.
//!
//! The shape here mirrors the spec §5 model with one deviation: the live
//! `Term` handle does not live on `Tile`. Instead, `Tile::pty_id`
//! identifies the live terminal, and `kookaburra-pty::PtyManager` owns
//! the `Arc<FairMutex<Term<_>>>`. This keeps `kookaburra-core` free of
//! the alacritty_terminal dependency (see NOTES.md).

use std::path::PathBuf;
use std::time::Instant;

use crate::config::Config;
use crate::ids::{PtyId, TileId, WorkspaceId};
use crate::layout::Layout;
use crate::worktree::Worktree;

/// Top-level application state. Mutated only by `apply_action`.
#[derive(Clone, Debug)]
pub struct AppState {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: WorkspaceId,
    pub focused_tile: Option<TileId>,
    pub config: Config,
    pub zen_mode: bool,
    /// Single redraw flag. PTY events, input events, and layout changes
    /// all set this; the main loop reads it at the end of each iteration
    /// to decide between `request_redraw` and `ControlFlow::Wait`.
    pub needs_redraw: bool,
}

impl AppState {
    /// Create a new app state with a single empty workspace.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let initial = Workspace::new("workspace 1");
        let active = initial.id;
        Self {
            workspaces: vec![initial],
            active_workspace: active,
            focused_tile: None,
            config,
            zen_mode: false,
            needs_redraw: true,
        }
    }

    /// Returns the currently active workspace.
    #[must_use]
    pub fn active_workspace(&self) -> &Workspace {
        self.workspace(self.active_workspace)
            .expect("active_workspace id always refers to an existing workspace")
    }

    /// Returns the currently active workspace, mutable.
    #[must_use]
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        let id = self.active_workspace;
        self.workspace_mut(id)
            .expect("active_workspace id always refers to an existing workspace")
    }

    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    #[must_use]
    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    /// Look up a tile by id across all workspaces.
    #[must_use]
    pub fn tile(&self, id: TileId) -> Option<&Tile> {
        self.workspaces.iter().find_map(|w| w.tile(id))
    }

    /// Look up a tile mutably across all workspaces.
    #[must_use]
    pub fn tile_mut(&mut self, id: TileId) -> Option<&mut Tile> {
        self.workspaces.iter_mut().find_map(|w| w.tile_mut(id))
    }

    /// Returns `true` if any workspace contains any tile with new output
    /// since last frame (cheap scan, used for the "unread" strip signal).
    #[must_use]
    pub fn any_tile_dirty(&self) -> bool {
        self.workspaces
            .iter()
            .flat_map(|w| w.tiles.iter())
            .any(|t| t.has_new_output)
    }

    /// Clear `needs_redraw` after a frame has been drawn.
    pub fn mark_redrawn(&mut self) {
        self.needs_redraw = false;
    }

    /// Ask for a redraw on the next loop iteration.
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }
}

/// A named group of related tiles. Maps 1:1 to a card in the strip.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
    pub layout: Layout,
    pub tiles: Vec<Tile>,
    /// Optional designated tile that gets focus when switching to this
    /// workspace.
    pub primary_tile: Option<TileId>,
}

impl Workspace {
    /// New empty workspace with a default 3×2 grid layout.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: WorkspaceId::new(),
            label: label.into(),
            layout: Layout::Grid { cols: 3, rows: 2 },
            tiles: Vec::new(),
            primary_tile: None,
        }
    }

    #[must_use]
    pub fn tile(&self, id: TileId) -> Option<&Tile> {
        self.tiles.iter().find(|t| t.id == id)
    }

    #[must_use]
    pub fn tile_mut(&mut self, id: TileId) -> Option<&mut Tile> {
        self.tiles.iter_mut().find(|t| t.id == id)
    }

    /// Insert a new tile at the end. Caller is responsible for layout
    /// capacity; grid slots beyond `cell_count` are simply not rendered.
    pub fn push_tile(&mut self, tile: Tile) -> TileId {
        let id = tile.id;
        self.tiles.push(tile);
        id
    }

    /// Remove a tile. Returns the removed tile if found. Clears the
    /// primary pointer if it was pointing at this tile.
    pub fn remove_tile(&mut self, id: TileId) -> Option<Tile> {
        let idx = self.tiles.iter().position(|t| t.id == id)?;
        if self.primary_tile == Some(id) {
            self.primary_tile = None;
        }
        Some(self.tiles.remove(idx))
    }
}

/// A single terminal pane within a workspace.
#[derive(Clone, Debug)]
pub struct Tile {
    pub id: TileId,
    pub pty_id: PtyId,
    /// Title as set by the shell via OSC sequences. Updated by the PTY
    /// event drain.
    pub title: String,
    /// Set whenever the PTY produces output; cleared when the user
    /// interacts with the tile.
    pub has_new_output: bool,
    /// Auto-scroll on new output.
    pub follow_mode: bool,
    /// Current working directory, as reported by shell integration (OSC
    /// 7). Used for smart labels and worktree detection.
    pub cwd: Option<PathBuf>,
    /// Populated when the tile is in worktree mode (see spec §4).
    pub worktree: Option<Worktree>,
    /// Whether the user has rung the bell since we last drew.
    pub bell_pending: bool,
    /// Wall-clock timestamp of the last PTY output batch, if any. Used by
    /// the strip to render a "generating" / recent-activity signal on
    /// workspace cards. Stays `None` for tiles that have never emitted
    /// output. Not persisted — lost across restarts, which is correct for
    /// a "what's active right now" hint.
    pub last_output_at: Option<Instant>,
}

impl Tile {
    /// Build a fresh tile wired to an existing PTY. The tile's id is
    /// auto-generated; use [`Tile::with_id`] when you need to decide the
    /// id up-front (e.g. so a PTY event listener can tag events with the
    /// same id before the tile is inserted into state).
    #[must_use]
    pub fn new(pty_id: PtyId) -> Self {
        Self::with_id(TileId::new(), pty_id)
    }

    /// Build a tile with an explicit id. Used by `apply_action::CreateTile`
    /// so the `TileId` passed to `PtySideEffects::spawn` matches the id of
    /// the resulting `Tile`.
    #[must_use]
    pub fn with_id(id: TileId, pty_id: PtyId) -> Self {
        Self {
            id,
            pty_id,
            title: String::new(),
            has_new_output: false,
            follow_mode: true,
            cwd: None,
            worktree: None,
            bell_pending: false,
            last_output_at: None,
        }
    }

    /// Attach worktree metadata to a tile that was created in worktree
    /// mode.
    #[must_use]
    pub fn with_worktree(mut self, wt: Worktree) -> Self {
        self.worktree = Some(wt);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pty() -> PtyId {
        PtyId::new()
    }

    #[test]
    fn new_state_has_one_workspace_no_tiles() {
        let s = AppState::new(Config::default());
        assert_eq!(s.workspaces.len(), 1);
        assert!(s.active_workspace().tiles.is_empty());
        assert_eq!(s.focused_tile, None);
        assert!(!s.zen_mode);
    }

    #[test]
    fn active_workspace_returns_pointer_to_first_workspace() {
        let s = AppState::new(Config::default());
        assert_eq!(s.active_workspace().id, s.workspaces[0].id);
    }

    #[test]
    fn tile_insert_and_remove_roundtrips() {
        let mut s = AppState::new(Config::default());
        let tile = Tile::new(dummy_pty());
        let tile_id = tile.id;
        s.active_workspace_mut().push_tile(tile);
        assert_eq!(s.active_workspace().tiles.len(), 1);
        assert!(s.tile(tile_id).is_some());

        let removed = s.active_workspace_mut().remove_tile(tile_id);
        assert!(removed.is_some());
        assert!(s.active_workspace().tiles.is_empty());
        assert!(s.tile(tile_id).is_none());
    }

    #[test]
    fn removing_primary_tile_clears_primary_pointer() {
        let mut s = AppState::new(Config::default());
        let tile = Tile::new(dummy_pty());
        let tile_id = tile.id;
        s.active_workspace_mut().push_tile(tile);
        s.active_workspace_mut().primary_tile = Some(tile_id);
        s.active_workspace_mut().remove_tile(tile_id);
        assert_eq!(s.active_workspace().primary_tile, None);
    }

    #[test]
    fn any_tile_dirty_reflects_has_new_output() {
        let mut s = AppState::new(Config::default());
        let mut tile = Tile::new(dummy_pty());
        tile.has_new_output = true;
        s.active_workspace_mut().push_tile(tile);
        assert!(s.any_tile_dirty());
    }
}
