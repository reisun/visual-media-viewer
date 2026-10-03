use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::OnceLock;

pub struct DecodedImage {
    pub pixels: egui::ColorImage,
    pub original_size: [usize; 2],
}

impl DecodedImage {
    pub fn load_preview(
        path: &Path,
        preview_dim: u32,
        max_texture_dim: u32,
    ) -> Result<Self, String> {
        let is_jpeg = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"));
        if is_jpeg {
            if let Ok(image) = Self::load_jpeg_scaled(path, max_texture_dim, Some(preview_dim)) {
                return Ok(image);
            }
        }
        Self::load(path, max_texture_dim)
    }

    pub fn load(path: &Path, max_texture_dim: u32) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        if ext == "heic" || ext == "heif" {
            return Self::load_wic(path);
        }

        if ext == "jpg" || ext == "jpeg" {
            if let Ok(img) = Self::load_jpeg_scaled(path, max_texture_dim, None) {
                return Ok(img);
            }
        }

        match Self::load_image_crate(path) {
            Ok(img) => Ok(img),
            Err(_) => Self::load_wic(path),
        }
    }

    fn load_jpeg_scaled(
        path: &Path,
        max_dim: u32,
        preview_dim: Option<u32>,
    ) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let jpeg_data =
            std::fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let read_time = started.elapsed();

        let mut decompressor =
            turbojpeg::Decompressor::new().map_err(|e| format!("turbojpeg init: {}", e))?;

        let header = decompressor
            .read_header(&jpeg_data)
            .map_err(|e| format!("JPEG header: {}", e))?;

        let original_size = [header.width, header.height];
        let scaling = if header.is_lossless {
            turbojpeg::ScalingFactor::ONE
        } else {
            jpeg_scaling_factor(original_size, max_dim, preview_dim)
        };
        if scaling != turbojpeg::ScalingFactor::ONE {
            decompressor
                .set_scaling_factor(scaling)
                .map_err(|e| format!("JPEG scaling: {}", e))?;
        }
        let out_w = scaling.scale(header.width);
        let out_h = scaling.scale(header.height);
        let pitch = out_w * 4;
        let mut pixels = vec![0u8; out_h * pitch];

        let image = turbojpeg::Image {
            pixels: pixels.as_mut_slice(),
            width: out_w,
            height: out_h,
            pitch,
            format: turbojpeg::PixelFormat::RGBA,
        };

        decompressor
            .decompress(&jpeg_data, image)
            .map_err(|e| format!("JPEG scaled decode: {}", e))?;
        let decode_time = started.elapsed().saturating_sub(read_time);

        // DCT scaling stops at 1/8; extreme images still need a bounded resize.
        let max_dim = max_dim.max(1);
        let (size, pixels) = if out_w > max_dim as usize || out_h > max_dim as usize {
            let rgba = image::RgbaImage::from_raw(out_w as u32, out_h as u32, pixels)
                .ok_or_else(|| "Invalid JPEG buffer size".to_owned())?;
            let resized = image::DynamicImage::ImageRgba8(rgba)
                .resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
                .into_rgba8();
            (
                [resized.width() as usize, resized.height() as usize],
                resized.into_raw(),
            )
        } else {
            ([out_w, out_h], pixels)
        };
        // JPEG is opaque, so its RGBA output is already premultiplied.
        let color_image = egui::ColorImage::from_rgba_premultiplied(size, &pixels);
        log::debug!(
            "JPEG {:?} -> {:?}, preview={}, read={:?}, decode={:?}, total={:?}",
            original_size,
            size,
            preview_dim.is_some(),
            read_time,
            decode_time,
            started.elapsed()
        );
        Ok(Self {
            pixels: color_image,
            original_size,
        })
    }

    fn load_image_crate(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let reader = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()
            .map_err(|e| format!("Failed to detect format {}: {}", path.display(), e))?;
        let img = reader
            .decode()
            .map_err(|e| format!("Failed to decode {}: {}", path.display(), e))?;
        let rgba = img.into_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Ok(Self {
            pixels,
            original_size: size,
        })
    }

    fn load_wic(path: &Path) -> Result<Self, String> {
        let pixels = crate::wic_decoder::decode_with_wic(path)?;
        Ok(Self {
            original_size: pixels.size,
            pixels,
        })
    }
}

fn jpeg_scaling_factor(
    size: [usize; 2],
    max_dim: u32,
    preview_dim: Option<u32>,
) -> turbojpeg::ScalingFactor {
    let max_dim = max_dim.max(1) as usize;
    let factors = [
        turbojpeg::ScalingFactor::ONE,
        turbojpeg::ScalingFactor::ONE_HALF,
        turbojpeg::ScalingFactor::ONE_QUARTER,
        turbojpeg::ScalingFactor::ONE_EIGHTH,
    ];
    if let Some(preview_dim) = preview_dim {
        let target = (preview_dim.max(1) as usize).min(size[0].max(size[1]));
        if let Some(factor) = factors.iter().rev().copied().find(|factor| {
            factor.scale(size[0].max(size[1])) >= target
                && factor.scale(size[0]) <= max_dim
                && factor.scale(size[1]) <= max_dim
        }) {
            return factor;
        }
    }
    factors
        .into_iter()
        .find(|factor| factor.scale(size[0]) <= max_dim && factor.scale(size[1]) <= max_dim)
        .unwrap_or(turbojpeg::ScalingFactor::ONE_EIGHTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestImage(std::path::PathBuf);

    impl TestImage {
        fn new(extension: &str) -> Self {
            static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "visual-media-viewer-decode-{}-{timestamp}-{id}.{extension}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestImage {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn jpeg_decode_preserves_original_dimensions_and_bounds_preview() {
        let fixture = TestImage::new("jpg");
        let source = image::RgbImage::from_fn(513, 257, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        image::codecs::jpeg::JpegEncoder::new_with_quality(File::create(&fixture.0).unwrap(), 95)
            .encode_image(&source)
            .unwrap();

        let full = DecodedImage::load(&fixture.0, 1024).unwrap();
        assert_eq!(full.original_size, [513, 257]);
        assert_eq!(full.pixels.size, [513, 257]);
        assert!(full.pixels.pixels.iter().all(|pixel| pixel.a() == 255));

        let preview = DecodedImage::load_preview(&fixture.0, 128, 1024).unwrap();
        assert_eq!(preview.original_size, full.original_size);
        assert_eq!(preview.pixels.size, [129, 65]);

        let bounded = DecodedImage::load(&fixture.0, 128).unwrap();
        assert_eq!(bounded.original_size, full.original_size);
        assert_eq!(bounded.pixels.size, [65, 33]);

        // Exercise the resize beyond the smallest available DCT scale.
        let tiny = DecodedImage::load_preview(&fixture.0, 128, 32).unwrap();
        assert_eq!(tiny.original_size, full.original_size);
        assert!(tiny.pixels.size.iter().all(|&dim| dim > 0 && dim <= 32));
    }

    #[test]
    fn png_preview_fallback_preserves_dimensions_and_transparency() {
        let fixture = TestImage::new("png");
        let source = image::RgbaImage::from_pixel(17, 9, image::Rgba([80, 120, 160, 128]));
        source.save(&fixture.0).unwrap();

        let preview = DecodedImage::load_preview(&fixture.0, 4, 1024).unwrap();
        assert_eq!(preview.original_size, [17, 9]);
        assert_eq!(preview.pixels.size, [17, 9]);
        assert!(preview.pixels.pixels.iter().all(|pixel| pixel.a() == 128));
    }

    #[test]
    fn jpeg_full_scale_rounds_odd_dimensions_up() {
        let factor = jpeg_scaling_factor([8193, 4097], 4096, None);
        assert_eq!(factor, turbojpeg::ScalingFactor::ONE_QUARTER);
        assert_eq!(factor.scale(8193), 2049);
        assert_eq!(factor.scale(4097), 1025);
    }

    #[test]
    fn jpeg_preview_covers_display_without_exceeding_gpu_limit() {
        assert_eq!(
            jpeg_scaling_factor([8000, 6000], 8192, Some(1920)),
            turbojpeg::ScalingFactor::ONE_QUARTER
        );
        assert_eq!(
            jpeg_scaling_factor([8000, 6000], 4096, Some(7000)),
            turbojpeg::ScalingFactor::ONE_HALF
        );
        assert_eq!(
            jpeg_scaling_factor([800, 600], 4096, Some(1920)),
            turbojpeg::ScalingFactor::ONE
        );
    }

    #[test]
    fn jpeg_scale_handles_tiny_targets_and_extreme_aspect_ratios() {
        let factor = jpeg_scaling_factor([65535, 1], 1024, None);
        assert_eq!(factor, turbojpeg::ScalingFactor::ONE_EIGHTH);
        assert_eq!(factor.scale(1), 1);
        assert_eq!(
            jpeg_scaling_factor([1, 1], 0, None),
            turbojpeg::ScalingFactor::ONE
        );
    }

    #[test]
    fn texture_downscale_preserves_single_pixel_axis() {
        let pixels = vec![egui::Color32::WHITE; 8];
        let (rgba, width, height) = nearest_half_from_pixels(&pixels, 8, 1);
        assert_eq!((width, height, rgba.len()), (4, 1, 16));
        let (rgba, width, height) = nearest_half_rgba(&rgba, width, height);
        assert_eq!((width, height, rgba.len()), (2, 1, 8));
    }
}

fn compute_mip_levels(width: u32, height: u32) -> u32 {
    (width.max(height) as f32).log2().floor() as u32 + 1
}

const MIPMAP_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index & 2u) * 2 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@group(0) @binding(0) var src_texture: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(src_texture, src_sampler, in.uv);
}
"#;

struct MipmapPipeline {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
}

static MIPMAP_PIPELINE: OnceLock<MipmapPipeline> = OnceLock::new();

fn get_mipmap_pipeline(device: &wgpu::Device) -> &'static MipmapPipeline {
    MIPMAP_PIPELINE.get_or_init(|| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mipmap_shader"),
            source: wgpu::ShaderSource::Wgsl(MIPMAP_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mipmap_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mipmap_pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mipmap_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mipmap_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        MipmapPipeline {
            pipeline,
            sampler,
            bind_group_layout,
        }
    })
}

fn generate_mipmaps_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip_levels: u32,
) {
    let mipmap = get_mipmap_pipeline(device);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("mipmap_encoder"),
    });

    for level in 1..mip_levels {
        let src_view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level - 1,
            mip_level_count: Some(1),
            ..Default::default()
        });

        let dst_view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &mipmap.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&mipmap.sampler),
                },
            ],
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&mipmap.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    queue.submit(std::iter::once(encoder.finish()));
}

fn color_image_to_rgba(img: &egui::ColorImage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(img.pixels.len() * 4);
    for pixel in &img.pixels {
        buf.push(pixel.r());
        buf.push(pixel.g());
        buf.push(pixel.b());
        buf.push(pixel.a());
    }
    buf
}

fn nearest_half_from_pixels(src: &[egui::Color32], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = (width / 2).max(1);
    let new_h = (height / 2).max(1);
    let stride = width as usize;
    let mut out = Vec::with_capacity((new_w as usize) * (new_h as usize) * 4);
    for y in 0..new_h as usize {
        let row = y * 2 * stride;
        for x in 0..new_w as usize {
            let p = src[row + x * 2];
            out.push(p.r());
            out.push(p.g());
            out.push(p.b());
            out.push(p.a());
        }
    }
    (out, new_w, new_h)
}

fn nearest_half_rgba(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = (width / 2).max(1);
    let new_h = (height / 2).max(1);
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity((new_w as usize) * (new_h as usize) * 4);
    for y in 0..new_h as usize {
        let row = y * 2 * stride;
        for x in 0..new_w as usize {
            let idx = row + x * 2 * 4;
            out.push(src[idx]);
            out.push(src[idx + 1]);
            out.push(src[idx + 2]);
            out.push(src[idx + 3]);
        }
    }
    (out, new_w, new_h)
}

pub fn create_mipmapped_texture(
    render_state: &eframe::egui_wgpu::RenderState,
    pixels: &egui::ColorImage,
) -> (egui::TextureId, wgpu::Texture, [usize; 2]) {
    let device = &render_state.device;
    let queue = &render_state.queue;
    let max_dim = device.limits().max_texture_dimension_2d;

    let orig_w = pixels.size[0] as u32;
    let orig_h = pixels.size[1] as u32;

    let (mut data, mut w, mut h) = if orig_w > max_dim || orig_h > max_dim {
        let (d, nw, nh) = nearest_half_from_pixels(&pixels.pixels, orig_w, orig_h);
        (d, nw, nh)
    } else {
        (color_image_to_rgba(pixels), orig_w, orig_h)
    };

    while w > max_dim || h > max_dim {
        let (shrunk, nw, nh) = nearest_half_rgba(&data, w, h);
        data = shrunk;
        w = nw;
        h = nh;
    }

    let actual_size = [w as usize, h as usize];
    let mip_levels = compute_mip_levels(w, h);

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mipmapped_image"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );

    generate_mipmaps_gpu(device, queue, &texture, mip_levels);

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut renderer = render_state.renderer.write();
    let tex_id = renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear);
    (tex_id, texture, actual_size)
}

pub fn paint_textured_rect(
    painter: &egui::Painter,
    tex_id: egui::TextureId,
    rect: egui::Rect,
    uvs: &[egui::Pos2; 4],
) {
    let tint = egui::Color32::WHITE;
    let mut mesh = egui::Mesh::with_texture(tex_id);
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.left_top(),
        uv: uvs[0],
        color: tint,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.right_top(),
        uv: uvs[1],
        color: tint,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.right_bottom(),
        uv: uvs[2],
        color: tint,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.left_bottom(),
        uv: uvs[3],
        color: tint,
    });
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}
