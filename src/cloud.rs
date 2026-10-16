use std::cmp::Ordering;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use ab_glyph::{CodepointIdIter, Font, FontArc, FontRef, FontVec, GlyphId, InvalidFont, Outline};
use image::{DynamicImage, ImageFormat, Pixel, Rgba, RgbaImage};

use crate::bitmap::{AlphaBitmap, BitGrid};
use crate::color::{ColorContext, Colorizer, Palette, SolidColor};
use crate::frequency::{
    collect_frequencies, process_tokens, sort_frequencies, unigrams_and_bigrams, IntoWordFrequency,
    WordFrequency,
};
use crate::mask::Mask;
use crate::text::{supports_word, TextRasterizer};
use crate::tokenizer::{DefaultTokenizer, StopWords, Tokenizer};
use crate::{Result, WordCloudError, MAX_CANVAS_PIXELS};

const DEFAULT_FONT: &[u8] = include_bytes!("../assets/OpenSans-Regular.ttf");
const DEFAULT_WIDTH: u32 = 400;
const DEFAULT_HEIGHT: u32 = 200;
const MAX_WORD_LENGTH_LIMIT: usize = 4_096;
/// Default Dunning log-likelihood threshold for bigram collocations, from
/// the Python package.
const DEFAULT_COLLOCATION_THRESHOLD: f64 = 30.0;
static DEFAULT_SEED_BASE: OnceLock<u64> = OnceLock::new();
static DEFAULT_SEED_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn default_random_seed() -> u64 {
    let base = *DEFAULT_SEED_BASE.get_or_init(|| {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        timestamp ^ u64::from(std::process::id()).rotate_left(32)
    });
    let sequence = DEFAULT_SEED_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    let mut value = base.wrapping_add(sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

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
    /// Zero-based position in the descending-frequency input list. Words
    /// dropped before placement (stopwords, unsupported glyphs) leave gaps,
    /// and `repeat` duplicates keep the rank of the word they repeat.
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
                // The JPEG encoder consumes the image one 8x8 block at a
                // time and drops alpha while converting each pixel to
                // YCbCr, so encoding straight from the RGBA buffer avoids
                // materializing a full-size RGB copy.
                let file = fs::File::create(path).map_err(image::ImageError::IoError)?;
                let mut writer = io::BufWriter::new(file);
                let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut writer);
                encoder.encode_image(&self.image)?;
                // Dropping a `BufWriter` ignores flush errors, which would
                // silently leave a truncated file behind on a full disk.
                writer.flush().map_err(image::ImageError::IoError)?;
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
    repeat: bool,
    min_font_size: f32,
    max_font_size: Option<f32>,
    font_step: f32,
    margin: u32,
    prefer_horizontal: f32,
    /// `None` selects the Python `relative_scaling="auto"` rule.
    relative_scaling: Option<f32>,
    random_seed: Option<u64>,
    background_color: Rgba<u8>,
    font_source: FontSource,
    mask: Option<Mask>,
    tokenizer: Arc<dyn Tokenizer>,
    stopwords: StopWords,
    lowercase: bool,
    include_numbers: bool,
    min_word_length: usize,
    max_word_length: usize,
    collocations: bool,
    collocation_threshold: f64,
    normalize_plurals: bool,
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
            repeat: false,
            min_font_size: 4.0,
            max_font_size: None,
            font_step: 1.0,
            margin: 2,
            prefer_horizontal: 0.9,
            relative_scaling: None,
            random_seed: None,
            background_color: Rgba([0, 0, 0, 255]),
            font_source: FontSource::default(),
            mask: None,
            tokenizer: Arc::new(DefaultTokenizer),
            stopwords: StopWords::default(),
            lowercase: false,
            include_numbers: false,
            min_word_length: 0,
            max_word_length: 256,
            collocations: true,
            collocation_threshold: DEFAULT_COLLOCATION_THRESHOLD,
            normalize_plurals: true,
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

    /// When fewer than `max_words` words are available, appends the existing
    /// words again with reduced weights until `max_words` candidates exist.
    ///
    /// Each repetition round multiplies the original frequencies by the
    /// smallest normalized frequency (minimum divided by maximum), matching
    /// the `repeat` option of the Python `wordcloud` package. Repeated words
    /// therefore render progressively smaller, and each duplicate keeps the
    /// rank of the word it repeats. Disabled by default.
    pub fn repeat(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
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

    /// Restores the Python auto-fit behavior: the largest font size is
    /// derived per generation by probing the top two words at canvas height.
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

    /// Sets how strongly frequency ratios drive font sizes.
    ///
    /// Without an explicit value the Python `"auto"` rule applies: `0.5`
    /// normally and `0.0` when `repeat` is enabled.
    pub fn relative_scaling(mut self, relative_scaling: f32) -> Self {
        self.relative_scaling = Some(relative_scaling);
        self
    }

    /// Restores the Python `relative_scaling="auto"` behavior.
    pub fn auto_relative_scaling(mut self) -> Self {
        self.relative_scaling = None;
        self
    }

    /// Uses a fixed seed, making repeated generations with the same input reproducible.
    ///
    /// Without this option, every generation uses a fresh seed.
    pub fn random_seed(mut self, seed: u64) -> Self {
        self.random_seed = Some(seed);
        self
    }

    /// Restores the default behavior of using a fresh seed for every generation.
    pub fn randomize_each_generation(mut self) -> Self {
        self.random_seed = None;
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

    /// Lowercases every token before counting, so `"Rust"` and `"rust"`
    /// always merge as `"rust"`. Disabled by default; without it, words are
    /// represented by their most frequent capitalization, like the Python
    /// package.
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

    /// Detects two-word collocations (bigrams) among adjacent tokens and
    /// promotes statistically significant pairs to single words, like the
    /// Python package's `collocations` option. Enabled by default; only
    /// affects natural-text input.
    pub fn collocations(mut self, collocations: bool) -> Self {
        self.collocations = collocations;
        self
    }

    /// Minimum Dunning log-likelihood score for a bigram to count as a
    /// collocation. Defaults to 30, the Python package's value.
    pub fn collocation_threshold(mut self, threshold: f64) -> Self {
        self.collocation_threshold = threshold;
        self
    }

    /// Merges trailing-`s` plurals into their singular when both forms
    /// appear (except words ending in `ss`), like the Python package's
    /// `normalize_plurals` option. Enabled by default; only affects
    /// natural-text input.
    pub fn normalize_plurals(mut self, normalize: bool) -> Self {
        self.normalize_plurals = normalize;
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
        if let Some(relative_scaling) = self.relative_scaling {
            validate_probability("relative_scaling", relative_scaling)?;
        }
        if self.min_word_length > self.max_word_length
            || self.max_word_length > MAX_WORD_LENGTH_LIMIT
        {
            return Err(WordCloudError::invalid(
                "max_word_length",
                format!("must be between min_word_length and {MAX_WORD_LENGTH_LIMIT}"),
            ));
        }
        if !self.collocation_threshold.is_finite() || self.collocation_threshold < 0.0 {
            return Err(WordCloudError::invalid(
                "collocation_threshold",
                "must be finite and non-negative",
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
        if pixels > MAX_CANVAS_PIXELS {
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
        let min_font_size = scale_font_size("min_font_size", self.min_font_size, self.scale)?;
        let canvas_font_limit = width.max(height) as f32;
        if min_font_size > canvas_font_limit {
            return Err(WordCloudError::invalid(
                "min_font_size",
                "must not exceed the longest canvas dimension",
            ));
        }
        // `None` keeps the Python auto-fit: the maximum is derived per
        // generation by probing the top two words at canvas height.
        let max_font_size = match self.max_font_size {
            Some(maximum) => {
                let scaled = scale_font_size("max_font_size", maximum, self.scale)?;
                let clamped = scaled.min(canvas_font_limit);
                if clamped > min_font_size && clamped - self.font_step >= clamped {
                    return Err(WordCloudError::invalid(
                        "font_step",
                        "is too small to reduce max_font_size at f32 precision",
                    ));
                }
                Some(clamped)
            }
            None => None,
        };
        let font_step = scale_font_size("font_step", self.font_step, self.scale)?;
        let relative_scaling = self
            .relative_scaling
            .unwrap_or(if self.repeat { 0.0 } else { 0.5 });
        let mask_blocked = self
            .mask
            .as_ref()
            .map(|mask| Arc::new(build_mask_blocked(mask, width, height, self.scale)));

        Ok(WordCloud {
            width,
            height,
            scale: self.scale,
            max_words: self.max_words,
            repeat: self.repeat,
            min_font_size,
            max_font_size,
            font_step,
            margin,
            prefer_horizontal: self.prefer_horizontal,
            relative_scaling,
            random_seed: self.random_seed,
            background_color: self.background_color,
            font,
            mask_blocked,
            tokenizer: self.tokenizer,
            stopwords: self.stopwords,
            lowercase: self.lowercase,
            include_numbers: self.include_numbers,
            min_word_length: self.min_word_length,
            max_word_length: self.max_word_length,
            collocations: self.collocations,
            collocation_threshold: self.collocation_threshold,
            normalize_plurals: self.normalize_plurals,
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
    repeat: bool,
    min_font_size: f32,
    /// `None` triggers the per-generation auto-fit probe.
    max_font_size: Option<f32>,
    font_step: f32,
    margin: u32,
    prefer_horizontal: f32,
    relative_scaling: f32,
    random_seed: Option<u64>,
    background_color: Rgba<u8>,
    font: FontArc,
    /// Pixels the mask forbids. Shared immutably across [`WordCloud`] clones
    /// (cloning the generator no longer deep-copies the grid); each
    /// generation works on its own merged copy for fast collision probes.
    mask_blocked: Option<Arc<BitGrid>>,
    tokenizer: Arc<dyn Tokenizer>,
    stopwords: StopWords,
    lowercase: bool,
    include_numbers: bool,
    min_word_length: usize,
    max_word_length: usize,
    collocations: bool,
    collocation_threshold: f64,
    normalize_plurals: bool,
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
        let mut words: Vec<String> = Vec::new();
        let mut invalid = None;
        self.tokenizer.for_each_token(text, &mut |candidate| {
            if invalid.is_some() {
                return;
            }
            let candidate = candidate.trim();
            if candidate.is_empty() {
                return;
            }
            // Trailing "'s" is stripped before any other filter, like
            // process_text of the Python package.
            let word = strip_possessive(candidate);
            if !self.include_numbers && word.chars().all(|character| character.is_numeric()) {
                return;
            }
            let word_length = word.chars().count();
            if word_length < self.min_word_length {
                return;
            }
            if word_length > self.max_word_length {
                invalid = Some(WordCloudError::InvalidWord {
                    word: word.to_owned(),
                    reason: format!("exceeds max_word_length ({})", self.max_word_length),
                });
                return;
            }
            // Tokens are kept in order, including repeats: bigram
            // collocations are built from adjacent tokens.
            words.push(if self.lowercase {
                word.to_lowercase()
            } else {
                word.to_owned()
            });
        });
        if let Some(error) = invalid {
            return Err(error);
        }
        let counts = if self.collocations {
            unigrams_and_bigrams(
                words,
                &self.stopwords,
                self.normalize_plurals,
                self.collocation_threshold,
            )
        } else {
            let filtered: Vec<String> = words
                .iter()
                .filter(|word| !self.is_stopword(word))
                .cloned()
                .collect();
            process_tokens(&filtered, self.normalize_plurals).counts
        };
        if counts.is_empty() {
            return Err(WordCloudError::EmptyInput);
        }
        let mut words: Vec<_> = counts
            .into_iter()
            .map(|(word, count)| WordFrequency {
                word,
                frequency: count as f64,
            })
            .collect();
        sort_frequencies(&mut words);
        Ok(words)
    }

    /// Checks the stop-word list without re-lowercasing words that the
    /// tokenizer path already lowercased.
    fn is_stopword(&self, word: &str) -> bool {
        if self.lowercase {
            self.stopwords.contains_lowercased(word)
        } else {
            self.stopwords.contains(word)
        }
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
        let renderable = if self.repeat {
            extend_with_repeats(renderable, self.max_words)
        } else {
            renderable
        };

        let max_frequency = renderable[0].1.frequency;
        let attempted = renderable.len();
        let random_seed = self.random_seed.unwrap_or_else(default_random_seed);
        let mut layout_rng = StableRng::new(random_seed ^ 0x4c41_594f_5554_5f31);
        let mut color_rng = StableRng::new(random_seed ^ 0x434f_4c4f_5253_5f31);
        let mut text_rasterizer = TextRasterizer::new();

        // Python auto-fit: with a single word, go as big as the canvas
        // height; otherwise place the top two words at canvas height and
        // start from the harmonic mean of their fitted sizes.
        let start_font_size = match self.max_font_size {
            Some(max) => max,
            None if renderable.len() == 1 => self.height as f32,
            None => {
                let (sizes, _, _) = self.place_words(
                    &renderable[..2],
                    self.height as f32,
                    max_frequency,
                    &mut layout_rng,
                    &mut color_rng,
                    &mut text_rasterizer,
                    false,
                )?;
                match (sizes.first(), sizes.get(1)) {
                    (Some(&first), Some(&second)) => {
                        ((2.0 * f64::from(first) * f64::from(second)) / f64::from(first + second))
                            as f32
                    }
                    (Some(&only), None) => only,
                    (None, _) => {
                        return Err(WordCloudError::NoWordsPlaced { attempted });
                    }
                }
            }
        };

        let (_, placed, image) = self.place_words(
            &renderable,
            start_font_size,
            max_frequency,
            &mut layout_rng,
            &mut color_rng,
            &mut text_rasterizer,
            true,
        )?;

        if placed.is_empty() {
            return Err(WordCloudError::NoWordsPlaced { attempted });
        }

        Ok(RenderedWordCloud {
            image: image.expect("a non-empty placement must initialize the image"),
            words: placed,
        })
    }

    /// Runs the placement ladder over `words`, starting every generation at
    /// `start_font_size`. Returns the placed font sizes in order, the
    /// placement metadata (empty when `collect` is false, used for the
    /// auto-fit probe), and the blended image when `collect` is true.
    #[allow(clippy::too_many_arguments)]
    fn place_words(
        &self,
        words: &[(usize, WordFrequency)],
        start_font_size: f32,
        max_frequency: f64,
        layout_rng: &mut StableRng,
        color_rng: &mut StableRng,
        text_rasterizer: &mut TextRasterizer,
        collect: bool,
    ) -> Result<(Vec<f32>, Vec<PlacedWord>, Option<RgbaImage>)> {
        let clearance = self.margin / 2;
        let mut font_size = start_font_size;
        // The scaling formula works on the ratio to the last placed word's
        // frequency, normalized like the Python package.
        let mut last_frequency = 1.0_f64;
        let mut placed_sizes = Vec::new();
        let mut placed = Vec::new();
        let mut image: Option<RgbaImage> = None;
        let mut occupied = self.initial_occupancy();

        'words: for &(rank, ref frequency) in words {
            let normalized = frequency.frequency / max_frequency;
            if self.relative_scaling != 0.0 {
                font_size = round_half_even(
                    (f64::from(self.relative_scaling) * (normalized / last_frequency)
                        + f64::from(1.0 - self.relative_scaling))
                        * f64::from(font_size),
                ) as f32;
            }

            // The orientation roll is drawn before the size ladder, matching
            // the reference implementation's RNG stream shape.
            let mut orientation = if layout_rng.unit_f32() < self.prefer_horizontal {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            };
            let mut tried_other_orientation = false;
            let mut selected = None;

            loop {
                if font_size < self.min_font_size {
                    break;
                }
                if let Some(ink) = text_rasterizer.word_ink(
                    &self.font,
                    &frequency.word,
                    font_size,
                    self.width,
                    self.height,
                ) {
                    let (window_width, window_height) = match orientation {
                        Orientation::Horizontal => {
                            (ink.width() + 2 * clearance, ink.height() + 2 * clearance)
                        }
                        Orientation::Vertical => {
                            (ink.height() + 2 * clearance, ink.width() + 2 * clearance)
                        }
                    };
                    if let Some((x, y)) =
                        sample_position(&occupied, window_width, window_height, layout_rng)
                    {
                        let x = x + clearance;
                        let y = y + clearance;
                        let ink = match orientation {
                            Orientation::Horizontal => ink,
                            Orientation::Vertical => ink.rotate_clockwise(),
                        };
                        let inserted = occupied.insert(&ink, x, y);
                        debug_assert!(inserted);
                        selected = Some((orientation, x, y, font_size));
                        break;
                    }
                }
                if !tried_other_orientation && self.prefer_horizontal < 1.0 {
                    tried_other_orientation = true;
                    if orientation != Orientation::Vertical {
                        orientation = Orientation::Vertical;
                        // Retry the same font size, rotated.
                        continue;
                    }
                    // The reference retries the identical vertical query
                    // here; skipping the duplicate is observationally
                    // equivalent.
                }
                font_size -= self.font_step;
                orientation = Orientation::Horizontal;
            }

            let Some((orientation, x, y, placed_size)) = selected else {
                // Like the Python package, stop placing entirely once a word
                // no longer fits at the minimum font size.
                break 'words;
            };

            placed_sizes.push(placed_size);
            last_frequency = normalized;
            if !collect {
                continue;
            }
            let bitmap = match orientation {
                Orientation::Horizontal => text_rasterizer.rasterize_word(
                    &self.font,
                    &frequency.word,
                    placed_size,
                    self.width,
                    self.height,
                ),
                Orientation::Vertical => text_rasterizer.rasterize_word_rotated(
                    &self.font,
                    &frequency.word,
                    placed_size,
                    self.width,
                    self.height,
                ),
            };
            let Some(bitmap) = bitmap else {
                continue;
            };
            let random = color_rng.unit_f32();
            let color = self.colorizer.color(&ColorContext {
                word: &frequency.word,
                frequency: frequency.frequency,
                normalized_frequency: normalized as f32,
                rank,
                font_size: placed_size,
                orientation,
                x,
                y,
                random,
            });
            let metadata = PlacedWord {
                word: frequency.word.clone(),
                frequency: frequency.frequency,
                normalized_frequency: normalized as f32,
                rank,
                x,
                y,
                width: bitmap.width,
                height: bitmap.height,
                font_size: placed_size,
                orientation,
                color,
            };
            let image = image.get_or_insert_with(|| {
                RgbaImage::from_pixel(self.width, self.height, self.background_color)
            });
            blend_bitmap(image, &bitmap, &metadata);
            placed.push(metadata);
        }

        Ok((placed_sizes, placed, image))
    }

    fn initial_occupancy(&self) -> BitGrid {
        // One memcpy per generation keeps collision probes single-grid: a
        // separate immutable mask check measurably doubles the placement
        // search time, far outweighing this transient copy.
        self.mask_blocked
            .as_deref()
            .cloned()
            .unwrap_or_else(|| BitGrid::new(self.width, self.height))
    }
}

/// Removes a trailing `"'s"` possessive, like `process_text` of the Python
/// package. The check is case-insensitive; the stripped input keeps its
/// original capitalization.
fn strip_possessive(word: &str) -> &str {
    if word.len() >= 2 {
        let bytes = word.as_bytes();
        // Only ASCII `s`/`S` lowercases to `s`, so matching bytes directly
        // is exact and keeps the split below on a char boundary.
        let last_is_s = matches!(bytes[word.len() - 1], b's' | b'S');
        let second_is_apostrophe = bytes[word.len() - 2] == b'\'';
        if last_is_s && second_is_apostrophe {
            return &word[..word.len() - 2];
        }
    }
    word
}

/// Rounds to the nearest integer, halves to even, like Python's `round`
/// used for font sizes.
fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    match (value - floor).partial_cmp(&0.5) {
        Some(Ordering::Less) => floor,
        Some(Ordering::Greater) => floor + 1.0,
        _ if (floor as i64) % 2 == 0 => floor,
        _ => floor + 1.0,
    }
}

/// Picks a uniformly random position where a `width` x `height` window fits
/// entirely in free space, mirroring `query_integral_image` of the Python
/// package: every valid position is enumerated and one is chosen at random.
fn sample_position(
    occupied: &BitGrid,
    width: u32,
    height: u32,
    rng: &mut StableRng,
) -> Option<(u32, u32)> {
    let valid = occupied.free_windows(width, height)?;
    let total = valid.popcount();
    if total == 0 {
        return None;
    }
    // Modulo reduction is slightly non-uniform; matching the reference's
    // behavior does not warrant rejection sampling.
    let goal = rng.next_u64() % total;
    valid.select_set(goal)
}

/// A [`Font`] that borrows its bytes from an [`Arc<[u8]>`] it keeps alive,
/// avoiding the copy that [`FontVec`] would require.
///
/// # Soundness
///
/// `font` borrows the bytes owned by `data` with its lifetime extended to
/// `'static`. This is sound because:
/// - `data` is owned by this struct and is only dropped when the struct is
///   dropped, at which point `font` can no longer be reached through the
///   `Font` trait object;
/// - `Arc<[u8]>` is immutable (`Arc::get_mut` returns `None` while this struct
///   holds a reference), so the borrowed bytes can never be mutated or moved;
/// - [`FontRef`] has no `Drop` impl and no interior mutability, so the
///   self-referential layout is safe to move.
struct ArcBorrowedFont {
    // Kept alive for `font`'s borrow below.
    _data: Arc<[u8]>,
    font: FontRef<'static>,
}

impl ArcBorrowedFont {
    fn try_from_bytes(data: Arc<[u8]>, index: u32) -> std::result::Result<Self, InvalidFont> {
        // SAFETY: `font` borrows the bytes owned by `data`. `data` is moved
        // into the returned struct, so it stays alive for as long as the
        // struct does. See the struct-level soundness note.
        let font = unsafe {
            let static_bytes: &'static [u8] = &*(&*data as *const [u8]);
            FontRef::try_from_slice_and_index(static_bytes, index)?
        };
        Ok(Self { _data: data, font })
    }
}

impl Font for ArcBorrowedFont {
    fn units_per_em(&self) -> Option<f32> {
        self.font.units_per_em()
    }
    fn ascent_unscaled(&self) -> f32 {
        self.font.ascent_unscaled()
    }
    fn descent_unscaled(&self) -> f32 {
        self.font.descent_unscaled()
    }
    fn line_gap_unscaled(&self) -> f32 {
        self.font.line_gap_unscaled()
    }
    fn italic_angle(&self) -> f32 {
        self.font.italic_angle()
    }
    fn glyph_id(&self, c: char) -> GlyphId {
        self.font.glyph_id(c)
    }
    fn h_advance_unscaled(&self, id: GlyphId) -> f32 {
        self.font.h_advance_unscaled(id)
    }
    fn h_side_bearing_unscaled(&self, id: GlyphId) -> f32 {
        self.font.h_side_bearing_unscaled(id)
    }
    fn v_advance_unscaled(&self, id: GlyphId) -> f32 {
        self.font.v_advance_unscaled(id)
    }
    fn v_side_bearing_unscaled(&self, id: GlyphId) -> f32 {
        self.font.v_side_bearing_unscaled(id)
    }
    fn kern_unscaled(&self, first: GlyphId, second: GlyphId) -> f32 {
        self.font.kern_unscaled(first, second)
    }
    fn outline(&self, id: GlyphId) -> Option<Outline> {
        self.font.outline(id)
    }
    fn glyph_count(&self) -> usize {
        self.font.glyph_count()
    }
    fn codepoint_ids(&self) -> CodepointIdIter<'_> {
        self.font.codepoint_ids()
    }
    fn glyph_raster_image2(
        &self,
        id: GlyphId,
        pixel_size: u16,
    ) -> Option<ab_glyph::v2::GlyphImage<'_>> {
        self.font.glyph_raster_image2(id, pixel_size)
    }
}

fn load_font(source: &FontSource) -> Result<FontArc> {
    let font: FontArc = match source {
        // `DEFAULT_FONT` is a `'static` slice baked into the binary, so
        // `FontRef` borrows it directly instead of copying it into a `Vec`.
        FontSource::Embedded { index } => FontRef::try_from_slice_and_index(DEFAULT_FONT, *index)
            .map(FontArc::new)
            .map_err(|reason| WordCloudError::InvalidFont {
                reason: reason.to_string(),
            })?,
        // `fs::read` already produces an owned buffer; `FontVec` takes it
        // without an additional copy.
        FontSource::Path { path, index } => {
            let bytes = fs::read(path).map_err(|source| WordCloudError::FontRead {
                path: path.clone(),
                source,
            })?;
            FontVec::try_from_vec_and_index(bytes, *index)
                .map(FontArc::new)
                .map_err(|reason| WordCloudError::InvalidFont {
                    reason: reason.to_string(),
                })?
        }
        // `clone()` only bumps the `Arc` refcount: the font bytes are shared
        // with the `FontSource` (and the caller), never copied.
        FontSource::Bytes { data, index } => ArcBorrowedFont::try_from_bytes(data.clone(), *index)
            .map(FontArc::new)
            .map_err(|reason| WordCloudError::InvalidFont {
                reason: reason.to_string(),
            })?,
    };
    Ok(font)
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

/// Pads a renderable word list with downweighted copies of itself, mirroring
/// the `repeat` option of the Python `wordcloud` package. Each repetition
/// round multiplies the original frequencies by the smallest normalized
/// frequency (minimum divided by maximum) raised to the round number, so
/// repeated words render progressively smaller. The result keeps the original
/// ranks and is truncated to `max_words` entries.
fn extend_with_repeats(
    mut renderable: Vec<(usize, WordFrequency)>,
    max_words: usize,
) -> Vec<(usize, WordFrequency)> {
    let originals_len = renderable.len();
    if originals_len >= max_words {
        return renderable;
    }
    let max_frequency = renderable[0].1.frequency;
    let downweight = renderable[originals_len - 1].1.frequency / max_frequency;
    let rounds = max_words.div_ceil(originals_len) - 1;
    for round in 1..=rounds {
        // Only clone the words the final round actually keeps, so the last
        // round never allocates strings that `truncate` would discard.
        let count = (max_words - round * originals_len).min(originals_len);
        let scale = downweight.powi(round as i32);
        // Indexed access keeps the borrow short: `push` may reallocate the
        // backing buffer between iterations.
        for index in 0..count {
            let (rank, frequency) = &renderable[index];
            renderable.push((
                *rank,
                WordFrequency {
                    word: frequency.word.clone(),
                    frequency: frequency.frequency * scale,
                },
            ));
        }
    }
    renderable
}

/// Builds a grid marking the pixels the mask forbids, at output scale.
fn build_mask_blocked(mask: &Mask, width: u32, height: u32, scale: u32) -> BitGrid {
    debug_assert_eq!(width, mask.width() * scale);
    debug_assert_eq!(height, mask.height() * scale);
    let mut blocked = BitGrid::new(width, height);
    for mask_y in 0..mask.height() {
        for mask_x in 0..mask.width() {
            if mask.allowed_grid().get(mask_x, mask_y) {
                continue;
            }
            let start_x = mask_x * scale;
            let start_y = mask_y * scale;
            for y in start_y..start_y + scale {
                for x in start_x..start_x + scale {
                    blocked.set(x, y);
                }
            }
        }
    }
    blocked
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
            .zip(target_row.as_chunks_mut::<4>().0)
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
    fn round_half_even_matches_python_round() {
        assert_eq!(round_half_even(62.4), 62.0);
        assert_eq!(round_half_even(62.6), 63.0);
        assert_eq!(round_half_even(62.5), 62.0);
        assert_eq!(round_half_even(63.5), 64.0);
        assert_eq!(round_half_even(-62.5), -62.0);
        assert_eq!(round_half_even(64.0), 64.0);
    }

    #[test]
    fn strip_possessive_removes_trailing_s() {
        assert_eq!(strip_possessive("ocean's"), "ocean");
        assert_eq!(strip_possessive("IT'S"), "IT");
        assert_eq!(strip_possessive("it"), "it");
        assert_eq!(strip_possessive("cats'"), "cats'");
        assert_eq!(strip_possessive("'s"), "");
    }

    #[test]
    fn uniform_sampling_returns_only_valid_positions() {
        let mut occupied = BitGrid::new(100, 50);
        for y in 10..40 {
            for x in 40..60 {
                occupied.set(x, y);
            }
        }
        let mut first_rng = StableRng::new(1);
        let mut second_rng = StableRng::new(2);
        let mut seen_first = None;
        let mut seen_second = None;
        for _ in 0..64 {
            if let Some((x, y)) = sample_position(&occupied, 10, 5, &mut first_rng) {
                assert!(x <= 90, "{x}");
                assert!(y <= 45, "{y}");
                // The window must not touch the blocked rectangle.
                assert!(x + 10 <= 40 || x >= 60 || y + 5 <= 10 || y >= 40);
                seen_first = Some((x, y));
            }
            if let Some(position) = sample_position(&occupied, 10, 5, &mut second_rng) {
                seen_second = Some(position);
            }
        }
        assert!(seen_first.is_some());
        assert_ne!(seen_first, seen_second);

        // A window that cannot fit anywhere yields None.
        assert_eq!(sample_position(&occupied, 200, 5, &mut first_rng), None);
    }

    #[test]
    fn extend_with_repeats_downweights_and_truncates() {
        let renderable = vec![
            (
                0,
                WordFrequency {
                    word: "rust".to_owned(),
                    frequency: 100.0,
                },
            ),
            (
                1,
                WordFrequency {
                    word: "cloud".to_owned(),
                    frequency: 10.0,
                },
            ),
        ];
        // ceil(5 / 2) - 1 = 2 extra rounds; downweight = 10 / 100 = 0.1.
        let extended = extend_with_repeats(renderable, 5);
        let keys: Vec<_> = extended
            .iter()
            .map(|(rank, frequency)| (*rank, frequency.word.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                (0, "rust"),
                (1, "cloud"),
                (0, "rust"),
                (1, "cloud"),
                (0, "rust"),
            ]
        );
        let frequencies: Vec<f64> = extended
            .iter()
            .map(|(_, frequency)| frequency.frequency)
            .collect();
        let expected = [100.0, 10.0, 10.0, 1.0, 1.0];
        for (actual, expected) in frequencies.iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        }
    }

    #[test]
    fn extend_with_repeats_is_noop_when_max_words_is_reached() {
        let renderable = vec![
            (
                0,
                WordFrequency {
                    word: "rust".to_owned(),
                    frequency: 2.0,
                },
            ),
            (
                1,
                WordFrequency {
                    word: "cloud".to_owned(),
                    frequency: 1.0,
                },
            ),
        ];
        assert_eq!(extend_with_repeats(renderable.clone(), 2), renderable);
        assert_eq!(extend_with_repeats(renderable.clone(), 1), renderable);
    }

    #[test]
    fn builder_seed_mode_can_be_selected() {
        assert_eq!(WordCloudBuilder::new().random_seed, None);
        assert_eq!(WordCloudBuilder::new().random_seed(0).random_seed, Some(0));
        assert_eq!(
            WordCloudBuilder::new()
                .random_seed(42)
                .randomize_each_generation()
                .random_seed,
            None
        );
    }

    #[test]
    fn relative_scaling_auto_follows_repeat() {
        assert_eq!(
            WordCloudBuilder::new().relative_scaling,
            None,
            "auto by default"
        );
        let plain = WordCloudBuilder::new().repeat(false).build().unwrap();
        let repeated = WordCloudBuilder::new().repeat(true).build().unwrap();
        assert_eq!(plain.relative_scaling, 0.5);
        assert_eq!(repeated.relative_scaling, 0.0);
        let explicit = WordCloudBuilder::new()
            .repeat(true)
            .relative_scaling(0.7)
            .build()
            .unwrap();
        assert_eq!(explicit.relative_scaling, 0.7);
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
    fn mask_blocked_grid_matches_pixel_mapping() {
        for logical_width in [63, 64, 65, 127] {
            let logical_height = 7;
            let mask = Mask::from_predicate(logical_width, logical_height, |x, y| {
                (x.wrapping_mul(17) + y * 11) % 13 > 3
            })
            .unwrap();
            for scale in [1, 2, 3] {
                let width = logical_width * scale;
                let height = logical_height * scale;
                let occupied = build_mask_blocked(&mask, width, height, scale);
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
        // With the shared blocked grid this holds by construction: layout
        // never writes into `mask_blocked`, only into its own occupancy.
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

    #[test]
    fn auto_max_font_size_fits_the_canvas_and_is_deterministic() {
        // A single word is probed at canvas height, then shrunk until it
        // fits, so its placed size can never exceed the height.
        let cloud = WordCloud::builder()
            .dimensions(400, 220)
            .random_seed(5)
            .build()
            .unwrap();
        let rendered = cloud
            .generate_detailed_from_frequencies([("wordcloud", 7)])
            .unwrap();
        assert_eq!(rendered.words().len(), 1);
        assert!(rendered.words()[0].font_size <= cloud.height as f32);

        // Several words use the two-word harmonic-mean probe, which stays
        // below the canvas height as well and reproduces exactly.
        let frequencies = [("rust", 30), ("safety", 24), ("performance", 20)];
        let first = cloud
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        let second = cloud
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        assert!(first.words()[0].font_size <= cloud.height as f32);
        assert_eq!(first.words(), second.words());
        assert_eq!(first.image().as_raw(), second.image().as_raw());
    }

    #[test]
    fn placement_stops_once_a_word_no_longer_fits() {
        // A tiny canvas with a large minimum font size: the first words
        // fill it, then the ladder gives up and remaining words are not
        // attempted, matching the Python package.
        let cloud = WordCloud::builder()
            .dimensions(160, 70)
            .min_font_size(24.0)
            .prefer_horizontal(1.0)
            .random_seed(3)
            .build()
            .unwrap();
        let rendered = cloud
            .generate_detailed_from_frequencies([
                ("aaaaaa", 40),
                ("bbbbbb", 35),
                ("cccccc", 30),
                ("dddddd", 25),
                ("eeeeee", 20),
                ("ffffff", 15),
                ("gggggg", 10),
                ("hhhhhh", 5),
            ])
            .unwrap();
        assert!(!rendered.words().is_empty());
        assert!(
            rendered.words().len() < 8,
            "expected the ladder to give up, placed {}",
            rendered.words().len()
        );
        // Placed words keep descending font sizes.
        for pair in rendered.words().windows(2) {
            assert!(pair[0].font_size >= pair[1].font_size);
        }
    }

    #[test]
    fn font_data_borrows_the_arc_without_copying() {
        let arc: Arc<[u8]> = Arc::from(DEFAULT_FONT);
        let cloud = WordCloud::builder()
            .font_data(arc.clone())
            .random_seed(91)
            .build()
            .unwrap();
        // `build()` consumes the builder, so `font_source` is dropped here.
        // The generated font still shares the caller's allocation (caller +
        // `ArcBorrowedFont` = 2). A copy-based path would leave only the
        // caller's ref (1), because the copied `Vec` lives separately.
        assert_eq!(Arc::strong_count(&arc), 2);
        // Dropping the caller's reference must not invalidate the generated
        // font: the guard keeps the shared bytes alive.
        drop(arc);
        let rendered = cloud
            .generate_detailed_from_frequencies([("rust", 30), ("cloud", 20)])
            .unwrap();
        assert!(!rendered.words().is_empty());
    }

    #[test]
    fn jpeg_streaming_save_matches_explicit_rgb_conversion() {
        let cloud = WordCloud::builder()
            .dimensions(140, 90)
            .transparent_background()
            .random_seed(91)
            .build()
            .unwrap();
        let rendered = cloud
            .generate_detailed_from_frequencies([("rust", 30), ("cloud", 20), ("jpeg", 10)])
            .unwrap();

        // New path: encode straight from the RGBA buffer.
        let mut streamed = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut streamed)
            .encode_image(rendered.image())
            .unwrap();

        // Old path: convert to RGB first, then encode.
        let reference = image::RgbImage::from_fn(
            rendered.image().width(),
            rendered.image().height(),
            |x, y| {
                let pixel = rendered.image()[(x, y)];
                image::Rgb([pixel[0], pixel[1], pixel[2]])
            },
        );
        let mut expected = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new(&mut expected);
        use image::ImageEncoder;
        encoder
            .write_image(
                reference.as_raw(),
                reference.width(),
                reference.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();

        assert_eq!(streamed, expected);
    }

    #[test]
    fn embedded_and_borrowed_fonts_render_identically() {
        let embedded = WordCloud::builder().random_seed(91).build().unwrap();
        let borrowed = WordCloud::builder()
            .font_data(Arc::<[u8]>::from(DEFAULT_FONT))
            .random_seed(91)
            .build()
            .unwrap();
        let frequencies = [
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
        ];
        let a = embedded
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        let b = borrowed
            .generate_detailed_from_frequencies(frequencies)
            .unwrap();
        assert_eq!(a.words(), b.words());
        assert_eq!(a.image().as_raw(), b.image().as_raw());
    }
}
