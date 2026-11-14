//! Draws badges onto the tray icon at runtime: the error badge while a port
//! is in use, and, for Windows, which has no text next to tray icons, the
//! Unread count. Plain RGBA pixels, no image library.

use tauri::image::Image;

/// A fully transparent pixel.
pub const CLEAR: [u8; 4] = [0; 4];
const RED: [u8; 4] = [0xE5, 0x48, 0x4D, 0xFF];
const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// `icon` with a round badge in its bottom-right corner, `fill` with a `mark`
/// coloured "!", set apart from the icon by a transparent ring.
pub fn with_error_badge(icon: &Image<'_>, fill: [u8; 4], mark: [u8; 4]) -> Image<'static> {
    let mut canvas = Canvas::of(icon);
    let size = canvas.width.min(canvas.height) as f32;
    let radius = size * 0.26;
    let ring = size * 0.06;
    let center_x = canvas.width as f32 - radius;
    let center_y = canvas.height as f32 - radius;
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let (dx, dy) = (x as f32 + 0.5 - center_x, y as f32 + 0.5 - center_y);
            let distance = dx.hypot(dy);
            if distance <= radius {
                let on_mark = on_exclamation_mark(dx, dy, radius);
                canvas.put(x, y, if on_mark { mark } else { fill });
            } else if distance <= radius + ring {
                canvas.put(x, y, CLEAR);
            }
        }
    }
    canvas.into_image()
}

/// Whether the point `(dx, dy)` from the centre of a badge of `radius` is on
/// its "!": a bar above a dot.
fn on_exclamation_mark(dx: f32, dy: f32, radius: f32) -> bool {
    let half_width = radius * 0.17;
    let bar = -radius * 0.62..=radius * 0.16;
    let dot = radius * 0.34..=radius * 0.34 + 2.0 * half_width;
    dx.abs() <= half_width && (bar.contains(&dy) || dot.contains(&dy))
}

/// `icon` with `count` drawn on it in a red pill in the bottom-right corner,
/// in white 3x5 digits; counts above 99 show as "99+", which is all a 16 px
/// icon has room for.
#[cfg_attr(
    target_os = "macos",
    allow(dead_code, reason = "macOS shows the count as text")
)]
pub fn with_count(icon: &Image<'_>, count: usize) -> Image<'static> {
    let text = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    let mut canvas = Canvas::of(icon);
    let (width, height) = (canvas.width, canvas.height);

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
                canvas.put(left + x, top + y, RED);
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
                        canvas.put(
                            glyph_left + column * scale + dx,
                            top + padding + row * scale + dy,
                            WHITE,
                        );
                    }
                }
            }
        }
    }
    canvas.into_image()
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

/// A copy of an icon's pixels to draw on.
struct Canvas {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Canvas {
    fn of(icon: &Image<'_>) -> Self {
        Self {
            rgba: icon.rgba().to_vec(),
            width: icon.width(),
            height: icon.height(),
        }
    }

    /// Sets the pixel at `(x, y)`; one outside the icon is left out.
    fn put(&mut self, x: u32, y: u32, pixel: [u8; 4]) {
        if x < self.width && y < self.height {
            let at = ((y * self.width + x) * 4) as usize;
            self.rgba[at..at + 4].copy_from_slice(&pixel);
        }
    }

    fn into_image(self) -> Image<'static> {
        Image::new_owned(self.rgba, self.width, self.height)
    }
}
