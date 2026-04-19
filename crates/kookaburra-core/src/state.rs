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
    /// New workspace with a default 3×2 grid layout. Tiles are pre-filled
    /// with `layout.cell_count()` empty slots so the grid is visible from
    /// the moment the workspace exists — users instantiate a slot by
    /// clicking it or by focusing it and pressing Enter.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        let layout = Layout::Grid { cols: 3, rows: 2 };
        let tiles = (0..layout.cell_count()).map(|_| Tile::empty()).collect();
        Self {
            id: WorkspaceId::new(),
            label: label.into(),
            layout,
            tiles,
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

/// A single slot within a workspace. A slot with `pty_id == Some(_)` is
/// live (a terminal is running in it). A slot with `pty_id == None` is an
/// empty placeholder — rendered as a "+" box that the user can click or
/// focus+Enter to instantiate.
#[derive(Clone, Debug)]
pub struct Tile {
    pub id: TileId,
    pub pty_id: Option<PtyId>,
    /// Title as set by the shell via OSC sequences. Updated by the PTY
    /// event drain. Empty string for empty slots.
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
    /// Build a fresh live tile wired to an existing PTY. The tile's id is
    /// auto-generated; use [`Tile::with_id`] when you need to decide the
    /// id up-front (e.g. so a PTY event listener can tag events with the
    /// same id before the tile is inserted into state).
    #[must_use]
    pub fn new(pty_id: PtyId) -> Self {
        Self::with_id(TileId::new(), pty_id)
    }

    /// Build a live tile with an explicit id.
    #[must_use]
    pub fn with_id(id: TileId, pty_id: PtyId) -> Self {
        Self {
            id,
            pty_id: Some(pty_id),
            title: String::new(),
            has_new_output: false,
            follow_mode: true,
            cwd: None,
            worktree: None,
            bell_pending: false,
            last_output_at: None,
        }
    }

    /// Build an empty placeholder slot with a fresh id.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            id: TileId::new(),
            pty_id: None,
            title: String::new(),
            has_new_output: false,
            follow_mode: true,
            cwd: None,
            worktree: None,
            bell_pending: false,
            last_output_at: None,
        }
    }

    /// True if this slot currently hosts a running PTY.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.pty_id.is_some()
    }

    /// Promote an empty slot to live with the given PTY. Caller is
    /// responsible for having called [`PtySideEffects::spawn`] first.
    /// No-op if already live.
    pub fn promote(&mut self, pty_id: PtyId) {
        if self.pty_id.is_none() {
            self.pty_id = Some(pty_id);
        }
    }

    /// Demote a live slot back to empty, returning the former `PtyId` so
    /// the caller can close it. No-op and returns `None` if already empty.
    /// All live-state fields (title, worktree, output flags, etc.) are
    /// reset so the slot looks freshly minted.
    pub fn demote(&mut self) -> Option<PtyId> {
        let pty = self.pty_id.take()?;
        self.title.clear();
        self.worktree = None;
        self.has_new_output = false;
        self.cwd = None;
        self.last_output_at = None;
        self.bell_pending = false;
        self.follow_mode = true;
        Some(pty)
    }

    /// Move the live state of `src` into `self`. `self` must be empty;
    /// `src` is left empty. Both retain their original `TileId`s — this is
    /// how `MoveTile` relocates a terminal across workspaces without
    /// disturbing slot identity.
    pub fn take_live_state_from(&mut self, src: &mut Tile) {
        debug_assert!(self.pty_id.is_none(), "destination slot must be empty");
        debug_assert!(src.pty_id.is_some(), "source slot must be live");
        self.pty_id = src.pty_id.take();
        self.title = std::mem::take(&mut src.title);
        self.worktree = src.worktree.take();
        self.has_new_output = std::mem::replace(&mut src.has_new_output, false);
        self.cwd = src.cwd.take();
        self.last_output_at = src.last_output_at.take();
        self.bell_pending = std::mem::replace(&mut src.bell_pending, false);
        self.follow_mode = std::mem::replace(&mut src.follow_mode, true);
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
    fn new_state_has_one_workspace_full_of_empty_slots() {
        let s = AppState::new(Config::default());
        assert_eq!(s.workspaces.len(), 1);
        let ws = s.active_workspace();
        assert_eq!(ws.tiles.len(), ws.layout.cell_count());
        assert!(ws.tiles.iter().all(|t| !t.is_live()));
        assert_eq!(s.focused_tile, None);
        assert!(!s.zen_mode);
    }

    #[test]
    fn workspace_new_fills_tiles_with_cell_count_empties() {
        let ws = Workspace::new("scratch");
        assert_eq!(ws.tiles.len(), ws.layout.cell_count());
        assert!(ws.tiles.iter().all(|t| !t.is_live()));
        assert!(ws.tiles.iter().all(|t| t.title.is_empty()));
        assert!(ws.primary_tile.is_none());
    }

    #[test]
    fn workspace_new_gives_each_empty_slot_a_unique_id() {
        let ws = Workspace::new("scratch");
        let ids: std::collections::HashSet<_> = ws.tiles.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), ws.tiles.len(), "slot IDs must be unique");
    }

    #[test]
    fn active_workspace_returns_pointer_to_first_workspace() {
        let s = AppState::new(Config::default());
        assert_eq!(s.active_workspace().id, s.workspaces[0].id);
    }

    #[test]
    fn tile_insert_and_remove_roundtrips() {
        let mut s = AppState::new(Config::default());
        let starting_len = s.active_workspace().tiles.len();
        let tile = Tile::new(dummy_pty());
        let tile_id = tile.id;
        s.active_workspace_mut().push_tile(tile);
        assert_eq!(s.active_workspace().tiles.len(), starting_len + 1);
        assert!(s.tile(tile_id).is_some());

        let removed = s.active_workspace_mut().remove_tile(tile_id);
        assert!(removed.is_some());
        assert_eq!(s.active_workspace().tiles.len(), starting_len);
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

    #[test]
    fn any_tile_dirty_is_false_when_workspace_is_all_empty_slots() {
        let s = AppState::new(Config::default());
        assert!(!s.any_tile_dirty(), "empty slots never have new output");
    }

    #[test]
    fn empty_tile_has_no_pty_and_default_fields() {
        let t = Tile::empty();
        assert!(!t.is_live());
        assert!(t.pty_id.is_none());
        assert_eq!(t.title, "");
        assert!(t.worktree.is_none());
        assert!(!t.has_new_output);
        assert!(t.follow_mode, "follow_mode defaults true so new terminals auto-scroll");
    }

    #[test]
    fn live_tile_reports_is_live() {
        let t = Tile::new(dummy_pty());
        assert!(t.is_live());
        assert!(t.pty_id.is_some());
    }

    #[test]
    fn promote_fills_empty_slot() {
        let mut t = Tile::empty();
        let pty = dummy_pty();
        t.promote(pty);
        assert!(t.is_live());
        assert_eq!(t.pty_id, Some(pty));
    }

    #[test]
    fn promote_is_noop_on_live_slot() {
        let first = dummy_pty();
        let second = dummy_pty();
        let mut t = Tile::new(first);
        t.promote(second);
        assert_eq!(t.pty_id, Some(first), "second promote should not clobber existing PTY");
    }

    #[test]
    fn demote_clears_fields_and_returns_former_pty() {
        let pty = dummy_pty();
        let mut t = Tile::new(pty);
        t.title = "shell".into();
        t.has_new_output = true;
        t.bell_pending = true;
        t.last_output_at = Some(Instant::now());

        let returned = t.demote();
        assert_eq!(returned, Some(pty));
        assert!(!t.is_live());
        assert_eq!(t.title, "");
        assert!(!t.has_new_output);
        assert!(!t.bell_pending);
        assert!(t.last_output_at.is_none());
        assert!(t.follow_mode, "follow_mode resets to default");
    }

    #[test]
    fn demote_on_empty_slot_returns_none() {
        let mut t = Tile::empty();
        assert!(t.demote().is_none());
        assert!(!t.is_live());
    }

    #[test]
    fn take_live_state_from_relocates_pty_and_keeps_both_ids() {
        let pty = dummy_pty();
        let mut src = Tile::new(pty);
        src.title = "shell".into();
        let src_id = src.id;

        let mut dest = Tile::empty();
        let dest_id = dest.id;

        dest.take_live_state_from(&mut src);

        assert_eq!(dest.pty_id, Some(pty));
        assert_eq!(dest.title, "shell");
        assert_eq!(dest.id, dest_id, "destination keeps its slot TileId");
        assert!(!src.is_live());
        assert_eq!(src.title, "");
        assert_eq!(src.id, src_id, "source keeps its slot TileId");
    }
}
