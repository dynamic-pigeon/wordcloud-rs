use std::env;
use std::io;

use wordcloud::{Rgba, WordCloud};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let font_path = env::args_os().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --example chinese -- /path/to/chinese-font.ttf",
        )
    })?;

    // Chinese segmentation is deliberately external. These frequencies can
    // come from jieba-rs, a database, analytics, or any application logic.
    let frequencies = [
        ("词云", 100),
        ("Rust", 86),
        ("可视化", 72),
        ("性能", 58),
        ("安全", 54),
        ("确定性", 48),
        ("布局", 42),
        ("图像", 38),
        ("字体", 35),
        ("开源", 31),
        ("依赖", 28),
        ("数据", 24),
        ("并发", 20),
    ];

    let cloud = WordCloud::builder()
        .dimensions(900, 520)
        .font_path(font_path)
        .max_font_size(120.0)
        .prefer_horizontal(0.86)
        .background_color(Rgba([250, 250, 248, 255]))
        .random_seed(42)
        .build()?;
    cloud.save_from_frequencies(frequencies, "wordcloud-zh.png")?;
    println!("saved wordcloud-zh.png");
    Ok(())
}
