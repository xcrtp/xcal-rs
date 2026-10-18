// frame.rs — 具体后端的**帧回调构造**（S0：从 `xcal-egui` 归位）。
//
// `xcal-render::frame` 负责"场景 → 后端中立输入"（`FrameInputs`）；
// 本模块负责"后端中立输入 → `SceneCompositorCallback`"。
// 分层原因：`xcal-render` 不能依赖具体后端（依赖方向 render ← backends）。
//
// 本模块**不依赖 `xcal-scene`**（S2 CI 规则）：`SceneLayer` 定义在 `xcal-render`。

use xc_object::LayerKind;
use xcal_render::{FrameInputs, SceneLayer};

use crate::scene_callback::SceneCompositorCallback;
use crate::wgpu_2d::{Recorded2D, snapshot_queue};

/// 从**后端中立**的帧输入构造唯一合成回调；无图层 ⇒ `None`（只画背景）。
///
/// 图层顺序：`input.layers` 的索引序即合成序；`flip_layer_order` 只在这里翻转一次。
pub fn build_scene_callback(input: FrameInputs<'_>) -> Option<SceneCompositorCallback> {
    if input.layers.is_empty() {
        return None;
    }

    let order: Vec<usize> = if input.flip_layer_order {
        (0..input.layers.len()).rev().collect()
    } else {
        (0..input.layers.len()).collect()
    };
    let layers: Vec<SceneLayer> = order.iter().map(|&index| input.layers[index]).collect();

    // 内容纹理层：单份队列快照 → 关联到唯一的 `ContentTexture` 图层 id。
    let content: Vec<(xc_render_ir::LayerId, Vec<Recorded2D>)> = input
        .content
        .map(|queue| {
            let snapshot = snapshot_queue(&queue.0);
            layers
                .iter()
                .filter(|layer| matches!(layer.kind, LayerKind::ContentTexture { .. }))
                .map(|layer| (layer.id, snapshot.clone()))
                .collect()
        })
        .unwrap_or_default();

    let morphs = input.morphs.map(|queue| queue.0.clone()).unwrap_or_default();
    let reveals = input
        .reveals
        .map(|queue| queue.0.clone())
        .unwrap_or_default();
    let filters = input.filters.cloned().unwrap_or_default();

    Some(SceneCompositorCallback::new(
        input.viewport,
        layers,
        input.queue_2d,
        input.queue_3d,
        content,
        morphs,
        reveals,
        filters,
    ))
}
