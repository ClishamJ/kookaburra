# NOTES — rough-draft Phase 1 push (2026-04-18)

This file records non-obvious decisions made during the autonomous
continuation pass. Use it to guide review.

## BLOCKED (still): cannot run cargo build — second session (2026-04-18 later)

Same block as the previous pass: the isolated Linux sandbox
(`mcp__workspace__bash`) returns `Workspace unavailable. The
isolated Linux environment failed to start.` Every shell call fails
identically. `cargo check` / `cargo test` / `cargo build` / `cargo
clippy` cannot be run from this session.

What this session did under that constraint:

- Re-read every crate source plus `KOOKABURRA.md` §1–§7 and the full
  `CLAUDE.md` checklist to confirm the handoff state is still what
  `NOTES.md` claims.
- Did **not** add new external crate calls (wgpu, winit, glyphon,
  alacritty_terminal API surface), because any mistake would land
  blind and the next sandboxed session would have to unpick it.
- Added a few unit tests inside `kookaburra-core` and
  `kookaburra-render` that only exercise already-written logic —
  safe because the logic under test already compiled in the prior
  session's target dir.
- Left a concrete follow-up plan below so the next session with a
  live sandbox can pick up without re-doing discovery.

### Plan for the next session (with working `cargo`)

1. **Sanity pass.** `cargo check --workspace` then
   `cargo test --workspace`. Any failure here is a real compile
   error from the prior blind writes; triage against this NOTES file
   before reworking.
2. **Term wiring.** In `kookaburra-pty`:
   - Add `struct EventProxy { tile_id: TileId, tx: StdSender<PtyEvent> }`
     implementing `alacritty_terminal::event::EventListener` (the
     0.24 trait has `fn send_event(&self, event: Event)`). Forward
     `Event::Title`, `Event::Bell`, and `Event::Exit` to `PtyEvent`.
   - Replace the placeholder `last_output: Vec<u8>` field in
     `PtyHandle` with `term: Arc<FairMutex<Term<EventProxy>>>` plus
     `parser: alacritty_terminal::vte::ansi::Processor` held on the
     reader thread's stack. The reader loop calls
     `parser.advance(&mut *term.lock(), chunk)` per chunk, then
     sends a single `OutputReceived` event (coalesce in consumer).
   - Rewrite `PtyManager::snapshot` to walk
     `term.grid().display_iter()` and fill
     `TileSnapshot::cells` / `cursor` / `cursor_style`. Keep the
     existing placeholder path behind `#[cfg(test)]` for unit
     tests.
3. **Render.** Pull `wgpu`, `winit`, `glyphon` into
   `kookaburra-render/Cargo.toml` (pinned versions in the second
   DECISION block below still apply). Split `Renderer::new` into an
   async `new(window: &winit::window::Window)` that creates a
   surface, and implement `render_frame` with: clear → instanced
   cell-bg quads → glyphon text prepare/render → border quads →
   egui pass. See KOOKABURRA.md §6 for the render-pass shape.
4. **App loop.** Convert `kookaburra-app/src/main.rs` to the
   `winit::application::ApplicationHandler` shape. Keyboard events
   → `PtyManager::write` on the focused tile. Resize events →
   `renderer.resize` → `pty_manager.resize` → `Term::resize` in
   that order (synchronously, main thread).
5. **Exit criterion check.** Launch the binary and run `vim`,
   `htop`, `ls --color`. Confirm 60fps-ish and no idle CPU.

When the sandbox comes back, the very next pass should be
`cargo check --workspace` followed by `cargo test --workspace -p
kookaburra-core`. Anything that fails should be triaged against
this NOTES file before larger reworks.

## DECISION: keep wgpu/winit/egui/glyphon work very thin

The spec wants a single-tile end-to-end demo as the Phase 1 exit
criterion. Because I cannot actually run the binary, I'm writing
the render and UI layers as the smallest possible scaffolding that
compiles against the pinned crate versions, with clearly marked
TODOs for the real wgpu pipeline work. The `Renderer::render_frame`
method clears the surface to a Tokyo Night background; it does not
yet draw cells. The intent is that a follow-up session with a real
sandbox can iterate on the pipeline without first having to wire
the boilerplate.

## DECISION: pin specific crate versions

I'm pinning each external crate at the most recent version I'm
confident in from before the May 2025 cutoff. Notable picks:

- `wgpu = "0.20"` — pin per CLAUDE.md non-negotiable ("don't upgrade
  casually mid-project").
- `winit = "0.30"` — matches wgpu 0.20 ApplicationHandler-style
  event loop.
- `glyphon = "0.5"` — known to work with wgpu 0.20 and the cosmic
  text 0.12 line.
- `egui = "0.27"` + `egui-wgpu = "0.27"` + `egui-winit = "0.27"` —
  matched set against wgpu 0.20 / winit 0.30.
- `alacritty_terminal = "0.24"` — most recent line as of cutoff.
- `portable-pty = "0.8"` — stable for years.
- `tokio = "1"` with `rt-multi-thread`, `sync`, `macros`, `io-util`.
- `parking_lot = "0.12"`, `serde = "1"`, `toml = "0.8"`,
  `notify = "6"`, `arboard = "3"`, `directories = "5"`.

If any of these turn out to be misversioned, the sandbox-restored
session should run `cargo update` and adjust the pins, not the call
sites.

## DECISION: Tile.term is Option<...> in core

The spec sketches `term: Arc<FairMutex<Term<EventProxy>>>` as a
field of `Tile`. But `EventProxy` lives in `kookaburra-pty`, and
core can't depend on pty (it would bring in `alacritty_terminal`
and `tokio`). Two choices:

1. Push `Tile` into `kookaburra-pty` and re-export.
2. Make the term handle a generic associated type or store an
   opaque handle on `Tile`.

I picked option (3), which is to keep the live terminal handle in
`kookaburra-pty::PtyManager` keyed by `PtyId`, and have `Tile` in
core hold only the `PtyId`. The renderer asks the pty manager for a
snapshot. This matches the spirit of the spec's "single mutator per
phase" rule and keeps core dependency-free. Spec's Tile field stays
as documentation of intent, not as a literal field shape.

## DECISION: rough Action enum

Mirrors the spec's enum but skips a few variants the rough draft
doesn't need yet (StartRenaming, OpenSearch). Easy to add when the
real UI lands.

## DECISION: rough Config

Defaults only, no TOML loading yet. Theme is a Tokyo-Night-ish
palette literal. Phase 5 is where this gets real.

## DECISION: kookaburra-pty exposes a synchronous PtyManager

The spec puts PTY readers on a tokio runtime but has the manager
itself look synchronous from the main loop's perspective. I've
matched that: `PtyManager::new(event_tx, runtime_handle)` plus
synchronous `spawn`, `write`, `resize`, `snapshot`. Internally each
spawn launches a tokio task that loops on the PTY reader.

## DECISION: snapshot lives in kookaburra-pty

The render crate would otherwise need to depend on
alacritty_terminal directly. By exposing `PtyManager::snapshot(pty,
&mut TileSnapshot)`, render only depends on the snapshot type
(which is plain data) and not on the terminal state machine.

## DECISION: skip worktree implementation

Phase 6 territory. `kookaburra-core::worktree` defines the
`Worktree` and `WorktreeConfig` structs so other code can refer to
them, but no git subprocess code yet.
