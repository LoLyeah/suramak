//! Deterministic host rasteriser for the phone-viewport screenshot suite.
//!
//! The desktop's real frame is drawn by wgpu under Xvfb; the phone suite needs
//! the *same egui shape stream* without a GPU, because CI must compare layout
//! on a runner whose llvmpipe output is not stable. This module runs one egui
//! frame, tessellates it exactly as the renderer would, and rasterises the
//! triangles into an sRGB image. Text comes from egui's font atlas, so the
//! glyphs — and therefore every wrapping and clipping decision — are the real
//! ones.
//!
//! It is a **test seam**, not a shipping renderer: no wgpu, no window, no
//! Android surface. `tests/ui-compact` drives it under Xvfb (the suite command
//! is `xvfb-run cargo test …`) so the harness lives in the same harness family
//! as the desktop smoke even though this path needs no display.

use std::collections::HashMap;
use std::path::Path;

use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive, Vertex};
use egui::{Color32, Context, Pos2, RawInput, Rect, TextureId, ViewportId, ViewportInfo};

/// One rendered frame.
#[derive(Debug, Clone)]
pub struct Shot {
    pub width: usize,
    pub height: usize,
    /// Row-major sRGB pixels.
    pub pixels: Vec<Color32>,
}

impl Shot {
    pub fn pixel(&self, x: usize, y: usize) -> Color32 {
        self.pixels[y * self.width + x]
    }

    /// Fraction of pixels that are not the background (a trivial "something
    /// was painted" gate for a blank frame).
    pub fn painted_fraction(&self, background: Color32) -> f32 {
        let painted = self
            .pixels
            .iter()
            .filter(|pixel| **pixel != background)
            .count();
        painted as f32 / self.pixels.len() as f32
    }
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

impl Texture {
    fn sample(&self, uv: Pos2) -> Color32 {
        // M20B: the material grain tiles with `TextureWrapMode::Repeat`, and
        // the CPU rasteriser has no sampler state — wrap the UV here (font
        // atlas UVs stay in 0..=1 and are untouched).
        let wrap = |value: f32| {
            if (0.0..=1.0).contains(&value) {
                value
            } else {
                value.rem_euclid(1.0)
            }
        };
        let uv = Pos2::new(wrap(uv.x), wrap(uv.y));
        let x = (uv.x * self.width as f32 - 0.5).clamp(0.0, self.width as f32 - 1.0);
        let y = (uv.y * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let p00 = self.pixels[y0 * self.width + x0];
        let p10 = self.pixels[y0 * self.width + x1];
        let p01 = self.pixels[y1 * self.width + x0];
        let p11 = self.pixels[y1 * self.width + x1];
        let lerp = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color32::from_rgba_premultiplied(
            lerp(lerp(p00.r(), p10.r(), fx), lerp(p01.r(), p11.r(), fx), fy),
            lerp(lerp(p00.g(), p10.g(), fx), lerp(p01.g(), p11.g(), fx), fy),
            lerp(lerp(p00.b(), p10.b(), fx), lerp(p01.b(), p11.b(), fx), fy),
            lerp(lerp(p00.a(), p10.a(), fx), lerp(p01.a(), p11.a(), fx), fy),
        )
    }
}

/// Renders one frame of `draw` at `screen` (points) and `scale`.
pub fn render(ctx: &Context, screen: Rect, scale: f32, mut draw: impl FnMut(&Context)) -> Shot {
    let mut raw = RawInput {
        viewport_id: ViewportId::ROOT,
        screen_rect: Some(screen),
        time: Some(0.0),
        ..Default::default()
    };
    raw.viewports.insert(
        ViewportId::ROOT,
        ViewportInfo {
            native_pixels_per_point: Some(scale),
            ..Default::default()
        },
    );
    let output = ctx.run(raw, |ctx| draw(ctx));
    let width = (screen.width() * scale).round() as usize;
    let height = (screen.height() * scale).round() as usize;
    let mut textures: HashMap<TextureId, Texture> = HashMap::new();
    for (id, delta) in &output.textures_delta.set {
        apply_delta(&mut textures, *id, delta);
    }
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);

    let background = Color32::BLACK;
    let mut pixels = vec![background; width * height];
    let white = Texture {
        width: 1,
        height: 1,
        pixels: vec![Color32::WHITE],
    };
    for ClippedPrimitive {
        clip_rect,
        primitive,
    } in primitives
    {
        let Primitive::Mesh(mesh) = primitive else {
            continue;
        };
        let texture = textures.get(&mesh.texture_id).unwrap_or(&white);
        for triangle in mesh.indices.chunks_exact(3) {
            let vertices = [
                mesh.vertices[triangle[0] as usize],
                mesh.vertices[triangle[1] as usize],
                mesh.vertices[triangle[2] as usize],
            ];
            raster_triangle(
                &mut pixels,
                width,
                height,
                scale * clip_rect,
                &vertices,
                texture,
                scale,
            );
        }
    }
    Shot {
        width,
        height,
        pixels,
    }
}

fn apply_delta(textures: &mut HashMap<TextureId, Texture>, id: TextureId, delta: &ImageDelta) {
    // egui 0.33 carries the font atlas as an ordinary premultiplied colour
    // image (white with coverage alpha), so one branch covers both textures.
    let egui::ImageData::Color(image) = &delta.image;
    let (width, height, pixels) = (image.size[0], image.size[1], image.pixels.clone());
    match delta.pos {
        None => {
            textures.insert(
                id,
                Texture {
                    width,
                    height,
                    pixels,
                },
            );
        }
        Some([x0, y0]) => {
            if let Some(texture) = textures.get_mut(&id) {
                for y in 0..height {
                    for x in 0..width {
                        let dst_x = x0 + x;
                        let dst_y = y0 + y;
                        if dst_x < texture.width && dst_y < texture.height {
                            texture.pixels[dst_y * texture.width + dst_x] = pixels[y * width + x];
                        }
                    }
                }
            }
        }
    }
}

fn raster_triangle(
    pixels: &mut [Color32],
    width: usize,
    height: usize,
    clip: Rect,
    vertices: &[Vertex; 3],
    texture: &Texture,
    scale: f32,
) {
    // egui's shape stream is in points; a renderer scales into pixels. Do the
    // same here so a shot at 2.6x is the same layout the wgpu surface shows.
    let p = vertices.map(|vertex| vertex.pos * scale);
    let min_x = p
        .iter()
        .map(|pos| pos.x)
        .fold(f32::INFINITY, f32::min)
        .max(clip.left().max(0.0));
    let max_x = p
        .iter()
        .map(|pos| pos.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.right().min(width as f32 - 1.0));
    let min_y = p
        .iter()
        .map(|pos| pos.y)
        .fold(f32::INFINITY, f32::min)
        .max(clip.top().max(0.0));
    let max_y = p
        .iter()
        .map(|pos| pos.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .min(clip.bottom().min(height as f32 - 1.0));
    if !(min_x <= max_x && min_y <= max_y) {
        return;
    }
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let start_x = min_x.floor() as usize;
    let end_x = (max_x.ceil() as usize).min(width - 1);
    let start_y = min_y.floor() as usize;
    let end_y = (max_y.ceil() as usize).min(height - 1);

    for y in start_y..=end_y {
        for x in start_x..=end_x {
            let point = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p[1], p[2], point) / area;
            let w1 = edge(p[2], p[0], point) / area;
            let w2 = edge(p[0], p[1], point) / area;
            if w0 < -0.0001 || w1 < -0.0001 || w2 < -0.0001 {
                continue;
            }
            let uv = Pos2::new(
                w0 * vertices[0].uv.x + w1 * vertices[1].uv.x + w2 * vertices[2].uv.x,
                w0 * vertices[0].uv.y + w1 * vertices[1].uv.y + w2 * vertices[2].uv.y,
            );
            let texel = texture.sample(uv);
            let mut color = [0.0_f32; 4];
            for (channel, out) in color.iter_mut().enumerate() {
                let vertex_channel = [vertices[0].color, vertices[1].color, vertices[2].color];
                let value = w0 * f32::from(vertex_channel[0][channel])
                    + w1 * f32::from(vertex_channel[1][channel])
                    + w2 * f32::from(vertex_channel[2][channel]);
                *out = value;
            }
            let texel = [
                texel.r() as f32,
                texel.g() as f32,
                texel.b() as f32,
                texel.a() as f32,
            ];
            let src = [
                color[0] * texel[0] / 255.0,
                color[1] * texel[1] / 255.0,
                color[2] * texel[2] / 255.0,
                color[3] * texel[3] / 255.0,
            ];
            let alpha = (src[3] / 255.0).clamp(0.0, 1.0);
            let dst = pixels[y * width + x];
            let blend =
                |s: f32, d: u8| (s + d as f32 * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
            pixels[y * width + x] = Color32::from_rgba_premultiplied(
                blend(src[0], dst.r()),
                blend(src[1], dst.g()),
                blend(src[2], dst.b()),
                255,
            );
        }
    }
}

fn edge(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

/// Writes a shot as a PNG (8-bit sRGB, no compression — a small, dependency-free
/// encoder so the suite adds no crate to the workspace).
pub fn write_png(path: &Path, shot: &Shot) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut raw = Vec::with_capacity((shot.width * 4 + 1) * shot.height);
    for y in 0..shot.height {
        raw.push(0); // filter: none
        for x in 0..shot.width {
            let pixel = shot.pixel(x, y);
            raw.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), 255]);
        }
    }
    let mut zlib = Vec::new();
    zlib.extend_from_slice(&[0x78, 0x01]);
    for (index, chunk) in raw.chunks(65_535).enumerate() {
        let last = (index + 1) * 65_535 >= raw.len();
        zlib.push(if last { 1 } else { 0 });
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(shot.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(shot.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_png_encoder_writes_a_valid_signature_and_chunks() {
        let shot = Shot {
            width: 2,
            height: 2,
            pixels: vec![Color32::RED; 4],
        };
        let dir = std::env::temp_dir().join("suramak-snapshot-test");
        let path = dir.join("shot.png");
        write_png(&path, &shot).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        assert_eq!(&bytes[12..16], b"IHDR");
        assert!(bytes.windows(4).any(|window| window == b"IDAT"));
        assert!(bytes.windows(4).any(|window| window == b"IEND"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_frame_has_no_painted_pixels() {
        let ctx = Context::default();
        let shot = render(
            &ctx,
            Rect::from_min_size(Pos2::ZERO, egui::vec2(40.0, 40.0)),
            1.0,
            |_| {},
        );
        assert_eq!(shot.width, 40);
        assert!(shot.painted_fraction(Color32::BLACK) < 0.001);
    }

    #[test]
    fn a_filled_rect_is_actually_painted() {
        let ctx = Context::default();
        let shot = render(
            &ctx,
            Rect::from_min_size(Pos2::ZERO, egui::vec2(40.0, 40.0)),
            1.0,
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(Color32::from_rgb(200, 10, 10)))
                    .show(ctx, |_| {});
            },
        );
        assert!(shot.painted_fraction(Color32::BLACK) > 0.9);
        assert_eq!(shot.pixel(20, 20).r(), 200);
    }
}
