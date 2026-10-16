use std::collections::HashSet;

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

/// A word character under the Python `re` `\w` class: alphanumeric
/// characters plus the underscore.
fn is_word_start(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Continuation characters of the default pattern: `\w` plus the apostrophe.
fn is_word_continue(character: char) -> bool {
    is_word_start(character) || character == '\''
}

/// Default tokenizer matching the Python `wordcloud` package regexp
/// `r"\w[\w']*"`: a token starts at a word character (Unicode alphanumeric
/// or underscore) and continues over word characters and apostrophes, so
/// `don't` stays one token while `co-op` splits into two. Contiguous CJK
/// runs are single tokens, matching Python's Unicode `\w`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultTokenizer;

impl DefaultTokenizer {
    fn for_each<F>(&self, text: &str, visit: &mut F)
    where
        F: FnMut(&str) + ?Sized,
    {
        let mut start = None;
        for (index, character) in text.char_indices() {
            let in_token = start.is_some();
            let accepted = if in_token {
                is_word_continue(character)
            } else {
                is_word_start(character)
            };
            if accepted {
                if start.is_none() {
                    start = Some(index);
                }
            } else if let Some(begin) = start.take() {
                visit(&text[begin..index]);
            }
        }
        if let Some(begin) = start {
            visit(&text[begin..]);
        }
    }
}

impl Tokenizer for DefaultTokenizer {
    fn tokenize(&self, text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        self.for_each(text, &mut |token: &str| tokens.push(token.to_owned()));
        tokens
    }

    fn for_each_token(&self, text: &str, visit: &mut dyn FnMut(&str)) {
        self.for_each(text, visit);
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

    /// Returns the stop-word list shipped with the Python `wordcloud`
    /// package, matched verbatim.
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

// The stop-word list bundled with Python wordcloud 1.9.6 (wordcloud/stopwords).
const ENGLISH_STOP_WORDS: &str = "
a about above after again against all also am an and any
are aren't as at be because been before being below between both
but by can can't cannot com could couldn't did didn't do does
doesn't doing don't down during each else ever few for from further
get had hadn't has hasn't have haven't having he he'd he'll he's
hence her here here's hers herself him himself his how how's however
http i i'd i'll i'm i've if in into is isn't it
it's its itself just k let's like me more most mustn't my
myself no nor not of off on once only or other otherwise
ought our ours ourselves out over own r same shall shan't she
she'd she'll she's should shouldn't since so some such than that that's
the their theirs them themselves then there there's therefore these they they'd
they'll they're they've this those through to too under until up very
was wasn't we we'd we'll we're we've were weren't what what's when
when's where where's which while who who's whom why why's with won't
would wouldn't www you you'd you'll you're you've your yours yourself yourselves
";

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        DefaultTokenizer.tokenize(text)
    }

    #[test]
    fn default_tokenizer_matches_the_python_regexp_semantics() {
        assert_eq!(tokens("don't panic"), ["don't", "panic"]);
        assert_eq!(tokens("co-op"), ["co", "op"]);
        assert_eq!(tokens("it's Rust's"), ["it's", "Rust's"]);
        assert_eq!(tokens("hello_world foo_bar"), ["hello_world", "foo_bar"]);
        assert_eq!(tokens("42 3.14 x1"), ["42", "3", "14", "x1"]);
        assert_eq!(tokens("词云布局"), ["词云布局"]);
        assert_eq!(tokens("trailing'"), ["trailing'"]);
        assert_eq!(tokens("'lead"), ["lead"]);
        assert_eq!(tokens("a"), ["a"]);
        assert_eq!(tokens("..."), Vec::<String>::new());
    }

    #[test]
    fn english_stopwords_match_the_python_wordcloud_list() {
        let stopwords = StopWords::english();
        assert_eq!(stopwords.len(), 192);
        // Distinctive Python entries.
        for word in [
            "com",
            "www",
            "http",
            "r",
            "k",
            "hence",
            "otherwise",
            "let's",
        ] {
            assert!(stopwords.contains(word), "{word}");
        }
        // Newlines around the constant never leak in.
        assert!(!stopwords.contains(""));
    }
}
