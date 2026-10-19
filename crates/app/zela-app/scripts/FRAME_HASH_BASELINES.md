# 帧像素序列哈希基线（跨运行时对拍口径）

> 用途：WASM 第二阶段的 `native vs 浏览器` 画面对拍。
> **不覆盖 mp4 / PNG 容器**，只覆盖帧缓冲像素 ⇒ 浏览器侧无需导出、无需 ffmpeg。

## 算法（V1）

实现：[`zela_scene_bundle::frame_hash::FrameSequenceHash`](../../../scene/zela-scene-bundle/src/frame_hash.rs)

- 像素：RGBA8、行优先、左上原点、行距 `width*4`、直色（非预乘）；
- `state0 = SHA-256("XCAL-FRAME-SEQ-V1" || width_le_u32 || height_le_u32)`；
- `state_{i+1} = SHA-256(state_i || i_le_u32 || SHA-256(frame_i))`；
- 输出 `hex(state_n)`（小写 64 字符）。

尺寸进入初始状态；像素格式不进入 —— 两端都必须先规范化到上述 RGBA8
（native 用 `zela_app::export::to_rgba`，浏览器按回读纹理的通道序做同样转换）。

## 复现命令

```bash
# 1) 编译 ZSB（可选：--script 与 --bundle 两条路径都要跑一遍）
cargo run -q -p zela-compile --no-default-features --features typst -- \
  crates/app/zela-app/scripts/fourier.lua --out /tmp/fourier.zsb

# 2) 渲染并输出序列哈希（--frames 表示不依赖 ffmpeg；--hash-out 可选）
cargo run -q -p zela-app --no-default-features --features typst --bin zela-render -- \
  --script crates/app/zela-app/scripts/fourier.lua \
  --frames /tmp/f_s --fps 30 --size 640x480 --hash-out /tmp/f_s.hash

cargo run -q -p zela-app --no-default-features --features typst --bin zela-render -- \
  --bundle /tmp/fourier.zsb \
  --frames /tmp/f_b --fps 30 --size 640x480 --hash-out /tmp/f_b.hash
```

标准错误输出：

```text
[frame-hash] sequence = <64 hex>（N 帧，640×480，RGBA8）
```

本机自动对拍：

```bash
cargo verify --step smoke    # 短对拍：smoke.lua，15 帧 @ 320×240
cargo verify --step hashes   # 完整对拍：fourier 216 帧 + storyboard 135 帧 @ 640×480
```

CI 分层（`.github/workflows/architecture.yml` 的 `hashes` job）：

| 触发 | 步骤 | 说明 |
|---|---|---|
| PR | `cargo verify --step smoke --skip-baseline` | 快速反馈，覆盖 `--script` / `--bundle`（同机自洽） |
| dev 等分支 push | `cargo verify --step smoke --skip-baseline` | 快速反馈（同机自洽） |
| main / master push | `cargo verify --step hashes --skip-baseline` | 完整对拍（同机自洽） |
| nightly（cron） | `cargo verify --step hashes --skip-baseline` | 完整对拍（同机自洽） |
| 手动 dispatch | `cargo verify --step hashes --skip-baseline` | 完整对拍（同机自洽） |

> **短对拍是 smoke test，不是完整基线的等价替代**：已知跨端分歧首次出现在
> fourier 第 103 帧，smoke 只渲前 15 帧 ⇒ 它绿不代表跨端确定性没问题；
> 完整基线必须在 main push / nightly 跑。
>
> **基线是设备相关的**：CI 的托管 runner 无 GPU（安装 lavapipe 软件 Vulkan），
> 因此 CI 只断言 `--script == --bundle`（`--skip-baseline`），**不比对**本文件的
> 哈希；基线回归由录制设备（RTX 4060）上的 `cargo verify` 负责。

## 基线（2026-10-19，Windows / RTX 4060 Laptop）

**哈希值的唯一真源是 [`baselines.txt`](baselines.txt)**；本文件只解释口径，
不重复写哈希。三个基线**都要保留**，来源不同、用途不同：

| 段 | 端 | 作品 | 帧数 / 分辨率 | 来源 | 自动比对 |
|---|---|---|---|---|---|
| `[native]` | native（Vulkan headless） | fourier | 216 / 640×480 | `zela-render --script/--bundle` | ✅ 不符即 fail |
| `[native]` | native（Vulkan headless） | storyboard | 135 / 640×480 | 同上 | ✅ 不符即 fail |
| `[smoke]` | native（Vulkan headless） | smoke | 15 / 320×240 | 同上（PR / `--fast` 快速反馈） | ✅ 不符即 fail，但**不等价于完整基线** |
| `[browser]` | 浏览器（Chrome WebGPU） | fourier | 216 / 640×480 | `zela-web` + `?compare=1` | ❌ **仅留档** |

> `[browser]` 段**绝不参与自动比对**：本机跑不出浏览器产物，
> 纳入比对只会让本机永远红，并诱导人去"修"一个不该修的东西。

> **两者不同是已知可接受项**，不是缺陷、不是回归。原因见下节：
> 单帧 1 个像素的 GPU/求值末位差异，被 SHA-256 链放大成整串不同。

`--script` 与 `--bundle` 两条路径在同一分辨率 / 帧率下**逐帧像素一致**，
序列哈希因而相同（已在本机复跑核对，差值 0）。

独立复核：用 Python + Pillow 解码落盘的 PNG（`Image.tobytes()` 即紧凑 RGBA8），
按同一 V1 公式复算，得到与上表完全相同的两个值 —— 证明该值只依赖帧像素，
与导出容器 / 编码器无关。

## 浏览器侧如何得到序列哈希

1. 每帧渲染完成后从目标纹理回读像素（或直接取 WebGPU canvas 的 readback 结果）；
2. 去掉行对齐填充（紧凑 `width*4`），按与 native **相同的目标格式**做通道规范化
   （本项目 headless 目标格式为 `Bgra8UnormSrgb`，`to_rgba` 只做 R/B 交换）；
3. 对每帧调用同一个 `FrameSequenceHash::push_frame(rgba)`（该模块在
   `zela-scene-bundle` 中，是纯 Rust、已在 wasm gate 内）；
4. 最后一帧后取 `hex()`。

> **格式前提**：V1 只做通道顺序规范化（不改变 sRGB / 线性域）。跨端对拍必须
> 使用与 native **相同的目标纹理格式**（`Bgra8UnormSrgb`）；浏览器显示用
> canvas surface 格式，对拍另建独立 Srgb 离屏上下文，两者不混用。

> 该基线是**设备相关**的：跨 GPU 厂商不保证逐位相同。对拍前提是
> **同一台机器、同一分辨率与帧率**下的 native 与浏览器两端。

## 跨运行时分歧（2026-10-19 实测）

| 指标 | 实测值 |
|---|---|
| 帧数 | 216 / 216 |
| **结构层**（计数 + 拓扑序聚合哈希） | **216/216 一致**，两端均为 `3cbb0cb65ce50842` |
| 逐帧累计哈希首次分歧 | **n=103**（前 102 帧完全一致） |
| 逐帧像素 digest 分歧 | **1 / 216 帧**（frame index 102，即第 103 帧） |
| 分歧像素数 | **1 个像素** |
| 该像素 RGB 绝对差 | `(43, 26, 10)` |
| 单帧单通道平均绝对差（frame 103） | **0.000086** |
| 其他采样帧（0 / 130 / 160 / 215） | MAD = 0，0 个差异像素 |

分歧帧的 **CPU 侧绘制指纹**在 GPU 之前就已不同：一个屏幕空间
`Circle` 的 center/radius 末位相差约 1e-5
（native `(61.85997, 298.7306) r=-12.713349` vs wasm
`(61.85994, 298.7306) r=-12.713354`）。

**根因状态（未 100% 坐实）**：历史假设指向 `sin/cos` 的跨目标 libm 实现差异；
但 `f32` / `f64` 的 `sin/cos` 位模式在 64 点探针上**完全一致** ⇒ 该假设
**未被探针支持**。现有证据只能确定分歧在 **CPU 求值链路**（相机/几何变换），
具体运算尚未锁定。不能把未坐实的推断写成结论。

## 容差判据（三层，必须分开判定）

| 层 | 判据 | 能否容差 | 当前状态 |
|---|---|---|---|
| **结构** | 对象数、绘制项数、DrawQueue 长度、拓扑序（**无浮点**） | ❌ 必须逐位相同；不同即**逻辑错误**，直接失败 | ✅ 216/216 一致（`3cbb0cb65ce50842`） |
| **数值** | 像素通道值、变换矩阵 | ✅ 有阈值 | ✅ 1 帧 1 像素，MAD 0.000086 |
| **同目标** | native `--script` vs `--bundle` | ❌ 必须逐位相同 | ✅ 差值 0，哈希不变 |

**采纳阈值**（`zela-playback/examples/fingerprint.rs`）：

- 单帧单通道平均绝对差 `mad_rgb ≤ 1.0`（一个 8bit 级）；
- 分歧帧比例 `≤ 1%`（实测 1/216 = 0.46%，约 2.2× 余量）；
- 结构层：不一致立即失败，不进入容差。
- 结构串**不含任何浮点**（`draw.z` 已移除；绘制顺序由队列迭代序表达），
  变体名由 `ze_render_ir::Shape2D::kind_name` **穷举**给出，新增变体编译失败。

**回归信号**：误差不得随版本单调增长。若某次改动让 MAD 或分歧帧数
涨一个量级（即使仍在阈值内），必须单独报出。

## 确定性口径（术语约定）

本仓库谈“确定性”时，必须指明是哪一层：

| 口径 | 含义 | 当前结论 |
|---|---|---|
| **同目标可复现** | 同一编译目标、同一输入 ⇒ 逐位相同 | ✅ 成立（最硬保证；`--script`/`--bundle` 哈希相同） |
| **跨端结构一致** | native 与浏览器 ⇒ 对象/绘制项/拓扑序相同 | ✅ 成立（结构聚合哈希一致） |
| **跨端逐位相同** | native 与浏览器 ⇒ 每个像素逐位相同 | ❌ 不成立（已知可接受；1 帧 1 像素） |

> “跨端逐位相同”的放宽**不等于**核心保证退化：同目标可复现仍是最硬的，
> 跨端结构一致是新增的硬判据；只有“跨端逐位”这一层被收窄为容差。


