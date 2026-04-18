# CLAUDE.md — Kookaburra working guide

Kookaburra is a fast, focused-mode terminal multiplexer for Claude Code sessions. Built in Rust with wgpu + alacritty_terminal + glyphon + egui. Targets macOS primarily.

**`KOOKABURRA.md` is the canonical spec.** Read it end-to-end before any non-trivial change. This file is the working guide: conventions, invariants, and a tickable implementation checklist.

---

## How we work

- Treat `KOOKABURRA.md` as the source of truth. If this file drifts, update it; if the spec is wrong, propose a spec edit before changing code.
- After finishing a checklist item below, tick the box in the same change that lands the work. Don't let the checklist drift from reality.
- Write tests alongside code, not after. Core layout math, action application, and config parsing must be unit-tested.
- Prefer editing existing files. No new docs files unless asked.
- **Architectural deadlock escape hatch:** if a new architectural design decision comes up and we can't reach consensus, check `docs/original_chat` for the original design intent before continuing. It records the reasoning behind decisions that pre-date the spec.

## Non-negotiables (from the spec)

- **Crate name:** `kookaburra` (lowercase). Binary: `kookaburra`. Display: `Kookaburra`.
- **Cargo workspace layout:** `crates/kookaburra-{core,pty,render,ui,app}`. Dependency direction is strict — no rendering in core, no UI in pty, no wgpu/winit/tokio in core.
- **Concurrency model:** main thread owns winit + wgpu + rendering; tokio owns PTY I/O; an `mpsc` channel connects them. PTY readers send dirty signals, never payload bytes.
- **Terminal state:** `Arc<parking_lot::FairMutex<Term<EventProxy>>>`. FairMutex is load-bearing — don't swap for `std::sync::Mutex`.
- **Strongly-typed IDs:** `WorkspaceId`, `TileId`, `PtyId` are newtypes around `u64`. Never pass raw ints.
- **Action pattern:** UI produces `Vec<Action>`; `apply_action(&mut AppState, &mut PtyManager, Action)` is the only mutation site. Keep it pure and testable.
- **Terminals are NOT drawn in egui.** egui draws strip/cards/dialogs only; terminals get their own wgpu pipelines in a single shared render pass.
- **Redraw strategy:** one app-wide `needs_redraw` flag + 60fps cap under pathological load. No per-tile dirty tracking.
- **Logo:** 1-bit pure white, `shape-rendering="crispEdges"`, transparent bg. No colors, gradients, or AA — ever.
- **Worktree ops:** shell out to `git`, not `git2`, for `worktree add/remove`. Auto-generated branch names must include a short random suffix to avoid collisions.
- **wgpu version:** pin it; don't upgrade casually mid-project.

## Tech stack reminders

See §2 of the spec for the full table. Short version: `alacritty_terminal`, `wgpu`, `winit`, `glyphon`, `egui` (+ `egui-wgpu`, `egui-winit`), `portable-pty`, `tokio`, `parking_lot`, `serde`+`toml`, `notify`, `arboard`, `directories`.

---

## Implementation checklist

Checklist mirrors §8 of the spec. Tick items as they land. Sub-items that aren't in the spec are expansions for tracking; don't add scope beyond what the spec calls for.

### Phase 0 — Repo bootstrap

- [x] `Cargo.toml` workspace root with member crates listed
- [x] `rust-toolchain.toml` pinning stable Rust
- [x] `.gitignore` (Rust + macOS + editor detritus)
- [x] `README.md` with logo at top
- [x] `assets/logo/kookaburra.svg` in place (1-bit, white, crispEdges)
- [ ] Raster derivatives: `kookaburra-{32,64,128,256,512}.png`
- [ ] `kookaburra.icns` (macOS) via `iconutil`
- [ ] `kookaburra.ico` (Windows)
- [ ] `kookaburra-linux.png` (512px)
- [x] CI: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`

### Phase 1 — Single tile end-to-end

- [x] Crate skeletons: `kookaburra-{core,pty,render,ui,app}` compile empty
- [x] `kookaburra-core::ids` — `WorkspaceId`, `TileId`, `PtyId` newtypes + generators
- [ ] `kookaburra-core::state` — `AppState`, `Workspace`, `Tile` structs
- [ ] `kookaburra-core::layout` — `Layout` enum + rect computation
- [ ] `kookaburra-core::action` — `Action` enum
- [ ] `kookaburra-core::config` — `Config` stub
- [ ] `kookaburra-core::worktree` — `Worktree` types (no impl yet)
- [ ] Unit tests: layout rect math for 1×1, 2×1, 1×2, 2×2, 3×2, 2×3
- [x] Unit tests: ID uniqueness
- [ ] Unit tests: basic `AppState` construction + tile insert/remove
- [ ] `kookaburra-pty`: spawn a PTY via `portable-pty`, read bytes, verify with stdout dump
- [ ] Wire `alacritty_terminal::Term` + parser; log grid on change
- [ ] `EventProxy` impl of `EventListener` forwarding to `mpsc`
- [ ] `kookaburra-render`: wgpu init, surface, single tile glyphon text + cursor + bg
- [ ] `CellMetrics` computed from 'M'/'0' at startup and on font change
- [ ] Color resolution: named / spec / indexed → theme palette (Tokyo Night default)
- [ ] Keyboard input → PTY writer
- [ ] Window resize → surface resize → PTY `TIOCSWINSZ` → `Term` grid resize (in that order, sync, main thread)
- [ ] **Exit criterion:** open app, run `vim` / `htop` in a single tile, it works

### Phase 2 — Multi-tile and layouts

- [ ] N tiles rendered from layout enum
- [ ] Focus model + keyboard focus switching (`Cmd+Opt+1..6`)
- [ ] Per-tile PTY resize on window resize
- [ ] Mouse click-to-focus
- [ ] Tile borders + focused-tile accent
- [ ] Inactive tile dimming (~10–15% opacity reduction)
- [ ] Layout preset switching via keybinding
- [ ] **Exit criterion:** 3×2 grid, click/keyboard focus, borders indicate focus

### Phase 3 — Strip and workspaces

- [ ] egui integration in render pipeline (`egui-wgpu` + `egui-winit`)
- [ ] Event routing: egui first → focused tile → terminal mouse → main loop; respect `wants_keyboard_input` / `wants_pointer_input`
- [ ] Blank `TopBottomPanel` strip (56px, logo 24×24 top-left)
- [ ] Cards (~140×48) with labels + active highlight + click-to-switch
- [ ] Multi-workspace state + `Cmd+1..9` keybinds
- [ ] Mini tile-activity indicators on cards
- [ ] "Claude is generating" subtle signal on cards
- [ ] Workspace rename inline (double-click label, `Cmd+L`)
- [ ] Drag to reorder workspaces
- [ ] Drag tile onto card → `Action::MoveTile` (the signature interaction)
- [ ] Drag tile onto empty strip → new workspace containing that tile
- [ ] `+` button to add workspace; close-workspace path
- [ ] Horizontal scroll when strip overflows
- [ ] **Exit criterion:** multiple workspaces, visual strip, tile drag between workspaces works

### Phase 4 — Terminal UX essentials

- [ ] Mouse text selection (single, word on double-click, line on triple-click)
- [ ] Selection wrapping semantics across soft-wrapped lines
- [ ] Selection into scrollback
- [ ] Rectangular selection
- [ ] Clipboard copy/paste via `arboard`
- [ ] OSC 52 clipboard request → `arboard`
- [ ] `Cmd+C` / `Cmd+V` semantics (passthrough when no selection)
- [ ] Scrollback: mouse wheel + keyboard
- [ ] `Cmd+F` in-tile search via `alacritty_terminal::RegexSearch`
- [ ] Bell handling + visual indicator
- [ ] OSC title changes → `Tile::title`
- [ ] OSC hyperlinks (render + click)
- [ ] New-output highlight (the "unread" edge pulse)

### Phase 5 — Polish and config

- [ ] TOML config load from XDG path via `directories`
- [ ] `notify`-based hot reload
- [ ] Keybinding system driven by config
- [ ] Theme system + external theme files
- [ ] Builtin themes: Tokyo Night, Catppuccin Mocha, Solarized Dark
- [ ] Font configuration + live switching
- [ ] Background font enumeration on startup (keep cold start fast)
- [ ] `Cmd+Enter` zen mode (maximize focused tile, hide strip)
- [ ] Output-aware dimming tuned
- [ ] Frame-budget cap (60fps under pathological load)

### Phase 6 — Worktrees

- [ ] `kookaburra-core::worktree` implementation
- [ ] New-tile dialog with worktree toggle (disabled when cwd isn't a repo)
- [ ] Branch name + base-ref prompt (dropdown from `git branch -a`)
- [ ] `git worktree add <path> -b <branch> <base>` via subprocess
- [ ] Short random suffix in auto-generated branch names
- [ ] PTY spawn with CWD = worktree path
- [ ] `git status --porcelain=v2 --branch` poll every 2–3s
- [ ] Branch + dirty indicator on tile + strip card
- [ ] Close-tile cleanup prompt: Keep / Remove / Copy-branch-and-remove
- [ ] Loud warning on dirty close; default Keep; force-remove needs confirm
- [ ] Orphan scan on startup (`git worktree list`) → cleanup offer, never auto-delete
- [ ] `Action::ForkTile` — new tile with new worktree branched from same base
- [ ] Document submodule + hooks caveats in README

### Phase 7 — Cross-tile and templates

- [ ] `Cmd+Shift+F` cross-tile search UI
- [ ] Workspace template format (TOML) + loader
- [ ] Template invocation UI (TBD: palette vs. menu vs. CLI arg — see §10)
- [ ] Follow mode per tile (toggle + auto-scroll behavior)
- [ ] Primary tile designation + default-focus behavior on workspace switch

### Phase 8 — Distribution

- [ ] macOS: code signing + notarization + DMG
- [ ] Linux: AppImage or deb/rpm
- [ ] Windows: MSI or portable exe
- [ ] Release CI that builds and uploads artifacts
- [ ] Auto-update: **deferred to v2**

---

## Open design questions (decide during implementation)

Tracked in spec §10. When one gets resolved, record the decision here with a one-liner and link the commit.

- [ ] Config schema shape (keybindings, themes, templates, worktree)
- [ ] Theme: single source for terminal + UI, or split? (spec leans single)
- [ ] Ligatures: v1 off, v2 config option?
- [ ] Template invocation surface
- [ ] Worktree merge-back UX (v1: just copy branch name)
- [ ] Session persistence scope — scrollback yes/no?
- [ ] Search: regex vs. plain text toggle
- [ ] Strip overflow beyond 9 — scroll confirmed; dropdown too?

## Risks to watch (spec §9)

Keep these in mind during review. Any PR touching the relevant area should call out how it handles the risk:

- Font rendering edge cases (emoji, CJK, powerline, ZWJ, combining)
- Mouse selection (2–3× longer than estimated, historically)
- macOS input quirks (dead keys, IME, Option-as-Meta)
- Pathological load (`yes`, `find /`, huge `cat`) — don't route bytes through channels
- wgpu version churn — pin it
- PTY resize signaling — both `TIOCSWINSZ` and `Term` grid, in order
- Font loading cold start — background thread
- Borrow checker on `AppState` — drain PTY events → UI produces actions → apply_action; resist `Arc<Mutex<_>>`
- Surface resize races — synchronous, main-thread, correct order
- Worktree orphans after crash — offer, never auto-delete
