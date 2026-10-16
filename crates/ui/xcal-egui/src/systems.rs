// systems.rs: 场景侧 System（新架构，B 阶段 UI 解耦后）
//
// 每个 System 显式声明 `Reads` / `Writes` 关联类型，使用
// `ctx.get` / `ctx.get_mut` 访问 Resource。
//
// B 阶段迁移（计划 §6.4）：
// - CameraInput 消费 `InputEvents`（UI 适配器 host.rs 翻译的 SceneEvent），
//   不再直接读 egui 输入——可 headless 测试。
// - ControlPanel（纯 UI chrome）移出 ECS → host.rs::draw_control_panel；
// - ViewRectUpdate（布局信息本就属 UI）→ host.rs::update_view_rect；
// - RenderSubmit（内容提交）→ host.rs::present。
//
// Phase 分配（避开同 phase 内 read/write 冲突）：
// - Input:        CameraInput  (R=InputEvents, W=AppCamera)
// - Prepare:      AxesRefresh / FunctionGraphRefresh（M4：几何在场景对象层，
//                 本层只做接线，故不再写 DrawQueue）
// - RenderCommit: SceneRecord  (R=ViewRect, AppCamera, W=DrawQueue)
//
// 注：时间推进由 DirectorAdvanceSystem（xcal-scene）负责，动画由 BindingApplySystem
// （xcal-scene::animation）驱动。

use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xcal_scene::contract::{SceneRead, WorldScene, WorldView};
use xcal_scene::domain::{CameraCommand, ContentTextureSpec, InputEvents, SceneEvent};
use xcal_scene::scene::components::{ContentTextureItem, MorphShape, PhaseAnim};
use xcal_render::components::{Path, StrokeSolid};
use xcal_render::{
    ContentTextureQueue, DrawQueue, MorphQueue, push_morph_item, record_path_stroke, record_scene,
};
use xc_object::{Axes2D, FunctionGraph};

use crate::resources::{AppCamera, UiConfig, ViewRect};

// ─── CameraInputSystem ─────────────────────────────────────────────────────

/// 相机输入（Input 阶段）：消费相机语义命令，驱动 [`AppCamera`]。
///
/// 只依赖 [`InputEvents`] 纯数据契约——可 headless 构造命令并驱动。
/// 本 System 不解释命令语义：翻译（egui→CameraCommand）在 host，
/// 命令→状态的控制器纯函数在 [`AppCamera::apply_command`]（PanZoom 模式）。
/// 当前只有 2D 视口产生相机命令，故全部路由到 AppCamera；
/// 3D Orbit 控制器接入后按 `layer` 分发。
pub struct CameraInputSystem;
impl System for CameraInputSystem {
    type Reads = (InputEvents,);
    type Writes = (AppCamera,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先拷贝命令（释放对 resources 的共享借用），再可变访问相机。
        let commands: Vec<CameraCommand> = match ctx.get::<InputEvents>() {
            Some(e) => e
                .iter()
                .filter_map(|ev| match ev {
                    SceneEvent::Camera { command, .. } => Some(*command),
                    _ => None,
                })
                .collect(),
            None => return,
        };
        if commands.is_empty() {
            return;
        }

        let Some(camera) = ctx.get_mut::<AppCamera>() else {
            return;
        };
        for command in commands {
            camera.apply_command(command);
        }
    }
}

// ─── AxesRefreshSystem（M4-4：原网格构建 System 退役）───────────────────────

/// 坐标轴 / 网格刷新（Prepare 阶段）。
///
/// **M4-4：原网格构建 System（本层旧 `Grid` 构建器）已退役。** 原先 279 行的四段几何逻辑
/// （相机裁剪、步长自适应、刻度线、坐标轴）全部归场景对象层的
/// [`xc_object::Axes2D`]；本 System 只剩**接线**：取本帧相机 → 逐个刷新场景里的
/// 坐标轴对象。本文件因此**不再有任何几何计算**，也不再把相机缩放带进任何
/// 宽度换算（换算只发生在 `xcal-render` 的唯一换算点）。
///
/// 语义与迁移前一致：`UiConfig::show_grid` 关闭时清空线段（容器对象保留），
/// 打开时按相机重建（线段数量随缩放变化，故必须"清旧建新"）。
pub struct AxesRefreshSystem;
impl System for AxesRefreshSystem {
    type Reads = (ViewRect, AppCamera, UiConfig);
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先取只读值（释放对 resources 的共享借用），再取 world 的可变借用。
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let show_grid = ctx.get::<UiConfig>().map(|c| c.show_grid).unwrap_or(true);
        let camera = cam.to_camera_2d(rect);

        let mut scene = WorldScene::new(ctx.borrow_world_mut());
        // 不能边遍历边写：先收集规格（克隆），再逐个刷新。
        let mut jobs = Vec::new();
        scene.for_each::<Axes2D>(|id, spec| jobs.push((id, spec.clone())));
        for (id, spec) in jobs {
            if show_grid {
                spec.refresh(&mut scene, id, &camera);
            } else {
                spec.clear(&mut scene, id);
            }
        }
    }
}

// ─── FunctionGraphRefreshSystem（M4-1：函数采样归对象层）────────────────────

/// 函数图像刷新（Prepare 阶段）：按对象自己的采样规格重采样折线。
///
/// **M4-1**：采样循环原先在本文件 `SceneRecordSystem` 内（逐点 `f(x + phase)`）；
/// 现归场景对象层 [`xc_object::FunctionGraph`]，本 System 只负责把当前相位
/// （由 `BindingApplySystem` 写入实体的 `PhaseAnim`）交给它。
///
/// 写路径是契约层 [`xc_scene_contract::Scene::set`]（M4-3b 的消费方）：
/// 采样点数固定 ⇒ 覆盖既有 `Path` 组件即可，无需清旧建新。
pub struct FunctionGraphRefreshSystem;
impl System for FunctionGraphRefreshSystem {
    type Reads = ();
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let mut scene = WorldScene::new(ctx.borrow_world_mut());
        let mut jobs = Vec::new();
        scene.for_each_pair::<FunctionGraph, PhaseAnim>(|id, graph, anim| {
            jobs.push((id, graph.clone(), anim.phase));
        });
        for (id, graph, phase) in jobs {
            graph.refresh(&mut scene, id, phase);
        }
    }
}

// ─── SceneRecordSystem ─────────────────────────────────────────────────────

/// 2D 场景录制（RenderCommit 阶段）：把场景对象录进 [`DrawQueue`]。
///
/// # 与 hecs 的接触面（替换 `Scene` 实现时要改的地方）
///
/// 本 System **自己不认识具体组件**：7 个 `(图元, 样式)` 组合的枚举与翻译都在
/// `xcal-render::record_scene` 里 —— `Circle` / `Path` / `Line` / `Rect` 分别配
/// `StrokeSolid` / `FillSolid`（`Line` 只有描边、`Rect` 两种都有），共 7 组。
/// 本 System 只负责"**取数据**"：拿到世界与队列，交给 `record_scene` 翻译。
///
/// 具体两步接线（也即"换实现时要重写"之处）：
/// 1. `ctx.with_world_and_resource_mut::<DrawQueue, _>` —— `ExecutorContext` 目前只提供
///    `&hecs::World`（不可变），用它同时借到世界与 `&mut DrawQueue`；
/// 2. `WorldView::new(world)` —— 把 hecs 世界接成契约层 `SceneRead`（hecs 侧适配器）。
///
/// 换成非 ECS 的场景实现时，`record_scene` 与组件组合枚举都不必改：只需把这 1、2 两步
/// 换成新实现的取数据方式（这正是 M3-7「record_scene 不认识 hecs」的收益）。
///
/// ⚠️ 与本文件另外两个录制 System 的区别：`ContentTextureRecordSystem` /
/// `MorphRecordSystem` **内联**了 hecs 查询（见各自的 doc），它们才需要随实现整体重写。
pub struct SceneRecordSystem;
impl System for SceneRecordSystem {
    type Reads = (ViewRect, AppCamera);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let camera = cam.to_camera_2d(rect);

        // 同时需要 &hecs::World（不可变）与 &mut DrawQueue；
        // 通过 split-borrow 辅助 API 在闭包内同时持有两个引用。
        ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, queue| {
            // M3-7：`record_scene` 只吃契约层 `SceneRead`；hecs 侧经只读适配器
            // `WorldView`（`xcal_scene::contract`）接入 —— 渲染管线不认识 hecs，
            // 且这里用的是不可变借用（渲染只需要读）。
            //
            // M4-1：函数图像的采样循环已归对象层 `FunctionGraph`（见
            // `FunctionGraphRefreshSystem`），本 System 只录制场景对象。
            record_scene(&WorldView::new(world), &camera, queue);
        });
    }
}

// ─── MorphRecordSystem（S4.3）──────────────────────────────────────────────

/// 形变录制（RenderCommit 阶段，S4.3）：把 `(MorphShape, StrokeSolid)` 实体
/// 录成 [`MorphItem`]（`from` / `to` 原样 + 逐帧 `t`）。
///
/// **不做 CPU 插值**：两端几何交给 2D 后端的形变管线（双 vertex buffer + `t`
/// uniform），因此几何帧间不变 ⇒ 三角化结果与 GPU 缓冲都命中缓存，
/// 每帧只多写一个 `t` uniform。CPU 侧的 `MorphApplySystem` 因此不再参与
/// 形变实体的渲染（本 System 与它二选一；App 用本 System）。
///
/// 换算口径与 [`SceneRecordSystem`] 完全一致（世界 → 屏幕的位置映射 +
/// 描边宽度按相机的唯一换算点解析）：两者都收在 `xcal-render` 内
/// （本 System 只调 [`push_morph_item`]）—— 唯一换算点纪律不变。
///
/// 为什么是独立 System 而不是并进 `SceneRecordSystem`：既有 System 的
/// `Reads` / `Writes` 是冻结契约（§6），不能给它新增写资源。
///
/// ⚠️ **内联 hecs 查询**：本 System 直接 `world.query::<(&MorphShape, &StrokeSolid)>()`，
/// **替换场景实现时需整体重写**。
pub struct MorphRecordSystem;
impl System for MorphRecordSystem {
    type Reads = (ViewRect, AppCamera);
    type Writes = (MorphQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let camera = cam.to_camera_2d(rect);

        ctx.with_world_and_resource_mut::<MorphQueue, _>(|world, queue| {
            queue.0.clear();
            for (morph, stroke) in world.query::<(&MorphShape, &StrokeSolid)>().iter() {
                // 世界 → 屏幕的二端几何映射 + 描边宽度换算都在 `push_morph_item`
                // （xcal-render 内，唯一换算点不外泄到 UI 层）。
                push_morph_item(
                    queue,
                    &morph.from,
                    &morph.to,
                    morph.t,
                    morph.samples,
                    stroke,
                    &camera,
                );
            }
        });
    }
}

// ─── ContentTextureRecordSystem ────────────────────────────────────────────

/// 内容纹理层录制（RenderCommit 阶段，S3-3）。
///
/// 只录**带 [`ContentTextureItem`] 标记**的路径描边实体到
/// [`ContentTextureQueue`]（网格线 / 坐标轴等未标记实体不进贴图，见计划 D5）。
///
/// 位置与描边宽度经 [`record_path_stroke`] 按 `spec.camera` 换算——与主 2D
/// 层同一口径（唯一换算点在 xcal-render 内）。
///
/// ⚠️ **内联 hecs 查询**：本 System 直接
/// `world.query::<(&Path, &StrokeSolid, &ContentTextureItem)>()` —— 标记过滤与组件组合
/// 都写在 System 里。**替换场景实现时本 System 需整体重写**（不能像
/// [`SceneRecordSystem`] 那样只换"取数据"的两步接线）。
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
            for (path, stroke, _) in world
                .query::<(&Path, &StrokeSolid, &ContentTextureItem)>()
                .iter()
            {
                record_path_stroke(&mut queue.0, path, stroke, &spec.camera);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_scene::domain::LayerId;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    fn zoom_event(factor: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Zoom { factor },
        }
    }

    fn pan_event(dx: f32, dy: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Pan {
                delta: (dx, dy).into(),
            },
        }
    }

    /// headless 决定性测试（B 阶段验收）：构造含 Zoom 命令的 InputEvents，
    /// 直接跑 CameraInputSystem，断言 AppCamera.scale 按预期变化。
    #[test]
    fn camera_input_applies_zoom_event_headless() {
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(InputEvents::from_events(vec![zoom_event(1.1)]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let scale = sched.resources().get::<AppCamera>().unwrap().scale;
        assert!((scale - 44.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 缩放 clamp：factor 过大时 scale 不超过上限 500。
    #[test]
    fn camera_input_zoom_is_clamped() {
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(InputEvents::from_events(vec![zoom_event(1000.0)]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let scale = sched.resources().get::<AppCamera>().unwrap().scale;
        assert!((scale - 500.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 平移：Pan 命令直接叠加到取景中心（世界坐标，Y-up）。
    #[test]
    fn camera_input_applies_drag_event_headless() {
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(InputEvents::from_events(vec![pan_event(2.0, -1.0)]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let cam = sched.resources().get::<AppCamera>().unwrap();
        assert!((cam.center.x - 2.0).abs() < 1e-5, "x = {}", cam.center.x);
        assert!((cam.center.y - (-1.0)).abs() < 1e-5, "y = {}", cam.center.y);
    }

    /// 无事件帧不改变相机状态。
    #[test]
    fn camera_input_noop_without_events() {
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(InputEvents::default());

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let cam = sched.resources().get::<AppCamera>().unwrap();
        assert_eq!(cam.center.x, 0.0);
        assert_eq!(cam.scale, 40.0);
    }

    /// 控制器纯函数：Orbit/Dolly 在 PanZoom 模式下被忽略，状态不变。
    #[test]
    fn panzoom_controller_ignores_orbit_and_dolly() {
        let mut cam = AppCamera::default();
        cam.apply_command(CameraCommand::Orbit {
            delta_yaw: 1.0,
            delta_pitch: 0.5,
        });
        cam.apply_command(CameraCommand::Dolly { delta: 3.0 });
        assert_eq!(cam.center.x, 0.0);
        assert_eq!(cam.center.y, 0.0);
        assert_eq!(cam.scale, 40.0);
    }

    // ─── AxesRefreshSystem（M4-4：原网格构建 System 退役）──────────────────

    use xcal_scene::contract::entity_of;
    use xc_object::{AxesLine, Circle, ZOrder};

    fn count_axes_lines(world: &hecs::World) -> usize {
        world.query::<&AxesLine>().iter().count()
    }

    /// M4-4：网格几何由对象层 `Axes2D` 生成，System 只接线 ——
    /// 一帧之后场景里出现线段对象，且每帧"清旧建新"（不累积）。
    #[test]
    fn axes_refresh_rebuilds_grid_each_frame() {
        let mut world = hecs::World::new();
        let axes = Axes2D::new((-10.0, 10.0), (-10.0, 10.0));
        axes.build(&mut WorldScene::new(&mut world))
            .expect("Axes2D 已登记");

        let mut resources = Resources::new();
        resources.insert(ViewRect::default()); // 800 x 600
        resources.insert(AppCamera::default()); // scale = 40
        resources.insert(UiConfig::default()); // show_grid = true

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        // 视口 800x600、scale 40 ⇒ 可见 x ∈ [-10,10]（21 条刻度）、
        // y ∈ [-7.5,7.5]（16 条刻度，自 -8 起）⇒ 37 条网格线 + 2 条坐标轴 = 39。
        assert_eq!(count_axes_lines(sched.world()), 39);

        // 再来一帧：旧线段被清掉，数量不累积。
        sched.run_frame(0.016);
        assert_eq!(count_axes_lines(sched.world()), 39);

        // 网格线上的 ZOrder 必须压在内容之下（-100 网格 / -90 坐标轴）。
        let zs: Vec<i32> = sched.world().query::<&ZOrder>().iter().map(|z| z.0).collect();
        assert!(zs.iter().all(|z| *z < 0), "网格不得落在内容层：{zs:?}");
    }

    /// M4-4：`show_grid` 关闭 ⇒ 线段清空（容器对象保留，开回来能重建）。
    #[test]
    fn axes_refresh_respects_show_grid_toggle() {
        let mut world = hecs::World::new();
        let axes = Axes2D::unbounded();
        axes.build(&mut WorldScene::new(&mut world)).unwrap();

        let mut resources = Resources::new();
        resources.insert(ViewRect::default());
        resources.insert(AppCamera::default());
        resources.insert(UiConfig {
            show_grid: false,
            ..UiConfig::default()
        });

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(count_axes_lines(sched.world()), 0, "关闭网格后不应有线段");

        sched.resources_mut().get_mut::<UiConfig>().unwrap().show_grid = true;
        sched.run_frame(0.016);
        assert!(count_axes_lines(sched.world()) > 0, "重新打开应重建网格");
    }

    // ─── FunctionGraphRefreshSystem（M4-1）───────────────────────────────

    /// M4-1：函数采样归对象层；System 只把实体上的相位（`PhaseAnim`，由
    /// `BindingApplySystem` 写入）交给 `FunctionGraph::refresh` —— 走契约层
    /// `Scene::set` 的写路径（M4-3b）。
    #[test]
    fn function_graph_refresh_samples_with_entity_phase() {
        let mut world = hecs::World::new();
        let graph = FunctionGraph::new(
            xc_notion::FunctionX2Y::new(|x: f32| x),
            xc_notion::Sampler1D::new(0.0f32, 1.0, 1.0),
        );
        let handle = graph
            .build(
                &mut WorldScene::new(&mut world),
                StrokeSolid::new(1.0, (0, 200, 200, 255)),
            )
            .expect("FunctionGraph 已登记");

        // 相位组件（运行时由 BindingApplySystem 写入）。
        let mut anim = PhaseAnim::new(0.0, 1.5);
        anim.phase = 1.0;
        world.insert_one(entity_of(handle).unwrap(), anim).unwrap();

        let mut sched = Scheduler::new(world, Resources::new());
        sched.add_system(Phase::Prepare, FunctionGraphRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let path = sched.world().get::<&Path>(entity_of(handle).unwrap()).unwrap();
        let mut pts = Vec::new();
        for cmd in &path.0.commands {
            if let xc_notion::PathCommand::MoveTo(p) | xc_notion::PathCommand::LineTo(p) = cmd {
                pts.push(*p);
            }
        }
        // 相位加在自变量上：x = {0,1} ⇒ y = {1,2}。
        assert_eq!(
            pts,
            vec![
                xc_notion::Vec2::new(0.0, 1.0),
                xc_notion::Vec2::new(1.0, 2.0)
            ]
        );
    }

    // ─── ContentTextureRecordSystem（S3-3） ────────────────────────────────

    use xcal_render::Shape2D;
    use xcal_scene::scene::components::TextureHandle;
    use xc_notion::{Path2D, Rect2D};
    use xc_object::Camera2D;
    use xc_notion::PathCommand;

    const TEX_W: u32 = 512;
    const TEX_H: u32 = 512;

    /// 内容相机：`rect` 即纹理矩形（0,0–512,512），世界原点居中。
    fn content_camera() -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            25.0,
            Rect2D::new(xc_notion::Vec2::ZERO, xc_notion::Vec2::new(TEX_W as f32, TEX_H as f32)),
        )
    }

    fn content_spec(camera: Camera2D) -> ContentTextureSpec {
        ContentTextureSpec {
            camera,
            size_px: [TEX_W, TEX_H],
            texture: TextureHandle::new(1),
            composite: false,
        }
    }

    /// 从队列里取出全部路径点（屏幕/纹理空间）。
    fn recorded_points(queue: &ContentTextureQueue) -> Vec<glam::Vec2> {
        let mut pts = Vec::new();
        for cmd in queue.0.commands() {
            if let Shape2D::Path(path) = &cmd.shape {
                for c in &path.commands {
                    match c {
                        PathCommand::MoveTo(p) | PathCommand::LineTo(p) => {
                            pts.push(glam::Vec2::new(p.x, p.y));
                        }
                        _ => {}
                    }
                }
            }
        }
        pts
    }

    /// 只录带 `ContentTextureItem` 标记的实体：未标记的干扰 `Circle` 不应进队列。
    ///
    /// 证伪：去掉 `ContentTextureItem` 过滤（录全部路径/圆）→ 长度变 2 而非 1。
    #[test]
    fn content_record_filters_unmarked() {
        let mut world = hecs::World::new();
        let path = Path::new(Path2D::from_polyline(
            &[glam::Vec2::new(-0.5, -0.5), glam::Vec2::new(0.5, 0.5)],
            false,
        ));
        world.spawn((
            path,
            StrokeSolid::new(0.02, (255, 255, 255, 255)),
            ContentTextureItem,
        ));
        // 干扰项：带描边但**无标记**的圆。
        world.spawn((
            Circle::new((0.0, 0.0), 0.2),
            StrokeSolid::new(0.02, (255, 0, 0, 255)),
        ));

        let mut resources = Resources::new();
        resources.insert(content_spec(content_camera()));
        resources.insert(ContentTextureQueue::default());

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let queue = sched.resources().get::<ContentTextureQueue>().unwrap();
        assert_eq!(queue.0.len(), 1, "干扰项未被录制，队列应恰有 1 条命令");
    }

    /// 录制的点必须落在纹理矩形内（`0 <= x <= w`, `0 <= y <= h`）。
    ///
    /// 证伪：把 `record_path_stroke(..., &spec.camera)` 换成主相机/错位相机后，
    /// 点会落到矩形外（见本测试尾部的反向断言）。
    #[test]
    fn content_record_maps_into_texture_rect() {
        let mut world = hecs::World::new();
        let path = Path::new(Path2D::from_polyline(
            &[
                glam::Vec2::new(-0.5, -0.5),
                glam::Vec2::new(0.0, 0.0),
                glam::Vec2::new(0.5, 0.5),
            ],
            false,
        ));
        world.spawn((
            path,
            StrokeSolid::new(0.02, (255, 255, 255, 255)),
            ContentTextureItem,
        ));

        let mut resources = Resources::new();
        resources.insert(content_spec(content_camera()));
        resources.insert(ContentTextureQueue::default());

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let queue = sched.resources().get::<ContentTextureQueue>().unwrap();
        let pts = recorded_points(queue);
        assert!(!pts.is_empty(), "应至少录到一个路径点");
        for p in &pts {
            assert!(
                p.x >= 0.0 && p.x <= TEX_W as f32 && p.y >= 0.0 && p.y <= TEX_H as f32,
                "点 {p:?} 落在纹理矩形 [0,{TEX_W}]x[0,{TEX_H}] 之外"
            );
        }

        // 证伪性：同样的世界点、换成错位相机（中心移到 (100,100)）后
        // 映射结果必然出界 —— 证明上面的“落在矩形内”断言确实有区分度。
        let wrong = Camera2D::new(
            (100.0, 100.0).into(),
            25.0,
            Rect2D::new(xc_notion::Vec2::ZERO, xc_notion::Vec2::new(TEX_W as f32, TEX_H as f32)),
        );
        let mut wrong_queue = ContentTextureQueue::default();
        for (path, stroke, _) in sched
            .world()
            .query::<(&Path, &StrokeSolid, &ContentTextureItem)>()
            .iter()
        {
            record_path_stroke(&mut wrong_queue.0, path, stroke, &wrong);
        }
        let wrong_pts = recorded_points(&wrong_queue);
        assert!(
            wrong_pts.iter().any(|p| p.x < 0.0
                || p.x > TEX_W as f32
                || p.y < 0.0
                || p.y > TEX_H as f32),
            "错位相机应把点映射到矩形外（否则本测试无法证伪）"
        );
    }
}
