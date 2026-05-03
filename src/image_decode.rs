use eframe::egui;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub struct DecodedImage {
    pub pixels: egui::ColorImage,
}

impl DecodedImage {
    pub fn load(path: &Path) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        if ext == "heic" || ext == "heif" {
            return Self::load_wic(path);
        }

        match Self::load_image_crate(path) {
            Ok(img) => Ok(img),
            Err(_) => Self::load_wic(path),
        }
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
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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

    let mut current_data = data;
    let mut current_w = w;
    let mut current_h = h;
    for level in 1..mip_levels {
        let (mip_data, mip_w, mip_h) = box_filter_mip(&current_data, current_w, current_h);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &mip_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip_w * 4),
                rows_per_image: Some(mip_h),
            },
            wgpu::Extent3d {
                width: mip_w,
                height: mip_h,
                depth_or_array_layers: 1,
            },
        );
        current_data = mip_data;
        current_w = mip_w;
        current_h = mip_h;
    }

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
