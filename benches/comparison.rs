use std::env;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use wordcloud::{Mask, Rgba, WordCloud};

const SEED: u64 = 42;
const PREFIXES: [&str; 16] = [
    "adaptive",
    "atomic",
    "binary",
    "cloud",
    "compute",
    "concurrent",
    "data",
    "dynamic",
    "efficient",
    "fast",
    "graphic",
    "indexed",
    "layout",
    "memory",
    "parallel",
    "pixel",
];
const SUFFIXES: [&str; 16] = [
    "engine", "buffer", "system", "render", "vector", "matrix", "service", "runtime", "module",
    "stream", "kernel", "thread", "cache", "image", "signal", "model",
];

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    width: u32,
    height: u32,
    candidates: usize,
    min_font_size: f32,
    max_font_size: f32,
    relative_scaling: f32,
    masked: bool,
}

struct RenderMetrics {
    placed: usize,
    min_font_size: f32,
    max_font_size: f32,
    font_size_sum: f64,
    ink_pixels: usize,
}

const CASES: [Case; 5] = [
    Case {
        name: "fixed_sparse_400x200",
        width: 400,
        height: 200,
        candidates: 20,
        min_font_size: 12.0,
        max_font_size: 12.0,
        relative_scaling: 0.0,
        masked: false,
    },
    Case {
        name: "fixed_standard_800x400",
        width: 800,
        height: 400,
        candidates: 100,
        min_font_size: 10.0,
        max_font_size: 10.0,
        relative_scaling: 0.0,
        masked: false,
    },
    Case {
        name: "fixed_dense_1200x600",
        width: 1200,
        height: 600,
        candidates: 200,
        min_font_size: 10.0,
        max_font_size: 10.0,
        relative_scaling: 0.0,
        masked: false,
    },
    Case {
        name: "fixed_mask_800x600",
        width: 800,
        height: 600,
        candidates: 100,
        min_font_size: 10.0,
        max_font_size: 10.0,
        relative_scaling: 0.0,
        masked: true,
    },
    Case {
        name: "weighted_800x400",
        width: 800,
        height: 400,
        candidates: 100,
        min_font_size: 4.0,
        max_font_size: 128.0,
        relative_scaling: 0.5,
        masked: false,
    },
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `cargo test --all-targets` invokes harness-free benches without `--bench`.
    if env::args_os().len() == 1 {
        return Ok(());
    }
    let (warmup, samples) = parse_args()?;

    for case in CASES {
        let frequencies = frequencies(case.candidates);
        let checksum = workload_checksum(&frequencies);
        let mask_checksum = mask_checksum(case);
        let cloud = build_cloud(case)?;

        let mut reference_render = None;
        for _ in 0..warmup {
            let rendered = generate(&cloud, &frequencies)?;
            validate_render(&rendered, case)?;
            reference_render.get_or_insert_with(|| render_metrics(&rendered));
            black_box((rendered.words().len(), rendered.image().as_raw().len()));
        }

        let mut measurements = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            let rendered = generate(&cloud, &frequencies)?;
            let elapsed_ns = started.elapsed().as_nanos();
            validate_render(&rendered, case)?;
            let placed = rendered.words().len();
            black_box((placed, rendered.image().as_raw().len()));
            measurements.push((elapsed_ns, placed));
        }

        println!(
            "RUST_CASE\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{checksum:016x}\t{mask_checksum}",
            case.name,
            case.width,
            case.height,
            case.candidates,
            case.min_font_size,
            case.max_font_size,
            case.relative_scaling,
            u8::from(case.masked),
        );
        let reference_render = reference_render.expect("warmup must run at least once");
        println!(
            "RUST_RENDER\t{}\t{}\t{}\t{}\t{}\t{}",
            case.name,
            reference_render.placed,
            reference_render.min_font_size,
            reference_render.max_font_size,
            reference_render.font_size_sum,
            reference_render.ink_pixels,
        );
        for (elapsed_ns, placed) in measurements {
            println!("RUST_SAMPLE\t{}\t{elapsed_ns}\t{placed}", case.name);
        }
    }

    Ok(())
}

fn parse_args() -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut warmup = 3;
    let mut samples = 15;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--bench" {
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {argument}"))?;
        let parsed: usize = value.parse()?;
        if parsed == 0 {
            return Err(format!("{argument} must be at least 1").into());
        }
        match argument.as_str() {
            "--warmup" => warmup = parsed,
            "--samples" => samples = parsed,
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    Ok((warmup, samples))
}

fn build_cloud(case: Case) -> Result<WordCloud, Box<dyn std::error::Error>> {
    let font_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("OpenSans-Regular.ttf");
    let mut builder = WordCloud::builder()
        .dimensions(case.width, case.height)
        .scale(1)
        .max_words(case.candidates)
        .min_font_size(case.min_font_size)
        .max_font_size(case.max_font_size)
        .font_step(2.0)
        .margin(2)
        .prefer_horizontal(1.0)
        .relative_scaling(case.relative_scaling)
        .random_seed(SEED)
        .font_path(font_path)
        .background_color(Rgba([255, 255, 255, 255]))
        .solid_color(Rgba([0, 0, 0, 255]));

    if case.masked {
        let width = case.width;
        let height = case.height;
        let mask = Mask::from_predicate(width, height, move |x, y| {
            mask_pixel_is_allowed(x, y, width, height)
        })?;
        builder = builder.mask(mask);
    }

    Ok(builder.build()?)
}

fn generate(
    cloud: &WordCloud,
    frequencies: &[(String, u64)],
) -> Result<wordcloud::RenderedWordCloud, wordcloud::WordCloudError> {
    cloud.generate_detailed_from_frequencies(
        frequencies
            .iter()
            .map(|(word, frequency)| (word.as_str(), *frequency)),
    )
}

fn validate_render(
    rendered: &wordcloud::RenderedWordCloud,
    case: Case,
) -> Result<(), Box<dyn std::error::Error>> {
    if rendered.image().dimensions() != (case.width, case.height) {
        return Err(format!(
            "{} produced {:?}, expected {}x{}",
            case.name,
            rendered.image().dimensions(),
            case.width,
            case.height
        )
        .into());
    }
    if rendered.words().is_empty() || rendered.words().len() > case.candidates {
        return Err(format!(
            "{} placed an invalid number of words: {}",
            case.name,
            rendered.words().len()
        )
        .into());
    }
    Ok(())
}

fn render_metrics(rendered: &wordcloud::RenderedWordCloud) -> RenderMetrics {
    let min_font_size = rendered
        .words()
        .iter()
        .map(|word| word.font_size)
        .reduce(f32::min)
        .expect("a validated render has at least one word");
    let max_font_size = rendered
        .words()
        .iter()
        .map(|word| word.font_size)
        .reduce(f32::max)
        .expect("a validated render has at least one word");
    let font_size_sum = rendered
        .words()
        .iter()
        .map(|word| f64::from(word.font_size))
        .sum();
    let ink_pixels = rendered
        .image()
        .pixels()
        .filter(|pixel| pixel.0 != [255, 255, 255, 255])
        .count();
    RenderMetrics {
        placed: rendered.words().len(),
        min_font_size,
        max_font_size,
        font_size_sum,
        ink_pixels,
    }
}

fn frequencies(count: usize) -> Vec<(String, u64)> {
    assert!(count <= PREFIXES.len() * SUFFIXES.len());
    (0..count)
        .map(|index| {
            let word = format!(
                "{}{}",
                PREFIXES[index % PREFIXES.len()],
                SUFFIXES[index / PREFIXES.len()]
            );
            let frequency = 1_000_000 / (index as u64 + 1);
            (word, frequency)
        })
        .collect()
}

fn workload_checksum(frequencies: &[(String, u64)]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for (word, frequency) in frequencies {
        for byte in format!("{word}={frequency}\n").bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn mask_checksum(case: Case) -> String {
    if !case.masked {
        return "none".to_owned();
    }
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for y in 0..case.height {
        for x in 0..case.width {
            let byte = if mask_pixel_is_allowed(x, y, case.width, case.height) {
                0
            } else {
                255
            };
            hash ^= byte;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

fn mask_pixel_is_allowed(x: u32, y: u32, width: u32, height: u32) -> bool {
    let width = i64::from(width);
    let height = i64::from(height);
    let dx = i64::from(x) * 2 + 1 - width;
    let dy = i64::from(y) * 2 + 1 - height;
    let inside_ellipse =
        dx * dx * height * height + dy * dy * width * width <= width * width * height * height;
    let top_notch = dx.abs() < width / 6 && dy < -height / 3;
    inside_ellipse && !top_notch
}
