<p align="center">
  <img src="assets/logo/kookaburra.svg" alt="Kookaburra" width="160" height="160" />
</p>

<h1 align="center">Kookaburra</h1>

<p align="center">
  A focused-mode terminal multiplexer for running multiple Claude Code sessions in parallel.
</p>

<p align="center">
  <a href="https://github.com/ClishamJ/kookaburra/actions/workflows/ci.yml"><img src="https://github.com/ClishamJ/kookaburra/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License">
  <img src="https://img.shields.io/badge/status-pre--alpha-orange" alt="Status">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey" alt="Platform">
</p>

---

Kookaburra is a GPU-rendered terminal multiplexer built around a single workflow: running several
Claude Code sessions at once, each in its own git worktree, organized spatially instead of hidden
behind tabs. Workspaces group related terminals; a top strip makes every parallel context visible
and draggable. Built in Rust with `alacritty_terminal`, `wgpu`, `glyphon`, and `egui`. macOS-first,
cross-platform for free.

> **Status: pre-alpha.** The core renderer, tile grid, workspace strip, and config hot-reload work
> end-to-end, but nothing is packaged for distribution yet and large pieces of terminal UX
> (selection, in-tile search, worktree integration) are still in flight. See the implementation
> checklist in [`CLAUDE.md`](./CLAUDE.md) for current progress.

<!--
TODO: add a screenshot once the UI stabilizes.
Target: 1600x1000, dark theme (Tokyo Night), 3x2 grid with a populated workspace strip.
-->

## Features

- Tiled terminal grid with preset layouts (1×1, 2×1, 1×2, 2×2, 3×2, 2×3)
- Multi-workspace top strip: click to switch, drag tiles between workspaces, drag to reorder
- Keyboard navigation: `Cmd+1..9` for workspaces, `Cmd+Opt+1..6` for tiles, `Cmd+Enter` for zen mode
- Inline workspace rename (`Cmd+L`), new workspace (`Cmd+N`), new tile (`Cmd+T`)
- Activity indicators: unread-output pulse and "generating" dots surface live tile state on cards
- TOML config with live hot-reload; builtin themes (Tokyo Night, Catppuccin Mocha, Solarized Dark)
- OSC title updates, bell flash indicator, bracketed paste, mouse-wheel scrollback

## Design philosophy

- **Speed first.** Instant cold start, 60+ fps, zero idle CPU. Architecture defers to this.
- **Focus.** The chrome disappears when you're working and reappears when you need it.
- **Spatial clarity.** Parallel work should feel parallel, not stacked behind tabs.
- **Honest defaults.** Good themes, good keybindings, good fonts out of the box.
- **macOS-first, cross-platform for free.** Native-feeling on macOS; functional on Linux and Windows.

## Non-goals

- Not a tmux replacement — no session persistence of running processes across restarts.
- Not a window manager — it manages tiles inside its own window, not OS windows.
- Not an IDE — no file tree, no editor integration. It's terminals, organized.
- Not infinitely configurable — opinionated defaults over a thousand knobs.

## Built with

[`alacritty_terminal`](https://github.com/alacritty/alacritty) (VT parser and grid model),
[`wgpu`](https://github.com/gfx-rs/wgpu) (GPU abstraction; Metal / Vulkan / DX12),
[`winit`](https://github.com/rust-windowing/winit) (windowing and input),
[`glyphon`](https://github.com/grovesNL/glyphon) (GPU text rendering, built on `cosmic-text`),
[`egui`](https://github.com/emilk/egui) (immediate-mode UI for the strip and dialogs),
[`portable-pty`](https://github.com/wezterm/wezterm) (cross-platform PTY management),
and [`tokio`](https://github.com/tokio-rs/tokio) (async PTY I/O).

## Building from source

Prerequisites: a recent stable Rust toolchain (via [rustup](https://rustup.rs/)). The toolchain
version is pinned in [`rust-toolchain.toml`](./rust-toolchain.toml).

```sh
git clone https://github.com/ClishamJ/kookaburra.git
cd kookaburra
cargo run -p kookaburra-app --release
```

Configuration lives at `$XDG_CONFIG_HOME/kookaburra/config.toml` (typically
`~/.config/kookaburra/config.toml` on Linux, `~/Library/Application Support/kookaburra/config.toml`
on macOS). Missing or malformed configs fall back to defaults — Kookaburra never refuses to start
because of a config error.

## Project layout

Kookaburra is a Cargo workspace with strict dependency boundaries:

```
crates/
├── kookaburra-core/    Domain types, layout math, action reducer. No I/O, no rendering.
├── kookaburra-pty/     PTY management and async reader tasks. Owns alacritty_terminal state.
├── kookaburra-render/  wgpu + glyphon rendering. Draws terminals.
├── kookaburra-ui/      egui strip, cards, dialogs. Produces Actions from input.
└── kookaburra-app/     Binary. Ties the event loop, runtime, and crates together.
```

The dependency direction is one-way (`core` → `pty`/`render`/`ui` → `app`) and enforced in the
workspace manifest.

## Contributing

Kookaburra is a personal project under active, fairly opinionated development. Issues and small
PRs are welcome — please read [`KOOKABURRA.md`](./KOOKABURRA.md) (design spec) and
[`CLAUDE.md`](./CLAUDE.md) (working guide, invariants, and checklist) before opening anything
substantial so we're aligned on scope.

## License

Dual-licensed under [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0) or
[MIT license](https://opensource.org/licenses/MIT) at your option.

## Acknowledgments

Kookaburra stands on the shoulders of [Alacritty](https://github.com/alacritty/alacritty),
[Wezterm](https://github.com/wezterm/wezterm), and [Zellij](https://github.com/zellij-org/zellij) —
their implementations shaped many of the decisions here, from the terminal state machine to the
render pipeline.
