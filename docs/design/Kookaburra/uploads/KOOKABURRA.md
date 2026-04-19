# Kookaburra — Design & Implementation Handoff

> A fast, focused-mode terminal multiplexer for Claude Code sessions. Built in Rust with wgpu + alacritty_terminal + glyphon + egui. Targets macOS primarily, with cross-platform support coming along for free.

This document is the complete design handoff from the initial planning conversation. It is intended to be read end-to-end before writing code. Every significant decision has a rationale attached so future-you (or Claude Code) can revisit choices with context.

---

## 1. Product vision

**Kookaburra is a focused-mode app for running multiple Claude Code sessions (and supporting terminals) in parallel, with a spatial UI that makes parallel work tangible.**

The defining use case: a developer wants to run several Claude Code sessions at once — one refactoring the frontend, one exploring a library, one running tests — each in its own context, each ideally in its own git worktree so they don't collide. Today this is painful: tmux is keyboard-heavy and text-only, iTerm tabs don't give you spatial overview, and window managers don't understand that "these three terminals belong together."

Kookaburra organizes work into **workspaces** (groups of related terminals), displays them in a **tiled grid** (default 3×2), and surfaces them all via a **top strip of cards** you can click or drag between. Each tile can optionally be a **git worktree**, making "let me try three approaches to this problem" a first-class workflow.

### Non-goals for v1

- Not a tmux replacement. No session persistence across restarts (layout yes, processes no).
- Not a general window manager. It manages tiles inside its own window, not OS windows.
- Not a fancy IDE. No file tree, no editor integration. It's terminals, organized.
- Not infinitely configurable. Opinionated defaults beat a thousand knobs.

### Design values, in priority order

1. **Speed** — instant cold start, 60+ fps, zero idle CPU. Every architectural decision defers to this.
2. **Focus** — the UI should disappear when you're working and reappear when you need it.
3. **Spatial clarity** — parallel work should feel parallel, not stacked.
4. **Honest defaults** — ship with good themes, good keybindings, good fonts. No "configure this to be useful."
5. **Cross-platform, but macOS-first** — native-feeling on macOS; functional on Linux and Windows.

### Name and branding

**Kookaburra.** The bird is loud, focused, unmistakably recognizable by silhouette — fitting vibe for a focused-mode terminal. The crate name throughout the codebase is `kookaburra` (lowercase); the binary is `kookaburra`; the display name is `Kookaburra`.

**Logo.** 1-bit pixel-art kookaburra in profile, pure white on transparent. Rendered on an 8-pixel grid — the low-fi pixel aesthetic is intentional and reads as "terminal-native." Key silhouette features: oversized head, characteristically long/heavy beak (the single most recognizable kookaburra feature), compact body, tail feathers, perched on a branch. A single pixel gap in the head creates the eye.

The canonical logo asset lives at `assets/logo/kookaburra.svg`. It must always render as pure white (`#ffffff`) with no anti-aliasing (`shape-rendering="crispEdges"`), with transparent background. Do not add colors, gradients, or shading — 1-bit is load-bearing for the brand.

Required asset derivatives (generate from the SVG):
- `assets/logo/kookaburra.svg` — master, 320×320 viewBox
- `assets/logo/kookaburra-32.png`, `-64.png`, `-128.png`, `-256.png`, `-512.png` — raster sizes for window icon and docs
- `assets/logo/kookaburra.icns` — macOS app icon bundle (generate via `iconutil`)
- `assets/logo/kookaburra.ico` — Windows icon
- `assets/logo/kookaburra-linux.png` — 512px for Linux desktop entries

The logo appears in: the macOS Dock/app icon, the window's title bar icon, the "About" dialog, and small-scale in the top-left corner of the strip (24×24px) as a subtle brand anchor.

---

## 2. Technology stack

### Core crates

| Crate | Purpose | Notes |
|---|---|---|
| `alacritty_terminal` | Terminal state machine, VT100/xterm parser, grid model | The hard stuff (escape sequences, scrollback, modes) done for us |
| `wgpu` | GPU abstraction layer | Metal on macOS, Vulkan on Linux, DX12 on Windows |
| `winit` | Windowing and input event loop | Standard pairing with wgpu |
| `glyphon` | GPU text rendering (built on `cosmic-text`) | Best-in-class for Rust GPU text; handles shaping, fallback, subpixel positioning |
| `egui` + `egui-wgpu` + `egui-winit` | UI for strip, cards, dialogs | Immediate-mode, integrates cleanly with wgpu |
| `portable-pty` | Cross-platform PTY creation and management | Cleaner API than rolling our own; handles Windows ConPTY too |
| `tokio` | Async runtime for PTY I/O | Standard choice; `smol` also viable but tokio has wider ecosystem |
| `parking_lot` | Faster mutexes, especially `FairMutex` | We need fairness guarantees between PTY reader and renderer |
| `serde` + `toml` | Config serialization | |
| `notify` | File watching for config hot reload | |
| `arboard` | Clipboard access across platforms | |
| `directories` | XDG-compliant config paths | |

### Why this stack over alternatives

**Why Rust + wgpu over Swift + AppKit?** The design conversation surfaced this tradeoff explicitly. Native macOS (Swift/AppKit) would win on OS integration but require a full rewrite for cross-platform. Rust+wgpu with `alacritty_terminal` is what Wezterm and Zellij use and their performance is genuinely excellent — Alacritty itself outperforms Terminal.app on macOS because GPU text rendering beats AppKit for this specific workload. We accept a small tax on window chrome polish in exchange for a codebase that runs everywhere at full speed.

**Why egui over hand-rolled wgpu UI?** The strip, cards, dialogs, and drag-and-drop are immediate-mode by nature and would take weeks to build from scratch with proper hover/focus/accessibility. egui integrates cleanly with wgpu (we share a surface), adds maybe 1-2ms per frame, and handles the tedious parts. Terminals themselves are NOT drawn with egui — they get their own wgpu render pass for maximum speed.

**Why not Tauri or Electron?** Max speed is the top priority. Webview-based approaches add latency we don't need.

**Why `alacritty_terminal` as a library?** It's the most battle-tested terminal state machine in Rust. The alternative is writing a VT parser ourselves, which is a 6-month project to get the edge cases right. Not happening.

---

## 3. UX design

### The mental model

Three levels of hierarchy, no more:

1. **App** — one window, one Kookaburra instance.
2. **Workspace** — a named group of related terminals. Think "the auth refactor" or "exploring the new API."
3. **Tile** — a single terminal within a workspace. Up to 6 per workspace in the default grid.

**Explicitly rejected:** tabs inside tiles (would be a 4th level and users won't hold it in their head).

### The top strip

A narrow horizontal strip at the top of the window showing one **card** per workspace. The strip is the spatial UI for parallel work.

**Card contents:**
- Workspace label (user-editable, with smart defaults from CWD)
- Mini-indicators for each tile (small dots showing activity/new output)
- Subtle "Claude is generating" signal when any tile has active Claude Code output
- Visual state: active workspace is highlighted, others are dimmed

**Card dimensions:** ~140×48px. Small enough to feel like a status bar, big enough to read.

**Interactions:**
- **Click** a card: switch to that workspace
- **Double-click** a card label: rename inline
- **Drag a card**: reorder workspaces
- **Drag a tile (from the grid) onto a card**: move that tile to that workspace (the magic interaction)
- **Drag a tile onto empty strip space**: create a new workspace containing that tile
- **`Cmd+1` through `Cmd+9`**: jump directly to workspaces 1-9
- **`+` button at the end of the strip**: create new workspace

**Overflow:** When there are too many workspaces to fit, the strip scrolls horizontally. The first 9 are always keybindable via `Cmd+1..9`.

### The tile grid

Below the strip, the remaining window area is divided into tiles according to the current workspace's **layout**.

**v1 layout presets:**
- 1×1 (single tile fullscreen-in-app)
- 2×1 (side by side)
- 1×2 (stacked)
- 2×2
- 3×2 (default)
- 2×3

**Explicitly deferred:** arbitrary splits, resizable dividers, i3-style tree layouts. The preset set covers 95% of use and avoids the complexity spiral that tiling WMs fall into.

**Tile visual treatment:**
- Subtle border, accentuated for the focused tile
- **Primary tile** (optional, user-designated): slightly brighter accent border; becomes default focus when switching to the workspace
- **Inactive tiles** dim slightly (reduce opacity by ~10-15%)
- **Tiles with new output since last viewed** get an edge highlight or pulse (the "unread" indicator)

### Keyboard model

Default bindings (all configurable later):

| Binding | Action |
|---|---|
| `Cmd+T` | New tile in current workspace |
| `Cmd+W` | Close focused tile |
| `Cmd+Shift+T` | New workspace |
| `Cmd+1..9` | Switch to workspace 1-9 |
| `Cmd+Opt+1..6` | Focus tile 1-6 in current workspace |
| `Cmd+Enter` | Zen mode (maximize focused tile, hide strip) |
| `Cmd+F` | Search in focused tile's scrollback |
| `Cmd+Shift+F` | Search across all tiles in workspace |
| `Cmd+L` | Rename current workspace |
| `Cmd+,` | Open config (in a tile, with `$EDITOR`) |
| `Cmd+C` / `Cmd+V` | Copy / paste (when selection exists; otherwise pass to shell) |

### Additional features for v1

- **Primary tile per workspace** — mark one tile as primary, it gets focus by default when switching to that workspace.
- **Output-aware dimming** — inactive tiles dim; tiles with new output get a subtle highlight.
- **"Follow" mode per tile** — toggle whether a tile auto-scrolls to the latest output (useful for log tailers).
- **Zen mode** — `Cmd+Enter` maximizes the focused tile and hides the strip. Press again to restore.
- **Cross-tile search** — `Cmd+Shift+F` searches scrollback across all tiles in the current workspace.
- **Workspace templates** — save/load tile configurations (count, CWDs, startup commands, worktree settings).

### Features explicitly deferred to v2+

- Per-tile themes (uniform visual treatment is what makes a grid readable)
- Tabs within tiles (hierarchy overload)
- Session persistence of running processes (genuinely hard; reattaching to PTYs is a rabbit hole)
- Arbitrary splits (preset layouts cover 95% of use)
- Plugin system
- Remote/SSH terminal types (local PTYs only in v1)
- Auto-update mechanism

---

## 4. Git worktree integration

This is a signature feature. The defining workflow: create three tiles in a workspace, each a worktree on a different branch, each with a Claude Code session trying a different approach to the same problem. At the end, compare, pick the winner, discard the rest.

### Model

A tile can optionally be in **worktree mode**, which means:
- It has a **source repository** (the "real" repo the user started from)
- It has a **worktree path** (under a managed directory, e.g. `~/.kookaburra/worktrees/<repo>-<short-id>/`)
- It has a **branch name** (default auto-generated: `kookaburra/<workspace-slug>-<short-id>`)
- It has a **base reference** (what the worktree branched from — usually `HEAD` or a named branch)
- The tile's PTY is spawned with CWD set to the worktree path

From the shell's perspective, the tile is in a normal git repository. Everything (git commands, Claude Code, build tools) just works.

### Lifecycle

**On tile creation with worktree enabled:**
1. User toggles "worktree" in the new-tile dialog.
2. Detect repo root via `git -C <cwd> rev-parse --show-toplevel`. If not a repo, disable the option.
3. Prompt for (or auto-generate) branch name and base ref (dropdown populated from `git branch -a`).
4. Run `git -C <repo> worktree add <worktree-path> -b <branch> <base-ref>`.
5. Spawn PTY with CWD = worktree path.
6. Store worktree metadata on the tile.

**During tile life:**
- Poll `git -C <worktree> status --porcelain=v2 --branch` every 2-3 seconds to update UI indicators.
- Surface branch name and dirty indicator (●) on the tile and in the strip card.

**On tile close:**
- Check for uncommitted changes.
- If clean: prompt with options — "Keep worktree", "Remove worktree", "Copy branch name and remove".
- If dirty: warn loudly. Default to "Keep worktree" so nothing is lost. Require explicit confirmation to force-remove.
- Run `git worktree remove` only on confirmation.

**Fork-this-tile action:** Given an existing worktree tile, create a new tile with a new worktree branched from the same base. Enables "try two variations from a common starting point." Trivial to implement, looks like magic.

### Implementation notes

**Use subprocess `git`, not `git2`.** For structural operations (worktree add/remove), shelling out to the system `git` binary is safer — it handles every edge case, respects user config, runs hooks correctly. `git2` is worth considering for status polling (slightly faster), but even there the subprocess cost is negligible on modern systems.

### Gotchas

1. **Branch naming collisions.** User creates tile, closes without cleanup, creates another with same default name → `git worktree add -b` fails. Mitigation: include short random suffix in auto-generated names (what we do), or detect collision and increment.
2. **Submodules.** `git worktree add` doesn't auto-init submodules. For v1, document this. For v2, offer an option to run `git submodule update --init --recursive` after creation.
3. **Hooks.** `.git/hooks` is NOT shared across worktrees. Tools that install hooks (pre-commit, husky) need re-installation per worktree. Not our problem to fix, but document it.
4. **Disk space.** Working files are duplicated per worktree. Six worktrees of a 500MB repo = ~3GB. Surface worktree count and approximate disk usage in a settings/worktrees panel.
5. **Orphaned worktrees after crash.** On startup, scan `git worktree list` for paths under our managed directory and offer cleanup. Don't auto-delete — user might have uncommitted work.
6. **Merge back UX.** Intentionally minimal for v1: a "Copy branch name" button. Users do the merge in another tile. More automation can come later if demand exists.

### Config

```toml
[worktree]
base_dir = "~/.kookaburra/worktrees"  # where worktrees are created
default_cleanup = "prompt"            # "always" | "never" | "prompt"
branch_template = "kookaburra/{workspace}-{short_id}"
```

---

## 5. Architecture

### Three concurrency domains

1. **Main thread** — owns winit's event loop (required on macOS), wgpu, and all rendering. Reads shared state, never writes application logic.
2. **Tokio runtime** — owns all PTY I/O. One async task per PTY: reads bytes, feeds them into `alacritty_terminal::Term`, emits dirty signals.
3. **Message bus** — `mpsc::channel` connects them. PTY events go main-ward; UI actions go mutation-ward.

This separation keeps us out of the two traps that kill Rust GUI projects: **blocking the render thread on I/O**, and **lock contention between readers and the renderer**.

### Workspace layout (Cargo workspace)

```
kookaburra/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── kookaburra-core/         # domain types, no I/O, no rendering
│   ├── kookaburra-pty/          # PTY management, async readers
│   ├── kookaburra-render/       # wgpu + glyphon rendering
│   ├── kookaburra-ui/           # strip, cards, input handling (egui)
│   └── kookaburra-app/          # binary: ties everything together
└── assets/
    ├── fonts/                  # bundled fallback font(s)
    └── logo/
        ├── kookaburra.svg      # 1-bit master logo
        ├── kookaburra-{32,64,128,256,512}.png
        ├── kookaburra.icns     # macOS
        └── kookaburra.ico      # Windows
```

**Dependency graph:**
- `core` depends on nothing app-specific (serde is fine, wgpu/winit/tokio are not)
- `pty` depends on `core`, `tokio`, `portable-pty`, `alacritty_terminal`
- `render` depends on `core`, `wgpu`, `glyphon`
- `ui` depends on `core`, `winit`, `egui`, `render`
- `app` depends on all four

This split forces good design (no rendering code in core, no UI code in pty, etc.) and keeps compile times sane. It's not premature — even a small project benefits from these boundaries.

### Core domain types

```rust
// kookaburra-core/src/lib.rs

pub struct AppState {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: WorkspaceId,
    pub focused_tile: Option<TileId>,
    pub config: Config,
    pub zen_mode: bool,
}

pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
    pub layout: Layout,
    pub tiles: Vec<Tile>,
    pub primary_tile: Option<TileId>,
}

pub struct Tile {
    pub id: TileId,
    pub pty_id: PtyId,                       // handle into PtyManager
    pub term: Arc<FairMutex<Term<EventProxy>>>, // alacritty_terminal
    pub has_new_output: bool,
    pub follow_mode: bool,
    pub cwd: Option<PathBuf>,                // smart labels, worktree detection
    pub worktree: Option<Worktree>,          // Some if tile is in worktree mode
    pub title: String,                       // from OSC sequences
}

pub enum Layout {
    Grid { cols: u8, rows: u8 },
    // Later: Split { direction, ratio, children }
}

// Strongly-typed IDs prevent mixing at compile time
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct WorkspaceId(u64);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct TileId(u64);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PtyId(u64);
```

**Why `Arc<FairMutex<Term>>`:** Both the PTY reader task and the renderer need access — the reader to push parsed bytes into the terminal state, the renderer to read the grid for drawing. `parking_lot::FairMutex` prevents the reader from starving the renderer under heavy output (think `yes` or a noisy compile). Plain `Mutex` could work but fairness matters for pathological workloads.

**Why newtype IDs:** Raw integers invite the "passed a tile ID where a workspace ID was expected" class of bugs. Newtypes catch this at compile time with zero runtime cost.

### PTY layer

```rust
// kookaburra-pty/src/lib.rs

pub struct PtyManager {
    ptys: HashMap<PtyId, PtyHandle>,
    event_tx: mpsc::Sender<PtyEvent>,
}

pub struct PtyHandle {
    writer: Box<dyn Write + Send>,            // for input
    resize: Box<dyn Fn(PtySize) + Send>,
    _reader_task: tokio::task::JoinHandle<()>,
}

pub enum PtyEvent {
    OutputReceived { tile_id: TileId },       // just a dirty signal
    ProcessExited { tile_id: TileId, status: ExitStatus },
    TitleChanged { tile_id: TileId, title: String },
    BellRang { tile_id: TileId },
    ClipboardRequest { tile_id: TileId, data: String }, // OSC 52
}
```

**The reader task** does this in a loop: read up to 4KB from the PTY, feed into the `Term`'s parser (mutating its grid), send a lightweight `OutputReceived` event. It does NOT send output bytes through the channel — the renderer reads directly from the `Term`. This is the critical performance design: we signal "something changed," we don't transport data.

**EventProxy:** `alacritty_terminal` uses a trait-based event system (`EventListener`) to report bells, title changes, OSC 52 clipboard requests, etc. We implement a thin proxy that forwards these to the mpsc channel:

```rust
pub struct EventProxy {
    tile_id: TileId,
    tx: mpsc::Sender<PtyEvent>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event;
        let pty_event = match event {
            Event::Title(title) => PtyEvent::TitleChanged { tile_id: self.tile_id, title },
            Event::Bell => PtyEvent::BellRang { tile_id: self.tile_id },
            Event::Exit => return, // handled separately
            // ... etc
            _ => return,
        };
        let _ = self.tx.try_send(pty_event);
    }
}
```

**Dirty signal coalescing:** If a tile fires `OutputReceived` 500 times between frames, the renderer redraws once. Drain the channel at the top of each frame and mark tiles dirty; don't process one event = one redraw.

### Render pipeline

See section 6 for the deep dive. Summary: snapshot each terminal's visible grid into a preallocated `Vec<RenderCell>` under a brief lock, release the lock, then build GPU buffers and issue a single render pass with multiple pipelines (backgrounds → text → overlays → borders → egui).

### UI layer

See section 7. Summary: egui for the strip, cards, dialogs, and drag-and-drop. Integrates via `egui-wgpu` (shares our wgpu surface) and `egui-winit` (routes window events). Terminals are NOT drawn inside egui.

### The action pattern (state flow)

UI code draws against a read-only `&AppState` and produces a `Vec<Action>`. After the UI pass, the action vec is drained and each action is applied to `&mut AppState` (and to the `PtyManager` if it needs to create/close PTYs).

```rust
pub enum Action {
    SwitchWorkspace(WorkspaceId),
    CreateWorkspace,
    DeleteWorkspace(WorkspaceId),
    RenameWorkspace { id: WorkspaceId, new_label: String },
    ReorderWorkspaces { from: usize, to: usize },

    CreateTile { workspace: WorkspaceId, worktree: Option<WorktreeConfig> },
    CloseTile(TileId),
    FocusTile(TileId),
    MoveTile { tile_id: TileId, target_workspace: WorkspaceId },
    SetPrimaryTile { workspace: WorkspaceId, tile: TileId },
    ToggleFollowMode(TileId),
    ForkTile(TileId), // worktree tiles only

    SetLayout { workspace: WorkspaceId, layout: Layout },
    ToggleZenMode,

    OpenSearch { scope: SearchScope },
    // ...
}
```

**Three benefits:**
1. Every user interaction is a serializable event → future undo/redo and session replay are free.
2. State mutations are localized to one `apply_action` function → easy to audit.
3. Testing is trivial: construct state, apply action, assert.

### Main loop sketch

```rust
// kookaburra-app/src/main.rs (sketched, not final)

fn main() -> Result<()> {
    env_logger::init();

    let event_loop = EventLoop::new()?;
    let window = WindowBuilder::new()
        .with_title("Kookaburra")
        .with_inner_size(PhysicalSize::new(1400, 900))
        .build(&event_loop)?;

    let runtime = tokio::runtime::Runtime::new()?;

    let (pty_event_tx, mut pty_event_rx) = mpsc::channel(256);

    let config = Config::load_or_default();
    let mut app_state = AppState::new(config);
    let mut pty_manager = PtyManager::new(pty_event_tx, runtime.handle().clone());
    let mut renderer = Renderer::new(&window)?;
    let mut ui = UiLayer::new(&window, renderer.device(), renderer.surface_format());
    let mut actions: Vec<Action> = Vec::with_capacity(16);

    event_loop.run(move |event, elwt| {
        // 1. Drain PTY events (non-blocking)
        while let Ok(pty_event) = pty_event_rx.try_recv() {
            handle_pty_event(&mut app_state, pty_event);
        }

        // 2. Handle window/input events
        match event {
            Event::WindowEvent { event, .. } => {
                // Let egui see it first
                let consumed = ui.handle_event(&window, &event);
                if !consumed {
                    // Route to focused tile or handle app-level
                    handle_window_event(&event, &mut app_state, &pty_manager);
                }
                match event {
                    WindowEvent::CloseRequested => elwt.exit(),
                    WindowEvent::Resized(size) => {
                        renderer.resize(size);
                        propagate_resize_to_ptys(&app_state, &pty_manager, size);
                    }
                    WindowEvent::RedrawRequested => {
                        actions.clear();
                        renderer.render_frame(&app_state, &mut ui, &mut actions)?;
                        for action in actions.drain(..) {
                            apply_action(&mut app_state, &mut pty_manager, action);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        // 3. Decide whether to request another frame
        if app_state.needs_redraw() {
            window.request_redraw();
        } else {
            elwt.set_control_flow(ControlFlow::Wait);
        }
    })?;

    Ok(())
}
```

The main loop is deliberately dumb: drain events, route input, render, apply actions. All *logic* lives in pure functions over `&mut AppState`. This keeps the whole app testable without a GPU.

---

## 6. Render pipeline — deep dive

### The frame, end to end

Each frame does this sequence:

1. **Acquire surface texture** from wgpu (blocks briefly if GPU is behind; usually instant).
2. **Compute tile rectangles** from current layout and window size (pure math, microseconds).
3. **For each visible tile**, snapshot its terminal grid:
   - Lock `Term`
   - Copy visible rows into a preallocated `Vec<RenderCell>`
   - Release lock
4. **Build draw data**: background quads per cell (non-default bg only), glyph runs for text, cursor quad, tile borders, selection overlays.
5. **Single render pass**, multiple pipelines:
   - Cell backgrounds (instanced quads)
   - Text (glyphon)
   - Overlays (cursor, selection)
   - Tile borders
   - egui on top
6. **Submit and present.**

The performance discipline: **steps 3 and 4 run on the main thread with zero allocations in the hot path. Step 5 is where the GPU does the work.**

### The cell snapshot

```rust
#[derive(Copy, Clone)]
pub struct RenderCell {
    pub ch: char,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    pub flags: CellFlags,  // bold, italic, underline, inverse, wide_char, etc.
}

pub struct TileSnapshot {
    pub cells: Vec<RenderCell>,   // row-major, len == cols * rows
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub cursor_style: CursorStyle,
    pub selection: Option<SelectionRange>,
}
```

**Preallocate these.** Keep one `TileSnapshot` per tile, reuse across frames, `clear()` + refill. Zero steady-state allocations is the goal and it's achievable.

```rust
fn snapshot_tile(tile: &Tile, snapshot: &mut TileSnapshot) {
    let term = tile.term.lock();  // parking_lot, ~20ns uncontended
    let grid = term.grid();
    let display_offset = grid.display_offset();

    snapshot.cells.clear();
    snapshot.cols = grid.columns() as u16;
    snapshot.rows = grid.screen_lines() as u16;

    for row_idx in 0..snapshot.rows {
        let line_idx = row_idx as i32 - display_offset as i32;
        let line = &grid[Line(line_idx)];
        for cell in line.iter() {
            snapshot.cells.push(RenderCell {
                ch: cell.c,
                fg: resolve_color(cell.fg, &theme),
                bg: resolve_color(cell.bg, &theme),
                flags: convert_flags(cell.flags),
            });
        }
    }

    snapshot.cursor = if term.mode().contains(TermMode::SHOW_CURSOR) {
        let c = term.grid().cursor.point;
        Some((c.column.0 as u16, c.line.0 as u16))
    } else {
        None
    };
    // lock released here
}
```

**Lock duration:** Microseconds, not milliseconds. 80×24 = 2000 cells of simple struct copies is nanoseconds. 300×100 = 30k cells is well under a millisecond. The PTY reader won't notice.

### The two GPU pipelines

**Background pipeline — instanced quads.** One instance per cell with a non-default background. Vertex shader takes instance data (x, y, w, h, color), fragment shader outputs color. Fastest possible draw path on any GPU. Easily handles 1M+ cells/sec.

**Text pipeline — glyphon.** Builds a glyph atlas, issues textured quad draw calls. Handles font shaping, fallback, subpixel positioning. Under the hood it's also instanced quads.

### Per-frame flow

```rust
fn render_frame(&mut self, state: &AppState, ui: &mut UiLayer,
                actions: &mut Vec<Action>) -> Result<()> {
    let output = self.surface.get_current_texture()?;
    let view = output.texture.create_view(&Default::default());

    // 1. Layout
    let tile_rects = compute_tile_rects(
        &state.active_workspace().layout,
        self.window_size,
        state.zen_mode,
        state.focused_tile,
    );

    // 2. Snapshot all visible tiles
    for (tile, rect) in state.active_workspace().tiles.iter().zip(&tile_rects) {
        snapshot_tile(tile, self.snapshots.get_mut(&tile.id).unwrap());
    }

    // 3. Build CPU-side draw data
    self.bg_quads.clear();
    self.border_quads.clear();
    self.overlay_quads.clear();
    for (snapshot, rect) in self.snapshots.values().zip(&tile_rects) {
        build_bg_quads(snapshot, rect, &self.cell_metrics, &mut self.bg_quads);
        build_overlays(snapshot, rect, &self.cell_metrics, &mut self.overlay_quads);
    }
    build_tile_borders(&tile_rects, state.focused_tile, &mut self.border_quads);

    // 4. Update glyphon text buffers
    for (snapshot, rect) in self.snapshots.values().zip(&tile_rects) {
        update_text_buffer(snapshot, rect, &mut self.text_buffers[snapshot.id],
                          &mut self.font_system);
    }
    self.text_renderer.prepare(
        &self.device, &self.queue, &mut self.font_system,
        &mut self.atlas, viewport,
        text_areas_iter(&self.text_buffers, &tile_rects),
        &mut self.swash_cache,
    )?;

    // 5. Run egui pass
    let raw_input = ui.egui_winit.take_egui_input(&self.window);
    let egui_output = ui.egui_ctx.run(raw_input, |ctx| {
        draw_ui(ctx, state, actions);
    });
    ui.egui_winit.handle_platform_output(&self.window, egui_output.platform_output);
    let tris = ui.egui_ctx.tessellate(egui_output.shapes, egui_output.pixels_per_point);
    for (id, delta) in &egui_output.textures_delta.set {
        ui.egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
    }
    ui.egui_renderer.update_buffers(&self.device, &self.queue,
                                     &mut encoder, &tris, &screen_descriptor);

    // 6. Single render pass
    let mut encoder = self.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.bg_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        self.quad_pipeline.draw(&mut pass, &self.bg_quads);         // cell backgrounds
        self.text_renderer.render(&self.atlas, viewport, &mut pass)?; // text
        self.quad_pipeline.draw(&mut pass, &self.overlay_quads);    // cursor, selection
        self.quad_pipeline.draw(&mut pass, &self.border_quads);     // tile borders
        ui.egui_renderer.render(&mut pass, &tris, &screen_descriptor); // egui strip
    }

    // 7. Cleanup and present
    for id in &egui_output.textures_delta.free {
        ui.egui_renderer.free_texture(id);
    }
    self.queue.submit(Some(encoder.finish()));
    output.present();

    Ok(())
}
```

**Why one render pass:** Starting/ending render passes has nontrivial overhead on some backends (especially Metal). Batching everything into one pass matters.

### Font metrics and cell grid

The foundation of correctness. Every cell is the same width, rows are uniform height, glyphs centered consistently.

```rust
pub struct CellMetrics {
    pub width: f32,    // advance width of 'M' in the monospace font
    pub height: f32,   // line height
    pub ascent: f32,   // baseline offset from top
    pub descent: f32,
}
```

Compute once at startup and on font change. Use 'M' or '0' as the reference glyph.

**CJK / double-width:** Render spanning two cells, skip the next cell. `alacritty_terminal` marks these with the `WIDE_CHAR` flag — honor it.

**Ligatures:** Break the cell grid (`=>` in Fira Code = one glyph spanning two cells). v1: disable ligatures (what Alacritty does). v2: make it a config option. Users who care will tell us.

### Redraw strategy — avoid the dirty tracking trap

**The seductive idea:** only redraw tiles that changed.

**Why it's mostly a trap for our use case:** Claude Code emits output continuously during generation. `cargo watch` fires on every save. `htop` refreshes every second. Something is always dirty. Per-tile dirtiness adds complexity for minimal gain.

**Better strategy:** one `needs_redraw` flag for the whole app. PTY events set it, input events set it, animations set it. At the end of each event loop iteration, if false → `ControlFlow::Wait`. Result: 0% idle CPU, 60fps when active. This is what most good terminals do.

**The one refinement:** cap redraws at 60fps under extreme load. Otherwise pathological workloads (`yes` piped to self) will process 10,000 PTY events per frame and starve the renderer. A simple frame-budget check in the event loop handles this.

### Color resolution

`alacritty_terminal` reports colors as `Color::Named(NamedColor)`, `Color::Spec(Rgb)`, or `Color::Indexed(u8)`. We need a palette:

```rust
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],          // ANSI colors (dim variants as 8-15)
}

pub fn resolve_color(color: alacritty_terminal::vte::ansi::Color,
                    theme: &Theme) -> [u8; 4] {
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    match color {
        Color::Named(NamedColor::Foreground) => theme.foreground.into_rgba(),
        Color::Named(NamedColor::Background) => theme.background.into_rgba(),
        Color::Named(NamedColor::Red) => theme.ansi[1].into_rgba(),
        // ... etc for named colors
        Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b, 255],
        Color::Indexed(i) if i < 16 => theme.ansi[i as usize].into_rgba(),
        Color::Indexed(i) => xterm_256_color(i).into_rgba(),  // standard 256-color table
    }
}
```

**Ship with:** Tokyo Night, Catppuccin Mocha, Solarized Dark as builtin defaults.

---

## 7. UI layer (egui) — deep dive

### Integration setup

```rust
pub struct UiLayer {
    pub egui_ctx: egui::Context,
    pub egui_winit: egui_winit::State,
    pub egui_renderer: egui_wgpu::Renderer,
}

impl UiLayer {
    pub fn new(window: &Window, device: &wgpu::Device,
               surface_format: wgpu::TextureFormat) -> Self {
        let egui_ctx = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            None, None, None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            device, surface_format, None, 1, false,
        );
        Self { egui_ctx, egui_winit, egui_renderer }
    }

    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.egui_winit.on_window_event(window, event).consumed
    }
}
```

egui runs each frame between terminal rendering and presentation. It draws on top of terminals in the same render pass via alpha blending — so dialogs and overlays composite correctly.

### Event routing (critical)

Priority order for every input event:

1. **egui first.** If `on_window_event` returns `consumed: true`, stop. User is interacting with the strip or a dialog.
2. **Keyboard → focused tile's PTY** (if egui didn't want it AND `egui_ctx.wants_keyboard_input()` is false).
3. **Mouse within terminal bounds → terminal mouse handling** (selection, scroll, click-to-focus).
4. **Otherwise → let the main loop handle it** (resize, close, etc.).

```rust
fn route_keyboard(&mut self, event: &KeyEvent) {
    if self.ui.egui_ctx.wants_keyboard_input() {
        return; // egui text field has focus
    }
    if let Some(tile_id) = self.app_state.focused_tile {
        self.pty_manager.send_key(tile_id, event);
    }
}
```

**Focus coordination:** egui has its own focus model for its widgets; tiles have a focus model for keystrokes. The two must not fight. Use `wants_keyboard_input()` and `wants_pointer_input()` religiously.

### Drawing the strip

```rust
fn draw_strip(ctx: &egui::Context, state: &AppState, actions: &mut Vec<Action>) {
    egui::TopBottomPanel::top("workspace_strip")
        .exact_height(56.0)
        .frame(egui::Frame::none().fill(ctx.style().visuals.window_fill))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for workspace in &state.workspaces {
                    draw_card(ui, workspace,
                             workspace.id == state.active_workspace,
                             actions);
                }
                if ui.add(egui::Button::new("+").min_size(egui::vec2(32.0, 48.0)))
                     .clicked() {
                    actions.push(Action::CreateWorkspace);
                }
            });
        });
}

fn draw_card(ui: &mut egui::Ui, workspace: &Workspace, is_active: bool,
             actions: &mut Vec<Action>) {
    let card_size = egui::vec2(140.0, 48.0);
    let (rect, response) = ui.allocate_exact_size(
        card_size,
        egui::Sense::click_and_drag(),
    );

    let painter = ui.painter();
    let bg_color = if is_active {
        ui.style().visuals.selection.bg_fill
    } else {
        ui.style().visuals.widgets.inactive.bg_fill
    };
    painter.rect_filled(rect, 6.0, bg_color);

    painter.text(
        rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        &workspace.label,
        egui::FontId::proportional(13.0),
        ui.style().visuals.text_color(),
    );

    // Mini tile indicators
    let indicator_y = rect.bottom() - 12.0;
    for (i, tile) in workspace.tiles.iter().enumerate().take(6) {
        let x = rect.left() + 8.0 + (i as f32) * 10.0;
        let dot_color = if tile.has_new_output {
            egui::Color32::from_rgb(100, 200, 100)
        } else {
            ui.style().visuals.weak_text_color()
        };
        painter.circle_filled(egui::pos2(x, indicator_y), 3.0, dot_color);
    }

    if response.clicked() {
        actions.push(Action::SwitchWorkspace(workspace.id));
    }
    if response.double_clicked() {
        actions.push(Action::StartRenaming(workspace.id));
    }
}
```

### Drag-and-drop

egui's built-in drag handling covers the mechanics. The pattern:

```rust
// Dragged tile:
let response = ui.interact(tile_rect, tile_id_as_egui_id, egui::Sense::drag());
if response.drag_started() {
    ui.ctx().memory_mut(|mem| {
        mem.data.insert_temp(egui::Id::new("dragged_tile"), tile.id);
    });
}
if response.dragged() {
    // Paint a ghost rectangle following the cursor
    if let Some(pos) = ui.ctx().pointer_interact_pos() {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip, egui::Id::new("drag_ghost")
        ));
        painter.rect_filled(
            egui::Rect::from_center_size(pos, egui::vec2(120.0, 80.0)),
            4.0,
            egui::Color32::from_rgba_premultiplied(100, 100, 100, 180),
        );
    }
}

// Drop target (a workspace card):
if response.hovered() && ui.ctx().dragged_id().is_some() {
    painter.rect_stroke(rect, 6.0, egui::Stroke::new(2.0, egui::Color32::WHITE));
}
if response.drag_stopped() && response.hovered() {
    if let Some(tile_id) = ui.ctx().memory(|mem|
        mem.data.get_temp::<TileId>(egui::Id::new("dragged_tile"))
    ) {
        actions.push(Action::MoveTile {
            tile_id,
            target_workspace: workspace.id
        });
    }
}
```

### Style pass

egui defaults are functional, not beautiful. Budget time once the app works:

- `ctx.style_mut().visuals.widgets.*` — per-state colors
- `ctx.style_mut().spacing.item_spacing` — layout tightness
- `ctx.set_fonts(...)` — match terminal font family or use UI font
- `Frame::none().fill(...).rounding(...)` — card backgrounds with rounded corners
- Animations: `ctx.request_repaint_after(Duration::from_millis(16))` during transitions

**Goal:** strip feels like it belongs with the terminals, not like a generic immediate-mode GUI slapped on top.

### egui performance notes

- Re-tessellates every frame. For a 5-10 card strip: sub-millisecond. Not a concern.
- Requests continuous repaint while mouse hovers an interactive widget. Prevents 0% idle CPU when cursor is in the strip. Usually fine; if it bothers us, we can throttle with `request_repaint_after`.
- **Don't put terminals in egui.** Terminals are their own wgpu render pass. egui draws chrome only.

---

## 8. Implementation roadmap

Phased work, with effort estimates for a focused solo developer.

### Phase 1 — Single tile working end-to-end (3-5 days)

- [ ] Set up Cargo workspace and crates (few hours)
- [ ] `kookaburra-core`: types, layout rect computation, unit tests (half day)
- [ ] `kookaburra-pty`: spawn one PTY, read bytes, dump to stdout to verify (half day)
- [ ] Wire `alacritty_terminal` state machine, log grid on change (half day)
- [ ] `kookaburra-render`: minimal viable — one tile, glyphon text, cursor, background color (1-2 days)
- [ ] Keyboard input → PTY writer (few hours)
- [ ] Window resize → surface resize + PTY resize (`TIOCSWINSZ`) (few hours)

**Exit criterion:** You can open the app, type commands, run `vim` or `htop`, and it works. Single tile, no strip, no workspaces.

### Phase 2 — Multi-tile and layouts (2-3 days)

- [ ] N tiles in a grid from layout enum (half day)
- [ ] Focus model and keyboard focus switching (few hours)
- [ ] Per-tile resize propagation on window resize (few hours)
- [ ] Mouse click-to-focus (half day)
- [ ] Tile borders, focused tile accent (few hours)
- [ ] Layout preset switching via keybinding (half day)

**Exit criterion:** 3×2 grid of terminals, click to focus, keyboard to type, borders indicate focus.

### Phase 3 — Strip and workspaces (3-4 days)

- [ ] egui integration in render pipeline (half day)
- [ ] Blank `TopBottomPanel` strip (few hours)
- [ ] Cards with labels, active highlight, click to switch (half day)
- [ ] Multi-workspace state management (half day)
- [ ] Activity indicators on cards (few hours)
- [ ] Workspace rename inline (few hours)
- [ ] Drag to reorder workspaces (half day)
- [ ] Drag tile between workspaces (1 day — the finicky one)
- [ ] `+` button to add workspace, close workspace (few hours)

**Exit criterion:** Multiple workspaces, visual strip, drag tiles between workspaces.

### Phase 4 — Terminal UX essentials (2-3 days)

- [ ] Mouse text selection with proper wrapping (half day — harder than it looks)
- [ ] Clipboard copy/paste via `arboard` (few hours)
- [ ] Scrollback via mouse wheel and keyboard (half day)
- [ ] In-tile search via `alacritty_terminal::RegexSearch` (half day)
- [ ] Bell handling + visual indication (few hours)
- [ ] OSC sequences: title changes, hyperlinks (half day)

### Phase 5 — Polish and config (2-3 days)

- [ ] Config file load (TOML at XDG path) (half day)
- [ ] Config hot reload via `notify` (few hours)
- [ ] Keybinding system driven from config (half day)
- [ ] Theme system with external theme files (half day)
- [ ] Font configuration + live switching (few hours)
- [ ] Zen mode (few hours)
- [ ] Output-aware dimming (few hours)

### Phase 6 — Worktrees (2 days)

- [ ] `kookaburra-core::worktree` module (half day)
- [ ] Tile creation flow with worktree toggle (half day)
- [ ] Status polling and UI indicators (few hours)
- [ ] Cleanup prompts on close (few hours)
- [ ] Orphan detection on startup (few hours)
- [ ] Fork-this-tile action (few hours)

### Phase 7 — Cross-tile and templates (2-3 days)

- [ ] Cross-tile search UI (half day)
- [ ] Workspace template format + loader (half day)
- [ ] Template invocation UI (few hours)
- [ ] Follow mode per tile (few hours)
- [ ] Primary tile designation (few hours)

### Phase 8 — Distribution (2-3 days, platform-dependent)

- [ ] macOS: code signing, notarization, DMG packaging (1-2 days)
- [ ] Linux: AppImage or deb/rpm (half day)
- [ ] Windows: MSI or portable exe (half day)
- [ ] Auto-update: **deferred to v2**

### Total estimate

**~18-26 days of focused work for a polished v1.** Call it 4-6 weeks of real calendar time accounting for distractions and rabbit holes.

---

## 9. Known risks and gotchas

Things most likely to surprise:

1. **Font rendering edge cases.** Emoji, CJK, powerline glyphs, zero-width joiners, combining characters. `cosmic-text` handles most but expect weird cases. Test with a "torture test" file early — Alacritty's test suite has good examples.

2. **Mouse selection.** Multi-line selection with proper wrapping semantics, selection across the scrollback boundary, double-click-word, triple-click-line, rectangular selection. Consistently takes 2-3× longer than estimated in every terminal project.

3. **macOS-specific input quirks.** Dead keys, IME for CJK input, Cmd vs. Ctrl semantics, Option-as-Meta. Budget real time.

4. **Performance under pathological load.** `yes`, `find /`, `cat large-log.txt`. The coalescing design in this doc handles this, but easy to break if someone routes bytes through a channel.

5. **wgpu version churn.** `wgpu` has breaking changes between versions. Pin the version and don't upgrade casually mid-project.

6. **PTY resize signaling.** On tile/window resize, MUST send `TIOCSWINSZ` to the PTY AND update the `Term`'s grid size. Miss either → visual corruption or programs that don't know they resized. `portable-pty` handles ioctl; `alacritty_terminal` handles the grid.

7. **Font loading cold start.** `cosmic-text`/`glyphon` enumerates system fonts (100-400ms). Do this on a background thread during window creation so cold start stays fast.

8. **Borrow checker on `AppState`.** Renderer wants `&AppState`, UI wants `&mut`, PTY events want to mutate a tile. Solution: PTY events drained at top of frame; UI produces `Vec<Action>`; renderer only ever sees `&AppState`. One owner, one mutator per phase. Resist wrapping everything in `Arc<Mutex>`.

9. **Surface resize races.** Resize wgpu surface → recompute rects → resize PTYs. In that order, synchronously, on main thread. Async resize is where subtle rendering bugs live.

10. **Worktree orphans after crash.** On startup, scan `git worktree list` for our managed paths. Offer cleanup; never auto-delete (user may have work).

---

## 10. Open design questions (to decide during implementation)

These don't block starting. Decide when the surrounding code is being written.

- **Config schema finalization** — exact TOML shape for keybindings, themes, templates, worktree settings.
- **Theme source of truth** — one theme for terminal + UI, or separate? (v1 recommendation: one.)
- **Ligature support** — v1 off, v2 config option?
- **Template invocation** — command palette, right-click menu on `+`, or CLI arg?
- **Merge-back UX for worktrees** — just "copy branch name" (v1), or guided flow (v2)?
- **Session persistence scope** — layout and CWDs yes; running processes no. Scrollback?
- **Search regex vs. plain text** — probably both, toggle in search UI.
- **Strip overflow** — horizontal scroll confirmed; dropdown for workspaces beyond index 9?

---

## 11. Getting started: first commit

The immediate next step is `kookaburra-core` with compilable types. Suggested first commit:

```
kookaburra/
├── Cargo.toml                  # workspace definition
├── rust-toolchain.toml         # pin stable Rust
├── .gitignore
├── README.md                   # includes the logo at the top
├── assets/
│   └── logo/
│       └── kookaburra.svg      # 1-bit master logo (provided in handoff)
└── crates/
    └── kookaburra-core/
        ├── Cargo.toml
        └── src/
            ├── lib.rs          # module declarations
            ├── ids.rs          # WorkspaceId, TileId, PtyId newtypes
            ├── state.rs        # AppState, Workspace, Tile
            ├── layout.rs       # Layout enum + rect computation
            ├── action.rs       # Action enum
            ├── config.rs       # Config struct (empty to start)
            └── worktree.rs     # Worktree types (no implementation yet)
```

**First unit tests should cover:**
- Layout rect computation for each preset (1×1, 2×1, 1×2, 2×2, 3×2, 2×3)
- ID generation uniqueness
- Basic `AppState` construction and tile insertion/removal

Once this compiles and tests pass, move to `kookaburra-pty` (Phase 1, step 3).

---

## 12. Glossary

- **Workspace** — a named group of related tiles. The unit of "parallel work context."
- **Tile** — a single terminal within a workspace.
- **Strip** — the top bar containing workspace cards.
- **Card** — the visual representation of a workspace in the strip.
- **Primary tile** — the tile that gets focus when switching to a workspace (optional, per-workspace).
- **Follow mode** — per-tile flag: auto-scroll to latest output.
- **Zen mode** — app-wide flag: maximize focused tile, hide strip.
- **Worktree mode** — per-tile flag: tile's CWD is a git worktree rather than the plain repo.
- **Fork** (of a tile) — create a new tile with a new worktree branched from the same base as an existing worktree tile.
- **Snapshot** — a per-frame copy of a tile's visible terminal grid, used for rendering without holding locks.

---

*End of handoff. Now go write some Rust.*
