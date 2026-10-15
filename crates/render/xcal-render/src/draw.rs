// draw.rs
//
// 2D 绘制抽象：Painter2D trait + Shape2D/Style2D + ZIndex + DrawQueue +
// SceneRender + record_scene。
// 数学部分（Transform2D/Transform2DInto/Circle 手动 impl）在 xcal_scene::transform。
//
// M0（图元体系迁移）：载荷已统一为 [`Shape2D`](crate::shape::Shape2D) /
// [`Style2D`](crate::shape::Style2D)，旧 `Shape` / `Style` 枚举已删除。
// 录制语义保持不变：
// - 场景图元在 [`record_scene`] 里经 `to_screen(camera)` 落到**屏幕空间**后
//   再转成 `Shape2D`（`Shape2D::Path` 承载折线/闭合语义）；
// - 描边宽度在 [`record_scene`] 里经 [`resolve_width`] 结合 `camera.scale`
//   解析为屏幕像素（**唯一换算点**）。

use crate::components::{Circle, FillSolid, Line, Path, StrokeSolid};
use crate::shape::{Shape2D, Style2D};
use xcal_scene::transform::Transform2D;
use xcal_scene::types::{Camera2D, Rect, Vec3};
use xc_geometry::{MeasureSpace, Path2D, Rect2D, Vec2 as GeoVec2, resolve_width};

/// 2D 绘制器 Trait。
pub trait Painter2D {
    fn paint(&self, shape: &Shape2D, style: &Style2D);
}

/// 绘制层级。值越小越先绘制（位于画面下层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_new::new)]
pub struct ZIndex(pub i32);

/// 单条 2D 绘制命令 = [`ZIndex`] + [`Shape2D`] + [`Style2D`]。
#[derive(Debug, Clone, derive_new::new)]
pub struct DrawCommand {
    pub z: ZIndex,
    pub shape: Shape2D,
    pub style: Style2D,
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
pub struct DrawQueue {
    commands: Vec<DrawCommand>,
    sorted: bool,
}

// ─── 组件 → Shape2D 转换（屏幕空间） ────────────────────────────────────────

/// 屏幕空间折线点（`Vec3` 取 xy）→ `Shape2D::Path`。
///
/// `closed` 语义用 `Path2D::from_polyline(points, closed)` 表达（M0 方案 b：
/// 闭合路径追加 `Close` 命令，不新增 `Shape2D::Polyline` 字段）。
fn polyline_shape(vertices: &[Vec3], closed: bool) -> Shape2D {
    let pts: Vec<GeoVec2> = vertices
        .iter()
        .map(|v| GeoVec2::new(v.x, v.y))
        .collect();
    Shape2D::Path(Path2D::from_polyline(&pts, closed))
}

fn rect_shape(r: &Rect) -> Shape2D {
    let (min, max) = (r.min(), r.max());
    Shape2D::Rect(Rect2D::new(
        GeoVec2::new(min.x, min.y),
        GeoVec2::new(max.x, max.y),
    ))
}

/// 场景图元 → 屏幕空间 [`Shape2D`]。
trait ToShape2D {
    fn to_shape2d(&self, camera: &Camera2D) -> Shape2D;
}

impl ToShape2D for Circle {
    fn to_shape2d(&self, camera: &Camera2D) -> Shape2D {
        let s = self.to_screen(camera);
        Shape2D::Circle {
            center: GeoVec2::new(s.center.x, s.center.y),
            radius: s.radius,
        }
    }
}

impl ToShape2D for Path {
    fn to_shape2d(&self, camera: &Camera2D) -> Shape2D {
        let s = self.to_screen(camera);
        polyline_shape(&s.vertices, s.closed)
    }
}

impl ToShape2D for Line {
    fn to_shape2d(&self, camera: &Camera2D) -> Shape2D {
        let s = self.to_screen(camera);
        Shape2D::Line {
            start: GeoVec2::new(s.a.x, s.a.y),
            end: GeoVec2::new(s.b.x, s.b.y),
        }
    }
}

impl ToShape2D for Rect {
    fn to_shape2d(&self, camera: &Camera2D) -> Shape2D {
        let s = self.to_screen(camera);
        rect_shape(&s)
    }
}

// ─── 样式解析（唯一换算点） ────────────────────────────────────────────────

/// 把样式组件解析为**屏幕空间**的 [`Style2D`]。
///
/// ⚠️ **本任务是唯一换算点**：描边宽度在此经 [`resolve_width`] 结合
/// `camera.scale` 解析为屏幕像素。项目中其它任何地方都**不得**再乘除 `scale`——
/// 否则会出现"某些描边乘过、某些没乘"的不一致。
trait ResolveStyle {
    fn resolve_style(&self, scale: f32) -> Style2D;
}

impl ResolveStyle for StrokeSolid {
    fn resolve_style(&self, scale: f32) -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(self.color),
            stroke_width: resolve_width(self.width, self.space, scale),
            // 已解析为屏幕像素，不再是世界度量。
            space: MeasureSpace::Screen,
        }
    }
}

impl ResolveStyle for FillSolid {
    fn resolve_style(&self, _scale: f32) -> Style2D {
        Style2D {
            fill: Some(self.color),
            stroke: None,
            stroke_width: 0.0,
            space: MeasureSpace::Screen,
        }
    }
}

/// 队列便捷方法的样式构造：**不做** scale 换算（调用方给的就是屏幕空间量，
/// 与迁移前的队列行为一致；世界→屏幕换算只发生在 [`record_scene`]）。
fn fill_style(fill: &FillSolid) -> Style2D {
    Style2D {
        fill: Some(fill.color),
        stroke: None,
        stroke_width: 0.0,
        space: MeasureSpace::Screen,
    }
}

fn stroke_style_raw(stroke: &StrokeSolid) -> Style2D {
    Style2D {
        fill: None,
        stroke: Some(stroke.color),
        stroke_width: stroke.width,
        space: stroke.space,
    }
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

    /// 圆填充：`circle.center` 已在屏幕空间。
    pub fn circle_fill(&mut self, z: ZIndex, circle: Circle, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Circle {
                center: GeoVec2::new(circle.center.x, circle.center.y),
                radius: circle.radius,
            },
            fill_style(&fill),
        ));
    }

    /// 圆描边：`circle.center` 已在屏幕空间。
    pub fn circle_stroke(&mut self, z: ZIndex, circle: Circle, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Circle {
                center: GeoVec2::new(circle.center.x, circle.center.y),
                radius: circle.radius,
            },
            stroke_style_raw(&stroke),
        ));
    }

    /// 路径填充：`path` 顶点已在屏幕空间。
    pub fn path_fill(&mut self, z: ZIndex, path: Path, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            polyline_shape(&path.vertices, path.closed),
            fill_style(&fill),
        ));
    }

    /// 路径描边：`path` 顶点已在屏幕空间。
    pub fn path_stroke(&mut self, z: ZIndex, path: Path, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            polyline_shape(&path.vertices, path.closed),
            stroke_style_raw(&stroke),
        ));
    }

    /// 线段描边：端点已在屏幕空间。
    pub fn line_stroke(&mut self, z: ZIndex, line: Line, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Line {
                start: GeoVec2::new(line.a.x, line.a.y),
                end: GeoVec2::new(line.b.x, line.b.y),
            },
            stroke_style_raw(&stroke),
        ));
    }

    /// 矩形填充：`rect` 已在屏幕空间。
    pub fn rect_fill(&mut self, z: ZIndex, rect: Rect, fill: FillSolid) {
        self.push(DrawCommand::new(z, rect_shape(&rect), fill_style(&fill)));
    }

    /// 矩形描边：`rect` 已在屏幕空间。
    pub fn rect_stroke(&mut self, z: ZIndex, rect: Rect, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            rect_shape(&rect),
            stroke_style_raw(&stroke),
        ));
    }

    pub fn ensure_sorted(&mut self) {
        if !self.sorted {
            self.commands.sort();
            self.sorted = true;
        }
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn is_sorted(&self) -> bool {
        self.sorted
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    /// 归并两个队列，结果保持按 [`ZIndex`] 升序。
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

    pub fn execute<P: Painter2D>(&mut self, painter: &P) {
        self.ensure_sorted();
        for cmd in &self.commands {
            painter.paint(&cmd.shape, &cmd.style);
        }
    }

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

/// 场景渲染器：遍历 World 中图元组件，使用 Camera2D 转换到屏幕坐标后交给 Painter2D。
pub struct SceneRender<T: Painter2D> {
    painter: T,
}

macro_rules! for_each_scene_primitive {
    ($w:ident $cma:ident $paint:expr; $(($shape:ident, $sty:ty))*) => {
        $(for (s, c) in $w.query::<(&$shape, &$sty)>().iter() {
            $paint(&s.to_shape2d($cma), &c.resolve_style($cma.scale));
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
            |sh: &Shape2D, st: &Style2D| self.painter.paint(sh, st);
            (Circle, StrokeSolid)
            (Circle, FillSolid)
            (Path, StrokeSolid)
            (Path, FillSolid)
            (Line, StrokeSolid)
            (Rect, FillSolid)
            (Rect, StrokeSolid)
        );
    }
}

/// 把场景实体录制进 [`DrawQueue`]（z=0）。
///
/// 描边宽度在此按 `camera.scale` 解析（**唯一换算点**，见 [`ResolveStyle`]）。
pub fn record_scene(world: &hecs::World, camera: &Camera2D, queue: &mut DrawQueue) {
    for_each_scene_primitive!(
        world camera
        |sh: &Shape2D, st: &Style2D| queue.push(DrawCommand::new(ZIndex::new(0), sh.clone(), *st));
        (Circle, StrokeSolid)
        (Circle, FillSolid)
        (Path, StrokeSolid)
        (Path, FillSolid)
        (Line, StrokeSolid)
        (Rect, FillSolid)
        (Rect, StrokeSolid)
    );
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use xcal_scene::scene::components::*;

    struct MockPainter;

    impl Painter2D for MockPainter {
        fn paint(&self, _: &Shape2D, _: &Style2D) {}
    }

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
                xcal_scene::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_scene::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
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
                xcal_scene::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_scene::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
            ],
            false,
        );
        world.spawn((path, FillSolid::new((0, 0, 255, 255))));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    use std::cell::RefCell;

    /// 记录绘制调用名（`形状_样式`），用于验证队列顺序与录制分支。
    #[derive(Default)]
    struct RecordingPainter {
        calls: RefCell<Vec<String>>,
    }

    fn shape_kind(shape: &Shape2D) -> &'static str {
        match shape {
            Shape2D::Circle { .. } => "circle",
            Shape2D::Path(_) => "path",
            Shape2D::Line { .. } => "line",
            Shape2D::Rect(_) => "rect",
            _ => "other",
        }
    }

    impl Painter2D for RecordingPainter {
        fn paint(&self, shape: &Shape2D, style: &Style2D) {
            let style_kind = if style.fill.is_some() { "fill" } else { "stroke" };
            self.calls
                .borrow_mut()
                .push(format!("{}_{}", shape_kind(shape), style_kind));
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
                xcal_scene::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_scene::types::Vec3::new(glam::Vec3::new(1.0, 0.0, 0.0)),
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
        let mut world = hecs::World::new();
        world.spawn((mk_line(5.0), mk_stroke()));
        let renderer = SceneRender {
            painter: RecordingPainter::default(),
        };
        renderer.render(&world, &test_camera());
        let SceneRender { painter } = renderer;
        assert_eq!(*painter.calls.borrow(), vec!["line_stroke".to_string()]);
    }

    fn cam_with_scale(scale: f32) -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            scale,
            Rect::new((0.0, 0.0).into(), (100.0, 100.0).into()),
        )
    }

    fn queue_stroke_width(cmd: &DrawCommand) -> f32 {
        assert!(cmd.style.stroke.is_some(), "expected stroke style");
        cmd.style.stroke_width
    }

    /// `World` 描边：屏幕宽度随 camera.scale 线性变化。
    #[test]
    fn world_stroke_scales_with_camera_scale() {
        let mut world = hecs::World::new();
        world.spawn((
            mk_line(1.0),
            StrokeSolid::new_in(2.0, (0, 0, 0, 255), MeasureSpace::World),
        ));

        let mut q10 = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q10);
        let mut q40 = DrawQueue::new();
        record_scene(&world, &cam_with_scale(40.0), &mut q40);

        assert!((queue_stroke_width(&q10.commands()[0]) - 20.0).abs() < 1e-4);
        assert!((queue_stroke_width(&q40.commands()[0]) - 80.0).abs() < 1e-4);
    }

    /// `Screen` 描边（默认）：屏幕宽度与 camera.scale 无关（缩放不变性）。
    #[test]
    fn screen_stroke_is_scale_invariant() {
        let mut world = hecs::World::new();
        world.spawn((mk_line(1.0), StrokeSolid::new(2.0, (0, 0, 0, 255))));

        let mut q10 = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q10);
        let mut q40 = DrawQueue::new();
        record_scene(&world, &cam_with_scale(40.0), &mut q40);

        assert!((queue_stroke_width(&q10.commands()[0]) - 2.0).abs() < 1e-6);
        assert!((queue_stroke_width(&q40.commands()[0]) - 2.0).abs() < 1e-6);
    }

    /// `WorldWithMinPx`：小 scale 下钳到下限像素。
    #[test]
    fn world_with_min_px_clamps_in_record_scene() {
        let mut world = hecs::World::new();
        world.spawn((
            mk_line(1.0),
            StrokeSolid::new_in(0.01, (0, 0, 0, 255), MeasureSpace::WorldWithMinPx(1.5)),
        ));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(1.0), &mut q);
        // 0.01 * 1 = 0.01 < 1.5 → 钳到 1.5
        assert!((queue_stroke_width(&q.commands()[0]) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn test_record_scene_into_queue() {
        let mut world = hecs::World::new();
        world.spawn((mk_circle(1.0), mk_fill()));
        world.spawn((mk_line(2.0), mk_stroke()));
        let mut queue = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut queue);
        assert_eq!(queue.len(), 2);
        let kinds: Vec<String> = queue
            .commands()
            .iter()
            .map(|c| {
                let style_kind = if c.style.fill.is_some() { "fill" } else { "stroke" };
                format!("{}_{}", shape_kind(&c.shape), style_kind)
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
        q.circle_stroke(ZIndex::new(5), mk_circle(1.0), mk_stroke());
        q.circle_fill(ZIndex::new(5), mk_circle(2.0), mk_fill());
        q.path_stroke(ZIndex::new(5), mk_path(), mk_stroke());

        let painter = RecordingPainter::default();
        q.execute(&painter);
        assert_eq!(
            *painter.calls.borrow(),
            vec![
                "circle_stroke".to_string(),
                "circle_fill".to_string(),
                "path_stroke".to_string()
            ]
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
        assert_eq!(
            *painter.calls.borrow(),
            vec![
                "circle_stroke".to_string(),
                "circle_fill".to_string(),
                "path_fill".to_string(),
                "path_stroke".to_string()
            ]
        );
    }

    #[test]
    fn test_draw_queue_merge_unsorted_input_sorts_first() {
        let mut a = DrawQueue::new();
        a.circle_fill(ZIndex::new(40), mk_circle(4.0), mk_fill());
        a.circle_stroke(ZIndex::new(10), mk_circle(1.0), mk_stroke());

        let mut b = DrawQueue::new();
        b.path_fill(ZIndex::new(30), mk_path(), mk_fill());
        b.path_stroke(ZIndex::new(20), mk_path(), mk_stroke());

        let merged = a.merge(b);
        let zs: Vec<i32> = merged.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![10, 20, 30, 40]);
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

    /// 闭合语义（M0 方案 b）：`closed = true` 经 `Shape2D::Path` 保留 `Close`。
    #[test]
    fn closed_path_keeps_close_command() {
        let mut world = hecs::World::new();
        world.spawn((
            Path::new(
                vec![
                    xcal_scene::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                    xcal_scene::types::Vec3::new(glam::Vec3::new(1.0, 0.0, 0.0)),
                    xcal_scene::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
                ],
                true,
            ),
            mk_stroke(),
        ));
        let mut q = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut q);
        let Shape2D::Path(p) = &q.commands()[0].shape else {
            panic!("expected Shape2D::Path");
        };
        assert!(
            matches!(p.commands.last(), Some(xc_geometry::PathCommand::Close)),
            "闭合路径必须以 Close 结尾"
        );
    }

    /// 圆半径随 camera.scale 缩放（`Circle::to_screen` 语义经 `Shape2D` 保留）。
    #[test]
    fn circle_radius_scales_with_camera() {
        let mut world = hecs::World::new();
        world.spawn((Circle::new((0.0, 0.0), 2.0), mk_stroke()));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);
        let Shape2D::Circle { radius, .. } = &q.commands()[0].shape else {
            panic!("expected Shape2D::Circle");
        };
        assert!((radius - 20.0).abs() < 1e-4, "radius = {radius}");
    }
}
