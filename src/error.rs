use std::path::PathBuf;

/// A result returned by this crate.
pub type Result<T> = std::result::Result<T, WordCloudError>;

/// Errors produced while configuring, laying out, rendering, or saving a cloud.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WordCloudError {
    /// A builder option is outside its accepted range.
    #[error("invalid parameter `{parameter}`: {reason}")]
    InvalidParameter {
        parameter: &'static str,
        reason: String,
    },

    /// Reading a custom font failed.
    #[error("failed to read font `{path}`: {source}")]
    FontRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Font bytes could not be parsed.
    #[error("invalid font: {reason}")]
    InvalidFont { reason: String },

    /// Text contained no words after tokenization and filtering.
    #[error("input contains no usable words")]
    EmptyInput,

    /// Explicit frequency input contained an invalid value.
    #[error("invalid frequency for `{word}`: {value}; frequencies must be finite and positive")]
    InvalidFrequency { word: String, value: f64 },

    /// An input word is empty or exceeds a configured resource limit.
    #[error("invalid word `{word}`: {reason}")]
    InvalidWord { word: String, reason: String },

    /// None of the requested words is supported by the configured font.
    #[error("the configured font cannot render any input word")]
    NoRenderableWords,

    /// Words were valid but none fit in the available shape.
    #[error("no words could be placed (attempted {attempted})")]
    NoWordsPlaced { attempted: usize },

    /// The requested output would require an unsafe allocation.
    #[error("canvas {width}x{height} is too large")]
    CanvasTooLarge { width: u32, height: u32 },

    /// Encoding or saving an image failed.
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

impl WordCloudError {
    pub(crate) fn invalid(parameter: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidParameter {
            parameter,
            reason: reason.into(),
        }
    }
}
