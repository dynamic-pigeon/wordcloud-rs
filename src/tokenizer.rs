use std::collections::HashSet;

use unicode_segmentation::UnicodeSegmentation;

/// Converts natural text into candidate words.
pub trait Tokenizer: Send + Sync {
    fn tokenize(&self, text: &str) -> Vec<String>;

    /// Streams each token into `visit` without materializing a vector. The
    /// default implementation falls back to [`Tokenizer::tokenize`];
    /// tokenizers that can borrow from the input should override it.
    fn for_each_token(&self, text: &str, visit: &mut dyn FnMut(&str)) {
        for token in self.tokenize(text) {
            visit(&token);
        }
    }
}

impl<F> Tokenizer for F
where
    F: Fn(&str) -> Vec<String> + Send + Sync,
{
    fn tokenize(&self, text: &str) -> Vec<String> {
        self(text)
    }
}

/// Unicode word-boundary tokenizer used by default.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultTokenizer;

impl Tokenizer for DefaultTokenizer {
    fn tokenize(&self, text: &str) -> Vec<String> {
        text.unicode_words().map(str::to_owned).collect()
    }

    fn for_each_token(&self, text: &str, visit: &mut dyn FnMut(&str)) {
        for word in text.unicode_words() {
            visit(word);
        }
    }
}

/// A case-insensitive collection of words omitted from text input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopWords {
    words: HashSet<String>,
}

impl StopWords {
    /// Creates an empty stop-word collection.
    pub fn new() -> Self {
        Self {
            words: HashSet::new(),
        }
    }

    /// Returns a practical English stop-word list similar to Python wordcloud.
    pub fn english() -> Self {
        let words = ENGLISH_STOP_WORDS
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect();
        Self { words }
    }

    pub fn insert(&mut self, word: impl AsRef<str>) -> bool {
        self.words.insert(word.as_ref().to_lowercase())
    }

    pub fn extend<I, S>(&mut self, words: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.words
            .extend(words.into_iter().map(|word| word.as_ref().to_lowercase()));
    }

    pub fn contains(&self, word: &str) -> bool {
        self.words.contains(&word.to_lowercase())
    }

    /// Looks up a word that is already lowercased, avoiding a second
    /// lowercase allocation on the caller side.
    pub(crate) fn contains_lowercased(&self, word: &str) -> bool {
        self.words.contains(word)
    }

    pub fn clear(&mut self) {
        self.words.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.words.iter().map(String::as_str)
    }
}

impl Default for StopWords {
    fn default() -> Self {
        Self::english()
    }
}

const ENGLISH_STOP_WORDS: &str = "
a about above after again against all also am an and any are aren't as at be because been before
being below between both but by can can't cannot could couldn't did didn't do does doesn't doing
don't down during each else ever few for from further get gets got had hadn't has hasn't have
haven't having he he'd he'll he's her here here's hers herself him himself his how how's however i
i'd i'll i'm i've if in into is isn't it it's its itself just like may me might more most mustn't my
myself no nor not of off on once only or other ought our ours ourselves out over own same shall
shan't she she'd she'll she's should shouldn't since so some such than that that's the their theirs
them themselves then there there's these they they'd they'll they're they've this those through to
too under until up very was wasn't we we'd we'll we're we've were weren't what what's when when's
where where's which while who who's whom why why's will with won't would wouldn't you you'd you'll
you're you've your yours yourself yourselves
";
