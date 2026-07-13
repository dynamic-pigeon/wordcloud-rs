use wordcloud::{Rgba, WordCloud};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "
        Rust empowers everyone to build reliable and efficient software.
        Rust combines performance, safety, fearless concurrency, and a
        productive toolchain. Memory safety without garbage collection makes
        Rust useful for systems, services, command line tools, WebAssembly,
        embedded software, graphics, and data processing. Rust Rust Rust.
    ";

    let cloud = WordCloud::builder()
        .dimensions(960, 540)
        .max_words(80)
        .max_font_size(112.0)
        .margin(3)
        .prefer_horizontal(0.82)
        .background_color(Rgba([248, 248, 246, 255]))
        .palette([
            Rgba([20, 78, 102, 255]),
            Rgba([26, 127, 112, 255]),
            Rgba([222, 139, 38, 255]),
            Rgba([180, 57, 62, 255]),
            Rgba([92, 72, 125, 255]),
        ])
        .random_seed(2026)
        .build()?;

    let rendered = cloud.generate_detailed(text)?;
    rendered.save("wordcloud.png")?;
    println!(
        "saved wordcloud.png ({}x{}, {} words)",
        rendered.image().width(),
        rendered.image().height(),
        rendered.words().len()
    );
    Ok(())
}
