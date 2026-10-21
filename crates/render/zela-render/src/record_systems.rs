// record_systems.rs — 三个**录制** System（P4：从 `zela-egui` 归位到渲染层）。
//
// # 为什么归位
//
// 它们本来就是**渲染动作**（把场景录进队列），却住在 UI 层，且 Reads 里带着
// `ViewRect` / `AppCamera` —— 那两个 Resource 定义在 `zela-egui`。
// 即："render 的事请住在 UI 层，且靠 UI 定义的类型活着"；换 UI（或加 headless
// 烘焙器）就得连 Resource 一起搬。归位后本模块只读**渲染层**与**契约层**的类型。
//
// # 阶段分配（与迁入前逐位一致）
//
// - `SceneRecordSystem`        RenderCommit，`R=(ViewRect, Camera2DHandle, GroupVisibility)`，`W=(DrawQueue,)`
// - `ContentTextureRecordSystem` RenderCommit，`R=(ContentTextureSpec,)`，`W=(ContentTextureQueue,)`
// - `MorphRecordSystem`        RenderCommit，`R=(ViewRect, Camera2DHandle, GroupVisibility)`，`W=(MorphQueue,)`
// - `RevealRecordSystem`       RenderCommit，`R=(ViewRect, Camera2DHandle, GroupVisibility)`，`W=(RevealQueue,)`
//
// 四个写集类型不相交，可同相注册（debug 的 BorrowTracker 不会报冲突）。
//
// # 对象级可见性 / 不透明度（N3 / N4 / N5）
//
// 三个 2D 录制 System 都消费对象级的 [`Visibility`](ze_scene_contract::Visibility) /
// [`Opacity`](ze_render_ir::Opacity)（缺席 = 全局可见 / 1.0）：
// - `camera_tag` 取自 `Camera2DHandle`（`VisibilityTag::Camera` 判定用）；
// - 组级开关 `GroupVisibility` 是可缺席资源（缺席 = 默认空 = 所有组可见）。
//
// ⚠️ **`ContentTextureRecordSystem` 本轮不参与过滤**（记账）：内容纹理层是**纹理源**
// 而非显示层（它的"相机"是 `spec.camera`，并非当前 2D 相机），按相机 tag 过滤没有
// 明确语义；且它不受图层总闸约束。若将来出现"内容层也要跟着对象走"的需求再补。
//
// # 相机从哪来（P1 之后）
//
// 场景里的 2D 相机对象持 `Framing2D`（取景参数，无 rect）；本模块用
// [`Camera2DHandle`] 指认它，**在录制时绑定** [`ViewRect`] 得到本帧 `Camera2D`
// —— `record_scene` 的签名（`&Camera2D`）因此不用动（§9.1）。
//
// # 与 hecs 的接触面（M3-7 纪律不变）
//
// 本 crate **仍不认识 hecs**：`WorldView` 把 `&hecs::World` 接成契约层
// [`SceneRead`]（hecs 侧适配器在 `zela-scene`），三个 System 只经契约层的
// `for_each*` / `with` 取数据。原先 `MorphRecordSystem` / `ContentTextureRecordSystem`
// 的**内联 hecs 查询**（`world.query::<..>()`）随归位一并改为契约层查询
// —— 这是把 "换场景实现要整体重写" 变成 "只需换适配器"。

use std::sync::Arc;

use zela_scene::contract::{SceneRead, WorldView};
use zela_scene::domain::ContentTextureSpec;
use zela_scene::scene::components::{
    ContentTextureItem, Light, LightKind, Material3D, MeshGeometry, MorphShape, Reveal,
};
use zela_scene::transform_hierarchy::WorldTransforms;
use zela_schedule::context::ExecutorContext;
use zela_schedule::system::System;

use ze_object::{Camera2DHandle, Path, Transform, ZOrder};

use crate::draw::{
    ContentTextureQueue, DrawQueue, MorphQueue, RevealQueue, opacity_of, push_morph_item,
    push_reveal_item, record_path_stroke, record_scene_tagged_with, visible_for,
};
use crate::draw_queue_3d::DrawQueue3D;
use crate::light_queue::{DirectionalLightRecord, LightQueue, PointLightRecord};
use crate::view_rect::ViewRect;
use ze_render_ir::StrokeSolid;
// N5：组级开关（可缺席资源 ⇒ 缺席 = 所有组可见）。
use ze_scene_contract::GroupVisibility;

/// 本帧的相机（取景参数 + 视口矩形）解析；句柄失效 / 无相机对象 ⇒ `None`。
fn resolve_camera(
    ctx: &mut ExecutorContext<'_>,
    rect: ze_notion::Rect2D,
) -> Option<ze_object::Camera2D> {
    let handle = *ctx.get::<Camera2DHandle>()?;
    let world = ctx.borrow_world();
    handle.resolve(&WorldView::new(world), rect)
}

// ─── SceneRecordSystem ─────────────────────────────────────────────────────

/// 2D 场景录制（RenderCommit）：把场景对象录进 [`DrawQueue`]。
///
/// 本 System **自己不认识具体组件**：7 个 `(图元, 样式)` 组合的枚举与翻译都在
/// `record_scene` 里。本 System 只负责"取数据"：取相机 → 取世界 → 交给它翻译。
pub struct SceneRecordSystem;
impl System for SceneRecordSystem {
    // N5：`GroupVisibility` 进入 Read（组级开关；缺席 ⇒ 默认空 ⇒ 所有组可见）。
    type Reads = (ViewRect, Camera2DHandle, GroupVisibility, WorldTransforms);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        // N5：相机 tag（`VisibilityTag::Camera` 判定用）取自同一个 `Camera2DHandle`；
        // 组级开关是可缺席资源 ⇒ 缺席按默认（空集 = 所有组可见）处理。
        let camera_tag = ctx.get::<Camera2DHandle>().map(|h| h.0);
        let groups = ctx.get::<GroupVisibility>().cloned().unwrap_or_default();
        // S5：父子累乘后的世界仿射（资源很小；clone 一次以解开与 world 的借用）。
        let worlds = ctx.get::<WorldTransforms>().cloned();
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        // 同时需要 &hecs::World（不可变）与 &mut DrawQueue；
        // 通过 split-borrow 辅助 API 在闭包内同时持有两个引用。
        ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, queue| {
            // `record_scene_tagged_with` 只吃契约层 `SceneRead`；hecs 侧经只读适配器
            // `WorldView` 接入 —— 渲染管线不认识 hecs（不可变借用，渲染只需要读）。
            record_scene_tagged_with(
                &WorldView::new(world),
                &camera,
                camera_tag,
                &groups,
                worlds.as_ref(),
                queue,
            );
        });
    }
}

// ─── MorphRecordSystem（S4.3）──────────────────────────────────────────────

/// 形变录制（RenderCommit，S4.3）：把 `(MorphShape, StrokeSolid)` 对象录成
/// [`MorphItem`](crate::draw::MorphItem)（`from` / `to` 原样 + 逐帧 `t`）。
///
/// **不做 CPU 插值**：两端几何交给 2D 后端的形变管线（双 vertex buffer + `t`
/// uniform），因此几何帧间不变 ⇒ 三角化结果与 GPU 缓冲都命中缓存，
/// 每帧只多写一个 `t` uniform。CPU 侧的 `MorphApplySystem` 因此不再参与
/// 形变实体的渲染（本 System 与它二选一；App 用本 System）。
///
/// 为什么是独立 System 而不是并进 [`SceneRecordSystem`]：既有 System 的
/// `Reads` / `Writes` 是冻结契约（§9.3），不能给它新增写资源。
pub struct MorphRecordSystem;
impl System for MorphRecordSystem {
    // N5：与 `SceneRecordSystem` 同口径（过滤 + 组级开关）。
    type Reads = (ViewRect, Camera2DHandle, GroupVisibility);
    type Writes = (MorphQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let camera_tag = ctx.get::<Camera2DHandle>().map(|h| h.0);
        let groups = ctx.get::<GroupVisibility>().cloned().unwrap_or_default();
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        ctx.with_world_and_resource_mut::<MorphQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            // 契约层查询（原为内联 `world.query::<(&MorphShape, &StrokeSolid)>()`）：
            // 世界 → 屏幕的二端几何映射 + 描边宽度换算都在 `push_morph_item`（唯一换算点）。
            //
            // N5：闭包首参即 `ObjectId`，直接用 `scene.with::<..>` 点查询做可见性过滤与
            // 不透明度取值 —— 契约层**不必**新增访客（`for_each_pair` 的 3 个位置已占满）。
            scene.for_each_pair::<MorphShape, StrokeSolid>(|id, morph, stroke| {
                if !visible_for(&scene, id, camera_tag, &groups) {
                    return;
                }
                push_morph_item(
                    queue,
                    &morph.from,
                    &morph.to,
                    morph.t,
                    morph.samples,
                    stroke,
                    opacity_of(&scene, id),
                    &camera,
                );
            });
        });
    }
}

// ─── RevealRecordSystem（R0 书写动画）──────────────────────────────────────

/// 书写动画录制（RenderCommit，R0）：把 `(Reveal, StrokeSolid)` 对象录成
/// [`RevealItem`](crate::draw::RevealItem)（路径原样 + 逐帧区间 `[start, end]`）。
///
/// 与 [`MorphRecordSystem`] 完全同一模式（独立队列 + 每项一次 draw + 动态 uniform
/// 偏移）：几何帧间不变 ⇒ 三角化与 GPU 缓冲都命中缓存，每帧只多写一个区间 uniform。
///
/// **不做**任何路径预处理：预细分（`REVEAL_STEP_PX`）与弧长归一化都在后端三角化时
/// 完成（`tessellate_reveal`），因为那属于几何口径、必须与三角化同处一地。
///
/// 为什么是独立 System 而不是并进 [`SceneRecordSystem`]：既有 System 的
/// `Reads` / `Writes` 是冻结契约（§9.3），不能给它新增写资源。
pub struct RevealRecordSystem;
impl System for RevealRecordSystem {
    // N5：与 `SceneRecordSystem` / `MorphRecordSystem` 同口径。
    type Reads = (ViewRect, Camera2DHandle, GroupVisibility, WorldTransforms);
    type Writes = (RevealQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let camera_tag = ctx.get::<Camera2DHandle>().map(|h| h.0);
        let groups = ctx.get::<GroupVisibility>().cloned().unwrap_or_default();
        let worlds = ctx.get::<WorldTransforms>().cloned();
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        ctx.with_world_and_resource_mut::<RevealQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            // N5：点查询做可见性过滤 + 不透明度取值（与 `MorphRecordSystem` 同法）。
            scene.for_each_pair::<Reveal, StrokeSolid>(|id, reveal, stroke| {
                if !visible_for(&scene, id, camera_tag, &groups) {
                    return;
                }
                // S2（G4）+ S5：Reveal 消费**世界**变换（S1：2D 读投影视图；世界累乘优先，缺席回退 local）。
                let local = scene.with::<ze_object::Transform, _>(id, |tr| tr.to_affine2d());
                let transform = worlds
                    .as_ref()
                    .and_then(|w| w.get2d(id))
                    .or(local);
                let path = match transform {
                    Some(transform) => reveal.path.transformed(&transform),
                    None => reveal.path.clone(),
                };
                push_reveal_item(
                    queue,
                    &path,
                    reveal.start,
                    reveal.end,
                    stroke,
                    opacity_of(&scene, id),
                    &camera,
                );
            });
        });
    }
}

// ─── ContentTextureRecordSystem ────────────────────────────────────────────

/// 内容纹理层录制（RenderCommit，S3-3）。
///
/// 只录**带 [`ContentTextureItem`] 标记**的路径描边对象到 [`ContentTextureQueue`]
/// （网格线 / 坐标轴等未标记对象不进贴图，见计划 D5）。
///
/// 位置与描边宽度经 [`record_path_stroke`] 按 `spec.camera` 换算——与主 2D 层同一口径
/// （唯一换算点在 zela-render 内）。
pub struct ContentTextureRecordSystem;
impl System for ContentTextureRecordSystem {
    type Reads = (ContentTextureSpec,);
    type Writes = (ContentTextureQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let Some(spec) = ctx.get::<ContentTextureSpec>().copied() else {
            return;
        };

        ctx.with_world_and_resource_mut::<ContentTextureQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            // `for_each_pair_opt`：标记缺席（`None`）的对象被过滤掉。
            scene.for_each_pair_opt::<Path, StrokeSolid, ContentTextureItem>(
                |_, path, stroke, mark| {
                    if mark.is_some() {
                        record_path_stroke(&mut queue.0, path, stroke, &spec.camera);
                    }
                },
            );
        });
    }
}

// ─── SceneRecord3DSystem（A6：从 app 归位到渲染层）─────────────────────────

/// 3D 图层录制（RenderCommit）：每帧清空 [`DrawQueue3D`]，把世界中的全部
/// `Arc<MeshGeometry>`（带可选 [`Material3D`] 与可选 [`Transform`]）录进队列；
/// 同时把世界中的全部 [`Light`] 组件录进 [`LightQueue`]（B1-A 光照数据化）。
///
/// 几何以 `Arc` 持有：此处仅克隆引用计数，不再深拷贝顶点数据。
pub struct SceneRecord3DSystem;
impl System for SceneRecord3DSystem {
    // S1：3D 录制改消费 `WorldTransforms`（世界矩阵优先，缺席回退 local ——
    // 缺席行为逐位不变）。Reads 变更影响 system 注册相位，见 `register_record_systems`。
    type Reads = (WorldTransforms,);
    // B1-A：新增写集 `LightQueue`（光源快照；缺资源时由 `init_resources` 注入默认空队列）。
    type Writes = (DrawQueue3D, LightQueue);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let worlds = ctx.get::<WorldTransforms>().cloned();
        // 光源：清空并录制。方向光的 `direction` 声明在组件内；点光（B1-B）的
        // **位置**由 `Transform` → `WorldTransforms` 解析 —— `Light` 组件本身
        // 不带 position（单一真源，铁律 3）。
        ctx.with_world_and_resource_mut::<LightQueue, _>(|world, lights| {
            lights.clear();
            let scene = WorldView::new(world);
            scene.for_each::<Light>(|id, light| {
                match light.kind {
                    LightKind::Directional { direction } => {
                        lights.directional.push(DirectionalLightRecord::new(
                            direction,
                            light.color,
                            light.intensity,
                        ));
                    }
                    LightKind::Point { range } => {
                        // 位置解析：世界矩阵优先（父子累乘的平移列），缺席
                        // WorldTransforms / 该对象 ⇒ local `Transform::position`，
                        // 再无 ⇒ 原点（缺位置对象默认在原点，不静默丢弃光源）。
                        let position = worlds
                            .as_ref()
                            .and_then(|w| w.get(id))
                            .map(|t| t.matrix.transform_point3(ze_notion::Vec3::ZERO))
                            .or_else(|| scene.with::<Transform, _>(id, |t| t.position))
                            .unwrap_or_default();
                        lights.points.push(PointLightRecord::new(
                            position,
                            light.color,
                            light.intensity,
                            range,
                        ));
                    }
                }
            });
        });
        ctx.with_world_and_resource_mut::<DrawQueue3D, _>(|world, queue| {
            // B2-1：统一队列里 2D / 3D 共存，只清 3D 载荷（顺序不保证，整队清会擦掉 2D）。
            queue.clear_meshes_and_lines();
            let scene = WorldView::new(world);
            // 先收集（读借用），再统一入队（写借用已由 `queue` 独占）。
            let mut items = Vec::new();
            scene.for_each::<Arc<MeshGeometry>>(|id, mesh| {
                let material = scene
                    .with::<Material3D, _>(id, |material| *material)
                    .unwrap_or_default();
                // B2-1：统一层序（缺席 ⇒ 默认层 0），与 2D 共用同一个 ZOrder 概念。
                let z = scene
                    .with::<ZOrder, _>(id, |z| *z)
                    .unwrap_or_default();
                // S1：世界矩阵优先（父子累乘结果）；缺席该对象 / WorldTransforms ⇒ local，
                // 逐位等于旧行为（`transform.matrix()`）。
                let model = worlds
                    .as_ref()
                    .and_then(|w| w.get(id))
                    .map(|t| t.matrix)
                    .or_else(|| scene.with::<Transform, _>(id, |transform| transform.matrix()))
                    .unwrap_or(ze_notion::Mat4::IDENTITY);
                items.push((id, z, Arc::clone(mesh), model, material));
            });
            // B2-0a：逐对象录制带实体身份（G7 记账前提 2）；B2-1：带统一层序。
            for (id, z, mesh, model, material) in items {
                queue.draw_mesh_with(id, z, mesh, model, material);
            }
        });
    }
}

/// **唯一的渲染录制注册点**（A6：注册序列不再住在 app）。
///
/// 五个 System 的写集（`DrawQueue` / `ContentTextureQueue` / `MorphQueue` /
/// `RevealQueue` / `DrawQueue3D`）互不相交，可同相位注册。
pub fn register_record_systems(scheduler: &mut zela_schedule::scheduler::Scheduler) {
    use zela_schedule::scheduler::Phase;

    scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
    scheduler.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
    scheduler.add_system(Phase::RenderCommit, MorphRecordSystem);
    scheduler.add_system(Phase::RenderCommit, RevealRecordSystem);
    scheduler.add_system(Phase::RenderCommit, SceneRecord3DSystem);
}

#[cfg(test)]
mod tests {
    use super::*;
    // ⚠️ 测试用 `hecs::World`（Scheduler 的容器就是它）——`hecs` 只在 **dev-dependencies**，
    // 库本体（M3-7 纪律：本 crate 不认识 hecs）不受影响。
    use hecs::World;
    use ze_notion::{Path2D, PathCommand, Rect2D};
    use ze_object::{Circle, Framing2D};
    use ze_render_ir::Opacity;
    use zela_scene::contract::{GroupId, Visibility, WorldScene};
    use zela_scene::scene::components::TextureHandle;
    use zela_scene::types::Vec2;
    use zela_schedule::resource::Resources;
    use zela_schedule::scheduler::{Phase, Scheduler};

    /// 建一个含 2D 相机对象的场景（取景为 `framing`），返回世界与句柄。
    fn world_with_camera(framing: Framing2D) -> (World, Camera2DHandle) {
        let mut world = World::new();
        let handle = spawn_camera(&mut world, framing);
        (world, handle)
    }

    fn spawn_camera(world: &mut World, framing: Framing2D) -> Camera2DHandle {
        let mut scene = WorldScene::new(world);
        ze_object::spawn_camera2d(&mut scene, framing).expect("Framing2D 已登记")
    }

    fn resources_with(rect: Rect2D, handle: Camera2DHandle) -> Resources {
        let mut r = Resources::new();
        r.insert(ViewRect { rect });
        r.insert(handle);
        r
    }

    /// 默认视口 800×600、取景 scale 40 ⇒ 录到世界原点处的 2D 圆（半径 1 世界单位）。
    #[test]
    fn scene_record_uses_scene_camera_bound_to_view_rect() {
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));

        let mut res = resources_with(Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)), handle);
        res.insert(DrawQueue::new());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<DrawQueue>().unwrap();
        assert_eq!(q.len(), 1);
        let crate::shape::Shape2D::Circle { center, radius } = &q.commands()[0].shape else {
            panic!("expected Circle");
        };
        // 世界原点 → 视口中心 (400, 300)；半径 1 世界单位 × 40 = 40px。
        assert!((center.x - 400.0).abs() < 1e-3 && (center.y - 300.0).abs() < 1e-3);
        assert!((*radius - 40.0).abs() < 1e-3, "radius = {radius}");
    }

    /// **S2（G4）**：`Reveal` 必须消费逐对象 `Transform`（与 SceneRecord 同口径）。
    #[test]
    fn reveal_record_consumes_transform() {
        use crate::draw::RevealQueue;
        use ze_notion::Vec3;

        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        let path = Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(1.0, 0.0)], false);
        world.spawn((
            Reveal::new(path),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
            ze_object::Transform {
                position: Vec3::new(1.0, 0.0, 0.0),
                ..ze_object::Transform::default()
            },
        ));

        let mut res = resources_with(Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)), handle);
        res.insert(RevealQueue::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, RevealRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let queue = sched.resources().get::<RevealQueue>().unwrap();
        assert_eq!(queue.0.len(), 1);
        let mut pts = Vec::new();
        for cmd in &queue.0[0].path.commands {
            if let PathCommand::MoveTo(p) | PathCommand::LineTo(p) = cmd {
                pts.push(*p);
            }
        }
        assert_eq!(pts.len(), 2);
        // 世界 (0,0) + Transform (1,0) ⇒ 屏幕 (440,300)；世界 (1,0) ⇒ (480,300)。
        assert!((pts[0].x - 440.0).abs() < 1e-3, "p0 = {:?}", pts[0]);
        assert!((pts[1].x - 480.0).abs() < 1e-3, "p1 = {:?}", pts[1]);
    }

    /// 句柄失效（无相机对象）⇒ 不录任何东西（不 panic）。
    #[test]
    fn scene_record_skips_when_camera_missing() {
        let mut world = World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        let mut res = Resources::new();
        res.insert(ViewRect::default());
        res.insert(Camera2DHandle(ze_scene_contract::ObjectId::DANGLING));
        res.insert(DrawQueue::new());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(sched.resources().get::<DrawQueue>().unwrap().len(), 0);
    }

    /// 形变：一个 `(MorphShape, StrokeSolid)` 对象 ⇒ 一条 `MorphItem`（`t` 原样带过）。
    #[test]
    fn morph_record_emits_constant_endpoints_and_t() {
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        let path = Path2D::from_polyline(
            &[
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
            ],
            true,
        );
        let mut morph = MorphShape::new(path.clone(), path);
        morph.t = 0.25;
        world.spawn((morph, StrokeSolid::new(2.0, (0, 255, 0, 255))));

        let mut res = resources_with(Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)), handle);
        res.insert(MorphQueue::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, MorphRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<MorphQueue>().unwrap();
        assert_eq!(q.0.len(), 1);
        assert!((q.0[0].t - 0.25).abs() < 1e-6);
    }

    /// 内容纹理：只录带 `ContentTextureItem` 标记的对象（干扰圆不进队列）。
    #[test]
    fn content_texture_record_filters_unmarked() {
        let mut world = World::new();
        world.spawn((
            Path::new(Path2D::from_polyline(
                &[Vec2::new(-0.5, -0.5), Vec2::new(0.5, 0.5)],
                false,
            )),
            StrokeSolid::new(0.02, (255, 255, 255, 255)),
            ContentTextureItem,
        ));
        world.spawn((
            Circle::new((0.0, 0.0), 0.2),
            StrokeSolid::new(0.02, (255, 0, 0, 255)),
        ));

        let tex_rect = Rect2D::new(Vec2::ZERO, Vec2::new(512.0, 512.0));
        let mut res = Resources::new();
        res.insert(ContentTextureSpec {
            camera: ze_object::Camera2D::new(Vec2::ZERO, 25.0, tex_rect),
            size_px: [512, 512],
            texture: TextureHandle::new(1),
            composite: false,
        });
        res.insert(ContentTextureQueue::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<ContentTextureQueue>().unwrap();
        assert_eq!(q.0.len(), 1, "干扰项未被录制");
        // 录到的点必须落在纹理矩形内（相机 = 纹理矩形，世界原点居中）。
        for cmd in q.0.commands() {
            if let crate::shape::Shape2D::Path(p) = &cmd.shape {
                for c in &p.commands {
                    if let PathCommand::MoveTo(pt) | PathCommand::LineTo(pt) = c {
                        assert!(pt.x >= 0.0 && pt.x <= 512.0 && pt.y >= 0.0 && pt.y <= 512.0);
                    }
                }
            }
        }
    }

    // ─── N5：对象级可见性 / 不透明度过滤 ──────────────────────────────────

    fn rect_800x600() -> Rect2D {
        Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0))
    }

    /// 相机 tag 过滤：只对**当前相机**可见的对象被录制，`Camera(其它)` 的落选。
    #[test]
    fn scene_record_filters_by_camera_tag() {
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        let other = ze_scene_contract::ObjectId::new(999, 1);
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
            Visibility::for_camera(handle.0), // 当前相机 ⇒ 可见
        ));
        world.spawn((
            Circle::new((1.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
            Visibility::for_camera(other), // 别的相机 ⇒ 落选
        ));

        let mut res = resources_with(rect_800x600(), handle);
        res.insert(DrawQueue::new());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        assert_eq!(
            sched.resources().get::<DrawQueue>().unwrap().len(),
            1,
            "只录对当前相机可见的那个对象"
        );
    }

    /// 组级开关过滤：`GroupVisibility` 隐藏该组 ⇒ 随组走的对象不录，全局对象仍录。
    #[test]
    fn scene_record_group_switch_filters() {
        let g = GroupId::new(1);
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
            Visibility::of_group(g),
        ));
        world.spawn((
            Circle::new((1.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));

        let mut res = resources_with(rect_800x600(), handle);
        res.insert(DrawQueue::new());
        let mut groups = GroupVisibility::new();
        groups.hide(g);
        res.insert(groups);

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        assert_eq!(
            sched.resources().get::<DrawQueue>().unwrap().len(),
            1,
            "隐藏组的对象落选，全局对象仍录"
        );
    }

    /// 形变路径：不可见对象不录（队列为空）；可见对象的不透明度施加到样式 alpha。
    #[test]
    fn morph_record_honors_visibility_and_opacity() {
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        let path = Path2D::from_polyline(&[Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0)], false);

        let mut visible = MorphShape::new(path.clone(), path.clone());
        visible.t = 0.4;
        world.spawn((
            visible,
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
            Opacity::new(0.5), // 只缩 alpha：255 → 128
        ));
        world.spawn((
            MorphShape::new(path.clone(), path),
            StrokeSolid::new(2.0, (0, 255, 0, 255)),
            Visibility::for_camera(ze_scene_contract::ObjectId::new(999, 1)),
        ));

        let mut res = resources_with(rect_800x600(), handle);
        res.insert(MorphQueue::default());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, MorphRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<MorphQueue>().unwrap();
        assert_eq!(q.0.len(), 1, "不可见对象不录");
        let stroke = q.0[0].style.stroke.expect("形变项应有描边样式");
        assert_eq!(
            (stroke.r(), stroke.g(), stroke.b()),
            (255, 0, 0),
            "rgb 不动"
        );
        assert_eq!(stroke.a(), 128, "opacity = 0.5 ⇒ alpha 减半");
    }

    /// 书写动画路径：同上（不可见不录 + 不透明度只缩 alpha）。
    #[test]
    fn reveal_record_honors_visibility_and_opacity() {
        let (mut world, handle) = world_with_camera(Framing2D::new(Vec2::ZERO, 40.0));
        let path = Path2D::from_polyline(&[Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0)], false);

        world.spawn((
            Reveal::new(path.clone()),
            StrokeSolid::new(2.0, (10, 20, 30, 200)),
            Opacity::new(0.5), // 200 * 0.5 = 100
        ));
        world.spawn((
            Reveal::new(path),
            StrokeSolid::new(2.0, (0, 255, 0, 255)),
            Visibility::for_camera(ze_scene_contract::ObjectId::new(999, 1)),
        ));

        let mut res = resources_with(rect_800x600(), handle);
        res.insert(RevealQueue::default());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, RevealRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<RevealQueue>().unwrap();
        assert_eq!(q.0.len(), 1, "不可见对象不录");
        let stroke = q.0[0].style.stroke.expect("reveal 项应有描边样式");
        assert_eq!((stroke.r(), stroke.g(), stroke.b()), (10, 20, 30));
        assert_eq!(stroke.a(), 100, "200 * 0.5 = 100");
    }

    // ─── S1：3D 录制消费 `WorldTransforms` ─────────────────────────────────

    /// **S1-5 #2**：父子两级 3D，`DrawQueue3D` 的 model 矩阵 = **累乘结果**（不是 local）。
    #[test]
    fn record_3d_consumes_world_transforms() {
        use zela_scene::contract::object_id_of;
        use zela_scene::scene::primitives::wireframe_cube;
        use zela_scene::transform_hierarchy::TransformHierarchySystem;

        let parent_geom = Arc::new(wireframe_cube(1.0));
        let child_geom = Arc::new(wireframe_cube(0.5));
        let mut world = World::new();
        let parent = world.spawn((
            Arc::clone(&parent_geom),
            ze_object::Transform {
                position: ze_notion::Vec3::new(10.0, 0.0, 0.0),
                ..ze_object::Transform::default()
            },
        ));
        let parent_id = object_id_of(parent);
        // 子 local 平移 (0,5,0)，挂在 (10,0,0) 的父下 ⇒ 世界 = (10,5,0)。
        // 若系统不吃 WorldTransforms（旧行为），子 model 会停在 local (0,5,0)。
        let child = world.spawn((
            Arc::clone(&child_geom),
            ze_object::Transform {
                position: ze_notion::Vec3::new(0.0, 5.0, 0.0),
                ..ze_object::Transform::default()
            },
            ze_scene_contract::Parent(parent_id),
        ));
        let _child_id = object_id_of(child);

        let mut res = Resources::new();
        res.insert(DrawQueue3D::new());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::Prepare, TransformHierarchySystem);
        sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<DrawQueue3D>().unwrap();
        assert_eq!(q.mesh_count(), 2);
        // 用 Arc 指针精确指认父 / 子 mesh（不依赖查询顺序）。
        let mut parent_origin = None;
        let mut child_origin = None;
        for m in q.meshes() {
            if Arc::ptr_eq(&m.geometry, &parent_geom) {
                parent_origin = Some(m.transform.transform_point3(ze_notion::Vec3::ZERO));
            } else if Arc::ptr_eq(&m.geometry, &child_geom) {
                child_origin = Some(m.transform.transform_point3(ze_notion::Vec3::ZERO));
            }
        }
        assert_eq!(
            parent_origin,
            Some(ze_notion::Vec3::new(10.0, 0.0, 0.0)),
            "无父 ⇒ 世界 = local"
        );
        assert_eq!(
            child_origin,
            Some(ze_notion::Vec3::new(10.0, 5.0, 0.0)),
            "子 model 必须是累乘结果，不是 local"
        );
    }

    /// **S1-5 #3**：无 `Parent` 时 3D model 与旧 `transform.matrix()` **逐位相同**（回退不变性）。
    #[test]
    fn no_parent_3d_model_is_bitwise_local() {
        use zela_scene::contract::object_id_of;
        use zela_scene::scene::primitives::wireframe_cube;
        use zela_scene::transform_hierarchy::TransformHierarchySystem;

        let tr = ze_object::Transform {
            position: ze_notion::Vec3::new(1.0, -2.0, 3.0),
            rotation: ze_notion::Quat::from_rotation_z(0.7),
            scale: ze_notion::Vec3::new(2.0, 0.5, 1.0),
        };
        let expected = tr.matrix();
        let mut world = World::new();
        let e = world.spawn((Arc::new(wireframe_cube(1.0)), tr));
        let _id = object_id_of(e);

        let mut res = Resources::new();
        res.insert(DrawQueue3D::new());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::Prepare, TransformHierarchySystem);
        sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let q = sched.resources().get::<DrawQueue3D>().unwrap();
        assert_eq!(q.mesh_count(), 1);
        assert_eq!(
            q.meshes().next().unwrap().transform,
            expected,
            "无 Parent 时 3D model 必须逐位等于 local matrix"
        );
    }

    /// **B1-A 验收**：世界中的 `Light` 组件被录进 `LightQueue`（方向光逐字段保留，
    /// 颜色归一化到 [0,1]、强度原样）。`Light` **不带 position**（单一真源）。
    #[test]
    fn record_directional_lights_into_queue() {
        use ze_notion::Color32;

        let mut world = World::new();
        world.spawn((
            Light::directional_with(ze_notion::Vec3::new(0.0, 0.0, 1.0), Color32::WHITE, 1.0),
        ));
        world.spawn((
            Light::directional_with(
                ze_notion::Vec3::new(-1.0, 0.5, 0.2),
                Color32::new(255, 128, 0, 255),
                2.0,
            ),
        ));

        let mut res = Resources::new();
        res.insert(DrawQueue3D::new());
        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let lq = sched.resources().get::<LightQueue>().unwrap();
        assert_eq!(lq.directional.len(), 2);
        // 第一光：(0,0,1) 纯白强度 1
        assert_eq!(
            lq.directional[0].direction,
            ze_notion::Vec3::new(0.0, 0.0, 1.0)
        );
        assert_eq!(lq.directional[0].color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(lq.directional[0].intensity, 1.0);
        // 第二光：颜色归一化、强度保留
        assert_eq!(
            lq.directional[1].direction,
            ze_notion::Vec3::new(-1.0, 0.5, 0.2)
        );
        assert_eq!(lq.directional[1].color, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        assert_eq!(lq.directional[1].intensity, 2.0);
    }

    /// **B1-B 验收**：点光被录进 `LightQueue.points`，且位置**来自世界矩阵**
    /// （父子累乘），而非 `Light` 组件字段（单一真源：Light 不带 position）。
    #[test]
    fn record_point_lights_resolve_world_position() {
        use ze_notion::Color32;
        use zela_scene::contract::object_id_of;
        use zela_scene::scene::components::Light;
        use zela_scene::transform_hierarchy::TransformHierarchySystem;

        let mut world = World::new();
        // 父平移 (10, 0, 0)；点光挂在父下，local 平移 (0, 5, 0)
        // ⇒ 世界位置 = (10, 5, 0)。
        let parent = world.spawn((
            ze_object::Transform {
                position: ze_notion::Vec3::new(10.0, 0.0, 0.0),
                ..ze_object::Transform::default()
            },
        ));
        let parent_id = object_id_of(parent);
        world.spawn((
            Light::point_with(3.0, Color32::new(255, 128, 0, 255), 2.0),
            ze_object::Transform {
                position: ze_notion::Vec3::new(0.0, 5.0, 0.0),
                ..ze_object::Transform::default()
            },
            ze_scene_contract::Parent(parent_id),
        ));

        let mut res = Resources::new();
        res.insert(DrawQueue3D::new());
        let mut sched = Scheduler::new(world, res);
        // 世界矩阵由 TransformHierarchySystem 累乘（Prepare 相位，早于录制）。
        sched.add_system(Phase::Prepare, TransformHierarchySystem);
        sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let lq = sched.resources().get::<LightQueue>().unwrap();
        assert_eq!(lq.points.len(), 1);
        let p = &lq.points[0];
        // 世界位置 = 父 (10,0,0) + local (0,5,0) = (10,5,0)。
        assert_eq!(p.position, ze_notion::Vec3::new(10.0, 5.0, 0.0));
        // range / 颜色（归一化）/ 强度原样保留。
        assert_eq!(p.range, 3.0);
        assert_eq!(p.color, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        assert_eq!(p.intensity, 2.0);
        // 方向光队列不受影响（此场景无方向光）。
        assert!(lq.directional.is_empty());
    }
}
