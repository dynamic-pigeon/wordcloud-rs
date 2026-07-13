use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use fontdue::Font;

use crate::bitmap::AlphaBitmap;

pub(crate) fn supports_word(font: &Font, word: &str) -> bool {
    let mut visible = false;
    for character in word.chars().filter(|character| !character.is_whitespace()) {
        visible = true;
        if font.lookup_glyph_index(character) == 0 {
            return false;
        }
    }
    visible
}

pub(crate) fn rasterize_word(
    font: &Font,
    word: &str,
    font_size: f32,
    canvas_width: u32,
    canvas_height: u32,
) -> Option<AlphaBitmap> {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font], &TextStyle::new(word, font_size, 0));

    let mut min_x = i64::MAX;
    let mut min_y = i64::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;
    let mut has_visible_glyph = false;
    for glyph in layout
        .glyphs()
        .iter()
        .filter(|glyph| glyph.width != 0 && glyph.height != 0)
    {
        if !glyph.x.is_finite() || !glyph.y.is_finite() {
            return None;
        }
        has_visible_glyph = true;
        let x = glyph.x.floor() as i64;
        let y = glyph.y.floor() as i64;
        let right = x.checked_add(i64::try_from(glyph.width).ok()?)?;
        let bottom = y.checked_add(i64::try_from(glyph.height).ok()?)?;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(right);
        max_y = max_y.max(bottom);
    }
    if !has_visible_glyph {
        return None;
    }

    let width = u32::try_from(max_x.checked_sub(min_x)?).ok()?;
    let height = u32::try_from(max_y.checked_sub(min_y)?).ok()?;
    let horizontal_fits = width <= canvas_width && height <= canvas_height;
    let vertical_fits = height <= canvas_width && width <= canvas_height;
    if width == 0 || height == 0 || (!horizontal_fits && !vertical_fits) {
        return None;
    }
    let mut alpha = vec![0_u8; (width as usize).checked_mul(height as usize)?];

    for glyph in layout
        .glyphs()
        .iter()
        .filter(|glyph| glyph.width != 0 && glyph.height != 0)
    {
        let (_, glyph_alpha) = font.rasterize_config(glyph.key);
        let glyph_x = (glyph.x.floor() as i64).checked_sub(min_x)?;
        let glyph_y = (glyph.y.floor() as i64).checked_sub(min_y)?;
        for y in 0..glyph.height {
            for x in 0..glyph.width {
                let source = y * glyph.width + x;
                let target_x = usize::try_from(glyph_x).ok()?.checked_add(x)?;
                let target_y = usize::try_from(glyph_y).ok()?.checked_add(y)?;
                if target_x >= width as usize || target_y >= height as usize {
                    return None;
                }
                let target = target_y * width as usize + target_x;
                alpha[target] = alpha[target].max(glyph_alpha[source]);
            }
        }
    }

    Some(AlphaBitmap {
        width,
        height,
        alpha,
    })
}
