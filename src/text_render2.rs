//! Defines the types used to render text.
//!
use crate::{
    custom_glyph::CustomGlyphCacheKey, text_render::GlyphonCacheKey, ColorMode, ContentType,
    FontSystem, GlyphDetails, GlyphToRender, GpuCacheStatus, PrepareError,
    RasterizeCustomGlyphRequest, RasterizedCustomGlyph, RenderError, SwashCache, SwashContent,
    TextArea, TextAtlas, TextBounds, Viewport,
};
use cosmic_text::{Color, LayoutGlyph, PhysicalGlyph, SubpixelBin};
use std::{slice, sync::Arc};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, DepthStencilState, Device, Extent3d, ImageCopyTexture,
    ImageDataLayout, MultisampleState, Origin3d, Queue, RenderPass, RenderPipeline, TextureAspect,
    COPY_BUFFER_ALIGNMENT,
};

#[derive(Debug)]
pub struct RenderableTextArea {
    pub(crate) layout_glyphs: Vec<LayoutGlyphs>,
    pub(crate) custom_glyphs: Vec<GlyphToRender>,
}

#[derive(Debug)]
pub(crate) struct LayoutGlyphs {
    bounds: TextBounds,
    glyphs: Vec<GlyphToRender>,
}

/// A text renderer that uses cached glyphs to render text into an existing render pass.
pub struct TextRenderer2 {
    vertex_buffer: Buffer,
    vertex_buffer_size: u64,
    glyph_vertices_len: usize,
    pipeline: Arc<RenderPipeline>,
    position_mapping: PositionMapping,
}

pub struct TextRenderer2Builder<'a> {
    atlas: &'a mut TextAtlas,
    device: &'a Device,
    multisample: MultisampleState,
    depth_stencil: Option<DepthStencilState>,
    position_mapping: PositionMapping,
}

impl<'a> TextRenderer2Builder<'a> {
    pub fn new(atlas: &'a mut TextAtlas, device: &'a Device) -> Self {
        Self {
            atlas,
            device,
            multisample: MultisampleState::default(),
            depth_stencil: None,
            position_mapping: PositionMapping::Subpixel,
        }
    }

    pub fn with_multisample(&mut self, multisample: MultisampleState) -> &mut Self {
        self.multisample = multisample;
        self
    }

    pub fn with_depth_stencil(&mut self, depth_stencil: DepthStencilState) -> &mut Self {
        self.depth_stencil = Some(depth_stencil);
        self
    }

    // TODO: Move to preparer
    pub fn with_position_mapping(&mut self, position_mapping: PositionMapping) -> &mut Self {
        self.position_mapping = position_mapping;
        self
    }

    pub fn build(&mut self) -> TextRenderer2 {
        TextRenderer2::new(
            self.atlas,
            self.device,
            self.multisample,
            self.depth_stencil.clone(),
            self.position_mapping.clone(),
        )
    }
}

#[derive(Debug, Clone)]
pub enum PositionMapping {
    Subpixel,
    Pixel,
}

impl TextRenderer2 {
    /// Creates a new `TextRenderer`.
    fn new(
        atlas: &mut TextAtlas,
        device: &Device,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
        position_mapping: PositionMapping,
    ) -> Self {
        let vertex_buffer_size = next_copy_buffer_size(4096);
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyphon vertices"),
            size: vertex_buffer_size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = atlas.get_or_create_pipeline(device, multisample, depth_stencil);

        Self {
            vertex_buffer,
            vertex_buffer_size,
            glyph_vertices_len: 0,
            pipeline,
            position_mapping,
        }
    }

    pub fn prepare_text_areas<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        //TODO: Fix
        // metadata_to_depth: Option<impl FnMut(usize) -> f32>,
        // rasterize_custom_glyph: Option<
        //     impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
        // >,
    ) -> Result<Vec<RenderableTextArea>, PrepareError> {
        self.prepare_text_areas_with_depth_and_custom(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            text_areas,
            cache,
            // TODO: Fix
            zero_depth,
            |_| None,
        )
    }

    pub fn prepare_renderable_text_areas(
        &mut self,
        device: &Device,
        queue: &Queue,
        renderable_text_areas: &[RenderableTextArea],
    ) {
        // TODO: Consider culling

        let glyph_vertices = renderable_text_areas
            .iter()
            .flat_map(|renderable_text_area| {
                renderable_text_area
                    .layout_glyphs
                    .iter()
                    .flat_map(|layout_glyphs| {
                        layout_glyphs
                            .glyphs
                            .iter()
                            .chain(renderable_text_area.custom_glyphs.iter())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();

        self.glyph_vertices_len = glyph_vertices.len();

        let vertices = glyph_vertices.as_slice();

        let vertices_raw = unsafe {
            slice::from_raw_parts(
                vertices as *const _ as *const u8,
                std::mem::size_of_val(vertices),
            )
        };

        if self.vertex_buffer_size >= vertices_raw.len() as u64 {
            queue.write_buffer(&self.vertex_buffer, 0, vertices_raw);
        } else {
            self.vertex_buffer.destroy();

            let (buffer, buffer_size) = create_oversized_buffer(
                device,
                Some("glyphon vertices"),
                vertices_raw,
                BufferUsages::VERTEX | BufferUsages::COPY_DST,
            );

            self.vertex_buffer = buffer;
            self.vertex_buffer_size = buffer_size;
        }
    }

    pub fn render(
        &self,
        atlas: &TextAtlas,
        viewport: &Viewport,
        pass: &mut RenderPass<'_>,
    ) -> Result<(), RenderError> {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..4, 0..self.glyph_vertices_len as u32);

        Ok(())
    }

    fn prepare_text_areas_with_depth_and_custom<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        text_areas: impl IntoIterator<Item = TextArea<'a>>,
        cache: &mut SwashCache,
        mut metadata_to_depth: impl FnMut(usize) -> f32,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
    ) -> Result<Vec<RenderableTextArea>, PrepareError> {
        let mut renderable_text_areas = Vec::new();

        for text_area in text_areas {
            let bounds_min_x = text_area.bounds.left.max(0);
            let bounds_min_y = text_area.bounds.top.max(0);
            let bounds_max_x = text_area
                .bounds
                .right
                .min(viewport.resolution().width as i32);
            let bounds_max_y = text_area
                .bounds
                .bottom
                .min(viewport.resolution().height as i32);

            let mut custom_glyph_vertices = Vec::with_capacity(text_area.custom_glyphs.len());

            for glyph in text_area.custom_glyphs.iter() {
                let x = text_area.left + (glyph.left * text_area.scale);
                let y = text_area.top + (glyph.top * text_area.scale);
                let width = (glyph.width * text_area.scale).round() as u16;
                let height = (glyph.height * text_area.scale).round() as u16;

                let (x, y, x_bin, y_bin) =
                    if matches!(self.position_mapping, PositionMapping::Pixel)
                        || glyph.snap_to_physical_pixel
                    {
                        (
                            x.round() as i32,
                            y.round() as i32,
                            SubpixelBin::Zero,
                            SubpixelBin::Zero,
                        )
                    } else {
                        let (x, x_bin) = SubpixelBin::new(x);
                        let (y, y_bin) = SubpixelBin::new(y);
                        (x, y, x_bin, y_bin)
                    };

                let cache_key = GlyphonCacheKey::Custom(CustomGlyphCacheKey {
                    glyph_id: glyph.id,
                    width,
                    height,
                    x_bin,
                    y_bin,
                });

                let color = glyph.color.unwrap_or(text_area.default_color);

                if let Some(glyph_to_render) = prepare_glyph(
                    x,
                    y,
                    0.0,
                    color,
                    glyph.metadata,
                    cache_key,
                    atlas,
                    device,
                    queue,
                    cache,
                    font_system,
                    text_area.scale,
                    bounds_min_x,
                    bounds_min_y,
                    bounds_max_x,
                    bounds_max_y,
                    |_cache, _font_system, rasterize_custom_glyph| -> Option<GetGlyphImageResult> {
                        if width == 0 || height == 0 {
                            return None;
                        }

                        let input = RasterizeCustomGlyphRequest {
                            id: glyph.id,
                            width,
                            height,
                            x_bin,
                            y_bin,
                            scale: text_area.scale,
                        };

                        let output = (rasterize_custom_glyph)(input)?;

                        output.validate(&input, None);

                        Some(GetGlyphImageResult {
                            content_type: output.content_type,
                            top: 0,
                            left: 0,
                            width,
                            height,
                            data: output.data,
                        })
                    },
                    &mut metadata_to_depth,
                    &mut rasterize_custom_glyph,
                )? {
                    custom_glyph_vertices.push(glyph_to_render);
                }
            }

            let layout_runs = text_area.buffer.layout_runs();

            let mut layout_glyphs = Vec::new();

            for run in layout_runs {
                let mut glyph_vertices = Vec::with_capacity(run.glyphs.len());
                for glyph in run.glyphs.iter() {
                    let physical_glyph = self.physical_glyph(glyph, &text_area);

                    let color = match glyph.color_opt {
                        Some(some) => some,
                        None => text_area.default_color,
                    };

                    if let Some(glyph_to_render) = prepare_glyph(
                        physical_glyph.x,
                        physical_glyph.y,
                        run.line_y,
                        color,
                        glyph.metadata,
                        GlyphonCacheKey::Text(physical_glyph.cache_key),
                        atlas,
                        device,
                        queue,
                        cache,
                        font_system,
                        text_area.scale,
                        bounds_min_x,
                        bounds_min_y,
                        bounds_max_x,
                        bounds_max_y,
                        |cache,
                         font_system,
                         _rasterize_custom_glyph|
                         -> Option<GetGlyphImageResult> {
                            let image =
                                cache.get_image_uncached(font_system, physical_glyph.cache_key)?;

                            let content_type = match image.content {
                                SwashContent::Color => ContentType::Color,
                                SwashContent::Mask => ContentType::Mask,
                                SwashContent::SubpixelMask => {
                                    // Not implemented yet, but don't panic if this happens.
                                    ContentType::Mask
                                }
                            };

                            Some(GetGlyphImageResult {
                                content_type,
                                top: image.placement.top as i16,
                                left: image.placement.left as i16,
                                width: image.placement.width as u16,
                                height: image.placement.height as u16,
                                data: image.data,
                            })
                        },
                        &mut metadata_to_depth,
                        &mut rasterize_custom_glyph,
                    )? {
                        glyph_vertices.push(glyph_to_render);
                    }
                }

                layout_glyphs.push(LayoutGlyphs {
                    bounds: TextBounds {
                        top: (text_area.top + run.line_top) as i32,
                        left: text_area.left as i32,
                        right: (text_area.left + run.line_w) as i32,
                        bottom: (text_area.top + run.line_top + run.line_height) as i32,
                    },
                    glyphs: glyph_vertices,
                });
            }

            renderable_text_areas.push(RenderableTextArea {
                layout_glyphs,
                custom_glyphs: custom_glyph_vertices,
            });
        }

        Ok(renderable_text_areas)
    }

    fn physical_glyph(&self, glyph: &LayoutGlyph, text_area: &TextArea) -> PhysicalGlyph {
        let scale = text_area.scale;
        let offset = (text_area.left, text_area.top);

        match self.position_mapping {
            PositionMapping::Subpixel => glyph.physical(offset, scale),
            PositionMapping::Pixel => {
                // Fast path for non subpixel rendering.
                // Avoids calculating the `SubpixelBin`.
                let x_offset = glyph.font_size * glyph.x_offset;
                let y_offset = glyph.font_size * glyph.y_offset;

                let x = ((glyph.x + x_offset) * scale + offset.0) as i32;
                let y = ((glyph.y - y_offset) * scale + offset.1) as i32;

                let cache_key = cosmic_text::CacheKey {
                    font_id: glyph.font_id,
                    glyph_id: glyph.glyph_id,
                    font_size_bits: (glyph.font_size * scale).to_bits(),
                    x_bin: SubpixelBin::Zero,
                    y_bin: SubpixelBin::Zero,
                    flags: glyph.cache_key_flags,
                };

                PhysicalGlyph { cache_key, x, y }
            }
        }
    }
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TextColorConversion {
    None = 0,
    ConvertToLinear = 1,
}

fn next_copy_buffer_size(size: u64) -> u64 {
    let align_mask = COPY_BUFFER_ALIGNMENT - 1;
    ((size.next_power_of_two() + align_mask) & !align_mask).max(COPY_BUFFER_ALIGNMENT)
}

fn create_oversized_buffer(
    device: &Device,
    label: Option<&str>,
    contents: &[u8],
    usage: BufferUsages,
) -> (Buffer, u64) {
    let size = next_copy_buffer_size(contents.len() as u64);
    let buffer = device.create_buffer(&BufferDescriptor {
        label,
        size,
        usage,
        mapped_at_creation: true,
    });
    buffer.slice(..).get_mapped_range_mut()[..contents.len()].copy_from_slice(contents);
    buffer.unmap();
    (buffer, size)
}

fn zero_depth(_: usize) -> f32 {
    0f32
}

struct GetGlyphImageResult {
    content_type: ContentType,
    top: i16,
    left: i16,
    width: u16,
    height: u16,
    data: Vec<u8>,
}

fn prepare_glyph<R>(
    x: i32,
    y: i32,
    line_y: f32,
    color: Color,
    metadata: usize,
    cache_key: GlyphonCacheKey,
    atlas: &mut TextAtlas,
    device: &Device,
    queue: &Queue,
    cache: &mut SwashCache,
    font_system: &mut FontSystem,
    scale_factor: f32,
    bounds_min_x: i32,
    bounds_min_y: i32,
    bounds_max_x: i32,
    bounds_max_y: i32,
    get_glyph_image: impl FnOnce(
        &mut SwashCache,
        &mut FontSystem,
        &mut R,
    ) -> Option<GetGlyphImageResult>,
    mut metadata_to_depth: impl FnMut(usize) -> f32,
    mut rasterize_custom_glyph: R,
) -> Result<Option<GlyphToRender>, PrepareError>
where
    R: FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
{
    let details = if let Some(details) = atlas.mask_atlas.glyph_cache.get(&cache_key) {
        details
    } else if let Some(details) = atlas.color_atlas.glyph_cache.get(&cache_key) {
        details
    } else {
        let Some(image) = (get_glyph_image)(cache, font_system, &mut rasterize_custom_glyph) else {
            return Ok(None);
        };

        let should_rasterize = image.width > 0 && image.height > 0;

        let (gpu_cache, atlas_id, inner) = if should_rasterize {
            let mut inner = atlas.inner_for_content_mut(image.content_type);

            // Find a position in the packer
            let allocation = loop {
                match inner.try_allocate(image.width as usize, image.height as usize) {
                    Some(a) => break a,
                    None => {
                        if !atlas.grow(
                            device,
                            queue,
                            font_system,
                            cache,
                            image.content_type,
                            scale_factor,
                            &mut rasterize_custom_glyph,
                        ) {
                            return Err(PrepareError::AtlasFull);
                        }

                        inner = atlas.inner_for_content_mut(image.content_type);
                    }
                }
            };
            let atlas_min = allocation.rectangle.min;

            queue.write_texture(
                ImageCopyTexture {
                    texture: &inner.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: atlas_min.x as u32,
                        y: atlas_min.y as u32,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image.data,
                ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(image.width as u32 * inner.num_channels() as u32),
                    rows_per_image: None,
                },
                Extent3d {
                    width: image.width as u32,
                    height: image.height as u32,
                    depth_or_array_layers: 1,
                },
            );

            (
                GpuCacheStatus::InAtlas {
                    x: atlas_min.x as u16,
                    y: atlas_min.y as u16,
                    content_type: image.content_type,
                },
                Some(allocation.id),
                inner,
            )
        } else {
            let inner = &mut atlas.color_atlas;
            (GpuCacheStatus::SkipRasterization, None, inner)
        };

        inner.glyph_cache.entry(cache_key).or_insert(GlyphDetails {
            width: image.width,
            height: image.height,
            gpu_cache,
            atlas_id,
            top: image.top,
            left: image.left,
        })
    };

    let mut x = x + details.left as i32;
    let mut y = (line_y * scale_factor).round() as i32 + y - details.top as i32;

    let (mut atlas_x, mut atlas_y, content_type) = match details.gpu_cache {
        GpuCacheStatus::InAtlas { x, y, content_type } => (x, y, content_type),
        GpuCacheStatus::SkipRasterization => return Ok(None),
    };

    let mut width = details.width as i32;
    let mut height = details.height as i32;

    // Starts beyond right edge or ends beyond left edge
    let max_x = x + width;
    if x > bounds_max_x || max_x < bounds_min_x {
        return Ok(None);
    }

    // Starts beyond bottom edge or ends beyond top edge
    let max_y = y + height;
    if y > bounds_max_y || max_y < bounds_min_y {
        return Ok(None);
    }

    // Clip left ege
    if x < bounds_min_x {
        let right_shift = bounds_min_x - x;

        x = bounds_min_x;
        width = max_x - bounds_min_x;
        atlas_x += right_shift as u16;
    }

    // Clip right edge
    if x + width > bounds_max_x {
        width = bounds_max_x - x;
    }

    // Clip top edge
    if y < bounds_min_y {
        let bottom_shift = bounds_min_y - y;

        y = bounds_min_y;
        height = max_y - bounds_min_y;
        atlas_y += bottom_shift as u16;
    }

    // Clip bottom edge
    if y + height > bounds_max_y {
        height = bounds_max_y - y;
    }

    let depth = metadata_to_depth(metadata);

    Ok(Some(GlyphToRender {
        pos: [x, y],
        dim: [width as u16, height as u16],
        uv: [atlas_x, atlas_y],
        color: color.0,
        content_type_with_srgb: [
            content_type as u16,
            match atlas.color_mode {
                ColorMode::Accurate => TextColorConversion::ConvertToLinear,
                ColorMode::Web => TextColorConversion::None,
            } as u16,
        ],
        depth,
    }))
}
