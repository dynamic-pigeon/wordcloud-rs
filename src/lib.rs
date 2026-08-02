//! Random or reproducible word-cloud generation for Rust.
//!
//! [`WordCloud`] can generate an image either from natural text or from explicit
//! word frequencies. The latter is useful for languages that need an external
//! segmenter.
//!
//! ```no_run
//! use wordcloud::{Rgba, WordCloud};
//!
//! let cloud = WordCloud::builder()
//!     .dimensions(800, 500)
//!     .background_color(Rgba([248, 248, 246, 255]))
//!     .build()?;
//!
//! cloud.save(
//!     "Rust makes systems programming productive and reliable. Rust Rust Rust.",
//!     "wordcloud.png",
//! )?;
//! # Ok::<(), wordcloud::WordCloudError>(())
//! ```

mod bitmap;
mod cloud;
mod color;
mod error;
mod frequency;
mod mask;
mod text;
mod tokenizer;

pub use cloud::{
    FontSource, Orientation, PlacedWord, RenderedWordCloud, WordCloud, WordCloudBuilder,
};
pub use color::{ColorContext, Colorizer, Palette, SolidColor};
pub use error::{Result, WordCloudError};
pub use frequency::{FrequencyValue, IntoWordFrequency, WordFrequency};
pub use image::{DynamicImage, GrayImage, Rgba, RgbaImage};
pub use mask::{Mask, MaskPolarity};
pub use tokenizer::{DefaultTokenizer, StopWords, Tokenizer};
