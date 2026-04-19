//! Custom instanced-quad terminal pipeline.
//!
//! **Why this exists.** Glyphon + cosmic-text are general-purpose text
//! rendering — BiDi, wrap, fallback fonts, complex shaping, per-glyph
//! font selection. A terminal grid needs none of that: every cell is at
//! `(col*cell_w, row*cell_h)`, every glyph advance is exactly `cell_w`,
//! no wrapping, no BiDi. We bypass the layout engine entirely and
//! rasterize each codepoint on demand into a shared R8 atlas, then
//! submit one instanced draw per pass.
//!
//! This is the same shape alacritty / kitty / wezterm use.
//!
//! Typical per-frame cost:
//!   * One instance per non-space cell (pushed into a reusable `Vec`).
//!   * One instance per non-default-bg cell (cursor + future selection).
//!   * Atlas rasterize only on first sighting of a glyph.
//!   * Two draw calls: bg pass, fg pass.

// set_font / set_scale_factor / get_cached held for later phases
// (config reload, monitor DPI, tight-loop batching).
#![allow(dead_code)]

use std::collections::HashMap;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use etagere::{size2, AtlasAllocator};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;
use swash::{FontRef, GlyphId};
use wgpu::util::DeviceExt;

use kookaburra_core::config::Rgba;

/// Atlas size in texels. 1024×1024 holds thousands of ASCII glyphs easily
/// for reasonable font sizes. If we run out, new glyphs fall back to a
/// zero-coverage slot (renders as blank) and we log once — in practice
/// terminal input stays within a tight character set.
const ATLAS_DIM: u32 = 1024;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct BgInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: u32,
    _pad: [u32; 3],
}

impl BgInstance {
    pub const STRIDE: u64 = std::mem::size_of::<Self>() as u64;

    pub fn new(x: f32, y: f32, w: f32, h: f32, color: Rgba) -> Self {
        Self {
            pos: [x, y],
            size: [w, h],
            color: pack_rgba(color),
            _pad: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct FgInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: u32,
    _pad: [u32; 3],
}

impl FgInstance {
    pub const STRIDE: u64 = std::mem::size_of::<Self>() as u64;
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    surface_size: [f32; 2],
    atlas_size: [f32; 2],
}

/// Pack an `Rgba` (sRGB 8-bit channels) into little-endian `u32` such
/// that byte 0 = r, byte 1 = g, byte 2 = b, byte 3 = a. The shader's
/// `unpack_rgba` reverses this and converts the RGB channels from sRGB
/// to linear so the `Bgra8UnormSrgb` surface's linear->sRGB encoding
/// round-trips to the original byte values. Egui-wgpu does the same
/// conversion, so tile and strip colors agree byte-for-byte.
#[inline]
fn pack_rgba(c: Rgba) -> u32 {
    (c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16) | ((c.a as u32) << 24)
}

/// One glyph's atlas slot and placement metrics relative to the cell.
#[derive(Copy, Clone, Debug)]
pub(crate) struct GlyphSlot {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    /// Pixel size of the rasterized bitmap.
    pixel_size: [f32; 2],
    /// Offset from the pen position (cell-left baseline) to the bitmap
    /// top-left, in pixels.
    offset: [f32; 2],
}

/// Font loaded from disk + scaled raster metrics.
pub struct LoadedFont {
    pub bytes: Vec<u8>,
    pub face_index: u32,
    pub size_px: f32,
    pub ascent: f32,
    pub descent: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl LoadedFont {
    /// Load a monospace font via the given glyphon `FontSystem` and
    /// compute cell metrics for the requested pixel size. Picks the
    /// first monospace regular face fontdb knows about.
    pub fn from_font_system(font_system: &mut glyphon::FontSystem, size_px: f32) -> Option<Self> {
        let db = font_system.db();
        // Find a monospace regular face. On macOS fontdb will populate
        // SFMono, Menlo, Monaco, etc.
        let face = db
            .faces()
            .find(|f| f.monospaced && matches!(f.style, glyphon::fontdb::Style::Normal))
            .or_else(|| db.faces().find(|f| f.monospaced))?;
        let face_id = face.id;
        let face_index = face.index;

        let bytes = db.with_face_data(face_id, |data, _idx| data.to_vec())?;

        let font = FontRef::from_index(&bytes, face_index as usize)?;
        let metrics = font.metrics(&[]).scale(size_px);
        let ascent = metrics.ascent.ceil();
        let descent = metrics.descent.ceil();
        let leading = metrics.leading.max(0.0);
        let cell_height = (ascent + descent + leading).ceil().max(size_px * 1.2);

        // Cell width = advance width of 'M' in a monospace font. If
        // unavailable (0-advance), fall back to 0.6× size which matches
        // typical monospace proportions.
        let gmetrics = font.glyph_metrics(&[]);
        let m_glyph: GlyphId = font.charmap().map('M' as u32);
        let raw_advance = gmetrics.advance_width(m_glyph);
        let cell_width = if raw_advance > 0.0 && metrics.units_per_em > 0 {
            (raw_advance * size_px / metrics.units_per_em as f32).ceil()
        } else {
            (size_px * 0.6).ceil()
        };

        Some(Self {
            bytes,
            face_index,
            size_px,
            ascent,
            descent,
            cell_width,
            cell_height,
        })
    }

    #[must_use]
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.bytes, self.face_index as usize)
    }
}

/// The whole rendering pipeline.
pub struct GlyphPipeline {
    font: LoadedFont,
    scale_factor: f32,

    // Per-glyph atlas.
    atlas_texture: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    allocator: AtlasAllocator,
    glyphs: HashMap<u32, GlyphSlot>,
    /// Tombstone slot — used on atlas-full or on codepoints we can't
    /// rasterize (e.g. skin-tone emoji). Transparent; renders as blank.
    empty_slot: GlyphSlot,

    // swash state.
    scaler_ctx: ScaleContext,

    // GPU resources.
    uniform_buf: wgpu::Buffer,
    bg_pipeline: wgpu::RenderPipeline,
    fg_pipeline: wgpu::RenderPipeline,
    bg_bind_group: wgpu::BindGroup,
    fg_bind_group: wgpu::BindGroup,
    bg_buffer: wgpu::Buffer,
    bg_capacity: u64,
    fg_buffer: wgpu::Buffer,
    fg_capacity: u64,

    /// Scratch — reused across frames.
    pub bg_instances: Vec<BgInstance>,
    pub fg_instances: Vec<FgInstance>,
    /// Reusable buffer for swash Image data; avoids per-glyph alloc.
    scratch_image: swash::scale::image::Image,
}

impl GlyphPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        font: LoadedFont,
        scale_factor: f32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kookaburra-glyph-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "glyph.wgsl"
            ))),
        });

        // Uniforms.
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("kookaburra-glyph-uniforms"),
            contents: bytemuck::cast_slice(&[Uniforms {
                surface_size: [1.0, 1.0],
                atlas_size: [ATLAS_DIM as f32, ATLAS_DIM as f32],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Atlas texture (R8Unorm, 1024×1024).
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kookaburra-glyph-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_DIM,
                height: ATLAS_DIM,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kookaburra-atlas-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // Nearest for the terminal — bilinear makes glyph edges look
            // muddy. Rasterization already produces alpha-coverage.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Layouts: bg = uniform only, fg = uniform + texture + sampler.
        let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let fg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fg-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        let bg_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg-bind"),
            layout: &bg_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let fg_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fg-bind"),
            layout: &fg_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let bg_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg-pipeline-layout"),
            bind_group_layouts: &[&bg_layout],
            push_constant_ranges: &[],
        });
        let fg_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fg-pipeline-layout"),
            bind_group_layouts: &[&fg_layout],
            push_constant_ranges: &[],
        });

        let bg_vertex_buffers = [wgpu::VertexBufferLayout {
            array_stride: BgInstance::STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2, // pos
                1 => Float32x2, // size
                2 => Uint32,    // color
            ],
        }];
        let fg_vertex_buffers = [wgpu::VertexBufferLayout {
            array_stride: FgInstance::STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2, // pos
                1 => Float32x2, // size
                2 => Float32x2, // uv_min
                3 => Float32x2, // uv_max
                4 => Uint32,    // color
            ],
        }];

        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg-pipeline"),
            layout: Some(&bg_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_bg",
                compilation_options: Default::default(),
                buffers: &bg_vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_bg",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let fg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fg-pipeline"),
            layout: Some(&fg_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_fg",
                compilation_options: Default::default(),
                buffers: &fg_vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_fg",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Pre-size instance buffers for ~8000 cells (6 tiles × 80 × 50 with
        // some headroom). They grow on demand.
        let initial_capacity = (8192u64 * BgInstance::STRIDE.max(FgInstance::STRIDE)).max(4096);
        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg-instances"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fg-instances"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let _ = queue; // no queue work yet; atlas writes happen on first glyph

        Self {
            font,
            scale_factor,
            atlas_texture,
            atlas_view,
            allocator: AtlasAllocator::new(size2(ATLAS_DIM as i32, ATLAS_DIM as i32)),
            glyphs: HashMap::new(),
            empty_slot: GlyphSlot {
                uv_min: [0.0, 0.0],
                uv_max: [0.0, 0.0],
                pixel_size: [0.0, 0.0],
                offset: [0.0, 0.0],
            },
            scaler_ctx: ScaleContext::new(),
            uniform_buf,
            bg_pipeline,
            fg_pipeline,
            bg_bind_group,
            fg_bind_group,
            bg_buffer,
            bg_capacity: initial_capacity,
            fg_buffer,
            fg_capacity: initial_capacity,
            bg_instances: Vec::with_capacity(2048),
            fg_instances: Vec::with_capacity(8192),
            scratch_image: swash::scale::image::Image::new(),
        }
    }

    #[must_use]
    pub fn cell_size(&self) -> (f32, f32) {
        (self.font.cell_width, self.font.cell_height)
    }

    #[must_use]
    pub fn ascent(&self) -> f32 {
        self.font.ascent
    }

    /// Replace the loaded font (e.g., on config reload or scale-factor
    /// change). Clears the atlas so stale glyphs aren't reused.
    pub fn set_font(&mut self, queue: &wgpu::Queue, font: LoadedFont) {
        self.font = font;
        self.clear_atlas(queue);
    }

    pub fn set_scale_factor(&mut self, queue: &wgpu::Queue, scale_factor: f32) {
        if (self.scale_factor - scale_factor).abs() > f32::EPSILON {
            self.scale_factor = scale_factor;
            self.clear_atlas(queue);
        }
    }

    fn clear_atlas(&mut self, queue: &wgpu::Queue) {
        self.allocator = AtlasAllocator::new(size2(ATLAS_DIM as i32, ATLAS_DIM as i32));
        self.glyphs.clear();
        // Wipe the texture so sampling misses don't pull stale data.
        let zero = vec![0u8; (ATLAS_DIM * ATLAS_DIM) as usize];
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zero,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_DIM),
                rows_per_image: Some(ATLAS_DIM),
            },
            wgpu::Extent3d {
                width: ATLAS_DIM,
                height: ATLAS_DIM,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Look up a glyph slot for `ch`, rasterizing into the atlas on miss.
    /// Returns `None` if `ch` has no outline in the font or the atlas is
    /// full (caller falls back to blank).
    pub fn glyph(&mut self, queue: &wgpu::Queue, ch: char) -> GlyphSlot {
        let code = ch as u32;
        if let Some(slot) = self.glyphs.get(&code) {
            return *slot;
        }
        // Cold path: rasterize.
        let font = match self.font.font() {
            Some(f) => f,
            None => return self.empty_slot,
        };
        let glyph_id = font.charmap().map(code);
        if glyph_id == 0 {
            self.glyphs.insert(code, self.empty_slot);
            return self.empty_slot;
        }

        let mut scaler = self
            .scaler_ctx
            .builder(font)
            .size(self.font.size_px * self.scale_factor)
            .hint(true)
            .build();

        self.scratch_image.clear();
        let ok = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render_into(&mut scaler, glyph_id, &mut self.scratch_image);
        if !ok {
            self.glyphs.insert(code, self.empty_slot);
            return self.empty_slot;
        }
        let w = self.scratch_image.placement.width as i32;
        let h = self.scratch_image.placement.height as i32;
        if w <= 0 || h <= 0 {
            // Glyph with zero-area bitmap (e.g. space). Cache an empty
            // slot so we don't keep re-rasterizing.
            self.glyphs.insert(code, self.empty_slot);
            return self.empty_slot;
        }

        // +1 padding on right/bottom to stop neighbours bleeding under
        // nearest-filter sampling at the atlas edge.
        let alloc = match self.allocator.allocate(size2(w + 1, h + 1)) {
            Some(a) => a,
            None => {
                log::warn!(
                    "glyph atlas full, dropping glyph U+{:04X}; increase ATLAS_DIM",
                    code
                );
                self.glyphs.insert(code, self.empty_slot);
                return self.empty_slot;
            }
        };
        let rect = alloc.rectangle;
        let x = rect.min.x as u32;
        let y = rect.min.y as u32;

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &self.scratch_image.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(w as u32),
                rows_per_image: Some(h as u32),
            },
            wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
        );

        let atlas_dim = ATLAS_DIM as f32;
        let slot = GlyphSlot {
            uv_min: [x as f32 / atlas_dim, y as f32 / atlas_dim],
            uv_max: [
                (x + w as u32) as f32 / atlas_dim,
                (y + h as u32) as f32 / atlas_dim,
            ],
            pixel_size: [w as f32 / self.scale_factor, h as f32 / self.scale_factor],
            offset: [
                self.scratch_image.placement.left as f32 / self.scale_factor,
                -self.scratch_image.placement.top as f32 / self.scale_factor,
            ],
        };
        self.glyphs.insert(code, slot);
        slot
    }

    /// Look up a glyph slot **without** atlas mutation. Returns `None` on
    /// miss. Useful when building the fg instance list in a tight loop
    /// that can't borrow the pipeline mutably — caller pre-warms with
    /// [`Self::glyph`] first.
    pub fn get_cached(&self, ch: char) -> Option<GlyphSlot> {
        self.glyphs.get(&(ch as u32)).copied()
    }

    /// Push a glyph instance into `fg_instances`. `cell_x` and `cell_y`
    /// are logical pixels of the cell's top-left; `baseline_y` is the
    /// baseline (cell_y + ascent).
    pub fn push_fg(
        &mut self,
        ch: char,
        cell_x: f32,
        baseline_y: f32,
        color: Rgba,
        queue: &wgpu::Queue,
    ) {
        if ch == ' ' || ch == '\0' {
            return;
        }
        let slot = self.glyph(queue, ch);
        if slot.pixel_size[0] <= 0.0 {
            return;
        }
        self.fg_instances.push(FgInstance {
            pos: [cell_x + slot.offset[0], baseline_y + slot.offset[1]],
            size: slot.pixel_size,
            uv_min: slot.uv_min,
            uv_max: slot.uv_max,
            color: pack_rgba(color),
            _pad: [0; 3],
        });
    }

    pub fn push_bg(&mut self, cell_x: f32, cell_y: f32, w: f32, h: f32, color: Rgba) {
        if color.a == 0 {
            return;
        }
        self.bg_instances
            .push(BgInstance::new(cell_x, cell_y, w, h, color));
    }

    /// Upload current instance buffers and issue draw calls into `pass`.
    /// `surface_size` is in physical pixels.
    pub fn render<'pass>(
        &'pass mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'pass>,
        surface_size: (u32, u32),
    ) {
        // Update uniforms each frame — cheap and handles resize.
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[Uniforms {
                surface_size: [surface_size.0 as f32, surface_size.1 as f32],
                atlas_size: [ATLAS_DIM as f32, ATLAS_DIM as f32],
            }]),
        );

        let bg_bytes = bytemuck::cast_slice::<BgInstance, u8>(&self.bg_instances);
        let fg_bytes = bytemuck::cast_slice::<FgInstance, u8>(&self.fg_instances);
        if (bg_bytes.len() as u64) > self.bg_capacity {
            self.bg_capacity = (bg_bytes.len() as u64 * 2).max(self.bg_capacity * 2);
            self.bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bg-instances"),
                size: self.bg_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if (fg_bytes.len() as u64) > self.fg_capacity {
            self.fg_capacity = (fg_bytes.len() as u64 * 2).max(self.fg_capacity * 2);
            self.fg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fg-instances"),
                size: self.fg_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bg_bytes.is_empty() {
            queue.write_buffer(&self.bg_buffer, 0, bg_bytes);
        }
        if !fg_bytes.is_empty() {
            queue.write_buffer(&self.fg_buffer, 0, fg_bytes);
        }

        if !self.bg_instances.is_empty() {
            pass.set_pipeline(&self.bg_pipeline);
            pass.set_bind_group(0, &self.bg_bind_group, &[]);
            pass.set_vertex_buffer(0, self.bg_buffer.slice(..));
            pass.draw(0..6, 0..self.bg_instances.len() as u32);
        }
        if !self.fg_instances.is_empty() {
            pass.set_pipeline(&self.fg_pipeline);
            pass.set_bind_group(0, &self.fg_bind_group, &[]);
            pass.set_vertex_buffer(0, self.fg_buffer.slice(..));
            pass.draw(0..6, 0..self.fg_instances.len() as u32);
        }
    }

    pub fn clear_instances(&mut self) {
        self.bg_instances.clear();
        self.fg_instances.clear();
    }
}
