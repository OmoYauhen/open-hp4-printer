//! Text -> label image (text runs along the tape), with a built-in font.

use ab_glyph::{point, Font, FontArc, Glyph, PxScale, ScaleFont};
use anyhow::Context;
use image::{Rgb, RgbImage};

static DEFAULT_FONT: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

pub fn load_font(path: Option<&str>) -> anyhow::Result<FontArc> {
    match path {
        Some(p) => {
            let data = std::fs::read(p).with_context(|| format!("reading font {p}"))?;
            FontArc::try_from_vec(data).with_context(|| format!("{p} is not a usable TTF/OTF font"))
        }
        None => Ok(FontArc::try_from_slice(DEFAULT_FONT)?),
    }
}

struct Layout {
    glyphs: Vec<Glyph>,
    /// ink bounds: min x, min y, max x, max y
    bounds: (f32, f32, f32, f32),
}

/// Lay out centred lines at `size` px and measure the ink.
fn layout(font: &FontArc, lines: &[&str], size: f32) -> Layout {
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    let line_advance = sf.height() + sf.line_gap() + (size / 8.0).floor();
    let widths: Vec<f32> = lines.iter().map(|l| line_width(font, l, scale)).collect();
    let max_w = widths.iter().cloned().fold(0.0, f32::max);
    let mut glyphs = Vec::new();
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (i, line) in lines.iter().enumerate() {
        let mut x = (max_w - widths[i]) / 2.0;
        let y = sf.ascent() + i as f32 * line_advance;
        let mut prev = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, point(x, y));
            x += sf.h_advance(id);
            prev = Some(id);
            if let Some(o) = font.outline_glyph(g.clone()) {
                let r = o.px_bounds();
                b = (
                    b.0.min(r.min.x),
                    b.1.min(r.min.y),
                    b.2.max(r.max.x),
                    b.3.max(r.max.y),
                );
            }
            glyphs.push(g);
        }
    }
    if b.0 > b.2 {
        b = (0.0, 0.0, 0.0, 0.0); // only spaces
    }
    Layout { glyphs, bounds: b }
}

fn line_width(font: &FontArc, line: &str, scale: PxScale) -> f32 {
    let sf = font.as_scaled(scale);
    let mut w = 0.0;
    let mut prev = None;
    for c in line.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Render text so its ink fills `height` dots (or uses `size` px), with `padding` dots on both ends.
pub fn text_image(
    font: &FontArc,
    text: &str,
    height: u32,
    size: Option<f32>,
    padding: u32,
) -> RgbImage {
    let lines: Vec<&str> = text.split('\n').collect();
    let size = size.unwrap_or_else(|| {
        // largest size whose ink still fits the band
        let (mut lo, mut hi) = (4u32, height * 2);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let b = layout(font, &lines, mid as f32).bounds;
            if (b.3 - b.1).ceil() <= height as f32 {
                lo = mid
            } else {
                hi = mid - 1
            }
        }
        lo as f32
    });
    let l = layout(font, &lines, size);
    let (x0, y0, x1, y1) = l.bounds;
    let w = (x1 - x0).ceil() as u32 + 2 * padding;
    let dx = padding as f32 - x0.floor();
    let dy = ((height as f32 - (y1 - y0).ceil()) / 2.0).floor() - y0.floor();
    let mut img = RgbImage::from_pixel(w.max(1), height, Rgb([255, 255, 255]));
    for g in l.glyphs {
        let Some(o) = font.outline_glyph(g) else {
            continue;
        };
        let r = o.px_bounds();
        o.draw(|gx, gy, cov| {
            let x = (r.min.x + dx) as i64 + gx as i64;
            let y = (r.min.y + dy) as i64 + gy as i64;
            if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                let p = img.get_pixel_mut(x as u32, y as u32);
                let v = (255.0 * (1.0 - cov.clamp(0.0, 1.0))) as u8;
                p.0 = [p.0[0].min(v); 3];
            }
        });
    }
    img
}
