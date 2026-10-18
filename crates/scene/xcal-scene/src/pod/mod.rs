//! 脚本 Pod 的 **domain 侧**：能力定义 + `scene` / `content` 两个 Service。
//!
//! 语言适配器（`xc-pod-lua` / `xc-pod-py`）只依赖这里的能力类型；
//! 本模块不认识任何语言运行时（无 mlua / pyo3）。
//!
//! ```text
//! xcal-scene::pod
//! ├── api      SceneApi（16）/ ContentApi（2）
//! ├── types    结构化参数 / 返回值（PodType schema）
//! ├── scene_service    SceneServicePod
//! └── content_service  ContentServicePod + PathStore
//! ```
//!
//! B1 的绑定驱动（`xc-pod-binding`）只消费 `C::calls()`，与本模块无编译期耦合。

pub mod api;
mod content_service;
mod scene_service;
pub mod types;

pub use api::{ContentApi, SceneApi};
pub use content_service::{CONTENT_SERVICE_NAME, ContentServicePod, PathStore};
pub use scene_service::{FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServicePod};
pub use types::*;

use std::sync::Arc;

use xc_pod::Pod;
use xc_prerender::InMemoryEngine;

/// 构造场景 Service（返回 trait object：适配层不需要知道具体类型）。
pub fn scene_service(paths: PathStore) -> Arc<dyn Pod> {
    Arc::new(SceneServicePod::with_paths(paths))
}

/// 构造内容 Service。
pub fn content_service(engine: Arc<InMemoryEngine>, paths: PathStore) -> Arc<dyn Pod> {
    Arc::new(ContentServicePod::new(engine, paths))
}
