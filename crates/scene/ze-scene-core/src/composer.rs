// composer.rs — **合成器对象化**（S3）。
//
// # 定位
//
// `Composer` 是**行为对象**：它本身不进 DrawQueue，也不是每帧遍历的节点；
// 它通过 S1 的生命周期事件**增量维护**自己的派生态（[`ComposerQueue`]）。
//
// ```text
// LifetimeEvents（S1） ──增量──▶ ComposerQueue（派生态）
//                                   ▲
// 全量重算（registration / 对拍）────┘
// ```
//
// # 为什么派生态不是场景状态
//
// `ComposerQueue` 由事件维护，事件不修改 `Composer` 配置本身（红线 §0.3）。
// `Composer` 的配置（layer / blend / opacity）是**场景状态**，只能经
// `Scene::set`（命令路径）修改；`ComposerSystem` 的 `Writes` 不含 `Composer`，
// 因而事件消费者在类型层面就无法改配置。
//
// # 初始回放
//
// 后注册的合成器收不到注册前的事件 ⇒ 首次看到 `Composer` 时做一次**全量快照**
// （`full_recompute`）。这与 `ReplaySystem` 的"按台账清账"是同一类边界处理：
// 注册/重播时用全量，稳态用增量。
//
// # 与最终 DrawQueue 的关系
//
// 当前 `DrawQueue` 是**屏幕空间**且随相机变化（分镜相机每帧在动），必须每帧重录。
// 本轮合成器维护的是**相机无关**的内容索引（`ComposerQueue`）：增量结果与全量
// 重算可逐项对拍。待相机无关批处理落地后，它就是 DrawQueue 的增量来源。

use std::collections::BTreeSet;

use ze_render_ir::{LayerBlend, LayerId};
use ze_scene_contract::{LifetimeTracked, ObjectId, SceneRead};

/// 合成器配置（**场景状态**；只能经 `Scene::set` / 命令路径修改）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Composer {
    /// 目标图层。
    pub layer: LayerId,
    /// 与下层的混合方式。
    pub blend: LayerBlend,
    /// 图层不透明度（[0,1]，消费前钳制）。
    pub opacity: f32,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            layer: LayerId(0),
            blend: LayerBlend::Alpha,
            opacity: 1.0,
        }
    }
}

impl Composer {
    pub fn new(layer: LayerId, blend: LayerBlend, opacity: f32) -> Self {
        Self {
            layer,
            blend,
            opacity,
        }
    }

    /// 钳制后的不透明度（合成唯一取值口径）。
    pub fn clamped_opacity(&self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }
}

/// 合成器的**派生态**：相机无关的内容索引（Resource）。
///
/// 稳态由生命周期事件增量维护；注册时由 [`full_recompute`] 全量填充。
/// `items` 是 `BTreeSet` ⇒ 天然按 `ObjectId` 排序，输出确定。
#[derive(Debug, Default)]
pub struct ComposerQueue {
    initialized: bool,
    items: BTreeSet<ObjectId>,
}

impl ComposerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已完成初始回放。
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// 当前内容项（按 `ObjectId` 升序）。
    pub fn items(&self) -> Vec<ObjectId> {
        self.items.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.items.contains(&id)
    }

    // ─── 派生态变更（ECS 侧 `ComposerSystem` 的唯一写入口） ───────────────
    // 字段保持私有：增量维护的语义（`initialized` 与 `items` 同步）不由调用方拼装。

    /// 首次全量回放：用快照替换内容并标记已初始化。
    pub fn initialize_with(&mut self, snapshot: Vec<ObjectId>) {
        self.items = snapshot.into_iter().collect();
        self.initialized = true;
    }

    /// 合成器被销毁 ⇒ 派生态失效（下一次注册重新做全量回放）。
    pub fn invalidate(&mut self) {
        self.initialized = false;
        self.items.clear();
    }

    /// 增量：对象出现。
    pub fn insert(&mut self, id: ObjectId) {
        self.items.insert(id);
    }

    /// 增量：对象消失。
    pub fn remove(&mut self, id: ObjectId) {
        self.items.remove(&id);
    }
}

/// **全量重算**：当前场景里全部已追踪对象（排序后）。
///
/// 保留这条路径用于初始回放与"增量 == 全量"对拍。
pub fn full_recompute(scene: &impl SceneRead) -> Vec<ObjectId> {
    let mut ids = BTreeSet::new();
    scene.for_each::<LifetimeTracked>(|id, _| {
        ids.insert(id);
    });
    ids.into_iter().collect()
}
