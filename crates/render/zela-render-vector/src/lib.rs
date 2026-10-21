// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: 新建 crate 骨架（B4 Step 3 许可证基线；矢量后端移植落点）。

//! `zela-render-vector` — 矢量后端（移植自 manimgx 的 `rust/engine`）。
//!
//! # 现状（B4 Step 3）
//!
//! 本 crate 目前只有**许可证基线 + 骨架**：`LICENSES/` 收录上游两份许可原文，
//! [`UPSTREAM.md`](../UPSTREAM.md) 记录 upstream commit SHA 与逐文件许可清单。
//! 矢量核心（`vector.*` / `render.rs` / 着色栈 / 光栅后端，约 9.9k 行）在 Step 5 落进来。
//!
//! # 逐文件许可（移植时必须遵守）
//!
//! - **MIT**：上游 `REUSE.toml` 的 `path = "**"` 兜底覆盖的多数文件。这些文件**自身没有
//!   SPDX 头** ⇒ 移植时必须补齐：
//!   ```text
//!   // SPDX-FileCopyrightText: 2026 Academa, Inc.
//!   // SPDX-License-Identifier: MIT
//!   // Modified by Zela: <一句话说明改了什么>
//!   ```
//! - **Apache-2.0**：`light.wgsl` / `occlusion.wgsl` / `occlusion.rs` / `tone.wgsl` /
//!   `bloom.wgsl` / `dfg.bin`（AOSP 版权，改编自 Google Filament）。保留上游四行原文，
//!   追加一行 `// Modified by Zela: …`。
//!
//! # 依赖边界
//!
//! 只开上游的 `render` feature：`python` / `typeset` / `export` / `window` / `web` 全部关闭，
//! 因此 **x264(GPL) / ffmpeg(LGPL) / pyo3 / typst / winit / cpal 都不会进入依赖树**。

/// 上游快照的 commit SHA（`UPSTREAM.md` 的唯一机器可读副本）。
pub const UPSTREAM_COMMIT: &str = "dcfe01a932118263c238a50563d1bb6fab09e08e";

/// 上游仓库名（记录用）。
pub const UPSTREAM_REPO: &str = "manimgx";
