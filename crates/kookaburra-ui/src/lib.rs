//! UI strip, cards, dialogs, input routing.
//!
//! Rough-draft scaffolding. The spec (§7) calls for an egui-based strip
//! drawn via `egui-wgpu` inside the same render pass as the terminals,
//! plus an event router that prioritizes egui → focused tile → terminal
//! mouse → main loop.
//!
//! This commit defines the API shapes the rest of the workspace depends
//! on; the egui integration itself is a follow-up. See `NOTES.md`.

use kookaburra_core::action::Action;
use kookaburra_core::ids::WorkspaceId;
use kookaburra_core::state::AppState;

/// Strip dimensions per spec §3 ("Card dimensions: ~140×48px").
pub const STRIP_HEIGHT: f32 = 56.0;
pub const CARD_WIDTH: f32 = 140.0;
pub const CARD_HEIGHT: f32 = 48.0;

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

/// UI layer state. Real implementation will hold `egui::Context`,
/// `egui_winit::State`, `egui_wgpu::Renderer`. For now it just tracks
/// which widget is "hot" so the router can decide where input goes.
pub struct UiLayer {
    /// Whether an egui text widget currently wants keyboard focus.
    pub wants_keyboard: bool,
    /// Whether the cursor is hovering an egui widget.
    pub wants_pointer: bool,
}

impl Default for UiLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl UiLayer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            wants_keyboard: false,
            wants_pointer: false,
        }
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

    /// Build the strip and append any user actions to `actions`.
    /// Stub: walks the workspace list and would emit a `SwitchWorkspace`
    /// per click; the real implementation does this against `egui::Ui`.
    pub fn draw_strip(&mut self, _state: &AppState, _actions: &mut Vec<Action>) {
        // No-op for the rough draft. The signature matches the real one
        // so the main loop can already call it.
    }
}

/// Convenience: produce a `SwitchWorkspace` action. The strip would emit
/// this on card click. Lives here so unit tests can exercise it without
/// pulling in egui.
#[must_use]
pub fn switch_workspace_action(id: WorkspaceId) -> Action {
    Action::SwitchWorkspace(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_goes_to_tile_when_ui_idle() {
        let ui = UiLayer::new();
        assert_eq!(ui.route_keyboard(), InputRouting::ToFocusedTile);
    }

    #[test]
    fn keyboard_goes_to_ui_when_ui_wants_focus() {
        let mut ui = UiLayer::new();
        ui.wants_keyboard = true;
        assert_eq!(ui.route_keyboard(), InputRouting::ConsumedByUi);
    }

    #[test]
    fn draw_strip_does_not_panic_on_empty_state() {
        use kookaburra_core::config::Config;
        let state = AppState::new(Config::default());
        let mut ui = UiLayer::new();
        let mut actions = Vec::new();
        ui.draw_strip(&state, &mut actions);
        assert!(actions.is_empty());
    }
}
