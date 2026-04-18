//! PTY management with a live `alacritty_terminal::Term`.
//!
//! The manager owns one [`PtyHandle`] per spawned session. Each handle
//! holds the PTY master, a writer, a shared [`Term`] protected by a
//! `FairMutex`, and a std thread reading bytes from the reader side and
//! feeding them through a `vte::ansi::Processor` into the `Term`.
//!
//! The spec (KOOKABURRA.md §5) expects the `Term` to live behind an
//! `Arc<FairMutex<_>>` so the render thread can take a quick lock each
//! frame to build a snapshot. Reader threads hold the lock only long
//! enough to advance the parser on one chunk of bytes.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags as CellFlagsAlac;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{
    Color, NamedColor, Processor as AnsiProcessor, Rgb, StdSyncHandler,
};
use parking_lot::FairMutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use kookaburra_core::config::{Rgba, Theme};
use kookaburra_core::ids::{PtyId, TileId};
use kookaburra_core::snapshot::{CellFlags, CursorStyle, RenderCell, TileSnapshot};

/// Cross-channel events emitted by PTY readers. Coalesce in the consumer:
/// many `OutputReceived` events between frames should produce one redraw.
#[derive(Clone, Debug)]
pub enum PtyEvent {
    OutputReceived { tile_id: TileId },
    ProcessExited { tile_id: TileId },
    TitleChanged { tile_id: TileId, title: String },
    BellRang { tile_id: TileId },
}

/// Cross-thread sink for PTY events. The app crate implements this on top
/// of `winit::EventLoopProxy` so reader threads both enqueue the event
/// and wake the event loop in a single call — without this, winit sleeps
/// on `ControlFlow::Wait` and shell output would sit un-drained until the
/// next OS event (causing visible typing lag).
pub trait PtyEventSink: Send + Sync {
    fn emit(&self, event: PtyEvent);
}

/// What the spawn API needs from the caller.
#[derive(Clone, Debug)]
pub struct SpawnRequest {
    pub tile_id: TileId,
    pub cwd: Option<std::path::PathBuf>,
    pub shell: Option<String>,
    pub size: PtySize,
}

impl Default for SpawnRequest {
    fn default() -> Self {
        Self {
            tile_id: TileId::new(),
            cwd: None,
            shell: None,
            size: PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        }
    }
}

/// Listener we hand to the `Term`. Forwards title/bell/exit events through
/// the shared `PtyEventSink`.
#[derive(Clone)]
pub struct EventProxy {
    tile_id: TileId,
    sink: Arc<dyn PtyEventSink>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(title) => {
                self.sink.emit(PtyEvent::TitleChanged {
                    tile_id: self.tile_id,
                    title,
                });
            }
            TermEvent::ResetTitle => {
                self.sink.emit(PtyEvent::TitleChanged {
                    tile_id: self.tile_id,
                    title: String::new(),
                });
            }
            TermEvent::Bell => {
                self.sink.emit(PtyEvent::BellRang {
                    tile_id: self.tile_id,
                });
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => {
                self.sink.emit(PtyEvent::ProcessExited {
                    tile_id: self.tile_id,
                });
            }
            // Other events (clipboard, color queries, text-area-size, etc.)
            // are ignored until the matching spec phases wire them up.
            _ => {}
        }
    }
}

/// Minimal `Dimensions` impl used to seed/resize a `Term`.
#[derive(Copy, Clone, Debug)]
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// One live terminal session.
struct PtyHandle {
    tile_id: TileId,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    term: Arc<FairMutex<Term<EventProxy>>>,
    /// Reader thread join handle. Detached — closes when PTY EOFs.
    _reader: Option<JoinHandle<()>>,
}

/// Manager exposed to the main loop. Synchronous interface; readers run
/// on their own threads and forward dirty signals through the shared
/// `PtyEventSink`.
pub struct PtyManager {
    ptys: HashMap<PtyId, PtyHandle>,
    sink: Arc<dyn PtyEventSink>,
}

impl PtyManager {
    #[must_use]
    pub fn new(sink: Arc<dyn PtyEventSink>) -> Self {
        Self {
            ptys: HashMap::new(),
            sink,
        }
    }

    /// Spawn a shell in a fresh PTY.
    pub fn spawn(&mut self, req: SpawnRequest) -> Result<PtyId, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(req.size)
            .map_err(|e| format!("openpty failed: {e}"))?;

        let shell = req.shell.unwrap_or_else(default_shell);
        let mut cmd = CommandBuilder::new(shell);
        if let Some(cwd) = &req.cwd {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("KOOKABURRA", "1");

        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn_command failed: {e}"))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take_writer failed: {e}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("try_clone_reader failed: {e}"))?;

        let size = TermSize {
            cols: usize::from(req.size.cols),
            rows: usize::from(req.size.rows),
        };
        let proxy = EventProxy {
            tile_id: req.tile_id,
            sink: self.sink.clone(),
        };
        let term = Term::new(TermConfig::default(), &size, proxy);
        let term = Arc::new(FairMutex::new(term));

        let reader_thread =
            spawn_reader_thread(req.tile_id, reader, self.sink.clone(), term.clone());

        let pty_id = PtyId::new();
        self.ptys.insert(
            pty_id,
            PtyHandle {
                tile_id: req.tile_id,
                master: pair.master,
                writer,
                term,
                _reader: Some(reader_thread),
            },
        );
        Ok(pty_id)
    }

    /// Send raw bytes to the PTY's stdin.
    pub fn write(&mut self, pty: PtyId, bytes: &[u8]) -> Result<(), String> {
        let handle = self
            .ptys
            .get_mut(&pty)
            .ok_or_else(|| format!("unknown pty {pty}"))?;
        handle
            .writer
            .write_all(bytes)
            .map_err(|e| format!("pty write failed: {e}"))?;
        handle.writer.flush().ok();
        Ok(())
    }

    /// Resize the PTY + inner `Term`. Must be called on every window/tile
    /// resize so the inferior process learns about the new geometry
    /// (`TIOCSWINSZ`) and our grid matches.
    pub fn resize(&mut self, pty: PtyId, size: PtySize) -> Result<(), String> {
        let handle = self
            .ptys
            .get_mut(&pty)
            .ok_or_else(|| format!("unknown pty {pty}"))?;
        handle
            .master
            .resize(size)
            .map_err(|e| format!("resize failed: {e}"))?;
        let term_size = TermSize {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        };
        handle.term.lock().resize(term_size);
        Ok(())
    }

    /// Drop a PTY. The reader thread terminates on EOF.
    pub fn close(&mut self, pty: PtyId) {
        self.ptys.remove(&pty);
    }

    /// Scroll the display by `lines` (positive = up into scrollback, negative
    /// = down toward live output). Returns `true` if the viewport moved,
    /// which the caller uses to decide whether to mark the tile dirty.
    pub fn scroll(&self, pty: PtyId, lines: i32) -> bool {
        let Some(handle) = self.ptys.get(&pty) else {
            return false;
        };
        let mut term = handle.term.lock();
        let before = term.grid().display_offset();
        term.scroll_display(Scroll::Delta(lines));
        let after = term.grid().display_offset();
        before != after
    }

    /// Reset the viewport to live output. Called after the user types so
    /// the next key doesn't stay scrolled up.
    pub fn scroll_to_bottom(&self, pty: PtyId) -> bool {
        let Some(handle) = self.ptys.get(&pty) else {
            return false;
        };
        let mut term = handle.term.lock();
        let before = term.grid().display_offset();
        term.scroll_display(Scroll::Bottom);
        let after = term.grid().display_offset();
        before != after
    }

    /// Extract the visible grid as plain text. Rows are separated by `\n`.
    /// Used by Cmd+C / Cmd+A to feed the clipboard without needing a
    /// drag-selection implementation.
    #[must_use]
    pub fn visible_text(&self, pty: PtyId) -> String {
        let Some(handle) = self.ptys.get(&pty) else {
            return String::new();
        };
        let term = handle.term.lock();
        let cols = term.columns();
        let rows = term.screen_lines();
        if cols == 0 || rows == 0 {
            return String::new();
        }
        // Render row-major, trimming trailing spaces on each row. That
        // matches what users expect from terminal copy (no ragged padding).
        let mut grid: Vec<Vec<char>> = vec![vec![' '; cols]; rows];
        let content = term.renderable_content();
        let display_offset = content.display_offset as i32;
        for indexed in content.display_iter {
            let Point { line, column } = indexed.point;
            let r = line.0 + display_offset;
            if r < 0 || r as usize >= rows {
                continue;
            }
            let c = column.0;
            if c >= cols {
                continue;
            }
            grid[r as usize][c] = indexed.cell.c;
        }
        let mut out = String::with_capacity(rows * (cols + 1));
        for (i, row) in grid.iter().enumerate() {
            let mut end = row.len();
            while end > 0 && row[end - 1] == ' ' {
                end -= 1;
            }
            for ch in &row[..end] {
                out.push(*ch);
            }
            if i + 1 < rows {
                out.push('\n');
            }
        }
        out
    }

    /// Return the tile id paired with a PTY.
    #[must_use]
    pub fn tile_for(&self, pty: PtyId) -> Option<TileId> {
        self.ptys.get(&pty).map(|h| h.tile_id)
    }

    /// Snapshot the visible grid into `dst`. Walks `Term::renderable_content`
    /// and copies each cell into the destination, resolving named/indexed
    /// colors against the supplied theme.
    pub fn snapshot(&self, pty: PtyId, theme: &Theme, dst: &mut TileSnapshot) {
        dst.clear();
        let Some(handle) = self.ptys.get(&pty) else {
            return;
        };
        let term = handle.term.lock();
        let cols_usize = term.columns();
        let rows_usize = term.screen_lines();
        let cols = cols_usize.min(u16::MAX as usize) as u16;
        let rows = rows_usize.min(u16::MAX as usize) as u16;
        dst.cols = cols;
        dst.rows = rows;
        dst.cells
            .resize(cols_usize * rows_usize, RenderCell::default());

        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let colors = content.colors;

        // Fill cells row-major from the display_iter. The iterator yields
        // the visible screen (display_offset rows from the top of the
        // scrollback) plus one extra starting point; we just place each
        // cell at its normalized viewport row.
        for indexed in content.display_iter {
            let Point { line, column } = indexed.point;
            let viewport_row = line.0 + display_offset as i32;
            if viewport_row < 0 || viewport_row >= rows as i32 {
                continue;
            }
            let col = column.0;
            if col >= cols_usize {
                continue;
            }
            let idx = viewport_row as usize * cols_usize + col;
            let cell = indexed.cell;
            let flags = convert_flags(cell.flags);
            let (fg, bg) = resolve_colors(cell.fg, cell.bg, cell.flags, colors, theme);
            dst.cells[idx] = RenderCell {
                ch: cell.c,
                fg,
                bg,
                flags,
            };
        }

        // Cursor.
        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + display_offset as i32;
        if cursor_row >= 0 && cursor_row < rows as i32 {
            let col = cursor_point.column.0;
            if col < cols_usize {
                dst.cursor = Some((col as u16, cursor_row as u16));
            }
        }
        dst.cursor_style = CursorStyle::Block;
    }

    /// Current `Term` grid size for this pty, if known.
    #[must_use]
    pub fn grid_size(&self, pty: PtyId) -> Option<(u16, u16)> {
        let handle = self.ptys.get(&pty)?;
        let term = handle.term.lock();
        Some((
            term.columns().min(u16::MAX as usize) as u16,
            term.screen_lines().min(u16::MAX as usize) as u16,
        ))
    }

    /// Window-size struct for handing to `OnResize` consumers; used only
    /// by the app layer when it wants to tell the shell about pixel
    /// dimensions alongside the character grid.
    #[must_use]
    pub fn window_size(size: PtySize) -> WindowSize {
        WindowSize {
            num_lines: size.rows,
            num_cols: size.cols,
            cell_width: size.pixel_width / size.cols.max(1),
            cell_height: size.pixel_height / size.rows.max(1),
        }
    }
}

fn spawn_reader_thread(
    tile_id: TileId,
    mut reader: Box<dyn Read + Send>,
    sink: Arc<dyn PtyEventSink>,
    term: Arc<FairMutex<Term<EventProxy>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut processor: AnsiProcessor<StdSyncHandler> = AnsiProcessor::new();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    sink.emit(PtyEvent::ProcessExited { tile_id });
                    break;
                }
                Ok(n) => {
                    {
                        let mut guard = term.lock();
                        processor.advance(&mut *guard, &buf[..n]);
                    }
                    sink.emit(PtyEvent::OutputReceived { tile_id });
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    sink.emit(PtyEvent::ProcessExited { tile_id });
                    break;
                }
            }
        }
    })
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "/bin/sh".to_string()
        }
    })
}

fn convert_flags(flags: CellFlagsAlac) -> CellFlags {
    let mut out = CellFlags::empty();
    if flags.contains(CellFlagsAlac::BOLD) {
        out |= CellFlags::BOLD;
    }
    if flags.contains(CellFlagsAlac::ITALIC) {
        out |= CellFlags::ITALIC;
    }
    if flags.intersects(CellFlagsAlac::ALL_UNDERLINES) {
        out |= CellFlags::UNDERLINE;
    }
    if flags.contains(CellFlagsAlac::INVERSE) {
        out |= CellFlags::INVERSE;
    }
    if flags.contains(CellFlagsAlac::STRIKEOUT) {
        out |= CellFlags::STRIKE;
    }
    if flags.contains(CellFlagsAlac::WIDE_CHAR) {
        out |= CellFlags::WIDE_CHAR;
    }
    if flags.contains(CellFlagsAlac::HIDDEN) {
        out |= CellFlags::HIDDEN;
    }
    out
}

fn resolve_colors(
    fg: Color,
    bg: Color,
    flags: CellFlagsAlac,
    colors: &alacritty_terminal::term::color::Colors,
    theme: &Theme,
) -> (Rgba, Rgba) {
    let mut fg_rgba = resolve_color(fg, colors, theme, true);
    let mut bg_rgba = resolve_color(bg, colors, theme, false);
    if flags.contains(CellFlagsAlac::INVERSE) {
        std::mem::swap(&mut fg_rgba, &mut bg_rgba);
    }
    (fg_rgba, bg_rgba)
}

fn resolve_color(
    c: Color,
    colors: &alacritty_terminal::term::color::Colors,
    theme: &Theme,
    is_fg: bool,
) -> Rgba {
    match c {
        Color::Spec(rgb) => rgb_to_rgba(rgb),
        Color::Named(named) => {
            if let Some(rgb) = colors[named] {
                return rgb_to_rgba(rgb);
            }
            named_to_theme(named, theme, is_fg)
        }
        Color::Indexed(idx) => {
            if let Some(rgb) = colors[idx as usize] {
                return rgb_to_rgba(rgb);
            }
            if (idx as usize) < 16 {
                theme.ansi[idx as usize]
            } else if is_fg {
                theme.foreground
            } else {
                theme.background
            }
        }
    }
}

fn named_to_theme(named: NamedColor, theme: &Theme, is_fg: bool) -> Rgba {
    match named {
        NamedColor::Foreground | NamedColor::DimForeground | NamedColor::BrightForeground => {
            theme.foreground
        }
        NamedColor::Background => theme.background,
        NamedColor::Cursor => theme.cursor,
        NamedColor::Black => theme.ansi[0],
        NamedColor::Red => theme.ansi[1],
        NamedColor::Green => theme.ansi[2],
        NamedColor::Yellow => theme.ansi[3],
        NamedColor::Blue => theme.ansi[4],
        NamedColor::Magenta => theme.ansi[5],
        NamedColor::Cyan => theme.ansi[6],
        NamedColor::White => theme.ansi[7],
        NamedColor::BrightBlack | NamedColor::DimBlack => theme.ansi[8],
        NamedColor::BrightRed | NamedColor::DimRed => theme.ansi[9],
        NamedColor::BrightGreen | NamedColor::DimGreen => theme.ansi[10],
        NamedColor::BrightYellow | NamedColor::DimYellow => theme.ansi[11],
        NamedColor::BrightBlue | NamedColor::DimBlue => theme.ansi[12],
        NamedColor::BrightMagenta | NamedColor::DimMagenta => theme.ansi[13],
        NamedColor::BrightCyan | NamedColor::DimCyan => theme.ansi[14],
        NamedColor::BrightWhite | NamedColor::DimWhite => theme.ansi[15],
    }
    .into_fallback_or(if is_fg {
        theme.foreground
    } else {
        theme.background
    })
}

fn rgb_to_rgba(rgb: Rgb) -> Rgba {
    Rgba::rgb(rgb.r, rgb.g, rgb.b)
}

// We want to treat the default `Rgba::default()` (all-zeros) as "not set"
// for fallback purposes. This tiny trait lets the match above stay
// readable.
trait RgbaFallback {
    fn into_fallback_or(self, fallback: Rgba) -> Rgba;
}

impl RgbaFallback for Rgba {
    fn into_fallback_or(self, fallback: Rgba) -> Rgba {
        if self.a == 0 {
            fallback
        } else {
            self
        }
    }
}
