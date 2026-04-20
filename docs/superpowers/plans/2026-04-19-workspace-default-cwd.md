# Per-Workspace Default CWD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each workspace carries an optional `default_cwd`; new tiles spawn there, editable inline via the existing workspace-rename UI.

**Architecture:** Add `Workspace::default_cwd`, a new `Action::SetWorkspaceCwd`, and extend `PtySideEffects::spawn` to take a `cwd` arg. The existing rename editor gains a second `TextEdit` for the path. `CreateWorkspace` and `MoveTileToNewWorkspace` inherit the default from the active/source workspace. No worktree changes — worktree cwd, when it lands, will take precedence inside `PtyAdapter::spawn`.

**Tech Stack:** Rust, `kookaburra-core` (domain types + `apply_action`), `kookaburra-ui` (egui), `kookaburra-pty` (portable-pty), `kookaburra-app` (winit glue), `directories` crate for `~` expansion.

**Spec:** `docs/superpowers/specs/2026-04-19-workspace-default-cwd-design.md`

---

## File Structure

- **Modify** `crates/kookaburra-core/src/state.rs` — add `Workspace::default_cwd: Option<PathBuf>`; update `Workspace::new`, constructor sites, and existing unit tests that exhaustively match on `Workspace` fields (if any).
- **Modify** `crates/kookaburra-core/src/action.rs` — add `Action::SetWorkspaceCwd`; change `PtySideEffects::spawn` signature; forward cwd in `SpawnInTile`; inherit in `CreateWorkspace` / `MoveTileToNewWorkspace`; update `StubPty` in the test module; add new unit tests.
- **Modify** `crates/kookaburra-ui/src/lib.rs` — grow `RenameState` with a cwd buffer; replace `draw_rename_editor` with a two-field editor; add pure `expand_cwd_input` helper + tests.
- **Modify** `crates/kookaburra-app/src/main.rs` — update `PtyAdapter::spawn` to accept `cwd: Option<&Path>` and forward it into `SpawnRequest.cwd`.

No new files.

---

## Task 1: Add `default_cwd` field to `Workspace`

**Files:**
- Modify: `crates/kookaburra-core/src/state.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/kookaburra-core/src/state.rs` (near `workspace_new_fills_tiles_with_cell_count_empties`):

```rust
#[test]
fn workspace_new_has_no_default_cwd() {
    let ws = Workspace::new("scratch");
    assert!(ws.default_cwd.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kookaburra-core --lib workspace_new_has_no_default_cwd`
Expected: FAIL — `no field 'default_cwd' on type '&Workspace'`.

- [ ] **Step 3: Add the field and initialize it**

In `crates/kookaburra-core/src/state.rs`, add the field to `Workspace`:

```rust
#[derive(Clone, Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
    pub layout: Layout,
    pub tiles: Vec<Tile>,
    /// Optional designated tile that gets focus when switching to this
    /// workspace.
    pub primary_tile: Option<TileId>,
    /// Default working directory for new tiles spawned in this workspace.
    /// `None` means "process cwd". Worktree cwd, when present, overrides.
    pub default_cwd: Option<std::path::PathBuf>,
}
```

Update `Workspace::new` to initialize it:

```rust
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
        default_cwd: None,
    }
}
```

- [ ] **Step 4: Run the core test suite to verify nothing else broke**

Run: `cargo test -p kookaburra-core --lib`
Expected: PASS (all existing tests + the new one).

- [ ] **Step 5: Commit**

```bash
git add crates/kookaburra-core/src/state.rs
git commit -m "feat(core): add default_cwd field to Workspace"
```

---

## Task 2: Extend `PtySideEffects::spawn` to take a `cwd` arg

**Files:**
- Modify: `crates/kookaburra-core/src/action.rs`
- Modify: `crates/kookaburra-app/src/main.rs`

This is a signature change on a trait with only two implementers (`PtyAdapter` in `app`, `StubPty` in `action.rs` tests). Do both together in one compile-clean commit.

- [ ] **Step 1: Update the trait signature**

In `crates/kookaburra-core/src/action.rs`, add a `Path` import at the top alongside the existing `use`s:

```rust
use std::path::Path;
```

Replace the `PtySideEffects` trait:

```rust
pub trait PtySideEffects {
    /// Spawn a new PTY bound to `tile_id` and return its id. The `tile_id`
    /// is decided by `apply_action` before this call so the PTY's event
    /// listener can tag events with the same id the `Tile` will carry.
    ///
    /// `cwd` is the workspace's configured default working directory
    /// (from `Workspace::default_cwd`), or `None` to inherit the process
    /// cwd. When the implementation supports worktrees and `worktree` is
    /// `Some`, the worktree path takes precedence over `cwd`.
    fn spawn(
        &mut self,
        tile_id: TileId,
        cwd: Option<&Path>,
        worktree: Option<&WorktreeConfig>,
    ) -> PtyId;
    /// Close a PTY. Best-effort; failures should be logged, not returned.
    fn close(&mut self, pty: PtyId);
}
```

- [ ] **Step 2: Update `apply_action::SpawnInTile` to forward the workspace's default_cwd**

In the same file, replace the `Action::SpawnInTile` arm with:

```rust
        Action::SpawnInTile { tile_id, worktree } => {
            // Promote an empty slot to live. No-op if the tile doesn't
            // resolve or the slot is already live.
            let is_empty_slot = state.tile(tile_id).map(|t| !t.is_live()).unwrap_or(false);
            if is_empty_slot {
                let cwd = state
                    .workspaces
                    .iter()
                    .find(|w| w.tiles.iter().any(|t| t.id == tile_id))
                    .and_then(|w| w.default_cwd.clone());
                let pty_id = pty.spawn(tile_id, cwd.as_deref(), worktree.as_ref());
                if let Some(tile) = state.tile_mut(tile_id) {
                    tile.promote(pty_id);
                }
                state.focused_tile = Some(tile_id);
            }
        }
```

- [ ] **Step 3: Update `StubPty` in the test module**

In the `tests` submodule of `crates/kookaburra-core/src/action.rs`, replace `StubPty`:

```rust
    #[derive(Default)]
    struct StubPty {
        spawns: u32,
        closes: u32,
        last_spawn_tile: Option<TileId>,
        last_spawn_cwd: Option<std::path::PathBuf>,
    }

    impl PtySideEffects for StubPty {
        fn spawn(
            &mut self,
            tile_id: TileId,
            cwd: Option<&std::path::Path>,
            _worktree: Option<&WorktreeConfig>,
        ) -> PtyId {
            self.spawns += 1;
            self.last_spawn_tile = Some(tile_id);
            self.last_spawn_cwd = cwd.map(|p| p.to_path_buf());
            PtyId::new()
        }
        fn close(&mut self, _pty: PtyId) {
            self.closes += 1;
        }
    }
```

- [ ] **Step 4: Update `PtyAdapter::spawn` in the app**

In `crates/kookaburra-app/src/main.rs`, replace the `PtySideEffects` impl for `PtyAdapter`:

```rust
impl<'a> PtySideEffects for PtyAdapter<'a> {
    fn spawn(
        &mut self,
        tile_id: TileId,
        cwd: Option<&std::path::Path>,
        _worktree: Option<&WorktreeConfig>,
    ) -> PtyId {
        let req = SpawnRequest {
            tile_id,
            cwd: cwd.map(|p| p.to_path_buf()),
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
```

- [ ] **Step 5: Verify the whole workspace still builds and tests pass**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test --workspace --lib`
Expected: PASS (existing tests; `last_spawn_cwd` is currently unused but will be asserted in Task 4).

- [ ] **Step 6: Commit**

```bash
git add crates/kookaburra-core/src/action.rs crates/kookaburra-app/src/main.rs
git commit -m "feat(core): thread cwd through PtySideEffects::spawn"
```

---

## Task 3: Add `Action::SetWorkspaceCwd`

**Files:**
- Modify: `crates/kookaburra-core/src/action.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module of `crates/kookaburra-core/src/action.rs`:

```rust
    #[test]
    fn set_workspace_cwd_updates_field() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let id = state.active_workspace;
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id,
                cwd: Some(std::path::PathBuf::from("/tmp/proj")),
            },
        );
        assert_eq!(
            state.workspace(id).unwrap().default_cwd.as_deref(),
            Some(std::path::Path::new("/tmp/proj"))
        );
    }

    #[test]
    fn set_workspace_cwd_none_clears_field() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let id = state.active_workspace;
        state.workspace_mut(id).unwrap().default_cwd =
            Some(std::path::PathBuf::from("/tmp/old"));
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd { id, cwd: None },
        );
        assert!(state.workspace(id).unwrap().default_cwd.is_none());
    }

    #[test]
    fn set_workspace_cwd_with_unknown_id_is_noop() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let other = WorkspaceId::new();
        apply_action(
            &mut state,
            &mut pty,
            Action::SetWorkspaceCwd {
                id: other,
                cwd: Some(std::path::PathBuf::from("/tmp/x")),
            },
        );
        // Existing workspace unaffected.
        assert!(state.active_workspace().default_cwd.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kookaburra-core --lib set_workspace_cwd`
Expected: FAIL — `no variant 'SetWorkspaceCwd'`.

- [ ] **Step 3: Add the variant**

In `crates/kookaburra-core/src/action.rs`, add to the `Action` enum (in the "Workspaces" section, after `ReorderWorkspaces`):

```rust
    SetWorkspaceCwd {
        id: WorkspaceId,
        cwd: Option<std::path::PathBuf>,
    },
```

- [ ] **Step 4: Handle it in `apply_action`**

Add a new arm in `apply_action` (place it after `ReorderWorkspaces`):

```rust
        Action::SetWorkspaceCwd { id, cwd } => {
            if let Some(ws) = state.workspace_mut(id) {
                ws.default_cwd = cwd;
            }
        }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kookaburra-core --lib set_workspace_cwd`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/kookaburra-core/src/action.rs
git commit -m "feat(core): add SetWorkspaceCwd action"
```

---

## Task 4: Forward `default_cwd` on spawn + inherit on workspace creation/move

**Files:**
- Modify: `crates/kookaburra-core/src/action.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `crates/kookaburra-core/src/action.rs`:

```rust
    #[test]
    fn spawn_in_tile_forwards_workspace_default_cwd() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let ws_id = state.active_workspace;
        state.workspace_mut(ws_id).unwrap().default_cwd =
            Some(std::path::PathBuf::from("/tmp/proj"));
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert_eq!(
            pty.last_spawn_cwd.as_deref(),
            Some(std::path::Path::new("/tmp/proj"))
        );
    }

    #[test]
    fn spawn_in_tile_passes_none_cwd_when_workspace_has_no_default() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        assert!(pty.last_spawn_cwd.is_none());
    }

    #[test]
    fn create_workspace_inherits_default_cwd_from_active() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let active = state.active_workspace;
        state.workspace_mut(active).unwrap().default_cwd =
            Some(std::path::PathBuf::from("/tmp/proj"));
        apply_action(&mut state, &mut pty, Action::CreateWorkspace);
        assert_eq!(
            state.active_workspace().default_cwd.as_deref(),
            Some(std::path::Path::new("/tmp/proj"))
        );
    }

    #[test]
    fn move_tile_to_new_workspace_inherits_default_cwd_from_source() {
        let mut state = AppState::new(Config::default());
        let mut pty = StubPty::default();
        let source = state.active_workspace;
        state.workspace_mut(source).unwrap().default_cwd =
            Some(std::path::PathBuf::from("/tmp/proj"));
        let tile_id = state.active_workspace().tiles[0].id;
        apply_action(
            &mut state,
            &mut pty,
            Action::SpawnInTile {
                tile_id,
                worktree: None,
            },
        );
        apply_action(
            &mut state,
            &mut pty,
            Action::MoveTileToNewWorkspace { tile_id },
        );
        assert_eq!(
            state.active_workspace().default_cwd.as_deref(),
            Some(std::path::Path::new("/tmp/proj"))
        );
    }
```

- [ ] **Step 2: Run tests to verify them**

Run: `cargo test -p kookaburra-core --lib`
Expected:
- `spawn_in_tile_forwards_workspace_default_cwd` — PASS already (Task 2 wired the forwarding).
- `spawn_in_tile_passes_none_cwd_when_workspace_has_no_default` — PASS already.
- `create_workspace_inherits_default_cwd_from_active` — FAIL (new workspace is `None`).
- `move_tile_to_new_workspace_inherits_default_cwd_from_source` — FAIL.

- [ ] **Step 3: Inherit in `CreateWorkspace`**

In `apply_action`, replace the `Action::CreateWorkspace` arm:

```rust
        Action::CreateWorkspace => {
            let inherited_cwd = state.active_workspace().default_cwd.clone();
            let idx = state.workspaces.len() + 1;
            let mut ws = Workspace::new(format!("workspace {idx}"));
            ws.default_cwd = inherited_cwd;
            let id = ws.id;
            state.workspaces.push(ws);
            state.active_workspace = id;
            state.focused_tile = None;
        }
```

- [ ] **Step 4: Inherit in `MoveTileToNewWorkspace`**

Replace the `Action::MoveTileToNewWorkspace` arm:

```rust
        Action::MoveTileToNewWorkspace { tile_id } => {
            let Some((src_wi, src_ti)) = find_tile_loc(state, tile_id) else {
                return;
            };
            if !state.workspaces[src_wi].tiles[src_ti].is_live() {
                return;
            }
            let inherited_cwd = state.workspaces[src_wi].default_cwd.clone();
            let moved =
                std::mem::replace(&mut state.workspaces[src_wi].tiles[src_ti], Tile::empty());
            let idx = state.workspaces.len() + 1;
            let mut ws = Workspace::new(format!("workspace {idx}"));
            ws.default_cwd = inherited_cwd;
            let new_ws_id = ws.id;
            // Slot 0 of the new workspace receives the moved tile.
            ws.tiles[0] = moved;
            state.workspaces.push(ws);
            state.active_workspace = new_ws_id;
            state.focused_tile = Some(tile_id);
        }
```

- [ ] **Step 5: Run tests to verify all pass**

Run: `cargo test -p kookaburra-core --lib`
Expected: PASS (all existing + 4 new).

- [ ] **Step 6: Commit**

```bash
git add crates/kookaburra-core/src/action.rs
git commit -m "feat(core): forward and inherit workspace default_cwd"
```

---

## Task 5: UI helper for parsing user cwd input

**Files:**
- Modify: `crates/kookaburra-ui/src/lib.rs`

This is a pure helper so we can unit-test the empty-string / `~` expansion / trim rules without spinning up egui.

- [ ] **Step 1: Write the failing tests**

Add a new `#[cfg(test)] mod cwd_input_tests { ... }` block at the bottom of `crates/kookaburra-ui/src/lib.rs`:

```rust
#[cfg(test)]
mod cwd_input_tests {
    use super::expand_cwd_input;
    use std::path::PathBuf;

    #[test]
    fn empty_returns_none() {
        assert!(expand_cwd_input("").is_none());
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert!(expand_cwd_input("   ").is_none());
    }

    #[test]
    fn absolute_path_passes_through() {
        assert_eq!(
            expand_cwd_input("/tmp/proj"),
            Some(PathBuf::from("/tmp/proj"))
        );
    }

    #[test]
    fn leading_tilde_expands_to_home() {
        let home = directories::UserDirs::new()
            .and_then(|u| u.home_dir().to_path_buf().into())
            .expect("test env has a home dir");
        assert_eq!(expand_cwd_input("~"), Some(home.clone()));
        assert_eq!(expand_cwd_input("~/proj"), Some(home.join("proj")));
    }

    #[test]
    fn tilde_without_slash_suffix_expands_and_keeps_rest_as_subpath() {
        // "~foo" is NOT username expansion — treat as literal path.
        assert_eq!(expand_cwd_input("~foo"), Some(PathBuf::from("~foo")));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            expand_cwd_input("  /tmp/proj  "),
            Some(PathBuf::from("/tmp/proj"))
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kookaburra-ui --lib cwd_input_tests`
Expected: FAIL — `expand_cwd_input` not found.

- [ ] **Step 3: Add the helper**

Check whether `kookaburra-ui` already depends on `directories`:

Run: `grep directories crates/kookaburra-ui/Cargo.toml`
- If present, skip to adding the helper.
- If not, add to `[dependencies]` in `crates/kookaburra-ui/Cargo.toml`:

```toml
directories = { workspace = true }
```

(Verify `directories` is declared in the root `Cargo.toml`'s `[workspace.dependencies]`; if not, use the same version string already used in `kookaburra-core`.)

Add the function to `crates/kookaburra-ui/src/lib.rs`, near the top of the file, outside any `impl`:

```rust
/// Parse a user-typed workspace cwd into an `Option<PathBuf>`.
///
/// Empty / whitespace-only input → `None`. Leading `~` or `~/…` is
/// expanded to the user's home directory. Other inputs pass through
/// verbatim — we do not validate that the path exists (the shell will
/// surface bad paths at spawn time, which is cheaper than a picker).
#[must_use]
pub fn expand_cwd_input(raw: &str) -> Option<std::path::PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return directories::UserDirs::new().map(|u| u.home_dir().to_path_buf());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return directories::UserDirs::new().map(|u| u.home_dir().join(rest));
    }
    Some(std::path::PathBuf::from(trimmed))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kookaburra-ui --lib cwd_input_tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kookaburra-ui/src/lib.rs crates/kookaburra-ui/Cargo.toml
git commit -m "feat(ui): add expand_cwd_input helper"
```

---

## Task 6: Two-field rename editor (label + default_cwd)

**Files:**
- Modify: `crates/kookaburra-ui/src/lib.rs`

- [ ] **Step 1: Grow `RenameState` with a cwd buffer**

In `crates/kookaburra-ui/src/lib.rs`, replace the `RenameState` struct:

```rust
/// Ephemeral state for the inline workspace editor. Lives on `UiLayer`
/// rather than `AppState` because it's a pure UI concern — the canonical
/// label and default_cwd only update when the user commits with Enter.
struct RenameState {
    id: WorkspaceId,
    label_buffer: String,
    cwd_buffer: String,
    /// Initial values, captured on open, so we can emit zero actions when
    /// neither field actually changed.
    initial_label: String,
    initial_cwd: String,
    focus_requested: bool,
}
```

- [ ] **Step 2: Update `UiLayer::start_rename` to seed both buffers**

Replace:

```rust
    pub fn start_rename(&mut self, id: WorkspaceId, initial: String) {
        self.renaming = Some(RenameState {
            id,
            buffer: initial,
            focus_requested: false,
        });
    }
```

with:

```rust
    /// Open the inline editor for a workspace card. Seeds the label buffer
    /// from `initial_label` and the cwd buffer from `initial_cwd` (empty
    /// string when the workspace has no default). Used by the `Cmd+L`
    /// keybinding; double-click is handled inside `build_strip`.
    pub fn start_rename(&mut self, id: WorkspaceId, initial_label: String, initial_cwd: String) {
        self.renaming = Some(RenameState {
            id,
            label_buffer: initial_label.clone(),
            cwd_buffer: initial_cwd.clone(),
            initial_label,
            initial_cwd,
            focus_requested: false,
        });
    }
```

- [ ] **Step 3: Update the double-click call site**

In `draw_workspace_slot` (where `resp.double_clicked()` handles open), replace:

```rust
    if resp.double_clicked() {
        *renaming = Some(RenameState {
            id: ws.id,
            buffer: ws.label.clone(),
            focus_requested: false,
        });
    }
```

with:

```rust
    if resp.double_clicked() {
        let initial_cwd = ws
            .default_cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        *renaming = Some(RenameState {
            id: ws.id,
            label_buffer: ws.label.clone(),
            cwd_buffer: initial_cwd.clone(),
            initial_label: ws.label.clone(),
            initial_cwd,
            focus_requested: false,
        });
    }
```

- [ ] **Step 4: Update any Cmd+L / app-side call sites**

Search and fix any remaining callers of `start_rename`:

Run: `grep -n "start_rename" crates/`
For each call site (likely in `crates/kookaburra-app/src/main.rs`), thread the workspace's `default_cwd` alongside `label`. Example fix pattern — if the existing code looks like:

```rust
ui.start_rename(ws.id, ws.label.clone());
```

replace with:

```rust
let initial_cwd = ws
    .default_cwd
    .as_ref()
    .map(|p| p.display().to_string())
    .unwrap_or_default();
ui.start_rename(ws.id, ws.label.clone(), initial_cwd);
```

- [ ] **Step 5: Replace `draw_rename_editor` with the two-field version**

Constants: the card now expands when editing. Add near the existing `CARD_WIDTH` / `CARD_HEIGHT`:

```rust
/// Height the card grows to while the inline editor is open (room for
/// label + cwd fields stacked).
pub const CARD_EDITOR_HEIGHT: f32 = 76.0;
```

Replace the entire `draw_rename_editor` function with:

```rust
fn draw_rename_editor(
    ui: &mut egui::Ui,
    id: WorkspaceId,
    ws_default_cwd: Option<&std::path::Path>,
    actions: &mut Vec<Action>,
    renaming: &mut Option<RenameState>,
) -> egui::Rect {
    let size = Vec2::new(CARD_WIDTH, CARD_EDITOR_HEIGHT);
    let r = renaming
        .as_mut()
        .expect("draw_rename_editor only called when renaming is Some");

    let frame = Frame::none()
        .fill(BG_DIM)
        .stroke(Stroke::new(1.5, ACCENT))
        .rounding(Rounding::ZERO)
        .inner_margin(egui::Margin::symmetric(6.0, 6.0));

    let inner = frame.show(ui, |ui| {
        ui.allocate_ui_with_layout(
            size,
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let label_edit = egui::TextEdit::singleline(&mut r.label_buffer)
                    .desired_width(CARD_WIDTH - 12.0)
                    .text_color(FG)
                    .font(FontId::proportional(13.0))
                    .hint_text("name")
                    .frame(false);
                let label_resp = ui.add(label_edit);

                ui.add_space(2.0);

                let cwd_edit = egui::TextEdit::singleline(&mut r.cwd_buffer)
                    .desired_width(CARD_WIDTH - 12.0)
                    .text_color(FG)
                    .font(FontId::proportional(11.0))
                    .hint_text("path (e.g. ~/projects/foo)")
                    .frame(false);
                let cwd_resp = ui.add(cwd_edit);

                // Focus the label on first draw; Tab moves to cwd.
                if !r.focus_requested {
                    label_resp.request_focus();
                    r.focus_requested = true;
                }
                (label_resp, cwd_resp)
            },
        )
        .inner
    });

    let (label_resp, cwd_resp) = inner.inner;
    let rect = inner.response.rect;

    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let lost_focus_without_moving = (label_resp.lost_focus() && !cwd_resp.has_focus())
        || (cwd_resp.lost_focus() && !label_resp.has_focus());

    if enter || (lost_focus_without_moving && !escape) {
        // Commit label if changed and non-empty after trimming.
        let new_label = r.label_buffer.trim().to_string();
        if !new_label.is_empty() && new_label != r.initial_label {
            actions.push(Action::RenameWorkspace {
                id,
                new_label,
            });
        }

        // Commit cwd if the user's typed input parses to something
        // different from the current default.
        let parsed = expand_cwd_input(&r.cwd_buffer);
        let current = ws_default_cwd.map(|p| p.to_path_buf());
        if parsed != current {
            actions.push(Action::SetWorkspaceCwd { id, cwd: parsed });
        }
        *renaming = None;
    } else if escape {
        *renaming = None;
    }

    rect
}
```

- [ ] **Step 6: Update the `draw_rename_editor` call site**

In `draw_workspace_slot`, change:

```rust
    if renaming.as_ref().is_some_and(|r| r.id == ws.id) {
        return (draw_rename_editor(ui, ws.id, actions, renaming), false);
    }
```

to:

```rust
    if renaming.as_ref().is_some_and(|r| r.id == ws.id) {
        return (
            draw_rename_editor(ui, ws.id, ws.default_cwd.as_deref(), actions, renaming),
            false,
        );
    }
```

- [ ] **Step 7: Build and run the entire test suite**

Run: `cargo build --workspace`
Expected: clean build. If there are references to `RenameState.buffer` anywhere in the file (search `grep -n "r\.buffer\|\.buffer\b" crates/kookaburra-ui/src/lib.rs`), rename them to `r.label_buffer`.

Run: `cargo test --workspace --lib`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 8: Smoke-test the feature by running the app**

Run: `cargo run -p kookaburra-app`
Manually verify:
1. Double-click a workspace card → card expands to two-field editor.
2. Type a name, Tab, type `~/tmp` → press Enter.
3. Open the card again: name persisted, path field shows `/Users/…/tmp`.
4. Click the empty slot (`+`): a new terminal spawns in `~/tmp` (run `pwd`).
5. `Cmd+N` creates a new workspace with the same default cwd; `pwd` in a new tile there is the same.
6. Clear the path (select all, delete, Enter): new tiles spawn in the process cwd again.

If the UI is unreachable in your environment, say so in the commit message so the reviewer knows manual verification is pending.

- [ ] **Step 9: Commit**

```bash
git add crates/kookaburra-ui/src/lib.rs
git commit -m "feat(ui): edit workspace default_cwd alongside rename"
```

---

## Task 7: Tick the checklist and note the change in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the Phase 3 rename bullet**

Find the existing line in `CLAUDE.md`:

```
- [x] Workspace rename inline (double-click label, `Cmd+L`) — double-click or `Cmd+L` flips the card into a `TextEdit`; Enter commits, Esc cancels.
```

Replace with:

```
- [x] Workspace rename inline (double-click label, `Cmd+L`) — double-click or `Cmd+L` flips the card into a two-field editor (label + `default_cwd`); Tab moves between fields, Enter commits both, Esc cancels. New tiles spawned in a workspace start in its `default_cwd`. `CreateWorkspace` and `MoveTileToNewWorkspace` inherit the default from their source.
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: note per-workspace default_cwd in checklist"
```

---

## Self-Review

Spec coverage — each design section has a task:

- State: `Workspace::default_cwd` → Task 1.
- `Action::SetWorkspaceCwd` + handler → Task 3.
- `PtySideEffects::spawn` signature change + cwd forwarding → Task 2.
- Inheritance on `CreateWorkspace` / `MoveTileToNewWorkspace` → Task 4.
- UI two-field editor → Task 6; pure parsing helper → Task 5.
- `PtyAdapter::spawn` forwarding → Task 2.
- Tests — every design-section test is covered: SetWorkspaceCwd set/clear, forwarding, both inheritance paths, expand_cwd_input edge cases.

Placeholders — none. Every code step has full code blocks. Every test step names the file, function, and expected FAIL/PASS outcome.

Type consistency — `PtySideEffects::spawn(&mut self, TileId, Option<&Path>, Option<&WorktreeConfig>) -> PtyId` is used identically in the trait definition (Task 2 step 1), `PtyAdapter` impl (Task 2 step 4), and `StubPty` impl (Task 2 step 3). `start_rename(id, label, cwd)` signature matches at definition and call site (Task 6 steps 2 + 4). `RenameState` field names (`label_buffer`, `cwd_buffer`, `initial_label`, `initial_cwd`, `focus_requested`) are consistent across steps 1, 5, 6.

One non-obvious bit: Task 2's `StubPty` change adds `last_spawn_cwd` but Task 2 tests don't assert on it — Task 4's new tests do. That's intentional; Task 2 must compile-clean before Task 4 can build on it.
