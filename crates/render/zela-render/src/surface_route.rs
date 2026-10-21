// surface_route.rs — Surface 路由：2D 图元**何时走 Direct（3D 网格）**（B3 Step 2）。
//
// # 规则（显式，不做内容类型推断）
// 对象的归属沿 `Parent` 链解析（[`ze_scene_core::resolve_surface`]）：
// - 无 `Surface` 祖先 ⇒ 默认 2D 路径（像素逐位不变）；
// - `SurfaceRenderer::Renderer2D` ⇒ 默认 2D 路径（Step 4 接纹理路径）；
// - `SurfaceRenderer::Renderer3D` ⇒ **Direct**：`derive_mesh` 后作为 `MeshDraw`
//   进同一 `DrawQueue`（与 3D 共用深度/层序）。
//
// # 显式失败（不静默）
// - `Surface` 缺 `SurfaceRenderer` / `Parent` 链成环 ⇒ [`SurfaceRecordError::Route`]；
// - Direct + `Screen` 映射 ⇒ [`SurfaceRecordError::ScreenMappingUnsupported`]
//   （3D 里的屏幕映射语义尚未定义，不猜）；
// - Direct + `Reveal` / `MorphShape` / `ContentTextureItem` ⇒
//   [`SurfaceRecordError::UnsupportedCapability`]（advance 槽 / 形变 / 纹理不在网格里）；
// - 派生失败 ⇒ [`SurfaceRecordError::Derive`]。
//
// 错误写进 [`DrawQueue::surface_errors`]，由宿主读取；**不影响**其余对象录制。

use ze_notion::{Mat4, Transform2D};
use ze_render_ir::{ResolveStyle, ToShape2D, ZOrder};
use ze_scene_contract::{ObjectId, SceneRead};
use ze_scene_core::components::{ContentTextureItem, MorphShape, Reveal};
use ze_scene_core::{
    resolve_surface, Surface, SurfaceError, SurfaceMapping, SurfaceOwner, SurfaceRenderer, Transform,
    WorldTransforms,
};

use crate::draw::{apply_opacity, opacity_of, DrawItem, DrawQueue};
use crate::draw_queue_3d::MeshDraw;
use crate::shape_to_mesh::{derive_mesh, ShapeMeshError};

/// Surface 路由结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRoute {
    /// 无 Surface 归属 / 策略为 `Renderer2D`：走默认 2D 路径。
    Default,
    /// 策略为 `Renderer3D`：内容派生为 3D 网格。
    Direct(SurfaceOwner),
}

/// Surface 路由 / Direct 派生失败（显式错误）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SurfaceRecordError {
    #[error("对象 {id:?} 的 Surface 归属无效：{source}")]
    Route { id: ObjectId, source: SurfaceError },
    #[error("Direct 路径不支持 {kind}（对象 {id:?}）；请把该 Surface 改为 Renderer2D")]
    UnsupportedCapability { id: ObjectId, kind: &'static str },
    #[error("Direct 路径要求 SurfaceMapping::World（对象 {id:?}）；Screen 映射的 3D 语义未定义")]
    ScreenMappingUnsupported { id: ObjectId },
    #[error("Direct 路径派生失败（对象 {id:?}）：{source}")]
    Derive { id: ObjectId, source: ShapeMeshError },
}

/// 解析某对象的 Surface 路由（纯查询，不写队列）。
pub fn surface_route<S: SceneRead>(
    scene: &S,
    id: ObjectId,
) -> Result<SurfaceRoute, SurfaceError> {
    match resolve_surface(scene, id)? {
        Some(owner) if owner.renderer == SurfaceRenderer::Renderer3D => {
            Ok(SurfaceRoute::Direct(owner))
        }
        _ => Ok(SurfaceRoute::Default),
    }
}

/// 尝试把对象按 Direct 路径录制。
///
/// 返回 `true` ⇒ 已处理（Direct 或显式错误），调用方**不得**再走 2D 路径；
/// 返回 `false` ⇒ 默认 2D 路径（调用方继续原有录制）。
pub(crate) fn try_record_direct<S, A, B>(
    scene: &S,
    id: ObjectId,
    shape_component: &A,
    style_component: &B,
    z: Option<ZOrder>,
    worlds: Option<&WorldTransforms>,
    queue: &mut DrawQueue,
) -> bool
where
    S: SceneRead,
    A: ToShape2D + Send + Sync + 'static,
    B: ResolveStyle + Send + Sync + 'static,
{
    let route = match surface_route(scene, id) {
        Ok(route) => route,
        Err(source) => {
            queue.push_surface_error(SurfaceRecordError::Route { id, source });
            return true;
        }
    };
    let SurfaceRoute::Direct(owner) = route else {
        return false;
    };

    // Direct 不支持的能力：显式报错，不静默降级。
    if scene.with::<Reveal, _>(id, |_| ()).is_some() {
        queue.push_surface_error(SurfaceRecordError::UnsupportedCapability {
            id,
            kind: "Reveal",
        });
        return true;
    }
    if scene.with::<MorphShape, _>(id, |_| ()).is_some() {
        queue.push_surface_error(SurfaceRecordError::UnsupportedCapability {
            id,
            kind: "MorphShape",
        });
        return true;
    }
    if scene.with::<ContentTextureItem, _>(id, |_| ()).is_some() {
        queue.push_surface_error(SurfaceRecordError::UnsupportedCapability {
            id,
            kind: "ContentTextureItem",
        });
        return true;
    }

    // 映射语义：Direct 只定义 World（Screen 需要相机，语义未定 ⇒ 显式拒绝）。
    let mapping = scene
        .with::<Surface, _>(owner.surface, |surface| surface.mapping)
        .unwrap_or(SurfaceMapping::World);
    if mapping != SurfaceMapping::World {
        queue.push_surface_error(SurfaceRecordError::ScreenMappingUnsupported { id });
        return true;
    }

    // 本地几何 + 世界仿射：Direct 不使用相机仿射（相机是 3D 的）。
    let local_shape = shape_component.to_shape2d(&Transform2D::identity());
    // 1 px = 1 世界单位（与 `derive_mesh` 的 tolerance 同一约定）。
    let mut style = style_component.resolve_style(1.0);
    apply_opacity(&mut style, opacity_of(scene, id));

    let derived = match derive_mesh(&local_shape, &style) {
        Ok(derived) => derived,
        Err(source) => {
            queue.push_surface_error(SurfaceRecordError::Derive { id, source });
            return true;
        }
    };

    let world = worlds
        .and_then(|worlds| worlds.get2d(id))
        .or_else(|| scene.with::<Transform, _>(id, |transform| transform.to_affine2d()))
        .unwrap_or_else(Transform2D::identity);
    let model = Mat4::from_mat3(world.matrix);

    // Step 4a-A：fill / stroke 各一份网格。
    for part in derived {
        queue.push_item(DrawItem::Mesh(
            MeshDraw::new(part.geometry, model, part.material)
                .with_entity(id)
                .with_z(z.unwrap_or_default()),
        ));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use hecs::World;
    use ze_notion::{Path2D, Rect2D, Vec2};
    use ze_render_ir::{FillSolid, StrokeSolid};
    use ze_scene_contract::Parent;
    use ze_scene_core::{Camera2D, Circle};
    use ze_scene_hecs::{object_id_of, WorldView};

    use crate::draw::record_scene;

    fn camera() -> Camera2D {
        Camera2D::new(
            Vec2::ZERO,
            1.0,
            Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
        )
    }

    fn world_bounds() -> Rect2D {
        Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 100.0))
    }

    /// 建一个 Surface 对象；`renderer = None` ⇒ 缺策略。
    fn spawn_surface(
        world: &mut World,
        mapping: SurfaceMapping,
        renderer: Option<SurfaceRenderer>,
    ) -> ObjectId {
        let surface = Surface::new(world_bounds(), mapping, Vec2::ZERO);
        match renderer {
            Some(renderer) => object_id_of(world.spawn((surface, renderer))),
            None => object_id_of(world.spawn((surface,))),
        }
    }

    /// Renderer3D ⇒ 子对象派生为网格，不再产生 2D 命令。
    #[test]
    fn direct_surface_records_mesh_instead_of_shape() {
        let mut world = World::new();
        let surface = spawn_surface(
            &mut world,
            SurfaceMapping::World,
            Some(SurfaceRenderer::Renderer3D),
        );
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            FillSolid::new((255, 255, 255, 255)),
            Parent(surface),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert_eq!(queue.commands().len(), 0, "Direct 不得再录 2D 命令");
        assert_eq!(queue.meshes().count(), 1, "Direct 应派生为一份网格");
        assert!(queue.surface_errors().is_empty());
    }

    /// Renderer2D ⇒ 默认 2D 路径（Step 4 才接纹理）。
    #[test]
    fn renderer2d_keeps_default_shape_path() {
        let mut world = World::new();
        let surface = spawn_surface(
            &mut world,
            SurfaceMapping::World,
            Some(SurfaceRenderer::Renderer2D),
        );
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            FillSolid::new((255, 255, 255, 255)),
            Parent(surface),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert_eq!(queue.commands().len(), 1);
        assert_eq!(queue.meshes().count(), 0);
        assert!(queue.surface_errors().is_empty());
    }

    /// 无 Surface ⇒ 行为不变（默认 2D）。
    #[test]
    fn no_surface_keeps_default_shape_path() {
        let mut world = World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            FillSolid::new((255, 255, 255, 255)),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert_eq!(queue.commands().len(), 1);
        assert_eq!(queue.meshes().count(), 0);
    }

    /// 缺 SurfaceRenderer ⇒ 显式错误（不静默取默认策略）。
    #[test]
    fn missing_renderer_is_explicit_error() {
        let mut world = World::new();
        let surface = spawn_surface(&mut world, SurfaceMapping::World, None);
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            FillSolid::new((255, 255, 255, 255)),
            Parent(surface),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert!(matches!(
            queue.surface_errors(),
            [SurfaceRecordError::Route {
                source: SurfaceError::MissingRenderer(_),
                ..
            }]
        ));
        assert_eq!(queue.commands().len(), 0);
    }

    /// Direct + Screen 映射 ⇒ 显式错误（3D 语义未定义，不猜）。
    #[test]
    fn direct_screen_mapping_is_explicit_error() {
        let mut world = World::new();
        let surface = spawn_surface(
            &mut world,
            SurfaceMapping::Screen,
            Some(SurfaceRenderer::Renderer3D),
        );
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            FillSolid::new((255, 255, 255, 255)),
            Parent(surface),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert!(matches!(
            queue.surface_errors(),
            [SurfaceRecordError::ScreenMappingUnsupported { .. }]
        ));
        assert_eq!(queue.meshes().count(), 0);
    }

    /// Direct + Reveal ⇒ 显式错误（advance 槽无处安放）。
    #[test]
    fn direct_reveal_is_explicit_error() {
        let mut world = World::new();
        let surface = spawn_surface(
            &mut world,
            SurfaceMapping::World,
            Some(SurfaceRenderer::Renderer3D),
        );
        let path = Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(1.0, 0.0)], false);
        world.spawn((
            ze_scene_core::Path::new(path.clone()),
            StrokeSolid::new(2.0, (255, 255, 255, 255)),
            Reveal::new(path),
            Parent(surface),
        ));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera(), &mut queue);

        assert!(matches!(
            queue.surface_errors(),
            [SurfaceRecordError::UnsupportedCapability {
                kind: "Reveal",
                ..
            }]
        ));
        assert_eq!(queue.meshes().count(), 0);
    }
}
