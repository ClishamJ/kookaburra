//! wgpu-backed terminal renderer.
//!
//! Phase 1 slice. Opens a wgpu surface against a winit `Window`, clears
//! each frame to the theme background, and draws each terminal tile via
//! our custom instanced-quad glyph pipeline (see `glyph_pipeline.rs`).
//!
//! Historical note: an earlier revision drew text through `glyphon`
//! (`TextRenderer` + `TextAtlas` + `cosmic-text` layout). Keystroke
//! latency profiles showed glyphon's per-frame shape/prepare was the
//! dominant cost even after per-row dirty hashing — because a terminal
//! grid doesn't need bidi, fallback fonts, or wrapping, we bypass the
//! layout engine entirely and rasterize each codepoint on demand into a
//! shared R8 atlas. Glyphon is still a dependency only because its
//! `FontSystem` gives us font-file bytes for free.
//!
//! Per-cell selection bg and borders are deferred (§§4-5 of the spec).

mod glyph_pipeline;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};
use glyph_pipeline::{GlyphPipeline, LoadedFont};
use wgpu::{
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, Instance, InstanceDescriptor,
    LoadOp, Operations, PresentMode, RenderPassColorAttachment, RenderPassDescriptor,
    RequestAdapterOptions, SurfaceConfiguration, TextureFormat, TextureUsages,
    TextureViewDescriptor,
};
use winit::window::Window;

use kookaburra_core::config::{Rgba, Theme};
use kookaburra_core::ids::TileId;
use kookaburra_core::layout::Rect;
use kookaburra_core::snapshot::{CellFlags, TileSnapshot};

/// One frame's worth of tessellated egui geometry + texture deltas. Built
/// by `kookaburra-ui` and ferried through here. Opaque to the render
/// crate beyond the fields on the struct — we just hand them to
/// `egui-wgpu`.
pub struct UiFrame {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

/// Per-frame render budget cap (60fps).
pub const FRAME_BUDGET_MS: u64 = 16;

/// Logical pixel width of a tile border.
pub const TILE_BORDER_PX: f32 = 1.0;

/// Multiplier applied to rgb channels of unfocused tiles' fg colors — a
/// cheap "the inactive tile is dim" effect without a second render pass.
/// Tuned visually against Tokyo Night on dark backgrounds.
pub const UNFOCUSED_DIM: f32 = 0.82;

/// One tile's worth of render input: where to draw, whether this tile
/// should be rendered at full brightness, and optionally a fresh
/// snapshot. If `update` is `None` the renderer reuses the previous
/// snapshot for this tile.
pub struct RenderTile {
    pub tile_id: TileId,
    pub rect: Rect,
    pub focused: bool,
    /// Whether this tile is actively receiving output (streaming).
    pub generating: bool,
    /// Whether this tile is the designated "primary" tile (first to wake,
    /// default focus on workspace switch). Shown with a ◆ star indicator.
    pub primary: bool,
    /// Whether follow-mode is active for this tile. Shown as "▼ follow"
    /// at the tile's bottom-right corner.
    pub follow_mode: bool,
    /// 1-based tile index within the workspace (shown in the header chip).
    pub tile_index: usize,
    pub update: Option<TileSnapshot>,
}

/// Cell font metrics.
#[derive(Copy, Clone, Debug, Default)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
}

impl CellMetrics {
    /// Hard-coded fallback metrics so layout math doesn't divide by zero
    /// when no renderer is available (unit tests, headless).
    #[must_use]
    pub fn fallback(font_size_px: f32) -> Self {
        Self {
            width: font_size_px * 0.6,
            height: font_size_px * 1.25,
            ascent: font_size_px,
            descent: font_size_px * 0.25,
        }
    }
}

/// How many cells fit in a rect for the given metrics.
#[must_use]
pub fn cells_in_rect(rect: Rect, metrics: CellMetrics) -> (u16, u16) {
    let cols = (rect.width / metrics.width).floor().max(1.0);
    let rows = (rect.height / metrics.height).floor().max(1.0);
    (cols as u16, rows as u16)
}

/// Resolve an indexed ANSI color against a theme.
#[must_use]
pub fn ansi_color(theme: &Theme, idx: u8) -> Rgba {
    theme.ansi[(idx as usize).min(15)]
}

/// wgpu-backed renderer.
pub struct Renderer {
    pub theme: Theme,
    pub metrics: CellMetrics,
    pub font_size_px: f32,
    pub line_height_px: f32,

    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: SurfaceConfiguration,

    /// Kept around so we can rebuild `LoadedFont` on font or scale-factor
    /// changes. Not used on the live draw path.
    #[allow(dead_code)]
    font_system: glyphon::FontSystem,
    pipeline: GlyphPipeline,
    egui_renderer: EguiRenderer,

    /// Cached per-tile snapshot so tiles whose `update` is `None` can
    /// still be drawn this frame (they don't change, but every frame
    /// re-emits every visible tile's instances).
    snapshots: HashMap<TileId, TileSnapshot>,

    /// Monotonic start time, used for time-based animations (cursor blink,
    /// marching ants, scanline shimmer).
    start_time: Instant,

    // Keep window alive for the lifetime of the surface. Dropping this
    // after the surface drops is important on some platforms.
    _window: Arc<Window>,
}

impl Renderer {
    /// Construct a renderer bound to the given window. Synchronous — uses
    /// `pollster` to drive the async adapter/device init.
    pub fn new(window: Arc<Window>, theme: Theme, font_size_px: f32) -> Self {
        pollster::block_on(Self::new_async(window, theme, font_size_px))
    }

    async fn new_async(window: Arc<Window>, theme: Theme, font_size_px: f32) -> Self {
        let size = window.inner_size();
        let scale_factor = window.scale_factor() as f32;

        let instance = Instance::new(InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .expect("create wgpu surface");

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("request adapter");

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default(), None)
            .await
            .expect("request device");

        let swapchain_format = TextureFormat::Bgra8UnormSrgb;
        let surface_caps = surface.get_capabilities(&adapter);
        // Prefer Mailbox (non-blocking triple-buffer) for minimum keystroke
        // latency — under `ControlFlow::Wait` each keystroke triggers one
        // render; Fifo would block the main thread up to ~16ms waiting for
        // vsync on every one. Fall back to Fifo if Mailbox isn't available.
        let present_mode = if surface_caps.present_modes.contains(&PresentMode::Mailbox) {
            PresentMode::Mailbox
        } else {
            PresentMode::Fifo
        };
        log::info!("wgpu surface present_mode={present_mode:?}");
        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: swapchain_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
            // 1 frame latency — we render on demand, not continuously, so
            // queueing more than one frame ahead just adds input lag.
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &surface_config);

        let mut font_system = glyphon::FontSystem::new();
        let font = LoadedFont::from_font_system(&mut font_system, font_size_px)
            .expect("no monospace font available on this system");
        let metrics = CellMetrics {
            width: font.cell_width,
            height: font.cell_height,
            ascent: font.ascent,
            descent: font.descent,
        };
        let line_height_px = font.cell_height;
        let pipeline = GlyphPipeline::new(&device, &queue, swapchain_format, font, scale_factor);
        // egui-wgpu expects the same swapchain format + multisample state
        // the pass uses. `msaa_samples: 1` matches our wgpu::MultisampleState::default().
        let egui_renderer = EguiRenderer::new(&device, swapchain_format, None, 1, false);

        Self {
            theme,
            metrics,
            font_size_px,
            line_height_px,
            device,
            queue,
            surface,
            surface_config,
            font_system,
            pipeline,
            egui_renderer,
            snapshots: HashMap::new(),
            start_time: Instant::now(),
            _window: window,
        }
    }

    /// Reconfigure the surface to a new physical pixel size.
    pub fn resize(&mut self, new_size: (u32, u32)) {
        let (w, h) = new_size;
        if w == 0 || h == 0 {
            return;
        }
        self.surface_config.width = w;
        self.surface_config.height = h;
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Current surface size in physical pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    /// Draw a frame. Clears to theme background, emits every visible
    /// tile's bg+fg quads through the glyph pipeline, then draws the
    /// egui strip (if a `UiFrame` is provided) into the same pass.
    pub fn render_frame(&mut self, tiles: &[RenderTile], ui: Option<&UiFrame>) {
        // Update per-tile snapshot cache with any fresh snapshots.
        for t in tiles {
            if let Some(snap) = t.update.as_ref() {
                self.snapshots.insert(t.tile_id, snap.clone());
            }
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            Err(e) => {
                log::warn!("surface acquire failed: {e:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());

        // Rebuild instance lists from scratch. Without layout/shaping, this
        // is just byte arithmetic per cell — <0.1ms even for a dense 6×2
        // grid at 4K.
        self.pipeline.clear_instances();

        // Time since renderer creation, for animations.
        let t_secs = self.start_time.elapsed().as_secs_f64();

        // Grid background: fill the entire tile area with bg color so
        // gaps between tiles read as continuous near-black. Focused tile
        // border is the only thing that reads against it.
        if let (Some(first), Some(last)) = (tiles.first(), tiles.last()) {
            let grid_color = self.theme.background;
            let gx = first.rect.x - 4.0;
            let gy = first.rect.y - 4.0;
            let gw = (last.rect.x + last.rect.width - first.rect.x + 8.0).max(1.0);
            let gh = (last.rect.y + last.rect.height - first.rect.y + 8.0).max(1.0);
            self.pipeline.push_bg(gx, gy, gw, gh, grid_color);
        }

        for t in tiles {
            if let Some(snap) = self.snapshots.get(&t.tile_id) {
                Self::emit_tile(
                    &mut self.pipeline,
                    &self.queue,
                    &self.theme,
                    self.metrics,
                    snap,
                    t.rect,
                    t.focused,
                    t.generating,
                    t.primary,
                    t.follow_mode,
                    t.tile_index,
                    t_secs,
                );
            }
        }

        let clear = rgba_to_wgpu_color(self.theme.background);
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("kookaburra-frame"),
            });

        // Upload egui texture deltas + vertex buffers outside the pass —
        // `update_buffers` records copy commands onto the encoder.
        let screen = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: ui.map(|u| u.pixels_per_point).unwrap_or(1.0),
        };
        if let Some(ui) = ui {
            for (id, delta) in &ui.textures_delta.set {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
            self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &ui.primitives,
                &screen,
            );
        }
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("kookaburra-main-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let size = (self.surface_config.width, self.surface_config.height);
            self.pipeline
                .render(&self.device, &self.queue, &mut pass, size);

            // `egui_wgpu::Renderer::render` wants a `RenderPass<'static>`.
            // `forget_lifetime` drops the encoder-tied lifetime; safe here
            // because the pass is still in-scope and we drop it below.
            if let Some(ui) = ui {
                let mut static_pass = pass.forget_lifetime();
                self.egui_renderer
                    .render(&mut static_pass, &ui.primitives, &screen);
            }
        }

        if let Some(ui) = ui {
            for id in &ui.textures_delta.free {
                self.egui_renderer.free_texture(id);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }

    /// Drop the cached snapshot for a tile (call when the tile closes).
    pub fn drop_tile(&mut self, tile_id: TileId) {
        self.snapshots.remove(&tile_id);
    }

    /// Walk every cell in `snap` and push the bg+fg instances needed to
    /// draw it at `rect`. Takes `&mut pipeline` + `&queue` as disjoint
    /// borrows so the caller can hold them both.
    #[allow(clippy::too_many_arguments)]
    fn emit_tile(
        pipeline: &mut GlyphPipeline,
        queue: &wgpu::Queue,
        theme: &Theme,
        metrics: CellMetrics,
        snap: &TileSnapshot,
        rect: Rect,
        focused: bool,
        generating: bool,
        _primary: bool,
        _follow_mode: bool,
        tile_index: usize,
        t_secs: f64,
    ) {
        if snap.cols == 0 || snap.rows == 0 || snap.cells.is_empty() {
            return;
        }
        let dim = if focused { 1.0 } else { UNFOCUSED_DIM };
        // Cursor blink: 530ms on, 530ms off (stepped, like koo-blink).
        let cursor_visible = ((t_secs / 0.53) as u64) % 2 == 0;
        let cursor = if focused && cursor_visible { snap.cursor } else { None };
        let cw = metrics.width;
        let ch = metrics.height;
        let ascent = metrics.ascent;
        let bg_default = theme.background;
        let cols = snap.cols as usize;
        let rows = snap.rows as usize;

        let tile_w = cols as f32 * cw;
        let tile_h = rows as f32 * ch;

        // --- tile background fill ---
        pipeline.push_bg(rect.x, rect.y, tile_w, tile_h, bg_default);

        // --- tile border ---
        // Mirror the strip card's Stroke::new(2.0, ACCENT/GRID_LINE): flat
        // 2px all around, amber when focused, grid-line otherwise.
        let accent = Rgba::rgb(0xff, 0xa5, 0x1c);
        let grid_line = Rgba::rgb(0x1a, 0x15, 0x10);
        let border_color = if focused { accent } else { grid_line };
        let border_w = 2.0_f32;
        // Top
        pipeline.push_bg(rect.x - border_w, rect.y - border_w, tile_w + border_w * 2.0, border_w, border_color);
        // Bottom
        pipeline.push_bg(rect.x - border_w, rect.y + tile_h, tile_w + border_w * 2.0, border_w, border_color);
        // Left
        pipeline.push_bg(rect.x - border_w, rect.y, border_w, tile_h, border_color);
        // Right
        pipeline.push_bg(rect.x + tile_w, rect.y, border_w, tile_h, border_color);

        // --- focused tile: 3px amber bottom accent bar ---
        // Mirrors `draw_card`'s 3px bar on the active workspace card so the
        // focused tile reads as "selected" the same way cards do.
        if focused {
            pipeline.push_bg(rect.x, rect.y + tile_h - 3.0, tile_w, 3.0, accent);
        }

        // --- tile header bar ---
        // Flat BG_DEEP in both states — no animated glow. Just a strip for
        // the tile index + title, with a 1px grid-line separator below.
        let header_h = 22.0;
        let header_bg = Rgba::rgb(0x04, 0x03, 0x02); // BG_DEEP
        pipeline.push_bg(rect.x, rect.y, tile_w, header_h, header_bg);
        pipeline.push_bg(rect.x, rect.y + header_h, tile_w, 1.0, grid_line);

        // --- tile index chip ---
        // Mirrors the ⌘N chip on the workspace card: amber fill on focused,
        // grid-line fill otherwise.
        if tile_index > 0 && tile_index <= 9 {
            let chip_w = 16.0;
            let chip_h = 14.0;
            let chip_x = rect.x + 6.0;
            let chip_y = rect.y + (header_h - chip_h) / 2.0;
            let (chip_bg, chip_fg) = if focused {
                (accent, Rgba::rgb(0x08, 0x06, 0x04))
            } else {
                (grid_line, Rgba::rgb(0xa9, 0xa4, 0x9d))
            };
            pipeline.push_bg(chip_x, chip_y, chip_w, chip_h, chip_bg);
            let digit = char::from_digit(tile_index as u32, 10).unwrap_or('?');
            let digit_x = chip_x + (chip_w - cw) / 2.0;
            let digit_y = chip_y + ascent * 0.85;
            pipeline.push_fg(digit, digit_x, digit_y, chip_fg, queue);
        }

        // --- running process indicator dot ---
        // Static 6×6 dot between the index chip and the title. Amber when
        // generating, green on focused idle, grey otherwise. No scale pulse
        // — the strip's tile-count dots aren't animated either.
        let dot_size = 6.0_f32;
        let dot_x = rect.x + 26.0;
        let dot_y = rect.y + (header_h - dot_size) / 2.0;
        let dot_color = if generating {
            accent
        } else if focused {
            Rgba::rgb(0x78, 0xc8, 0x50)
        } else {
            Rgba::rgb(0xa9, 0xa4, 0x9d)
        };
        pipeline.push_bg(dot_x, dot_y, dot_size, dot_size, dot_color);

        // --- tile title text ---
        let title_start_x = rect.x + 36.0;
        if !snap.title.is_empty() {
            let title_fg = if focused {
                theme.foreground
            } else {
                dim_rgba(theme.foreground, dim)
            };
            let title_y = rect.y + (header_h - ch) / 2.0 + ascent;
            let max_chars = ((tile_w - 42.0) / cw) as usize;
            for (i, ch_char) in snap.title.chars().take(max_chars).enumerate() {
                if ch_char != ' ' {
                    pipeline.push_fg(ch_char, title_start_x + i as f32 * cw, title_y, title_fg, queue);
                }
            }
        }

        // Offset cell grid below the header bar + separator.
        let content_y = rect.y + header_h + 1.0;

        for row in 0..rows {
            let row_start = row * cols;
            let cy = content_y + row as f32 * ch;
            let by = cy + ascent;
            for col in 0..cols {
                let cell = &snap.cells[row_start + col];
                let cx = rect.x + col as f32 * cw;

                let is_cursor = matches!(
                    cursor,
                    Some((cc, cr)) if cc as usize == col && cr as usize == row
                );

                // --- bg pass ---
                // Inverse video flips fg/bg for this cell.
                let inverse = cell.flags.contains(CellFlags::INVERSE);
                let (mut cell_fg, mut cell_bg) = if inverse {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                // Empty/zero bg means "default" — don't push a quad for
                // the common case of all-default backgrounds.
                if cell_bg.a == 0 {
                    cell_bg = bg_default;
                }

                if is_cursor {
                    // Cursor owns the bg; glyph fg swaps to theme.background
                    // so the char is legible over the cursor block.
                    pipeline.push_bg(cx, cy, cw, ch, theme.cursor);
                    cell_fg = theme.background;
                } else if cell_bg != bg_default {
                    let bg = if dim < 1.0 {
                        dim_rgba(cell_bg, dim)
                    } else {
                        cell_bg
                    };
                    pipeline.push_bg(cx, cy, cw, ch, bg);
                }

                // --- fg pass ---
                if cell.flags.contains(CellFlags::HIDDEN) {
                    continue;
                }
                let glyph = if cell.ch == '\0' { ' ' } else { cell.ch };
                if glyph == ' ' {
                    continue;
                }
                let fg = if dim < 1.0 {
                    dim_rgba(cell_fg, dim)
                } else {
                    cell_fg
                };
                pipeline.push_fg(glyph, cx, by, fg, queue);
            }
        }
    }
}

fn dim_rgba(c: Rgba, factor: f32) -> Rgba {
    Rgba {
        r: ((c.r as f32) * factor).round().clamp(0.0, 255.0) as u8,
        g: ((c.g as f32) * factor).round().clamp(0.0, 255.0) as u8,
        b: ((c.b as f32) * factor).round().clamp(0.0, 255.0) as u8,
        a: c.a,
    }
}

fn rgba_to_wgpu_color(c: Rgba) -> wgpu::Color {
    wgpu::Color {
        r: srgb_to_linear(c.r),
        g: srgb_to_linear(c.g),
        b: srgb_to_linear(c.b),
        a: f64::from(c.a) / 255.0,
    }
}

fn srgb_to_linear(c: u8) -> f64 {
    let s = f64::from(c) / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_in_rect_floors_to_at_least_one_cell() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let (c, r) = cells_in_rect(rect, CellMetrics::fallback(14.0));
        assert!(c >= 1);
        assert!(r >= 1);
    }

    #[test]
    fn cells_in_rect_partitions_a_simple_grid() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let metrics = CellMetrics {
            width: 8.0,
            height: 16.0,
            ascent: 14.0,
            descent: 2.0,
        };
        let (c, r) = cells_in_rect(rect, metrics);
        assert_eq!(c, 100);
        assert_eq!(r, 37); // 600 / 16 = 37.5 → 37
    }

    #[test]
    fn cell_metrics_fallback_nonzero() {
        let m = CellMetrics::fallback(14.0);
        assert!(m.width > 0.0);
        assert!(m.height > 0.0);
    }
}
