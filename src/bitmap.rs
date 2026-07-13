#[derive(Clone, Debug)]
pub(crate) struct AlphaBitmap {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

impl AlphaBitmap {
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

    /// Returns an ink mask expanded by `radius`, with the original bitmap
    /// located at `(radius, radius)` inside it.
    pub fn dilated_bits(&self, radius: u32) -> Option<BitGrid> {
        let width = self.width.checked_add(radius.checked_mul(2)?)?;
        let height = self.height.checked_add(radius.checked_mul(2)?)?;
        if radius == 0 {
            return Some(self.bits());
        }

        let diameter = radius.checked_mul(2)?.checked_add(1)?;

        // Two sliding-window passes perform a square dilation in O(output
        // area) while retaining the compact bit-grid memory footprint.
        let mut horizontal = BitGrid::new(width, self.height);
        for y in 0..self.height {
            let mut ink_count = 0_u32;
            for output_x in 0..width {
                if output_x < self.width && self.alpha[(y * self.width + output_x) as usize] != 0 {
                    ink_count += 1;
                }
                if let Some(expired_x) = output_x.checked_sub(diameter) {
                    if expired_x < self.width
                        && self.alpha[(y * self.width + expired_x) as usize] != 0
                    {
                        ink_count -= 1;
                    }
                }
                if ink_count != 0 {
                    horizontal.set(output_x, y);
                }
            }
        }

        let mut dilated = BitGrid::new(width, height);
        for x in 0..width {
            let mut ink_count = 0_u32;
            for output_y in 0..height {
                if output_y < self.height && horizontal.get(x, output_y) {
                    ink_count += 1;
                }
                if let Some(expired_y) = output_y.checked_sub(diameter) {
                    if expired_y < self.height && horizontal.get(x, expired_y) {
                        ink_count -= 1;
                    }
                }
                if ink_count != 0 {
                    dilated.set(x, output_y);
                }
            }
        }
        Some(dilated)
    }
}

#[derive(Clone, Debug)]
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

    pub fn collides(&self, other: &Self, x: u32, y: u32) -> bool {
        if x.checked_add(other.width)
            .is_none_or(|right| right > self.width)
            || y.checked_add(other.height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return true;
        }
        self.for_each_shifted_chunk(other, x, y, |target, value| self.rows[target] & value != 0)
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
    fn dilation_expands_once_around_ink() {
        let bitmap = AlphaBitmap {
            width: 3,
            height: 2,
            alpha: vec![0, 0, 0, 0, 255, 0],
        };
        let expanded = bitmap.dilated_bits(2).unwrap();
        assert_eq!((expanded.width(), expanded.height()), (7, 6));
        for y in 1..=5 {
            for x in 1..=5 {
                assert!(expanded.get(x, y));
            }
        }
        assert!(!expanded.get(0, 0));
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
}
