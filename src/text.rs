use std::collections::HashMap;

use fontdue::layout::{CoordinateSystem, GlyphRasterConfig, Layout, LayoutSettings, TextStyle};
use fontdue::Font;

use crate::bitmap::{AlphaBitmap, BitGrid};

const DEFAULT_GLYPH_CACHE_CAPACITY_BYTES: usize = 256 * 1024;
const DEFAULT_GLYPH_CACHE_MAX_ENTRIES: usize = 4_096;

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

pub(crate) struct TextRasterizer {
    layout: Layout,
    glyph_cache: GlyphRasterCache,
}

/// The bounding box of a laid-out word, in pixels, relative to the top-left
/// corner of the box itself.
struct WordBounds {
    min_x: i64,
    min_y: i64,
    width: u32,
    height: u32,
}

impl TextRasterizer {
    pub(crate) fn new() -> Self {
        Self::with_glyph_cache_capacity(DEFAULT_GLYPH_CACHE_CAPACITY_BYTES)
    }

    fn with_glyph_cache_capacity(capacity_bytes: usize) -> Self {
        Self {
            layout: Layout::new(CoordinateSystem::PositiveYDown),
            glyph_cache: GlyphRasterCache::new(capacity_bytes),
        }
    }

    /// Lays out `word` and returns its pixel bounding box when the word is
    /// visible and fits the canvas in at least one orientation. This only
    /// reads glyph metrics; it never rasterizes.
    fn layout_bounds(
        &mut self,
        font: &Font,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<WordBounds> {
        self.layout.reset(&LayoutSettings::default());
        self.layout
            .append(&[font], &TextStyle::new(word, font_size, 0));

        let mut min_x = i64::MAX;
        let mut min_y = i64::MAX;
        let mut max_x = i64::MIN;
        let mut max_y = i64::MIN;
        let mut has_visible_glyph = false;
        for glyph in self
            .layout
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

        Some(WordBounds {
            min_x,
            min_y,
            width,
            height,
        })
    }

    /// Calls `visit` with the rasterized coverage and pixel offset of every
    /// visible glyph of the most recently laid-out word.
    fn for_each_glyph<R>(
        &mut self,
        font: &Font,
        bounds: &WordBounds,
        mut visit: impl FnMut(usize, usize, usize, &[u8]) -> Option<R>,
    ) -> Option<()> {
        for glyph in self
            .layout
            .glyphs()
            .iter()
            .filter(|glyph| glyph.width != 0 && glyph.height != 0)
        {
            let glyph_x =
                usize::try_from((glyph.x.floor() as i64).checked_sub(bounds.min_x)?).ok()?;
            let glyph_y =
                usize::try_from((glyph.y.floor() as i64).checked_sub(bounds.min_y)?).ok()?;
            let right = glyph_x.checked_add(glyph.width)?;
            let bottom = glyph_y.checked_add(glyph.height)?;
            let source_len = glyph.width.checked_mul(glyph.height)?;
            if right > bounds.width as usize || bottom > bounds.height as usize {
                return None;
            }

            self.glyph_cache
                .with_rasterized(font, glyph.key, |glyph_alpha| {
                    if glyph_alpha.len() != source_len {
                        return None;
                    }
                    visit(glyph_x, glyph_y, glyph.width, glyph_alpha)
                })?;
        }
        Some(())
    }

    /// Returns the ink mask of `word` without materializing its coverage
    /// bitmap. Bits are set exactly where [`TextRasterizer::rasterize_word`]
    /// would produce non-zero coverage.
    pub(crate) fn word_ink(
        &mut self,
        font: &Font,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<BitGrid> {
        let bounds = self.layout_bounds(font, word, font_size, canvas_width, canvas_height)?;
        let mut ink = BitGrid::new(bounds.width, bounds.height);
        self.for_each_glyph(font, &bounds, |glyph_x, glyph_y, width, glyph_alpha| {
            for (row, source_row) in glyph_alpha.chunks_exact(width).enumerate() {
                let y = (glyph_y + row) as u32;
                for (column, coverage) in source_row.iter().enumerate() {
                    if *coverage != 0 {
                        ink.set((glyph_x + column) as u32, y);
                    }
                }
            }
            Some(())
        })?;
        Some(ink)
    }

    pub(crate) fn rasterize_word(
        &mut self,
        font: &Font,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<AlphaBitmap> {
        let bounds = self.layout_bounds(font, word, font_size, canvas_width, canvas_height)?;
        let width_usize = usize::try_from(bounds.width).ok()?;
        let mut alpha = vec![0_u8; width_usize.checked_mul(bounds.height as usize)?];

        self.for_each_glyph(font, &bounds, |glyph_x, glyph_y, width, glyph_alpha| {
            let right = glyph_x + width;
            let source_rows = glyph_alpha.chunks_exact(width);
            let target_rows = alpha.chunks_exact_mut(width_usize).skip(glyph_y);
            for (source_row, target_row) in source_rows.zip(target_rows) {
                for (target, source) in target_row[glyph_x..right].iter_mut().zip(source_row) {
                    *target = (*target).max(*source);
                }
            }
            Some(())
        })?;

        Some(AlphaBitmap {
            width: bounds.width,
            height: bounds.height,
            alpha,
        })
    }
}

impl Default for TextRasterizer {
    fn default() -> Self {
        Self::new()
    }
}

struct GlyphRasterCache {
    entries: HashMap<GlyphRasterConfig, Vec<u8>>,
    capacity_bytes: usize,
    max_entries: usize,
    used_bytes: usize,
}

impl GlyphRasterCache {
    fn new(capacity_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity_bytes,
            max_entries: DEFAULT_GLYPH_CACHE_MAX_ENTRIES,
            used_bytes: 0,
        }
    }

    fn with_rasterized<R>(
        &mut self,
        font: &Font,
        key: GlyphRasterConfig,
        use_alpha: impl FnOnce(&[u8]) -> R,
    ) -> R {
        if let Some(alpha) = self.entries.get(&key) {
            return use_alpha(alpha);
        }

        let (_, alpha) = font.rasterize_config(key);
        let allocation_bytes = alpha.capacity();
        if allocation_bytes == 0 || allocation_bytes > self.capacity_bytes {
            return use_alpha(&alpha);
        }

        if self.entries.len() >= self.max_entries
            || self.used_bytes > self.capacity_bytes - allocation_bytes
        {
            self.entries.clear();
            self.used_bytes = 0;
        }
        self.used_bytes += allocation_bytes;
        self.entries.insert(key, alpha);
        debug_assert!(self.used_bytes <= self.capacity_bytes);
        use_alpha(
            self.entries
                .get(&key)
                .expect("inserted glyph must be cached"),
        )
    }
}

#[cfg(test)]
mod tests {
    use fontdue::{Font, FontSettings};

    use super::*;

    fn test_font() -> Font {
        Font::from_bytes(
            include_bytes!("../assets/OpenSans-Regular.ttf") as &[u8],
            FontSettings::default(),
        )
        .unwrap()
    }

    fn reference_rasterize_word(
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

    #[test]
    fn word_ink_matches_rasterized_coverage() {
        let font = test_font();
        let mut rasterizer = TextRasterizer::new();

        for (word, font_size) in [("Rust", 61.0), ("glyph", 27.0), ("affinity", 43.0)] {
            let ink = rasterizer
                .word_ink(&font, word, font_size, 500, 240)
                .unwrap();
            let bitmap = rasterizer
                .rasterize_word(&font, word, font_size, 500, 240)
                .unwrap();
            let expected = bitmap.bits();

            assert_eq!((ink.width(), ink.height()), (bitmap.width, bitmap.height));
            for y in 0..ink.height() {
                for x in 0..ink.width() {
                    assert_eq!(ink.get(x, y), expected.get(x, y));
                }
            }
        }
    }

    #[test]
    fn reused_workspace_matches_previous_rasterization() {
        let font = test_font();
        let mut rasterizer = TextRasterizer::with_glyph_cache_capacity(512);

        for (word, font_size) in [
            ("Rust", 61.0),
            ("glyph", 27.0),
            ("affinity", 43.0),
            ("Rust", 61.0),
            ("gj", 18.0),
        ] {
            let expected = reference_rasterize_word(&font, word, font_size, 500, 240).unwrap();
            let actual = rasterizer
                .rasterize_word(&font, word, font_size, 500, 240)
                .unwrap();
            assert_eq!(
                (actual.width, actual.height),
                (expected.width, expected.height)
            );
            assert_eq!(actual.alpha, expected.alpha);
        }
    }

    #[test]
    fn glyph_cache_tracks_allocations_within_byte_capacity() {
        const CAPACITY: usize = 1_024;

        let font = test_font();
        let mut rasterizer = TextRasterizer::with_glyph_cache_capacity(CAPACITY);
        for font_size in [8.0, 10.0, 12.0, 14.0, 16.0, 18.0] {
            rasterizer
                .rasterize_word(&font, "cache", font_size, 500, 240)
                .unwrap();
            assert!(rasterizer.glyph_cache.used_bytes <= CAPACITY);
            assert!(rasterizer.glyph_cache.entries.len() <= DEFAULT_GLYPH_CACHE_MAX_ENTRIES);
            assert_eq!(
                rasterizer.glyph_cache.used_bytes,
                rasterizer
                    .glyph_cache
                    .entries
                    .values()
                    .map(Vec::capacity)
                    .sum::<usize>()
            );
        }
        assert!(!rasterizer.glyph_cache.entries.is_empty());
    }

    #[test]
    fn glyph_larger_than_capacity_bypasses_cache() {
        let font = test_font();
        let expected = reference_rasterize_word(&font, "large", 72.0, 500, 240).unwrap();
        let mut rasterizer = TextRasterizer::with_glyph_cache_capacity(1);

        let actual = rasterizer
            .rasterize_word(&font, "large", 72.0, 500, 240)
            .unwrap();

        assert_eq!(actual.alpha, expected.alpha);
        assert!(rasterizer.glyph_cache.entries.is_empty());
        assert_eq!(rasterizer.glyph_cache.used_bytes, 0);
    }
}
