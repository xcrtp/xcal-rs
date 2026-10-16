// graph.rs — 函数图像场景对象（M4-1）。
//
// 归位依据（M4 计划 §4.1）：函数采样循环原在 UI 层的 `SceneRecordSystem` 里
// （`(FunctionX2Y, Sampler1D, StrokeSolid, PhaseAnim)` → 逐点 `f(x + phase)` 折线）。
// 采样是**纯几何**，与 egui 无关 ⇒ 归场景对象层。
//
// 形态（数据纯、行为不纯）：
// - **数据部分**纯：`func`（理念层 `FunctionX2Y`）+ `sampler`（理念层 `Sampler1D` 区间 / 步长）；
// - **语义方法**纯：`sample_points` / `sample_path`；
// - **装配行为**不纯（[`FunctionGraph::build`] / [`FunctionGraph::refresh`]）：依赖契约层
//   [`Scene`]，把采样结果写成对象的 `Path` 组件。
//
// 与迁移前的逐位一致性：
// - 采样点 `(x, f(x + phase))`，`x` 自 `sampler.start` 起、步长 `sampler.step`、
//   止于 `x <= sampler.end`；
// - 折线**不闭合**；位置为**世界空间**（相机换算仍只发生在渲染录制的唯一换算点）；
// - 层级 z = 0（与内容同层）。
//
// 为什么 `refresh` 不取相机：采样在世界空间完成，相机换算发生在渲染录制
// （`Path::to_screen`），故对象层不需要相机 —— 唯一换算点纪律的必然结果。
// 计划给的签名是 `refresh(scene, handle, camera)`，本实现按事实去掉 `camera`
// 并补上**相位**（函数图像逐帧变化的自变量，来自实体的 `PhaseAnim`）。

use xc_notion::{FunctionX2Y, Path2D, Sampler1D, Vec2};
use xc_render_ir::StrokeSolid;
use xc_scene_contract::{ObjectId, Scene};

use crate::primitives::Path;
use crate::zorder::ZOrder;

/// 函数图像内容层级（与其它内容同层）。
pub const CONTENT_Z: i32 = 0;

/// 一元函数图像对象：`y = f(x + phase)` 在 `sampler` 区间上的折线。
#[derive(Clone)]
pub struct FunctionGraph {
    /// 被绘制的函数（理念层函数元语）。
    pub func: FunctionX2Y<f32, f32>,
    /// 采样规格（区间 + 步长）。
    pub sampler: Sampler1D<f32>,
}

impl FunctionGraph {
    pub fn new(func: FunctionX2Y<f32, f32>, sampler: Sampler1D<f32>) -> Self {
        Self { func, sampler }
    }

    // ─── 语义方法（纯）────────────────────────────────────────────────

    /// 按相位采样出折线顶点（世界空间）。
    ///
    /// 相位偏移直接加在**自变量**上（`f(x + phase)`），这是迁移前
    /// `SceneRecordSystem` 的口径 —— 视觉上是曲线随时间水平滑动。
    pub fn sample_points(&self, phase: f32) -> Vec<Vec2> {
        let (start, end, step) = (self.sampler.start, self.sampler.end, self.sampler.step);
        let mut pts = Vec::new();
        if !(step > 0.0) || !step.is_finite() || !start.is_finite() || !end.is_finite() || end < start
        {
            return pts;
        }
        // 步数上限：区间有限 ⇒ `(end - start) / step` 有限。上限兜底防浮点漂移死循环。
        let max_n = ((end - start) / step).ceil() as usize + 2;
        let mut x = start;
        for _ in 0..max_n {
            if x > end {
                break;
            }
            pts.push(Vec2::new(x, (self.func.f)(x + phase)));
            x += step;
        }
        pts
    }

    /// 采样为**不闭合**折线。
    pub fn sample_path(&self, phase: f32) -> Path2D {
        Path2D::from_polyline(&self.sample_points(phase), false)
    }

    // ─── 装配（不纯：依赖契约层 Scene）────────────────────────────────

    /// 创建函数图像对象（含初相位 0 的折线），返回句柄。
    ///
    /// 同一规格可装配多次（消耗 `&self`）。
    pub fn build<S: Scene>(&self, scene: &mut S, stroke: StrokeSolid) -> Option<ObjectId> {
        scene.spawn_dyn(vec![
            Box::new(self.clone()),
            Box::new(Path::new(self.sample_path(0.0))),
            Box::new(stroke),
            Box::new(ZOrder::new(CONTENT_Z)),
        ])
    }

    /// 按相位刷新折线（**写路径**：覆盖对象上既有的 `Path` 组件）。
    ///
    /// 采样点数与区间固定 ⇒ 不需要清旧建新（与 `Axes2D` 不同，那里线段数量随缩放变）。
    /// 返回 `false` = 句柄失效。
    pub fn refresh<S: Scene>(&self, scene: &mut S, handle: ObjectId, phase: f32) -> bool {
        scene.set(handle, Path::new(self.sample_path(phase)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(f: fn(f32) -> f32, start: f32, end: f32, step: f32) -> FunctionGraph {
        FunctionGraph::new(FunctionX2Y::new(f), Sampler1D::new(start, end, step))
    }

    /// 采样口径：`(x, f(x + phase))`，端点含 `end`（迁移前逐位一致）。
    #[test]
    fn sample_points_cover_closed_interval() {
        let g = graph(|x| 2.0 * x, -1.0, 1.0, 0.5);
        let pts = g.sample_points(0.0);
        assert_eq!(pts.len(), 5);
        assert_eq!(pts[0], Vec2::new(-1.0, -2.0));
        assert_eq!(pts[4], Vec2::new(1.0, 2.0));
    }

    /// 相位加在自变量上（水平滑动），不是加在函数值上。
    #[test]
    fn phase_shifts_along_x_axis() {
        let g = graph(|x| x, 0.0, 2.0, 1.0);
        let pts = g.sample_points(1.0);
        assert_eq!(pts[0], Vec2::new(0.0, 1.0));
        assert_eq!(pts[2], Vec2::new(2.0, 3.0));
    }

    /// 退化采样规格（非正步长 / 非有限区间）返回空，不死循环。
    #[test]
    fn degenerate_sampler_yields_nothing() {
        assert!(graph(|x| x, 0.0, 1.0, 0.0).sample_points(0.0).is_empty());
        assert!(
            graph(|x| x, 0.0, f32::INFINITY, 0.1)
                .sample_points(0.0)
                .is_empty()
        );
    }

    /// 折线不闭合。
    #[test]
    fn sample_path_is_open() {
        let g = graph(|x| x, 0.0, 1.0, 0.5);
        let path = g.sample_path(0.0);
        assert!(!matches!(
            path.commands.last(),
            Some(xc_notion::PathCommand::Close)
        ));
    }
}
