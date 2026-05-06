# Kookaburra: roadmap to "serious" use

## Context

The user asked what's implemented and what's missing for Kookaburra to be used seriously day-to-day, citing "no settings yet" as an example. The CLAUDE.md checklist marks Phase 5 (config) done, and on a literal reading it is — TOML loader, XDG paths, hot-reload watcher, theme/keybinding live-swap all work. But the *configurable surface* is narrow (font family/size, theme, 8 keybindings) and several signature daily-driver capabilities (mouse text selection, in-tile search, PTY crash recovery, worktrees) are absent or stubbed. The user's intuition is correct: the app boots and renders cleanly but lacks the depth and ergonomics to be a serious replacement for iTerm/Ghostty/Wezterm.

This doc inventories what's done, what's missing organized by gap cluster, and proposes a sequenced roadmap across three scope tiers (personal daily-driver → share-ready release → spec-complete v1). The user has asked for the writeup; **no implementation should begin until they approve and select a tier or cluster to attack first**.

---

## What's solid today

Verified against the code, not just the checklist:

- **Architecture invariants:** `Arc<FairMutex<Term<EventProxy>>>` (`crates/kookaburra-pty/src/lib.rs:26`), main thread owns winit/wgpu, tokio owns PTY I/O, `mpsc` carries dirty signals not bytes. `apply_action` is the single mutation site (`crates/kookaburra-core/src/action.rs`). Strongly-typed `WorkspaceId`/`TileId`/`PtyId` newtypes throughout.
- **Phase 1–3 surface:** wgpu surface, glyphon text rendering, click-to-focus, multi-tile layouts (1×1 → 2×1 → 2×2 → 3×2 cycle on `Cmd+G`), workspace strip with cards, drag-tile-onto-card, drag-to-reorder workspaces, inline rename (`Cmd+L`), middle-click to delete, drop-on-empty-strip-creates-workspace, breathing "unread" dot, "Claude is generating" three-dot signal, click particle puffs, bell flash header.
- **Config plumbing:** `notify` watcher on the parent dir (so editor rename-and-replace fires), live theme + keybinding swap, four builtin themes (Kookaburra default, Tokyo Night, Catppuccin Mocha, Solarized Dark), inline-palette and external-file theme support.
- **`kookaburra-core` is well-tested** — ~122 unit tests covering action application, layout math, config parsing, keybinding resolution, state transitions.

---

## Cluster A — Phase 4 terminal essentials

The single biggest day-to-day usability delta. Without these, Kookaburra cannot replace a real terminal.

### A1. Mouse text selection

**Status:** Unimplemented. The blocker for several other Phase 4 items.

**Required:**
- Single-click drag → cell-range selection.
- Double-click → word selection (alacritty-style word boundaries).
- Triple-click → line selection.
- Shift-click → extend existing selection.
- Selection wraps correctly across soft-wrapped lines (alacritty `Term::semantic_search_left/right` and `Selection` types).
- Selection extends into scrollback when dragging past viewport edge.
- Rectangular selection on Alt-drag.
- Visible highlight: requires a wgpu quad/background pipeline. **This is the same pipeline that's needed for proper tile borders (Phase 2 partial item)** — build once, use twice.

**Critical files:**
- New: `crates/kookaburra-core/src/selection.rs` — wraps `alacritty_terminal::selection::Selection` semantics in our snapshot model.
- `crates/kookaburra-render/src/lib.rs` — add a quad pipeline alongside the glyphon text pipeline. Selection cells render with `theme.selection_bg`; existing text re-renders on top with `theme.selection_fg` (or unchanged fg).
- `crates/kookaburra-app/src/main.rs` — mouse drag state machine on tiles (when not consumed by terminal mouse mode), translate viewport-pixel → cell coords using existing `CellMetrics`.
- `crates/kookaburra-core/src/action.rs` — `Action::SetSelection`, `Action::ClearSelection`, `Action::ExtendSelection`.

**Reuse:** alacritty_terminal already exports `Selection`, `SelectionType`, and `Term::selection_to_string`. Don't re-implement word boundaries — use what alacritty ships.

### A2. Selection-aware copy

**Status:** Stand-in only. `Cmd+C` currently copies the visible grid via `arboard`.

**Required:** Once selection lands, `Cmd+C` (and middle-click on platforms where that means copy) reads `Term::selection_to_string()` and writes via `arboard`. Visible-grid fallback should still exist when selection is empty (it's surprisingly useful for Claude Code review work).

**Critical files:** `crates/kookaburra-app/src/main.rs` — replace the current visible-grid copy at the existing `Cmd+C` handler.

### A3. In-tile search (`Cmd+F`)

**Status:** `Action::OpenSearch` is a no-op handler at `crates/kookaburra-core/src/action.rs:316–318`. No UI, no regex wiring.

**Required:**
- Small egui search bar overlaid at the bottom of the focused tile when `Cmd+F` is pressed (egui is already in the render pipeline; this is a thin floating panel, not part of the strip).
- Plain text match by default, regex toggle (decision item — see "Cross-cutting decisions").
- `alacritty_terminal::Term::search_next` / `RegexSearch` for the actual matching.
- Match highlighting reuses the same quad pipeline from A1.
- `Enter` jumps next, `Shift+Enter` jumps previous, `Esc` closes.
- Search scope: visible viewport + scrollback.

**Critical files:**
- `crates/kookaburra-ui/src/lib.rs` — new `SearchBar` widget. State (query, current match index) lives in `Tile` or a new `TileEphemeralState` so it doesn't survive tile close.
- `crates/kookaburra-core/src/action.rs` — flesh out `OpenSearch`, add `Action::SearchNext`, `SearchPrev`, `CloseSearch`.
- `crates/kookaburra-pty/src/lib.rs` — expose a snapshot helper that runs the regex search with `Term` locked.

### A4. OSC 8 hyperlinks

**Status:** Not wired. `alacritty_terminal` recognizes them and stores hyperlink IDs on cells.

**Required:**
- Read hyperlink data off snapshot cells.
- Render with underline + a configurable accent color (or just the existing `theme.link`).
- Hover (with a small tracking delay) shows the URL in a tooltip near the cursor.
- `Cmd+click` opens via the OS (`open` on macOS, `xdg-open` on Linux, `start` on Windows). Plain click does nothing — preserves selection drag.

**Critical files:**
- `crates/kookaburra-core/src/snapshot.rs` — add hyperlink id/url to `RenderCell` if not already present; check first.
- `crates/kookaburra-render/src/lib.rs` — underline pass for hyperlink cells (might be a thin quad under the text or an underline glyph; reuse the new quad pipeline).
- `crates/kookaburra-app/src/main.rs` — Cmd+click hit-test, `std::process::Command::new("open")`.

### A5. OSC 52 clipboard

**Status:** Not wired.

**Required:** When a PTY emits OSC 52 set-clipboard, write to `arboard`. Off by default per security best practices? Or on by default? **Decision item.** OSC 52 read requests should be ignored or prompted (decision item).

**Critical files:**
- `crates/kookaburra-pty/src/lib.rs` — `EventProxy` already handles a few OSC events; add OSC 52 handling there. The alacritty `EventListener` trait may surface this directly.

### A6. New-output edge pulse

**Status:** Not wired. `Tile::has_new_output` exists; only the strip card uses it (breathing dot). The tile itself doesn't pulse.

**Required:** Subtle 1px accent edge on inactive tiles when their `has_new_output` flips, fading over ~1.2s. Reuses the quad pipeline from A1.

**Critical files:** `crates/kookaburra-render/src/lib.rs` — once quad pipeline exists, this is ~30 lines.

### Cluster A summary

Order within the cluster: **A1 first** (unblocks A2, A3, A4, A6 by establishing the quad pipeline). Then A2 (cheapest payoff once A1 lands). Then A3 (search is high-value but requires its own UI work). A4–A6 in any order, all small once the pipeline exists.

---

## Cluster B — Settings depth + window UX

Addresses the user's "no settings yet" intuition directly. Splits cleanly into config schema additions and ergonomic affordances.

### B1. Config schema additions

`crates/kookaburra-core/src/config.rs` needs new fields. Each addition needs: serde default, validation, and a hot-reload path.

| Field | Type | Default | Hot-reloadable? |
|---|---|---|---|
| `cursor.style` | `Block` / `Beam` / `Underline` | Block | Yes |
| `cursor.blink` | `bool` | true | Yes |
| `cursor.blink_interval_ms` | `u32` | 500 | Yes |
| `scrollback.lines` | `u32` | 10000 | No (alacritty `Term` rebuild) |
| `padding.window_inset_px` | `f32` | 8.0 | Yes (relayout) |
| `padding.tile_gap_px` | `f32` | 6.0 | Yes (relayout) |
| `shell.program` | `String?` | `$SHELL` | No (existing PTYs unchanged) |
| `shell.args` | `Vec<String>` | `[]` | No |
| `shell.cwd` | `String?` | inherit | No |
| `shell.env` | `HashMap<String, String>` | `{}` | No |
| `bell.flash_ms` | `u32` | 150 | Yes |
| `bell.flash_color` | `Color?` | `theme.ansi[1]` | Yes |
| `bell.audible` | `bool` | false | Yes |
| `clipboard.copy_on_select` | `bool` | false | Yes |
| `clipboard.osc_52_enabled` | `bool` | false (security) | Yes |
| `window.opacity` | `f32` (0.0–1.0) | 1.0 | macOS-conditional |
| `window.decorations` | `bool` | true | Restart-required |
| `font.ligatures` | `bool` | false | No (font-system rebuild) |

**Hardcoded constants to remove:** `WINDOW_INSET_PX = 8.0`, `TILE_GAP_PX = 6.0` (`crates/kookaburra-app/src/main.rs:41`), `DEFAULT_WIDTH/HEIGHT = 1400/900` (`main.rs:38`), bell flash duration in main.rs.

**Live-reload mechanics:** the existing config watcher already fires `AppEvent::ConfigReloaded`. Extend the diff handler to detect each field's change and either apply immediately or queue a "restart-required" toast (see B5).

### B2. Font live-switching

**Status:** Currently broken. `crates/kookaburra-render/src/glyph_pipeline.rs:19` says `set_font` / `set_scale_factor` "held for later phases." Hot-reload logs "restart required."

**Required:** On `font.family` or `font.size_px` change in `AppEvent::ConfigReloaded`, rebuild `LoadedFont` and `glyphon::TextRenderer` against the new face/size, recompute `CellMetrics`, fire a window relayout. PTYs need `TIOCSWINSZ` resends because cell-grid dimensions change with font size.

**Critical files:**
- `crates/kookaburra-render/src/glyph_pipeline.rs` — implement `set_font`, `set_scale_factor`.
- `crates/kookaburra-app/src/main.rs` — config-reload handler must trigger the rebuild and resize sequence.

### B3. CLI args

**Status:** None.

**Required:** Add `clap` (or hand-rolled — there are few enough args). Surface:
- `kookaburra -e <command>` / `--exec <command>` — run command in initial tile instead of shell.
- `kookaburra -d <dir>` / `--cwd <dir>` — set initial cwd.
- `kookaburra --config <path>` — override XDG config path.
- `kookaburra --workspace <name>` — start with a named workspace from a template (depends on Phase 7 templates; can stub now and hook later).
- `kookaburra --version`, `--help` (clap gives these free).

**Critical files:** `crates/kookaburra-app/src/main.rs` — parse before window creation, thread into initial `SpawnRequest`. Add `clap` to `kookaburra-app`'s `Cargo.toml`.

### B4. Window size + position persistence

**Status:** Hardcoded `1400×900`, position OS-default.

**Required:** On window close, write `<xdg>/kookaburra/state.toml` with last window size, position, and active workspace id. On startup, read it; fall back to defaults if missing/malformed.

**Critical files:**
- New: `crates/kookaburra-core/src/persisted_state.rs` — small `PersistedState` struct, load/save, separate file from config (config = user authored, state = app authored).
- `crates/kookaburra-app/src/main.rs` — read on startup, write on `WindowEvent::CloseRequested` and periodically (every 30s, debounced) so a crash doesn't lose state.

### B5. In-app help overlay

**Status:** None. Users discover keybindings only by reading `config.rs` defaults or their own `config.toml`.

**Required:** A modal egui overlay triggered by `?` or `Cmd+/` (configurable). Lists current keybindings grouped by category (workspace, tile, layout, search, copy/paste). Reads from the live `ResolvedKeybindings`, so it reflects user customization. Includes a "settings file" hint with the actual XDG path so users know where to edit.

**Critical files:**
- `crates/kookaburra-ui/src/lib.rs` — new `HelpOverlay` widget.
- `crates/kookaburra-core/src/keybinding.rs` — expose a `human_readable` for each binding (e.g., "⌘N" instead of `super+n`).

### B6. Status / error toast surface

**Status:** Errors logged to stderr only. Config parse failure, font load failure, clipboard failure → silent UI.

**Required:** A small toast queue rendered as an egui overlay near the bottom. Categories: `Info` (config reloaded), `Warn` (binding parse failed, font family not installed), `Error` (config malformed, clipboard write failed). Auto-dismiss after 4–6s; stack up to 3.

**Critical files:**
- `crates/kookaburra-ui/src/lib.rs` — new `ToastQueue`.
- `crates/kookaburra-app/src/main.rs` — sites currently calling `log::warn!`/`log::error!` on user-influenced paths should also enqueue a toast.

### B7. First-run guidance

**Status:** App boots silently; users have to know `<xdg>/kookaburra/config.toml` exists.

**Required:** On startup, if the config file does not exist, write a fully-commented default config to the XDG path (or a sibling `config.example.toml`). Log a single line at startup with the resolved config path. Optionally show a one-time toast: "Config: <path>. Press ? for help."

**Critical files:** `crates/kookaburra-core/src/config.rs` — `Config::ensure_default_written()` helper.

### Cluster B summary

Order: B1 (schema) before B2 (font live-switch needs the size field plumbed). B5 (help overlay) and B6 (toasts) pair well — both are egui widget work. B3 (CLI), B4 (state persistence), B7 (first-run) are independent and can land in any order, each is a 1–3 hour task.

---

## Cluster C — Stability + crash recovery

Makes the app robust enough for someone other than the author to try, and removes most "thread panicked" surprises.

### C1. Render init graceful degradation

**Status:** Three `.expect()` calls at `crates/kookaburra-render/src/lib.rs:190–204` on wgpu surface/adapter/device creation. Headless context, broken drivers, or unsupported backends → process panics with a Rust backtrace.

**Required:**
- Convert each `.expect()` to `?` and bubble through a new `RenderInitError` enum.
- Top-level catches in `main.rs` show a native-OS error dialog (use `winit`'s `MessageDialog` or a small `tinyfiledialogs` dependency) before exiting cleanly.
- Adapter selection retries low-power if high-performance fails, then software fallback if available.

**Critical files:** `crates/kookaburra-render/src/lib.rs`, `crates/kookaburra-app/src/main.rs:1043,1264`.

### C2. Font load fallback

**Status:** `LoadedFont::from_font_system(...).expect("no monospace font available")` at `crates/kookaburra-render/src/lib.rs:234` panics if no monospace font is installed.

**Required:**
- Three-tier fallback: requested family → any monospace family → bundled `JetBrainsMono` (or similar) embedded in the binary as a backstop.
- Bundle adds ~250 KB to binary size; acceptable price for "always boots."
- Toast (via B6) when fallback fires: "Font 'Foo' not found, using JetBrainsMono."

**Critical files:** `crates/kookaburra-render/src/glyph_pipeline.rs`, `crates/kookaburra-render/Cargo.toml` (add font asset), `assets/fonts/` (new).

### C3. PTY crash recovery

**Status:** When a PTY's `ProcessExited` fires, `crates/kookaburra-app/src/main.rs:351–353` logs and drops. The tile becomes a darkened frozen square forever.

**Required:**
- New `Tile` state: `TileLifecycle::{Live, Exited(ExitStatus), Restarting}`.
- Visual indicator on exited tiles: dim further, header shows "exited (code 1) — press R to restart, X to close".
- `R` (or click a small button) re-spawns the PTY with the same shell/cwd/env config. Original scrollback can be preserved as a "previous session" header, or cleared (decision item — preserving feels right; matches tmux respawn semantics).
- `X` closes the tile (existing `Action::CloseTile`).

**Critical files:**
- `crates/kookaburra-core/src/state.rs` — extend `Tile` with lifecycle field.
- `crates/kookaburra-core/src/action.rs` — `Action::RestartTile`.
- `crates/kookaburra-app/src/main.rs` — handle `ProcessExited` → flip state, request redraw; handle `RestartTile` → respawn via `PtyManager`.
- `crates/kookaburra-render/src/lib.rs` — render exit-state header.

### C4. Action hot-path unwrap audit

**Status:** ~25 `.unwrap()` calls in `crates/kookaburra-core/src/action.rs` on workspace/tile lookups. Safe today *if* `apply_action` is the only mutation site and never given a stale id. But a logic bug becomes a hard crash.

**Required:**
- Convert each lookup to `if let Some(...)` with a `log::warn!` on miss describing the action and id.
- Consider adding a feature flag `strict_actions` that *does* panic in debug builds (catches bugs in tests) but returns safely in release.
- Corresponding unit tests: every action handler should have a "stale id is a no-op" test.

**Critical files:** `crates/kookaburra-core/src/action.rs` only. Pure refactor + tests.

### C5. Config error surfacing

**Status:** `crates/kookaburra-core/src/config.rs:408–410` and `crates/kookaburra-core/src/keybinding.rs:174` log warnings then fall back. Users learn their config is broken via stderr.

**Required:**
- `Config::load_or_default` returns `(Config, Vec<ConfigDiagnostic>)` instead of just `Config`.
- Diagnostics get fed into the toast queue (B6) on startup and on each hot-reload.
- `Diagnostic` carries severity, file path, line number (when toml parse error has one), human description.

**Critical files:** `crates/kookaburra-core/src/config.rs`, `crates/kookaburra-core/src/keybinding.rs`, plus toast wiring in app.

### C6. PTY reader thread panic recovery

**Status:** If the PTY reader thread panics (corrupt term state, OOM in vte parser), the app crashes silently — no log, just thread death. Untested.

**Required:**
- `std::thread::Builder::spawn` with explicit name; wrap reader body in `catch_unwind`.
- On unwind, send a `PtyEvent::ReaderPanicked { tile_id }` to main; treat the same as `ProcessExited` so C3's restart flow handles it.

**Critical files:** `crates/kookaburra-pty/src/lib.rs` reader spawn site.

### Cluster C summary

Order: C1 + C2 are pre-flight (without these, the app can't even reach a usable state on some machines). C3 + C6 belong together (PTY death and reader panics share the recovery path). C4 + C5 are independent cleanups.

---

## Cluster D — Worktrees (Phase 6)

The signature differentiating feature per the spec. Currently 46 lines of types in `crates/kookaburra-core/src/worktree.rs` and two no-op action handlers (`ForkTile` at `action.rs:305–307`, `SetWorktreeMode` at `action.rs:332–335`).

### D1. Subprocess layer

**Required:** A `WorktreeOps` module that shells out to `git` (per the non-negotiable in CLAUDE.md — no `git2`). Functions: `add(repo, path, branch, base)`, `remove(repo, path, force)`, `list(repo)`, `status(path)` (using `git status --porcelain=v2 --branch`).

- Each invocation in a `tokio::task::spawn_blocking` with a 10s timeout (worktree add can be slow on big repos with hooks; bigger budget needed in practice — TBD).
- Return rich error types: `GitNotFound`, `NotARepo`, `BranchExists`, `WorktreeAlreadyExists`, `WorkingTreeDirty`, `Other(String)`.
- Branch name auto-generation: `<base>-<short-random-suffix>` per the non-negotiable. Use `rand::random::<u32>()` and base32-encode 5 chars to avoid ambiguity.

**Critical files:** New `crates/kookaburra-pty/src/worktree.rs` (PTY crate already shells out for spawning; this fits there) or new dedicated crate. Likely PTY is the right home — both are subprocess work. If it grows large, split later.

### D2. New-tile worktree dialog

**Required:**
- When `Cmd+T` is pressed *and* the current cwd is inside a git repo (detect via `git rev-parse --is-inside-work-tree`), show an egui modal: "New tile" with options:
  - Plain (no worktree) — default if not in a repo.
  - Worktree on new branch — branch name (prefilled with auto-generated suffix), base ref (dropdown from `git branch -a`), worktree path (defaulted to `<repo>/.worktrees/<branch>` but editable).
- On confirm: invoke D1 add, on success spawn PTY with cwd = worktree path.
- Modal disabled (toggle grayed out) when outside a repo.

**Critical files:**
- `crates/kookaburra-ui/src/lib.rs` — new `NewTileDialog`.
- `crates/kookaburra-core/src/action.rs` — extend `Action::CreateTile` to carry an optional `WorktreeRequest`.
- `crates/kookaburra-app/src/main.rs` — dispatch path that runs D1 add before spawn.

### D3. Status polling + indicators

**Required:**
- Background tokio task per worktree-tile, runs every 2.5s, executes `git status --porcelain=v2 --branch`, parses, sends `PtyEvent::WorktreeStatus { tile_id, branch, ahead, behind, dirty }` to main.
- Tile header chip displays the branch name (truncated to 20 chars) and a • dot when dirty. Strip card shows a tiny branch icon next to the workspace name when any tile in the workspace has worktree status.
- Polling stops when the tile closes.

**Critical files:**
- `crates/kookaburra-pty/src/worktree.rs` — `poll_status` task.
- `crates/kookaburra-core/src/state.rs` — `Tile::worktree_status: Option<WorktreeStatus>`.
- `crates/kookaburra-render/src/lib.rs` — header chip rendering.

### D4. Close-tile cleanup prompt

**Required:**
- When a worktree-tile is closed via `Cmd+W` or middle-click, show an egui modal: "Close tile":
  - **Keep worktree** (default — the safe option).
  - **Remove worktree** (only enabled if clean; runs `git worktree remove <path>`).
  - **Copy branch name to clipboard, then remove worktree.**
  - **Cancel.**
- If the worktree is dirty, the "Remove" buttons require an additional explicit confirmation ("This will discard uncommitted changes. Are you sure?"). Force flag passed to D1.

**Critical files:** `crates/kookaburra-ui/src/lib.rs` for the dialog, `crates/kookaburra-app/src/main.rs` to interpose on the close path.

### D5. Orphan scan on startup

**Required:**
- On app startup, for each repo a tile *previously* used as a worktree (tracked in B4's `state.toml`?), run `git worktree list` and identify any worktrees we created (heuristic: in `.worktrees/` subdir, or marked with our prefix) that are no longer represented by an active tile.
- Show a one-time dialog or persistent toast: "N orphan worktrees found. Clean up?" with details.
- Never auto-delete (per non-negotiable). Always offer.

**Critical files:**
- `crates/kookaburra-core/src/persisted_state.rs` (from B4) — track repos with worktree history.
- `crates/kookaburra-ui/src/lib.rs` — orphan dialog.

### D6. `Action::ForkTile`

**Required:**
- New tile spawned with a new worktree, branched from the same base ref as the source tile's worktree. Useful for "I want a parallel branch off the same base."
- Triggered from a hypothetical right-click menu (deferred — the menu itself is a future ask), or from the keybinding `Cmd+Shift+T` on a focused worktree tile.

**Critical files:** `crates/kookaburra-core/src/action.rs:305–307` becomes a real implementation; calls D1 add then spawn.

### D7. Submodules + hooks documentation

**Required:** A README section noting that worktrees with submodules and post-checkout hooks may behave unexpectedly. Not implementation, but ships with the feature so users aren't surprised.

### Cluster D summary

Order: D1 (subprocess core) blocks everything else. Then D2 (creation flow) is the user-facing milestone. D3 (status) is high-value polish that can land same week. D4 (cleanup) is required before this can be considered "shippable" — without it, users accumulate orphans. D5 + D6 are bonuses. D7 is a README edit.

---

## Cross-cutting decisions to settle before implementation

These don't have implementation cost on their own but block clean specs:

1. **OSC 52 default.** Off (security default — paste injection risk) or on (convenience)? Recommend off-by-default with a config flag.
2. **Search regex toggle.** Plain text default with a `.*` toggle, or always-regex with sane escaping? Recommend plain default with toggle.
3. **PTY restart preserves scrollback?** Recommend yes — matches tmux respawn-pane semantics. Optional `--clear` flag on the action.
4. **OSC 52 read requests.** Ignore silently, prompt, or always-deny? Recommend ignore with a debug-level log.
5. **Tile borders.** Build the quad pipeline and render proper 1px focused-tile accent (per spec §3) — yes, since A1 already requires the pipeline.
6. **Cmd+1..9 workspace switching.** The user's recent CLAUDE.md edit confirms "intentionally removed." Ratify: spec §3 needs an edit to match.
7. **Strip overflow.** Spec §10 leaves "scroll vs dropdown" open. Recommend keep scroll-only for v1, defer dropdown.
8. **Ligatures.** Default off, expose `font.ligatures = true` in config (B1) for users who want them. Defer the actual rendering tuning until someone reports a problem.
9. **Worktree path convention.** `<repo>/.worktrees/<branch>` or sibling directory? Recommend `<repo>/.worktrees/<branch>` — easier to gitignore, easier to clean up.

---

## Sequenced roadmap by scope tier

### Tier 1 — Personal daily-driver

You replace iTerm/Ghostty with Kookaburra for your own Claude Code work. Nobody else needs to install it yet.

**Recommended order:**
1. **C1 + C2** — Render init + font fallback. Without these the app may not even start reliably on different machines / display configs. Half a day combined.
2. **A1** — Mouse text selection + quad pipeline. Unblocks everything else in A and resolves the spec-vs-code "tile borders" partial. **Single biggest day-to-day delta.** 1–2 days.
3. **A2** — Selection-aware copy. ~1 hour after A1.
4. **A3** — In-tile search. 1 day.
5. **C3 + C6** — PTY crash recovery + reader panic recovery. Without this a single bad shell command can leave a frozen tile mid-session. 1 day.
6. **B1 partial** — Just the fields the user feels missing: `cursor.style`, `cursor.blink`, `scrollback.lines`, `padding.*`, `shell.*`. ~1 day.
7. **B4** — Window size persistence. ~2 hours.
8. **B5** — Help overlay. ~3 hours, unlocks easier exploration of the rest.
9. **B6** — Toast queue. ~3 hours, paired with C5 below.
10. **C5** — Config error surfacing into toasts. ~2 hours after B6.

**Estimated calendar:** 1.5–2 weeks of focused work.

**Tier 1 exit criterion:** can use Kookaburra exclusively for a week of Claude Code work, including reviewing long output (selection + search), without the app crashing or losing state, without needing to restart for config changes (theme/keybindings live; cursor + padding live; font requires restart but documented).

### Tier 2 — Share-ready release

Friends or coworkers can install and use without coaching.

**On top of Tier 1, add:**
1. **A4** — OSC 8 hyperlinks. Modern shells emit them; absence is jarring.
2. **A5** — OSC 52 clipboard (off by default).
3. **A6** — New-output edge pulse.
4. **B1 remainder** — Remaining config fields: `bell.*`, `clipboard.*`, `window.opacity`, `font.ligatures`.
5. **B2** — Font live-switching. Without this, users editing `config.toml` and not seeing font changes will think the config is broken.
6. **B3** — CLI args. `kookaburra -e claude` is the natural invocation.
7. **B7** — First-run guidance: write commented default config, log path.
8. **C4** — Action hot-path unwrap audit. Reduces "thread panicked" surface area for unfamiliar users.
9. **Phase 0 packaging:** raster icon derivatives (`kookaburra-{32,64,128,256,512}.png`, `.icns`, `.ico`, Linux 512px PNG).
10. **Phase 8 minimum:** macOS code signing + notarization + a signed DMG with the icon. README install instructions. (Linux + Windows can wait.)

**Estimated calendar:** an additional 1.5–2 weeks.

**Tier 2 exit criterion:** a non-author can download a DMG, drag to Applications, launch, and use Kookaburra for an afternoon without hitting a "thread panicked," without confused "where's the settings file" question, and without needing the author to explain anything beyond "press ? for help."

### Tier 3 — Spec-complete v1

Every Phase 4–7 item per KOOKABURRA.md.

**On top of Tier 2, add:**
1. **Cluster D entire** — Worktrees (D1 → D2 → D3 → D4 → D5 → D6 → D7). The signature differentiating feature; this is what makes Kookaburra not "just another terminal." 2–3 weeks.
2. **Phase 7** — Cross-tile search (`Cmd+Shift+F`), workspace templates (TOML format + loader), follow mode per tile, primary-tile UI.
3. **Phase 5 polish** — Background font enumeration, output-aware dimming tuning, frame-budget cap (60fps under pathological load).
4. **Phase 8 full** — Linux AppImage/deb/rpm, Windows MSI/portable, release CI uploading artifacts.

**Estimated calendar:** an additional 4–6 weeks.

**Tier 3 exit criterion:** every checkbox in CLAUDE.md is `[x]` (Phases 0–7 minimum; auto-update is explicitly v2 per spec).

---

## Critical files to know

| File | Role |
|---|---|
| `crates/kookaburra-core/src/action.rs` | Single mutation site. Every new behavior adds a variant + a handler here. |
| `crates/kookaburra-core/src/config.rs` | Schema + load. All Cluster B field additions go here. |
| `crates/kookaburra-core/src/keybinding.rs` | Resolved keybindings; needed for B5 help overlay. |
| `crates/kookaburra-core/src/state.rs` | `AppState`, `Workspace`, `Tile`. Lifecycle, worktree status, search state extensions land here. |
| `crates/kookaburra-app/src/main.rs` | Event loop. Mouse drag state machines, config-reload diff handling, PTY exit handling, CLI parsing. |
| `crates/kookaburra-render/src/lib.rs` | wgpu pipelines. New quad pipeline for selection/borders/highlights belongs here. |
| `crates/kookaburra-render/src/glyph_pipeline.rs` | Font + text rendering. B2 font live-switch lands here. |
| `crates/kookaburra-pty/src/lib.rs` | PTY spawn, reader thread, OSC events. C6 reader-panic catch and A5 OSC 52 land here. |
| `crates/kookaburra-ui/src/lib.rs` | egui widgets. Help overlay, toast queue, search bar, new-tile dialog, worktree close dialog all land here. |

---

## Verification plan

End-to-end checks per cluster, runnable when each cluster lands:

**Cluster A:** Open a tile, run `seq 1 1000`, drag-select 50 lines, `Cmd+C`, paste into another tile — output matches. Open a tile, `Cmd+F`, type a regex, navigate matches, close. Open a tile that emits OSC 8 hyperlinks (e.g., `gh issue list`) — links underlined, Cmd+click opens browser.

**Cluster B:** Edit `config.toml` to set `cursor.style = "Beam"` — cursor changes shape live without restart. Edit `font.size_px` to 18 — font scales live, tiles relayout, `TIOCSWINSZ` fires (verify with `stty size` inside a tile). Run `kookaburra -e htop -d /tmp` — htop launches in `/tmp`. Resize window to 1800×1200, close app, relaunch — opens at 1800×1200. Press `?` — help overlay shows current keybindings.

**Cluster C:** Run `kookaburra` on a system with no Metal/Vulkan adapter — exits cleanly with a dialog, not a panic. Inside a tile, run `exec false` — tile shows "exited (1) — press R to restart"; press R — new shell, scrollback preserved with separator. Edit `config.toml` to invalid TOML — toast appears with line number; previous config remains active.

**Cluster D:** In a git repo, `Cmd+T` → new-tile dialog appears with worktree toggle enabled, default branch name has random suffix. Confirm with `master` as base — new tile spawns in `.worktrees/<branch>/`, header shows branch name. Edit a file in the worktree — • dot appears within ~3s. `Cmd+W` → cleanup dialog; pick "Copy branch + remove" — branch name on clipboard, worktree gone. Force-quit the app mid-worktree-session, relaunch — orphan dialog offers cleanup of the still-present worktree.

---

## Out of scope for this doc

- **Phase 7** in detail (cross-tile search, templates, follow mode) — left at a high level; deserves its own doc when Tier 3 begins.
- **v2 features** — auto-update, ligature rendering tuning, session persistence beyond layout, full Linux/Windows packaging.
- **Spec edits** — several decisions above (e.g., ratifying Cmd+1..9 removal, tile-border resolution) imply small KOOKABURRA.md edits. Those should land in the same change as the implementation, per the existing rule in CLAUDE.md.
