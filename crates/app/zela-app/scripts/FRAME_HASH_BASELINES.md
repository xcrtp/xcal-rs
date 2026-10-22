# 帧像素序列哈希基线（同后端 + 同机器口径）

> **定位（B3-0.5 起正式澄清）**：本文件记录的是**测试夹具**（regression fixture），
> 不是产品承诺。它保证的是「**同一后端实现 + 同一台机器**」下，渲染结果逐像素可复现：
> 代码改动若意外改变画面，`cargo verify --step hashes` 会红。
>
> - **不追求跨端（native vs 浏览器 / 不同 GPU 厂商）逐像素一致**：浮点求值、
>   WGSL 编译与驱动差异天然存在末位分歧，被 SHA-256 链放大成整串不同。
> - **`--script == --bundle`** 是装配正确性判据（同一机器的两条输入路径必须同像素）。
> - `[browser]` 段**仅作历史存档**，是设计而非缺口；它不参与自动比对。
>
> 用途：渲染回归对拍。**不覆盖 mp4 / PNG 容器**，只覆盖帧缓冲像素 ⇒ 浏览器侧
> 无需导出、无需 ffmpeg。

## `@ <commit>` 口径（B4 P1-4 定案：D，补充 B）

值行 `key = <hash> @ <commit>` 里的 commit 表示**产生该哈希值时的代码状态**。

- `--record` 写入 `# reason` / `# slotB` / 生效行；下一次轮换时 `# slotA` 拿到的
  就是这条产生记录，而不是"下次重录的 HEAD"。
- **脏工作树 ⇒ `@ <sha>+dirty`**：重录必然发生在"改完代码、还没提交"时，所以
  当时的 HEAD **并不包含**这份状态。直接标 `@ <HEAD>` 是错的（P1-4 的成因：
  6 条值全标 `@ da9ea93`，实际由 `642b9d4` 携带）。
  `+dirty` 的含义是"这份值不属于 `<sha>` 这个提交，它由紧随其后的提交携带"。
- **提交后必须归因**：把 `baselines.txt` 与代码**一起提交**，然后立刻跑

  ```bash
  cargo verify --rebind   # 要求干净工作树；把 <sha>+dirty 整段换成当前 HEAD
  ```

  `--rebind` 要求干净工作树：否则"当前 HEAD"仍不是携带这份录制的提交。
  它替换的是**整段 `<sha>+dirty`**（连同 sha），而不只是删后缀 —— 只删后缀会
  把值留在录制前的 HEAD 上，正是要修的那个错误。
- **`+dirty` 是门禁**：`smoke` / `hashes` 步骤见到未归因的 `+dirty` 直接失败。
  放行等于允许"值不属于任何提交"进入历史，归因就永久失效。
- **`# delta:` 逐条留痕**：一次重录里**每个值变了的 key** 都写一行
  `# delta: <key> <旧前缀> -> <新前缀>（本次重录改变，原因见上一条 reason）`。
  `slotA` / `slotB` 给机器看，`delta` 给人看；没变的 key 不写噪音。
- **旧文件里的 `@ pending`** 没有记录产生时 HEAD，按**最后已知有效提交**理解
  （即同段 `# slotA` 的 commit）。`@ pending` 不再作为新录写的默认值。
- ⚠️ **回填值警告**：早期几条 `@ f2c8778` / `@ da9ea93` 是**回填**的
  （`b6eaa864` 等值的实际产生时间远早于该提交）。跨批次归因时
  **不得采信这几行的 `@`** —— 它只表示"最后已知有效"，不表示"该值产生于该提交"。
  自口径 D 起新录写的值会先标 `+dirty`、再由 `--rebind` 归因到真实提交。
- `[browser]` 段仍只有裸值：它不参与自动比对，不适用本口径。


## 算法（V1）

实现：[`zela_scene_bundle::frame_hash::FrameSequenceHash`](../../../scene/zela-scene-bundle/src/frame_hash.rs)

- 像素：RGBA8、行优先、左上原点、行距 `width*4`、直色（非预乘）；
- `state0 = SHA-256("ZELA-FRAME-SEQ-V1" || width_le_u32 || height_le_u32)`；
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

## 基线（2026-10-20，Windows / RTX 4060 Laptop）

**哈希值的唯一真源是 [`baselines.txt`](baselines.txt)**；本文件只解释口径，
不重复写哈希。三个基线**都要保留**，来源不同、用途不同：

| 段 | 端 | 作品 | 帧数 / 分辨率 | 来源 | 自动比对 |
|---|---|---|---|---|---|
| `[native]` | native（Vulkan headless） | fourier | 216 / 640×480 | `zela-render --script/--bundle` | ✅ 不符即 fail |
| `[native]` | native（Vulkan headless） | storyboard | 135 / 640×480 | 同上 | ✅ 不符即 fail |
| `[native]` | native（Vulkan headless） | demo3d | 120 / 640×480 | 同上（3D 首入对拍网，B2-8） | ✅ 不符即 fail |
| `[native]` | native（Vulkan headless） | surface_matrix | 1 / 640×480 | 同上（B3 Step 5-3 能力矩阵，`composite=true`，静态） | ✅ 不符即 fail |
| `[native]` | native（Vulkan headless） | atlas | 120 / 640×480 | 同上（B3 Step 5-4：2D 公式内容 → 3D 平面） | ✅ 不符即 fail |
| `[smoke]` | native（Vulkan headless） | smoke | 15 / 320×240 | 同上（PR / `--fast` 快速反馈） | ✅ 不符即 fail，但**不等价于完整基线** |
| `[browser]` | 浏览器（Chrome WebGPU） | fourier | 216 / 640×480 | `zela-web` + `?compare=1` | ❌ **仅留档** |

> `surface_matrix` / `atlas` 于 2026-10-22（B3 Step 5）首次登记。首次登记时两项**没有旧值行**，
> 只写 `# slotB:` 与生效行、**没有 `# slotA:`**（`--record` 允许"新增 key 直接登记"）。
> 作品 A 之后重录过三次：① 补 `Path × Fill` 组合；② 修「Surface 归属内容被屏幕视口重复绘制」；
> ③ 对齐纹理/承载平面的宽高比（bounds = texel = 4:3，消除横向拉伸）。当前值见 `baselines.txt`。
> 作品 B 另在 ③ 之后重录一次：`Planar` 兜底 UV 的 V 轴取反（贴图不再上下倒转），
> 平面 8×6（与 bounds / texel 同比例）、相机拉远、公式改一阶表达式 + 深色描边。
> 因此 `# slotA:` 只保留**上一次**值，更早的值看 git 历史。
>
> `surface_matrix` 只渲 1 帧（静态能力矩阵）：像素覆盖的是"全部组合同屏一次"，
> 组合覆盖度的主判据是**非像素断言**（`ContentTextureQueue` 里每种组合都录到，
> 见 `zela-app` 测试 `surface_matrix_records_all_script_expressible_combos`）。

> **demo3d 基线变更（2026-10-22，B3 Step 5 修正）**：`dc8b1388…` → `ddd69b3e…`。
> 不是"画面漂移"，是渲染管线三处显式修正的结果，每个变化都能解释：
> 1. `Planar` 兜底 UV 的 V 轴取反（纹理行首在 `+y`，贴图不再上下倒转）；
> 2. `solid_cube` 改为 24 顶点：每面显式 UV + **面法线**（旧实现 8 顶点共享、无 UV / 无
>    法线 ⇒ Planar 兜底在侧面退化成 0 面积）；
> 3. `cube.wgsl` 的 PBR TBN 在 `det ≈ 0` 时**跳过法线贴图**（旧实现 `normalize(0)` → NaN）。
> ⇒ 立方体右脸的随机噪点消失，三个可见面的明暗变成正确的面法线着色。
> `atlas` 同步重录（它用 Cube 当承载平面）。

> `[browser]` 段**绝不参与自动比对**：本机跑不出浏览器产物，
> 纳入比对只会让本机永远红，并诱导人去"修"一个不该修的东西。
>
> 当前 `[browser]` 的 fourier 值录于旧域分隔符时期，尚未用
> `ZELA-FRAME-SEQ-V1` 重录；在重录前它只作为历史存档，不代表当前口径。

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


