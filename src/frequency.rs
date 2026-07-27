use std::collections::{hash_map::Entry, HashMap};

use crate::{Result, WordCloudError};

/// A word and its (unnormalized) frequency.
#[derive(Clone, Debug, PartialEq)]
pub struct WordFrequency {
    pub word: String,
    pub frequency: f64,
}

/// Numeric values accepted by [`WordCloud::generate_from_frequencies`](crate::WordCloud::generate_from_frequencies).
pub trait FrequencyValue {
    fn to_f64(self) -> f64;
}

macro_rules! frequency_values {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl FrequencyValue for $ty {
                fn to_f64(self) -> f64 {
                    self as f64
                }
            }
        )+
    };
}

frequency_values!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);

impl<T> FrequencyValue for &T
where
    T: FrequencyValue + Copy,
{
    fn to_f64(self) -> f64 {
        (*self).to_f64()
    }
}

/// An item accepted by the explicit-frequency generation methods.
///
/// This is implemented for `(word, number)`, owned [`WordFrequency`] values,
/// and references to `WordFrequency` values.
pub trait IntoWordFrequency {
    fn into_word_frequency(self) -> WordFrequency;
}

impl<S, N> IntoWordFrequency for (S, N)
where
    S: AsRef<str>,
    N: FrequencyValue,
{
    fn into_word_frequency(self) -> WordFrequency {
        WordFrequency {
            word: self.0.as_ref().to_owned(),
            frequency: self.1.to_f64(),
        }
    }
}

impl IntoWordFrequency for WordFrequency {
    fn into_word_frequency(self) -> WordFrequency {
        self
    }
}

impl IntoWordFrequency for &WordFrequency {
    fn into_word_frequency(self) -> WordFrequency {
        self.clone()
    }
}

pub(crate) fn collect_frequencies<I, T>(input: I) -> Result<Vec<WordFrequency>>
where
    I: IntoIterator<Item = T>,
    T: IntoWordFrequency,
{
    let mut merged = HashMap::<String, f64>::new();

    for item in input {
        let WordFrequency { word, frequency } = item.into_word_frequency();
        let value = frequency;
        if word.trim().is_empty() {
            return Err(WordCloudError::InvalidWord {
                word,
                reason: "must not be empty".to_owned(),
            });
        }
        if !value.is_finite() || value <= 0.0 {
            return Err(WordCloudError::InvalidFrequency { word, value });
        }

        match merged.entry(word) {
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
            Entry::Occupied(mut entry) => {
                let total = *entry.get() + value;
                if !total.is_finite() {
                    return Err(WordCloudError::InvalidFrequency {
                        word: entry.key().clone(),
                        value: total,
                    });
                }
                *entry.get_mut() = total;
            }
        }
    }

    if merged.is_empty() {
        return Err(WordCloudError::EmptyInput);
    }

    let mut words: Vec<_> = merged
        .into_iter()
        .map(|(word, frequency)| WordFrequency { word, frequency })
        .collect();
    sort_frequencies(&mut words);
    Ok(words)
}

/// Orders words by descending frequency with a word tie-break, so the result
/// is fully deterministic regardless of the collection order upstream.
pub(crate) fn sort_frequencies(words: &mut [WordFrequency]) {
    words.sort_by(|left, right| {
        right
            .frequency
            .total_cmp(&left.frequency)
            .then_with(|| left.word.cmp(&right.word))
    });
}
