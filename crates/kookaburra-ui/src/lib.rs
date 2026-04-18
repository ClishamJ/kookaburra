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
use kookaburra_core::ids::WorkspaceId;
use kookaburra_core::state::{AppState, Workspace};

/// Bytes arriving on a tile within this window treat it as "actively
/// streaming" (the "Claude is generating" signal). Longer means the marker
/// lingers after the last chunk; shorter means it flickers during brief
/// pauses between tokens. 600 ms is comfortably above inter-token jitter
/// without keeping the marker lit during purely idle sessions.
const GENERATING_LATENCY_MS: u64 = 600;
/// How often we re-request a frame while an animation is visible. 50 ms ≈
/// 20 fps — plenty for breathing alpha / moving dots, cheap enough to not
/// matter for a mostly-idle UI.
const ANIMATION_TICK: Duration = Duration::from_millis(50);

/// Strip dimensions per spec §3 ("Card dimensions: ~140×48px").
pub const STRIP_HEIGHT: f32 = 56.0;
pub const CARD_WIDTH: f32 = 140.0;
pub const CARD_HEIGHT: f32 = 44.0;

/// Tokyo Night-ish chrome palette (matches the default theme).
const STRIP_BG: Color32 = Color32::from_rgb(26, 27, 38);
const CARD_BG: Color32 = Color32::from_rgb(40, 42, 54);
const CARD_BG_ACTIVE: Color32 = Color32::from_rgb(60, 62, 80);
const CARD_FG_ACTIVE: Color32 = Color32::from_rgb(224, 229, 255);
const CARD_FG_INACTIVE: Color32 = Color32::from_rgb(150, 153, 168);
const ACCENT: Color32 = Color32::from_rgb(122, 162, 247);
/// Pulse dot for the "has unread output" hint on inactive cards.
const ACTIVITY_DOT: Color32 = Color32::from_rgb(158, 206, 106);
const ACTIVITY_DOT_RADIUS: f32 = 3.5;

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
}

impl UiLayer {
    /// Build a fresh UI layer bound to `window`. The egui pixels-per-point
    /// is seeded from the window's scale factor; this can be mutated later
    /// via [`Self::set_scale_factor`].
    #[must_use]
    pub fn new(window: &Window) -> Self {
        let ctx = egui::Context::default();
        let pixels_per_point = window.scale_factor() as f32;
        ctx.set_pixels_per_point(pixels_per_point);
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
        }
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
            );
        });
        self.wants_keyboard = ctx.wants_keyboard_input();
        self.wants_pointer = ctx.wants_pointer_input();
        self.winit_state
            .handle_platform_output(window, full_output.platform_output);
        let pixels_per_point = ctx.pixels_per_point();
        let primitives = ctx.tessellate(full_output.shapes, pixels_per_point);
        PreparedFrame {
            primitives,
            textures_delta: full_output.textures_delta,
            pixels_per_point,
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
                .inner_margin(egui::Margin::symmetric(10.0, 6.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Logo — placeholder monogram until the SVG is rasterized
                // into an egui texture. 1-bit white "K".
                logo_placeholder(ui);
                ui.add_space(12.0);

                // Horizontal scroll wraps cards + the trailing `+` so the
                // strip stays navigable when there are more workspaces
                // than fit on screen.
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for (idx, ws) in state.workspaces.iter().enumerate() {
                                let (rect, animating) = draw_workspace_slot(
                                    ui, ws, idx, state, actions, renaming, reorder, now,
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
) -> (egui::Rect, bool) {
    let active = ws.id == state.active_workspace;

    // Rename editor takes over the card slot for the ws being renamed.
    if renaming.as_ref().is_some_and(|r| r.id == ws.id) {
        return (draw_rename_editor(ui, ws.id, actions, renaming), false);
    }

    let signals = workspace_signals(ws, now);
    let show_activity_dot = !active && signals.dirty && !signals.generating;
    let dragging_this = reorder.as_ref().is_some_and(|r| r.source_idx == idx);
    let resp = draw_card(
        ui,
        &ws.label,
        ws.id,
        active,
        ws.tiles.len(),
        show_activity_dot,
        signals.generating,
        dragging_this,
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
    let animating = show_activity_dot || signals.generating;
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
        .text_color(CARD_FG_ACTIVE)
        .font(FontId::proportional(13.0))
        .frame(false);
    let frame = Frame::none()
        .fill(CARD_BG_ACTIVE)
        .stroke(Stroke::new(1.5, ACCENT))
        .rounding(Rounding::same(6.0))
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
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
    let painter = ui.painter();
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "K",
        FontId::monospace(20.0),
        Color32::WHITE,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    ui: &mut egui::Ui,
    label: &str,
    id: WorkspaceId,
    active: bool,
    tile_count: usize,
    activity: bool,
    generating: bool,
    dragging: bool,
    time_secs: f64,
) -> egui::Response {
    let size = Vec2::new(CARD_WIDTH, CARD_HEIGHT);
    let (bg, mut fg) = if active {
        (CARD_BG_ACTIVE, CARD_FG_ACTIVE)
    } else {
        (CARD_BG, CARD_FG_INACTIVE)
    };
    // Fade the card being dragged so the drop indicator reads clearer.
    if dragging {
        fg = fg.gamma_multiply(0.55);
    }
    let text = if label.is_empty() {
        format!("Workspace {}", id.raw())
    } else {
        label.to_string()
    };
    let button = Button::new(
        RichText::new(text)
            .color(fg)
            .font(FontId::proportional(13.0)),
    )
    .fill(if dragging { bg.gamma_multiply(0.7) } else { bg })
    .stroke(if active {
        Stroke::new(1.5, ACCENT)
    } else {
        Stroke::NONE
    })
    .rounding(Rounding::same(6.0))
    // `click_and_drag` so egui reports drag-started / dragged / drag-stopped
    // on the card for the reorder flow. Plain clicks still fire on release
    // when the drag threshold wasn't crossed.
    .sense(Sense::click_and_drag())
    .min_size(size);
    let response = ui.add_sized(size, button);
    // Sub-label: tile count.
    if tile_count > 0 {
        let painter = ui.painter();
        let pos = response.rect.right_bottom() - Vec2::new(8.0, 6.0);
        painter.text(
            pos,
            egui::Align2::RIGHT_BOTTOM,
            format!("{tile_count}"),
            FontId::monospace(10.0),
            fg.gamma_multiply(0.75),
        );
    }
    // Activity / generating markers live in the top-right corner. Generating
    // wins over the static "unread" dot: if bytes are actively streaming we
    // surface that, and the unread state becomes redundant anyway.
    if generating {
        let painter = ui.painter();
        let center = response.rect.right_top() + Vec2::new(-12.0, 10.0);
        draw_generating_marker(painter, center, time_secs);
    } else if activity {
        // Breathe the dot alpha with a sine in [0.55, 1.0] so inactive
        // workspaces gently wave "come look" without distracting.
        let phase = (time_secs * std::f64::consts::TAU / 1.6).sin();
        let alpha = 0.55 + 0.225 * (phase as f32 + 1.0);
        let dot = ACTIVITY_DOT.gamma_multiply(alpha);
        let painter = ui.painter();
        let center = response.rect.right_top() + Vec2::new(-8.0, 8.0);
        painter.circle_filled(center, ACTIVITY_DOT_RADIUS, dot);
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
    const DOT_RADIUS: f32 = 2.0;
    const SPACING: f32 = 5.0;
    // Period ≈ 1.2 s per cycle; stagger each dot by a third of the cycle.
    let period = 1.2;
    for i in 0..3 {
        let phase = ((time_secs / period) + i as f64 / 3.0) * std::f64::consts::TAU;
        // Remap sin from [-1, 1] to [0.35, 1.0].
        let alpha = 0.35 + 0.325 * (phase.sin() as f32 + 1.0);
        let dx = (i as f32 - 1.0) * SPACING;
        painter.circle_filled(
            center + Vec2::new(dx, 0.0),
            DOT_RADIUS,
            ACCENT.gamma_multiply(alpha),
        );
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
        Rounding::same(6.0),
        CARD_BG_ACTIVE.gamma_multiply(0.95),
        Stroke::new(1.0, ACCENT),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        CARD_FG_ACTIVE,
    );
}

fn plus_button(ui: &mut egui::Ui) -> egui::Response {
    let size = Vec2::new(CARD_HEIGHT, CARD_HEIGHT);
    let button = Button::new(
        RichText::new("+")
            .color(CARD_FG_INACTIVE)
            .font(FontId::proportional(20.0)),
    )
    .fill(CARD_BG)
    .stroke(Stroke::NONE)
    .rounding(Rounding::same(6.0))
    .min_size(size);
    ui.add_sized(size, button)
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
        painter.rect_filled(bar, Rounding::same(1.5), ACCENT);
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
