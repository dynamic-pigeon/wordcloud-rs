# Rust / Python performance comparison

这个基准比较当前 Rust 库与 Python [`wordcloud`](https://pypi.org/project/wordcloud/)
生成一张完整内存 RGBA 图片所需的时间。它不是 tokenizer benchmark，也不包含 PNG 编码。

## 运行

需要 Rust 工具链和 Python 3.12。Python 依赖已经锁定：

```bash
python3.12 -m venv .venv-benchmark
.venv-benchmark/bin/python -m pip install -r benchmarks/requirements.txt
.venv-benchmark/bin/python benchmarks/compare.py \
  --warmup 5 \
  --samples 20 \
  --output benchmarks/results/local.json
```

运行器会用 Cargo 的 `bench` profile 构建 Rust 目标。Rust 样本分成两半，分别在
全部 Python 样本前后执行，以降低固定执行顺序带来的偏差。NumPy/OpenBLAS 相关线程数
被限制为 1；两端每个样本都生成 1 张图片。

## 统一口径

- 两端读取同一个 `assets/OpenSans-Regular.ttf`。
- 输入为严格降序的显式词频，绕过两端不同的 tokenizer、stopwords 和 collocation 逻辑。
- 词频表和 mask 都计算 FNV-1a 校验值；不一致时运行直接失败。
- 生成器在计时外构造并复用，mask 构造不计时。Rust 在 `build()` 时解析字体，
  因而解析不在热路径内；Python 构造器只保存字体路径，Pillow 字体创建仍是其生成路径的一部分。
- Python 的完整端点是 `generate_from_frequencies(...).to_image()`。只调用前者不会绘制最终彩色图片，不能与 Rust 返回完整 `RgbaImage` 的接口直接比较。
- 统一使用 RGBA、单色、`scale=1`、`font_step=2`、`margin=2`、纯横排和 seed 42。
- 四个主场景固定 `min_font_size == max_font_size` 且 `relative_scaling=0`，运行器要求
  双方放置全部词，并校验最小/最大字号与字号总和完全相同。另有一个
  `relative_scaling=0.5` 的加权场景，用来观察两库各自字号算法下的真实 API 行为。
- Python RNG 在每轮计时前恢复到相同状态；Rust 本身会在每次生成时从 seed 重新初始化 RNG。
- 每轮验证输出尺寸、格式和实际放置词数；预热输出还记录字号范围、字号总和与非背景像素数。
  报告以中位数为主，同时保留 P95、MAD、标准差和所有原始样本。
- 结果 JSON 记录库源码、两端 harness、字体和依赖锁的 SHA-256；即使测量发生在未提交工作树中，也能核对实际输入文件。

固定字号场景覆盖 400x200 的 20 词稀疏画布、800x400 的 100 词标准画布、
1200x600 的 200 词密集画布，以及 800x600 的 100 词非矩形 mask 画布。
加权场景使用 800x400 画布和 100 个词。

## 如何解读

这是库级端到端性能比较，不是相同算法的 Rust/Python 语言微基准。Python 使用
Pillow/FreeType 栅格化和积分图矩形搜索；当前 Rust 库使用 `fontdue`、紧凑 `u64`
位图和精确 glyph alpha 碰撞，并限制候选位置搜索次数。Python 在布局后还会在
`to_image()` 中重新栅格化文字，而 Rust 复用本轮布局生成的 alpha 位图完成混合。

因此，即使字体、词频和主要参数一致，两端也不会产生相同坐标或像素。固定 seed
只保证各自重复运行稳定，不能统一不同 RNG 和搜索算法。最终结果还应结合放置词数判断；
如果某个自定义负载中两端放置数量或字号不同，不能只看总耗时比。即使固定字号完全
一致，FreeType 与 `fontdue` 的字形边界和覆盖像素也不同，结果仍是端点比较而非同算法比较。

测量结果写入 `--output` 指定的 JSON 文件。`benchmarks/results/` 已被 Git 忽略，
用于保存本机临时结果。
