# wordcloud

[简体中文](README.md) | English

A pure-Rust word-cloud generator that uses Python `wordcloud` 1.9.6 as its
behavioral baseline: it builds layouts from text or explicit word
frequencies, scales font sizes by frequency, places words without collisions
inside a mask, supports horizontal and 90-degree vertical text, fixed random
seeds, custom fonts and colors, and outputs `image::RgbaImage` or saves image
files directly. Defaults and algorithmic semantics (tokenization,
collocations, the font-size ladder, placement sampling) follow the Python
package; the remaining gaps are listed under "Differences from Python" below.

Open Sans is bundled, so English works out of the box without OS fonts.
Chinese and other languages need a TTF/OTF/TTC font covering the characters;
segmentation for such languages should happen in the application, which then
passes word frequencies to this crate.

## Using as a dependency

During development a path dependency works directly:

```toml
[dependencies]
wordcloud = { path = "../wordcloud" }
```

From natural text:

```rust
use wordcloud::{Rgba, WordCloud};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cloud = WordCloud::builder()
        .dimensions(800, 450)
        .max_words(100)
        .max_font_size(96.0)
        .background_color(Rgba([248, 248, 246, 255]))
        .build()?;

    cloud.save(
        "Rust is fast safe productive. Rust ownership safety performance.",
        "wordcloud.png",
    )?;
    Ok(())
}
```

From explicit frequencies; integer and float weights both work:

```rust
use wordcloud::WordCloud;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cloud = WordCloud::builder()
        .dimensions(900, 520)
        .font_path("/path/to/NotoSansCJK-Regular.ttc")
        .font_index(0)
        .build()?;

    cloud.save_from_frequencies(
        [("词云", 100), ("Rust", 85), ("可视化", 64), ("性能", 48)],
        "wordcloud-zh.png",
    )?;
    Ok(())
}
```

`generate_detailed` and `generate_detailed_from_frequencies` return a
`RenderedWordCloud` holding both the `RgbaImage` and each word's position,
font size, orientation, color, and normalized frequency — useful for
interactive hotspots or further compositing. `generate` /
`generate_from_frequencies` return an `RgbaImage`; use the `generate_dynamic*`
methods when you need a `DynamicImage`.

## Masks and colors

`Mask::from_luma8` matches the common Python semantics: pure white (255)
pixels are blocked, everything else is usable. For anti-aliasing thresholds
or inverted black/white, use `from_luma8_with_threshold` and `MaskPolarity`.
Shapes can also be defined by a predicate directly:

```rust
use wordcloud::{Mask, Rgba, WordCloud};

fn main() -> Result<(), wordcloud::WordCloudError> {
    let mask = Mask::from_predicate(600, 600, |x, y| {
        let dx = x as i64 - 300;
        let dy = y as i64 - 300;
        dx * dx + dy * dy < 270 * 270
    })?;

    let _cloud = WordCloud::builder()
        .mask(mask) // the mask dimensions define the canvas
        .palette([
            Rgba([25, 97, 128, 255]),
            Rgba([37, 139, 119, 255]),
            Rgba([214, 151, 56, 255]),
        ])
        .build()?;
    Ok(())
}
```

A single color is available through `solid_color`, and a `colorizer` closure
can color words dynamically from the word, frequency, position, orientation,
and a stable random value. Use `transparent_background()` for a transparent
background.

## Behavior notes

This crate treats Python `wordcloud` 1.9.6 as its reference; the items below
follow its source, and the remaining gaps are listed explicitly.

- Every generation uses a fresh random seed by default. Set `random_seed`
  explicitly for reproducible output: with identical dependency versions,
  configuration, font, and input, both the layout and the pixels are
  identical. `randomize_each_generation()` restores the default mode.
  Custom tokenizers/colorizers must also be deterministic to keep this
  guarantee.
- Explicit frequency input merges duplicate words and bypasses the
  tokenizer, case handling, and stopwords.
- The text pipeline mirrors Python's `process_text`: a `\w[\w']*` regexp
  tokenizer (apostrophes stay inside words, e.g. `don't`; contiguous CJK
  runs become single tokens), trailing `'s` stripping (`that's` → `that`),
  pure-number tokens filtered by default (`include_numbers(true)` keeps
  them), collocations enabled by default (adjacent bigrams whose Dunning
  log-likelihood score exceeds 30 are promoted, e.g. `new york`), and plural
  merging enabled by default (`words` folds into `word`, `ss` endings
  excepted).
- Case handling matches Python: variants of a word fuse into its most
  frequent capitalization (ties keep the first seen); `lowercase(true)`
  opts back into plain lowercasing. The default stopwords are the 192-word
  English list shipped with the Python package (including `com`, `www`,
  `http`).
- The font-size ladder matches Python: `relative_scaling` defaults to "auto"
  (0.5 normally, 0 with `repeat`), sizes update by the ratio to the previous
  word's frequency (Python's round-half-even), and an unset `max_font_size`
  uses the Python auto-fit — a single word takes the canvas height, several
  words probe the top two words at canvas height and start from the
  harmonic mean of their fitted sizes. Placement stops entirely once a word
  no longer fits at `min_font_size`. The default `font_step` is 1.
- Placement search is equivalent to Python's `query_integral_image`: every
  completely free candidate window is enumerated and one is chosen
  uniformly at random (no center bias), with roughly `margin/2` of
  clearance per side (Python's box-plus-margin semantics; odd margins round
  down).
- When fewer than `max_words` words are available, `repeat(true)` pads the
  list with downweighted copies of the existing words, matching Python's
  `repeat` semantics; repeated words render progressively smaller. Off by
  default.
- Languages that need segmentation (Chinese, Japanese, ...) should be
  segmented in the application — for example with `jieba-rs` — and passed as
  frequencies (the default tokenizer keeps contiguous CJK runs as single
  tokens, exactly like Python).
- `font_bytes` embeds your own font; TTC collections select a face with
  `font_index`.
- Glyph rasterization is based on `ab_glyph`. The current version suits
  Latin, CJK, and other scripts that need no complex shaping; advanced
  shaping (Arabic, some Indic scripts) and color emoji glyphs are not
  implemented.
- `scale(n)` re-lays out and collides at physical pixels (finer than
  Python, which renders a logical layout scaled up); the output is
  `width*n` x `height*n`.
- Words are limited to 256 Unicode scalar values by default
  (`max_word_length` adjusts this; hard limit 4096) and the canvas is
  capped at 16 million pixels to avoid accidental huge allocations in
  downstream applications.
- `save` supports PNG, JPEG, and WebP by extension; unknown or disabled
  formats return `WordCloudError::Image`. JPEG keeps no alpha: a
  transparent background loses its alpha channel when saved as JPEG — use
  PNG or WebP when transparency matters.

**Differences from Python**: the default colors are the bundled 6-color
palette (Python samples a viridis colormap randomly; pass a custom
`colorizer` or `palette` to replicate it); the default background is black,
as in Python; the orientation retry does not repeat Python's redundant
second vertical probe (the observable behavior is equivalent); mask
anti-aliasing thresholds beyond the default, contour lines, and SVG output
are not implemented.

## Examples and verification

```bash
cargo run --example basic
cargo run --example chinese -- /path/to/chinese-font.ttf
cargo test
```

## Performance comparison

The repository contains a reproducible end-to-end benchmark against Python
`wordcloud` (full in-memory RGBA image generation); see
[benchmarks/README.md](benchmarks/README.md) for how to run it and what is
compared. Measurements are produced locally and not committed.

The default font `assets/OpenSans-Regular.ttf` is licensed under the SIL OFL
1.1 (see `assets/OFL.txt`). The library code is MIT licensed.
