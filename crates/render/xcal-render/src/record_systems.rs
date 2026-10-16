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
// - `SceneRecordSystem`        RenderCommit，`R=(ViewRect, Camera2DHandle)`，`W=(DrawQueue,)`
// - `ContentTextureRecordSystem` RenderCommit，`R=(ContentTextureSpec,)`，`W=(ContentTextureQueue,)`
// - `MorphRecordSystem`        RenderCommit，`R=(ViewRect, Camera2DHandle)`，`W=(MorphQueue,)`
// - `RevealRecordSystem`       RenderCommit，`R=(ViewRect, Camera2DHandle)`，`W=(RevealQueue,)`
//
// 四个写集类型不相交，可同相注册（debug 的 BorrowTracker 不会报冲突）。
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

use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xcal_scene::contract::{SceneRead, WorldView};
use xcal_scene::domain::ContentTextureSpec;
use xcal_scene::scene::components::{ContentTextureItem, MorphShape, Reveal};

use xc_object::{Camera2DHandle, Path};

use crate::draw::{
    ContentTextureQueue, DrawQueue, MorphQueue, RevealQueue, push_morph_item, push_reveal_item,
    record_path_stroke, record_scene,
};
use crate::view_rect::ViewRect;
use xc_render_ir::StrokeSolid;

/// 本帧的相机（取景参数 + 视口矩形）解析；句柄失效 / 无相机对象 ⇒ `None`。
fn resolve_camera(ctx: &mut ExecutorContext<'_>, rect: xc_notion::Rect2D) -> Option<xc_object::Camera2D> {
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
    type Reads = (ViewRect, Camera2DHandle);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        // 同时需要 &hecs::World（不可变）与 &mut DrawQueue；
        // 通过 split-borrow 辅助 API 在闭包内同时持有两个引用。
        ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, queue| {
            // `record_scene` 只吃契约层 `SceneRead`；hecs 侧经只读适配器
            // `WorldView` 接入 —— 渲染管线不认识 hecs（不可变借用，渲染只需要读）。
            record_scene(&WorldView::new(world), &camera, queue);
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
    type Reads = (ViewRect, Camera2DHandle);
    type Writes = (MorphQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        ctx.with_world_and_resource_mut::<MorphQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            // 契约层查询（原为内联 `world.query::<(&MorphShape, &StrokeSolid)>()`）：
            // 世界 → 屏幕的二端几何映射 + 描边宽度换算都在 `push_morph_item`（唯一换算点）。
            scene.for_each_pair::<MorphShape, StrokeSolid>(|_, morph, stroke| {
                push_morph_item(
                    queue,
                    &morph.from,
                    &morph.to,
                    morph.t,
                    morph.samples,
                    stroke,
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
    type Reads = (ViewRect, Camera2DHandle);
    type Writes = (RevealQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let Some(camera) = resolve_camera(ctx, rect) else {
            return;
        };

        ctx.with_world_and_resource_mut::<RevealQueue, _>(|world, queue| {
            queue.0.clear();
            let scene = WorldView::new(world);
            scene.for_each_pair::<Reveal, StrokeSolid>(|_, reveal, stroke| {
                push_reveal_item(
                    queue,
                    &reveal.path,
                    reveal.start,
                    reveal.end,
                    stroke,
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

#[cfg(test)]
mod tests {
    use super::*;
    // ⚠️ 测试用 `hecs::World`（Scheduler 的容器就是它）——`hecs` 只在 **dev-dependencies**，
    // 库本体（M3-7 纪律：本 crate 不认识 hecs）不受影响。
    use hecs::World;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use xcal_scene::contract::WorldScene;
    use xcal_scene::scene::components::TextureHandle;
    use xcal_scene::types::Vec2;
    use xc_notion::{Path2D, PathCommand, Rect2D};
    use xc_object::{Circle, Framing2D};

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

        let mut res = resources_with(
            Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
            handle,
        );
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
            &[Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)],
            true,
        );
        let mut morph = MorphShape::new(path.clone(), path);
        morph.t = 0.25;
        world.spawn((morph, StrokeSolid::new(2.0, (0, 255, 0, 255))));

        let mut res = resources_with(
            Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
            handle,
        );
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
}
