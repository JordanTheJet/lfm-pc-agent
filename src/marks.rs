//! Optional "Set-of-Marks" overlay: draw the numbered accessibility boxes onto the
//! screenshot so the model can visually correlate ids with on-screen elements. This is
//! the classic technique (Yang et al. 2023; Microsoft OmniParser) for letting a weak
//! VLM ground reliably by *picking a number* instead of regressing pixels.

use crate::perceive::ObservedElement;
use anyhow::{anyhow, Result};
use image::{Rgba, RgbaImage};

/// Annotate `in_png` (a capture of the window at `window` point-bounds) with numbered
/// boxes for each element, writing the result to `out_png`.
pub fn annotate(
    in_png: &str,
    els: &[ObservedElement],
    window: Option<(f64, f64, f64, f64)>,
    out_png: &str,
) -> Result<()> {
    let (wx, wy, ww, wh) = window.ok_or_else(|| anyhow!("set-of-marks needs window bounds"))?;
    if ww <= 0.0 || wh <= 0.0 {
        return Err(anyhow!("invalid window bounds"));
    }
    let mut img = image::open(in_png)?.to_rgba8();
    let (pw, ph) = img.dimensions();
    // Screenshot pixels are point-bounds × backing scale (2× on Retina); recover scale.
    let sx = pw as f64 / ww;
    let sy = ph as f64 / wh;

    let box_color = Rgba([255, 40, 40, 255]);
    for el in els {
        let rx = ((el.x - wx) * sx).round() as i64;
        let ry = ((el.y - wy) * sy).round() as i64;
        let rw = (el.w * sx).round() as i64;
        let rh = (el.h * sy).round() as i64;
        draw_rect(&mut img, rx, ry, rw, rh, box_color, 2);
        draw_number(&mut img, el.id, rx + 1, ry + 1);
    }
    img.save(out_png)?;
    Ok(())
}

fn put(img: &mut RgbaImage, x: i64, y: i64, c: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn draw_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, c: Rgba<u8>, t: i64) {
    if w <= 0 || h <= 0 {
        return;
    }
    for i in 0..t {
        for dx in 0..w {
            put(img, x + dx, y + i, c);
            put(img, x + dx, y + h - 1 - i, c);
        }
        for dy in 0..h {
            put(img, x + i, y + dy, c);
            put(img, x + w - 1 - i, y + dy, c);
        }
    }
}

/// 3×5 bitmap glyphs for digits 0-9 (each row uses the low 3 bits, MSB = left column).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

fn draw_number(img: &mut RgbaImage, n: usize, x: i64, y: i64) {
    let scale: i64 = 3;
    let digits = n.to_string();
    let glyph_w = 3 * scale + 1;
    let bg_w = glyph_w * digits.len() as i64 + 2;
    let bg_h = 5 * scale + 2;
    // Dark backing rectangle for contrast.
    for dy in 0..bg_h {
        for dx in 0..bg_w {
            put(img, x + dx, y + dy, Rgba([0, 0, 0, 220]));
        }
    }
    let fg = Rgba([255, 235, 40, 255]);
    let mut cx = x + 1;
    for ch in digits.chars() {
        let d = ch.to_digit(10).unwrap_or(0) as usize;
        let glyph = DIGITS[d];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(
                                img,
                                cx + col * scale + sx,
                                y + 1 + row as i64 * scale + sy,
                                fg,
                            );
                        }
                    }
                }
            }
        }
        cx += glyph_w;
    }
}
