# 帧像素序列哈希基线（跨运行时对拍口径）

> 用途：WASM 第二阶段的 `native vs 浏览器` 画面对拍。
> **不覆盖 mp4 / PNG 容器**，只覆盖帧缓冲像素 ⇒ 浏览器侧无需导出、无需 ffmpeg。

## 算法（V1）

实现：[`xcal_scene_bundle::frame_hash::FrameSequenceHash`](../../../scene/xcal-scene-bundle/src/frame_hash.rs)

- 像素：RGBA8、行优先、左上原点、行距 `width*4`、直色（非预乘）；
- `state0 = SHA-256("XCAL-FRAME-SEQ-V1" || width_le_u32 || height_le_u32)`；
- `state_{i+1} = SHA-256(state_i || i_le_u32 || SHA-256(frame_i))`；
- 输出 `hex(state_n)`（小写 64 字符）。

尺寸进入初始状态；像素格式不进入 —— 两端都必须先规范化到上述 RGBA8
（native 用 `xcal_app::export::to_rgba`，浏览器按回读纹理的通道序做同样转换）。

## 复现命令

```bash
# 1) 编译 XSB（可选：--script 与 --bundle 两条路径都要跑一遍）
cargo run -q -p xcal-compile --no-default-features --features typst -- \
  crates/app/xcal-app/scripts/fourier.lua --out /tmp/fourier.xsb

# 2) 渲染并输出序列哈希（--frames 表示不依赖 ffmpeg；--hash-out 可选）
cargo run -q -p xcal-app --no-default-features --features typst --bin xcal-render -- \
  --script crates/app/xcal-app/scripts/fourier.lua \
  --frames /tmp/f_s --fps 30 --size 640x480 --hash-out /tmp/f_s.hash

cargo run -q -p xcal-app --no-default-features --features typst --bin xcal-render -- \
  --bundle /tmp/fourier.xsb \
  --frames /tmp/f_b --fps 30 --size 640x480 --hash-out /tmp/f_b.hash
```

标准错误输出：

```text
[frame-hash] sequence = <64 hex>（N 帧，640×480，RGBA8）
```

## 基线（2026-10-19，Windows / RTX 4060 Laptop，640×480 @ 30fps）

| 作品 | 帧数 | 序列哈希 |
|---|---|---|
| fourier | 216 | `fd40329fe6d18045265e801b5a8b0459926f461b73489e8208ff2779ad2a690d` |
| storyboard | 135 | `7e1a46e89dbad7fcf5a3f955fab56fa7dc85e2b8127c8acfe449b7131a1e4e43` |

`--script` 与 `--bundle` 两条路径在同一分辨率 / 帧率下**逐帧像素一致**，
序列哈希因而相同（已在本机复跑核对）。

独立复核：用 Python + Pillow 解码落盘的 PNG（`Image.tobytes()` 即紧凑 RGBA8），
按同一 V1 公式复算，得到与上表完全相同的两个值 —— 证明该值只依赖帧像素，
与导出容器 / 编码器无关。

## 浏览器侧如何得到同一个值

1. 每帧渲染完成后从目标纹理回读像素（或直接取 WebGPU canvas 的 readback 结果）；
2. 去掉行对齐填充（紧凑 `width*4`），按与 native **相同的目标格式**做通道规范化
   （本项目 headless 目标格式为 `Bgra8UnormSrgb`，`to_rgba` 只做 R/B 交换）；
3. 对每帧调用同一个 `FrameSequenceHash::push_frame(rgba)`（该模块在
   `xcal-scene-bundle` 中，是纯 Rust、已在 wasm gate 内）；
4. 最后一帧后取 `hex()` 与上表比对。

> **格式前提**：V1 只做通道顺序规范化（不改变 sRGB / 线性域）。第二轮对拍时，
> 浏览器必须使用与 native **相同的目标纹理格式**（`Bgra8UnormSrgb`）；
> 若换成非 sRGB 格式，像素值语义不同，不能直接与上表比对。

> 该基线是**设备相关**的：跨 GPU 厂商不保证逐位相同。第二阶段的对拍前提是
> **同一台机器、同一分辨率与帧率**下的 native 与浏览器两端。
