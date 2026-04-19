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

        // Grid background: fill the entire tile area with grid-line color
        // so the gaps between tiles read as visible dividers. The tiles
        // themselves will paint their own bg over this.
        if let (Some(first), Some(last)) = (tiles.first(), tiles.last()) {
            let grid_color = Rgba::rgb(0x48, 0x40, 0x3a); // GRID_LINE
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

        // --- CRT scanline overlay ---
        // Semi-transparent black lines every 3rd pixel row across the
        // visible surface. A moving brightness wave simulates CRT refresh.
        let surf_w = self.surface_config.width as f32;
        let surf_h = self.surface_config.height as f32;
        {
            // CRT refresh wave: a bright band that scrolls downward at
            // ~120px/sec (wraps around the screen height).
            let wave_y = ((t_secs * 120.0) % surf_h as f64) as f32;
            let wave_half = 60.0_f32; // half-width of the bright band
            let mut y = 0.0_f32;
            while y < surf_h {
                // Distance from the wave center → modulate alpha.
                let dist = (y - wave_y).abs().min(surf_h - (y - wave_y).abs());
                let wave_factor = if dist < wave_half {
                    1.0 - dist / wave_half
                } else {
                    0.0
                };
                // Base alpha ~7%, wave reduces it by up to 40% (brighter band).
                let alpha = (18.0 - 7.0 * wave_factor).max(0.0) as u8;
                let scanline_color = Rgba { r: 0, g: 0, b: 0, a: alpha };
                self.pipeline.push_bg(0.0, y, surf_w, 1.0, scanline_color);
                y += 3.0;
            }
        }

        // --- Vignette ---
        // Darken edges with a stepped radial approximation: four border
        // strips with increasing opacity toward the edges.
        {
            let vignette_steps: &[(f32, u8)] = &[
                (0.08, 8),   // outer 8% of each edge, alpha 8
                (0.05, 16),  // outer 5%, alpha 16
                (0.025, 28), // outer 2.5%, alpha 28
            ];
            for &(frac, alpha) in vignette_steps {
                let v = Rgba { r: 0, g: 0, b: 0, a: alpha };
                let dx = surf_w * frac;
                let dy = surf_h * frac;
                // Left
                self.pipeline.push_bg(0.0, 0.0, dx, surf_h, v);
                // Right
                self.pipeline.push_bg(surf_w - dx, 0.0, dx, surf_h, v);
                // Top
                self.pipeline.push_bg(0.0, 0.0, surf_w, dy, v);
                // Bottom
                self.pipeline.push_bg(0.0, surf_h - dy, surf_w, dy, v);
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
        primary: bool,
        follow_mode: bool,
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

        // --- pixel drop shadow ---
        // Hard 2px + 4px stepped shadow (from pixel.css .koo-tile).
        {
            let shadow1 = Rgba { r: 0, g: 0, b: 0, a: 230 }; // 90% opacity
            let shadow2 = Rgba { r: 0, g: 0, b: 0, a: 115 }; // 45% opacity
            pipeline.push_bg(rect.x + 2.0, rect.y + 2.0, tile_w, tile_h, shadow1);
            pipeline.push_bg(rect.x + 4.0, rect.y + 4.0, tile_w, tile_h, shadow2);
        }

        // --- tile background fill ---
        pipeline.push_bg(rect.x, rect.y, tile_w, tile_h, bg_default);

        // --- tile border ---
        // Focused = bright amber, primary-but-not-focused = muted amber
        // (accentDeep), otherwise grid line.
        let border_color = if focused {
            Rgba::rgb(0xd4, 0xa0, 0x40) // ACCENT amber
        } else if primary {
            Rgba::rgb(0x8f, 0x60, 0x20) // ACCENT_DEEP
        } else {
            Rgba::rgb(0x48, 0x40, 0x3a) // GRID_LINE
        };
        let border_w = 2.0;
        // Top
        pipeline.push_bg(rect.x - border_w, rect.y - border_w, tile_w + border_w * 2.0, border_w, border_color);
        // Bottom
        pipeline.push_bg(rect.x - border_w, rect.y + tile_h, tile_w + border_w * 2.0, border_w, border_color);
        // Left
        pipeline.push_bg(rect.x - border_w, rect.y, border_w, tile_h, border_color);
        // Right
        pipeline.push_bg(rect.x + tile_w, rect.y, border_w, tile_h, border_color);

        // --- tile header bar ---
        let header_h = 22.0;
        let header_bg = if focused {
            Rgba::rgb(0x36, 0x30, 0x2a) // BG_DIM
        } else {
            Rgba::rgb(0x2b, 0x24, 0x20) // BG
        };
        pipeline.push_bg(rect.x, rect.y, cols as f32 * cw, header_h, header_bg);
        // Header bottom separator
        pipeline.push_bg(rect.x, rect.y + header_h, cols as f32 * cw, 1.0, border_color);

        // --- focused tile inner shadow (inset 0 0 0 1px) ---
        // The design applies `box-shadow: inset 0 0 0 1px bgColor` to the
        // focused tile, creating a subtle inner frame. We approximate with
        // 1px semi-transparent strips just inside the tile.
        if focused {
            let inner = Rgba { r: 0x2b, g: 0x24, b: 0x20, a: 100 }; // bg at ~40%
            // top inner
            pipeline.push_bg(rect.x + 1.0, rect.y + 1.0, tile_w - 2.0, 1.0, inner);
            // bottom inner
            pipeline.push_bg(rect.x + 1.0, rect.y + tile_h - 2.0, tile_w - 2.0, 1.0, inner);
            // left inner
            pipeline.push_bg(rect.x + 1.0, rect.y + 1.0, 1.0, tile_h - 2.0, inner);
            // right inner
            pipeline.push_bg(rect.x + tile_w - 2.0, rect.y + 1.0, 1.0, tile_h - 2.0, inner);
        }

        // --- focused tile: marching ants top accent ---
        if focused {
            // Animated marching ants: alternating 8px amber / 8px gap segments
            // that scroll rightward at ~6.67px/sec (2.4s period = 16px).
            let accent = Rgba::rgb(0xd4, 0xa0, 0x40);
            let segment = 8.0_f32;
            let period = 2.4; // seconds for one full segment cycle
            let offset = ((t_secs % period) / period * (segment * 2.0) as f64) as f32;
            let mut sx = rect.x - offset;
            while sx < rect.x + tile_w {
                let x0 = sx.max(rect.x);
                let x1 = (sx + segment).min(rect.x + tile_w);
                if x1 > x0 {
                    pipeline.push_bg(x0, rect.y - border_w, x1 - x0, border_w, accent);
                }
                sx += segment * 2.0;
            }

            // Subtle glow: a 1px amber line with reduced alpha below the
            // marching ants, extending across the full width.
            let glow = Rgba { r: 0xd4, g: 0xa0, b: 0x40, a: 60 };
            pipeline.push_bg(rect.x, rect.y - border_w - 1.0, tile_w, 1.0, glow);
        }

        // --- tile index chip ---
        // Render the tile number (1-based) in the header bar as a colored chip.
        if tile_index > 0 && tile_index <= 9 {
            let chip_w = 16.0;
            let chip_h = 14.0;
            let chip_x = rect.x + 6.0;
            let chip_y = rect.y + (header_h - chip_h) / 2.0;
            let chip_bg = if focused {
                Rgba::rgb(0xd4, 0xa0, 0x40) // ACCENT
            } else {
                Rgba::rgb(0x48, 0x40, 0x3a) // GRID_LINE
            };
            let chip_fg = if focused {
                Rgba::rgb(0x20, 0x1c, 0x18) // BG_DEEP (dark on amber)
            } else {
                Rgba::rgb(0xa9, 0xa4, 0x9d) // FG_DIM
            };
            pipeline.push_bg(chip_x, chip_y, chip_w, chip_h, chip_bg);
            // Render the digit character
            let digit = char::from_digit(tile_index as u32, 10).unwrap_or('?');
            let digit_x = chip_x + (chip_w - cw) / 2.0;
            let digit_y = chip_y + ascent * 0.85;
            pipeline.push_fg(digit, digit_x, digit_y, chip_fg, queue);
        }

        // --- running process indicator dot ---
        // A 6×6 square between the index chip and the title. When
        // generating, it pulses (koo-pulse: opacity + scale oscillation).
        let dot_base_size = 6.0_f32;
        let dot_x = rect.x + 26.0;
        let dot_y = rect.y + (header_h - dot_base_size) / 2.0;
        {
            let (dot_color, dot_size) = if generating {
                // koo-pulse: opacity 0.5–1.0, scale 0.7–1.0 at 1.2s period.
                let phase = (t_secs * std::f64::consts::TAU / 1.2).sin() as f32;
                let opacity = 0.5 + 0.5 * phase;
                let scale = 0.7 + 0.3 * (phase + 1.0) / 2.0;
                let alpha = (255.0 * opacity) as u8;
                (Rgba { r: 0xd4, g: 0xa0, b: 0x40, a: alpha }, dot_base_size * scale)
            } else if focused {
                (Rgba::rgb(0x78, 0xc8, 0x50), dot_base_size) // GREEN
            } else {
                (Rgba::rgb(0xa9, 0xa4, 0x9d), dot_base_size) // FG_DIM
            };
            // Center the dot if it's scaled down.
            let offset = (dot_base_size - dot_size) / 2.0;
            pipeline.push_bg(dot_x + offset, dot_y + offset, dot_size, dot_size, dot_color);
        }

        // --- tile title text ---
        // Render the tile title (from OSC) in the header bar.
        let title_start_x = rect.x + 36.0; // after index chip + dot
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

        // --- primary star indicator (◆) ---
        // The design shows a ◆ in the tile header right side when this tile
        // is the designated primary tile. 11px amber glyph, right-aligned
        // in the header.
        if primary {
            let star_color = Rgba::rgb(0xd4, 0xa0, 0x40); // ACCENT
            let star_x = rect.x + tile_w - 18.0;
            let star_y = rect.y + (header_h - ch) / 2.0 + ascent;
            pipeline.push_fg('◆', star_x, star_y, star_color, queue);
        }

        // --- follow-mode indicator ("▼ follow") ---
        // Rendered at the tile's bottom-right corner in teal, matching the
        // design's follow-mode hint.
        if follow_mode {
            let follow_color = Rgba::rgb(0x5c, 0xb8, 0xb8); // TEAL
            let text = "▼ follow";
            let follow_x = rect.x + tile_w - (text.len() as f32) * cw - 8.0;
            let follow_y = rect.y + tile_h - 6.0;
            for (i, c) in text.chars().enumerate() {
                if c != ' ' {
                    pipeline.push_fg(c, follow_x + i as f32 * cw, follow_y, follow_color, queue);
                }
            }
        }

        // --- generating: output drip effect ---
        // When a tile is actively streaming, draw a short animated "drip"
        // below the header separator — an amber bar that grows downward
        // then fades, inspired by the design's OutputDrip component.
        if generating {
            let drip_period = 1.4; // seconds per drip cycle
            let drip_phase = (t_secs % drip_period) / drip_period;
            let drip_h = if drip_phase < 0.7 {
                // Growing phase: 0 → 48px
                (drip_phase / 0.7) as f32 * 48.0
            } else {
                48.0
            };
            let drip_alpha = if drip_phase < 0.7 {
                255
            } else {
                // Fade out: 255 → 0
                (255.0 * (1.0 - (drip_phase - 0.7) / 0.3) as f32) as u8
            };
            if drip_alpha > 0 && drip_h > 0.0 {
                let drip_color = Rgba { r: 0xd4, g: 0xa0, b: 0x40, a: drip_alpha / 3 };
                // Drip bar at left edge of tile, just below header separator
                pipeline.push_bg(rect.x, rect.y + header_h + 1.0, 3.0, drip_h, drip_color);
                // Mirror on right edge
                pipeline.push_bg(rect.x + tile_w - 3.0, rect.y + header_h + 1.0, 3.0, drip_h, drip_color);
            }

            // Pulsing header glow: subtle amber overlay on the header bar
            // that breathes with a sine wave.
            let glow_alpha = (20.0 + 15.0 * (t_secs * std::f64::consts::TAU / 1.2).sin() as f32) as u8;
            let glow = Rgba { r: 0xd4, g: 0xa0, b: 0x40, a: glow_alpha };
            pipeline.push_bg(rect.x, rect.y, tile_w, header_h, glow);
        }

        // Offset cell grid below the header bar + separator.
        let content_y = rect.y + header_h + 1.0;

        // --- dithered background pattern ---
        // Subtle horizontal lines every 4px across the tile content area,
        // mirroring pixel.css's `repeating-linear-gradient` dither.
        {
            let dither = Rgba { r: 255, g: 255, b: 255, a: 4 }; // ~1.5% white
            let content_h = tile_h - header_h - 1.0;
            let mut dy = 0.0_f32;
            while dy < content_h {
                pipeline.push_bg(rect.x, content_y + dy, tile_w, 2.0, dither);
                dy += 4.0;
            }
        }

        // --- koo-glitch: occasional horizontal offset on a few rows ---
        // Triggers for ~200ms every ~10 seconds. During the glitch window,
        // rows whose index matches a pseudo-random pattern get a 1-2px
        // horizontal shift. Very subtle, very retro.
        let glitch_period = 10.0;
        let glitch_phase = t_secs % glitch_period;
        let glitching = glitch_phase < 0.2; // active for 200ms
        let glitch_seed = (t_secs * 5.0) as u32;

        for row in 0..rows {
            let row_start = row * cols;
            let cy = content_y + row as f32 * ch;
            let by = cy + ascent;
            // Glitch offset for this row (if active).
            let row_glitch_x = if glitching {
                // Pseudo-random: XOR row with seed, check low bits.
                let h = (row as u32) ^ glitch_seed;
                if h % 5 == 0 {
                    // Shift this row by -2, -1, +1, or +2 px.
                    let shift = ((h >> 2) % 5) as f32 - 2.0;
                    shift
                } else {
                    0.0
                }
            } else {
                0.0
            };
            for col in 0..cols {
                let cell = &snap.cells[row_start + col];
                let cx = rect.x + col as f32 * cw + row_glitch_x;

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
