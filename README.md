# wordcloud

一个纯 Rust 的词云生成库。它提供与 Python `wordcloud` 相近的核心工作流：从文本或词频生成布局，按频率缩放字号，在 mask 中避碰放置，支持横排/90 度竖排、固定随机种子、自定义字体和颜色，并输出 `image::RgbaImage` 或直接保存图片。

库内嵌 Open Sans，因此英文开箱可用，不依赖操作系统字体。中文等字符需要传入覆盖相应字符的 TTF/OTF/TTC 字体；中文分词应在应用层完成，再把词频交给库。

## 作为依赖使用

开发期可直接使用路径依赖：

```toml
[dependencies]
wordcloud = { path = "../wordcloud" }
```

从自然文本生成：

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

从词频生成，整数和浮点权重都可以直接使用：

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

`generate_detailed` 和 `generate_detailed_from_frequencies` 返回 `RenderedWordCloud`，其中同时包含 `RgbaImage` 与每个词的坐标、字号、方向、颜色和归一化频率，适合制作交互热点或做进一步合成。`generate` / `generate_from_frequencies` 返回 `RgbaImage`；需要 `DynamicImage` 时可使用对应的 `generate_dynamic*` 方法。

## Mask 与颜色

`Mask::from_luma8` 与 Python 的常用语义一致：纯白（255）像素禁止放置，其他像素可用。需要抗锯齿阈值或反转黑白时，可以使用 `from_luma8_with_threshold` 和 `MaskPolarity`。也可以用谓词直接定义形状：

```rust
use wordcloud::{Mask, Rgba, WordCloud};

fn main() -> Result<(), wordcloud::WordCloudError> {
    let mask = Mask::from_predicate(600, 600, |x, y| {
        let dx = x as i64 - 300;
        let dy = y as i64 - 300;
        dx * dx + dy * dy < 270 * 270
    })?;

    let _cloud = WordCloud::builder()
        .mask(mask) // mask 的尺寸决定画布尺寸
        .palette([
            Rgba([25, 97, 128, 255]),
            Rgba([37, 139, 119, 255]),
            Rgba([214, 151, 56, 255]),
        ])
        .build()?;
    Ok(())
}
```

还可以通过 `solid_color` 使用单色，或通过 `colorizer` 闭包根据词、频率、位置、方向和稳定随机值动态配色。透明背景使用 `transparent_background()`。

## 行为说明

- 默认每次生成都会使用新的随机种子，最大词也会在画布中心附近随机起步，因此复用同一个 `WordCloud` 生成相同输入仍会得到不同布局。需要可复现结果时可显式设置 `random_seed`；此时在依赖版本、配置、字体与输入相同的前提下，布局和像素完全相同。调用 `randomize_each_generation()` 可以恢复默认随机模式。自定义 tokenizer/colorizer 也需要是确定性的，才能维持固定 seed 的保证。
- 显式词频输入会合并重复词并绕过 tokenizer、大小写转换与 stopwords。
- 可用词数不足 `max_words` 时，开启 `repeat(true)` 会把已有词按最小归一化频率逐轮降权重复填充（与 Python `wordcloud` 的 `repeat` 语义一致），重复词的字号会越来越小；默认关闭。
- 文本入口采用 Unicode word boundary 和英文 stopwords。中文、日文等需要语言分词的文本，建议先用 `jieba-rs` 等工具分词并传词频。
- 使用 `font_bytes` 可以嵌入应用自己的字体；TTC 字体通过 `font_index` 选择 face。
- 字形栅格化基于 `ab_glyph`。当前版本适合 Latin、CJK 等无需复杂 shaping 的文本；阿拉伯文、部分印度文字和彩色 emoji 的高级 shaping/彩色字形尚未实现。
- `scale(n)` 会把画布、字号、margin 和 mask 同步放大，并在物理像素上重新布局和碰撞；输出尺寸为 `width*n` x `height*n`。
- 默认单词长度上限为 256 个 Unicode 字符，可通过 `max_word_length` 调整（硬上限 4096）；输出画布上限为 1600 万像素，以避免依赖端意外的大额内存分配。
- `save` 根据扩展名支持 PNG、JPEG 和 WebP；未知或未启用的格式会返回 `WordCloudError::Image`。

## 示例与验证

```bash
cargo run --example basic
cargo run --example chinese -- /path/to/chinese-font.ttf
cargo test
```

## 性能比较

仓库包含与 Python `wordcloud` 的可复现完整图片生成 benchmark，运行方法和比较口径见
[benchmarks/README.md](benchmarks/README.md)。测量结果仅在本地生成，不纳入仓库。

默认字体 `assets/OpenSans-Regular.ttf` 的许可见 `assets/OFL.txt`。库代码使用 MIT License。
