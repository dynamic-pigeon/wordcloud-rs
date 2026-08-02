use wordcloud::{Mask, Orientation, RenderedWordCloud, Rgba, WordCloud, WordCloudBuilder};

const FREQUENCIES: [(&str, u64); 12] = [
    ("optimization", 1_000),
    ("deterministic", 850),
    ("bitmap", 700),
    ("collision", 620),
    ("layout", 510),
    ("raster", 430),
    ("memory", 360),
    ("vector", 300),
    ("pixel", 250),
    ("cache", 200),
    ("thread", 160),
    ("render", 130),
];

fn builder(width: u32, height: u32) -> WordCloudBuilder {
    WordCloud::builder()
        .dimensions(width, height)
        .max_words(FREQUENCIES.len())
        .min_font_size(5.0)
        .max_font_size(64.0)
        .font_step(3.0)
        .relative_scaling(0.5)
        .solid_color(Rgba([31, 79, 113, 255]))
}

fn render(builder: WordCloudBuilder) -> RenderedWordCloud {
    builder
        .build()
        .unwrap()
        .generate_detailed_from_frequencies(FREQUENCIES)
        .unwrap()
}

fn add(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn fingerprint(rendered: &RenderedWordCloud) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    add(&mut hash, &rendered.image().width().to_le_bytes());
    add(&mut hash, &rendered.image().height().to_le_bytes());
    add(&mut hash, rendered.image().as_raw());
    add(&mut hash, &(rendered.words().len() as u64).to_le_bytes());
    for word in rendered.words() {
        add(&mut hash, &(word.word.len() as u64).to_le_bytes());
        add(&mut hash, word.word.as_bytes());
        add(&mut hash, &word.frequency.to_bits().to_le_bytes());
        add(
            &mut hash,
            &word.normalized_frequency.to_bits().to_le_bytes(),
        );
        add(&mut hash, &(word.rank as u64).to_le_bytes());
        add(&mut hash, &word.x.to_le_bytes());
        add(&mut hash, &word.y.to_le_bytes());
        add(&mut hash, &word.width.to_le_bytes());
        add(&mut hash, &word.height.to_le_bytes());
        add(&mut hash, &word.font_size.to_bits().to_le_bytes());
        add(
            &mut hash,
            &[match word.orientation {
                Orientation::Horizontal => 0,
                Orientation::Vertical => 1,
            }],
        );
        add(&mut hash, &word.color.0);
    }
    hash
}

fn orientation_counts(rendered: &RenderedWordCloud) -> (usize, usize) {
    let horizontal = rendered
        .words()
        .iter()
        .filter(|word| word.orientation == Orientation::Horizontal)
        .count();
    (horizontal, rendered.words().len() - horizontal)
}

#[test]
fn optimized_pipeline_matches_reference_fingerprints() {
    // Baselines include the center-biased random placement of the first word.
    let mut actual = Vec::new();
    let horizontal = render(
        builder(321, 179)
            .margin(0)
            .prefer_horizontal(1.0)
            .random_seed(11),
    );
    assert_eq!(orientation_counts(&horizontal), (12, 0));
    actual.push(("horizontal_margin_0", fingerprint(&horizontal)));

    let mixed = render(
        builder(333, 211)
            .margin(2)
            .prefer_horizontal(0.45)
            .random_seed(22),
    );
    assert_eq!(orientation_counts(&mixed), (7, 5));
    actual.push(("mixed_margin_2", fingerprint(&mixed)));

    let vertical = render(
        builder(257, 331)
            .margin(2)
            .prefer_horizontal(0.0)
            .random_seed(33),
    );
    assert_eq!(orientation_counts(&vertical), (0, 12));
    actual.push(("vertical_margin_2", fingerprint(&vertical)));

    let mask = Mask::from_predicate(127, 83, |x, y| {
        let inside = x > 2 && y > 1 && x < 124 && y < 81;
        let notch = (47..80).contains(&x) && y < 27;
        inside && !notch
    })
    .unwrap();
    actual.push((
        "mask_scale_3",
        fingerprint(&render(
            builder(1, 1)
                .mask(mask)
                .scale(3)
                .margin(1)
                .prefer_horizontal(0.6)
                .random_seed(44),
        )),
    ));
    actual.push((
        "transparent_alpha",
        fingerprint(&render(
            builder(289, 173)
                .transparent_background()
                .solid_color(Rgba([190, 43, 71, 153]))
                .margin(2)
                .prefer_horizontal(0.75)
                .random_seed(55),
        )),
    ));

    for margin in [1, 2, 8, 9] {
        let name = match margin {
            1 => "margin_1",
            2 => "margin_2",
            8 => "margin_8",
            9 => "margin_9",
            _ => unreachable!(),
        };
        actual.push((
            name,
            fingerprint(&render(
                builder(347, 229)
                    .margin(margin)
                    .prefer_horizontal(0.5)
                    .random_seed(66),
            )),
        ));
    }

    let expected: [(&str, u64); 9] = [
        ("horizontal_margin_0", 0x3406_5907_6642_0690),
        ("mixed_margin_2", 0x798a_f137_9915_3b18),
        ("vertical_margin_2", 0x6366_45ef_8b52_8e2c),
        ("mask_scale_3", 0xe847_9e4f_af9b_2a3f),
        ("transparent_alpha", 0x2844_9987_6f51_4469),
        ("margin_1", 0xd638_e456_ee21_adec),
        ("margin_2", 0x9d26_dfcf_6487_2097),
        ("margin_8", 0xdf52_fbc0_029a_e69d),
        ("margin_9", 0xee22_f30e_20c5_a8ee),
    ];
    assert_eq!(actual, expected);
}

#[test]
fn fully_transparent_words_keep_the_opaque_layout() {
    let opaque = builder(311, 187)
        .transparent_background()
        .solid_color(Rgba([90, 120, 170, 255]))
        .prefer_horizontal(0.55)
        .random_seed(88)
        .build()
        .unwrap()
        .generate_detailed_from_frequencies(FREQUENCIES)
        .unwrap();
    let invisible = builder(311, 187)
        .transparent_background()
        .solid_color(Rgba([90, 120, 170, 0]))
        .prefer_horizontal(0.55)
        .random_seed(88)
        .build()
        .unwrap()
        .generate_detailed_from_frequencies(FREQUENCIES)
        .unwrap();

    assert_eq!(opaque.words().len(), invisible.words().len());
    for (opaque, invisible) in opaque.words().iter().zip(invisible.words()) {
        assert_eq!(opaque.word, invisible.word);
        assert_eq!(opaque.frequency, invisible.frequency);
        assert_eq!(opaque.normalized_frequency, invisible.normalized_frequency);
        assert_eq!(opaque.rank, invisible.rank);
        assert_eq!((opaque.x, opaque.y), (invisible.x, invisible.y));
        assert_eq!(
            (opaque.width, opaque.height),
            (invisible.width, invisible.height)
        );
        assert_eq!(opaque.font_size, invisible.font_size);
        assert_eq!(opaque.orientation, invisible.orientation);
    }
    assert!(invisible
        .image()
        .pixels()
        .all(|pixel| *pixel == Rgba([0, 0, 0, 0])));
}
