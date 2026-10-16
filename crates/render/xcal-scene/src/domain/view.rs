// view.rs — 场景对外的视图声明（向下契约：场景 → UI）
//
// 场景只声明"有什么"（图层 ID + 类型 + 内容句柄），UI 决定"怎么画"。
// 本模块是纯数据：不得出现任何 `egui::` 类型（B 阶段验收红线）。
//
// `ContentHandle` 刻意保持不透明——内容的具体形态是 S3 统一管线的职责，
// B 不阻塞 S3，S3 也不用等 B。

use crate::scene::components::{Camera3D, TextureHandle};
use crate::types::{Camera2D, Color32};

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
    /// 内容纹理层（S3-3）：渲染到**独立纹理**，供 3D 材质采样。
    ///
    /// 与视口层的区别：不绑定屏幕视口，绑定一张固定尺寸纹理。
    ContentTexture {
        /// 内容相机：`rect` 为**纹理矩形**（决定世界 → 纹理坐标的映射）。
        camera: Camera2D,
        /// 纹理尺寸（物理像素）。
        size_px: [u32; 2],
        /// 该层渲染到的纹理句柄；3D 材质用**同一句柄**引用它。
        texture: TextureHandle,
        /// 是否参与合成。默认 `false`（仅作纹理源）；`true` 用于调试直视纹理。
        composite: bool,
    },
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

/// 图层与下层的混合方式（S3-2 合成器）。
///
/// 只声明语义，具体 wgpu 混合状态属于后端（`xcal-render-backends`）：
/// - [`Opaque`](Self::Opaque) → `BlendState::REPLACE`（该层完全覆盖下层，
///   要求图层纹理本身不透明）；
/// - [`Alpha`](Self::Alpha) → 预乘 alpha（`src: One, dst: OneMinusSrcAlpha`）；
/// - [`Additive`](Self::Additive) → `src: One, dst: One`（发光/高亮）。
///
/// `Default` 是 `Opaque`（"等价于当前行为"的语义默认），但**当前 2D/3D 图层
/// 的纹理都是透明底**（背景已外移到合成器清屏，见 S3-2 §1.1），
/// 因此 `build_scene_view` 显式声明为 `Alpha`——用 `Opaque` 的 REPLACE
/// 会把图层外的透明像素写进主 pass，抹掉下层与背景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayerBlend {
    /// 不透明覆盖（默认；等价于当前行为）。
    #[default]
    Opaque,
    /// 预乘 alpha 混合（图层纹理 alpha 参与）。
    Alpha,
    /// 加法混合（发光/高亮效果）。
    Additive,
}

/// 一个图层的声明：ID + 类型 + 内容句柄 + 混合参数。
#[derive(Debug, Clone, Copy)]
pub struct LayerDescriptor {
    pub id: LayerId,
    pub kind: LayerKind,
    pub content: ContentHandle,
    /// 不透明度 [0,1]；配合 `blend` 使用。默认 1.0。
    ///
    /// 取值可能越界，混合前经 [`LayerDescriptor::clamped_opacity`] 钳制。
    pub opacity: f32,
    /// 与下层的混合方式。默认 `Opaque`。
    pub blend: LayerBlend,
}

impl LayerDescriptor {
    /// 钳制到 [0,1] 的不透明度（合成器唯一取值口径）。
    pub fn clamped_opacity(&self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }
}

/// 内容纹理层的配置（S3-3）：图层声明与内容录制都从这里取，是唯一真相来源。
///
/// ⚠️ **只支持一个内容纹理层**（R4）：本资源是单份。若将来需要多个内容纹理，
/// 需改成 `HashMap<LayerId, ContentTextureSpec>`，本轮不做（YAGNI）。
#[derive(Debug, Clone, Copy)]
pub struct ContentTextureSpec {
    /// 内容相机：`rect` 为**纹理矩形**（世界 → 纹理坐标的映射）。
    pub camera: Camera2D,
    /// 纹理尺寸（物理像素）。
    pub size_px: [u32; 2],
    /// 该层渲染到的纹理句柄；3D 材质用**同一句柄**引用它。
    pub texture: TextureHandle,
    /// 是否参与合成。默认 `false`（仅作纹理源）；`true` 用于调试直视纹理。
    pub composite: bool,
}

/// 合成器清屏色默认值（与改动前 2D 图层内的背景矩形一致）。
pub const DEFAULT_BACKGROUND: Color32 = Color32::new(20, 20, 20, 255);

/// 场景对外的完整视图声明。
///
/// **顺序语义**：`layers` 的索引顺序即合成顺序，索引 0 在最下，末位在最上。
#[derive(Debug, Clone)]
pub struct SceneView {
    pub layers: Vec<LayerDescriptor>,
    /// 合成器清屏色（**不属于任何图层**）。
    pub background: Color32,
}

impl Default for SceneView {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            background: DEFAULT_BACKGROUND,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Rect;

    fn layer(opacity: f32, blend: LayerBlend) -> LayerDescriptor {
        LayerDescriptor {
            id: LayerId(0),
            kind: LayerKind::Viewport2D {
                camera: Camera2D::new(
                    (0.0, 0.0).into(),
                    40.0,
                    Rect::new((0.0, 0.0).into(), (100.0, 100.0).into()),
                ),
            },
            content: ContentHandle::OPAQUE,
            opacity,
            blend,
        }
    }

    /// C1：`SceneView::default()` 的背景 = 改动前 2D 图层内的背景矩形 (20,20,20,255)。
    #[test]
    fn default_scene_view_background_matches_legacy_rect() {
        let view = SceneView::default();
        assert!(view.layers.is_empty());
        assert_eq!(view.background, Color32::new(20, 20, 20, 255));
        assert_eq!(view.background.a, 255, "默认背景必须不透明");
    }

    /// C1：`LayerBlend` 默认 `Opaque`（三变体齐备）。
    #[test]
    fn layer_blend_default_is_opaque() {
        assert_eq!(LayerBlend::default(), LayerBlend::Opaque);
        // 三变体互不相等（保证 HashMap<LayerBlend, _> 可作键）。
        assert_ne!(LayerBlend::Opaque, LayerBlend::Alpha);
        assert_ne!(LayerBlend::Alpha, LayerBlend::Additive);
    }

    /// C1：`opacity` clamp 到 [0,1]（合成器唯一取值口径）。
    #[test]
    fn opacity_is_clamped_to_unit_range() {
        assert_eq!(layer(1.0, LayerBlend::Alpha).clamped_opacity(), 1.0);
        assert_eq!(layer(0.0, LayerBlend::Alpha).clamped_opacity(), 0.0);
        assert_eq!(layer(2.5, LayerBlend::Alpha).clamped_opacity(), 1.0);
        assert_eq!(layer(-1.0, LayerBlend::Alpha).clamped_opacity(), 0.0);
        assert_eq!(layer(0.25, LayerBlend::Alpha).clamped_opacity(), 0.25);
    }

    /// C1 红线：`SceneView` / `LayerDescriptor` 仍为纯数据（无 egui 类型）。
    #[test]
    fn scene_view_layer_order_is_declaration_order() {
        let mut view = SceneView::default();
        view.layers.push(layer(1.0, LayerBlend::Alpha));
        view.layers.push(layer(1.0, LayerBlend::Alpha));
        assert_eq!(view.layers[0].id, LayerId(0));
        assert_eq!(view.layers[1].id, LayerId(0));
        // 索引 0 = 最下层（顺序即声明，无额外 z 字段）。
        assert_eq!(view.layers.len(), 2);
    }
}
