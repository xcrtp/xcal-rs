// frame.rs — **渲染无关的帧体构造**（S0：从 `zela-egui` 归位）。
//
// 这里只做"场景 + 资源 → 本帧视图声明 / 队列清理"：
// - `build_scene_view`：2D/3D/内容纹理图层的纯数据声明（`SceneView`）；
// - `end_frame`：清空已被呈现消费的 2D 队列；
// - `draw_call_count`：可观测性取数。
//
// **不出现 egui 类型**：egui 的每帧编排（输入 / 面板 / 呈现）仍在 `zela-egui`。
// 回调的**后端构造**（`SceneCompositorCallback`）在 `zela-render-backends::frame`
// —— `zela-render` 不能依赖具体后端（依赖方向是 render ← backends）。
//
// 依赖方向：`zela-scene` → `zela-render`；本模块只读场景契约与渲染队列。

use ze_notion::Rect2D;
use ze_scene_core::{Camera2DHandle, Camera3D, LayerKind};
use zela_scene_ecs::contract::SceneRead;
use zela_scene_ecs::domain::{
    ContentHandle, ContentTextureSpec, LayerBlend, LayerDescriptor, LayerId, SceneView,
};
use zela_schedule::resource::Resources;

use crate::{
    ContentTextureQueue, DrawQueue, DrawQueue3D, LightQueue, MorphQueue, PostFilterChain,
    RevealQueue, ViewRect,
};

/// 一个**后端中立**的图层声明：只带合成所需的最小信息。
///
/// 由 `SceneView::layers` 复制而来；`zela-render-backends` 消费它，
/// 因此后端不必依赖 `zela-scene`（S2 CI 规则）。
#[derive(Debug, Clone, Copy)]
pub struct SceneLayer {
    /// 图层 id（按层寻址离屏目标）。
    pub id: LayerId,
    /// 图层类型（含相机）。
    pub kind: LayerKind,
    /// 与下层的混合方式。
    pub blend: LayerBlend,
    /// 已钳制的不透明度。
    pub opacity: f32,
}

/// 帧体构造的**渲染无关**开关（由宿主从 UI 配置或命令行提取）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOptions {
    /// 是否声明 2D 视口图层。
    pub show_2d: bool,
    /// 是否声明 3D 视口图层。
    pub show_3d: bool,
    /// 是否翻转图层叠放顺序（验收开关；默认不翻转）。
    pub flip_layer_order: bool,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self {
            show_2d: true,
            show_3d: true,
            flip_layer_order: false,
        }
    }
}

impl FrameOptions {
    /// 从场景 `UiConfig` 提取渲染相关字段（UI 层负责调用；本 crate 不反向依赖 UI）。
    pub fn from_ui_config(config: &zela_scene_ecs::domain::UiConfig) -> Self {
        Self {
            show_2d: config.show_2d,
            show_3d: config.show_3d,
            flip_layer_order: config.flip_layer_order,
        }
    }
}

/// 从场景状态构建本帧视图声明（纯数据，无 egui / 无后端类型）。
///
/// - 2D 图层：相机来自场景对象（`Camera2DHandle` 指认 `Framing2D`），
///   视口矩形由 `ViewRect` 绑定；
/// - 3D 图层：相机取 World 中第一个 `Camera3D` 组件；
/// - 内容纹理层：仅当注册了 `ContentTextureSpec` 时声明；
/// - 两个视口图层都渲染到透明底离屏纹理 ⇒ 声明为 [`LayerBlend::Alpha`]。
pub fn build_scene_view(
    scene: &impl SceneRead,
    res: &Resources,
    options: FrameOptions,
) -> SceneView {
    let rect = res
        .get::<ViewRect>()
        .map(|v| v.rect)
        .unwrap_or_else(|| ViewRect::default().rect);

    let mut view = SceneView::default();

    // ⚠️ **总闸优先**：`show_2d` / `show_3d` 是图层级总闸，关掉即整层不声明。
    if options.show_2d {
        if let Some(handle) = res.get::<Camera2DHandle>() {
            if let Some(camera) = handle.resolve(scene, rect) {
                view.layers.push(LayerDescriptor {
                    id: LayerId(view.layers.len() as u32),
                    kind: LayerKind::Viewport2D { camera },
                    content: ContentHandle::OPAQUE,
                    opacity: 1.0,
                    blend: LayerBlend::Alpha,
                });
            }
        }
    }

    if options.show_3d {
        let mut first_camera = None;
        scene.for_each::<Camera3D>(|_, camera| {
            if first_camera.is_none() {
                first_camera = Some(*camera);
            }
        });
        if let Some(camera) = first_camera {
            view.layers.push(LayerDescriptor {
                id: LayerId(view.layers.len() as u32),
                kind: LayerKind::Viewport3D { camera },
                content: ContentHandle::OPAQUE,
                opacity: 1.0,
                blend: LayerBlend::Alpha,
            });
        }
    }

    // 内容纹理层（S3-3）：渲染到独立纹理供 3D 材质采样（默认不参与合成）。
    if let Some(spec) = res.get::<ContentTextureSpec>() {
        view.layers.push(LayerDescriptor {
            id: LayerId(view.layers.len() as u32),
            kind: LayerKind::ContentTexture {
                camera: spec.camera,
                size_px: spec.size_px,
                texture: spec.texture,
                composite: spec.composite,
            },
            content: ContentHandle::OPAQUE,
            opacity: 1.0,
            blend: LayerBlend::Alpha,
        });
    }

    view
}

/// 帧末清理：2D 队列内容已被呈现消费，清空以便下帧重新录制。
pub fn end_frame(res: &mut Resources) {
    if let Some(queue) = res.get_mut::<DrawQueue>() {
        queue.clear();
    }
}

/// **一帧的后端输入**：视图声明 + 队列引用（不拥有数据）。
///
/// 由本 crate 从 `Resources` 收集；具体后端（`zela-render-backends`）消费它来
/// 构造自己的渲染回调。这样 `zela-render` 不必认识任何后端类型，后端也不必认识
/// `zela_schedule::Resources`。
pub struct FrameInputs<'a> {
    /// 本帧图层声明（索引顺序即合成顺序）。
    pub layers: Vec<SceneLayer>,
    /// 合成视口（像素）。
    pub viewport: Rect2D,
    /// 是否翻转图层叠放顺序。
    pub flip_layer_order: bool,
    /// 当帧 2D 命令队列。
    pub queue_2d: &'a DrawQueue,
    /// 当帧 3D 队列。
    pub queue_3d: &'a DrawQueue3D,
    /// 当帧光源快照（可缺席 ⇒ 后端兜底默认光照）。
    pub lights: Option<&'a LightQueue>,
    /// 内容纹理层的当帧录制结果（可缺席）。
    pub content: Option<&'a ContentTextureQueue>,
    /// 形变项（可缺席）。
    pub morphs: Option<&'a MorphQueue>,
    /// 书写动画项（可缺席）。
    pub reveals: Option<&'a RevealQueue>,
    /// 合成后滤镜链（可缺席 ⇒ 直通）。
    pub filters: Option<&'a PostFilterChain>,
}

/// 从本帧视图与资源收集后端输入；`DrawQueue` / `DrawQueue3D` 缺席 ⇒ `None`。
pub fn build_frame_inputs<'a>(
    view: &SceneView,
    res: &'a Resources,
    options: FrameOptions,
) -> Option<FrameInputs<'a>> {
    let view_rect = res
        .get::<ViewRect>()
        .map(|v| v.rect)
        .unwrap_or_else(|| ViewRect::default().rect);
    let queue_2d = res.get::<DrawQueue>()?;
    let queue_3d = res.get::<DrawQueue3D>()?;
    let layers = view
        .layers
        .iter()
        .map(|layer| SceneLayer {
            id: layer.id,
            kind: layer.kind,
            blend: layer.blend,
            opacity: layer.clamped_opacity(),
        })
        .collect();
    Some(FrameInputs {
        layers,
        viewport: Rect2D::new(view_rect.min, view_rect.max),
        flip_layer_order: options.flip_layer_order,
        queue_2d,
        queue_3d,
        lights: res.get::<LightQueue>(),
        content: res.get::<ContentTextureQueue>(),
        morphs: res.get::<MorphQueue>(),
        reveals: res.get::<RevealQueue>(),
        filters: res.get::<PostFilterChain>(),
    })
}

/// 本帧的**绘制调用数**（2D 形状命令 + 3D mesh/line 条数）。
///
/// B2-1 起 2D / 3D 共处同一个 [`DrawQueue`]，故只统计一次（避免重复计入）。
pub fn draw_call_count(res: &Resources) -> usize {
    res.get::<DrawQueue>()
        .map_or(0, |q| q.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_scene_core::{Camera3D, Framing2D, Projection};
    use zela_scene_ecs::contract::WorldScene;

    fn spawn_3d_camera(world: &mut hecs::World) {
        world.spawn((Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
        ),));
    }

    fn spawn_2d_camera(world: &mut hecs::World) -> Camera2DHandle {
        let mut scene = WorldScene::new(world);
        ze_scene_core::spawn_camera2d(&mut scene, Framing2D::default()).expect("Framing2D 已登记")
    }

    /// headless：无 egui 上下文即可构建 SceneView；2D/3D 各一层、LayerId 唯一。
    #[test]
    fn build_scene_view_declares_2d_and_3d_layers() {
        let mut world = hecs::World::new();
        spawn_3d_camera(&mut world);
        let handle = spawn_2d_camera(&mut world);

        let mut res = Resources::new();
        res.insert(handle);
        res.insert(ViewRect::default());

        let view = build_scene_view(
            &zela_scene_ecs::contract::WorldView::new(&world),
            &res,
            FrameOptions::default(),
        );
        assert_eq!(view.layers.len(), 2);
        assert!(matches!(view.layers[0].kind, LayerKind::Viewport2D { .. }));
        assert!(matches!(view.layers[1].kind, LayerKind::Viewport3D { .. }));
        for (i, a) in view.layers.iter().enumerate() {
            for b in view.layers.iter().skip(i + 1) {
                assert_ne!(a.id, b.id, "图层 LayerId 必须两两不相等");
            }
        }
    }

    /// 图层总闸：关掉 2D/3D 后不再声明对应图层。
    #[test]
    fn build_scene_view_respects_layer_toggles() {
        let mut world = hecs::World::new();
        spawn_3d_camera(&mut world);
        let handle = spawn_2d_camera(&mut world);

        let mut res = Resources::new();
        res.insert(handle);
        res.insert(ViewRect::default());

        let view = build_scene_view(
            &zela_scene_ecs::contract::WorldView::new(&world),
            &res,
            FrameOptions {
                show_2d: true,
                show_3d: false,
                flip_layer_order: false,
            },
        );
        assert_eq!(view.layers.len(), 1);
        assert!(matches!(view.layers[0].kind, LayerKind::Viewport2D { .. }));
    }
}
