//! Kookaburra binary entrypoint.
//!
//! Phase 1 + a touch of Phase 2: single window, 3×2 grid of tiles,
//! keyboard input goes to the focused tile, resize propagates through
//! renderer → PTY → `Term`. Strip, workspaces, drag-to-move and proper
//! focus indicators come in later phases.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use kookaburra_core::action::{apply_action, Action, PtySideEffects};
use kookaburra_core::config::Config;
use kookaburra_core::ids::{PtyId, TileId};
use kookaburra_core::layout::{compute_tile_rects, Layout, Rect};
use kookaburra_core::snapshot::TileSnapshot;
use kookaburra_core::state::AppState;
use kookaburra_core::worktree::WorktreeConfig;

use kookaburra_pty::{PtyEvent, PtyEventSink, PtyManager, SpawnRequest};
use kookaburra_render::{cells_in_rect, RenderTile, Renderer};
use kookaburra_ui::UiLayer;

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
    ui: UiLayer,
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
            ui: UiLayer::new(),
            actions: Vec::with_capacity(16),
            render_scratch: Vec::new(),
            dirty_tiles: HashSet::new(),
            last_rendered_focus: None,
            modifiers: ModifiersState::empty(),
            cursor_pos: PhysicalPosition::new(0.0, 0.0),
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

    /// Compute the layout of tile rects inside the current window, using
    /// the active workspace's layout.
    fn tile_rects(&self, layout: Layout) -> Vec<Rect> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Vec::new();
        };
        let (win_w, win_h) = renderer.size();
        let area = Rect {
            x: WINDOW_INSET_PX,
            y: WINDOW_INSET_PX,
            width: (win_w as f32 - 2.0 * WINDOW_INSET_PX).max(1.0),
            height: (win_h as f32 - 2.0 * WINDOW_INSET_PX).max(1.0),
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

    /// Pick a PTY size for a tile occupying `rect`.
    fn pty_size_for_rect(&self, rect: Rect) -> PtySize {
        let metrics = self
            .renderer
            .as_ref()
            .map(|r| r.metrics)
            .unwrap_or_else(|| kookaburra_render::CellMetrics::fallback(self.config.font.size_px));
        let (cols, rows) = cells_in_rect(rect, metrics);
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

    fn render_if_needed(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let (win_w, win_h) = renderer.size();
        let area = Rect {
            x: WINDOW_INSET_PX,
            y: WINDOW_INSET_PX,
            width: (win_w as f32 - 2.0 * WINDOW_INSET_PX).max(1.0),
            height: (win_h as f32 - 2.0 * WINDOW_INSET_PX).max(1.0),
        };
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
                self.render_scratch.push(RenderTile {
                    tile_id: tile.id,
                    rect: area,
                    focused: true,
                    update,
                });
            }
        } else {
            let layout = self.state.active_workspace().layout;
            let rects = compute_tile_rects(layout, area);
            let tiles: Vec<(TileId, PtyId, String)> = self
                .state
                .active_workspace()
                .tiles
                .iter()
                .map(|t| (t.id, t.pty_id, t.title.clone()))
                .collect();
            for (i, (tile_id, pty_id, title)) in tiles.iter().enumerate() {
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
                    update,
                });
            }
        }

        renderer.render_frame(&self.render_scratch);
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

    fn handle_mouse_click(&mut self, phys_x: f64, phys_y: f64) {
        if let Some(tile_id) = self.tile_at(phys_x as f32, phys_y as f32) {
            self.actions.push(Action::FocusTile(tile_id));
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
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.ensure_starter_tiles();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
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
                self.handle_key(&key);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = position;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                self.handle_mouse_click(self.cursor_pos.x, self.cursor_pos.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_wheel(delta);
            }
            WindowEvent::RedrawRequested => {
                self.render_if_needed();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: PtyEvent) {
        self.apply_pty_event(event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.ui.draw_strip(&self.state, &mut self.actions);
        if !self.actions.is_empty() {
            let pending: Vec<Action> = self.actions.drain(..).collect();
            let default_size = self
                .window
                .as_ref()
                .map(|_| {
                    let layout = self.state.active_workspace().layout;
                    let rects = self.tile_rects(layout);
                    rects
                        .first()
                        .copied()
                        .map(|r| self.pty_size_for_rect(r))
                        .unwrap_or(PtySize {
                            rows: 24,
                            cols: 80,
                            pixel_width: 0,
                            pixel_height: 0,
                        })
                })
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
            // Layout / zen / new-tile may have changed per-tile rects.
            self.resize_all_ptys();
            // After any action we don't know precisely which tiles are
            // stale — SetLayout/CreateTile/CloseTile all reshuffle things.
            // Mark everything currently visible dirty; reshapes are cheap
            // now that they're content-hash guarded in the renderer.
            for tile in &self.state.active_workspace().tiles {
                self.dirty_tiles.insert(tile.id);
            }
        }
        // Render INLINE here rather than calling `request_redraw()` and
        // waiting for winit to deliver `RedrawRequested` on the next loop
        // turn. That round-trip was adding a full event-loop wakeup of
        // latency per keystroke — user_event set the flag, about_to_wait
        // asked winit to schedule a redraw, winit then woke the loop a
        // second time to deliver RedrawRequested, and only then did we
        // render. Rendering here collapses the round-trip.
        if self.state.needs_redraw && self.renderer.is_some() {
            self.render_if_needed();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
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
