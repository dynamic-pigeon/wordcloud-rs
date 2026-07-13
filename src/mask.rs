use image::{DynamicImage, GrayImage};

use crate::{Result, WordCloudError};

const MAX_MASK_PIXELS: u64 = 16_000_000;

/// Determines which side of a grayscale threshold is blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskPolarity {
    /// Bright pixels are blocked. This matches Python wordcloud masks.
    LightBlocked,
    /// Bright pixels are usable and dark pixels are blocked.
    LightAllowed,
}

/// A shape restricting where word pixels may be placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    allowed: Vec<bool>,
}

impl Mask {
    /// Creates a mask from a grayscale image using Python-compatible semantics:
    /// pure-white pixels are blocked.
    pub fn from_luma8(image: GrayImage) -> Self {
        Self::from_luma8_with_threshold(image, 255, MaskPolarity::LightBlocked)
    }

    pub fn from_dynamic(image: &DynamicImage) -> Self {
        Self::from_luma8(image.to_luma8())
    }

    pub fn from_luma8_with_threshold(
        image: GrayImage,
        threshold: u8,
        polarity: MaskPolarity,
    ) -> Self {
        let (width, height) = image.dimensions();
        let allowed = image
            .pixels()
            .map(|pixel| match polarity {
                MaskPolarity::LightBlocked => pixel[0] < threshold,
                MaskPolarity::LightAllowed => pixel[0] >= threshold,
            })
            .collect();
        Self {
            width,
            height,
            allowed,
        }
    }

    /// Creates a mask where the predicate returns `true` for usable pixels.
    pub fn from_predicate<F>(width: u32, height: u32, mut is_allowed: F) -> Result<Self>
    where
        F: FnMut(u32, u32) -> bool,
    {
        if width == 0 || height == 0 {
            return Err(WordCloudError::invalid(
                "mask dimensions",
                "width and height must be greater than zero",
            ));
        }
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_MASK_PIXELS {
            return Err(WordCloudError::CanvasTooLarge { width, height });
        }
        let capacity = usize::try_from(pixels)
            .map_err(|_| WordCloudError::CanvasTooLarge { width, height })?;
        let mut allowed = Vec::new();
        allowed
            .try_reserve_exact(capacity)
            .map_err(|_| WordCloudError::CanvasTooLarge { width, height })?;
        for y in 0..height {
            for x in 0..width {
                allowed.push(is_allowed(x, y));
            }
        }
        Ok(Self {
            width,
            height,
            allowed,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn is_allowed(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.allowed[(y as usize) * (self.width as usize) + (x as usize)]
    }

    pub(crate) fn allowed_pixels(&self) -> &[bool] {
        &self.allowed
    }
}
