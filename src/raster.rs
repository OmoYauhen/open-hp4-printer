//! Label image -> packed print-head rows.

use image::{imageops, GrayImage, Luma, Rgb, RgbImage, RgbaImage};

use crate::media::{Kind, Media};
use crate::protocol::{DOTS_PER_MM, HEAD_DOTS, ROW_BYTES};

/// The printer's dots sit 4 dots off the head's centre line.
const HEAD_SHIFT: u32 = 4;

/// Flatten transparency onto white.
pub fn flatten(img: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        let mix = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        Rgb([mix(r), mix(g), mix(b)])
    })
}

/// Centre `img` on a white canvas of the printable band (scaling down if too big).
/// Label orientation: x runs along the tape, y across it.
pub fn fit(img: &RgbImage, media: &Media) -> RgbImage {
    let h = media.across_dots();
    let max_len = media.length_dots();
    let mut scale = (h as f32 / img.height() as f32).min(1.0);
    if let Some(l) = max_len {
        scale = scale.min(l as f32 / img.width() as f32);
    }
    let resized;
    let img = if scale < 1.0 {
        let w = ((img.width() as f32 * scale).round() as u32).max(1);
        let hh = ((img.height() as f32 * scale).round() as u32).max(1);
        resized = imageops::resize(img, w, hh, imageops::FilterType::Lanczos3);
        &resized
    } else {
        img
    };
    let w = max_len.unwrap_or(img.width());
    pad(img, w, h)
}

/// Paste `img` centred on a white `w` x `h` canvas.
pub fn pad(img: &RgbImage, w: u32, h: u32) -> RgbImage {
    let mut canvas = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
    let x = (w as i64 - img.width() as i64) / 2;
    let y = (h as i64 - img.height() as i64) / 2;
    imageops::overlay(&mut canvas, img, x, y);
    canvas
}

/// Add blank tape before and after the content so it clears the cutter.
pub fn add_margin(label: &RgbImage, media: &Media, margin_mm: f32, min_length_mm: f32) -> RgbImage {
    if media.kind != Kind::Tape {
        return label.clone();
    }
    let margin = (margin_mm * DOTS_PER_MM) as u32;
    let w = ((min_length_mm * DOTS_PER_MM) as u32).max(label.width() + 2 * margin);
    pad(label, w, label.height())
}

/// Rotate onto the head (one row per dot line across the tape), threshold, pack MSB-first.
pub fn to_rows(label: &RgbImage) -> anyhow::Result<Vec<u8>> {
    let head = imageops::rotate270(label); // 90 degrees counter-clockwise
    anyhow::ensure!(
        head.width() as usize <= HEAD_DOTS,
        "label is {} dots across, the head has {HEAD_DOTS}",
        head.width()
    );
    let x0 = (HEAD_DOTS as u32 - head.width()) / 2 + HEAD_SHIFT;
    let mut rows = vec![0u8; head.height() as usize * ROW_BYTES];
    for (x, y, px) in head.enumerate_pixels() {
        let [r, g, b] = px.0;
        let col = x0 + x;
        if (r as u32 + g as u32 + b as u32) / 3 <= 128 && (col as usize) < HEAD_DOTS {
            rows[y as usize * ROW_BYTES + col as usize / 8] |= 0x80 >> (col % 8);
        }
    }
    Ok(rows)
}

/// Pre-feed and rows to drop for die-cut labels, so the image lands on the label.
pub fn gap_placement(media: &Media, label_len: u32) -> (u16, usize) {
    let v = ((media.length - media.print_x) as f64 * DOTS_PER_MM as f64 - label_len as f64 * 1.008)
        as i64;
    if v > 0 {
        (v as u16, 0)
    } else {
        (0, (-v) as usize)
    }
}

/// Packed rows back to a label-orientation image, for --preview.
pub fn preview(rows: &[u8]) -> GrayImage {
    let n = (rows.len() / ROW_BYTES) as u32;
    let head = GrayImage::from_fn(HEAD_DOTS as u32, n, |x, y| {
        let on = rows[y as usize * ROW_BYTES + x as usize / 8] & (0x80 >> (x % 8)) != 0;
        Luma([if on { 0 } else { 255 }])
    });
    imageops::rotate90(&head)
}
