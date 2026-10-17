# wordcloud

简体中文 | [English](README_EN.md)

一个纯 Rust 的词云生成库，以 Python `wordcloud` 1.9.6 为行为基准：从文本或词频生成布局，按频率缩放字号，在 mask 中避碰放置，支持横排/90 度竖排、固定随机种子、自定义字体和颜色，并输出 `image::RgbaImage` 或直接保存图片。默认值与算法语义（分词、collocations、字号阶梯、放置采样）与 Python 版本对齐，剩余差异见下文"与 Python 的已知差异"。

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

本库以 Python `wordcloud` 1.9.6 为行为基准；以下条目按其源码对齐，并列出尚未对齐的差异。

- 默认每次生成都会使用新的随机种子。需要可复现结果时可显式设置 `random_seed`；此时在依赖版本、配置、字体与输入相同的前提下，布局和像素完全相同。调用 `randomize_each_generation()` 可以恢复默认随机模式。自定义 tokenizer/colorizer 也需要是确定性的，才能维持固定 seed 的保证。
- 显式词频输入会合并重复词并绕过 tokenizer、大小写处理与 stopwords。
- 文本入口复刻 Python 的 `process_text` 流程：`\w[\w']*` 正则语义分词（撇号保留在词内，如 `don't`；CJK 连续串为单个 token）、剥离词尾 `'s`（`that's` → `that`）、默认过滤纯数字词（`include_numbers(true)` 保留）、默认开启 collocations（相邻二元组的 Dunning 对数似然分数超过 30 时提升为搭配词，如 `new york`）、默认合并复数（`words` 并入 `word`，`ss` 结尾除外）。
- 大小写处理与 Python 一致：同一词的各种写法按出现次数融合，显示最常见写法（平局取先出现的写法）；`lowercase(true)` 可退回全小写合并。默认 stopwords 为 Python 包内置的 192 词英文表（含 `com`、`www`、`http` 等）。
- 字号阶梯与 Python 一致：`relative_scaling` 默认 auto（常规 0.5，`repeat` 时 0），字号按与上一词的频率比值更新（Python 的 round-half-even 取整）；未显式设置 `max_font_size` 时按 Python 的自适应规则——单词直接取画布高度，多词时先用画布高度试放前两个词再取两者调和平均；某个词在 `min_font_size` 下仍放不下时停止放置后续词。默认 `font_step = 1`。
- 放置搜索与 Python 的 `query_integral_image` 等价：枚举全部完全空闲的候选窗口并均匀随机取一个（不再中心偏置），词间净空约 `margin/2` 每侧（与 Python 的窗口加 margin 语义一致；奇数 margin 向下取整）。
- 可用词数不足 `max_words` 时，开启 `repeat(true)` 会把已有词按最小归一化频率逐轮降权重复填充（与 Python `repeat` 语义一致），重复词的字号会越来越小；默认关闭。
- 中文等需要分词的文本仍建议先在应用层用 `jieba-rs` 等分词后传词频（默认分词器会把 CJK 连续串当成单个 token，与 Python 相同）。
- 使用 `font_bytes` 可以嵌入应用自己的字体；TTC 字体通过 `font_index` 选择 face。
- 字形栅格化基于 `ab_glyph`。当前版本适合 Latin、CJK 等无需复杂 shaping 的文本；阿拉伯文、部分印度文字和彩色 emoji 的高级 shaping/彩色字形尚未实现。
- `scale(n)` 在物理像素上重新布局和碰撞（比 Python 的"逻辑布局、放大渲染"更精细）；输出尺寸为 `width*n` x `height*n`。
- 默认单词长度上限为 256 个 Unicode 字符，可通过 `max_word_length` 调整（硬上限 4096）；输出画布上限为 1600 万像素，以避免依赖端意外的大额内存分配。
- `save` 根据扩展名支持 PNG、JPEG 和 WebP；未知或未启用的格式会返回 `WordCloudError::Image`。JPEG 不保留 alpha：透明背景保存为 JPEG 时会直接丢弃 alpha 通道，需要透明度请使用 PNG 或 WebP。

**与 Python 的已知差异**：默认配色为内置 6 色 Palette（Python 默认是 viridis 色表随机采样，可传入自定义 `colorizer` 或 `palette` 实现）；默认背景为黑色（与 Python 相同）；方向重试不重复 Python 中"竖排失败后再试一次竖排"的冗余探测（可观察行为等价）；`mask` 反走样阈值、轮廓线（contour）、SVG 输出等未实现。

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
