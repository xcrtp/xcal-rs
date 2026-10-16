// morph.rs — MorphApplySystem：把 `MorphShape{from,to,t}` 插值成可渲染的 `Path`。
//
// 阶段：SceneSync（必须在 BindingApply 之后——后者写 `MorphShape.t`）。
// 纯组件读写（`ctx.world`），不读写任何 Resource。
//
// ⚠️ **S4.3 起本 System 是 CPU 回退路径，App 不再注册它**：形变改由 GPU 插值
// （`MorphItem` 的 `from`/`to` 常量 + `t` 走 uniform，见
// `xcal-render-backends::wgpu_2d` 的形变管线与 `xcal_egui::systems::MorphRecordSystem`）。
// 保留它的意义：无需 GPU 的宿主（headless / 测试）仍可用它把形变落成 `Path`
// 再交给普通 2D 录制。用法：形变实体同时带 `MorphShape` + `Path` 时本 System 生效；
// App 的形变实体**不带 `Path`**，故本 System 在 App 里自然不命中。

use xc_notion::Path2D;
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xcal_scene::scene::components::MorphShape;
use xc_object::Path;

/// 形变应用（CPU 回退）：`Path2D::morph_points(from, to, t, samples)` → 写入同实体的
/// `Path.path`（闭合折线，`from_polyline(..., true)` 携带闭合语义）。render 层由
/// `record_scene` 消费 `(Path, StrokeSolid)`。
pub struct MorphApplySystem;

impl System for MorphApplySystem {
    type Reads = ();
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        for (morph, path) in ctx.world.query::<(&MorphShape, &mut Path)>().iter() {
            let samples = morph.samples.max(1) as usize;
            let pts = Path2D::morph_points(&morph.from, &morph.to, morph.t, samples);
            path.0 = Path2D::from_polyline(&pts, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::binding::{Binding, BindingTable, SeekKind};
    use crate::animation::system::BindingApplySystem;
    use crate::animation::target::MorphT;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use crate::TimeState;
    use xc_time::{ClockForest, ClockNode, Curve};

    fn square(half: f32) -> Path2D {
        Path2D::from_polyline(
            &[
                glam::Vec2::new(-half, -half),
                glam::Vec2::new(half, -half),
                glam::Vec2::new(half, half),
                glam::Vec2::new(-half, half),
            ],
            true,
        )
    }

    /// headless：Binding 驱动 `MorphShape.t`，MorphApplySystem 把插值结果写进 `Path`。
    #[test]
    fn binding_drives_t_and_morph_writes_path() {
        let mut world = hecs::World::new();
        let entity = world.spawn((
            MorphShape::new(square(1.0), square(2.0)),
            Path::new(Path2D::new()),
        ));

        let mut forest = ClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();

        let mut table = BindingTable::new();
        table.add(Binding {
            clock: root,
            // Phase：sample(t) = omega * t = 0.5 → 形变参数 0.5
            curve: Curve::Phase { omega: 1.0 },
            target: std::sync::Arc::new(MorphT {
                id: crate::contract::object_id_of(entity),
            }),
            seek_kind: SeekKind::O1,
        });

        let mut resources = Resources::new();
        resources.insert(TimeState { elapsed: 0.5 });
        resources.insert(forest);
        resources.insert(table);

        let mut scheduler = Scheduler::new(world, resources);
        // SceneSync 内 BindingApply 先写 t，MorphApply 再消费。
        scheduler.add_system(Phase::SceneSync, BindingApplySystem);
        scheduler.add_system(Phase::SceneSync, MorphApplySystem);
        scheduler.run_frame(0.0);

        let t = scheduler.world().get::<&MorphShape>(entity).unwrap().t;
        assert!((t - 0.5).abs() < 1e-5, "t 未被写入: {t}");

        let path = scheduler.world().get::<&Path>(entity).unwrap();
        // 128 个采样点 + Close 回到起点 = 129 个展平点。
        let pts = path.0.flatten(0.01);
        assert_eq!(pts.len(), 129, "顶点数应恒为 samples + 闭合回起点");
        assert!(
            matches!(
                path.0.commands.last(),
                Some(xc_notion::PathCommand::Close)
            ),
            "形变输出应闭合（末命令为 Close）"
        );
        let max_x = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let min_x = pts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!((max_x - 1.5).abs() < 1e-3, "max_x={max_x}");
        assert!((min_x + 1.5).abs() < 1e-3, "min_x={min_x}");
    }
}
