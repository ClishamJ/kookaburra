//! UI strip, cards, dialogs, and input routing for Kookaburra.
//!
//! Owns `egui::Context` + `egui_winit::State`. Every frame the main loop
//! calls [`UiLayer::on_window_event`] for each winit event (egui handles
//! its own hit-testing and hover state), then [`UiLayer::run_frame`]
//! which builds the strip widgets, tessellates the shapes, and returns
//! a [`PreparedFrame`] the render crate draws into the shared pass.
//!
//! **Event routing.** `on_window_event` returns an
//! [`egui_winit::EventResponse`]; the main loop should skip terminal
//! input when `consumed == true` AND the focus is actually on egui —
//! egui tries hard not to eat raw keystrokes when no text widget wants
//! them, but hover / click events over the strip do get consumed.

use std::time::{Duration, Instant};

use egui::{Button, Color32, FontId, Frame, RichText, Rounding, Sense, Stroke, Vec2};
pub use egui_winit::EventResponse;
use winit::event::WindowEvent;
use winit::window::Window;

use kookaburra_core::action::Action;
use kookaburra_core::ids::{TileId, WorkspaceId};
use kookaburra_core::layout::{compute_tile_rects, Rect as CoreRect};
use kookaburra_core::state::{AppState, Workspace};

/// Bytes arriving on a tile within this window treat it as "actively
/// streaming" (the "Claude is generating" signal). Longer means the marker
/// lingers after the last chunk; shorter means it flickers during brief
/// pauses between tokens. 600 ms is comfortably above inter-token jitter
/// without keeping the marker lit during purely idle sessions.
const GENERATING_LATENCY_MS: u64 = 600;
/// How often we re-request a frame while an animation is visible. 16 ms ≈
/// 60 fps — matches egui's default scheduler and a 60 Hz display so
/// breathing alpha / moving dots / drag ghosts read smoothly. Still
/// coalesced by egui, so an idle UI with no animations doesn't repaint.
const ANIMATION_TICK: Duration = Duration::from_millis(16);

/// Strip dimensions per spec §3 ("Card dimensions: ~140×48px").
pub const STRIP_HEIGHT: f32 = 56.0;
pub const CARD_WIDTH: f32 = 140.0;
pub const CARD_HEIGHT: f32 = 44.0;
/// Status bar height at bottom of window.
pub const STATUS_BAR_HEIGHT: f32 = 22.0;

/// Kookaburra warm amber palette — OKLCH-derived from
/// `docs/design/Kookaburra/data.js`. Background is near-black with a
/// warm tint; amber (`ACCENT`) is the signature kookaburra highlight.
const STRIP_BG: Color32 = Color32::from_rgb(0x08, 0x06, 0x04);      // bg (near-black)
const BG_DEEP: Color32 = Color32::from_rgb(0x04, 0x03, 0x02);       // bgDeep
const BG_DIM: Color32 = Color32::from_rgb(0x12, 0x0d, 0x09);        // bgDim (active card)
const FG: Color32 = Color32::from_rgb(0xee, 0xeb, 0xe5);            // fg
const FG_DIM: Color32 = Color32::from_rgb(0x9c, 0x98, 0x90);        // fgDim
const FG_FAINT: Color32 = Color32::from_rgb(0x61, 0x5d, 0x56);      // fgFaint
const ACCENT: Color32 = Color32::from_rgb(0xff, 0xa5, 0x1c);        // kookaburra amber
#[allow(dead_code)]
const ACCENT_DEEP: Color32 = Color32::from_rgb(0xc2, 0x56, 0x00);   // darker beak
#[allow(dead_code)]
const TEAL: Color32 = Color32::from_rgb(0x48, 0xb7, 0xbd);          // worktree
const GREEN: Color32 = Color32::from_rgb(0x6e, 0xd2, 0x74);         // activity dot
const GRID_LINE: Color32 = Color32::from_rgb(0x1a, 0x15, 0x10);     // gridLine (very subtle)
/// Fill for empty tile slots: 92% STRIP_BG + 8% FG. Muted but visible so
/// the grid reads as "dormant, click to wake" rather than "empty void".
const EMPTY_SLOT_FILL: Color32 = Color32::from_rgb(0x1a, 0x18, 0x16);

/// Inner padding around each empty-slot overlay in logical pixels. Mirrors
/// the TILE_GAP used by the terminal renderer so empty and live tiles sit
/// on the same visual grid.
const EMPTY_SLOT_GAP: f32 = 6.0;
/// Below this physical-pixel slot height the "click or press ⏎" subtitle
/// overlaps the "+" glyph, so we hide it.
const EMPTY_SLOT_SUBTITLE_MIN_HEIGHT: f32 = 80.0;

/// Routing decision for an input event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputRouting {
    /// egui consumed the event (strip click, dialog interaction, etc.).
    ConsumedByUi,
    /// Forward to the focused tile's PTY.
    ToFocusedTile,
    /// Let the main loop handle it (resize, close, app-level keybinds).
    ToMainLoop,
}

/// One frame's worth of tessellated UI geometry + texture updates. The
/// render crate consumes this during the wgpu pass; the app crate just
/// ferries it.
pub struct PreparedFrame {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
    /// How long egui wants to wait before the next repaint. `Duration::MAX`
    /// means "no animation pending — sleep until the next input event."
    /// Anything shorter means a widget is animating (breathing dots, drag
    /// ghost, etc.) and the app should schedule a wake-up so animations
    /// don't stall between input events.
    pub repaint_delay: Duration,
}

/// Ephemeral state for the inline rename editor. Lives on `UiLayer`
/// rather than `AppState` because it's a pure UI concern — the canonical
/// label only updates when the user commits with Enter.
struct RenameState {
    id: WorkspaceId,
    buffer: String,
    focus_requested: bool,
}

/// Ephemeral state for drag-to-reorder. We only track the source card —
/// the drop index is computed fresh each frame from the pointer position.
struct ReorderState {
    source_idx: usize,
}

/// Ghost preview for a tile-to-card drag. The app layer sets this each
/// frame while a drag is in flight (see `set_tile_drag_ghost`); the strip
/// renders a small pill near the cursor so the user has a visual handle
/// while they aim at a drop target.
#[derive(Clone, Debug)]
pub struct TileDragGhost {
    /// Short label shown inside the ghost pill — usually the tile title,
    /// falling back to "Tile n" if empty.
    pub label: String,
}

/// UI layer state. Holds the egui `Context` and the `egui_winit::State`
/// adapter that converts `WindowEvent`s into egui input.
pub struct UiLayer {
    ctx: egui::Context,
    winit_state: egui_winit::State,
    wants_keyboard: bool,
    wants_pointer: bool,
    renaming: Option<RenameState>,
    reorder: Option<ReorderState>,
    /// Rects (logical pixels) of each workspace card as laid out in the
    /// most recent frame. Used by the app's drag-drop code to hit-test a
    /// tile-drop against the strip. Refreshed every `run_frame`.
    card_rects: Vec<(WorkspaceId, egui::Rect)>,
    /// Ghost pill to paint following the cursor while a tile drag is
    /// active. Set by the app; cleared on drop.
    tile_drag_ghost: Option<TileDragGhost>,
    /// Which workspace index is being "squish"-animated (koo-ws-squish)
    /// and when it started. Cleared after the animation (~340ms) elapses.
    squish: Option<(usize, f64)>,
    /// Last active workspace index, for detecting switches.
    last_ws_idx: usize,
    /// Rect (logical pixels) of the central area left after the strip +
    /// status bar panels render. Captured via `ctx.available_rect()` on
    /// each frame so the app layer can size terminal tiles to exactly
    /// the remaining space — `exact_height` on the panels doesn't account
    /// for `inner_margin`, so using egui's own measurement avoids overlap.
    /// `None` until the first frame has run.
    central_rect: Option<egui::Rect>,
}

impl UiLayer {
    /// Build a fresh UI layer bound to `window`. egui's pixels-per-point
    /// is derived from `zoom_factor × native_pixels_per_point`. We seed
    /// `native_pixels_per_point` from the window's scale factor via
    /// `egui_winit::State::new` and leave `zoom_factor` at its default of
    /// 1.0 — so the effective ppp matches the window scale.
    ///
    /// **Do not** call `ctx.set_pixels_per_point` here. At this point
    /// `ctx.native_pixels_per_point()` is still `None`, so `set_pixels_per_point`
    /// internally computes `zoom_factor = ppp / 1.0 = scale_factor`. Once
    /// the first `take_egui_input` populates `native_pixels_per_point`, the
    /// effective ppp becomes `scale_factor × scale_factor` (4.0 on Retina),
    /// halving every logical coordinate and making `available_rect()` return
    /// a rect half the expected size.
    #[must_use]
    pub fn new(window: &Window) -> Self {
        let ctx = egui::Context::default();
        let pixels_per_point = window.scale_factor() as f32;
        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(pixels_per_point),
            None,
            None,
        );
        Self {
            ctx,
            winit_state,
            wants_keyboard: false,
            wants_pointer: false,
            renaming: None,
            reorder: None,
            card_rects: Vec::new(),
            tile_drag_ghost: None,
            squish: None,
            last_ws_idx: 0,
            central_rect: None,
        }
    }

    /// Logical-pixel rect of the area remaining after the strip and status
    /// bar panels are laid out. Returns `None` before the first frame.
    #[must_use]
    pub fn central_rect(&self) -> Option<egui::Rect> {
        self.central_rect
    }

    /// Called by the app each frame a tile drag is in flight. Pass `None`
    /// on drop / cancel to clear the ghost.
    pub fn set_tile_drag_ghost(&mut self, ghost: Option<TileDragGhost>) {
        self.tile_drag_ghost = ghost;
    }

    /// Forward a winit event to egui. Returns egui's response so the
    /// caller can decide whether to also route it to the terminal.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    /// `scale_factor` here is treated as the effective `pixels_per_point`.
    /// `egui_winit` already tracks native scale via `ScaleFactorChanged`
    /// events, so this should only be called to apply a user zoom on top
    /// of the native scale. It sets ppp directly; egui internally derives
    /// the zoom factor from `ppp / native_ppp`.
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.ctx.set_pixels_per_point(scale_factor);
    }

    /// Open the inline rename editor for a workspace card. Used by the
    /// `Cmd+L` keybinding; double-click is handled inside `build_strip`.
    pub fn start_rename(&mut self, id: WorkspaceId, initial: String) {
        self.renaming = Some(RenameState {
            id,
            buffer: initial,
            focus_requested: false,
        });
    }

    /// Which workspace card (if any) is under the given logical-pixel
    /// point. Returns `None` when the point misses every card — including
    /// when the point is on the strip but between cards or over the `+`
    /// button. Used by the app layer's drag-drop handler so dropping a
    /// tile on a card produces `Action::MoveTile`.
    #[must_use]
    pub fn card_at(&self, logical_x: f32, logical_y: f32) -> Option<WorkspaceId> {
        let p = egui::pos2(logical_x, logical_y);
        self.card_rects
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(id, _)| *id)
    }

    /// Whether an egui text widget currently wants keyboard focus.
    #[must_use]
    pub fn wants_keyboard(&self) -> bool {
        self.wants_keyboard
    }

    /// Whether the cursor is hovering an egui widget.
    #[must_use]
    pub fn wants_pointer(&self) -> bool {
        self.wants_pointer
    }

    /// Decide where a keyboard event goes. Spec §7 priority order.
    #[must_use]
    pub fn route_keyboard(&self) -> InputRouting {
        if self.wants_keyboard {
            InputRouting::ConsumedByUi
        } else {
            InputRouting::ToFocusedTile
        }
    }

    /// Decide where a pointer event goes.
    #[must_use]
    pub fn route_pointer(&self) -> InputRouting {
        if self.wants_pointer {
            InputRouting::ConsumedByUi
        } else {
            InputRouting::ToFocusedTile
        }
    }

    /// Run one egui frame. Builds the strip, captures user interactions
    /// into `actions`, and returns a [`PreparedFrame`] ready for the GPU
    /// pass. Call once per visible frame.
    pub fn run_frame(
        &mut self,
        window: &Window,
        state: &AppState,
        actions: &mut Vec<Action>,
    ) -> PreparedFrame {
        let raw_input = self.winit_state.take_egui_input(window);
        // Clone is cheap (`Context` is `Arc` inside) and avoids the
        // self-borrow conflict between `self.ctx.run` and the closure.
        let ctx = self.ctx.clone();
        let renaming = &mut self.renaming;
        let reorder = &mut self.reorder;
        let card_rects = &mut self.card_rects;
        let tile_drag_ghost = self.tile_drag_ghost.as_ref();
        let now = Instant::now();
        card_rects.clear();

        // Detect workspace switch → trigger squish animation on the new card.
        let current_ws_idx = state.workspaces.iter().position(|ws| ws.id == state.active_workspace).unwrap_or(0);
        if current_ws_idx != self.last_ws_idx {
            let t = ctx.input(|i| i.time);
            self.squish = Some((current_ws_idx, t));
            self.last_ws_idx = current_ws_idx;
        }
        // Expire squish after 340ms.
        if let Some((_, started)) = self.squish {
            let t = ctx.input(|i| i.time);
            if t - started > 0.34 {
                self.squish = None;
            }
        }
        let squish = &self.squish;

        let mut central = None;
        let full_output = ctx.run(raw_input, |ctx| {
            build_strip(
                ctx,
                state,
                actions,
                renaming,
                reorder,
                card_rects,
                tile_drag_ghost,
                now,
                squish,
            );
            build_status_bar(ctx, state, now);
            // `available_rect` only works inside `ctx.run`; capture the
            // central area (what's left after the strip + status bar
            // panels) so the app can size terminals into exactly this
            // space.
            let area = ctx.available_rect();
            central = Some(area);
            // Empty-slot placeholders paint into the central area on top
            // of the cleared-to-black wgpu frame. Only non-live slots get
            // an overlay, so live tiles remain free for the terminal
            // mouse path. Zen mode hides everything but the focused tile.
            if !state.zen_mode {
                paint_empty_slots(ctx, state, actions, area);
            }
        });
        self.central_rect = central;
        self.wants_keyboard = ctx.wants_keyboard_input();
        self.wants_pointer = ctx.wants_pointer_input();
        self.winit_state
            .handle_platform_output(window, full_output.platform_output);
        let pixels_per_point = ctx.pixels_per_point();
        // Pull egui's next-repaint deadline for ROOT. Animations
        // (breathing dots, squish, drag ghost) set this via
        // `ctx.request_repaint_after`; without surfacing it to the event
        // loop, animations stall until the next input event wakes us.
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);
        let primitives = ctx.tessellate(full_output.shapes, pixels_per_point);
        PreparedFrame {
            primitives,
            textures_delta: full_output.textures_delta,
            pixels_per_point,
            repaint_delay,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_strip(
    ctx: &egui::Context,
    state: &AppState,
    actions: &mut Vec<Action>,
    renaming: &mut Option<RenameState>,
    reorder: &mut Option<ReorderState>,
    card_rects: &mut Vec<(WorkspaceId, egui::Rect)>,
    tile_drag_ghost: Option<&TileDragGhost>,
    now: Instant,
    squish: &Option<(usize, f64)>,
) {
    // Anything animating (pulse + generating dots) wants a steady repaint.
    // We set a single flag here so we only pay for one `request_repaint_after`
    // per frame regardless of how many cards are active.
    let mut any_animation = tile_drag_ghost.is_some();

    egui::TopBottomPanel::top("kookaburra-strip")
        .exact_height(STRIP_HEIGHT)
        .frame(
            Frame::none()
                .fill(STRIP_BG)
                .stroke(Stroke::new(1.0, GRID_LINE))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Logo + brand
                logo_placeholder(ui);
                ui.add_space(4.0);
                // Vertical separator
                let sep_rect = ui.allocate_exact_size(Vec2::new(1.0, 36.0), Sense::hover()).0;
                ui.painter().rect_filled(sep_rect, Rounding::ZERO, GRID_LINE);
                ui.add_space(8.0);

                // Horizontal scroll wraps cards + the trailing `+` so the
                // strip stays navigable when there are more workspaces
                // than fit on screen.
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for (idx, ws) in state.workspaces.iter().enumerate() {
                                let (rect, animating) = draw_workspace_slot(
                                    ui, ws, idx, state, actions, renaming, reorder, now, squish,
                                );
                                card_rects.push((ws.id, rect));
                                any_animation |= animating;
                                ui.add_space(6.0);
                            }

                            if plus_button(ui).clicked() {
                                actions.push(Action::CreateWorkspace);
                            }
                        });
                    });

                // Search box placeholder — from the design's strip search input.
                // Read-only decorative widget; actual search is Phase 7.
                ui.add_space(6.0);
                search_placeholder(ui);
            });
        });

    // Workspace vanished (e.g. deleted via middle-click while mid-rename)?
    // Drop the stale editor.
    if let Some(r) = renaming.as_ref() {
        if state.workspace(r.id).is_none() {
            *renaming = None;
        }
    }

    // Paint the reorder drop indicator + resolve drag-stop.
    resolve_reorder(ctx, state, card_rects, actions, reorder);

    // Paint the tile-drag ghost on top of everything so it reads against
    // both the strip and the tile area beneath.
    if let Some(ghost) = tile_drag_ghost {
        paint_tile_drag_ghost(ctx, ghost);
    }

    if any_animation {
        ctx.request_repaint_after(ANIMATION_TICK);
    }
}

/// Draw the card (or rename editor) for one workspace. Returns the
/// card's rect so the caller can record it for drag-drop hit testing,
/// plus a bool indicating the card is animating this frame (so the
/// outer pass can schedule a repaint tick).
#[allow(clippy::too_many_arguments)]
fn draw_workspace_slot(
    ui: &mut egui::Ui,
    ws: &Workspace,
    idx: usize,
    state: &AppState,
    actions: &mut Vec<Action>,
    renaming: &mut Option<RenameState>,
    reorder: &mut Option<ReorderState>,
    now: Instant,
    squish: &Option<(usize, f64)>,
) -> (egui::Rect, bool) {
    let active = ws.id == state.active_workspace;

    // Rename editor takes over the card slot for the ws being renamed.
    if renaming.as_ref().is_some_and(|r| r.id == ws.id) {
        return (draw_rename_editor(ui, ws.id, actions, renaming), false);
    }

    let signals = workspace_signals(ws, now);
    let show_activity_dot = !active && signals.dirty && !signals.generating;
    let dragging_this = reorder.as_ref().is_some_and(|r| r.source_idx == idx);
    let is_squishing = squish.map(|(si, _)| si == idx).unwrap_or(false);
    let resp = draw_card(
        ui,
        &ws.label,
        ws.id,
        idx,
        active,
        ws.tiles.len(),
        &ws.layout.label(),
        show_activity_dot,
        signals.generating,
        dragging_this,
        is_squishing,
        ui.ctx().input(|i| i.time),
    );
    // Drag start: plain left-drag on a card. Use this to reorder, not
    // to switch.
    if resp.drag_started_by(egui::PointerButton::Primary) {
        *reorder = Some(ReorderState { source_idx: idx });
    }
    // Plain click (no drag crossed threshold) switches to the workspace.
    if resp.clicked() {
        actions.push(Action::SwitchWorkspace(ws.id));
    }
    // Middle-click = close workspace (tmux / browser-tab convention).
    // Right-click is reserved for a future context menu.
    if resp.clicked_by(egui::PointerButton::Middle) {
        actions.push(Action::DeleteWorkspace(ws.id));
    }
    // Double-click the label to rename in place. `clicked()` fires on the
    // first release too, so the user sees the workspace switch immediately
    // and the editor opens on the second click — a small, acceptable
    // flicker.
    if resp.double_clicked() {
        *renaming = Some(RenameState {
            id: ws.id,
            buffer: ws.label.clone(),
            focus_requested: false,
        });
    }
    let animating = show_activity_dot || signals.generating || is_squishing;
    (resp.rect, animating)
}

fn draw_rename_editor(
    ui: &mut egui::Ui,
    id: WorkspaceId,
    actions: &mut Vec<Action>,
    renaming: &mut Option<RenameState>,
) -> egui::Rect {
    let size = Vec2::new(CARD_WIDTH, CARD_HEIGHT);
    let r = renaming
        .as_mut()
        .expect("draw_rename_editor only called when renaming is Some");
    let edit = egui::TextEdit::singleline(&mut r.buffer)
        .desired_width(CARD_WIDTH - 12.0)
        .text_color(FG)
        .font(FontId::proportional(13.0))
        .frame(false);
    let frame = Frame::none()
        .fill(BG_DIM)
        .stroke(Stroke::new(1.5, ACCENT))
        .rounding(Rounding::ZERO)
        .inner_margin(egui::Margin::symmetric(6.0, 8.0));
    let response = frame
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                size,
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.add(edit),
            )
            .inner
        })
        .inner;

    // Focus the field on first draw so the user can start typing.
    if !r.focus_requested {
        response.request_focus();
        r.focus_requested = true;
    }

    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let lost_focus = response.lost_focus();

    let rect = response.rect;
    if enter || (lost_focus && !escape) {
        let new_label = r.buffer.trim().to_string();
        if !new_label.is_empty() {
            actions.push(Action::RenameWorkspace { id, new_label });
        }
        *renaming = None;
    } else if escape {
        *renaming = None;
    }
    rect
}

fn logo_placeholder(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
    let painter = ui.painter();
    let t = ui.ctx().input(|i| i.time);

    // koo-idle-bob: gentle 1px sine bob (2s period).
    let bob = -((t * std::f64::consts::TAU / 2.0).sin() as f32);

    // koo-peck: a subtle forward-tilt every 1.6s. The design uses
    // `transformOrigin: 40% 70%` with a rotation, which we approximate as
    // a small vertical dip + horizontal nudge during the "peck" phase.
    let peck_cycle = (t % 1.6) / 1.6;
    let (peck_dx, peck_dy) = if peck_cycle < 0.15 {
        // Down-peck: dip 2px forward-down.
        let p = (peck_cycle / 0.15) as f32;
        (p * 1.5, p * 2.0)
    } else if peck_cycle < 0.30 {
        // Return: back to rest.
        let p = ((peck_cycle - 0.15) / 0.15) as f32;
        ((1.0 - p) * 1.5, (1.0 - p) * 2.0)
    } else {
        (0.0, 0.0)
    };

    let center = rect.center() + Vec2::new(peck_dx, bob + peck_dy);
    // Draw a pixel-art K: big monospace glyph
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        "K",
        FontId::monospace(24.0),
        ACCENT,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    ui: &mut egui::Ui,
    label: &str,
    _id: WorkspaceId,
    ws_index: usize,
    active: bool,
    tile_count: usize,
    layout_label: &str,
    activity: bool,
    generating: bool,
    dragging: bool,
    squishing: bool,
    time_secs: f64,
) -> egui::Response {
    // koo-ws-squish: a quick scale-bounce when switching to this card.
    // v2 keyframes:
    //   0%   scaleX(0.6) scaleY(1.15)  — compressed horizontally, tall
    //   60%  scaleX(1.08) scaleY(0.94) — overshot wide, short
    //   100% scaleX(1) scaleY(1)       — settle
    // 340ms total, cubic-bezier(.2,.9,.3,1.3).
    let (squish_sx, squish_sy) = if squishing {
        // Use a simple linear interpolation through the keyframes.
        // The egui time-based animation won't perfectly match CSS beziers
        // but captures the feel.
        let t_frac = ((time_secs * 1000.0) % 340.0) / 340.0;
        if t_frac < 0.6 {
            let p = (t_frac / 0.6) as f32;
            // 0→60%: scaleX 0.6→1.08, scaleY 1.15→0.94
            let sx = 0.6 + p * 0.48;
            let sy = 1.15 - p * 0.21;
            (sx, sy)
        } else {
            let p = ((t_frac - 0.6) / 0.4) as f32;
            // 60→100%: scaleX 1.08→1.0, scaleY 0.94→1.0
            let sx = 1.08 - p * 0.08;
            let sy = 0.94 + p * 0.06;
            (sx, sy)
        }
    } else {
        (1.0, 1.0)
    };
    // Active cards lift -2px (translateY), inactive scale to 0.96.
    let inactive_scale = if active { 1.0 } else { 0.96 };
    let card_h = CARD_HEIGHT * squish_sy * inactive_scale;
    let card_w = CARD_WIDTH * squish_sx;
    let size = Vec2::new(card_w, card_h);
    let (bg, mut fg) = if active {
        (BG_DIM, FG)
    } else {
        (BG_DEEP, FG_DIM)
    };
    // Fade the card being dragged so the drop indicator reads clearer.
    if dragging {
        fg = fg.gamma_multiply(0.55);
    }
    // Build label with optional generating pulse dot prefix.
    let text = if label.is_empty() {
        format!("Workspace {}", ws_index + 1)
    } else {
        label.to_string()
    };
    // Generating pulse: "● label" where the dot breathes.
    let label_rich = if generating {
        let pulse = 0.5 + 0.5 * ((time_secs * std::f64::consts::TAU / 1.2).sin() as f32);
        let mut job = egui::text::LayoutJob::default();
        job.append(
            "● ",
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(13.0),
                color: ACCENT.gamma_multiply(pulse),
                ..Default::default()
            },
        );
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(13.0),
                color: fg,
                ..Default::default()
            },
        );
        job
    } else {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(13.0),
                color: fg,
                ..Default::default()
            },
        );
        job
    };
    let button = Button::new(label_rich)
        .fill(if dragging { bg.gamma_multiply(0.7) } else { bg })
        .stroke(if active {
            Stroke::new(2.0, ACCENT)
        } else {
            Stroke::new(2.0, GRID_LINE)
        })
        .rounding(Rounding::ZERO)
        .sense(Sense::click_and_drag())
        .min_size(size);
    let response = ui.add_sized(size, button);

    let painter = ui.painter();

    // Active card bottom accent bar — a bright amber strip at the bottom
    // edge that reads as "this is the current workspace" at a glance.
    if active {
        let bar = egui::Rect::from_min_size(
            response.rect.left_bottom() - Vec2::new(0.0, 3.0),
            Vec2::new(CARD_WIDTH, 3.0),
        );
        painter.rect_filled(bar, Rounding::ZERO, ACCENT);
    }

    // Hotkey chip in top-right corner: ⌘N
    {
        let chip_text = format!("\u{2318}{}", ws_index + 1);
        let chip_rect = egui::Rect::from_min_size(
            response.rect.right_top() + Vec2::new(-24.0, -2.0),
            Vec2::new(22.0, 12.0),
        );
        let (chip_bg, chip_fg_color) = if active {
            (ACCENT, BG_DEEP)
        } else {
            (STRIP_BG, FG_FAINT)
        };
        painter.rect_filled(chip_rect, Rounding::ZERO, chip_bg);
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            chip_text,
            FontId::monospace(8.0),
            chip_fg_color,
        );
    }

    // Sub-label: tile count bottom-right
    if tile_count > 0 {
        let pos = response.rect.right_bottom() - Vec2::new(8.0, 6.0);
        painter.text(
            pos,
            egui::Align2::RIGHT_BOTTOM,
            format!("{tile_count}"),
            FontId::monospace(10.0),
            fg.gamma_multiply(0.75),
        );
    }
    // Tile indicator dots below the label — each dot is a small square,
    // and when the card is generating the dots cascade with a staggered
    // breathe animation (mimicking koo-pulse from the design).
    if tile_count > 0 {
        let painter = ui.painter();
        let dot_size = 6.0;
        let spacing = 3.0;
        let total_width = tile_count as f32 * (dot_size + spacing) - spacing;
        let start_x = response.rect.center().x - total_width / 2.0;
        let start_y = response.rect.bottom() - 10.0;
        for i in 0..tile_count {
            let x = start_x + i as f32 * (dot_size + spacing);
            // koo-bounce: generating dots get a staggered vertical bounce
            // (1.1s period, 180ms delay per dot modulo 1s).
            let bounce_y = if generating {
                let delay = (i as f64 * 0.18) % 1.0;
                let phase = ((time_secs - delay) * std::f64::consts::TAU / 1.1).sin();
                // Bounce only upward (negative y): clamp the sine to [0, 1].
                let up = (phase as f32).max(0.0);
                -3.0 * up
            } else {
                0.0
            };
            let dot_rect = egui::Rect::from_min_size(
                egui::Pos2::new(x, start_y + bounce_y),
                Vec2::splat(dot_size),
            );
            let dot_color = if generating {
                // Staggered breathe: each dot is phase-shifted.
                let phase = (time_secs * std::f64::consts::TAU / 1.2 + i as f64 * 0.8).sin();
                let alpha = 0.45 + 0.55 * ((phase as f32 + 1.0) / 2.0);
                ACCENT.gamma_multiply(alpha)
            } else if active {
                FG_FAINT
            } else {
                GRID_LINE
            };
            painter.rect_filled(dot_rect, Rounding::ZERO, dot_color);
        }
    }
    // Layout chip — bottom-left corner, showing "2x2" etc. like the design.
    {
        let chip_text = layout_label;
        let chip_w = chip_text.len() as f32 * 5.5 + 6.0;
        let chip_rect = egui::Rect::from_min_size(
            response.rect.left_bottom() + Vec2::new(-2.0, -12.0),
            Vec2::new(chip_w, 12.0),
        );
        painter.rect_filled(chip_rect, Rounding::ZERO, STRIP_BG);
        painter.rect_stroke(chip_rect, Rounding::ZERO, Stroke::new(1.0, GRID_LINE));
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            chip_text,
            FontId::monospace(8.0),
            FG_FAINT,
        );
    }

    // Activity / generating markers live in the top-right area. Generating
    // wins over the static "unread" dot.
    if generating {
        let painter = ui.painter();
        let center = response.rect.right_top() + Vec2::new(-12.0, 10.0);
        draw_generating_marker(painter, center, time_secs);
    } else if activity {
        // Breathe the dot alpha with a sine in [0.55, 1.0]
        let phase = (time_secs * std::f64::consts::TAU / 1.6).sin();
        let alpha = 0.55 + 0.225 * (phase as f32 + 1.0);
        let dot = GREEN.gamma_multiply(alpha);
        let painter = ui.painter();
        let center = response.rect.right_top() + Vec2::new(-8.0, 8.0);
        painter.rect_filled(
            egui::Rect::from_center_size(center, Vec2::splat(7.0)),
            Rounding::ZERO,
            dot,
        );
    }
    response
}

/// Aggregate signals for the card. `dirty` is "has unread output since the
/// user last touched it"; `generating` is "bytes arrived in the last
/// GENERATING_LATENCY_MS" — rough stand-in for a Claude-Code-specific
/// stream detector that'll ship in Phase 5 / 6 alongside config.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct WorkspaceSignals {
    dirty: bool,
    generating: bool,
}

fn workspace_signals(ws: &Workspace, now: Instant) -> WorkspaceSignals {
    let window = Duration::from_millis(GENERATING_LATENCY_MS);
    let mut out = WorkspaceSignals::default();
    for t in &ws.tiles {
        if t.has_new_output {
            out.dirty = true;
        }
        if let Some(at) = t.last_output_at {
            if now.saturating_duration_since(at) < window {
                out.generating = true;
            }
        }
    }
    out
}

/// Draw a three-dot streaming indicator. Each dot fades up and down on its
/// own phase so the trio reads as "…" in motion, a familiar "typing" cue.
fn draw_generating_marker(painter: &egui::Painter, center: egui::Pos2, time_secs: f64) {
    const DOT_SIZE: f32 = 3.0;
    const SPACING: f32 = 5.0;
    // Period ≈ 1.2 s per cycle; stagger each dot by a third of the cycle.
    let period = 1.2;
    for i in 0..3 {
        let phase = ((time_secs / period) + i as f64 / 3.0) * std::f64::consts::TAU;
        // Remap sin from [-1, 1] to [0.35, 1.0].
        let alpha = 0.35 + 0.325 * (phase.sin() as f32 + 1.0);
        let dx = (i as f32 - 1.0) * SPACING;
        let dot_pos = center + Vec2::new(dx, 0.0);
        let dot_rect = egui::Rect::from_center_size(dot_pos, Vec2::splat(DOT_SIZE));
        painter.rect_filled(dot_rect, Rounding::ZERO, ACCENT.gamma_multiply(alpha));
    }
}

fn paint_tile_drag_ghost(ctx: &egui::Context, ghost: &TileDragGhost) {
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("kookaburra-tile-drag-ghost"),
    ));
    let label = if ghost.label.is_empty() {
        "tile".to_string()
    } else if ghost.label.len() > 28 {
        // Cap the pill width — a full title would dwarf the strip.
        let mut truncated: String = ghost.label.chars().take(25).collect();
        truncated.push('…');
        truncated
    } else {
        ghost.label.clone()
    };
    // Offset so the cursor "grips" the top-left corner of the pill
    // without occluding it.
    let size = egui::vec2((label.len() as f32 * 7.5 + 20.0).max(72.0), 22.0);
    let rect = egui::Rect::from_min_size(pos + Vec2::new(10.0, 6.0), size);
    painter.rect(
        rect,
        Rounding::ZERO,
        BG_DIM.gamma_multiply(0.95),
        Stroke::new(1.0, ACCENT),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        FG,
    );
}

/// Strip search-box placeholder — the design has "⌕ search all tiles… ⌘⇧F"
/// as a read-only field in the strip. Actual search is Phase 7; this is
/// decorative only.
fn search_placeholder(ui: &mut egui::Ui) {
    let height = 28.0;
    let width = 160.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        Rounding::ZERO,
        BG_DEEP,
        Stroke::new(1.0, GRID_LINE),
    );
    // Left-aligned "⌕ search all tiles…" label
    painter.text(
        rect.left_center() + Vec2::new(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "\u{2315} search all tiles\u{2026}",
        FontId::monospace(10.0),
        FG_FAINT,
    );
    // Right-aligned keyboard shortcut chip
    let chip_w = 30.0;
    let chip_h = 14.0;
    let chip_rect = egui::Rect::from_min_size(
        rect.right_center() + Vec2::new(-chip_w - 6.0, -chip_h / 2.0),
        Vec2::new(chip_w, chip_h),
    );
    painter.rect(chip_rect, Rounding::ZERO, STRIP_BG, Stroke::new(1.0, GRID_LINE));
    painter.text(
        chip_rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{2318}\u{21e7}F",
        FontId::monospace(8.0),
        FG_FAINT,
    );
}

fn plus_button(ui: &mut egui::Ui) -> egui::Response {
    let size = Vec2::new(CARD_HEIGHT, CARD_HEIGHT);
    let button = Button::new(
        RichText::new("+")
            .color(FG_DIM)
            .font(FontId::proportional(20.0)),
    )
    .fill(BG_DEEP)
    .stroke(Stroke::new(2.0, GRID_LINE))
    .rounding(Rounding::ZERO)
    .min_size(size);
    ui.add_sized(size, button)
}

/// Build the status bar at the bottom of the window.
fn build_status_bar(ctx: &egui::Context, state: &AppState, now: Instant) {
    // Request periodic repaint for the pulsing ready dot + uptime clock.
    ctx.request_repaint_after(ANIMATION_TICK);
    egui::TopBottomPanel::bottom("kookaburra-status-bar")
        .exact_height(22.0)
        .frame(
            Frame::none()
                .fill(BG_DEEP)
                .stroke(Stroke::new(1.0, GRID_LINE))
                .inner_margin(egui::Margin::symmetric(10.0, 0.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // Active workspace dot + label
                if let Some(active_ws) = state.workspace(state.active_workspace) {
                    // Amber dot
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(6.0), Sense::hover());
                    ui.painter().rect_filled(dot_rect, Rounding::ZERO, ACCENT);
                    ui.label(
                        RichText::new(&active_ws.label)
                            .color(FG)
                            .font(FontId::monospace(10.0)),
                    );
                }

                // Separator
                sep(ui);

                // Tile count: "tile N/M"
                if let Some(active_ws) = state.workspace(state.active_workspace) {
                    let focused_idx = state.focused_tile
                        .and_then(|fid| active_ws.tiles.iter().position(|t| t.id == fid))
                        .unwrap_or(0) + 1;
                    let total = active_ws.tiles.len();
                    ui.label(
                        RichText::new(format!("tile {focused_idx}/{total}"))
                            .color(FG_DIM)
                            .font(FontId::monospace(10.0)),
                    );
                }

                sep(ui);

                // Generating count
                if let Some(active_ws) = state.workspace(state.active_workspace) {
                    let generating_count = active_ws.tiles.iter().filter(|t| {
                        if let Some(at) = t.last_output_at {
                            now.saturating_duration_since(at) < Duration::from_millis(GENERATING_LATENCY_MS)
                        } else {
                            false
                        }
                    }).count();
                    ui.label(
                        RichText::new(format!("{generating_count} generating"))
                            .color(FG_DIM)
                            .font(FontId::monospace(10.0)),
                    );
                }

                sep(ui);

                // Layout label
                if let Some(active_ws) = state.workspace(state.active_workspace) {
                    ui.label(
                        RichText::new(active_ws.layout.label())
                            .color(FG_FAINT)
                            .font(FontId::monospace(10.0)),
                    );
                }

                // Flexible space → right-aligned section
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;

                    // Pulsing "● ready" indicator — gentle sine breathe
                    let t = ui.ctx().input(|i| i.time);
                    let pulse = 0.7 + 0.3 * ((t * std::f64::consts::TAU / 2.0).sin() as f32);
                    ui.label(
                        RichText::new("● ready")
                            .color(GREEN.gamma_multiply(pulse))
                            .font(FontId::monospace(10.0)),
                    );

                    sep(ui);

                    // Zen state indicator
                    let zen_text = if state.zen_mode { "ZEN" } else { "zen" };
                    let zen_color = if state.zen_mode { ACCENT } else { FG_DIM };
                    ui.label(
                        RichText::new(zen_text)
                            .color(zen_color)
                            .font(FontId::monospace(10.0)),
                    );

                    sep(ui);

                    // Session uptime clock — uses egui's monotonic time.
                    let t_secs = t;
                    let mins = (t_secs / 60.0) as u64;
                    let secs = (t_secs % 60.0) as u64;
                    ui.label(
                        RichText::new(format!("{mins:02}:{secs:02}"))
                            .color(FG_FAINT)
                            .font(FontId::monospace(10.0)),
                    );
                });
            });
        });
}

/// Status bar separator: "│" in grid-line color.
fn sep(ui: &mut egui::Ui) {
    ui.label(
        RichText::new("│")
            .color(GRID_LINE)
            .font(FontId::monospace(10.0)),
    );
}

/// Convenience: produce a `SwitchWorkspace` action. Lives here so unit
/// tests can exercise it without pulling in egui plumbing.
#[must_use]
pub fn switch_workspace_action(id: WorkspaceId) -> Action {
    Action::SwitchWorkspace(id)
}

/// If a reorder is in flight, compute the drop slot from the pointer, paint
/// an accent marker between cards to show where the drop will land, and on
/// drag-release emit `Action::ReorderWorkspaces`.
fn resolve_reorder(
    ctx: &egui::Context,
    state: &AppState,
    card_rects: &[(WorkspaceId, egui::Rect)],
    actions: &mut Vec<Action>,
    reorder: &mut Option<ReorderState>,
) {
    let Some(state_copy) = reorder.as_ref().map(|r| r.source_idx) else {
        return;
    };
    let pointer_down = ctx.input(|i| i.pointer.primary_down());
    let pointer_pos = ctx.input(|i| i.pointer.interact_pos());

    // Compute the insert position: "insert before card k" for the smallest k
    // where cursor_x < card_k.center.x, else len.
    let target_idx = match pointer_pos {
        Some(p) => card_rects
            .iter()
            .position(|(_, r)| p.x < r.center().x)
            .unwrap_or(card_rects.len()),
        None => state_copy,
    };

    // Paint an accent bar at the drop position, between cards.
    if let Some(pos) = pointer_pos {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("kookaburra-reorder-marker"),
        ));
        let (x, strip_rect) = if target_idx >= card_rects.len() {
            let last = card_rects.last().map(|(_, r)| *r);
            let x = last.map(|r| r.right() + 3.0).unwrap_or(pos.x);
            (x, last.unwrap_or(egui::Rect::from_min_max(pos, pos)))
        } else {
            let r = card_rects[target_idx].1;
            (r.left() - 3.0, r)
        };
        let bar = egui::Rect::from_min_max(
            egui::pos2(x - 1.5, strip_rect.top()),
            egui::pos2(x + 1.5, strip_rect.bottom()),
        );
        painter.rect_filled(bar, Rounding::ZERO, ACCENT);
    }

    // Drop: primary button released while we had a drag in progress.
    if !pointer_down {
        let from = state_copy;
        // Target insert index is in "before this card" terms; remove +
        // insert math needs an adjustment when dragging right-to-left past
        // the source.
        let to = if target_idx > from {
            target_idx - 1
        } else {
            target_idx.min(state.workspaces.len().saturating_sub(1))
        };
        if from != to {
            actions.push(Action::ReorderWorkspaces { from, to });
        }
        *reorder = None;
    }
}

/// Paint an egui overlay (rounded fill + outline + "+" glyph + subtitle)
/// for every empty slot in the active workspace, and register click
/// hit-boxes that promote the slot via `SpawnInTile`. Live slots are
/// skipped so terminal mouse interactions in their rects fall through
/// to the app's existing terminal mouse path.
fn paint_empty_slots(
    ctx: &egui::Context,
    state: &AppState,
    actions: &mut Vec<Action>,
    area: egui::Rect,
) {
    let ws = state.active_workspace();
    let core_area = CoreRect {
        x: area.left(),
        y: area.top(),
        width: area.width(),
        height: area.height(),
    };
    let rects = compute_tile_rects(ws.layout, core_area);
    for (i, tile) in ws.tiles.iter().enumerate() {
        if tile.is_live() {
            continue;
        }
        let Some(r) = rects.get(i).copied() else {
            break;
        };
        let slot = egui::Rect::from_min_size(
            egui::pos2(
                r.x + EMPTY_SLOT_GAP * 0.5,
                r.y + EMPTY_SLOT_GAP * 0.5,
            ),
            egui::vec2(
                (r.width - EMPTY_SLOT_GAP).max(1.0),
                (r.height - EMPTY_SLOT_GAP).max(1.0),
            ),
        );
        let focused = state.focused_tile == Some(tile.id);
        paint_empty_slot(ctx, actions, tile.id, slot, focused);
    }
}

/// Paint a single empty-slot overlay and handle its click interaction.
/// Uses an `egui::Area` so input only fires within the slot rect — which
/// means clicks inside live tile rects fall through to the terminal.
fn paint_empty_slot(
    ctx: &egui::Context,
    actions: &mut Vec<Action>,
    tile_id: TileId,
    slot: egui::Rect,
    focused: bool,
) {
    let area_id = egui::Id::new(("kb-empty-slot", tile_id.raw()));
    egui::Area::new(area_id)
        .order(egui::Order::Background)
        .fixed_pos(slot.min)
        .movable(false)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(slot.size(), Sense::click());
            let painter = ui.painter();
            let rounding = Rounding::same(6.0);
            painter.rect_filled(rect, rounding, EMPTY_SLOT_FILL);
            let stroke = if focused {
                Stroke::new(1.5, ACCENT)
            } else {
                Stroke::new(1.0, GRID_LINE)
            };
            painter.rect_stroke(rect, rounding, stroke);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "+",
                FontId::proportional(28.0),
                FG_DIM,
            );
            if rect.height() > EMPTY_SLOT_SUBTITLE_MIN_HEIGHT {
                let sub = egui::pos2(rect.center().x, rect.center().y + 24.0);
                painter.text(
                    sub,
                    egui::Align2::CENTER_CENTER,
                    "click or press ⏎",
                    FontId::proportional(11.0),
                    FG_FAINT,
                );
            }
            if response.clicked() {
                // Focus first so if the spawn fails for any reason the
                // user can still press Enter on the focused slot.
                actions.push(Action::FocusTile(tile_id));
                actions.push(Action::SpawnInTile {
                    tile_id,
                    worktree: None,
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use kookaburra_core::ids::PtyId;
    use kookaburra_core::state::Tile;

    #[test]
    fn switch_workspace_action_wraps_id() {
        let id = WorkspaceId::new();
        match switch_workspace_action(id) {
            Action::SwitchWorkspace(got) => assert_eq!(got, id),
            _ => panic!("wrong action variant"),
        }
    }

    #[test]
    fn workspace_signals_flag_dirty_and_generating() {
        let mut ws = Workspace::new("w");
        let now = Instant::now();
        assert_eq!(
            workspace_signals(&ws, now),
            WorkspaceSignals::default(),
            "empty workspace is quiet"
        );

        ws.push_tile(Tile::new(PtyId::new()));
        assert_eq!(
            workspace_signals(&ws, now),
            WorkspaceSignals::default(),
            "idle tile shouldn't flag anything"
        );

        let mut noisy = Tile::new(PtyId::new());
        noisy.has_new_output = true;
        ws.push_tile(noisy);
        let s = workspace_signals(&ws, now);
        assert!(s.dirty, "one dirty tile lights the unread flag");
        assert!(!s.generating, "no timestamp means not generating");

        // A tile that emitted bytes within the generating window is flagged.
        let mut streaming = Tile::new(PtyId::new());
        streaming.last_output_at = Some(now);
        ws.push_tile(streaming);
        assert!(workspace_signals(&ws, now).generating);

        // An old timestamp past the window falls off.
        let mut stale = Tile::new(PtyId::new());
        stale.last_output_at = Some(now - Duration::from_secs(3));
        let mut ws2 = Workspace::new("w2");
        ws2.push_tile(stale);
        assert!(
            !workspace_signals(&ws2, now).generating,
            "stale output shouldn't trigger the marker"
        );
    }
}
