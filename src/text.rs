use std::collections::{HashMap, VecDeque};

use ab_glyph::{point, Font, FontArc, Glyph, GlyphId, PxScale, ScaleFont};

use crate::bitmap::{AlphaBitmap, BitGrid};

const DEFAULT_GLYPH_CACHE_CAPACITY_BYTES: usize = 256 * 1024;
const DEFAULT_GLYPH_CACHE_MAX_ENTRIES: usize = 4_096;
const BOUNDS_CACHE_MAX_ENTRIES: usize = 8_192;

pub(crate) fn supports_word(font: &FontArc, word: &str) -> bool {
    let mut visible = false;
    for character in word.chars().filter(|character| !character.is_whitespace()) {
        visible = true;
        if font.glyph_id(character).0 == 0 {
            return false;
        }
    }
    visible
}

/// A glyph of a laid-out word: its identifier and baseline x position.
struct PositionedGlyph {
    id: GlyphId,
    x: f32,
}

/// The bounding box of a laid-out word, in pixels, relative to the top-left
/// corner of the box itself.
struct WordBounds {
    min_x: i64,
    min_y: i64,
    width: u32,
    height: u32,
}

/// Coverage and placement of a single rasterized glyph, relative to its
/// layout position.
struct CachedGlyph {
    width: u32,
    height: u32,
    offset_x: i64,
    offset_y: i64,
    coverage: Vec<u8>,
}

/// The float pixel bounds of a glyph positioned at the origin. Bounds at any
/// baseline position are derived by translating `min_x`/`max_x`, which is the
/// same floating-point arithmetic the outline would report there.
#[derive(Clone, Copy)]
struct CachedBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

pub(crate) struct TextRasterizer {
    glyph_cache: GlyphRasterCache,
    bounds_cache: HashMap<(GlyphId, u32), Option<CachedBounds>>,
}

impl TextRasterizer {
    pub(crate) fn new() -> Self {
        Self::with_glyph_cache_capacity(DEFAULT_GLYPH_CACHE_CAPACITY_BYTES)
    }

    fn with_glyph_cache_capacity(capacity_bytes: usize) -> Self {
        Self {
            glyph_cache: GlyphRasterCache::new(capacity_bytes),
            bounds_cache: HashMap::new(),
        }
    }

    /// Positions the glyphs of `word` on a horizontal baseline: integer
    /// advances, no kerning, control characters produce nothing.
    fn layout_word(font: &FontArc, word: &str, font_size: f32) -> Vec<PositionedGlyph> {
        let scaled = font.as_scaled(PxScale::from(font_size));
        let mut x = 0.0_f32;
        // Pre-size by scalar count, not byte length: CJK characters are three
        // bytes each and would otherwise over-allocate the layout vector.
        let mut glyphs = Vec::with_capacity(word.chars().count());
        for character in word.chars() {
            if character.is_control() {
                continue;
            }
            let id = font.glyph_id(character);
            let advance = scaled.h_advance(id).ceil();
            if !advance.is_finite() {
                continue;
            }
            glyphs.push(PositionedGlyph { id, x });
            x += advance;
        }
        glyphs
    }

    /// Returns the pixel bounds of a laid-out glyph as `(min_x, min_y, width,
    /// height)`, or `None` when the glyph has no visible outline. The raw
    /// outline bounds are cached per (glyph, size) so the font-size retry
    /// loop does not re-extract outlines that only moved horizontally.
    fn glyph_pixel_bounds(
        &mut self,
        font: &FontArc,
        glyph: &PositionedGlyph,
        font_size: f32,
    ) -> Option<(i64, i64, u32, u32)> {
        let key = (glyph.id, font_size.to_bits());
        let cached = if let Some(cached) = self.bounds_cache.get(&key) {
            *cached
        } else {
            let bounds = font
                .outline_glyph(Glyph {
                    id: glyph.id,
                    scale: PxScale::from(font_size),
                    position: point(0.0, 0.0),
                })
                .map(|outlined| {
                    let bounds = outlined.px_bounds();
                    CachedBounds {
                        min_x: bounds.min.x,
                        min_y: bounds.min.y,
                        max_x: bounds.max.x,
                        max_y: bounds.max.y,
                    }
                });
            if self.bounds_cache.len() >= BOUNDS_CACHE_MAX_ENTRIES {
                self.bounds_cache.clear();
            }
            self.bounds_cache.insert(key, bounds);
            bounds
        };
        let bounds = cached?;
        let min_x = bounds.min_x + glyph.x;
        let max_x = bounds.max_x + glyph.x;
        let width = (max_x - min_x) as u32;
        let height = (bounds.max_y - bounds.min_y) as u32;
        if width == 0 || height == 0 {
            return None;
        }
        Some((
            min_x.floor() as i64,
            bounds.min_y.floor() as i64,
            width,
            height,
        ))
    }

    /// Computes the pixel bounding box of a word, returning `None` when the
    /// word has no visible glyphs or does not fit the canvas in at least one
    /// orientation. This only reads (cached) glyph outline bounds; it never
    /// rasterizes.
    fn word_bounds(
        &mut self,
        font: &FontArc,
        glyphs: &[PositionedGlyph],
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<WordBounds> {
        let mut min_x = i64::MAX;
        let mut min_y = i64::MAX;
        let mut max_x = i64::MIN;
        let mut max_y = i64::MIN;
        let mut has_visible_glyph = false;
        for glyph in glyphs {
            let Some((glyph_x, glyph_y, width, height)) =
                self.glyph_pixel_bounds(font, glyph, font_size)
            else {
                continue;
            };
            has_visible_glyph = true;
            let right = glyph_x.checked_add(i64::from(width))?;
            let bottom = glyph_y.checked_add(i64::from(height))?;
            min_x = min_x.min(glyph_x);
            min_y = min_y.min(glyph_y);
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

    /// Returns the ink mask of `word` without materializing its coverage
    /// bitmap. Bits are set exactly where [`TextRasterizer::rasterize_word`]
    /// would produce non-zero coverage.
    pub(crate) fn word_ink(
        &mut self,
        font: &FontArc,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<BitGrid> {
        let glyphs = Self::layout_word(font, word, font_size);
        let bounds = self.word_bounds(font, &glyphs, font_size, canvas_width, canvas_height)?;
        let mut ink = BitGrid::new(bounds.width, bounds.height);
        for positioned in &glyphs {
            self.glyph_cache
                .with_rasterized(font, positioned.id, font_size, |glyph| {
                    let base_x = positioned.x.round() as i64 + glyph.offset_x - bounds.min_x;
                    let base_y = glyph.offset_y - bounds.min_y;
                    debug_assert!(base_x >= 0 && base_y >= 0);
                    debug_assert!(
                        base_x + i64::from(glyph.width) <= i64::from(bounds.width)
                            && base_y + i64::from(glyph.height) <= i64::from(bounds.height)
                    );
                    // Columns are packed into 8-wide bit groups so a solid
                    // glyph row is written with one shifted word OR instead
                    // of eight per-pixel sets.
                    for row in 0..glyph.height {
                        let y = base_y + i64::from(row);
                        let coverage = &glyph.coverage[(row * glyph.width) as usize
                            ..(row * glyph.width + glyph.width) as usize];
                        let mut column = 0_u32;
                        while column < glyph.width {
                            let take = (glyph.width - column).min(8);
                            let mut bits = 0_u64;
                            for bit in 0..take {
                                if coverage[(column + bit) as usize] != 0 {
                                    bits |= 1_u64 << bit;
                                }
                            }
                            if bits != 0 {
                                ink.or_bits((base_x + i64::from(column)) as u32, y as u32, bits);
                            }
                            column += take;
                        }
                    }
                });
        }
        Some(ink)
    }

    pub(crate) fn rasterize_word(
        &mut self,
        font: &FontArc,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<AlphaBitmap> {
        self.rasterize_word_impl(font, word, font_size, canvas_width, canvas_height, false)
    }

    /// Like [`TextRasterizer::rasterize_word`], but materializes the coverage
    /// directly in clockwise-rotated orientation. Only one `width * height`
    /// buffer is allocated; a horizontal rasterization plus a rotation would
    /// briefly hold two.
    pub(crate) fn rasterize_word_rotated(
        &mut self,
        font: &FontArc,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<AlphaBitmap> {
        self.rasterize_word_impl(font, word, font_size, canvas_width, canvas_height, true)
    }

    fn rasterize_word_impl(
        &mut self,
        font: &FontArc,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
        rotated: bool,
    ) -> Option<AlphaBitmap> {
        let glyphs = Self::layout_word(font, word, font_size);
        let bounds = self.word_bounds(font, &glyphs, font_size, canvas_width, canvas_height)?;
        let (output_width, output_height) = if rotated {
            (bounds.height, bounds.width)
        } else {
            (bounds.width, bounds.height)
        };
        let width_usize = usize::try_from(output_width).ok()?;
        let mut alpha = vec![0_u8; width_usize.checked_mul(output_height as usize)?];

        for positioned in &glyphs {
            self.glyph_cache
                .with_rasterized(font, positioned.id, font_size, |glyph| {
                    let base_x = positioned.x.round() as i64 + glyph.offset_x - bounds.min_x;
                    let base_y = glyph.offset_y - bounds.min_y;
                    debug_assert!(base_x >= 0 && base_y >= 0);
                    debug_assert!(
                        base_x + i64::from(glyph.width) <= i64::from(bounds.width)
                            && base_y + i64::from(glyph.height) <= i64::from(bounds.height)
                    );
                    if !rotated {
                        let base_x = base_x as usize;
                        let base_y = base_y as usize;
                        let source_rows = glyph.coverage.chunks_exact(glyph.width as usize);
                        let target_rows = alpha.chunks_exact_mut(width_usize).skip(base_y);
                        for (source_row, target_row) in source_rows.zip(target_rows) {
                            let target_row = &mut target_row[base_x..base_x + glyph.width as usize];
                            for (target, source) in target_row.iter_mut().zip(source_row) {
                                *target = (*target).max(*source);
                            }
                        }
                    } else {
                        // Clockwise 90-degree mapping: source (gx, gy) lands
                        // at (output_width - 1 - gy, gx). `base_x`/`base_y`
                        // already account for the horizontal bounds offset.
                        let output_width_i64 = i64::from(output_width);
                        for gy in 0..glyph.height {
                            let target_x =
                                (output_width_i64 - 1 - (base_y + i64::from(gy))) as usize;
                            let source_row = &glyph.coverage[(gy * glyph.width) as usize
                                ..(gy * glyph.width + glyph.width) as usize];
                            for (gx, coverage) in source_row.iter().enumerate() {
                                if *coverage == 0 {
                                    continue;
                                }
                                let target_y = (base_x + i64::from(gx as u32)) as usize;
                                let target = target_y * width_usize + target_x;
                                alpha[target] = alpha[target].max(*coverage);
                            }
                        }
                    }
                });
        }

        Some(AlphaBitmap {
            width: output_width,
            height: output_height,
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
    entries: HashMap<(GlyphId, u32), CachedGlyph>,
    /// Insertion order of `entries`, for first-in-first-out eviction.
    order: VecDeque<(GlyphId, u32)>,
    capacity_bytes: usize,
    max_entries: usize,
    used_bytes: usize,
}

impl GlyphRasterCache {
    fn new(capacity_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity_bytes,
            max_entries: DEFAULT_GLYPH_CACHE_MAX_ENTRIES,
            used_bytes: 0,
        }
    }

    /// Calls `use_glyph` with the cached coverage of `id` at `font_size`,
    /// rasterizing and caching it on first use. Glyphs without a visible
    /// outline are skipped silently.
    fn with_rasterized<R>(
        &mut self,
        font: &FontArc,
        id: GlyphId,
        font_size: f32,
        use_glyph: impl FnOnce(&CachedGlyph) -> R,
    ) -> Option<R> {
        let key = (id, font_size.to_bits());
        if let Some(cached) = self.entries.get(&key) {
            return Some(use_glyph(cached));
        }

        let rasterized = rasterize_glyph(font, id, font_size)?;
        let allocation_bytes = rasterized.coverage.capacity();
        if allocation_bytes == 0 || allocation_bytes > self.capacity_bytes {
            return Some(use_glyph(&rasterized));
        }

        // Evict oldest-first until the new glyph fits. Clearing everything
        // instead would throw away the current word's glyphs between the
        // placement search and the final rasterization, forcing every placed
        // word to be rasterized twice.
        while self.entries.len() >= self.max_entries
            || self.used_bytes > self.capacity_bytes - allocation_bytes
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.used_bytes -= removed.coverage.capacity();
            }
        }
        self.used_bytes += allocation_bytes;
        self.entries.insert(key, rasterized);
        self.order.push_back(key);
        debug_assert!(self.used_bytes <= self.capacity_bytes);
        Some(use_glyph(
            self.entries
                .get(&key)
                .expect("inserted glyph must be cached"),
        ))
    }
}

/// Rasterizes a single glyph positioned at the origin, returning `None` when
/// the glyph has no visible outline.
fn rasterize_glyph(font: &FontArc, id: GlyphId, font_size: f32) -> Option<CachedGlyph> {
    let outlined = font.outline_glyph(Glyph {
        id,
        scale: PxScale::from(font_size),
        position: point(0.0, 0.0),
    })?;
    let bounds = outlined.px_bounds();
    let width = bounds.width() as u32;
    let height = bounds.height() as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let mut coverage = vec![0_u8; (width as usize).checked_mul(height as usize)?];
    outlined.draw(|x, y, value| {
        let index = (y as usize) * (width as usize) + x as usize;
        coverage[index] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    });
    Some(CachedGlyph {
        width,
        height,
        offset_x: bounds.min.x.floor() as i64,
        offset_y: bounds.min.y.floor() as i64,
        coverage,
    })
}

#[cfg(test)]
mod tests {
    use ab_glyph::{point, Font, Glyph, PxScale};

    use super::*;

    fn test_font() -> FontArc {
        FontArc::try_from_slice(include_bytes!("../assets/OpenSans-Regular.ttf") as &[u8]).unwrap()
    }

    /// An independent, cache-free rasterization used to cross-check the
    /// cached pipeline.
    fn reference_rasterize_word(
        font: &FontArc,
        word: &str,
        font_size: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<AlphaBitmap> {
        let scaled = font.as_scaled(PxScale::from(font_size));
        let mut x = 0.0_f32;
        let mut outlined = Vec::new();
        for character in word.chars() {
            if character.is_control() {
                continue;
            }
            let id = font.glyph_id(character);
            let advance = scaled.h_advance(id).ceil();
            if let Some(glyph) = font.outline_glyph(Glyph {
                id,
                scale: PxScale::from(font_size),
                position: point(x, 0.0),
            }) {
                outlined.push(glyph);
            }
            x += advance;
        }

        let mut min_x = i64::MAX;
        let mut min_y = i64::MAX;
        let mut max_x = i64::MIN;
        let mut max_y = i64::MIN;
        let mut has_visible_glyph = false;
        for glyph in &outlined {
            let bounds = glyph.px_bounds();
            if bounds.width() as u32 == 0 || bounds.height() as u32 == 0 {
                continue;
            }
            has_visible_glyph = true;
            let left = bounds.min.x.floor() as i64;
            let top = bounds.min.y.floor() as i64;
            min_x = min_x.min(left);
            min_y = min_y.min(top);
            max_x = max_x.max(left + bounds.width() as i64);
            max_y = max_y.max(top + bounds.height() as i64);
        }
        if !has_visible_glyph {
            return None;
        }

        let width = u32::try_from(max_x - min_x).ok()?;
        let height = u32::try_from(max_y - min_y).ok()?;
        let horizontal_fits = width <= canvas_width && height <= canvas_height;
        let vertical_fits = height <= canvas_width && width <= canvas_height;
        if width == 0 || height == 0 || (!horizontal_fits && !vertical_fits) {
            return None;
        }
        let mut alpha = vec![0_u8; (width as usize).checked_mul(height as usize)?];

        for glyph in &outlined {
            let bounds = glyph.px_bounds();
            let base_x = (bounds.min.x.floor() as i64 - min_x) as usize;
            let base_y = (bounds.min.y.floor() as i64 - min_y) as usize;
            glyph.draw(|glyph_x, glyph_y, value| {
                let target =
                    (base_y + glyph_y as usize) * width as usize + base_x + glyph_x as usize;
                let coverage = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                alpha[target] = alpha[target].max(coverage);
            });
        }

        Some(AlphaBitmap {
            width,
            height,
            alpha,
        })
    }

    #[test]
    fn rotated_rasterization_matches_horizontal_then_rotate() {
        let font = test_font();
        let mut rasterizer = TextRasterizer::new();

        for (word, font_size) in [
            ("Rust", 61.0),
            ("glyph", 27.0),
            ("affinity", 43.0),
            ("vertical", 19.0),
        ] {
            let horizontal = rasterizer
                .rasterize_word(&font, word, font_size, 500, 240)
                .unwrap();
            let rotated = rasterizer
                .rasterize_word_rotated(&font, word, font_size, 500, 240)
                .unwrap();
            let expected = horizontal.rotate_clockwise();
            assert_eq!(
                (rotated.width, rotated.height),
                (expected.width, expected.height),
                "{word}"
            );
            assert_eq!(rotated.alpha, expected.alpha, "{word}");
        }
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
                    .map(|glyph| glyph.coverage.capacity())
                    .sum::<usize>()
            );
            // The eviction queue lists every cached key exactly once.
            assert_eq!(rasterizer.glyph_cache.order.len(), rasterizer.glyph_cache.entries.len());
            for key in &rasterizer.glyph_cache.order {
                assert!(rasterizer.glyph_cache.entries.contains_key(key));
            }
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
