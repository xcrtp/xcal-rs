// record_systems.rs — 三个**录制** System（P4：从 `xcal-egui` 归位到渲染层）。
//
// # 为什么归位
//
// 它们本来就是**渲染动作**（把场景录进队列），却住在 UI 层，且 Reads 里带着
// `ViewRect` / `AppCamera` —— 那两个 Resource 定义在 `xcal-egui`。
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
// 三个 2D 录制 System 都消费对象级的 [`Visibility`](xc_scene_contract::Visibility) /
// [`Opacity`](xc_render_ir::Opacity)（缺席 = 全局可见 / 1.0）：
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
// [`SceneRead`]（hecs 侧适配器在 `xcal-scene`），三个 System 只经契约层的
// `for_each*` / `with` 取数据。原先 `MorphRecordSystem` / `ContentTextureRecordSystem`
// 的**内联 hecs 查询**（`world.query::<..>()`）随归位一并改为契约层查询
// —— 这是把 "换场景实现要整体重写" 变成 "只需换适配器"。

use std::sync::Arc;

use xcal_scene::contract::{SceneRead, WorldView};
use xcal_scene::domain::ContentTextureSpec;
use xcal_scene::scene::components::{
    ContentTextureItem, Material3D, MeshGeometry, MorphShape, Reveal,
};
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

use xc_object::{Camera2DHandle, Path, Transform};

use crate::draw::{
    ContentTextureQueue, DrawQueue, MorphQueue, RevealQueue, opacity_of, push_morph_item,
    push_reveal_item, record_path_stroke, record_scene_tagged, visible_for,
};
use crate::draw_queue_3d::DrawQueue3D;
use crate::view_rect::ViewRect;
use xc_render_ir::StrokeSolid;
// N5：组级开关（可缺席资源 ⇒ 缺席 = 所有组可见）。
use xc_scene_contract::GroupVisibility;

/// 本帧的相机（取景参数 + 视口矩形）解析；句柄失效 / 无相机对象 ⇒ `None`。
fn resolve_camera(
    ctx: &mut ExecutorContext<'_>,
    rect: xc_notion::Rect2D,
) -> Option<xc_object::Camera2D> {
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
    type Reads = (ViewRect, Camera2DHandle, GroupVisibility);
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
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        // 同时需要 &hecs::World（不可变）与 &mut DrawQueue；
        // 通过 split-borrow 辅助 API 在闭包内同时持有两个引用。
        ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, queue| {
            // `record_scene_tagged` 只吃契约层 `SceneRead`；hecs 侧经只读适配器
            // `WorldView` 接入 —— 渲染管线不认识 hecs（不可变借用，渲染只需要读）。
            record_scene_tagged(&WorldView::new(world), &camera, camera_tag, &groups, queue);
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
    type Reads = (ViewRect, Camera2DHandle, GroupVisibility);
    type Writes = (RevealQueue,);

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

        ctx.with_world_and_resource_mut::<RevealQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            // N5：点查询做可见性过滤 + 不透明度取值（与 `MorphRecordSystem` 同法）。
            scene.for_each_pair::<Reveal, StrokeSolid>(|id, reveal, stroke| {
                if !visible_for(&scene, id, camera_tag, &groups) {
                    return;
                }
                push_reveal_item(
                    queue,
                    &reveal.path,
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
/// （唯一换算点在 xcal-render 内）。
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
/// `Arc<MeshGeometry>`（带可选 [`Material3D`] 与可选 [`Transform`]）录进队列。
///
/// 几何以 `Arc` 持有：此处仅克隆引用计数，不再深拷贝顶点数据。
pub struct SceneRecord3DSystem;
impl System for SceneRecord3DSystem {
    type Reads = ();
    type Writes = (DrawQueue3D,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        ctx.with_world_and_resource_mut::<DrawQueue3D, _>(|world, queue| {
            queue.clear();
            let scene = WorldView::new(world);
            // 先收集（读借用），再统一入队（写借用已由 `queue` 独占）。
            let mut items = Vec::new();
            scene.for_each::<Arc<MeshGeometry>>(|id, mesh| {
                let material = scene
                    .with::<Material3D, _>(id, |material| *material)
                    .unwrap_or_default();
                let model = scene
                    .with::<Transform, _>(id, |transform| transform.matrix())
                    .unwrap_or(glam::Mat4::IDENTITY);
                items.push((Arc::clone(mesh), model, material));
            });
            for (mesh, model, material) in items {
                queue.draw_mesh(mesh, model, material);
            }
        });
    }
}

/// **唯一的渲染录制注册点**（A6：注册序列不再住在 app）。
///
/// 五个 System 的写集（`DrawQueue` / `ContentTextureQueue` / `MorphQueue` /
/// `RevealQueue` / `DrawQueue3D`）互不相交，可同相位注册。
pub fn register_record_systems(scheduler: &mut xcal_schedule::scheduler::Scheduler) {
    use xcal_schedule::scheduler::Phase;

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
    use xc_notion::{Path2D, PathCommand, Rect2D};
    use xc_object::{Circle, Framing2D};
    use xc_render_ir::Opacity;
    use xcal_scene::contract::{GroupId, Visibility, WorldScene};
    use xcal_scene::scene::components::TextureHandle;
    use xcal_scene::types::Vec2;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    /// 建一个含 2D 相机对象的场景（取景为 `framing`），返回世界与句柄。
    fn world_with_camera(framing: Framing2D) -> (World, Camera2DHandle) {
        let mut world = World::new();
        let handle = spawn_camera(&mut world, framing);
        (world, handle)
    }

    fn spawn_camera(world: &mut World, framing: Framing2D) -> Camera2DHandle {
        let mut scene = WorldScene::new(world);
        xc_object::spawn_camera2d(&mut scene, framing).expect("Framing2D 已登记")
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
        res.insert(Camera2DHandle(xc_scene_contract::ObjectId::DANGLING));
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
            camera: xc_object::Camera2D::new(Vec2::ZERO, 25.0, tex_rect),
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
        let other = xc_scene_contract::ObjectId::new(999, 1);
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
            Visibility::for_camera(xc_scene_contract::ObjectId::new(999, 1)),
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
            Visibility::for_camera(xc_scene_contract::ObjectId::new(999, 1)),
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
}
