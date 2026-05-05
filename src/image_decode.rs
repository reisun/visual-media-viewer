use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::OnceLock;

pub struct DecodedImage {
    pub pixels: egui::ColorImage,
}

impl DecodedImage {
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
            if let Ok(img) = Self::load_jpeg_scaled(path, max_texture_dim) {
                return Ok(img);
            }
        }

        match Self::load_image_crate(path) {
            Ok(img) => Ok(img),
            Err(_) => Self::load_wic(path),
        }
    }

    fn load_jpeg_scaled(path: &Path, max_dim: u32) -> Result<Self, String> {
        let jpeg_data = std::fs::read(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        let mut decompressor = turbojpeg::Decompressor::new()
            .map_err(|e| format!("turbojpeg init: {}", e))?;

        let header = decompressor.read_header(&jpeg_data)
            .map_err(|e| format!("JPEG header: {}", e))?;

        let orig_w = header.width;
        let orig_h = header.height;

        if (orig_w as u32) <= max_dim && (orig_h as u32) <= max_dim {
            return Self::load_image_crate(path);
        }

        let out_w = orig_w / 2;
        let out_h = orig_h / 2;
        let pitch = out_w * 4;
        let mut pixels = vec![0u8; out_h * pitch];

        let image = turbojpeg::Image {
            pixels: pixels.as_mut_slice(),
            width: out_w,
            height: out_h,
            pitch,
            format: turbojpeg::PixelFormat::RGBA,
        };

        decompressor.decompress(&jpeg_data, image)
            .map_err(|e| format!("JPEG scaled decode: {}", e))?;

        let size = [out_w, out_h];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
        Ok(Self { pixels: color_image })
    }

    fn load_image_crate(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let reader = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()
            .map_err(|e| format!("Failed to detect format {}: {}", path.display(), e))?;
        let img = reader
            .decode()
            .map_err(|e| format!("Failed to decode {}: {}", path.display(), e))?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Ok(Self { pixels })
    }

    fn load_wic(path: &Path) -> Result<Self, String> {
        let pixels = crate::wic_decoder::decode_with_wic(path)?;
        Ok(Self { pixels })
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

        MipmapPipeline { pipeline, sampler, bind_group_layout }
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

fn box_filter_mip(data: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let new_w = (width / 2).max(1);
    let new_h = (height / 2).max(1);
    let w = width as usize;
    let mut out = vec![0u8; (new_w * new_h * 4) as usize];
    for y in 0..new_h as usize {
        for x in 0..new_w as usize {
            let sx = x * 2;
            let sy = y * 2;
            let mut r = 0u32;
            let mut g = 0u32;
            let mut b = 0u32;
            let mut a = 0u32;
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let px = (sx + dx).min(width as usize - 1);
                    let py = (sy + dy).min(height as usize - 1);
                    let idx = (py * w + px) * 4;
                    r += data[idx] as u32;
                    g += data[idx + 1] as u32;
                    b += data[idx + 2] as u32;
                    a += data[idx + 3] as u32;
                }
            }
            let oi = (y * new_w as usize + x) * 4;
            out[oi] = (r / 4) as u8;
            out[oi + 1] = (g / 4) as u8;
            out[oi + 2] = (b / 4) as u8;
            out[oi + 3] = (a / 4) as u8;
        }
    }
    (out, new_w, new_h)
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
    let new_w = width / 2;
    let new_h = height / 2;
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
    let new_w = width / 2;
    let new_h = height / 2;
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
    mesh.vertices.push(egui::epaint::Vertex { pos: rect.left_top(), uv: uvs[0], color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: rect.right_top(), uv: uvs[1], color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: rect.right_bottom(), uv: uvs[2], color: tint });
    mesh.vertices.push(egui::epaint::Vertex { pos: rect.left_bottom(), uv: uvs[3], color: tint });
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}
