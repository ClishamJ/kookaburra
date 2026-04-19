//! Tile layouts and rect computation.

/// Axis-aligned rectangle in logical pixels (top-left origin).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Tile arrangement within a workspace. v1 supports uniform grids only;
/// arbitrary splits are deferred per spec §3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Grid { cols: u8, rows: u8 },
}

impl Layout {
    /// Total tile count for this layout.
    #[must_use]
    pub const fn cell_count(self) -> usize {
        match self {
            Self::Grid { cols, rows } => (cols as usize) * (rows as usize),
        }
    }

    /// Short human-readable label like "2x2", "3x2", "1x1".
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Grid { cols, rows } => format!("{cols}x{rows}"),
        }
    }
}

/// Computes per-tile rects in row-major order within the given `area`.
///
/// For `Grid { cols: 0, rows: _ }` or `{ cols: _, rows: 0 }` returns an
/// empty vec.
#[must_use]
pub fn compute_tile_rects(layout: Layout, area: Rect) -> Vec<Rect> {
    match layout {
        Layout::Grid { cols, rows } => {
            if cols == 0 || rows == 0 {
                return Vec::new();
            }
            let cell_w = area.width / f32::from(cols);
            let cell_h = area.height / f32::from(rows);
            let mut out = Vec::with_capacity(usize::from(cols) * usize::from(rows));
            for row in 0..rows {
                for col in 0..cols {
                    out.push(Rect {
                        x: area.x + f32::from(col) * cell_w,
                        y: area.y + f32::from(row) * cell_h,
                        width: cell_w,
                        height: cell_h,
                    });
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: f32, h: f32) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn grid_1x1_is_the_whole_area() {
        let rects = compute_tile_rects(Layout::Grid { cols: 1, rows: 1 }, area(800.0, 600.0));
        assert_eq!(
            rects,
            vec![Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0
            }]
        );
    }

    #[test]
    fn grid_2x1_splits_horizontally() {
        let rects = compute_tile_rects(Layout::Grid { cols: 2, rows: 1 }, area(800.0, 600.0));
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 600.0
                },
                Rect {
                    x: 400.0,
                    y: 0.0,
                    width: 400.0,
                    height: 600.0
                },
            ]
        );
    }

    #[test]
    fn grid_1x2_splits_vertically() {
        let rects = compute_tile_rects(Layout::Grid { cols: 1, rows: 2 }, area(800.0, 600.0));
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 800.0,
                    height: 300.0
                },
                Rect {
                    x: 0.0,
                    y: 300.0,
                    width: 800.0,
                    height: 300.0
                },
            ]
        );
    }

    #[test]
    fn grid_2x2_tiles_quadrants() {
        let rects = compute_tile_rects(Layout::Grid { cols: 2, rows: 2 }, area(800.0, 600.0));
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 0.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
            ]
        );
    }

    #[test]
    fn grid_3x2_is_row_major() {
        let rects = compute_tile_rects(Layout::Grid { cols: 3, rows: 2 }, area(1200.0, 600.0));
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 800.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 0.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 800.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
            ]
        );
    }

    #[test]
    fn grid_2x3_is_row_major() {
        let rects = compute_tile_rects(Layout::Grid { cols: 2, rows: 3 }, area(800.0, 900.0));
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 0.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 0.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 300.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 0.0,
                    y: 600.0,
                    width: 400.0,
                    height: 300.0
                },
                Rect {
                    x: 400.0,
                    y: 600.0,
                    width: 400.0,
                    height: 300.0
                },
            ]
        );
    }

    #[test]
    fn non_zero_origin_is_respected() {
        let a = Rect {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        };
        let rects = compute_tile_rects(Layout::Grid { cols: 2, rows: 1 }, a);
        assert_eq!(
            rects,
            vec![
                Rect {
                    x: 10.0,
                    y: 20.0,
                    width: 400.0,
                    height: 600.0
                },
                Rect {
                    x: 410.0,
                    y: 20.0,
                    width: 400.0,
                    height: 600.0
                },
            ]
        );
    }

    #[test]
    fn grid_with_zero_rows_or_cols_produces_no_rects() {
        assert!(
            compute_tile_rects(Layout::Grid { cols: 0, rows: 2 }, area(800.0, 600.0)).is_empty()
        );
        assert!(
            compute_tile_rects(Layout::Grid { cols: 2, rows: 0 }, area(800.0, 600.0)).is_empty()
        );
    }
}
