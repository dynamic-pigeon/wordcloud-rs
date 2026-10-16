use std::collections::HashMap;

use crate::tokenizer::StopWords;
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
    let mut words: Vec<WordFrequency> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

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

        match index.get(&word) {
            Some(&position) => {
                let total = words[position].frequency + value;
                if !total.is_finite() {
                    return Err(WordCloudError::InvalidFrequency {
                        word: words[position].word.clone(),
                        value: total,
                    });
                }
                words[position].frequency = total;
            }
            None => {
                index.insert(word.clone(), words.len());
                words.push(WordFrequency {
                    word,
                    frequency: value,
                });
            }
        }
    }

    if words.is_empty() {
        return Err(WordCloudError::EmptyInput);
    }
    sort_frequencies(&mut words);
    Ok(words)
}

/// Orders words by descending frequency with a stable sort, so words with
/// equal frequency keep the order they were first counted in. This matches
/// the Python package, whose layout order for ties is dict insertion order.
pub(crate) fn sort_frequencies(words: &mut [WordFrequency]) {
    words.sort_by(|left, right| right.frequency.total_cmp(&left.frequency));
}

/// Counted words plus the capitalization map produced while counting, both
/// following the Python `process_tokens` semantics.
pub(crate) struct FusedCounts {
    /// `(word, count)` pairs in first-seen order, each word in its most
    /// frequent capitalization.
    pub counts: Vec<(String, u64)>,
    /// Lower-cased word (or plural of one) to its standard capitalization.
    pub standard_form: HashMap<String, String>,
}

/// Counts words per lower-cased key and per exact capitalization, then
/// represents each key by its most frequent capitalization (ties keep the
/// first capitalization seen). With `normalize_plurals`, a key ending in a
/// single `s` is merged into its singular when both were seen, mirroring
/// `tokenization.process_tokens` of the Python package, including its
/// quirks: only the lower-cased key is checked for the `ss` exception, and
/// the capitalization variant always loses its last character.
pub(crate) fn process_tokens(words: &[String], normalize_plurals: bool) -> FusedCounts {
    // Keys in first-seen order; `cases[i]` holds `(capitalization, count)`
    // in first-seen order for `keys[i]`. Cleared case lists mark deleted
    // (merged) plural keys while keeping the indices stable.
    let mut keys: Vec<String> = Vec::new();
    let mut cases: Vec<Vec<(String, u64)>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for word in words {
        let key = word.to_lowercase();
        match index.get(&key) {
            Some(&position) => {
                let variants = &mut cases[position];
                if let Some(last) = variants.last_mut() {
                    if last.0 == *word {
                        last.1 += 1;
                        continue;
                    }
                }
                if let Some(existing) = variants.iter_mut().find(|(seen, _)| seen == word) {
                    existing.1 += 1;
                } else {
                    variants.push((word.clone(), 1));
                }
            }
            None => {
                index.insert(key.clone(), keys.len());
                keys.push(key);
                cases.push(vec![(word.clone(), 1)]);
            }
        }
    }

    let mut merged_plurals: Vec<(String, String)> = Vec::new();
    if normalize_plurals {
        for position in 0..keys.len() {
            let key = keys[position].clone();
            if !(key.ends_with('s') && !key.ends_with("ss")) {
                continue;
            }
            let singular_key = strip_last_char(&key);
            let Some(&singular) = index.get(&singular_key) else {
                continue;
            };
            if singular == position {
                continue;
            }
            let plural_variants = std::mem::take(&mut cases[position]);
            for (variant, count) in plural_variants {
                let singular_variant = strip_last_char(&variant);
                let variants = &mut cases[singular];
                if let Some(existing) = variants
                    .iter_mut()
                    .find(|(seen, _)| *seen == singular_variant)
                {
                    existing.1 += count;
                } else {
                    variants.push((singular_variant, count));
                }
            }
            index.remove(&key);
            merged_plurals.push((key, singular_key));
        }
    }

    let mut counts: Vec<(String, u64)> = Vec::new();
    let mut standard_form: HashMap<String, String> = HashMap::new();
    for (position, key) in keys.iter().enumerate() {
        let variants = &cases[position];
        let Some(first) = variants.first() else {
            continue;
        };
        // Most frequent capitalization; ties keep the first one seen, like
        // `max` over a Python insertion-ordered dict (Rust's `max_by` would
        // keep the last, so scan manually).
        let mut best = first;
        for entry in &variants[1..] {
            if entry.1 > best.1 {
                best = entry;
            }
        }
        counts.push((
            best.0.clone(),
            variants.iter().map(|(_, count)| count).sum(),
        ));
        standard_form.insert(key.clone(), best.0.clone());
    }
    for (plural, singular) in &merged_plurals {
        standard_form.insert(plural.clone(), standard_form[singular].clone());
    }

    FusedCounts {
        counts,
        standard_form,
    }
}

/// Dunning log-likelihood term from the Python package: `l(k, n, x)`.
fn log_likelihood_term(k: f64, n: f64, x: f64) -> f64 {
    x.max(1e-10).ln() * k + (1.0 - x).max(1e-10).ln() * (n - k)
}

/// Collocation score of a bigram, `tokenization.score` of the Python
/// package (Dunning's log-likelihood, scaled by -2).
pub(crate) fn dunning_score(bigram_count: u64, count1: u64, count2: u64, n_words: u64) -> f64 {
    if n_words <= count1 || n_words <= count2 {
        return 0.0;
    }
    let n = n_words as f64;
    let c12 = bigram_count as f64;
    let c1 = count1 as f64;
    let c2 = count2 as f64;
    let p = c2 / n;
    let p1 = c12 / c1;
    let p2 = (c2 - c12) / (n - c1);
    let score = log_likelihood_term(c12, c1, p) + log_likelihood_term(c2 - c12, n - c1, p)
        - log_likelihood_term(c12, c1, p1)
        - log_likelihood_term(c2 - c12, n - c1, p2);
    -2.0 * score
}

/// Counts unigrams and promoted bigram collocations, mirroring
/// `tokenization.unigrams_and_bigrams` of the Python package: bigrams are
/// built from adjacent non-stopword tokens, bigrams whose Dunning score
/// exceeds `threshold` are added and discount their constituent words
/// (possibly below zero, then dropped).
pub(crate) fn unigrams_and_bigrams(
    words: Vec<String>,
    stopwords: &StopWords,
    normalize_plurals: bool,
    threshold: f64,
) -> Vec<(String, u64)> {
    // Bigrams are built before stopword removal so a removed middle word
    // does not glue its neighbours together.
    let bigrams: Vec<String> = words
        .windows(2)
        .filter(|pair| !stopwords.contains(&pair[0]) && !stopwords.contains(&pair[1]))
        .map(|pair| format!("{} {}", pair[0], pair[1]))
        .collect();
    let unigrams: Vec<String> = words
        .iter()
        .filter(|word| !stopwords.contains(word))
        .cloned()
        .collect();
    let n_words = unigrams.len() as u64;

    let FusedCounts {
        counts: mut unigram_counts,
        standard_form,
    } = process_tokens(&unigrams, normalize_plurals);
    let FusedCounts {
        counts: bigram_counts,
        ..
    } = process_tokens(&bigrams, normalize_plurals);
    // Snapshot before any discounting so later bigrams see the same scores
    // regardless of iteration order.
    let original: HashMap<String, u64> = unigram_counts
        .iter()
        .map(|(word, count)| (word.clone(), *count))
        .collect();

    for (bigram, count) in &bigram_counts {
        let parts: Vec<&str> = bigram.split(' ').collect();
        // Tokens containing spaces (possible with a custom tokenizer) have
        // no Python equivalent here; skip them instead of panicking.
        if parts.len() != 2 {
            continue;
        }
        let Some(word1) = standard_form.get(&parts[0].to_lowercase()) else {
            continue;
        };
        let Some(word2) = standard_form.get(&parts[1].to_lowercase()) else {
            continue;
        };
        let (Some(&count1), Some(&count2)) =
            (original.get(word1.as_str()), original.get(word2.as_str()))
        else {
            continue;
        };
        if dunning_score(*count, count1, count2, n_words) > threshold {
            for word in [word1, word2] {
                if let Some(entry) = unigram_counts.iter_mut().find(|(seen, _)| seen == word) {
                    entry.1 = entry.1.saturating_sub(*count);
                }
            }
            unigram_counts.push((bigram.clone(), *count));
        }
    }
    unigram_counts.retain(|(_, count)| *count > 0);
    unigram_counts
}

/// Removes the last character of `word`; used by the plural merge, which
/// strips exactly one character regardless of its byte length.
fn strip_last_char(word: &str) -> String {
    word.chars()
        .take(word.chars().count().saturating_sub(1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn counts(words: &[&str], normalize_plurals: bool) -> Vec<(String, u64)> {
        process_tokens(&owned(words), normalize_plurals).counts
    }

    #[test]
    fn case_fusion_picks_the_most_frequent_capitalization() {
        assert_eq!(
            counts(&["Rust", "rust", "rust", "RUST"], false),
            [("rust".to_owned(), 4)]
        );
        // Ties keep the first capitalization seen.
        assert_eq!(
            counts(&["Rust", "RUST", "rust"], false),
            [("Rust".to_owned(), 3)]
        );
    }

    #[test]
    fn normalize_plurals_merges_into_the_singular() {
        assert_eq!(
            counts(&["word", "words", "word", "words"], true),
            [("word".to_owned(), 4)]
        );
        // A plural without its singular stays untouched.
        assert_eq!(
            counts(&["clouds", "clouds"], true),
            [("clouds".to_owned(), 2)]
        );
        // "ss" endings are not plurals.
        assert_eq!(
            counts(&["class", "clas"], true),
            [("class".to_owned(), 1), ("clas".to_owned(), 1)]
        );
        // Case variants lose their last character when merged.
        assert_eq!(
            counts(&["word", "Words", "word"], true),
            [("word".to_owned(), 3)]
        );
    }

    #[test]
    fn dunning_score_rises_with_bigram_exclusivity() {
        // A bigram whose words only ever appear together scores far above
        // two words that merely appear next to each other once.
        let exclusive = dunning_score(10, 10, 10, 30);
        let incidental = dunning_score(1, 5, 5, 50);
        assert!(exclusive > 30.0);
        assert!(incidental < 30.0);
        // Degenerate inputs are guarded like the reference.
        assert_eq!(dunning_score(1, 10, 3, 10), 0.0);
    }

    #[test]
    fn unigrams_and_bigrams_promotes_strong_collocations() {
        let stopwords = StopWords::new();
        let mut words = Vec::new();
        for _ in 0..10 {
            words.push("new".to_owned());
            words.push("york".to_owned());
        }
        for filler in [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        ] {
            words.push(filler.to_owned());
        }

        let result = unigrams_and_bigrams(words, &stopwords, false, 30.0);
        let lookup: HashMap<String, u64> = result.into_iter().collect();
        // "new york" appears ten times and its words appear nowhere else,
        // so it is promoted and the constituents are fully discounted.
        assert_eq!(lookup.get("new york"), Some(&10));
        assert!(!lookup.contains_key("new"));
        assert!(!lookup.contains_key("york"));
        // Filler singletons survive.
        assert_eq!(lookup.get("alpha"), Some(&1));
    }
}
