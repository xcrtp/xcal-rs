// view.rs — 场景对外的视图声明（向下契约：场景 → UI）
//
// 场景只声明"有什么"（图层 ID + 类型 + 内容句柄），UI 决定"怎么画"。
// 本模块是纯数据：不得出现任何 `egui::` 类型（B 阶段验收红线）。
//
// `ContentHandle` 刻意保持不透明——内容的具体形态是 S3 统一管线的职责，
// B 不阻塞 S3，S3 也不用等 B。

use crate::scene::components::Camera3D;
use crate::types::Camera2D;

/// 图层唯一标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

/// 图层类型：一个视口 + 其相机。
///
/// 保留 2D/3D 两个分支（2D 走 CPU 录制、3D 走 GPU 回调是短期事实），但：
/// - 变体里**只放相机数据**，不内联任何相机控制器逻辑（控制器消费
///   `CameraCommand`，是纯函数，见 xcal-egui 相机控制器 / 未来 3D 控制器）；
/// - `camera` 字段是相机统一的**接缝点**：统一后 `Camera2D` 收敛为
///   `Camera3D + Orthographic` 的薄壳（对外仍暴露 center/scale 语义），
///   本枚举与 LayerDescriptor 均无需改动。相机构造的唯一出处是
///   `AppCamera::to_camera_2d`（UI 侧），不要在各处硬编码 `Camera2D::new`。
#[derive(Debug, Clone, Copy)]
pub enum LayerKind {
    Viewport2D { camera: Camera2D },
    Viewport3D { camera: Camera3D },
}

/// 内容来源句柄（不透明；S3 统一管线后指向 RenderQueue 的分片）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHandle(u64);

impl ContentHandle {
    /// B 阶段占位：单一不透明句柄。S3 接管后由 RenderQueue 分片填充。
    pub const OPAQUE: ContentHandle = ContentHandle(0);

    pub fn opaque(id: u64) -> Self {
        ContentHandle(id)
    }

    pub fn id(&self) -> u64 {
        self.0
    }
}

/// 一个图层的声明：ID + 类型 + 内容句柄。
#[derive(Debug, Clone, Copy)]
pub struct LayerDescriptor {
    pub id: LayerId,
    pub kind: LayerKind,
    pub content: ContentHandle,
}

/// 场景对外的完整视图声明。
#[derive(Debug, Clone, Default)]
pub struct SceneView {
    pub layers: Vec<LayerDescriptor>,
}
