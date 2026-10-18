// systems.rs: 场景侧 System（UI 相关的**接线**部分）
//
// 每个 System 显式声明 `Reads` / `Writes` 关联类型，使用
// `ctx.get` / `ctx.get_mut` 访问 Resource。
//
// # 归位后本文件只剩两个"接线" System
//
// - `AxesRefreshSystem`（Prepare）：取本帧相机 → 逐个刷新场景里的坐标轴对象；
// - `FunctionGraphRefreshSystem`（Prepare）：取实体上的相位 → 重新采样函数图像。
//
// 已迁出（P4 / P1）：
// - `SceneRecord` / `MorphRecord` / `ContentTextureRecord` → `xcal-render::record_systems`
//   （渲染动作住渲染层；不再依赖 UI 定义的 `ViewRect` / `AppCamera`）；
// - `CameraInput` → `xcal_scene::camera_input`（相机状态归场景对象；
//   控制器纯函数 `apply_pan_zoom` 也搬过去了）。
//
// Phase 分配：
// - Prepare: AxesRefresh / FunctionGraphRefresh（几何在场景对象层，本层只接线）

use xc_object::{Axes2D, Camera2DHandle, FunctionGraph};
use xcal_render::ViewRect;
use xcal_scene::contract::{SceneRead, WorldScene, WorldView};
use xcal_scene::scene::components::PhaseAnim;
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

use crate::resources::UiConfig;

// ─── AxesRefreshSystem（M4-4：原网格构建 System 退役）───────────────────────

/// 坐标轴 / 网格刷新（Prepare 阶段）。
///
/// **M4-4：原网格构建 System（本层旧 `Grid` 构建器）已退役。** 原先 279 行的四段几何逻辑
/// （相机裁剪、步长自适应、刻度线、坐标轴）全部归场景对象层的
/// [`xc_object::Axes2D`]；本 System 只剩**接线**：取本帧相机 → 逐个刷新场景里的
/// 坐标轴对象。本文件因此**不再有任何几何计算**，也不再把相机缩放带进任何
/// 宽度换算（换算只发生在 `xcal-render` 的唯一换算点）。
///
/// P1 后相机来自**场景对象**（经 `Camera2DHandle` 指认 + `ViewRect` 绑定视口），
/// 不再是 UI 持有的一份状态。
///
/// 语义与迁移前一致：`UiConfig::show_grid` 关闭时清空线段（容器对象保留），
/// 打开时按相机重建（线段数量随缩放变化，故必须"清旧建新"）。
pub struct AxesRefreshSystem;
impl System for AxesRefreshSystem {
    type Reads = (ViewRect, Camera2DHandle, UiConfig);
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先取只读资源值（释放对 resources 的共享借用）。
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let handle = ctx.get::<Camera2DHandle>().copied().unwrap_or_default();
        let show_grid = ctx.get::<UiConfig>().map(|c| c.show_grid).unwrap_or(true);
        // 只读世界解析相机（借用收在块内），再取世界可变借用 —— 一读一写、不重叠。
        let camera = {
            let world = ctx.borrow_world();
            handle.resolve(&WorldView::new(world), rect)
        };
        let Some(camera) = camera else {
            return;
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::{AxesLine, Framing2D, ZOrder};
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    fn count_axes_lines(world: &hecs::World) -> usize {
        world.query::<&AxesLine>().iter().count()
    }

    /// M4-4：网格几何由对象层 `Axes2D` 生成，System 只接线 ——
    /// 一帧之后场景里出现线段对象，且每帧"清旧建新"（不累积）。
    ///
    /// P1：相机来自场景对象（`Camera2DHandle` + `Framing2D`），视口矩形来自 `ViewRect`。
    #[test]
    fn axes_refresh_rebuilds_grid_each_frame() {
        let mut world = hecs::World::new();
        let axes = Axes2D::new((-10.0, 10.0), (-10.0, 10.0));
        axes.build(&mut WorldScene::new(&mut world))
            .expect("Axes2D 已登记");
        let handle = {
            let mut ws = WorldScene::new(&mut world);
            xc_object::spawn_camera2d(&mut ws, Framing2D::new((0.0, 0.0).into(), 40.0)).unwrap()
        };

        let mut resources = Resources::new();
        resources.insert(ViewRect::default()); // 800 x 600
        resources.insert(handle);
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
        let zs: Vec<i32> = sched
            .world()
            .query::<&ZOrder>()
            .iter()
            .map(|z| z.0)
            .collect();
        assert!(zs.iter().all(|z| *z < 0), "网格不得落在内容层：{zs:?}");
    }

    /// M4-4：`show_grid` 关闭 ⇒ 线段清空（容器对象保留，开回来能重建）。
    #[test]
    fn axes_refresh_respects_show_grid_toggle() {
        let mut world = hecs::World::new();
        let axes = Axes2D::unbounded();
        axes.build(&mut WorldScene::new(&mut world)).unwrap();
        let handle = {
            let mut ws = WorldScene::new(&mut world);
            xc_object::spawn_camera2d(&mut ws, Framing2D::default()).unwrap()
        };

        let mut resources = Resources::new();
        resources.insert(ViewRect::default());
        resources.insert(handle);
        resources.insert(UiConfig {
            show_grid: false,
            ..UiConfig::default()
        });

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(count_axes_lines(sched.world()), 0, "关闭网格后不应有线段");

        sched
            .resources_mut()
            .get_mut::<UiConfig>()
            .unwrap()
            .show_grid = true;
        sched.run_frame(0.016);
        assert!(count_axes_lines(sched.world()) > 0, "重新打开应重建网格");
    }

    /// 句柄失效（场景里没有相机对象）⇒ 跳过（不 panic、不建线段）。
    #[test]
    fn axes_refresh_skips_without_camera() {
        let mut world = hecs::World::new();
        Axes2D::unbounded()
            .build(&mut WorldScene::new(&mut world))
            .unwrap();

        let mut resources = Resources::new();
        resources.insert(ViewRect::default());
        resources.insert(Camera2DHandle::default());
        resources.insert(UiConfig::default());

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(count_axes_lines(sched.world()), 0);
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
                xcal_render::StrokeSolid::new(1.0, (0, 200, 200, 255)),
            )
            .expect("FunctionGraph 已登记");

        // 相位组件（运行时由 BindingApplySystem 写入）。
        let mut anim = PhaseAnim::new(0.0, 1.5);
        anim.phase = 1.0;
        world
            .insert_one(xcal_scene::contract::entity_of(handle).unwrap(), anim)
            .unwrap();

        let mut sched = Scheduler::new(world, Resources::new());
        sched.add_system(Phase::Prepare, FunctionGraphRefreshSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let path = sched
            .world()
            .get::<&xc_object::Path>(xcal_scene::contract::entity_of(handle).unwrap())
            .unwrap();
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
}
