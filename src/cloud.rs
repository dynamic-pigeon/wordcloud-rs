use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fontdue::{Font, FontSettings};
use image::{ColorType, DynamicImage, ImageFormat, Pixel, Rgb, RgbImage, Rgba, RgbaImage};

use crate::bitmap::{AlphaBitmap, BitGrid};
use crate::color::{ColorContext, Colorizer, Palette, SolidColor};
use crate::frequency::{collect_frequencies, IntoWordFrequency, WordFrequency};
use crate::mask::Mask;
use crate::text::{supports_word, TextRasterizer};
use crate::tokenizer::{DefaultTokenizer, StopWords, Tokenizer};
use crate::{Result, WordCloudError};

const DEFAULT_FONT: &[u8] = include_bytes!("../assets/OpenSans-Regular.ttf");
const DEFAULT_WIDTH: u32 = 400;
const DEFAULT_HEIGHT: u32 = 200;
const MAX_OUTPUT_PIXELS: u64 = 16_000_000;
const MAX_WORD_LENGTH_LIMIT: usize = 4_096;

/// The font input selected on a [`WordCloudBuilder`].
#[derive(Clone, Debug)]
pub enum FontSource {
    /// The SIL Open Font License 1.1 Open Sans font bundled with this crate.
    Embedded { index: u32 },
    /// A font read when [`WordCloudBuilder::build`] is called.
    Path { path: PathBuf, index: u32 },
    /// Owned TTF, OTF, or TTC bytes.
    Bytes { data: Arc<[u8]>, index: u32 },
}

impl Default for FontSource {
    fn default() -> Self {
        Self::Embedded { index: 0 }
    }
}

impl FontSource {
    fn index(&self) -> u32 {
        match self {
            Self::Embedded { index } | Self::Path { index, .. } | Self::Bytes { index, .. } => {
                *index
            }
        }
    }

    fn set_index(&mut self, new_index: u32) {
        match self {
            Self::Embedded { index } | Self::Path { index, .. } | Self::Bytes { index, .. } => {
                *index = new_index;
            }
        }
    }
}

/// A supported text orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    Horizontal,
    /// Text rotated 90 degrees clockwise.
    Vertical,
}

/// Public placement metadata for a rendered word.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWord {
    pub word: String,
    pub frequency: f64,
    pub normalized_frequency: f32,
    pub rank: usize,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Font size in output pixels.
    pub font_size: f32,
    pub orientation: Orientation,
    pub color: Rgba<u8>,
}

/// An image together with the words used to produce it.
#[derive(Clone, Debug)]
pub struct RenderedWordCloud {
    image: RgbaImage,
    words: Vec<PlacedWord>,
}

impl RenderedWordCloud {
    pub fn image(&self) -> &RgbaImage {
        &self.image
    }

    pub fn words(&self) -> &[PlacedWord] {
        &self.words
    }

    pub fn into_image(self) -> RgbaImage {
        self.image
    }

    pub fn into_dynamic_image(self) -> DynamicImage {
        DynamicImage::ImageRgba8(self.image)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let format = ImageFormat::from_path(path)?;
        match format {
            ImageFormat::Jpeg => {
                let image = RgbImage::from_fn(self.image.width(), self.image.height(), |x, y| {
                    let pixel = self.image[(x, y)];
                    Rgb([pixel[0], pixel[1], pixel[2]])
                });
                image::save_buffer_with_format(
                    path,
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    ColorType::Rgb8,
                    format,
                )?;
            }
            _ => self.image.save_with_format(path, format)?,
        }
        Ok(())
    }
}

/// Configures a reusable [`WordCloud`] generator.
pub struct WordCloudBuilder {
    width: u32,
    height: u32,
    scale: u32,
    max_words: usize,
    min_font_size: f32,
    max_font_size: Option<f32>,
    font_step: f32,
    margin: u32,
    prefer_horizontal: f32,
    relative_scaling: f32,
    random_seed: u64,
    background_color: Rgba<u8>,
    font_source: FontSource,
    mask: Option<Mask>,
    tokenizer: Arc<dyn Tokenizer>,
    stopwords: StopWords,
    lowercase: bool,
    include_numbers: bool,
    min_word_length: usize,
    max_word_length: usize,
    search_attempts: usize,
    colorizer: Arc<dyn Colorizer>,
    colorizer_valid: bool,
}

impl Default for WordCloudBuilder {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            scale: 1,
            max_words: 200,
            min_font_size: 4.0,
            max_font_size: None,
            font_step: 2.0,
            margin: 2,
            prefer_horizontal: 0.9,
            relative_scaling: 0.5,
            random_seed: 0,
            background_color: Rgba([255, 255, 255, 255]),
            font_source: FontSource::default(),
            mask: None,
            tokenizer: Arc::new(DefaultTokenizer),
            stopwords: StopWords::default(),
            lowercase: true,
            include_numbers: false,
            min_word_length: 1,
            max_word_length: 256,
            search_attempts: 1_200,
            colorizer: Arc::new(Palette::default()),
            colorizer_valid: true,
        }
    }
}

impl WordCloudBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }

    /// Multiplies the canvas, font sizes, and margin by an integer output scale.
    pub fn scale(mut self, scale: u32) -> Self {
        self.scale = scale;
        self
    }

    pub fn max_words(mut self, max_words: usize) -> Self {
        self.max_words = max_words;
        self
    }

    pub fn min_font_size(mut self, size: f32) -> Self {
        self.min_font_size = size;
        self
    }

    pub fn max_font_size(mut self, size: f32) -> Self {
        self.max_font_size = Some(size);
        self
    }

    pub fn auto_max_font_size(mut self) -> Self {
        self.max_font_size = None;
        self
    }

    pub fn font_step(mut self, step: f32) -> Self {
        self.font_step = step;
        self
    }

    pub fn margin(mut self, margin: u32) -> Self {
        self.margin = margin;
        self
    }

    pub fn prefer_horizontal(mut self, probability: f32) -> Self {
        self.prefer_horizontal = probability;
        self
    }

    pub fn relative_scaling(mut self, relative_scaling: f32) -> Self {
        self.relative_scaling = relative_scaling;
        self
    }

    pub fn random_seed(mut self, seed: u64) -> Self {
        self.random_seed = seed;
        self
    }

    pub fn background_color(mut self, color: Rgba<u8>) -> Self {
        self.background_color = color;
        self
    }

    pub fn transparent_background(mut self) -> Self {
        self.background_color = Rgba([0, 0, 0, 0]);
        self
    }

    pub fn font_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.font_source = FontSource::Path {
            path: path.into(),
            index: self.font_source.index(),
        };
        self
    }

    /// Uses an explicitly constructed font source.
    pub fn font_source(mut self, source: FontSource) -> Self {
        self.font_source = source;
        self
    }

    pub fn font_bytes(mut self, bytes: impl AsRef<[u8]>) -> Self {
        self.font_source = FontSource::Bytes {
            data: Arc::from(bytes.as_ref()),
            index: self.font_source.index(),
        };
        self
    }

    /// Uses owned font data without requiring an additional copy.
    pub fn font_data(mut self, data: impl Into<Arc<[u8]>>) -> Self {
        self.font_source = FontSource::Bytes {
            data: data.into(),
            index: self.font_source.index(),
        };
        self
    }

    /// Selects a face in a TTC font collection.
    pub fn font_index(mut self, index: u32) -> Self {
        self.font_source.set_index(index);
        self
    }

    pub fn mask(mut self, mask: Mask) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn tokenizer<T>(mut self, tokenizer: T) -> Self
    where
        T: Tokenizer + 'static,
    {
        self.tokenizer = Arc::new(tokenizer);
        self
    }

    /// Replaces the default stop-word collection.
    pub fn stopwords<I, S>(mut self, words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut stopwords = StopWords::new();
        stopwords.extend(words);
        self.stopwords = stopwords;
        self
    }

    /// Replaces the default stop words with an existing collection.
    pub fn stopword_set(mut self, stopwords: StopWords) -> Self {
        self.stopwords = stopwords;
        self
    }

    pub fn add_stopwords<I, S>(mut self, words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.stopwords.extend(words);
        self
    }

    pub fn clear_stopwords(mut self) -> Self {
        self.stopwords.clear();
        self
    }

    pub fn lowercase(mut self, lowercase: bool) -> Self {
        self.lowercase = lowercase;
        self
    }

    pub fn include_numbers(mut self, include_numbers: bool) -> Self {
        self.include_numbers = include_numbers;
        self
    }

    pub fn min_word_length(mut self, length: usize) -> Self {
        self.min_word_length = length;
        self
    }

    /// Rejects individual words longer than this many Unicode scalar values.
    pub fn max_word_length(mut self, length: usize) -> Self {
        self.max_word_length = length;
        self
    }

    /// Maximum candidate positions checked for each orientation and font size.
    pub fn search_attempts(mut self, attempts: usize) -> Self {
        self.search_attempts = attempts;
        self
    }

    pub fn palette(mut self, colors: impl IntoIterator<Item = Rgba<u8>>) -> Self {
        let palette = Palette::new(colors);
        self.colorizer_valid = !palette.is_empty();
        self.colorizer = Arc::new(palette);
        self
    }

    pub fn solid_color(mut self, color: Rgba<u8>) -> Self {
        self.colorizer = Arc::new(SolidColor(color));
        self.colorizer_valid = true;
        self
    }

    pub fn colorizer<C>(mut self, colorizer: C) -> Self
    where
        C: Colorizer + 'static,
    {
        self.colorizer = Arc::new(colorizer);
        self.colorizer_valid = true;
        self
    }

    pub fn build(self) -> Result<WordCloud> {
        let (logical_width, logical_height) = self
            .mask
            .as_ref()
            .map(Mask::dimensions)
            .unwrap_or((self.width, self.height));
        validate_positive_u32("width", logical_width)?;
        validate_positive_u32("height", logical_height)?;
        validate_positive_u32("scale", self.scale)?;
        if self.max_words == 0 {
            return Err(WordCloudError::invalid("max_words", "must be at least 1"));
        }
        validate_positive_f32("min_font_size", self.min_font_size)?;
        validate_positive_f32("font_step", self.font_step)?;
        if let Some(maximum) = self.max_font_size {
            validate_positive_f32("max_font_size", maximum)?;
            if maximum < self.min_font_size {
                return Err(WordCloudError::invalid(
                    "max_font_size",
                    "must be greater than or equal to min_font_size",
                ));
            }
        }
        validate_probability("prefer_horizontal", self.prefer_horizontal)?;
        validate_probability("relative_scaling", self.relative_scaling)?;
        if self.min_word_length == 0 {
            return Err(WordCloudError::invalid(
                "min_word_length",
                "must be at least 1",
            ));
        }
        if self.max_word_length < self.min_word_length
            || self.max_word_length > MAX_WORD_LENGTH_LIMIT
        {
            return Err(WordCloudError::invalid(
                "max_word_length",
                format!("must be between min_word_length and {MAX_WORD_LENGTH_LIMIT}"),
            ));
        }
        if self.search_attempts == 0 {
            return Err(WordCloudError::invalid(
                "search_attempts",
                "must be at least 1",
            ));
        }
        if !self.colorizer_valid {
            return Err(WordCloudError::invalid(
                "palette",
                "must contain at least one color",
            ));
        }

        let width =
            logical_width
                .checked_mul(self.scale)
                .ok_or(WordCloudError::CanvasTooLarge {
                    width: logical_width,
                    height: logical_height,
                })?;
        let height =
            logical_height
                .checked_mul(self.scale)
                .ok_or(WordCloudError::CanvasTooLarge {
                    width: logical_width,
                    height: logical_height,
                })?;
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_OUTPUT_PIXELS {
            return Err(WordCloudError::CanvasTooLarge { width, height });
        }
        let margin = self
            .margin
            .checked_mul(self.scale)
            .ok_or(WordCloudError::CanvasTooLarge { width, height })?;
        if margin.saturating_mul(2) >= width || margin.saturating_mul(2) >= height {
            return Err(WordCloudError::invalid(
                "margin",
                "must leave usable space inside the canvas",
            ));
        }

        let font = load_font(&self.font_source)?;
        let maximum = self
            .max_font_size
            .unwrap_or(logical_height as f32 * 0.5)
            .max(self.min_font_size);
        let min_font_size = scale_font_size("min_font_size", self.min_font_size, self.scale)?;
        let requested_max_font_size = scale_font_size("max_font_size", maximum, self.scale)?;
        let canvas_font_limit = width.max(height) as f32;
        if min_font_size > canvas_font_limit {
            return Err(WordCloudError::invalid(
                "min_font_size",
                "must not exceed the longest canvas dimension",
            ));
        }
        let max_font_size = requested_max_font_size.min(canvas_font_limit);
        let font_step = scale_font_size("font_step", self.font_step, self.scale)?;
        if max_font_size > min_font_size && max_font_size - font_step >= max_font_size {
            return Err(WordCloudError::invalid(
                "font_step",
                "is too small to reduce max_font_size at f32 precision",
            ));
        }
        let mask_occupancy = self
            .mask
            .as_ref()
            .map(|mask| build_mask_occupancy(mask, width, height, self.scale));

        Ok(WordCloud {
            width,
            height,
            scale: self.scale,
            max_words: self.max_words,
            min_font_size,
            max_font_size,
            font_step,
            margin,
            prefer_horizontal: self.prefer_horizontal,
            relative_scaling: self.relative_scaling,
            random_seed: self.random_seed,
            background_color: self.background_color,
            font,
            mask_occupancy,
            tokenizer: self.tokenizer,
            stopwords: self.stopwords,
            lowercase: self.lowercase,
            include_numbers: self.include_numbers,
            min_word_length: self.min_word_length,
            max_word_length: self.max_word_length,
            search_attempts: self.search_attempts,
            colorizer: self.colorizer,
        })
    }
}

/// An immutable and reusable word-cloud generator.
#[derive(Clone)]
pub struct WordCloud {
    width: u32,
    height: u32,
    scale: u32,
    max_words: usize,
    min_font_size: f32,
    max_font_size: f32,
    font_step: f32,
    margin: u32,
    prefer_horizontal: f32,
    relative_scaling: f32,
    random_seed: u64,
    background_color: Rgba<u8>,
    font: Font,
    mask_occupancy: Option<BitGrid>,
    tokenizer: Arc<dyn Tokenizer>,
    stopwords: StopWords,
    lowercase: bool,
    include_numbers: bool,
    min_word_length: usize,
    max_word_length: usize,
    search_attempts: usize,
    colorizer: Arc<dyn Colorizer>,
}

impl WordCloud {
    pub fn builder() -> WordCloudBuilder {
        WordCloudBuilder::new()
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

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn word_frequencies(&self, text: &str) -> Result<Vec<WordFrequency>> {
        let mut counts = BTreeMap::<String, u64>::new();
        for candidate in self.tokenizer.tokenize(text) {
            let candidate = candidate.trim();
            if candidate.is_empty() {
                continue;
            }
            let word = if self.lowercase {
                candidate.to_lowercase()
            } else {
                candidate.to_owned()
            };
            let word_length = word.chars().count();
            if word_length > self.max_word_length {
                return Err(WordCloudError::InvalidWord {
                    word,
                    reason: format!("exceeds max_word_length ({})", self.max_word_length),
                });
            }
            if word_length < self.min_word_length
                || self.stopwords.contains(&word)
                || (!self.include_numbers
                    && word
                        .chars()
                        .filter(|character| !character.is_whitespace())
                        .all(|character| character.is_numeric()))
            {
                continue;
            }
            *counts.entry(word).or_default() += 1;
        }
        collect_frequencies(counts.into_iter().map(|(word, frequency)| WordFrequency {
            word,
            frequency: frequency as f64,
        }))
    }

    pub fn generate(&self, text: &str) -> Result<RgbaImage> {
        Ok(self.generate_detailed(text)?.into_image())
    }

    pub fn generate_dynamic(&self, text: &str) -> Result<DynamicImage> {
        Ok(DynamicImage::ImageRgba8(self.generate(text)?))
    }

    pub fn generate_detailed(&self, text: &str) -> Result<RenderedWordCloud> {
        let frequencies = self.word_frequencies(text)?;
        self.layout(frequencies)
    }

    pub fn generate_from_frequencies<I, T>(&self, input: I) -> Result<RgbaImage>
    where
        I: IntoIterator<Item = T>,
        T: IntoWordFrequency,
    {
        Ok(self.generate_detailed_from_frequencies(input)?.into_image())
    }

    pub fn generate_dynamic_from_frequencies<I, T>(&self, input: I) -> Result<DynamicImage>
    where
        I: IntoIterator<Item = T>,
        T: IntoWordFrequency,
    {
        Ok(DynamicImage::ImageRgba8(
            self.generate_from_frequencies(input)?,
        ))
    }

    pub fn generate_detailed_from_frequencies<I, T>(&self, input: I) -> Result<RenderedWordCloud>
    where
        I: IntoIterator<Item = T>,
        T: IntoWordFrequency,
    {
        self.layout(collect_frequencies(input)?)
    }

    pub fn save(&self, text: &str, path: impl AsRef<Path>) -> Result<()> {
        self.generate_detailed(text)?.save(path)
    }

    pub fn save_from_frequencies<I, T>(&self, input: I, path: impl AsRef<Path>) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        T: IntoWordFrequency,
    {
        self.generate_detailed_from_frequencies(input)?.save(path)
    }

    fn layout(&self, frequencies: Vec<WordFrequency>) -> Result<RenderedWordCloud> {
        for frequency in &frequencies {
            if frequency.word.chars().count() > self.max_word_length {
                return Err(WordCloudError::InvalidWord {
                    word: frequency.word.clone(),
                    reason: format!("exceeds max_word_length ({})", self.max_word_length),
                });
            }
        }
        let renderable: Vec<_> = frequencies
            .into_iter()
            .enumerate()
            .filter(|(_, frequency)| supports_word(&self.font, &frequency.word))
            .take(self.max_words)
            .collect();
        if renderable.is_empty() {
            return Err(WordCloudError::NoRenderableWords);
        }

        let max_frequency = renderable[0].1.frequency;
        let attempted = renderable.len();
        let mut occupied = self.initial_occupancy();
        let mut text_rasterizer = TextRasterizer::new();
        let mut layout_rng = StableRng::new(self.random_seed ^ 0x4c41_594f_5554_5f31);
        let mut color_rng = StableRng::new(self.random_seed ^ 0x434f_4c4f_5253_5f31);
        let mut placed = Vec::<PlacedWord>::new();
        let mut image = None;
        let mut previous: Option<(f64, f32)> = None;

        for (rank, frequency) in renderable {
            let normalized_frequency = (frequency.frequency / max_frequency) as f32;
            let target_size = previous
                .map(|(previous_frequency, previous_size)| {
                    let ratio = (frequency.frequency / previous_frequency) as f32;
                    previous_size * ((1.0 - self.relative_scaling) + self.relative_scaling * ratio)
                })
                .unwrap_or(self.max_font_size)
                .clamp(self.min_font_size, self.max_font_size);
            let (orientations, orientation_count) = self.orientation_order(&mut layout_rng);
            let mut font_size = target_size.floor().max(self.min_font_size);
            let mut selected = None;

            loop {
                let usable_width = self.width - self.margin * 2;
                let usable_height = self.height - self.margin * 2;
                if let Some(horizontal) = text_rasterizer.rasterize_word(
                    &self.font,
                    &frequency.word,
                    font_size,
                    usable_width,
                    usable_height,
                ) {
                    let mut horizontal = Some(horizontal);
                    for orientation in orientations[..orientation_count].iter().copied() {
                        match orientation {
                            Orientation::Horizontal => {
                                let bitmap = horizontal
                                    .as_ref()
                                    .expect("horizontal bitmap must be available");
                                if let Some((x, y, ink)) = find_bitmap_position(
                                    &occupied,
                                    bitmap,
                                    self.margin,
                                    self.width,
                                    self.height,
                                    self.search_attempts,
                                    &mut layout_rng,
                                ) {
                                    selected = Some((
                                        horizontal
                                            .take()
                                            .expect("selected horizontal bitmap must be available"),
                                        ink,
                                        orientation,
                                        x,
                                        y,
                                    ));
                                    break;
                                }
                            }
                            Orientation::Vertical => {
                                let horizontal_bitmap = horizontal
                                    .as_ref()
                                    .expect("horizontal bitmap must be available");
                                if !bitmap_fits_with_margin(
                                    horizontal_bitmap.height,
                                    horizontal_bitmap.width,
                                    self.margin,
                                    self.width,
                                    self.height,
                                ) {
                                    continue;
                                }
                                let bitmap = horizontal_bitmap.rotate_clockwise();
                                if let Some((x, y, ink)) = find_bitmap_position(
                                    &occupied,
                                    &bitmap,
                                    self.margin,
                                    self.width,
                                    self.height,
                                    self.search_attempts,
                                    &mut layout_rng,
                                ) {
                                    selected = Some((bitmap, ink, orientation, x, y));
                                    break;
                                }
                            }
                        }
                    }
                }
                if selected.is_some() || font_size <= self.min_font_size {
                    break;
                }
                font_size = next_font_size(font_size, self.font_step, self.min_font_size);
            }

            let Some((bitmap, ink, orientation, x, y)) = selected else {
                continue;
            };
            if !occupied.insert(&ink, x, y) {
                continue;
            }
            let random = color_rng.unit_f32();
            let color = self.colorizer.color(&ColorContext {
                word: &frequency.word,
                frequency: frequency.frequency,
                normalized_frequency,
                rank,
                font_size,
                orientation,
                x,
                y,
                random,
            });
            let metadata = PlacedWord {
                word: frequency.word,
                frequency: frequency.frequency,
                normalized_frequency,
                rank,
                x,
                y,
                width: bitmap.width,
                height: bitmap.height,
                font_size,
                orientation,
                color,
            };
            let image = image.get_or_insert_with(|| {
                RgbaImage::from_pixel(self.width, self.height, self.background_color)
            });
            blend_bitmap(image, &bitmap, &metadata);
            placed.push(metadata);
            previous = Some((frequency.frequency, font_size));
        }

        if placed.is_empty() {
            return Err(WordCloudError::NoWordsPlaced { attempted });
        }

        Ok(RenderedWordCloud {
            image: image.expect("a non-empty placement must initialize the image"),
            words: placed,
        })
    }

    fn initial_occupancy(&self) -> BitGrid {
        self.mask_occupancy
            .clone()
            .unwrap_or_else(|| BitGrid::new(self.width, self.height))
    }

    fn orientation_order(&self, rng: &mut StableRng) -> ([Orientation; 2], usize) {
        if self.prefer_horizontal >= 1.0 {
            ([Orientation::Horizontal, Orientation::Vertical], 1)
        } else if self.prefer_horizontal <= 0.0 {
            ([Orientation::Vertical, Orientation::Horizontal], 1)
        } else if rng.unit_f32() < self.prefer_horizontal {
            ([Orientation::Horizontal, Orientation::Vertical], 2)
        } else {
            ([Orientation::Vertical, Orientation::Horizontal], 2)
        }
    }
}

fn load_font(source: &FontSource) -> Result<Font> {
    let (bytes, index): (Arc<[u8]>, u32) = match source {
        FontSource::Embedded { index } => (Arc::from(DEFAULT_FONT), *index),
        FontSource::Path { path, index } => {
            let bytes = fs::read(path).map_err(|source| WordCloudError::FontRead {
                path: path.clone(),
                source,
            })?;
            (Arc::from(bytes), *index)
        }
        FontSource::Bytes { data, index } => (Arc::clone(data), *index),
    };
    Font::from_bytes(
        bytes,
        FontSettings {
            collection_index: index,
            ..FontSettings::default()
        },
    )
    .map_err(|reason| WordCloudError::InvalidFont {
        reason: reason.to_owned(),
    })
}

fn validate_positive_u32(parameter: &'static str, value: u32) -> Result<()> {
    if value == 0 {
        Err(WordCloudError::invalid(
            parameter,
            "must be greater than zero",
        ))
    } else {
        Ok(())
    }
}

fn validate_positive_f32(parameter: &'static str, value: f32) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        Err(WordCloudError::invalid(
            parameter,
            "must be finite and greater than zero",
        ))
    } else {
        Ok(())
    }
}

fn validate_probability(parameter: &'static str, value: f32) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        Err(WordCloudError::invalid(
            parameter,
            "must be between 0 and 1 inclusive",
        ))
    } else {
        Ok(())
    }
}

fn scale_font_size(parameter: &'static str, value: f32, scale: u32) -> Result<f32> {
    let scaled = value * scale as f32;
    if !scaled.is_finite() || scaled <= 0.0 {
        Err(WordCloudError::invalid(
            parameter,
            "overflows after applying scale",
        ))
    } else {
        Ok(scaled)
    }
}

fn next_font_size(current: f32, step: f32, minimum: f32) -> f32 {
    let next = current - step;
    if !next.is_finite() || next >= current || next < minimum {
        minimum
    } else {
        next
    }
}

fn build_mask_occupancy(mask: &Mask, width: u32, height: u32, scale: u32) -> BitGrid {
    debug_assert_eq!(width, mask.width() * scale);
    debug_assert_eq!(height, mask.height() * scale);
    let mut occupied = BitGrid::new(width, height);
    for mask_y in 0..mask.height() {
        for mask_x in 0..mask.width() {
            if mask.allowed_pixels()[(mask_y as usize) * (mask.width() as usize) + mask_x as usize]
            {
                continue;
            }
            let start_x = mask_x * scale;
            let start_y = mask_y * scale;
            for y in start_y..start_y + scale {
                for x in start_x..start_x + scale {
                    occupied.set(x, y);
                }
            }
        }
    }
    occupied
}

fn bitmap_fits_with_margin(
    width: u32,
    height: u32,
    margin: u32,
    canvas_width: u32,
    canvas_height: u32,
) -> bool {
    let Some(padding) = margin.checked_mul(2) else {
        return false;
    };
    width
        .checked_add(padding)
        .is_some_and(|width| width <= canvas_width)
        && height
            .checked_add(padding)
            .is_some_and(|height| height <= canvas_height)
}

fn find_bitmap_position(
    occupied: &BitGrid,
    bitmap: &AlphaBitmap,
    margin: u32,
    canvas_width: u32,
    canvas_height: u32,
    attempts: usize,
    rng: &mut StableRng,
) -> Option<(u32, u32, BitGrid)> {
    if !bitmap_fits_with_margin(
        bitmap.width,
        bitmap.height,
        margin,
        canvas_width,
        canvas_height,
    ) {
        return None;
    }
    let (ink, collision) = bitmap.placement_bits(margin)?;
    let (collision_x, collision_y) = find_position(occupied, &collision, attempts, rng)?;
    let ink = ink.unwrap_or_else(|| bitmap.bits());
    Some((collision_x + margin, collision_y + margin, ink))
}

fn find_position(
    occupied: &BitGrid,
    candidate: &BitGrid,
    attempts: usize,
    rng: &mut StableRng,
) -> Option<(u32, u32)> {
    let max_x = occupied.width().checked_sub(candidate.width())?;
    let max_y = occupied.height().checked_sub(candidate.height())?;
    let center = (max_x / 2, max_y / 2);
    if position_is_free(occupied, candidate, center.0, center.1) {
        return Some(center);
    }

    let biased_attempts = attempts.min(96);
    for _ in 0..biased_attempts {
        let x = ((u64::from(rng.range_inclusive(max_x)) + u64::from(rng.range_inclusive(max_x)))
            / 2) as u32;
        let y = ((u64::from(rng.range_inclusive(max_y)) + u64::from(rng.range_inclusive(max_y)))
            / 2) as u32;
        if position_is_free(occupied, candidate, x, y) {
            return Some((x, y));
        }
    }

    let positions_per_row = u64::from(max_x) + 1;
    let total = positions_per_row * (u64::from(max_y) + 1);
    let checks = (attempts - biased_attempts).min(total as usize);
    if checks == 0 {
        return None;
    }
    let start = rng.next_u64() % total;
    let mut step = (rng.next_u64() % total).max(1);
    while gcd(step, total) != 1 {
        step = if step + 1 == total { 1 } else { step + 1 };
    }
    let mut index = start;
    for _ in 0..checks {
        let x = (index % positions_per_row) as u32;
        let y = (index / positions_per_row) as u32;
        if position_is_free(occupied, candidate, x, y) {
            return Some((x, y));
        }
        index = (index + step) % total;
    }
    None
}

fn position_is_free(occupied: &BitGrid, candidate: &BitGrid, x: u32, y: u32) -> bool {
    !occupied.collides_in_bounds(candidate, x, y)
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn blend_bitmap(image: &mut RgbaImage, bitmap: &AlphaBitmap, word: &PlacedWord) {
    let image_width = image.width() as usize;
    let bitmap_width = bitmap.width as usize;
    let word_x = word.x as usize;
    let word_y = word.y as usize;
    let image_data = image.as_mut();
    for y in 0..bitmap.height as usize {
        let alpha_start = y * bitmap_width;
        let alpha_row = &bitmap.alpha[alpha_start..alpha_start + bitmap_width];
        let target_start = ((word_y + y) * image_width + word_x) * 4;
        let target_row = &mut image_data[target_start..target_start + bitmap_width * 4];
        for (coverage, target) in alpha_row
            .iter()
            .copied()
            .zip(target_row.chunks_exact_mut(4))
        {
            if coverage == 0 {
                continue;
            }
            let mut source = word.color;
            source[3] = ((u16::from(source[3]) * u16::from(coverage) + 127) / 255) as u8;
            Rgba::from_slice_mut(target).blend(&source);
        }
    }
}

/// A small fixed algorithm RNG. Its output is stable across dependency updates.
struct StableRng {
    state: u64,
}

impl StableRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn unit_f32(&mut self) -> f32 {
        let mantissa = (self.next_u64() >> 40) as u32;
        mantissa as f32 / 16_777_216.0
    }

    fn range_inclusive(&mut self, maximum: u32) -> u32 {
        if maximum == 0 {
            0
        } else {
            (self.next_u64() % (u64::from(maximum) + 1)) as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_rng_has_expected_first_values() {
        let mut rng = StableRng::new(42);
        assert_eq!(rng.next_u64(), 13_679_457_532_755_275_413);
        assert_eq!(rng.next_u64(), 2_949_826_092_126_892_291);
    }

    #[test]
    fn source_over_preserves_transparency() {
        let bitmap = AlphaBitmap {
            width: 1,
            height: 1,
            alpha: vec![128],
        };
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let word = PlacedWord {
            word: "x".to_owned(),
            frequency: 1.0,
            normalized_frequency: 1.0,
            rank: 0,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            font_size: 10.0,
            orientation: Orientation::Horizontal,
            color: Rgba([200, 100, 50, 255]),
        };
        blend_bitmap(&mut image, &bitmap, &word);
        assert_eq!(image[(0, 0)], Rgba([200, 100, 50, 128]));
    }

    #[test]
    fn blend_bitmap_matches_image_blend_for_alpha_matrix() {
        let destinations = [
            Rgba([7, 31, 83, 0]),
            Rgba([17, 29, 43, 73]),
            Rgba([241, 243, 247, 255]),
        ];
        for coverage in [0, 1, 127, 128, 254, 255] {
            for word_alpha in [0, 1, 128, 254, 255] {
                for destination in destinations {
                    let bitmap = AlphaBitmap {
                        width: 1,
                        height: 1,
                        alpha: vec![coverage],
                    };
                    let word = PlacedWord {
                        word: "x".to_owned(),
                        frequency: 1.0,
                        normalized_frequency: 1.0,
                        rank: 0,
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                        font_size: 10.0,
                        orientation: Orientation::Horizontal,
                        color: Rgba([193, 97, 41, word_alpha]),
                    };
                    let mut expected = destination;
                    let mut source = word.color;
                    source[3] = ((u16::from(word_alpha) * u16::from(coverage) + 127) / 255) as u8;
                    expected.blend(&source);

                    let mut image = RgbaImage::from_pixel(1, 1, destination);
                    blend_bitmap(&mut image, &bitmap, &word);
                    assert_eq!(image[(0, 0)], expected);
                }
            }
        }
    }

    #[test]
    fn cached_mask_occupancy_matches_pixel_mapping() {
        for logical_width in [63, 64, 65, 127] {
            let logical_height = 7;
            let mask = Mask::from_predicate(logical_width, logical_height, |x, y| {
                (x.wrapping_mul(17) + y * 11) % 13 > 3
            })
            .unwrap();
            for scale in [1, 2, 3] {
                let width = logical_width * scale;
                let height = logical_height * scale;
                let occupied = build_mask_occupancy(&mask, width, height, scale);
                for y in 0..height {
                    for x in 0..width {
                        assert_eq!(occupied.get(x, y), !mask.is_allowed(x / scale, y / scale));
                    }
                }
            }
        }
    }

    #[test]
    fn cached_mask_is_not_mutated_between_generations() {
        let mask = Mask::from_predicate(127, 83, |x, y| {
            x > 2 && y > 1 && x < 124 && y < 81 && !((47..80).contains(&x) && y < 27)
        })
        .unwrap();
        let cloud = WordCloud::builder()
            .mask(mask)
            .scale(2)
            .random_seed(77)
            .build()
            .unwrap();
        let frequencies = [("mask", 10), ("cached", 8), ("layout", 6), ("pixels", 4)];
        let first = cloud
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        let second = cloud
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        assert_eq!(first.words(), second.words());
        assert_eq!(first.image().as_raw(), second.image().as_raw());
    }

    #[test]
    fn generated_ink_is_in_bounds_and_never_overlaps() {
        let cloud = WordCloud::builder()
            .dimensions(520, 300)
            .max_font_size(76.0)
            .prefer_horizontal(0.65)
            .random_seed(91)
            .build()
            .unwrap();
        let rendered = cloud
            .generate_detailed_from_frequencies([
                ("rust", 30),
                ("safety", 24),
                ("performance", 20),
                ("ownership", 17),
                ("borrowing", 15),
                ("concurrency", 13),
                ("reliable", 11),
                ("productive", 9),
                ("tooling", 7),
                ("systems", 5),
            ])
            .unwrap();
        let mut occupied = BitGrid::new(cloud.width, cloud.height);
        let usable_width = cloud.width - cloud.margin * 2;
        let usable_height = cloud.height - cloud.margin * 2;
        let mut text_rasterizer = TextRasterizer::new();

        for word in rendered.words() {
            let horizontal = text_rasterizer
                .rasterize_word(
                    &cloud.font,
                    &word.word,
                    word.font_size,
                    usable_width,
                    usable_height,
                )
                .unwrap();
            let bitmap = match word.orientation {
                Orientation::Horizontal => horizontal,
                Orientation::Vertical => horizontal.rotate_clockwise(),
            };
            assert_eq!((bitmap.width, bitmap.height), (word.width, word.height));
            assert!(word.x + word.width <= cloud.width);
            assert!(word.y + word.height <= cloud.height);
            let bits = bitmap.bits();
            assert!(!occupied.collides(&bits, word.x, word.y));
            assert!(occupied.insert(&bits, word.x, word.y));
        }
    }
}
