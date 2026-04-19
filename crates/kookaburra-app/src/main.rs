//! Kookaburra binary entrypoint.
//!
//! Phase 1 + a touch of Phase 2: single window, 3×2 grid of tiles,
//! keyboard input goes to the focused tile, resize propagates through
//! renderer → PTY → `Term`. Strip, workspaces, drag-to-move and proper
//! focus indicators come in later phases.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kookaburra_core::action::{apply_action, Action, PtySideEffects};
use kookaburra_core::config::Config;
use kookaburra_core::ids::{PtyId, TileId};
use kookaburra_core::layout::{compute_tile_rects, Layout, Rect};
use kookaburra_core::snapshot::TileSnapshot;
use kookaburra_core::state::AppState;
use kookaburra_core::worktree::WorktreeConfig;

use kookaburra_pty::{PtyEvent, PtyEventSink, PtyManager, SpawnRequest};
use kookaburra_render::{cells_in_rect, RenderTile, Renderer, UiFrame};
use kookaburra_ui::{EventResponse, PreparedFrame, TileDragGhost, UiLayer, STRIP_HEIGHT, STATUS_BAR_HEIGHT};

use portable_pty::PtySize;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

const DEFAULT_WIDTH: u32 = 1400;
const DEFAULT_HEIGHT: u32 = 900;
const STARTER_TILES: usize = 6;
const TILE_GAP_PX: f32 = 6.0;
const WINDOW_INSET_PX: f32 = 8.0;
/// Pointer-drag threshold in physical pixels. Below this, a press/release
/// is treated as a plain click (focus); above it, the gesture is promoted
/// to a tile drag-to-card. Matches egui's own default threshold.
const DRAG_THRESHOLD_PX: f64 = 6.0;

/// A pending left-press on a tile. We defer the focus-vs-drag decision
/// until the user either releases the button (→ focus) or moves past the
/// threshold (→ drag).
#[derive(Copy, Clone, Debug)]
struct PressPending {
    tile_id: TileId,
    phys_pos: PhysicalPosition<f64>,
}

/// Sink the PTY reader threads emit into. Each call pushes the event onto
/// the winit event loop, which wakes `ApplicationHandler::user_event`
/// immediately. Without this, winit sleeps on `ControlFlow::Wait` and
/// shell output sits un-rendered until the user presses another key,
/// which felt like severe typing lag in the previous revision.
struct WinitSink(EventLoopProxy<PtyEvent>);

impl PtyEventSink for WinitSink {
    fn emit(&self, event: PtyEvent) {
        // Send-event can fail once the event loop has exited. We silently
        // drop in that case — the PTY reader will notice next time the
        // process closes.
        let _ = self.0.send_event(event);
    }
}

/// Adapter so `kookaburra-core::apply_action` can ask our `PtyManager`
/// to spawn and close PTYs without `core` depending on `pty`.
struct PtyAdapter<'a> {
    manager: &'a mut PtyManager,
    default_size: PtySize,
}

impl<'a> PtySideEffects for PtyAdapter<'a> {
    fn spawn(&mut self, tile_id: TileId, _worktree: Option<&WorktreeConfig>) -> PtyId {
        let req = SpawnRequest {
            tile_id,
            size: self.default_size,
            ..SpawnRequest::default()
        };
        match self.manager.spawn(req) {
            Ok(id) => id,
            Err(e) => {
                log::error!("pty spawn failed: {e}");
                PtyId::new()
            }
        }
    }

    fn close(&mut self, pty: PtyId) {
        self.manager.close(pty);
    }
}

struct App {
    config: Config,
    state: AppState,
    pty_manager: PtyManager,
    ui: Option<UiLayer>,
    actions: Vec<Action>,
    render_scratch: Vec<RenderTile>,
    /// Tiles that need a fresh snapshot on the next frame. Tiles NOT in
    /// this set reuse their cached glyphon buffer — which is the whole
    /// point: snapshotting + re-shaping every tile every frame was the
    /// typing-lag culprit. Populated on PTY output, tile creation, focus
    /// change, window resize; cleared after a successful render.
    dirty_tiles: HashSet<TileId>,
    /// Last focused tile we rendered. When focus changes the previously
    /// focused and newly focused tiles both need a reshape so dimming
    /// updates.
    last_rendered_focus: Option<TileId>,
    modifiers: ModifiersState,
    cursor_pos: PhysicalPosition<f64>,
    /// When `Some`, the user is actively dragging a tile (either via a
    /// Cmd+click or via a plain-click that crossed `DRAG_THRESHOLD_PX`).
    /// On release the pointer is tested against workspace cards / strip
    /// area to decide the drop target.
    dragging_tile: Option<TileId>,
    /// Plain left-press on a tile, waiting to find out whether the user
    /// means to focus (quick release) or drag (cursor moves far enough).
    press_pending: Option<PressPending>,
    clipboard: Option<arboard::Clipboard>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    last_frame: Instant,
    starter_spawned: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<PtyEvent>) -> Self {
        let config = Config::load_or_default();
        let state = AppState::new(config.clone());
        let sink: Arc<dyn PtyEventSink> = Arc::new(WinitSink(proxy));
        Self {
            config,
            state,
            pty_manager: PtyManager::new(sink),
            ui: None,
            actions: Vec::with_capacity(16),
            render_scratch: Vec::new(),
            dirty_tiles: HashSet::new(),
            last_rendered_focus: None,
            modifiers: ModifiersState::empty(),
            cursor_pos: PhysicalPosition::new(0.0, 0.0),
            dragging_tile: None,
            press_pending: None,
            // `arboard::Clipboard::new()` can fail in headless environments
            // (e.g. CI without a display). Degrade to "no paste support"
            // rather than panicking the app.
            clipboard: arboard::Clipboard::new().ok(),
            window: None,
            renderer: None,
            last_frame: Instant::now(),
            starter_spawned: false,
        }
    }

    fn active_tile(&self) -> Option<TileId> {
        self.state
            .focused_tile
            .or_else(|| self.state.active_workspace().tiles.first().map(|t| t.id))
    }

    fn active_pty(&self) -> Option<PtyId> {
        let tile_id = self.active_tile()?;
        self.state.tile(tile_id).map(|t| t.pty_id)
    }

    /// Physical-pixel rect of the area tiles can occupy this frame.
    /// Prefers egui's measured central rect (what's left after the strip
    /// and status bar panels lay out — this accounts for panel margins,
    /// which a manual `STRIP_HEIGHT` reservation misses). Falls back to
    /// the constants before the first UI frame, or when no renderer is
    /// attached yet.
    fn available_area(&self) -> Option<Rect> {
        let renderer = self.renderer.as_ref()?;
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0);
        if let Some(c) = self.ui.as_ref().and_then(|u| u.central_rect()) {
            return Some(Rect {
                x: c.left() * scale + WINDOW_INSET_PX,
                y: c.top() * scale + WINDOW_INSET_PX,
                width: (c.width() * scale - 2.0 * WINDOW_INSET_PX).max(1.0),
                height: (c.height() * scale - 2.0 * WINDOW_INSET_PX).max(1.0),
            });
        }
        let (win_w, win_h) = renderer.size();
        let strip_px = STRIP_HEIGHT * scale;
        let status_bar_px = STATUS_BAR_HEIGHT * scale;
        Some(Rect {
            x: WINDOW_INSET_PX,
            y: strip_px + WINDOW_INSET_PX,
            width: (win_w as f32 - 2.0 * WINDOW_INSET_PX).max(1.0),
            height: (win_h as f32 - strip_px - status_bar_px - 2.0 * WINDOW_INSET_PX).max(1.0),
        })
    }

    /// Compute the layout of tile rects inside the current window, using
    /// the active workspace's layout.
    fn tile_rects(&self, layout: Layout) -> Vec<Rect> {
        let Some(area) = self.available_area() else {
            return Vec::new();
        };
        let mut rects = compute_tile_rects(layout, area);
        // Shrink each rect by the gap so neighboring tiles are visibly
        // separated by the theme bg.
        for r in &mut rects {
            r.width = (r.width - TILE_GAP_PX).max(1.0);
            r.height = (r.height - TILE_GAP_PX).max(1.0);
        }
        rects
    }

    /// Pick a PTY size for a tile occupying `rect`. Subtracts the tile
    /// header height (22px + 1px separator) from the available area so
    /// terminal rows don't overflow the visible content region.
    fn pty_size_for_rect(&self, rect: Rect) -> PtySize {
        let metrics = self
            .renderer
            .as_ref()
            .map(|r| r.metrics)
            .unwrap_or_else(|| kookaburra_render::CellMetrics::fallback(self.config.font.size_px));
        let tile_header_px = 23.0; // 22px header + 1px separator
        let content_rect = Rect {
            x: rect.x,
            y: rect.y + tile_header_px,
            width: rect.width,
            height: (rect.height - tile_header_px).max(1.0),
        };
        let (cols, rows) = cells_in_rect(content_rect, metrics);
        PtySize {
            rows,
            cols,
            pixel_width: rect.width as u16,
            pixel_height: rect.height as u16,
        }
    }

    /// Spawn the starter tiles (currently 6 to fill a 3×2 grid).
    fn ensure_starter_tiles(&mut self) {
        if self.starter_spawned {
            return;
        }
        let ws = self.state.active_workspace;
        let layout = self
            .state
            .workspace(ws)
            .map(|w| w.layout)
            .unwrap_or(Layout::Grid { cols: 3, rows: 2 });
        let rects = self.tile_rects(layout);
        for i in 0..STARTER_TILES {
            let size = rects
                .get(i)
                .copied()
                .map(|r| self.pty_size_for_rect(r))
                .unwrap_or(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            let mut adapter = PtyAdapter {
                manager: &mut self.pty_manager,
                default_size: size,
            };
            apply_action(
                &mut self.state,
                &mut adapter,
                Action::CreateTile {
                    workspace: ws,
                    worktree: None,
                },
            );
        }
        // Focus the first tile.
        let first = self.state.active_workspace().tiles.first().map(|t| t.id);
        self.state.focused_tile = first;
        // Every starter tile needs its initial shaping pass.
        for tile in &self.state.active_workspace().tiles {
            self.dirty_tiles.insert(tile.id);
        }
        self.starter_spawned = true;
    }

    /// Handle a single PTY event delivered via the winit event-loop proxy.
    fn apply_pty_event(&mut self, event: PtyEvent) {
        match event {
            PtyEvent::OutputReceived { tile_id } => {
                if let Some(tile) = self.state.tile_mut(tile_id) {
                    tile.has_new_output = true;
                    tile.last_output_at = Some(Instant::now());
                }
                self.dirty_tiles.insert(tile_id);
                self.state.request_redraw();
            }
            PtyEvent::ProcessExited { tile_id } => {
                log::info!("pty for tile {tile_id} exited");
            }
            PtyEvent::TitleChanged { tile_id, title } => {
                if let Some(tile) = self.state.tile_mut(tile_id) {
                    tile.title = title;
                }
            }
            PtyEvent::BellRang { tile_id } => {
                if let Some(tile) = self.state.tile_mut(tile_id) {
                    tile.bell_pending = true;
                }
            }
        }
    }

    fn handle_key(&mut self, ev: &KeyEvent) {
        if ev.state != ElementState::Pressed {
            return;
        }
        let Some(pty_id) = self.active_pty() else {
            return;
        };
        let Some(bytes) = key_event_to_bytes(ev, self.modifiers) else {
            return;
        };
        if bytes.is_empty() {
            return;
        }
        if let Err(e) = self.pty_manager.write(pty_id, &bytes) {
            log::warn!("pty write failed: {e}");
        }
        // Typing should snap the viewport back to live output — otherwise
        // a user who scrolled back up stops seeing what they type.
        if self.pty_manager.scroll_to_bottom(pty_id) {
            if let Some(tile_id) = self.active_tile() {
                self.dirty_tiles.insert(tile_id);
            }
            self.state.request_redraw();
        }
        if let Some(tile_id) = self.active_tile() {
            if let Some(tile) = self.state.tile_mut(tile_id) {
                tile.has_new_output = false;
            }
        }
    }

    /// Wheel event → scroll the PTY under the cursor. Natural-scroll-style:
    /// wheel up reveals history. One line per notch (winit reports either
    /// line deltas or pixel deltas depending on device).
    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(tile_id) = self.tile_at(self.cursor_pos.x as f32, self.cursor_pos.y as f32) else {
            return;
        };
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
            MouseScrollDelta::PixelDelta(p) => (p.y / 16.0).round() as i32,
        };
        if lines == 0 {
            return;
        }
        let Some(pty_id) = self.state.tile(tile_id).map(|t| t.pty_id) else {
            return;
        };
        if self.pty_manager.scroll(pty_id, lines) {
            self.dirty_tiles.insert(tile_id);
            self.state.request_redraw();
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Copy the focused tile's currently-visible grid as plain text.
    fn copy_visible(&mut self) {
        let Some(pty_id) = self.active_pty() else {
            return;
        };
        let text = self.pty_manager.visible_text(pty_id);
        if text.is_empty() {
            return;
        }
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        if let Err(e) = clipboard.set_text(text) {
            log::warn!("clipboard set_text failed: {e}");
        }
    }

    fn render_if_needed(&mut self, ui_frame: Option<PreparedFrame>) {
        let Some(area) = self.available_area() else {
            return;
        };
        if self.renderer.is_none() {
            return;
        }
        self.render_scratch.clear();
        let theme = self.config.theme.clone();
        let focused = self.state.focused_tile;

        // When focus changes the previously-focused and newly-focused
        // tiles both need a reshape (the dim/bright color mix is baked
        // into the glyphon buffer). Mark them dirty so the per-tile
        // update loop below picks them up.
        if self.last_rendered_focus != focused {
            if let Some(old) = self.last_rendered_focus {
                self.dirty_tiles.insert(old);
            }
            if let Some(new) = focused {
                self.dirty_tiles.insert(new);
            }
        }

        if self.state.zen_mode {
            // Zen mode: render only the focused tile, full-area.
            let Some(tile_id) = focused else {
                return;
            };
            let tile = self.state.tile(tile_id).cloned();
            if let Some(tile) = tile {
                let update = if self.dirty_tiles.contains(&tile.id) {
                    let mut snap = TileSnapshot::new(tile.id);
                    self.pty_manager.snapshot(tile.pty_id, &theme, &mut snap);
                    snap.title = tile.title.clone();
                    Some(snap)
                } else {
                    None
                };
                let zen_generating = tile.last_output_at
                    .map(|at| std::time::Instant::now().saturating_duration_since(at) < std::time::Duration::from_millis(600))
                    .unwrap_or(false);
                let is_primary = self.state.active_workspace().primary_tile == Some(tile.id);
                self.render_scratch.push(RenderTile {
                    tile_id: tile.id,
                    rect: area,
                    focused: true,
                    generating: zen_generating,
                    primary: is_primary,
                    follow_mode: tile.follow_mode,
                    tile_index: 1,
                    update,
                });
            }
        } else {
            let layout = self.state.active_workspace().layout;
            let rects = compute_tile_rects(layout, area);
            let gen_window = std::time::Duration::from_millis(600);
            let now = std::time::Instant::now();
            let primary_tile = self.state.active_workspace().primary_tile;
            let tiles: Vec<(TileId, PtyId, String, bool, bool, bool)> = self
                .state
                .active_workspace()
                .tiles
                .iter()
                .map(|t| {
                    let generating = t.last_output_at
                        .map(|at| now.saturating_duration_since(at) < gen_window)
                        .unwrap_or(false);
                    let is_primary = primary_tile == Some(t.id);
                    (t.id, t.pty_id, t.title.clone(), generating, is_primary, t.follow_mode)
                })
                .collect();
            for (i, (tile_id, pty_id, title, tile_generating, tile_primary, tile_follow)) in tiles.iter().enumerate() {
                let Some(r) = rects.get(i).copied() else {
                    break;
                };
                let tile_rect = Rect {
                    x: r.x,
                    y: r.y,
                    width: (r.width - TILE_GAP_PX).max(1.0),
                    height: (r.height - TILE_GAP_PX).max(1.0),
                };
                let update = if self.dirty_tiles.contains(tile_id) {
                    let mut snap = TileSnapshot::new(*tile_id);
                    self.pty_manager.snapshot(*pty_id, &theme, &mut snap);
                    snap.title = title.clone();
                    Some(snap)
                } else {
                    None
                };
                self.render_scratch.push(RenderTile {
                    tile_id: *tile_id,
                    rect: tile_rect,
                    focused: focused == Some(*tile_id),
                    generating: *tile_generating,
                    primary: *tile_primary,
                    follow_mode: *tile_follow,
                    tile_index: i + 1,
                    update,
                });
            }
        }

        let ui_frame = ui_frame.map(|p| UiFrame {
            primitives: p.primitives,
            textures_delta: p.textures_delta,
            pixels_per_point: p.pixels_per_point,
        });
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.render_frame(&self.render_scratch, ui_frame.as_ref());
        }
        self.last_frame = Instant::now();
        self.last_rendered_focus = focused;
        self.dirty_tiles.clear();
        self.state.mark_redrawn();
    }

    /// Walk the layout and return the tile whose rect contains (x, y).
    fn tile_at(&self, x: f32, y: f32) -> Option<TileId> {
        let layout = self.state.active_workspace().layout;
        let rects = self.tile_rects(layout);
        let tiles: Vec<TileId> = self
            .state
            .active_workspace()
            .tiles
            .iter()
            .map(|t| t.id)
            .collect();
        for (i, r) in rects.iter().enumerate() {
            if x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height {
                return tiles.get(i).copied();
            }
        }
        None
    }

    /// Read the system clipboard and write its contents to the focused
    /// PTY. Bracketed-paste escapes are added so interactive programs
    /// (vim, zsh) can distinguish paste from keystrokes.
    fn paste_from_clipboard(&mut self) {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        let text = match clipboard.get_text() {
            Ok(t) => t,
            Err(e) => {
                log::warn!("clipboard read failed: {e}");
                return;
            }
        };
        let Some(pty_id) = self.active_pty() else {
            return;
        };
        // Bracketed paste: ESC [ 200 ~ <text> ESC [ 201 ~
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        if let Err(e) = self.pty_manager.write(pty_id, &bytes) {
            log::warn!("paste write failed: {e}");
        }
    }

    /// Begin a Cmd+drag from a tile. Records the tile under the pointer;
    /// `end_tile_drag` checks the drop target on release.
    fn begin_tile_drag(&mut self, phys_x: f64, phys_y: f64) {
        if let Some(tile_id) = self.tile_at(phys_x as f32, phys_y as f32) {
            self.dragging_tile = Some(tile_id);
        }
    }

    /// Finish a tile drag. If the pointer landed on a workspace card,
    /// emit `Action::MoveTile` — the spec's "signature" drag-tile-to-card
    /// interaction. A drop onto empty strip (strip area, no card hit)
    /// spins the tile out into its own new workspace. Otherwise no-op.
    fn end_tile_drag(&mut self) {
        let Some(tile_id) = self.dragging_tile.take() else {
            return;
        };
        let Some(ui) = self.ui.as_ref() else {
            return;
        };
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0);
        // `card_at` expects egui logical pixels; winit gives us physical.
        let logical_x = self.cursor_pos.x as f32 / scale;
        let logical_y = self.cursor_pos.y as f32 / scale;
        if let Some(target_workspace) = ui.card_at(logical_x, logical_y) {
            // Don't bother if the drop target already owns this tile.
            let same_workspace =
                self.state.workspaces.iter().any(|ws| {
                    ws.id == target_workspace && ws.tiles.iter().any(|t| t.id == tile_id)
                });
            if !same_workspace {
                self.actions.push(Action::MoveTile {
                    tile_id,
                    target_workspace,
                });
            }
            return;
        }
        // Dropped inside the strip area but not on a card → spin out.
        // Use egui's measured central rect so the hit zone matches the
        // actual strip (including its margin), not just the nominal height.
        let top_area = ui
            .central_rect()
            .map(|c| c.top())
            .unwrap_or(STRIP_HEIGHT);
        if (0.0..=top_area).contains(&logical_y) {
            self.actions
                .push(Action::MoveTileToNewWorkspace { tile_id });
        }
    }

    /// Cmd+Opt+1..6 = focus N-th tile, Cmd+Enter = zen, Cmd+T = new tile,
    /// Cmd+W = close focused, Cmd+G = cycle through preset layouts.
    /// Returns `true` when the event was consumed as a shortcut.
    fn handle_app_shortcut(&mut self, ev: &KeyEvent) -> bool {
        if ev.state != ElementState::Pressed {
            return false;
        }
        let cmd = self.modifiers.super_key();
        if !cmd {
            return false;
        }
        let alt = self.modifiers.alt_key();

        // Cmd+Enter: zen mode.
        if !alt {
            if let Key::Named(NamedKey::Enter) = &ev.logical_key {
                self.actions.push(Action::ToggleZenMode);
                return true;
            }
        }

        // Character-based shortcuts. `logical_key` is the key that was
        // pressed regardless of IME / dead-key folding; `text` may be
        // absent (Cmd suppresses .text on macOS for many combos).
        let key_char: Option<char> = match &ev.logical_key {
            Key::Character(s) => s.chars().next(),
            _ => None,
        };
        let Some(kc) = key_char else {
            return false;
        };
        let lower = kc.to_ascii_lowercase();

        if alt {
            // Cmd+Opt+1..6 — focus N-th tile.
            if let Some(d) = lower.to_digit(10) {
                if d == 0 {
                    return false;
                }
                let idx = (d as usize) - 1;
                if let Some(tile) = self.state.active_workspace().tiles.get(idx) {
                    self.actions.push(Action::FocusTile(tile.id));
                    return true;
                }
            }
            return false;
        }

        // Cmd+1..9 — switch to N-th workspace.
        if let Some(d) = lower.to_digit(10) {
            if d >= 1 {
                let idx = (d as usize) - 1;
                if let Some(ws) = self.state.workspaces.get(idx) {
                    self.actions.push(Action::SwitchWorkspace(ws.id));
                    return true;
                }
            }
        }

        match lower {
            't' => {
                let ws = self.state.active_workspace;
                self.actions.push(Action::CreateTile {
                    workspace: ws,
                    worktree: None,
                });
                true
            }
            'w' => {
                if let Some(tile_id) = self.state.focused_tile {
                    self.actions.push(Action::CloseTile(tile_id));
                }
                true
            }
            'v' => {
                self.paste_from_clipboard();
                true
            }
            'c' => {
                self.copy_visible();
                true
            }
            'n' => {
                self.actions.push(Action::CreateWorkspace);
                true
            }
            'l' => {
                // Open inline rename for the active workspace's card.
                let ws_id = self.state.active_workspace;
                let label = self.state.active_workspace().label.clone();
                if let Some(ui) = self.ui.as_mut() {
                    ui.start_rename(ws_id, label);
                }
                self.state.request_redraw();
                true
            }
            'g' => {
                // Cycle presets: 1x1 → 2x1 → 2x2 → 3x2 → 1x1.
                let ws_id = self.state.active_workspace;
                let next = match self.state.active_workspace().layout {
                    Layout::Grid { cols: 1, rows: 1 } => Layout::Grid { cols: 2, rows: 1 },
                    Layout::Grid { cols: 2, rows: 1 } => Layout::Grid { cols: 2, rows: 2 },
                    Layout::Grid { cols: 2, rows: 2 } => Layout::Grid { cols: 3, rows: 2 },
                    _ => Layout::Grid { cols: 1, rows: 1 },
                };
                self.actions.push(Action::SetLayout {
                    workspace: ws_id,
                    layout: next,
                });
                true
            }
            _ => false,
        }
    }

    /// Build one egui frame (strip widgets + tessellated geometry). Emits
    /// any user interactions (card clicks, `+` presses) into
    /// `self.actions` so the next `about_to_wait` tick applies them.
    fn build_ui_frame(&mut self) -> Option<PreparedFrame> {
        let window = self.window.as_ref()?.clone();
        // Compute the drag-ghost payload before mutably borrowing `ui` —
        // resolving the tile title needs a shared borrow of `self.state`.
        let ghost = self.dragging_tile.and_then(|tid| {
            self.state.tile(tid).map(|t| {
                let label = if t.title.is_empty() {
                    format!("Tile {}", tid)
                } else {
                    t.title.clone()
                };
                TileDragGhost { label }
            })
        });
        let ui = self.ui.as_mut()?;
        ui.set_tile_drag_ghost(ghost);
        Some(ui.run_frame(&window, &self.state, &mut self.actions))
    }

    /// Drain `self.actions` through `apply_action`. Marks every
    /// currently-visible tile dirty afterwards — most actions reshuffle
    /// rects (SetLayout, CreateTile, CloseTile, SwitchWorkspace), and
    /// recomputing per-tile dirtiness is cheaper than tracking which
    /// action affects which tile.
    fn apply_pending_actions(&mut self) {
        if self.actions.is_empty() {
            return;
        }
        let pending: Vec<Action> = self.actions.drain(..).collect();
        let layout = self.state.active_workspace().layout;
        let rects = self.tile_rects(layout);
        let default_size = rects
            .first()
            .copied()
            .map(|r| self.pty_size_for_rect(r))
            .unwrap_or(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            });
        let mut adapter = PtyAdapter {
            manager: &mut self.pty_manager,
            default_size,
        };
        for action in pending {
            apply_action(&mut self.state, &mut adapter, action);
        }
        self.resize_all_ptys();
        for tile in &self.state.active_workspace().tiles {
            self.dirty_tiles.insert(tile.id);
        }
        self.state.request_redraw();
    }

    /// Resize every live PTY so its rows/cols match the per-tile rect the
    /// renderer will draw it into. Called after a window resize.
    fn resize_all_ptys(&mut self) {
        let layout = self.state.active_workspace().layout;
        let rects = self.tile_rects(layout);
        let tiles: Vec<(usize, PtyId)> = self
            .state
            .active_workspace()
            .tiles
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.pty_id))
            .collect();
        for (i, pid) in tiles {
            let Some(r) = rects.get(i).copied() else {
                continue;
            };
            let size = self.pty_size_for_rect(r);
            if let Err(e) = self.pty_manager.resize(pid, size) {
                log::warn!("pty resize failed: {e}");
            }
        }
    }
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window_attrs = Window::default_attributes()
            .with_title("Kookaburra")
            .with_inner_size(LogicalSize::new(
                DEFAULT_WIDTH as f64,
                DEFAULT_HEIGHT as f64,
            ));
        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("create window"),
        );

        let renderer = Renderer::new(
            window.clone(),
            self.config.theme.clone(),
            self.config.font.size_px,
        );
        let ui = UiLayer::new(&window);
        self.renderer = Some(renderer);
        self.ui = Some(ui);
        self.window = Some(window);
        self.ensure_starter_tiles();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Give egui first crack at every event — it needs hover and
        // pointer state even for events it doesn't ultimately "consume".
        // Application-level shortcuts (Cmd+*) and app lifecycle events
        // still run regardless; terminal pointer/text input is gated
        // below.
        let ui_response = match (self.ui.as_mut(), self.window.as_ref()) {
            (Some(ui), Some(window)) => ui.on_window_event(window, &event),
            _ => EventResponse {
                consumed: false,
                repaint: false,
            },
        };
        if ui_response.repaint {
            self.state.request_redraw();
        }
        let ui_consumed = ui_response.consumed;
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize((size.width, size.height));
                }
                self.resize_all_ptys();
                // Grid dimensions changed — every tile needs a fresh
                // snapshot + reshape, not just the ones with new output.
                for tile in &self.state.active_workspace().tiles {
                    self.dirty_tiles.insert(tile.id);
                }
                self.state.request_redraw();
                // No explicit `w.request_redraw()` here — about_to_wait
                // will render inline on needs_redraw, which is the new
                // low-latency path.
            }
            WindowEvent::ModifiersChanged(new_mods) => {
                self.modifiers = new_mods.state();
            }
            WindowEvent::KeyboardInput { event: key, .. } => {
                if self.handle_app_shortcut(&key) {
                    return;
                }
                // A focused egui text widget consumes keystrokes; don't
                // double-dispatch to the terminal.
                let ui_wants_kb = self.ui.as_ref().is_some_and(|u| u.wants_keyboard());
                if !ui_wants_kb {
                    self.handle_key(&key);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = position;
                // Press-pending: if the pointer crossed the drag threshold
                // while held, upgrade to a tile drag so plain left-drag
                // moves a tile to a card.
                if let Some(p) = self.press_pending.as_ref() {
                    let dx = position.x - p.phys_pos.x;
                    let dy = position.y - p.phys_pos.y;
                    if (dx * dx + dy * dy).sqrt() > DRAG_THRESHOLD_PX {
                        self.dragging_tile = Some(p.tile_id);
                        self.press_pending = None;
                        self.state.request_redraw();
                    }
                }
                // While a tile drag is active the ghost pill needs to
                // follow the cursor; force a redraw each move so it
                // tracks smoothly.
                if self.dragging_tile.is_some() {
                    self.state.request_redraw();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            // Left press over a tile:
            //   - Cmd held → start tile drag immediately (legacy path, also
            //     useful when the tile is already focused and the user
            //     wants to drag without a focus flicker).
            //   - Plain → enter "press pending" and defer the focus-vs-drag
            //     decision to the next CursorMoved / Released event. This
            //     lets plain drag-to-card work without a modifier while
            //     still preserving single-click-to-focus.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !ui_consumed => {
                if self.modifiers.super_key() {
                    self.begin_tile_drag(self.cursor_pos.x, self.cursor_pos.y);
                } else if let Some(tile_id) =
                    self.tile_at(self.cursor_pos.x as f32, self.cursor_pos.y as f32)
                {
                    self.press_pending = Some(PressPending {
                        tile_id,
                        phys_pos: self.cursor_pos,
                    });
                }
            }
            // Release: if we were in press-pending and never crossed the
            // drag threshold, this was a plain click → focus the tile.
            // Otherwise the user was dragging — finish the drop.
            // `ui_consumed` is intentionally ignored here: a drop onto a
            // card is precisely when ui_consumed is true.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(p) = self.press_pending.take() {
                    self.actions.push(Action::FocusTile(p.tile_id));
                } else {
                    self.end_tile_drag();
                }
            }
            WindowEvent::MouseWheel { delta, .. } if !ui_consumed => {
                self.handle_mouse_wheel(delta);
            }
            WindowEvent::RedrawRequested => {
                let ui_frame = self.build_ui_frame();
                self.render_if_needed(ui_frame);
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: PtyEvent) {
        self.apply_pty_event(event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Drain any accumulated actions from earlier events (keyboard
        // shortcuts, click-to-focus) BEFORE building the UI frame so the
        // strip reflects the latest workspace state.
        if !self.actions.is_empty() {
            self.apply_pending_actions();
        }

        // Build the egui frame. This also emits any strip interactions
        // (card clicks, `+` presses) into `self.actions`.
        let ui_frame = self.build_ui_frame();

        // Fold those newly-emitted actions in too, so the next render
        // reflects the workspace switch / creation immediately.
        if !self.actions.is_empty() {
            self.apply_pending_actions();
        }

        // If egui has a finite repaint deadline (strip animations, drag
        // ghost, squish), render this frame and schedule the next wake-up.
        // Without this, animations only advance when a PTY / input event
        // happens to wake the loop — hence the stuttery "laggy" feel.
        let animation_delay = ui_frame.as_ref().map(|f| f.repaint_delay);

        // Render INLINE here rather than calling `request_redraw()` and
        // waiting for winit to deliver `RedrawRequested` on the next loop
        // turn. That round-trip was adding a full event-loop wakeup of
        // latency per keystroke — user_event set the flag, about_to_wait
        // asked winit to schedule a redraw, winit then woke the loop a
        // second time to deliver RedrawRequested, and only then did we
        // render. Rendering here collapses the round-trip.
        let animating = animation_delay.is_some_and(|d| d < Duration::MAX);
        if (self.state.needs_redraw || animating) && self.renderer.is_some() {
            self.render_if_needed(ui_frame);
        }

        // Schedule the next wake-up: the soonest of egui's animation
        // deadline (if any) or winit's default `Wait`. Zero delay → Poll
        // so the next frame runs immediately.
        match animation_delay {
            Some(d) if d.is_zero() => event_loop.set_control_flow(ControlFlow::Poll),
            Some(d) if d < Duration::MAX => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + d));
            }
            _ => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn main() {
    env_logger::init();
    log::info!("Kookaburra {} starting", env!("CARGO_PKG_VERSION"));

    let event_loop = EventLoop::<PtyEvent>::with_user_event()
        .build()
        .expect("build event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop exited with error: {e}");
    }
}

/// Convert a winit KeyEvent into bytes to send to the PTY. Returns None
/// if the event produces no PTY input (e.g. pressing a modifier alone).
fn key_event_to_bytes(ev: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    if let Key::Named(named) = &ev.logical_key {
        return Some(named_key_bytes(*named, mods).to_vec());
    }

    if let Some(text) = &ev.text {
        if text.is_empty() {
            return None;
        }
        if mods.control_key() && text.len() == 1 {
            let b = text.as_bytes()[0];
            let folded = if b.is_ascii_alphabetic() {
                Some(b.to_ascii_lowercase() & 0x1f)
            } else {
                match b {
                    b' ' | b'@' => Some(0x00),
                    b'[' => Some(0x1b),
                    b'\\' => Some(0x1c),
                    b']' => Some(0x1d),
                    b'^' => Some(0x1e),
                    b'_' | b'?' => Some(0x1f),
                    _ => None,
                }
            };
            if let Some(c) = folded {
                return Some(vec![c]);
            }
        }
        if mods.alt_key() {
            let mut out = Vec::with_capacity(1 + text.len());
            out.push(0x1b);
            out.extend_from_slice(text.as_bytes());
            return Some(out);
        }
        return Some(text.as_bytes().to_vec());
    }

    None
}

fn named_key_bytes(key: NamedKey, _mods: ModifiersState) -> &'static [u8] {
    match key {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => b"\x7f",
        NamedKey::Tab => b"\t",
        NamedKey::Escape => b"\x1b",
        NamedKey::Space => b" ",
        NamedKey::ArrowUp => b"\x1b[A",
        NamedKey::ArrowDown => b"\x1b[B",
        NamedKey::ArrowRight => b"\x1b[C",
        NamedKey::ArrowLeft => b"\x1b[D",
        NamedKey::Home => b"\x1b[H",
        NamedKey::End => b"\x1b[F",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        NamedKey::Delete => b"\x1b[3~",
        NamedKey::Insert => b"\x1b[2~",
        NamedKey::F1 => b"\x1bOP",
        NamedKey::F2 => b"\x1bOQ",
        NamedKey::F3 => b"\x1bOR",
        NamedKey::F4 => b"\x1bOS",
        NamedKey::F5 => b"\x1b[15~",
        NamedKey::F6 => b"\x1b[17~",
        NamedKey::F7 => b"\x1b[18~",
        NamedKey::F8 => b"\x1b[19~",
        NamedKey::F9 => b"\x1b[20~",
        NamedKey::F10 => b"\x1b[21~",
        NamedKey::F11 => b"\x1b[23~",
        NamedKey::F12 => b"\x1b[24~",
        _ => b"",
    }
}
