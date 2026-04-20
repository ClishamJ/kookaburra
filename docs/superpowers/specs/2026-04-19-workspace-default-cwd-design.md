# Per-workspace default path for new tiles

**Status:** Design
**Date:** 2026-04-19

## Problem

Every new tile currently spawns with the process's cwd (whatever directory `kookaburra` was launched from). A user who keeps one workspace per project wants new tiles in that workspace to start in the project's root without re-typing `cd ~/long/path/to/repo` each time.

## Goal

Each workspace carries an optional `default_cwd`. New tiles spawned in that workspace start there unless a worktree path overrides it. The user sets the path inline on the workspace card, alongside the existing rename flow.

## Non-goals

- File picker dialog.
- Validating that the path exists (shell will surface the error; cheap to iterate later).
- `notify`-driven hot reload from `config.toml`.
- Per-tile cwd override distinct from the workspace default.
- Persisting `default_cwd` across restarts — out of scope until session persistence lands (§6 open question). The field is serde-ready, so adding it later is non-breaking.

## Design

### State

Add one field to `Workspace` in `kookaburra-core::state`:

```rust
pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
    pub layout: Layout,
    pub tiles: Vec<Tile>,
    pub primary_tile: Option<TileId>,
    pub default_cwd: Option<PathBuf>,   // new
}
```

`Workspace::new` initializes `default_cwd: None`. No other state shape changes.

### Actions

One new variant in `kookaburra-core::action::Action`:

```rust
SetWorkspaceCwd { id: WorkspaceId, cwd: Option<PathBuf> },
```

`apply_action` for `SetWorkspaceCwd` simply assigns. Empty-string input from the UI maps to `None`. Leading `~` is expanded by the UI before the action is emitted (using `directories::UserDirs`).

`PtySideEffects::spawn` gains a `cwd: Option<&Path>` parameter:

```rust
fn spawn(&mut self, tile_id: TileId, cwd: Option<&Path>, worktree: Option<&WorktreeConfig>) -> PtyId;
```

In `apply_action::SpawnInTile`, look up the owning workspace's `default_cwd` and pass it through. Worktree branch path (when `Phase 6` lands) takes precedence; the implementation of `PtyAdapter::spawn` picks worktree cwd first, then `cwd`, then falls back to process cwd.

For the current rough-draft (no worktrees wired up), `PtyAdapter::spawn` maps `cwd` directly into `SpawnRequest.cwd`.

### Inheritance on new workspaces

- `Action::CreateWorkspace` copies `default_cwd` from the currently-active workspace.
- `Action::MoveTileToNewWorkspace` copies `default_cwd` from the tile's source workspace.

Rationale: new scratchpads should inherit project context by default. The user can edit later.

### UI — inline editor

The existing `RenameState` in `kookaburra-ui` grows a second buffer:

```rust
struct RenameState {
    id: WorkspaceId,
    label_buffer: String,
    cwd_buffer: String,
    focus_requested: bool,
}
```

While `renaming` targets a workspace, `draw_rename_editor` replaces the card with a two-field vertical stack:

- Row 1: label `TextEdit` (current behavior — autofocused).
- Row 2: path `TextEdit` with placeholder `"path (e.g. ~/projects/foo)"`.

Card height grows from `CARD_HEIGHT` (48) to an "editor height" (~76) while editing. Width unchanged.

Key handling:
- `Enter` commits both fields (emits `RenameWorkspace` if label changed; emits `SetWorkspaceCwd` if path changed) and closes the editor.
- `Esc` cancels both.
- `Tab` / `Shift+Tab` move focus between fields.
- `focus_requested` initializes focus on the label field only; user tabs down to the path.

`~` expansion happens at commit time:
- `""` → `None`.
- `"~"` or `"~/..."` → expanded via `directories::UserDirs::home_dir()`.
- Anything else → `PathBuf::from(trimmed)`.

### Component boundaries

- **`kookaburra-core`** owns `Workspace::default_cwd`, the new `SetWorkspaceCwd` variant, the `apply_action` logic, and the signature change on `PtySideEffects::spawn`. No UI, no path expansion.
- **`kookaburra-ui`** owns the two-field editor, `~` expansion, and emitting the two actions on commit. No direct state writes.
- **`kookaburra-app`** updates `PtyAdapter::spawn` to forward `cwd` into `SpawnRequest`.
- **`kookaburra-pty`** unchanged — it already honors `SpawnRequest.cwd`.

### Error handling

- Path string parsing does not validate existence. If the path is bogus, the shell reports the failure in the tile; that's consistent with how a user would experience a bad `cd`.
- Empty string is a valid way to clear the default.

## Tests

Unit tests in `kookaburra-core`:

1. `SetWorkspaceCwd` updates the field.
2. `SetWorkspaceCwd` with `None` clears the field.
3. `SpawnInTile` forwards workspace `default_cwd` into the stub's `spawn` call.
4. `SpawnInTile` passes `None` when workspace has no default.
5. `CreateWorkspace` copies `default_cwd` from the previously-active workspace.
6. `MoveTileToNewWorkspace` copies `default_cwd` from the source workspace.

UI-level logic (home expansion, empty → None) is small enough to unit-test in `kookaburra-ui` via a pure helper (`expand_cwd_input(&str) -> Option<PathBuf>`).

## Risks

- `PtySideEffects::spawn` signature change ripples through every caller. Mitigated: only two implementers (`PtyAdapter`, `StubPty` in tests).
- Card height change during edit may jar users mid-type. Mitigated: height change is a one-time resize when the editor opens, not animated, and reverts on commit/cancel.

## Checklist impact (CLAUDE.md)

No direct tick applies. This nudges two existing Phase-5 items (config schema, workspace templates) but doesn't complete them. Worth a one-liner under Phase 3 "workspace rename inline" noting that the editor now also edits default cwd.
