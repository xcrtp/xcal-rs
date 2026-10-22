// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: 新建 crate 入口（B4 Step 3/5）：承接上游 `rust/engine/src` 的
// 渲染模块树，去掉 python / typeset / export / window / web / player / take 等 feature。

//! `zela-render-vector` — 矢量后端（移植自 manimgx 的 `rust/engine`，SHA 见 `UPSTREAM.md`）。
//!
//! # 移植范围
//!
//! 只带渲染相关模块：`render`（公共类型 + GPU 装配）、`vector`（解析 AA 路径）、
//! `mesh`、`shadow`、`bloom`、`occlusion`、`environment`，以及它们的 WGSL。
//! `python` / `typeset` / `text` / `export` / `mp4` / `aac` / `audio` / `speaker` /
//! `chrome` / `window` / `web` / `player` / `take` / `project` / `stage` / `pack` /
//! `encode` / `nv12` / `lavapipe` / `dxc` **未移植**。
//!
//! # 许可
//!
//! 逐文件归属见 `UPSTREAM.md`：多数文件 MIT（上游无 SPDX 头，已由我们补齐）；
//! `light.wgsl` / `occlusion.wgsl` / `occlusion.rs` / `tone.wgsl` / `bloom.wgsl` /
//! `dfg.bin` 为 Apache-2.0（AOSP / Filament）。
//!
//! # 与上游的差异
//!
//! - 去掉 `take` 模块后，`render.rs` 里 `size_of::<Record>() == take::RECORD` 的
//!   编译期断言改为按本 crate 的常量表校验（见 `render.rs` 的 `Modified by Zela`）；
//! - 去掉 `dxc` / `lavapipe` 两个平台相关模块（DXC 由 wgpu 自带后端处理）。

/// 上游快照的 commit SHA（唯一凭据，详见 `UPSTREAM.md`）。
pub const UPSTREAM_COMMIT: &str = "dcfe01a932118263c238a50563d1bb6fab09e08e";

/// 上游仓库名（记录用）。
pub const UPSTREAM_REPO: &str = "manimgx";

mod environment;
mod mesh;
mod render;

pub mod api;
pub use api::{FilledPath, render_2d};
// B4 场景适配（V2-06）：`Shape2D` / `Style2D` → 填充记录（只依赖 kernel 层）。
pub mod scene;
pub use scene::{SceneItem, SceneMapError, fill_paths, render_scene_2d};

/// 内容 key：xxh3（64 位，0 保留）。等值内容只上传一次。
pub fn digest<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> u64 {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.digest().max(1)
}

/// 相机视图：(key, width, height, view, records)。
pub(crate) type CameraView = (u64, u32, u32, Vec<u8>, Vec<u8>);

pub(crate) fn check_key(key: u64) -> Result<(), String> {
    if key == 0 {
        Err("key 0 is reserved".into())
    } else {
        Ok(())
    }
}

/// 把字节按条读成 `T` 的向量（`what` 用于错误信息）。
pub(crate) fn read<T: bytemuck::Pod>(bytes: &[u8], what: &str) -> Result<Vec<T>, String> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<T>()) {
        return Err(format!(
            "{what}: {} bytes is not a whole number of items",
            bytes.len()
        ));
    }
    Ok(bytemuck::pod_collect_to_vec(bytes))
}

/// 一条 `Record` 的字节数（上游 `take::RECORD`）。
pub(crate) const RECORD_BYTES: usize = 336;
/// 一条 `Record` 头部的 key 槽位数（上游 `take::KEYS`）。
pub(crate) const KEYS: usize = 7;

/// 逐条读出记录头部的 key（`Modified by Zela`：内联上游 `take::slots`，去掉 take 模块）。
pub(crate) fn record_slots(records: &[u8]) -> impl Iterator<Item = [u64; KEYS]> + '_ {
    records.chunks_exact(RECORD_BYTES).map(|r| {
        std::array::from_fn(|k| u64::from_le_bytes(r[8 * k..8 * k + 8].try_into().unwrap()))
    })
}
