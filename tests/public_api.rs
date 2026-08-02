use std::collections::HashMap;

use wordcloud::{GrayImage, Mask, Orientation, Rgba, WordCloud, WordCloudError, WordFrequency};

fn non_background_pixels(image: &wordcloud::RgbaImage, background: Rgba<u8>) -> usize {
    image.pixels().filter(|pixel| **pixel != background).count()
}

#[test]
fn text_generation_is_nonempty_and_deterministic() {
    let background = Rgba([249, 248, 246, 255]);
    let cloud = WordCloud::builder()
        .dimensions(480, 280)
        .max_font_size(72.0)
        .background_color(background)
        .random_seed(17)
        .build()
        .unwrap();
    let text = "rust safety performance tooling ownership borrowing concurrency \
                rust safety performance tooling ownership reliable productive";

    let first = cloud.generate_detailed(text).unwrap();
    let second = cloud.generate_detailed(text).unwrap();
    assert_eq!(first.image().dimensions(), (480, 280));
    assert!(first.words().len() >= 6);
    assert!(non_background_pixels(first.image(), background) > 1_000);
    assert_eq!(first.words(), second.words());
    assert_eq!(first.image().as_raw(), second.image().as_raw());
}

#[test]
fn integer_frequency_map_works_and_bypasses_stopwords() {
    let frequencies = HashMap::from([
        ("the".to_owned(), 30_usize),
        ("rust".to_owned(), 18),
        ("cloud".to_owned(), 9),
    ]);
    let cloud = WordCloud::builder()
        .dimensions(360, 220)
        .prefer_horizontal(1.0)
        .random_seed(3)
        .build()
        .unwrap();
    let rendered = cloud
        .generate_detailed_from_frequencies(&frequencies)
        .unwrap();
    assert!(rendered.words().iter().any(|word| word.word == "the"));
    assert!(rendered
        .words()
        .iter()
        .all(|word| word.orientation == Orientation::Horizontal));
    assert!(rendered.words()[0].font_size >= rendered.words()[1].font_size);

    // A borrowed map is accepted without cloning it in the calling code.
    assert!(cloud.generate_from_frequencies(&frequencies).is_ok());
}

#[test]
fn word_frequency_output_can_be_fed_back_directly() {
    let cloud = WordCloud::builder().dimensions(360, 220).build().unwrap();
    let frequencies = cloud.word_frequencies("rust rust cloud layout").unwrap();
    let image = cloud.generate_from_frequencies(&frequencies).unwrap();
    assert_eq!(image.dimensions(), (360, 220));

    let duplicate = [
        WordFrequency {
            word: "rust".to_owned(),
            frequency: 2.0,
        },
        WordFrequency {
            word: "rust".to_owned(),
            frequency: 3.0,
        },
    ];
    let rendered = cloud.generate_detailed_from_frequencies(duplicate).unwrap();
    assert_eq!(rendered.words()[0].frequency, 5.0);
}

#[test]
fn text_stopwords_and_case_are_applied() {
    let cloud = WordCloud::builder().build().unwrap();
    let words = cloud
        .word_frequencies("Rust RUST rust and AND cloud")
        .unwrap();
    assert_eq!(words[0].word, "rust");
    assert_eq!(words[0].frequency, 3.0);
    assert_eq!(words.len(), 2);
    assert!(!words.iter().any(|word| word.word == "and"));
}

#[test]
fn custom_tokenizer_and_text_controls_are_applied() {
    let cloud = WordCloud::builder()
        .tokenizer(|_: &str| {
            vec![
                "Machine Learning".to_owned(),
                "42".to_owned(),
                "x".to_owned(),
            ]
        })
        .clear_stopwords()
        .lowercase(false)
        .include_numbers(true)
        .min_word_length(2)
        .build()
        .unwrap();
    let words = cloud.word_frequencies("ignored").unwrap();
    assert_eq!(words.len(), 2);
    assert!(words.iter().any(|word| word.word == "Machine Learning"));
    assert!(words.iter().any(|word| word.word == "42"));
}

#[test]
fn seed_changes_layout_but_palette_does_not() {
    let frequencies = [
        ("rust", 12),
        ("safety", 11),
        ("cloud", 10),
        ("layout", 9),
        ("image", 8),
        ("color", 7),
        ("font", 6),
        ("data", 5),
    ];
    let build = |seed, colors| {
        WordCloud::builder()
            .dimensions(420, 240)
            .prefer_horizontal(0.5)
            .palette(colors)
            .random_seed(seed)
            .build()
            .unwrap()
            .generate_detailed_from_frequencies(frequencies)
            .unwrap()
    };
    let first = build(10, [Rgba([1, 2, 3, 255]), Rgba([4, 5, 6, 255])]);
    let recolored = build(10, [Rgba([200, 20, 30, 255]), Rgba([20, 160, 80, 255])]);
    let second_seed = build(11, [Rgba([1, 2, 3, 255]), Rgba([4, 5, 6, 255])]);

    let geometry = |rendered: &wordcloud::RenderedWordCloud| {
        rendered
            .words()
            .iter()
            .map(|word| {
                (
                    word.word.clone(),
                    word.x,
                    word.y,
                    word.font_size.to_bits(),
                    word.orientation,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(geometry(&first), geometry(&recolored));
    assert_ne!(geometry(&first), geometry(&second_seed));
}

#[test]
fn default_generation_uses_a_fresh_layout_seed() {
    let frequencies = [
        ("rust", 12),
        ("safety", 11),
        ("cloud", 10),
        ("layout", 9),
        ("image", 8),
        ("color", 7),
        ("font", 6),
        ("data", 5),
    ];
    let cloud = WordCloud::builder()
        .dimensions(420, 240)
        .prefer_horizontal(0.5)
        .build()
        .unwrap();

    let first = cloud
        .generate_detailed_from_frequencies(frequencies)
        .unwrap();
    let second = cloud
        .generate_detailed_from_frequencies(frequencies)
        .unwrap();
    let geometry = |rendered: &wordcloud::RenderedWordCloud| {
        rendered
            .words()
            .iter()
            .map(|word| (word.word.clone(), word.x, word.y, word.orientation))
            .collect::<Vec<_>>()
    };

    assert_ne!(geometry(&first), geometry(&second));
}

#[test]
fn mask_controls_dimensions_and_blocks_output_pixels() {
    let background = Rgba([255, 255, 255, 255]);
    let mask = Mask::from_predicate(320, 240, |x, y| {
        let dx = x as i64 - 160;
        let dy = y as i64 - 120;
        dx * dx + dy * dy <= 105 * 105
    })
    .unwrap();
    let cloud = WordCloud::builder()
        .dimensions(10, 10)
        .mask(mask.clone())
        .max_font_size(58.0)
        .background_color(background)
        .random_seed(29)
        .build()
        .unwrap();
    let image = cloud
        .generate_from_frequencies([
            ("rust", 20),
            ("cloud", 16),
            ("safe", 12),
            ("fast", 10),
            ("data", 8),
        ])
        .unwrap();
    assert_eq!(image.dimensions(), mask.dimensions());
    for (x, y, pixel) in image.enumerate_pixels() {
        if !mask.is_allowed(x, y) {
            assert_eq!(*pixel, background);
        }
    }
}

#[test]
fn vertical_extreme_and_scale_are_observable() {
    let cloud = WordCloud::builder()
        .dimensions(220, 180)
        .scale(2)
        .max_font_size(38.0)
        .prefer_horizontal(0.0)
        .random_seed(5)
        .build()
        .unwrap();
    let rendered = cloud
        .generate_detailed_from_frequencies([("rust", 8), ("cloud", 5), ("safe", 3)])
        .unwrap();
    assert_eq!(rendered.image().dimensions(), (440, 360));
    assert!(rendered
        .words()
        .iter()
        .all(|word| word.orientation == Orientation::Vertical));
}

#[test]
fn solid_and_custom_colors_are_used() {
    let red = Rgba([210, 20, 30, 255]);
    let cloud = WordCloud::builder()
        .dimensions(260, 160)
        .solid_color(red)
        .build()
        .unwrap();
    let rendered = cloud
        .generate_detailed_from_frequencies([("rust", 2), ("cloud", 1)])
        .unwrap();
    assert!(rendered.words().iter().all(|word| word.color == red));

    let custom = WordCloud::builder()
        .dimensions(260, 160)
        .colorizer(|context: &wordcloud::ColorContext<'_>| {
            if context.rank == 0 {
                Rgba([1, 2, 3, 255])
            } else {
                Rgba([4, 5, 6, 255])
            }
        })
        .build()
        .unwrap();
    let rendered = custom
        .generate_detailed_from_frequencies([("rust", 2), ("cloud", 1)])
        .unwrap();
    assert_eq!(rendered.words()[0].color, Rgba([1, 2, 3, 255]));
}

#[test]
fn invalid_configuration_and_input_return_errors() {
    assert!(matches!(
        WordCloud::builder().dimensions(0, 10).build(),
        Err(WordCloudError::InvalidParameter {
            parameter: "width",
            ..
        })
    ));
    assert!(matches!(
        WordCloud::builder().prefer_horizontal(f32::NAN).build(),
        Err(WordCloudError::InvalidParameter {
            parameter: "prefer_horizontal",
            ..
        })
    ));
    assert!(matches!(
        WordCloud::builder().palette([]).build(),
        Err(WordCloudError::InvalidParameter {
            parameter: "palette",
            ..
        })
    ));
    assert!(matches!(
        WordCloud::builder().font_bytes(b"not a font").build(),
        Err(WordCloudError::InvalidFont { .. })
    ));
    assert!(matches!(
        WordCloud::builder()
            .max_font_size(f32::MAX)
            .scale(2)
            .build(),
        Err(WordCloudError::InvalidParameter {
            parameter: "max_font_size",
            ..
        })
    ));
    assert!(matches!(
        Mask::from_predicate(u32::MAX, u32::MAX, |_, _| true),
        Err(WordCloudError::CanvasTooLarge { .. })
    ));
    assert!(matches!(
        WordCloud::builder()
            .dimensions(32, 32)
            .max_font_size(100.0)
            .font_step(1.0e-8)
            .build(),
        Err(WordCloudError::InvalidParameter {
            parameter: "font_step",
            ..
        })
    ));
    assert!(matches!(
        WordCloud::builder().dimensions(4_001, 4_000).build(),
        Err(WordCloudError::CanvasTooLarge { .. })
    ));

    let cloud = WordCloud::builder().build().unwrap();
    assert!(matches!(
        cloud.generate_from_frequencies([("rust", f64::NAN)]),
        Err(WordCloudError::InvalidFrequency { .. })
    ));
    assert!(matches!(
        cloud.generate("the and or"),
        Err(WordCloudError::EmptyInput)
    ));
    assert!(matches!(
        cloud.generate_from_frequencies([("x".repeat(257), 1)]),
        Err(WordCloudError::InvalidWord { .. })
    ));
}

#[test]
fn repeat_pads_with_downweighted_duplicate_words() {
    let background = Rgba([255, 255, 255, 255]);
    let frequencies = [("rust", 100), ("cloud", 10)];
    let build = |repeat: bool| {
        WordCloud::builder()
            .dimensions(480, 280)
            .max_words(6)
            .repeat(repeat)
            .background_color(background)
            .random_seed(13)
            .build()
            .unwrap()
            .generate_detailed_from_frequencies(frequencies)
            .unwrap()
    };

    let plain = build(false);
    assert!(plain.words().len() <= 2);

    let repeated = build(true);
    assert!(repeated.words().len() > plain.words().len());
    // Duplicates of the same word carry strictly decreasing frequencies.
    let rust_frequencies: Vec<f64> = repeated
        .words()
        .iter()
        .filter(|word| word.word == "rust")
        .map(|word| word.frequency)
        .collect();
    assert!(rust_frequencies.len() >= 2);
    assert!(rust_frequencies
        .windows(2)
        .all(|pair| pair[0] > pair[1]));
    // Repeated words never render larger than their first occurrence.
    for word in repeated.words() {
        let first_font_size = repeated
            .words()
            .iter()
            .find(|other| other.word == word.word)
            .map(|other| other.font_size)
            .unwrap();
        assert!(word.font_size <= first_font_size + f32::EPSILON);
    }
}

#[test]
fn unsupported_words_do_not_consume_max_words_slots() {
    let cloud = WordCloud::builder()
        .dimensions(300, 180)
        .max_words(1)
        .build()
        .unwrap();
    let rendered = cloud
        .generate_detailed_from_frequencies([("词云", 10), ("rust", 5)])
        .unwrap();
    assert_eq!(rendered.words().len(), 1);
    assert_eq!(rendered.words()[0].word, "rust");
}

#[test]
fn fully_blocked_mask_and_missing_font_path_are_typed_errors() {
    let mask = Mask::from_predicate(100, 80, |_, _| false).unwrap();
    let cloud = WordCloud::builder().mask(mask).build().unwrap();
    assert!(matches!(
        cloud.generate_from_frequencies([("rust", 1)]),
        Err(WordCloudError::NoWordsPlaced { attempted: 1 })
    ));

    let missing = std::env::temp_dir().join("wordcloud-font-that-does-not-exist.ttf");
    assert!(matches!(
        WordCloud::builder().font_path(missing).build(),
        Err(WordCloudError::FontRead { .. })
    ));
}

#[test]
fn embedded_font_bytes_and_transparent_background_work() {
    let bytes = include_bytes!("../assets/OpenSans-Regular.ttf");
    let cloud = WordCloud::builder()
        .dimensions(240, 140)
        .font_bytes(bytes)
        .transparent_background()
        .solid_color(Rgba([20, 80, 160, 255]))
        .build()
        .unwrap();
    let image = cloud
        .generate_from_frequencies([("rust", 4), ("cloud", 2)])
        .unwrap();
    assert!(image.pixels().any(|pixel| pixel[3] == 0));
    assert!(image.pixels().any(|pixel| pixel[3] > 0));
    let dynamic = cloud
        .generate_dynamic_from_frequencies([("rust", 1)])
        .unwrap();
    assert_eq!((dynamic.width(), dynamic.height()), (240, 140));
}

#[test]
fn grayscale_mask_defaults_to_only_pure_white_blocked() {
    let image = GrayImage::from_raw(3, 1, vec![0, 254, 255]).unwrap();
    let mask = Mask::from_luma8(image);
    assert!(mask.is_allowed(0, 0));
    assert!(mask.is_allowed(1, 0));
    assert!(!mask.is_allowed(2, 0));
}

#[test]
fn png_save_can_be_read_back() {
    let cloud = WordCloud::builder().dimensions(240, 140).build().unwrap();
    let path = std::env::temp_dir().join(format!(
        "wordcloud-public-api-{}-{}.png",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    cloud
        .save_from_frequencies([("rust", 4), ("cloud", 2)], &path)
        .unwrap();
    let decoded = image::open(&path).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (240, 140));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn enabled_image_formats_save_and_decode() {
    let cloud = WordCloud::builder().dimensions(180, 100).build().unwrap();
    let rendered = cloud
        .generate_detailed_from_frequencies([("rust", 4), ("cloud", 2)])
        .unwrap();

    for extension in ["png", "jpg", "webp"] {
        let path = std::env::temp_dir().join(format!(
            "wordcloud-format-{}.{extension}",
            std::process::id()
        ));
        rendered
            .save(&path)
            .unwrap_or_else(|error| panic!("failed to save {extension}: {error:?}"));
        let decoded = image::open(&path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (180, 100));
        std::fs::remove_file(path).unwrap();
    }
}
