# Empty tile slots — design

Status: approved 2026-04-19. Implementation to follow.

## Context

Today Kookaburra boots with workspace 1 eagerly filled by six live PTYs (the
`App::ensure_starter_tiles` loop in `crates/kookaburra-app/src/main.rs`). Every
spawn costs a shell, and the grid commits to six long-running processes before
the user has done anything. Newly created workspaces, by contrast, start with
zero tiles and render as an empty void — the grid doesn't visibly exist until
tiles are created.

We want both ends of this to feel intentional:

- Workspace 1 boots with a single live tile in the top-left slot. The other
  five slots still occupy their grid space and invite interaction.
- New workspaces (Cmd+N, drag-tile-to-empty-strip, etc.) show six empty slots.
  Nothing is spawned until the user asks.

The user instantiates a slot by clicking it or by focusing it and pressing
Enter.

## Goals

- First-class "empty slot" concept that participates in focus, layout, and
  drag/drop without carrying a PTY.
- Predictable lifecycle: closing a live tile demotes the slot back to empty;
  slot identity (`TileId`) is stable for the lifetime of the workspace.
- Minimal new surface area — reuse the existing Action pipeline and egui
  overlay rather than building new subsystems.

## Non-goals

- Dynamic slot counts or tile-merging (collapse two slots into one).
- Layout cycling via `Cmd+G` when slot contents are mixed — left as a TODO;
  slot resize policy will be decided when that feature lands.
- Worktree dialog / shell chooser before spawn. Instantiation is immediate,
  matching current `ensure_starter_tiles` behavior. The Phase 6 worktree
  dialog will layer on top later.

## Design

### 1. State model

`Tile.pty_id` becomes optional. Workspaces maintain the invariant
`tiles.len() == layout.cell_count()` — the grid is fully populated with slots
from construction, even when none are live.

```rust
// crates/kookaburra-core/src/state.rs
pub struct Tile {
    pub id: TileId,
    pub pty_id: Option<PtyId>,      // None = empty placeholder
    pub title: String,              // empty string for empty tiles
    pub worktree: Option<Worktree>,
    pub has_new_output: bool,
    pub last_output_at: Option<Instant>,
    // …existing fields preserved…
}

impl Tile {
    pub fn empty() -> Self { /* fresh TileId, pty_id = None */ }
    pub fn is_live(&self) -> bool { self.pty_id.is_some() }
}
```

`Workspace::new` fills `tiles` with `layout.cell_count()` empty tiles. This
applies to every workspace — bootstrap, Cmd+N, and `MoveTileToNewWorkspace`.

### 2. Action model

`CreateTile` is replaced with a slot-addressed `SpawnInTile`. The action does
not allocate a new slot; it promotes an existing empty one.

```rust
// crates/kookaburra-core/src/action.rs
pub enum Action {
    SpawnInTile { tile_id: TileId, worktree: Option<WorktreeConfig> },
    CloseTile(TileId),   // demote: kill PTY, keep slot
    // MoveTile, MoveTileToNewWorkspace, FocusTile, CreateWorkspace, …unchanged
}
```

- `SpawnInTile`: if the target is already live, no-op. Otherwise call
  `pty.spawn(...)`, store `Some(pty_id)`, apply the worktree config.
- `CloseTile`: call `pty.kill(pty_id)`, set `tile.pty_id = None`, clear
  `title` / `worktree`. The slot stays.
- `MoveTile`: find the first empty slot in the destination workspace, move
  the source tile's contents into it, empty the source slot. If no empty
  slot exists in the destination, the move is a no-op (logged).
- `MoveTileToNewWorkspace`: create a workspace full of empties, move source
  contents into slot 0, empty the source slot.
- `CreateWorkspace`: no signature change; the constructor now yields a
  workspace full of empty slots.

`PtySideEffects` gains a `kill(pty_id)` method. `PtyAdapter` in `main.rs`
delegates to `PtyManager::kill` (new or existing — to be confirmed during
implementation).

### 3. Seeding & startup

`App::ensure_starter_tiles` collapses from six `CreateTile` calls to a single
`SpawnInTile` on `workspaces[0].tiles[0]`.

```rust
fn ensure_starter_tiles(&mut self) {
    let first_tile = self.state.workspaces[0].tiles[0].id;
    self.apply(Action::SpawnInTile { tile_id: first_tile, worktree: None });
}
```

Because `Workspace::new` already fills its slots, no extra plumbing is
needed for new workspaces — they ship with six empties by construction.

### 4. Render pipeline

Empty tiles are drawn as egui overlays on top of the cleared black rect that
the wgpu pass leaves behind. No new wgpu quad pipeline is required.

Per empty tile, the egui overlay paints:

- A rounded filled rect in `theme.background` mixed ~8% toward the foreground
  — present but dormant.
- A 1px outline in `theme.surface_dim`, or the accent color when focused
  (`theme.accent`, ~1.5px).
- A centered `+` glyph in muted fg, approximately 28px.
- A small subtitle `"click or press ⏎"` in muted fg (~11px), hidden below a
  minimum-size threshold so cramped grids don't overflow.

Live tiles continue through the existing wgpu + glyphon path, including the
`UNFOCUSED_DIM` mix for unfocused tiles. The empty-placeholder overlay runs
inside the egui pass that already handles the strip and cards.

Mouse hit-testing uses an `egui::InteractResponse` placed at the slot rect.
A click both focuses the slot (`Action::FocusTile`) and instantiates it
(`Action::SpawnInTile`) in one event. Live tiles have no egui hitbox, so
clicks in their rects fall through to the existing terminal mouse path.

### 5. Input routing

`handle_key` and `handle_mouse_wheel` currently early-return when
`active_pty()` is `None`. They become dispatch branches:

```rust
fn handle_key(&mut self, event: &KeyEvent) {
    let Some(tile) = self.active_tile() else { return; };
    match tile.pty_id {
        Some(pty_id) => self.forward_to_pty(pty_id, event),
        None         => self.handle_empty_tile_key(tile.id, event),
    }
}

fn handle_empty_tile_key(&mut self, tile_id: TileId, event: &KeyEvent) {
    if event.state == Pressed
        && event.logical_key == Key::Named(NamedKey::Enter)
    {
        self.apply(Action::SpawnInTile { tile_id, worktree: None });
    }
    // all other keys ignored on empty slots
}
```

Mouse wheel on an empty slot is a no-op. Focus still flows through
`FocusTile(TileId)`; empties are valid focus targets.

### 6. Keybindings

- `Cmd+W` (new): when a live tile is focused, fires `CloseTile(focused_tile)`.
  Demotes the slot. No-op when the focused slot is already empty.
- `Enter` (contextual): only active when the focused tile is empty. Fires
  `SpawnInTile` on that tile.
- `Cmd+Opt+1..6`, `Cmd+G`, `Cmd+1..9`, `Cmd+N`, `Cmd+L`, etc.: unchanged.

## Testing

Unit tests, colocated with existing tests in `core`:

- `layout` / `state`: a workspace constructed via `Workspace::new` holds
  exactly `cell_count()` tiles, all empty.
- `apply_action`:
  - `SpawnInTile` promotes an empty slot to live; second call is a no-op.
  - `CloseTile` demotes a live slot (kills PTY, clears title/worktree),
    preserves the slot.
  - `MoveTile` from `ws_a.tile[k]` to `ws_b` places contents in `ws_b`'s
    first empty slot and leaves `ws_a.tile[k]` empty.
  - `MoveTile` into a full destination is a no-op.
  - `CreateWorkspace` yields a workspace full of empties.
  - `MoveTileToNewWorkspace` yields a workspace whose slot 0 holds the
    moved tile and whose remaining slots are empty.
- `StubPty` gains a `kill` spy and tracks both `spawn` and `kill` calls.

Render + input changes are exercised manually (`cargo run`), since we have
no integration-test harness for the wgpu/egui surface yet.

## Migration notes

- `Tile` construction changes break call sites. The old `Tile::new(pty_id)`
  and `Tile::with_id(tile_id, pty_id)` stay if useful (live-tile shortcuts);
  an `empty()` constructor is added.
- Any external caller that assumed `tile.pty_id: PtyId` will fail to
  compile. There are no external callers today — this is purely internal
  cleanup across the workspace crates.
- `Action::CreateTile` is removed; every call site (tests, app bootstrap)
  becomes `SpawnInTile` targeting a specific `TileId`.
- The checklist item "Phase 1 — apply_action against StubPty (create / close
  / move / zen / delete)" stays passing, with updated assertions.

## Risks

- **egui/wgpu input precedence.** The placeholder overlay intercepts clicks
  before the terminal mouse path. We need to make sure the overlay is only
  present for empty slots — if it accidentally covers a live tile, terminal
  mouse interactions break. Mitigation: the overlay is emitted only when
  `tile.pty_id.is_none()`.
- **Focus correctness when a slot is instantiated mid-drag.** A click on an
  empty slot both focuses and instantiates. If the same event is also a
  drag-start, the drag-to-card logic could misfire on a slot that just
  became live. Mitigation: gate drag-start on `tile.is_live()` at mouse-down
  time.
- **`CloseTile` semantics churn.** Existing tests that rely on `CloseTile`
  removing a tile from the Vec need to be rewritten. Expected and accounted
  for in the testing plan.

## Open questions

Deferred — address when touched:

- Layout cycling (`Cmd+G`) with mixed empty/live slots. When the grid
  shrinks, do live tiles get demoted or preserved? When it grows, are new
  slots always empty? The existing Cmd+G path is tile-flow-based and needs
  redesign; out of scope here.
- Visual treatment of the focused empty slot vs. focused live slot accent.
  Reuse the live focus accent color; tune after we see it in motion.
