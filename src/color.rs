use image::Rgba;

use crate::Orientation;

/// Information supplied to a custom word color function.
#[derive(Clone, Copy, Debug)]
pub struct ColorContext<'a> {
    pub word: &'a str,
    pub frequency: f64,
    pub normalized_frequency: f32,
    pub rank: usize,
    pub font_size: f32,
    pub orientation: Orientation,
    pub x: u32,
    pub y: u32,
    /// A deterministic value in `[0, 1)` derived from the color RNG stream.
    pub random: f32,
}

/// Chooses a color after a word has been laid out.
pub trait Colorizer: Send + Sync {
    fn color(&self, context: &ColorContext<'_>) -> Rgba<u8>;
}

impl<F> Colorizer for F
where
    F: for<'a> Fn(&ColorContext<'a>) -> Rgba<u8> + Send + Sync,
{
    fn color(&self, context: &ColorContext<'_>) -> Rgba<u8> {
        self(context)
    }
}

/// A deterministic random choice from a list of colors.
#[derive(Clone, Debug)]
pub struct Palette {
    colors: Vec<Rgba<u8>>,
}

impl Palette {
    pub fn new(colors: impl IntoIterator<Item = Rgba<u8>>) -> Self {
        Self {
            colors: colors.into_iter().collect(),
        }
    }

    pub fn colors(&self) -> &[Rgba<u8>] {
        &self.colors
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::new([
            Rgba([25, 97, 128, 255]),
            Rgba([37, 139, 119, 255]),
            Rgba([83, 158, 96, 255]),
            Rgba([214, 151, 56, 255]),
            Rgba([188, 72, 73, 255]),
            Rgba([107, 78, 133, 255]),
        ])
    }
}

impl Colorizer for Palette {
    fn color(&self, context: &ColorContext<'_>) -> Rgba<u8> {
        if self.colors.is_empty() {
            return Rgba([0, 0, 0, 255]);
        }
        let index =
            ((context.random * self.colors.len() as f32) as usize).min(self.colors.len() - 1);
        self.colors[index]
    }
}

/// Uses one color for every word.
#[derive(Clone, Copy, Debug)]
pub struct SolidColor(pub Rgba<u8>);

impl Colorizer for SolidColor {
    fn color(&self, _context: &ColorContext<'_>) -> Rgba<u8> {
        self.0
    }
}
