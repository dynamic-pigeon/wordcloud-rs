#[derive(Clone, Debug)]
pub(crate) struct AlphaBitmap {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

impl AlphaBitmap {
    #[cfg(test)]
    pub fn rotate_clockwise(&self) -> Self {
        let mut alpha = vec![0; self.alpha.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                let source = (y * self.width + x) as usize;
                let target_x = self.height - 1 - y;
                let target_y = x;
                let target = (target_y * self.height + target_x) as usize;
                alpha[target] = self.alpha[source];
            }
        }
        Self {
            width: self.height,
            height: self.width,
            alpha,
        }
    }

    #[cfg(test)]
    pub fn bits(&self) -> BitGrid {
        let mut bits = BitGrid::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.alpha[(y * self.width + x) as usize] != 0 {
                    bits.set(x, y);
                }
            }
        }
        bits
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BitGrid {
    width: u32,
    height: u32,
    stride: usize,
    rows: Vec<u64>,
}

impl BitGrid {
    pub fn new(width: u32, height: u32) -> Self {
        let stride = (width as usize).div_ceil(64);
        Self {
            width,
            height,
            stride,
            rows: vec![0; stride.saturating_mul(height as usize)],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns a copy rotated 90 degrees clockwise, matching
    /// [`AlphaBitmap::rotate_clockwise`]. Only set bits are visited, so the
    /// cost scales with ink rather than with the grid area.
    pub fn rotate_clockwise(&self) -> Self {
        let mut rotated = Self::new(self.height, self.width);
        for y in 0..self.height as usize {
            let row_start = y * self.stride;
            for word in 0..self.stride {
                let mut value = self.rows[row_start + word];
                while value != 0 {
                    let bit = value.trailing_zeros();
                    let x = word as u32 * 64 + bit;
                    rotated.set(self.height - 1 - y as u32, x);
                    value &= value - 1;
                }
            }
        }
        rotated
    }

    /// Positions where a `width` x `height` window is completely free, as a
    /// `(self.width - width + 1)` x `(self.height - height + 1)` grid whose
    /// bit `(x, y)` means "the window with its top-left corner at `(x, y)`
    /// covers no set bit of this grid". Returns `None` when the window is
    /// larger than this grid in either dimension.
    ///
    /// This is the bit-grid equivalent of the Python `wordcloud` integral
    /// image query: two sliding passes (horizontal, then vertical) erode the
    /// free space, so every valid placement is enumerable exactly once.
    pub fn free_windows(&self, width: u32, height: u32) -> Option<BitGrid> {
        if width == 0 || height == 0 || width > self.width || height > self.height {
            return None;
        }
        let horizontal_width = self.width - width + 1;
        let mut horizontal = BitGrid::new(horizontal_width, self.height);
        for y in 0..self.height {
            let mut occupied_count = 0_u32;
            for x in 0..width {
                if self.get(x, y) {
                    occupied_count += 1;
                }
            }
            if occupied_count == 0 {
                horizontal.set(0, y);
            }
            for start in 1..horizontal_width {
                if self.get(start + width - 1, y) {
                    occupied_count += 1;
                }
                if self.get(start - 1, y) {
                    occupied_count -= 1;
                }
                if occupied_count == 0 {
                    horizontal.set(start, y);
                }
            }
        }

        let mut valid = BitGrid::new(horizontal_width, self.height - height + 1);
        for x in 0..horizontal_width {
            let mut free_count = 0_u32;
            for y in 0..height {
                if horizontal.get(x, y) {
                    free_count += 1;
                }
            }
            if free_count == height {
                valid.set(x, 0);
            }
            for start in 1..valid.height() {
                if horizontal.get(x, start + height - 1) {
                    free_count += 1;
                }
                if horizontal.get(x, start - 1) {
                    free_count -= 1;
                }
                if free_count == height {
                    valid.set(x, start);
                }
            }
        }
        Some(valid)
    }

    /// The number of set bits.
    pub fn popcount(&self) -> u64 {
        self.rows
            .iter()
            .map(|word| u64::from(word.count_ones()))
            .sum()
    }

    /// The `goal`-th set bit in row-major order, or `None` when `goal` is
    /// out of range.
    pub fn select_set(&self, goal: u64) -> Option<(u32, u32)> {
        let mut goal = goal;
        for (index, &word) in self.rows.iter().enumerate() {
            let ones = u64::from(word.count_ones());
            if goal < ones {
                let mut remaining = word;
                for _ in 0..goal {
                    remaining &= remaining - 1;
                }
                // `rows` is flat with `stride` words per row, so decode the
                // flat word index into (row, word) before adding the bit.
                let y = (index / self.stride) as u32;
                let x = (index % self.stride) as u32 * 64 + remaining.trailing_zeros();
                return Some((x, y));
            }
            goal -= ones;
        }
        None
    }

    pub fn set(&mut self, x: u32, y: u32) {
        debug_assert!(x < self.width && y < self.height);
        let index = y as usize * self.stride + x as usize / 64;
        self.rows[index] |= 1_u64 << (x % 64);
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        debug_assert!(x < self.width && y < self.height);
        let index = y as usize * self.stride + x as usize / 64;
        self.rows[index] & (1_u64 << (x % 64)) != 0
    }

    #[cfg(test)]
    pub fn collides(&self, other: &Self, x: u32, y: u32) -> bool {
        if x.checked_add(other.width)
            .is_none_or(|right| right > self.width)
            || y.checked_add(other.height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return true;
        }
        self.collides_in_bounds(other, x, y)
    }

    #[cfg(test)]
    pub fn collides_in_bounds(&self, other: &Self, x: u32, y: u32) -> bool {
        debug_assert!(x + other.width <= self.width);
        debug_assert!(y + other.height <= self.height);
        self.for_each_shifted_chunk(other, x, y, |target, value| self.rows[target] & value != 0)
    }

    /// ORs the low bits of `bits` into row `y` starting at column `x`.
    /// Caller guarantees that every set bit of `bits` corresponds to a
    /// column below `width`.
    pub(crate) fn or_bits(&mut self, x: u32, y: u32, bits: u64) {
        debug_assert!(x < self.width && y < self.height);
        let index = y as usize * self.stride + x as usize / 64;
        let shift = x % 64;
        self.rows[index] |= bits << shift;
        if shift != 0 {
            let spill = bits >> (64 - shift);
            if spill != 0 {
                debug_assert!(index + 1 < self.rows.len());
                self.rows[index + 1] |= spill;
            }
        }
    }

    pub fn insert(&mut self, other: &Self, x: u32, y: u32) -> bool {
        if x.checked_add(other.width)
            .is_none_or(|right| right > self.width)
            || y.checked_add(other.height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return false;
        }

        let target_stride = self.stride;
        let source_stride = other.stride;
        let word_offset = x as usize / 64;
        let shift = x % 64;
        for source_y in 0..other.height as usize {
            let target_row = (y as usize + source_y) * target_stride;
            let source_row = source_y * source_stride;
            for source_word in 0..source_stride {
                let value = other.rows[source_row + source_word];
                if value == 0 {
                    continue;
                }
                let target = target_row + word_offset + source_word;
                self.rows[target] |= value << shift;
                if shift != 0 && target + 1 < target_row + target_stride {
                    self.rows[target + 1] |= value >> (64 - shift);
                }
            }
        }
        true
    }

    #[cfg(test)]
    fn for_each_shifted_chunk<F>(&self, other: &Self, x: u32, y: u32, mut test: F) -> bool
    where
        F: FnMut(usize, u64) -> bool,
    {
        let word_offset = x as usize / 64;
        let shift = x % 64;
        for source_y in 0..other.height as usize {
            let target_row = (y as usize + source_y) * self.stride;
            let source_row = source_y * other.stride;
            for source_word in 0..other.stride {
                let value = other.rows[source_row + source_word];
                if value == 0 {
                    continue;
                }
                let target = target_row + word_offset + source_word;
                if test(target, value << shift) {
                    return true;
                }
                if shift != 0
                    && target + 1 < target_row + self.stride
                    && test(target + 1, value >> (64 - shift))
                {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_collision_and_insert_cross_word_boundaries() {
        for x in [0, 1, 63, 64, 65, 127] {
            let mut canvas = BitGrid::new(196, 4);
            let mut item = BitGrid::new(66, 2);
            item.set(0, 0);
            item.set(63, 0);
            item.set(64, 1);
            item.set(65, 1);
            assert!(!canvas.collides(&item, x, 1));
            assert!(canvas.insert(&item, x, 1));
            assert!(canvas.collides(&item, x, 1));
            assert!(canvas.get(x, 1));
            assert!(canvas.get(x + 63, 1));
            assert!(canvas.get(x + 64, 2));
            assert!(canvas.get(x + 65, 2));
        }
    }

    #[test]
    fn bit_grid_matches_a_pixel_oracle_across_sizes_and_offsets() {
        for canvas_width in [1, 2, 63, 64, 65, 127, 128, 129] {
            for item_width in [1, 2, 31, 63, 64, 65] {
                if item_width > canvas_width {
                    continue;
                }
                for x in 0..=canvas_width - item_width {
                    let mut canvas = BitGrid::new(canvas_width, 4);
                    for cy in 0..4 {
                        for cx in 0..canvas_width {
                            if (cx.wrapping_mul(17) + cy * 11) % 23 == 0 {
                                canvas.set(cx, cy);
                            }
                        }
                    }
                    let mut item = BitGrid::new(item_width, 3);
                    for iy in 0..3 {
                        for ix in 0..item_width {
                            if (ix.wrapping_mul(7) + iy * 5) % 13 == 0 {
                                item.set(ix, iy);
                            }
                        }
                    }

                    let expected = (0..3).any(|iy| {
                        (0..item_width).any(|ix| item.get(ix, iy) && canvas.get(x + ix, 1 + iy))
                    });
                    assert_eq!(canvas.collides(&item, x, 1), expected);

                    let mut inserted = canvas.clone();
                    assert!(inserted.insert(&item, x, 1));
                    for iy in 0..3 {
                        for ix in 0..item_width {
                            if item.get(ix, iy) {
                                assert!(inserted.get(x + ix, 1 + iy));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bit_grid_rotation_matches_bitmap_rotation() {
        let bitmap = AlphaBitmap {
            width: 5,
            height: 3,
            alpha: vec![
                1, 0, 0, 2, 0, //
                0, 0, 3, 0, 0, //
                0, 4, 0, 0, 5,
            ],
        };

        let rotated_bits = bitmap.bits().rotate_clockwise();
        let rotated_alpha_bits = bitmap.rotate_clockwise().bits();

        assert_eq!(rotated_bits.width, rotated_alpha_bits.width);
        assert_eq!(rotated_bits.height, rotated_alpha_bits.height);
        assert_eq!(rotated_bits.rows, rotated_alpha_bits.rows);

        let quadruple = rotated_bits
            .rotate_clockwise()
            .rotate_clockwise()
            .rotate_clockwise();
        let original = bitmap.bits();
        assert_eq!(quadruple.width, original.width);
        assert_eq!(quadruple.height, original.height);
        assert_eq!(quadruple.rows, original.rows);
    }

    #[test]
    fn or_bits_matches_per_pixel_sets() {
        let mut state = 0x5eed_cafe_1234_5678_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state
        };

        for width in [1_u32, 8, 9, 63, 64, 65, 71] {
            let height = 5_u32;
            for case in 0..64 {
                let mut packed = BitGrid::new(width, height);
                let mut scattered = BitGrid::new(width, height);
                for y in 0..height {
                    let mut x = (next() % 71) as u32;
                    while x < width {
                        let mut bits = 0_u64;
                        let take = (width - x).min(8);
                        for bit in 0..take {
                            if next() % 3 != 0 {
                                bits |= 1_u64 << bit;
                            }
                        }
                        if bits != 0 {
                            packed.or_bits(x, y, bits);
                            for bit in 0..take {
                                if bits & (1 << bit) != 0 {
                                    scattered.set(x + bit, y);
                                }
                            }
                        }
                        x += take;
                    }
                }
                assert_eq!(packed, scattered, "width {width} case {case}");
            }
        }
    }

    #[test]
    fn rotating_four_times_returns_original() {
        let original = AlphaBitmap {
            width: 3,
            height: 2,
            alpha: vec![1, 2, 3, 4, 5, 6],
        };
        let rotated = original
            .rotate_clockwise()
            .rotate_clockwise()
            .rotate_clockwise()
            .rotate_clockwise();
        assert_eq!(rotated.width, original.width);
        assert_eq!(rotated.height, original.height);
        assert_eq!(rotated.alpha, original.alpha);
    }

    #[test]
    fn free_windows_matches_a_pixel_oracle() {
        for canvas_width in [7_u32, 8, 63, 64, 65] {
            for canvas_height in [3_u32, 5] {
                let mut canvas = BitGrid::new(canvas_width, canvas_height);
                for y in 0..canvas_height {
                    for x in 0..canvas_width {
                        if (x.wrapping_mul(13) + y * 7) % 5 == 0 {
                            canvas.set(x, y);
                        }
                    }
                }
                for window_width in [1_u32, 2, 3, 5, canvas_width] {
                    for window_height in [1_u32, 2, 3, canvas_height] {
                        if window_width > canvas_width || window_height > canvas_height {
                            continue;
                        }
                        let actual = canvas.free_windows(window_width, window_height).unwrap();
                        assert_eq!(
                            (actual.width(), actual.height()),
                            (
                                canvas_width - window_width + 1,
                                canvas_height - window_height + 1
                            )
                        );
                        for y in 0..actual.height() {
                            for x in 0..actual.width() {
                                let mut free = true;
                                'window: for dy in 0..window_height {
                                    for dx in 0..window_width {
                                        if canvas.get(x + dx, y + dy) {
                                            free = false;
                                            break 'window;
                                        }
                                    }
                                }
                                assert_eq!(
                                    actual.get(x, y),
                                    free,
                                    "canvas {canvas_width}x{canvas_height} window \
                                     {window_width}x{window_height} at ({x}, {y})"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(BitGrid::new(8, 4).free_windows(9, 1).is_none());
        assert!(BitGrid::new(8, 4).free_windows(1, 5).is_none());
        assert!(BitGrid::new(8, 4).free_windows(0, 1).is_none());
    }

    #[test]
    fn popcount_and_select_set_walk_row_major_order() {
        let mut grid = BitGrid::new(70, 3);
        let expected = [
            (0_u32, 0_u32),
            (63, 0),
            (64, 0),
            (69, 0),
            (5, 2),
            (70 - 1, 2),
        ];
        for (x, y) in expected {
            grid.set(x, y);
        }
        assert_eq!(grid.popcount(), expected.len() as u64);
        for (goal, position) in expected.iter().enumerate() {
            assert_eq!(grid.select_set(goal as u64), Some(*position));
        }
        assert_eq!(grid.select_set(expected.len() as u64), None);
        assert_eq!(BitGrid::new(4, 4).select_set(0), None);
    }
}
