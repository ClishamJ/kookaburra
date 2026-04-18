//! Per-frame snapshot of a tile's visible terminal grid.
//!
//! Defined in core (rather than render or pty) because both crates need
//! to know its shape, and core has no I/O dependencies. The pty crate
//! fills snapshots; the render crate reads from them.
//!
//! Spec §6 ("Render pipeline — deep dive") describes the lifecycle:
//! preallocate one per tile, clear-and-refill each frame, never allocate
//! in the steady-state hot path.

use crate::config::Rgba;
use crate::ids::TileId;

bitflags::bitflags! {
    /// Per-cell rendering flags. Mirrors the subset of
    /// `alacritty_terminal`'s `Flags` we actually draw.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct CellFlags: u16 {
        const BOLD       = 0b0000_0001;
        const ITALIC     = 0b0000_0010;
        const UNDERLINE  = 0b0000_0100;
        const INVERSE    = 0b0000_1000;
        const STRIKE     = 0b0001_0000;
        const WIDE_CHAR  = 0b0010_0000;
        const HIDDEN     = 0b0100_0000;
    }
}

/// One terminal cell, ready to draw.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RenderCell {
    pub ch: char,
    pub fg: Rgba,
    pub bg: Rgba,
    pub flags: CellFlags,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Rgba::default(),
            bg: Rgba::default(),
            flags: CellFlags::empty(),
        }
    }
}

/// Cursor styles we surface to the renderer. Maps to whatever the shell
/// requested via DECSCUSR; default is a steady block.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Beam,
}

/// Inclusive selection range in (col, row) cell coordinates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: (u16, u16),
    pub end: (u16, u16),
}

/// One tile's drawable state for a single frame.
///
/// Allocate once per tile, then clear+refill in `pty::PtyManager::snapshot`.
#[derive(Clone, Debug)]
pub struct TileSnapshot {
    pub tile_id: TileId,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<RenderCell>,
    pub cursor: Option<(u16, u16)>,
    pub cursor_style: CursorStyle,
    pub selection: Option<SelectionRange>,
    /// Title from OSC sequences. The renderer doesn't draw this directly;
    /// the strip card does.
    pub title: String,
}

impl TileSnapshot {
    #[must_use]
    pub fn new(tile_id: TileId) -> Self {
        Self {
            tile_id,
            cols: 0,
            rows: 0,
            cells: Vec::new(),
            cursor: None,
            cursor_style: CursorStyle::default(),
            selection: None,
            title: String::new(),
        }
    }

    /// Reset for a fresh frame without freeing the backing allocation.
    pub fn clear(&mut self) {
        self.cols = 0;
        self.rows = 0;
        self.cells.clear();
        self.cursor = None;
        self.selection = None;
    }

    /// Flat index into `cells` for `(col, row)`. Returns `None` if out of
    /// bounds, useful for callers that walk row-by-row.
    #[must_use]
    pub fn index(&self, col: u16, row: u16) -> Option<usize> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        Some(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_snapshot_is_empty_and_has_no_cursor() {
        let s = TileSnapshot::new(TileId::new());
        assert_eq!(s.cols, 0);
        assert_eq!(s.rows, 0);
        assert!(s.cells.is_empty());
        assert!(s.cursor.is_none());
        assert!(s.selection.is_none());
        assert_eq!(s.cursor_style, CursorStyle::Block);
        assert!(s.title.is_empty());
    }

    #[test]
    fn clear_keeps_allocation_but_zeros_dimensions() {
        let mut s = TileSnapshot::new(TileId::new());
        s.cols = 4;
        s.rows = 2;
        s.cells.resize(8, RenderCell::default());
        s.cursor = Some((1, 1));
        s.selection = Some(SelectionRange {
            start: (0, 0),
            end: (3, 1),
        });
        let cap_before = s.cells.capacity();
        s.clear();
        assert_eq!(s.cols, 0);
        assert_eq!(s.rows, 0);
        assert!(s.cells.is_empty());
        assert!(s.cursor.is_none());
        assert!(s.selection.is_none());
        // The allocation must survive so the hot path doesn't re-alloc.
        assert!(s.cells.capacity() >= cap_before);
    }

    #[test]
    fn index_is_row_major_and_checks_bounds() {
        let mut s = TileSnapshot::new(TileId::new());
        s.cols = 4;
        s.rows = 2;
        s.cells.resize(8, RenderCell::default());
        assert_eq!(s.index(0, 0), Some(0));
        assert_eq!(s.index(3, 0), Some(3));
        assert_eq!(s.index(0, 1), Some(4));
        assert_eq!(s.index(3, 1), Some(7));
        assert_eq!(s.index(4, 0), None);
        assert_eq!(s.index(0, 2), None);
    }

    #[test]
    fn cell_flags_are_composable() {
        let f = CellFlags::BOLD | CellFlags::ITALIC;
        assert!(f.contains(CellFlags::BOLD));
        assert!(f.contains(CellFlags::ITALIC));
        assert!(!f.contains(CellFlags::UNDERLINE));
    }
}
