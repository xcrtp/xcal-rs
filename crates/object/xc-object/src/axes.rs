// axes.rs — 坐标轴 / 网格场景对象（M4-2）。
//
// 归位依据（M4 计划 §4.1）：网格与坐标轴的**几何**（相机裁剪、步长自适应、
// 刻度线生成）原本在 UI 层的网格构建 System 里（279 行四段独立逻辑）。
// 它是纯几何 + 纯数据，与 egui 无关 ⇒ 归场景对象层。
//
// 形态（数据纯、行为不纯）：
// - **数据部分**纯：`x_range` / `y_range` / `tick` 可序列化、可测试、可被脚本声明；
// - **语义方法**纯：`step` / `x_ticks` / `y_ticks` / `grid_lines` / `axis_lines`
//   只吃相机参数，不碰场景；
// - **装配行为**不纯（[`Axes2D::build`] / [`Axes2D::refresh`]）：依赖契约层
//   [`Scene`]，把线段作为对象放进场景（每帧"清旧建新"，与迁移前全量重建一致）。
//
// 与迁移前的逐位一致性（不得凭记忆改写）：
// - 裁剪：`half = viewport_px / 2 / scale`，绕 `camera.center` 取可见世界范围；
// - 步长：`unit = scale.max(min_px) / scale`（屏幕间距不小于 `min_px` 像素）；
// - 线段：竖线 `(x, y0)-(x, y1)`、横线 `(x0, y)-(x1, y)`，网格含 `x=0` / `y=0`；
// - 坐标轴：`(x0,0)-(x1,0)` 与 `(0,y0)-(0,y1)`；
// - 层级：网格 z = -100、坐标轴 z = -90（见 [`GRID_Z`] / [`AXIS_Z`]）；
// - 样式：屏幕空间（缩放时粗细恒定）。

use xc_notion::{MeasureSpace, Vec2};
use xc_render_ir::StrokeSolid;
use xc_scene_contract::{ObjectId, Scene};

use crate::camera::Camera2D;
use crate::primitives::Line;
use crate::zorder::ZOrder;

/// 网格线层级（在内容之下）。
pub const GRID_Z: i32 = -100;
/// 坐标轴层级（在网格之上、内容之下）。
pub const AXIS_Z: i32 = -90;

/// 刻度规格：刻度间距的**屏幕空间下限**（像素）。
///
/// 「步长自适应」的语义方法在这里（纯函数）：世界单位步长 = `scale.max(min_px) / scale`
/// —— 即屏幕间距不小于 `min_px` 像素，缩放时自动切换量级。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickSpec {
    /// 屏幕空间最小刻度间距（像素）。
    pub min_px: f32,
}

impl TickSpec {
    pub const fn new(min_px: f32) -> Self {
        Self { min_px }
    }

    /// 世界单位步长（`scale` = 每世界单位的像素数）。
    pub fn step_for(&self, scale: f32) -> f32 {
        if scale <= 0.0 || !scale.is_finite() {
            return self.min_px;
        }
        scale.max(self.min_px) / scale
    }
}

impl Default for TickSpec {
    fn default() -> Self {
        Self::new(10.0)
    }
}

/// 网格线段标记：该线段由哪个 [`Axes2D`] 对象生成。
///
/// `refresh` 靠它"清旧建新"（只清自己的线段，不误伤别的坐标系）；
/// 也因此**不需要**在容器上维护 `Vec<ObjectId>` 这类镜像状态
/// （镜像状态会与场景事实脱钩，是"两处真相"）。
pub struct AxesLine {
    /// 生成它的 [`Axes2D`] 容器对象。
    pub owner: ObjectId,
}

/// 轴对齐的坐标网格对象（网格线 + 坐标轴）。
#[derive(Debug, Clone)]
pub struct Axes2D {
    /// x 方向定义域（世界单位）；`refresh` 会与相机可见范围取交。
    pub x_range: (f32, f32),
    /// y 方向定义域（世界单位）。
    pub y_range: (f32, f32),
    /// 刻度规格。
    pub tick: TickSpec,
    /// 网格线样式。
    pub grid_stroke: StrokeSolid,
    /// 坐标轴样式。
    pub axis_stroke: StrokeSolid,
}

impl Default for Axes2D {
    /// 无界定义域 + 迁移前网格构建 System 的样式（灰网格 / 白轴线，屏幕空间）。
    fn default() -> Self {
        Self {
            x_range: (f32::NEG_INFINITY, f32::INFINITY),
            y_range: (f32::NEG_INFINITY, f32::INFINITY),
            tick: TickSpec::default(),
            grid_stroke: StrokeSolid::new_in(1.0, (50, 50, 50, 255), MeasureSpace::Screen),
            axis_stroke: StrokeSolid::new_in(2.0, (255, 255, 255, 255), MeasureSpace::Screen),
        }
    }
}

impl Axes2D {
    /// 以定义域构造（样式 / 刻度取默认）。
    pub fn new(x_range: (f32, f32), y_range: (f32, f32)) -> Self {
        Self {
            x_range,
            y_range,
            ..Default::default()
        }
    }

    /// 无界定义域（网格铺满可见范围，与迁移前行为一致）。
    pub fn unbounded() -> Self {
        Self::default()
    }

    // ─── 语义方法（纯：只吃相机，可单测）───────────────────────────────

    /// 世界原点。
    pub fn origin(&self) -> Vec2 {
        Vec2::ZERO
    }

    /// 当前可见的 x 范围（相机裁剪 ∘ 定义域）。
    pub fn visible_x(&self, camera: &Camera2D) -> (f32, f32) {
        let half = camera.rect.size().x * 0.5 / camera.scale;
        intersect((camera.center.x - half, camera.center.x + half), self.x_range)
    }

    /// 当前可见的 y 范围（相机裁剪 ∘ 定义域）。
    pub fn visible_y(&self, camera: &Camera2D) -> (f32, f32) {
        let half = camera.rect.size().y * 0.5 / camera.scale;
        intersect((camera.center.y - half, camera.center.y + half), self.y_range)
    }

    /// 当前世界单位步长（自适应）。
    pub fn step(&self, camera: &Camera2D) -> f32 {
        self.tick.step_for(camera.scale)
    }

    /// x 方向刻度位置。
    pub fn x_ticks(&self, camera: &Camera2D) -> Vec<f32> {
        ticks_in(self.visible_x(camera), self.step(camera))
    }

    /// y 方向刻度位置。
    pub fn y_ticks(&self, camera: &Camera2D) -> Vec<f32> {
        ticks_in(self.visible_y(camera), self.step(camera))
    }

    /// 网格线段（不含坐标轴；含 `x=0` / `y=0` 两条，与迁移前一致）。
    pub fn grid_lines(&self, camera: &Camera2D) -> Vec<Line> {
        let (x0, x1) = self.visible_x(camera);
        let (y0, y1) = self.visible_y(camera);
        let mut out = Vec::new();
        for x in self.x_ticks(camera) {
            out.push(Line::new((x, y0), (x, y1)));
        }
        for y in self.y_ticks(camera) {
            out.push(Line::new((x0, y), (x1, y)));
        }
        out
    }

    /// 坐标轴线段（x 轴 + y 轴）。
    pub fn axis_lines(&self, camera: &Camera2D) -> Vec<Line> {
        let (x0, x1) = self.visible_x(camera);
        let (y0, y1) = self.visible_y(camera);
        vec![Line::new((x0, 0.0), (x1, 0.0)), Line::new((0.0, y0), (0.0, y1))]
    }

    // ─── 装配（不纯：依赖契约层 Scene）────────────────────────────────

    /// 创建坐标系容器对象（返回其句柄；几何在 [`Axes2D::refresh`] 里按相机生成）。
    ///
    /// 为什么消耗 `&self` 而非 `self`：同一规格可装配多次（两个视口各一套坐标轴）。
    pub fn build<S: Scene>(&self, scene: &mut S) -> Option<ObjectId> {
        scene.spawn_dyn(vec![Box::new(self.clone())])
    }

    /// 按相机刷新几何：**清掉旧的再建新的**（线段数量随缩放变化，复用同一组不可行）。
    ///
    /// # 性能特征（记账，非缺陷）
    ///
    /// 当前实现每帧 despawn + spawn ≈39 个实体（默认视口 800×600、scale 40 的实测值：
    /// x / y 各 21 / 16 条刻度 + 2 条轴线）。这与迁移前的即时模式（直写 `DrawQueue`、
    /// 零实体开销）相比是 **Scene 化的真实代价**：F1 overlay 的实体计数会随之上升，
    /// 每帧有约 39 次 spawn/despawn。视觉无变化。
    ///
    /// 按本项目的「图元下沉判据」（实体数爆炸 + 每帧重算），`Axes2D` 是将来**第一个
    /// 该下沉成单一图元**的候选（例如一个"网格图元"承载全部线段，由后端一次性展开）。
    /// **下沉触发条件**：做真实动画时若网格刷新成为性能瓶颈 —— 而不是"看起来不够优雅"。
    pub fn refresh<S: Scene>(&self, scene: &mut S, handle: ObjectId, camera: &Camera2D) {
        self.clear(scene, handle);
        for line in self.grid_lines(camera) {
            self.spawn_line(scene, handle, line, &self.grid_stroke, GRID_Z);
        }
        for line in self.axis_lines(camera) {
            self.spawn_line(scene, handle, line, &self.axis_stroke, AXIS_Z);
        }
    }

    /// 清掉本对象生成的全部线段（`show_grid` 关闭时调用）。
    pub fn clear<S: Scene>(&self, scene: &mut S, handle: ObjectId) {
        let mut victims = Vec::new();
        scene.for_each::<AxesLine>(|id, marker| {
            if marker.owner == handle {
                victims.push(id);
            }
        });
        for id in victims {
            scene.despawn(id);
        }
    }

    fn spawn_line<S: Scene>(
        &self,
        scene: &mut S,
        owner: ObjectId,
        line: Line,
        stroke: &StrokeSolid,
        z: i32,
    ) {
        // 线段 = 形状 + 样式 + 层级 + 归属标记（四个组件，登记表见 xcal-scene::contract）。
        scene.spawn_dyn(vec![
            Box::new(line),
            Box::new(stroke.clone()),
            Box::new(ZOrder::new(z)),
            Box::new(AxesLine { owner }),
        ]);
    }
}

/// 两个区间的交（定义域 ∩ 可见范围）。
fn intersect(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0.max(b.0), a.1.min(b.1))
}

/// 区间内按 `step` 均匀取刻度（含左端点对齐到 `step` 的整数倍）。
///
/// 迁移前语义：`x = (x0 / unit).floor() * unit` 起，`x <= x1` 止。
fn ticks_in((a, b): (f32, f32), step: f32) -> Vec<f32> {
    let mut out = Vec::new();
    if !(step > 0.0) || !step.is_finite() || !a.is_finite() || !b.is_finite() || b < a {
        return out;
    }
    // 步数上限：区间有限 ⇒ `(b - a) / step` 有限。上限兜底防浮点漂移导致的死循环。
    let max_n = ((b - a) / step).ceil() as usize + 2;
    let mut x = (a / step).floor() * step;
    for _ in 0..max_n {
        if x > b {
            break;
        }
        out.push(x);
        x += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::Rect2D;

    fn camera(scale: f32, size: Vec2) -> Camera2D {
        Camera2D::new(
            Vec2::ZERO,
            scale,
            Rect2D::new(Vec2::ZERO, size),
        )
    }

    /// 步长自适应（迁移前 `unit = scale.max(10) / scale` 的语义）。
    #[test]
    fn step_adapts_to_scale() {
        let axes = Axes2D::unbounded();
        assert_eq!(axes.step(&camera(40.0, Vec2::new(800.0, 600.0))), 1.0);
        assert_eq!(axes.step(&camera(4.0, Vec2::new(800.0, 600.0))), 2.5);
        // 等价形式：世界步长 = min_px / scale（scale 越小、世界步长越大）。
        assert_eq!(axes.step(&camera(0.5, Vec2::new(800.0, 600.0))), 20.0);
    }

    /// 相机裁剪：可见范围 = 视口像素 / 2 / scale，绕相机中心。
    #[test]
    fn visible_range_follows_camera() {
        let axes = Axes2D::unbounded();
        let cam = camera(40.0, Vec2::new(800.0, 600.0));
        assert_eq!(axes.visible_x(&cam), (-10.0, 10.0));
        assert_eq!(axes.visible_y(&cam), (-7.5, 7.5));
    }

    /// 定义域与可见范围取交（有限定义域不得被相机撑破）。
    #[test]
    fn finite_domain_clamps_visible_range() {
        let axes = Axes2D::new((-1.0, 1.0), (-2.0, 2.0));
        let cam = camera(40.0, Vec2::new(800.0, 600.0));
        assert_eq!(axes.visible_x(&cam), (-1.0, 1.0));
        assert_eq!(axes.visible_y(&cam), (-2.0, 2.0));
    }

    /// 刻度：含 `x = 0`，且全部落在可见范围内。
    #[test]
    fn ticks_cover_visible_range() {
        let axes = Axes2D::unbounded();
        let cam = camera(40.0, Vec2::new(800.0, 600.0));
        let xs = axes.x_ticks(&cam);
        assert!(xs.contains(&0.0), "刻度必须含原点");
        assert!(xs.iter().all(|x| *x >= -10.0 && *x <= 10.0));
        assert_eq!(xs.len(), 21, "x ∈ [-10,10]、步长 1 ⇒ 21 条刻度");
    }

    /// 网格线段：竖线数与横线数等于两方向刻度数（迁移前逐位一致）。
    #[test]
    fn grid_lines_match_tick_counts() {
        let axes = Axes2D::unbounded();
        let cam = camera(40.0, Vec2::new(800.0, 600.0));
        let lines = axes.grid_lines(&cam);
        assert_eq!(lines.len(), axes.x_ticks(&cam).len() + axes.y_ticks(&cam).len());
        // 第一根竖线：端点落在可见 y 范围两端。
        assert_eq!(lines[0].a(), Vec2::new(-10.0, -7.5));
        assert_eq!(lines[0].b(), Vec2::new(-10.0, 7.5));
    }

    /// 坐标轴：恰好两条（x 轴 / y 轴）。
    #[test]
    fn axis_lines_are_two() {
        let axes = Axes2D::unbounded();
        let lines = axes.axis_lines(&camera(40.0, Vec2::new(800.0, 600.0)));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].a(), Vec2::new(-10.0, 0.0));
        assert_eq!(lines[1].b(), Vec2::new(0.0, 7.5));
    }
}
