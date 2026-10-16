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
// - Prepare:      GridBuild    (R=ViewRect, AppCamera, UiConfig, W=DrawQueue)
// - RenderCommit: SceneRecord  (R=ViewRect, AppCamera, W=DrawQueue)
//
// 注：时间推进由 DirectorAdvanceSystem（xcal-scene）负责，动画由 BindingApplySystem
// （xcal-scene::animation）驱动。

use xcal_schedule::context::ExecutorContext;
use xcal_scene::domain::{CameraCommand, ContentTextureSpec, InputEvents, SceneEvent};
use xcal_schedule::system::System;
use xcal_scene::scene::components::ContentTextureItem;
use xcal_scene::transform::Transform2D;
use xcal_scene::types::{Camera2D, Rect};
use xcal_render::components::{
    FunctionX2Y, Line, Path, PhaseAnim, Sampler1D, StrokeSolid,
};
use xcal_render::{ContentTextureQueue, DrawQueue, MeasureSpace, ZIndex, record_path_stroke, record_scene};
use xc_geometry::Path2D;

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

// ─── GridBuildSystem ───────────────────────────────────────────────────────

pub struct GridBuildSystem;
impl GridBuildSystem {
    fn push_grid(queue: &mut DrawQueue, camera: &Camera2D, rect: Rect) {
        let center = camera.center;
        let scale = camera.scale;
        let half_w = rect.size().x / 2.0 / scale;
        let half_h = rect.size().y / 2.0 / scale;
        let x0 = center.x - half_w;
        let x1 = center.x + half_w;
        let y0 = center.y - half_h;
        let y1 = center.y + half_h;

        // 网格与坐标轴都是"观测工具"→ 屏幕空间：缩放时粗细恒定。
        let grid = StrokeSolid::new_in(1.0, (50, 50, 50, 255), MeasureSpace::Screen);
        let axis = StrokeSolid::new_in(2.0, (255, 255, 255, 255), MeasureSpace::Screen);
        let z_grid = ZIndex::new(-100);
        let z_axis = ZIndex::new(-90);
        let unit = scale.max(10.0) / scale;

        let mut x = (x0 / unit).floor() * unit;
        while x <= x1 {
            queue.line_stroke(
                z_grid,
                Line::new((x, y0), (x, y1)).to_screen(camera),
                grid.clone(),
            );
            x += unit;
        }

        let mut y = (y0 / unit).floor() * unit;
        while y <= y1 {
            queue.line_stroke(
                z_grid,
                Line::new((x0, y), (x1, y)).to_screen(camera),
                grid.clone(),
            );
            y += unit;
        }

        queue.line_stroke(
            z_axis,
            Line::new((x0, 0.0), (x1, 0.0)).to_screen(camera),
            axis.clone(),
        );
        queue.line_stroke(
            z_axis,
            Line::new((0.0, y0), (0.0, y1)).to_screen(camera),
            axis,
        );
    }
}

impl System for GridBuildSystem {
    type Reads = (ViewRect, AppCamera, UiConfig);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 三阶段闭包：先克隆只读值，再写回。
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let show_grid = ctx.get::<UiConfig>().map(|c| c.show_grid).unwrap_or(true);

        let camera = cam.to_camera_2d(rect);

        let Some(queue) = ctx.get_mut::<DrawQueue>() else {
            return;
        };

        // S3-2 §1.1/C2：全屏背景矩形**已删除**。2D 图层只含内容，
        // 背景改由合成器清屏（GPU 路径）或 host 回放前补画（CPU 路径）提供；
        // 否则 2D 层搬到 3D 层之上时会用不透明背景盖死 3D。
        if show_grid {
            Self::push_grid(queue, &camera, rect);
        }
    }
}

// ─── SceneRecordSystem ─────────────────────────────────────────────────────

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
            record_scene(world, &camera, queue);

            for (func, sampler, stroke, phase) in world
                .query::<(
                    &FunctionX2Y<f32, f32>,
                    &Sampler1D<f32>,
                    &StrokeSolid,
                    Option<&PhaseAnim>,
                )>()
                .iter()
            {
                // BindingApplySystem 写入的相位偏移：让 sin 曲线随时间水平滑动。
                let phase = phase.map(|p| p.phase).unwrap_or(0.0);
                let mut pts: Vec<glam::Vec2> = Vec::new();
                let mut x = sampler.start;
                while x <= sampler.end {
                    pts.push(glam::Vec2::new(x, (func.f)(x + phase)));
                    x += sampler.step;
                }
                queue.path_stroke(
                    ZIndex::new(0),
                    Path::new(Path2D::from_polyline(&pts, false)).to_screen(&camera),
                    stroke.clone(),
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

    /// C2 验收：`GridBuildSystem` 产出的队列**不含**全屏背景矩形。
    ///
    /// 背景已外移到合成器清屏（GPU）/ host 回放前（CPU）：2D 图层若还带着
    /// 不透明背景矩形，2D 叠到 3D 之上时会把它整片盖死。
    #[test]
    fn grid_build_emits_no_fullscreen_background_rect() {
        let mut resources = Resources::new();
        resources.insert(ViewRect::default());
        resources.insert(AppCamera::default());
        resources.insert(UiConfig::default());
        resources.insert(DrawQueue::new());

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Prepare, GridBuildSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let queue = sched.resources().get::<DrawQueue>().unwrap();
        assert!(!queue.is_empty(), "网格开关默认打开，应有网格线命令");
        for cmd in queue.commands() {
            assert_ne!(cmd.z.0, -1000, "背景矩形的 z 层已删除");
            assert!(
                !matches!(cmd.shape, xcal_render::Shape2D::Rect(_)),
                "2D 图层不应再输出任何矩形填充（背景矩形残留）"
            );
        }
    }

    // ─── ContentTextureRecordSystem（S3-3） ────────────────────────────────

    use xcal_render::Shape2D;
    use xcal_scene::scene::components::{Circle, TextureHandle};
    use xc_geometry::PathCommand;

    const TEX_W: u32 = 512;
    const TEX_H: u32 = 512;

    /// 内容相机：`rect` 即纹理矩形（0,0–512,512），世界原点居中。
    fn content_camera() -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            25.0,
            Rect::new((0.0, 0.0).into(), (TEX_W as f32, TEX_H as f32).into()),
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
            Rect::new((0.0, 0.0).into(), (TEX_W as f32, TEX_H as f32).into()),
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
