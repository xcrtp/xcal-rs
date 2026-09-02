// render.rs

use crate::{
    scene::components::{Circle, FillSolid, Line, Path, StrokeSolid},
    types::{Camera2D, Rect, Vec2},
};

/// 2D 绘制器 Trait。
///
/// 实现者只需实现 [`paint`](Self::paint) 一个方法：根据 ([`Shape`], [`Style`])
/// 组合进行绘制。所有传入的 [`Shape`] 已是 **屏幕坐标系** 下的坐标，由调用方
/// （如 [`SceneRender`]) 通过 [`Transform2D`] 完成坐标转换后再传入。
///
/// Painter 不再管理相机，只负责把屏幕坐标下的图元绘制到目标画布。
pub trait Painter2D {
    /// 通用绘制入口：根据 ([`Shape`], [`Style`]) 分发到具体绘制逻辑。
    fn paint(&self, shape: &Shape, style: &Style);
}

/// 2D 坐标系转换 Trait（非消耗版本）。
///
/// 实现此 Trait 的对象可接受 [`Camera2D`] 相机参数，将自身在屏幕坐标系与
/// 逻辑坐标系之间相互转换，从而简化 2D Painter 的实现：调用方只需传入
/// 相机参数，由对象自行完成坐标变换，Painter 不必关心每个图元的细节。
///
/// - 屏幕坐标系：视口左上角为原点，y 向下，单位为像素。
/// - 逻辑坐标系：[`Camera2D::center`] 为中心，y 向上，单位为世界单位。
///
/// 对实现 [`Transform2DInto`] 且 `Clone` 的类型，本 Trait 由 blanket impl 自动提供，
/// 不需要手动实现。
pub trait Transform2D {
    /// 从屏幕坐标系转换到逻辑坐标系。
    fn to_logical(&self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系。
    fn to_screen(&self, camera: &Camera2D) -> Self;
}

/// 2D 坐标系转换 Trait（消耗 `self` 版本）。
///
/// 与 [`Transform2D`] 的区别：方法消耗 `self`，可以避免额外的 `clone`。
/// 适用于所有权可移动的场景，例如迭代器内部转换、过程宏派生等。
///
/// 派生方式：在结构体上添加 `#[derive(xcal_macro::Transform2DInto)]`。
/// 字段可用 `#[transform(skip)]` 跳过转换、`#[transform(iter)]` 走迭代器转换。
pub trait Transform2DInto {
    /// 从屏幕坐标系转换到逻辑坐标系（消耗 self）。
    fn to_logical_into(self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系（消耗 self）。
    fn to_screen_into(self, camera: &Camera2D) -> Self;
}

/// 自动为实现了 [`Transform2DInto`] 且 `Clone` 的类型派生 [`Transform2D`]。
/// 内部实现是先 `clone` 再调用 `*_into` 版本。
impl<T: Transform2DInto + Clone> Transform2D for T {
    fn to_logical(&self, camera: &Camera2D) -> Self {
        self.clone().to_logical_into(camera)
    }
    fn to_screen(&self, camera: &Camera2D) -> Self {
        self.clone().to_screen_into(camera)
    }
}

// ─── 底层 glam 类型的 Transform2DInto 实现 ─────────────────────────────────
// 由于 glam::Vec2 / glam::Vec3 是外部类型，但 Transform2DInto 是本 crate 的 trait，
// 孤儿规则允许为其实现。这些实现是所有 Vec2/Vec3 包装类型（在 types.rs 中通过
// `#[derive(xcal_macro::Transform2DInto)]` 派生）的底层依赖。

impl Transform2DInto for glam::Vec2 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            (self.x - rect_center.x) / camera.scale + camera.center.x,
            (rect_center.y - self.y) / camera.scale + camera.center.y,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            rect_center.x + (self.x - camera.center.x) * camera.scale,
            rect_center.y - (self.y - camera.center.y) * camera.scale,
        )
    }
}

impl Transform2DInto for glam::Vec3 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        // 对 3D 点做 2D 转换：x/y 走 2D 变换，z 维度保持原值。
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_logical_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_screen_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }
}

// ─── Circle 的 Transform2DInto 手动实现 ─────────────────────────────────────
// 不能直接派生：radius 需要按相机 scale 放缩（`/ scale` 或 `* scale`），
// 默认派生只能 move 或调用 `to_*_into`，无法表达这种算术关系。
impl Transform2DInto for Circle {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_logical_into(camera),
            self.radius / camera.scale,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_screen_into(camera),
            self.radius * camera.scale,
        )
    }
}

// ─── 2D 绘制命令抽象：Shape / Style / ZIndex / DrawQueue ─────────────────────
// 目的：将 Painter2D 的具体绘制函数（circle_stroke/circle_fill/...）抽象为
// (Shape, Style) 的组合，便于多线程录制 + 按 Z-index 排序的命令队列。
// 每个线程独立构建 DrawQueue（push 不立即排序，O(1)），录制完毕后再 merge
// 到主队列；merge 时双方各自 ensure_sorted（O(n log n)）后做 O(n) 归并。

/// 绘制层级。值越小越先绘制（位于画面下层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_new::new)]
pub struct ZIndex(pub i32);

/// 2D 形状变体。抽象所有可绘制的 2D 图元。
/// 与 [`Style`] 组合形成一条 [`DrawCommand`]。
#[derive(Debug, Clone)]
pub enum Shape {
    Circle(Circle),
    Path(Path),
    Line(Line),
    /// 矩形图元，复用 [`crate::types::Rect`]（min/max 角点独立完成坐标转换）。
    Rect(Rect),
}

/// 2D 样式变体。抽象所有绘制样式（填充 / 描边 / ...）。
/// 与 [`Shape`] 组合形成一条 [`DrawCommand`]。
#[derive(Debug, Clone)]
pub enum Style {
    Fill(FillSolid),
    Stroke(StrokeSolid),
}

/// 单条 2D 绘制命令 = [`ZIndex`] + [`Shape`] + [`Style`]。
///
/// 实现 `Ord`：按 `z` 升序排序（同 z 时保持插入相对顺序，因为 sort 是稳定的）。
#[derive(Debug, Clone, derive_new::new)]
pub struct DrawCommand {
    pub z: ZIndex,
    pub shape: Shape,
    pub style: Style,
}

impl PartialEq for DrawCommand {
    fn eq(&self, other: &Self) -> bool {
        self.z == other.z
    }
}
impl Eq for DrawCommand {}
impl PartialOrd for DrawCommand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for DrawCommand {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.z.cmp(&other.z)
    }
}

/// 2D 绘制命令队列，按 [`ZIndex`] 自动排序（延迟排序）。
///
/// - `push` 是 O(1) 追加，标记为未排序。
/// - `ensure_sorted` 在需要时做一次稳定排序（O(n log n)），幂等。
/// - `merge` 把两个已排序队列归并为一个，O(n)。
///
/// 典型用法：每个绘制线程持有一个 `DrawQueue`，并发 push 命令；
/// 全部完成后将各队列 merge 到主队列，再 `execute` 调用 Painter。
pub struct DrawQueue {
    commands: Vec<DrawCommand>,
    sorted: bool,
}

impl DrawQueue {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            sorted: true,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
            sorted: true,
        }
    }

    /// 追加一条命令。O(1)，不立即排序。
    pub fn push(&mut self, cmd: DrawCommand) {
        self.commands.push(cmd);
        self.sorted = false;
    }

    /// 便捷方法：压入一条 (Circle, FillSolid) 命令。
    pub fn circle_fill(&mut self, z: ZIndex, circle: Circle, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Circle(circle),
            Style::Fill(fill),
        ));
    }
    /// 便捷方法：压入一条 (Circle, StrokeSolid) 命令。
    pub fn circle_stroke(&mut self, z: ZIndex, circle: Circle, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Circle(circle),
            Style::Stroke(stroke),
        ));
    }
    /// 便捷方法：压入一条 (Path, FillSolid) 命令。
    pub fn path_fill(&mut self, z: ZIndex, path: Path, fill: FillSolid) {
        self.push(DrawCommand::new(z, Shape::Path(path), Style::Fill(fill)));
    }
    /// 便捷方法：压入一条 (Path, StrokeSolid) 命令。
    pub fn path_stroke(&mut self, z: ZIndex, path: Path, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Path(path),
            Style::Stroke(stroke),
        ));
    }
    /// 便捷方法：压入一条 (Line, StrokeSolid) 命令。
    pub fn line_stroke(&mut self, z: ZIndex, line: Line, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Line(line),
            Style::Stroke(stroke),
        ));
    }
    /// 便捷方法：压入一条 (Rect, FillSolid) 命令。
    pub fn rect_fill(&mut self, z: ZIndex, rect: Rect, fill: FillSolid) {
        self.push(DrawCommand::new(z, Shape::Rect(rect), Style::Fill(fill)));
    }
    /// 便捷方法：压入一条 (Rect, StrokeSolid) 命令。
    pub fn rect_stroke(&mut self, z: ZIndex, rect: Rect, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Rect(rect),
            Style::Stroke(stroke),
        ));
    }

    /// 确保队列按 [`ZIndex`] 升序排列。幂等。
    pub fn ensure_sorted(&mut self) {
        if !self.sorted {
            // Vec::sort 是稳定排序，同 z 命令保持插入顺序。
            self.commands.sort();
            self.sorted = true;
        }
    }

    /// 返回命令数量。
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// 是否已排序（仅用于测试与调试断言）。
    pub fn is_sorted(&self) -> bool {
        self.sorted
    }

    /// 返回命令切片。注意：调用前应先 [`ensure_sorted`] 以保证顺序。
    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// 归并两个队列，结果保持按 [`ZIndex`] 升序。
    /// 双方未排序会先各自排序，归并本身 O(n)，无 clone。
    pub fn merge(mut self, mut other: Self) -> Self {
        self.ensure_sorted();
        other.ensure_sorted();
        let total = self.commands.len() + other.commands.len();
        let mut a = self.commands.into_iter().peekable();
        let mut b = other.commands.into_iter().peekable();
        let mut merged = Vec::with_capacity(total);
        while let (Some(ac), Some(bc)) = (a.peek(), b.peek()) {
            if ac <= bc {
                merged.push(a.next().unwrap());
            } else {
                merged.push(b.next().unwrap());
            }
        }
        merged.extend(a);
        merged.extend(b);
        Self {
            commands: merged,
            sorted: true,
        }
    }

    /// 按顺序（已排序的 [`ZIndex`] 升序）将命令交给 Painter 执行。
    pub fn execute<P: Painter2D>(&mut self, painter: &P) {
        self.ensure_sorted();
        for cmd in &self.commands {
            painter.paint(&cmd.shape, &cmd.style);
        }
    }

    /// 清空队列（保留容量）。
    pub fn clear(&mut self) {
        self.commands.clear();
        self.sorted = true;
    }
}

impl Default for DrawQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 场景渲染器：遍历 [`hecs::World`] 中的图元组件，使用 [`Camera2D`] 将
/// 逻辑坐标转换为屏幕坐标后，通过 [`Painter2D::paint`] 交给底层绘制器。
///
/// Painter 不再管理相机——相机由调用方传入，[`SceneRender`] 负责坐标转换。
pub struct SceneRender<T: Painter2D> {
    painter: T,
}
/// 遍历场景中的 (图元, 样式) 组件对，用 [`Camera2D`] 转换到屏幕坐标后交给
/// `$paint` 处理。`$paint` 是一个接受 `(&Shape, &Style)` 参数的表达式
/// （如闭包），可以是 `painter.paint(...)`，也可以是录制进队列。
macro_rules! for_each_scene_primitive {
    ($w:ident $cma:ident $paint:expr; $(($shape:ident, $sty:ty, $cmd:ident))*) => {
        $(for (s, c) in $w.query::<(&$shape, &$sty)>().iter() {
            $paint(&Shape::$shape(s.to_screen($cma)), &Style::$cmd(c.clone()));
        })*
    };
}

impl<T: Painter2D> SceneRender<T> {
    pub fn new(painter: T) -> Self {
        Self { painter }
    }

    pub fn render(&self, world: &hecs::World, camera: &Camera2D) {
        for_each_scene_primitive!(
            world camera
            |sh: &Shape, st: &Style| self.painter.paint(sh, st);
            (Circle, StrokeSolid,Stroke)
            (Circle, FillSolid,Fill)
            (Path, StrokeSolid,Stroke)
            (Path, FillSolid,Fill)
            (Line, StrokeSolid,Stroke)
            (Rect, FillSolid,Fill)
            (Rect, StrokeSolid,Stroke)
        );
    }
}

/// 把场景实体录制进 [`DrawQueue`]（z=0），便于与应用自身压入的命令
/// （背景 / 网格 / 坐标轴等）按 [`ZIndex`] 统一排序后一次执行。
pub fn record_scene(world: &hecs::World, camera: &Camera2D, queue: &mut DrawQueue) {
    for_each_scene_primitive!(
        world camera
        |sh: &Shape, st: &Style| queue.push(DrawCommand::new(ZIndex::new(0), sh.clone(), st.clone()));
        (Circle, StrokeSolid,Stroke)
        (Circle, FillSolid,Fill)
        (Path, StrokeSolid,Stroke)
        (Path, FillSolid,Fill)
        (Line, StrokeSolid,Stroke)
        (Rect, FillSolid,Fill)
        (Rect, StrokeSolid,Stroke)
    );
}

#[cfg(feature = "egui")]
pub mod egui_support {
    use super::{Painter2D, Shape, Style};
    use egui::{Painter, Pos2, Shape as EguiShape, Stroke, StrokeKind};

    /// 基于 [`egui::Painter`] 的 2D 绘制器。
    ///
    /// 仅持有 [`egui::Painter`] 引用，不管理相机——所有传入的 [`Shape`] 已是
    /// 屏幕坐标系下的坐标，由 [`SceneRender`](super::SceneRender) 调用
    /// [`Transform2D`](super::Transform2D) 完成坐标转换后传入。
    pub struct Egui2dRender<'a> {
        painter: &'a Painter,
    }

    impl<'a> Egui2dRender<'a> {
        pub fn new(painter: &'a Painter) -> Self {
            Self { painter }
        }
    }

    impl<'a> Painter2D for Egui2dRender<'a> {
        fn paint(&self, shape: &Shape, style: &Style) {
            match (shape, style) {
                (Shape::Circle(c), Style::Stroke(s)) => {
                    self.painter.circle_stroke(
                        Pos2::new(c.center.x, c.center.y),
                        c.radius,
                        Stroke::new(s.width, s.color.into_egui_color32()),
                    );
                }
                (Shape::Circle(c), Style::Fill(f)) => {
                    self.painter.circle_filled(
                        Pos2::new(c.center.x, c.center.y),
                        c.radius,
                        f.color.into_egui_color32(),
                    );
                }
                (Shape::Path(p), Style::Stroke(s)) => {
                    let stroke = Stroke::new(s.width, s.color.into_egui_color32());
                    let pts: Vec<Pos2> = p.vertices.iter().map(|v| Pos2::new(v.x, v.y)).collect();
                    for w in pts.windows(2) {
                        self.painter.line_segment([w[0], w[1]], stroke);
                    }
                    if p.closed && pts.len() >= 2 {
                        self.painter
                            .line_segment([*pts.last().unwrap(), pts[0]], stroke);
                    }
                }
                (Shape::Path(p), Style::Fill(f)) => {
                    let pts: Vec<Pos2> = p.vertices.iter().map(|v| Pos2::new(v.x, v.y)).collect();
                    if pts.len() >= 3 {
                        self.painter.add(EguiShape::convex_polygon(
                            pts,
                            f.color.into_egui_color32(),
                            Stroke::NONE,
                        ));
                    }
                }
                (Shape::Line(l), Style::Stroke(s)) => {
                    self.painter.line_segment(
                        [Pos2::new(l.a.x, l.a.y), Pos2::new(l.b.x, l.b.y)],
                        Stroke::new(s.width, s.color.into_egui_color32()),
                    );
                }
                (Shape::Line(_), Style::Fill(_)) => {
                    // 线段没有填充语义，忽略。
                }
                (Shape::Rect(r), Style::Fill(f)) => {
                    self.painter.rect_filled(
                        (*r).into_egui_rect(),
                        0.0,
                        f.color.into_egui_color32(),
                    );
                }
                (Shape::Rect(r), Style::Stroke(s)) => {
                    self.painter.rect_stroke(
                        (*r).into_egui_rect(),
                        0.0,
                        Stroke::new(s.width, s.color.into_egui_color32()),
                        StrokeKind::Middle,
                    );
                }
            }
        }
    }
}

#[cfg(all(test, feature = "egui"))]
mod egui_tests {
    use super::egui_support::Egui2dRender;
    use super::{Painter2D, Shape, Style};
    use crate::scene::components::{Circle, FillSolid, Line, Path, StrokeSolid};
    use crate::types::{Rect, Vec3};

    /// 冒烟测试：Egui2dRender 能处理所有 (Shape, Style) 组合而不 panic。
    #[test]
    fn test_egui2d_render_paint_all_shapes() {
        let ctx = egui::Context::default();
        let painter = ctx.debug_painter();
        let r = Egui2dRender::new(&painter);

        let circle = Circle::new((0.0, 0.0), 1.0);
        let path = Path::new(
            vec![
                Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
                Vec3::new(glam::Vec3::new(2.0, 0.0, 0.0)),
            ],
            true,
        );
        let line = Line::new((0.0, 0.0), (1.0, 1.0));
        let rect = Rect::new((0.0, 0.0).into(), (10.0, 10.0).into());
        let stroke = StrokeSolid::new(1.0, (255, 0, 0, 255));
        let fill = FillSolid::new((0, 255, 0, 128));

        r.paint(
            &Shape::Circle(circle.clone()),
            &Style::Stroke(stroke.clone()),
        );
        r.paint(&Shape::Circle(circle), &Style::Fill(fill.clone()));
        r.paint(&Shape::Path(path.clone()), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Path(path), &Style::Fill(fill.clone()));
        r.paint(&Shape::Line(line), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Line(line), &Style::Fill(fill.clone()));
        r.paint(&Shape::Rect(rect), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Rect(rect), &Style::Fill(fill));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::{Circle, FillSolid, Line, Path, StrokeSolid};
    use crate::types::Rect;

    struct MockPainter;

    impl Painter2D for MockPainter {
        fn paint(&self, _: &Shape, _: &Style) {}
    }

    /// 构造测试用相机：中心 (0,0)、scale=1、视口 100x100。
    fn test_camera() -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            1.0,
            Rect::new((0.0, 0.0).into(), (100.0, 100.0).into()),
        )
    }

    #[test]
    fn test_scene_render_circle_stroke() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 10.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_circle_fill() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((5.0, 5.0), 20.0),
            FillSolid::new((0, 255, 0, 128)),
        ));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_path_stroke() {
        let mut world = hecs::World::new();
        let path = Path::new(
            vec![
                crate::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                crate::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
            ],
            false,
        );
        world.spawn((path, StrokeSolid::new(1.0, (0, 0, 255, 255))));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_empty_world() {
        let world = hecs::World::new();
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_mixed_entities() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 10.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        world.spawn((
            Circle::new((5.0, 5.0), 20.0),
            FillSolid::new((0, 255, 0, 128)),
        ));
        let path = Path::new(
            vec![
                crate::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                crate::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
            ],
            false,
        );
        world.spawn((path, FillSolid::new((0, 0, 255, 255))));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    // ─── DrawQueue 测试 ───────────────────────────────────────────────
    use std::cell::RefCell;

    /// 记录型 Painter：把每次调用记成 ("circle_stroke" 等) 字符串到内部 Vec。
    /// 用于断言命令队列的执行顺序。
    #[derive(Default)]
    struct RecordingPainter {
        calls: RefCell<Vec<&'static str>>,
    }

    impl Painter2D for RecordingPainter {
        fn paint(&self, shape: &Shape, style: &Style) {
            let name = match (shape, style) {
                (Shape::Circle(_), Style::Stroke(_)) => "circle_stroke",
                (Shape::Circle(_), Style::Fill(_)) => "circle_fill",
                (Shape::Path(_), Style::Stroke(_)) => "path_stroke",
                (Shape::Path(_), Style::Fill(_)) => "path_fill",
                (Shape::Line(_), Style::Stroke(_)) => "line_stroke",
                (Shape::Line(_), Style::Fill(_)) => "line_fill",
                (Shape::Rect(_), Style::Stroke(_)) => "rect_stroke",
                (Shape::Rect(_), Style::Fill(_)) => "rect_fill",
            };
            self.calls.borrow_mut().push(name);
        }
    }

    fn mk_circle(x: f32) -> Circle {
        Circle::new((x, 0.0), 1.0)
    }
    fn mk_stroke() -> StrokeSolid {
        StrokeSolid::new(1.0, (0, 0, 0, 255))
    }
    fn mk_fill() -> FillSolid {
        FillSolid::new((0, 0, 0, 255))
    }
    fn mk_path() -> Path {
        Path::new(
            vec![
                crate::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                crate::types::Vec3::new(glam::Vec3::new(1.0, 0.0, 0.0)),
            ],
            false,
        )
    }
    fn mk_line(x: f32) -> Line {
        Line::new((x, 0.0), (x, 10.0))
    }
    fn mk_rect() -> Rect {
        Rect::new((0.0, 0.0).into(), (10.0, 10.0).into())
    }

    #[test]
    fn test_scene_render_line_stroke() {
        let mut world = hecs::World::new();
        world.spawn((mk_line(5.0), mk_stroke()));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_rect_fill_and_stroke() {
        let mut world = hecs::World::new();
        world.spawn((mk_rect(), mk_fill()));
        world.spawn((mk_rect(), mk_stroke()));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_line_to_screen() {
        // SceneRender 应把 Line 端点从逻辑坐标转换为屏幕坐标。
        let mut world = hecs::World::new();
        world.spawn((mk_line(5.0), mk_stroke()));
        let renderer = SceneRender {
            painter: RecordingPainter::default(),
        };
        renderer.render(&world, &test_camera());
        // 解构取回 painter，检查录制的命令类型。
        let SceneRender { painter } = renderer;
        assert_eq!(*painter.calls.borrow(), vec!["line_stroke"]);
    }

    #[test]
    fn test_record_scene_into_queue() {
        // record_scene 把场景实体录制进队列（z=0）。
        let mut world = hecs::World::new();
        world.spawn((mk_circle(1.0), mk_fill()));
        world.spawn((mk_line(2.0), mk_stroke()));
        let mut queue = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut queue);
        assert_eq!(queue.len(), 2);
        let kinds: Vec<&str> = queue
            .commands()
            .iter()
            .map(|c| match (&c.shape, &c.style) {
                (Shape::Circle(_), Style::Fill(_)) => "circle_fill",
                (Shape::Line(_), Style::Stroke(_)) => "line_stroke",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["circle_fill", "line_stroke"]);
    }

    #[test]
    fn test_draw_queue_line_and_rect_helpers() {
        let mut q = DrawQueue::new();
        q.line_stroke(ZIndex::new(1), mk_line(1.0), mk_stroke());
        q.rect_fill(ZIndex::new(0), mk_rect(), mk_fill());
        q.rect_stroke(ZIndex::new(2), mk_rect(), mk_stroke());
        assert_eq!(q.len(), 3);
        q.ensure_sorted();
        let zs: Vec<i32> = q.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![0, 1, 2]);
    }

    #[test]
    fn test_draw_queue_push_and_sort() {
        let mut q = DrawQueue::new();
        // 故意乱序 push
        q.circle_stroke(ZIndex::new(30), mk_circle(3.0), mk_stroke());
        q.circle_fill(ZIndex::new(10), mk_circle(1.0), mk_fill());
        q.path_stroke(ZIndex::new(20), mk_path(), mk_stroke());

        q.ensure_sorted();
        let zs: Vec<i32> = q.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![10, 20, 30]);
    }

    #[test]
    fn test_draw_queue_stable_sort_same_z() {
        let mut q = DrawQueue::new();
        // 三个相同 z 的命令，push 顺序：circle_stroke / circle_fill / path_stroke
        q.circle_stroke(ZIndex::new(5), mk_circle(1.0), mk_stroke());
        q.circle_fill(ZIndex::new(5), mk_circle(2.0), mk_fill());
        q.path_stroke(ZIndex::new(5), mk_path(), mk_stroke());

        let painter = RecordingPainter::default();
        q.execute(&painter);
        // 稳定排序应保持 push 顺序
        assert_eq!(
            *painter.calls.borrow(),
            vec!["circle_stroke", "circle_fill", "path_stroke"]
        );
    }

    #[test]
    fn test_draw_queue_merge_preserves_order() {
        let mut a = DrawQueue::new();
        a.circle_stroke(ZIndex::new(10), mk_circle(1.0), mk_stroke());
        a.path_fill(ZIndex::new(30), mk_path(), mk_fill());

        let mut b = DrawQueue::new();
        b.circle_fill(ZIndex::new(20), mk_circle(2.0), mk_fill());
        b.path_stroke(ZIndex::new(40), mk_path(), mk_stroke());

        let mut merged = a.merge(b);
        let painter = RecordingPainter::default();
        merged.execute(&painter);
        // 期望按 z 升序：10(circle_stroke), 20(circle_fill), 30(path_fill), 40(path_stroke)
        assert_eq!(
            *painter.calls.borrow(),
            vec!["circle_stroke", "circle_fill", "path_fill", "path_stroke"]
        );
    }

    #[test]
    fn test_draw_queue_merge_unsorted_input_sorts_first() {
        // 双方都是乱序 push，merge 内部应先排序再归并
        let mut a = DrawQueue::new();
        a.circle_fill(ZIndex::new(40), mk_circle(4.0), mk_fill());
        a.circle_stroke(ZIndex::new(10), mk_circle(1.0), mk_stroke());

        let mut b = DrawQueue::new();
        b.path_fill(ZIndex::new(30), mk_path(), mk_fill());
        b.path_stroke(ZIndex::new(20), mk_path(), mk_stroke());

        let merged = a.merge(b);
        let zs: Vec<i32> = merged.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![10, 20, 30, 40]);
        // merged 已是 sorted 状态
        assert!(merged.is_sorted());
    }

    #[test]
    fn test_draw_queue_empty_merge() {
        let a = DrawQueue::new();
        let b = DrawQueue::new();
        let merged = a.merge(b);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_draw_queue_clear() {
        let mut q = DrawQueue::new();
        q.circle_fill(ZIndex::new(1), mk_circle(0.0), mk_fill());
        q.circle_fill(ZIndex::new(2), mk_circle(1.0), mk_fill());
        assert_eq!(q.len(), 2);
        q.clear();
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
    }

    #[test]
    fn test_draw_queue_with_capacity() {
        let q = DrawQueue::with_capacity(100);
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
    }
}
