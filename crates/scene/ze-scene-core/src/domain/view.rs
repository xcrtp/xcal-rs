// view.rs — 场景对外的视图声明（向下契约：场景 → UI）
//
// 场景只声明"有什么"（图层 ID + 类型 + 内容句柄），UI 决定"怎么画"。
// 本模块是纯数据：不得出现任何 `egui::` 类型（B 阶段验收红线）。
//
// `ContentHandle` 刻意保持不透明——内容的具体形态是 S3 统一管线的职责，
// B 不阻塞 S3，S3 也不用等 B。

use crate::object::Camera2D;
use crate::types::Color32;
use ze_scene_contract::ObjectId;

// S2：合成概念已下沉。
// - `LayerId` / `LayerBlend` / `TextureHandle` → `ze-render-ir`
// - `LayerKind`（含相机）→ `object` 模块（B3-1b 起在 ze-scene-core 内）
// 此处 re-export 保持 `ze_scene_core::domain::*` / `zela_scene_ecs::domain::*` 路径不变。
pub use crate::object::LayerKind;
pub use ze_render_ir::{LayerBlend, LayerId, TextureHandle};

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
/// **B3 Step 4c-2**：单份 → 按 id 索引的容器（[`ContentTextureSpecs`]）。
/// `LayerId` / `TextureHandle` 由 **Surface 的 `ObjectId` 稳定派生**（不是图层位置），
/// 因此增删其它图层不会让内容层 id 漂移。
#[derive(Debug, Clone, Copy)]
pub struct ContentTextureSpec {
    /// 归属 Surface（4c-4 起**必填**；容器不再有"旧式单份"分支）。
    pub surface: ObjectId,
    /// 内容相机：`rect` 为**纹理矩形**（世界 → 纹理坐标的映射）。
    pub camera: Camera2D,
    /// 纹理尺寸（物理像素）。
    pub size_px: [u32; 2],
    /// 是否参与合成。默认 `false`（仅作纹理源）；`true` 用于调试直视纹理。
    pub composite: bool,
}

impl ContentTextureSpec {
    /// 稳定图层 id：由 Surface 派生。
    pub fn layer_id(&self) -> LayerId {
        content_layer_id(self.surface)
    }

    /// 稳定纹理句柄：与 `layer_id` 同一条 id 链派生（3D 材质用同一句柄引用）。
    pub fn texture_handle(&self) -> TextureHandle {
        content_texture_handle(self.surface)
    }
}

/// 内容纹理层容器（按 `LayerId` 索引；支持多个 Surface 同时存在）。
#[derive(Debug, Clone, Default)]
pub struct ContentTextureSpecs(pub Vec<ContentTextureSpec>);

impl ContentTextureSpecs {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 视口图层的固定 id（不再用 `layers.len()` 位置分配）。
pub const LAYER_ID_VIEWPORT_2D: LayerId = LayerId(0);
pub const LAYER_ID_VIEWPORT_3D: LayerId = LayerId(1);

/// Surface 内容层的 id 基址（与视口 id 不重叠）。
const CONTENT_LAYER_ID_BASE: u32 = 1 << 20;
/// Surface 纹理句柄基址（避开 `CONTENT_TEXTURE_ID = 1` 的旧值）。
const CONTENT_TEXTURE_ID_BASE: u64 = 1 << 32;

/// `ObjectId → LayerId` 的**唯一**映射（内容层）。
pub fn content_layer_id(surface: ObjectId) -> LayerId {
    LayerId(CONTENT_LAYER_ID_BASE + surface.index)
}

/// `ObjectId → TextureHandle` 的**唯一**映射（内容层）。
pub fn content_texture_handle(surface: ObjectId) -> TextureHandle {
    TextureHandle::new(CONTENT_TEXTURE_ID_BASE + u64::from(surface.index))
}

/// 从场景里的 `Surface(Renderer2D)` 生成内容纹理规格（**唯一装配入口**）。
///
/// - 只收 `Renderer2D`（`Renderer3D` 走 Direct，不需要纹理）；
/// - `SurfaceTexture` 必填（缺失 / 非法 ⇒ 显式错误）；
/// - 顺序按 `ObjectId` 排序 ⇒ 帧间稳定。
pub fn content_texture_specs(
    scene: &impl ze_scene_contract::SceneRead,
) -> Result<ContentTextureSpecs, crate::surface::SurfaceError> {
    use crate::surface::{Surface, SurfaceError, SurfaceRenderer, SurfaceTexture};

    let mut ids = Vec::new();
    scene.for_each::<Surface>(|id, _| ids.push(id));
    ids.sort();

    let mut specs = Vec::with_capacity(ids.len());
    for id in ids {
        let renderer = scene
            .with::<SurfaceRenderer, _>(id, |renderer| *renderer)
            .ok_or(SurfaceError::MissingRenderer(id))?;
        if renderer != SurfaceRenderer::Renderer2D {
            continue;
        }
        let surface = scene
            .with::<Surface, _>(id, |surface| *surface)
            .ok_or(SurfaceError::MissingRenderer(id))?;
        let texture = scene
            .with::<SurfaceTexture, _>(id, |texture| *texture)
            .ok_or(SurfaceError::MissingTexture(id))?;
        let camera = texture.camera_for(surface.bounds)?;
        specs.push(ContentTextureSpec {
            surface: id,
            camera,
            size_px: texture.texel_size,
            composite: false,
        });
    }
    Ok(ContentTextureSpecs(specs))
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
    use ze_notion::{Rect2D, Vec2};

    fn layer(opacity: f32, blend: LayerBlend) -> LayerDescriptor {
        LayerDescriptor {
            id: LayerId(0),
            kind: LayerKind::Viewport2D {
                camera: Camera2D::new(
                    (0.0, 0.0).into(),
                    40.0,
                    Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 100.0)),
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
