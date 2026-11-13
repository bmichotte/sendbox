//! Draws the Unread count onto the tray icon at runtime, for Windows, which
//! has no text next to tray icons. Plain RGBA pixels, no image library: a
//! red pill in the bottom-right corner with the count in white 3x5 digits.

use tauri::image::Image;

const RED: [u8; 4] = [0xE5, 0x48, 0x4D, 0xFF];
const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// `icon` with `count` drawn on it; counts above 99 show as "99+", which
/// is all a 16 px icon has room for.
pub fn with_count(icon: &Image<'_>, count: usize) -> Image<'static> {
    let text = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    with_badge(icon, &text)
}

fn with_badge(icon: &Image<'_>, text: &str) -> Image<'static> {
    let (width, height) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    let mut put = |x: u32, y: u32, pixel: [u8; 4]| {
        if x < width && y < height {
            let at = ((y * width + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&pixel);
        }
    };

    // One font pixel per 16 icon pixels: 1 at 16x16, 2 at 32x32.
    let scale = (width.min(height) / 16).max(1);
    let (glyph_width, glyph_height, gap, padding) = (3 * scale, 5 * scale, scale, scale);
    let chars = text.chars().count() as u32;
    let text_width = chars * glyph_width + chars.saturating_sub(1) * gap;
    let badge_height = glyph_height + 2 * padding;
    // Twice the padding at the ends, where the pill rounds off.
    let badge_width = (text_width + 4 * padding).max(badge_height).min(width);
    let (left, top) = (width - badge_width, height - badge_height);

    // A pill: the rectangle with fully rounded ends.
    let radius = badge_height as f32 / 2.0;
    for y in 0..badge_height {
        for x in 0..badge_width {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let nearest_x = cx.clamp(radius, badge_width as f32 - radius);
            if (cx - nearest_x).powi(2) + (cy - radius).powi(2) <= radius * radius {
                put(left + x, top + y, RED);
            }
        }
    }

    let text_left = left + badge_width.saturating_sub(text_width) / 2;
    for (index, c) in text.chars().enumerate() {
        let glyph_left = text_left + index as u32 * (glyph_width + gap);
        for (row, bits) in (0..).zip(glyph(c)) {
            for column in 0..3 {
                if (bits >> (2 - column)) & 1 == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        put(
                            glyph_left + column * scale + dx,
                            top + padding + row * scale + dy,
                            WHITE,
                        );
                    }
                }
            }
        }
    }
    Image::new_owned(rgba, width, height)
}

/// A 3x5 pixel glyph: one byte per row, its 3 low bits the columns, the
/// highest one on the left.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b011, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}
