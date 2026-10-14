// draw.rs
//
// 绘制抽象：Painter2D trait + Shape/Style/ZIndex + DrawQueue + SceneRender + record_scene
// 数学部分（Transform2D/Transform2DInto/Circle 手动 impl）在 xcal_core::transform。

use crate::components::{Circle, FillSolid, Line, Path, StrokeSolid};
use xcal_core::transform::Transform2D;
use xcal_core::types::{Camera2D, Rect};
use xc_geometry::{MeasureSpace, resolve_width};

/// 2D 绘制器 Trait。
pub trait Painter2D {
    fn paint(&self, shape: &Shape, style: &Style);
}

/// 绘制层级。值越小越先绘制（位于画面下层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_new::new)]
pub struct ZIndex(pub i32);

/// 2D 形状变体。
#[derive(Debug, Clone)]
pub enum Shape {
    Circle(Circle),
    Path(Path),
    Line(Line),
    Rect(Rect),
}

/// 2D 样式变体。
#[derive(Debug, Clone)]
pub enum Style {
    Fill(FillSolid),
    Stroke(StrokeSolid),
}

/// 单条 2D 绘制命令 = [`ZIndex`] + [`Shape`] + [`Style`]。
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

    pub fn circle_fill(&mut self, z: ZIndex, circle: Circle, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Circle(circle),
            Style::Fill(fill),
        ));
    }
    pub fn circle_stroke(&mut self, z: ZIndex, circle: Circle, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Circle(circle),
            Style::Stroke(stroke),
        ));
    }
    pub fn path_fill(&mut self, z: ZIndex, path: Path, fill: FillSolid) {
        self.push(DrawCommand::new(z, Shape::Path(path), Style::Fill(fill)));
    }
    pub fn path_stroke(&mut self, z: ZIndex, path: Path, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Path(path),
            Style::Stroke(stroke),
        ));
    }
    pub fn line_stroke(&mut self, z: ZIndex, line: Line, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Line(line),
            Style::Stroke(stroke),
        ));
    }
    pub fn rect_fill(&mut self, z: ZIndex, rect: Rect, fill: FillSolid) {
        self.push(DrawCommand::new(z, Shape::Rect(rect), Style::Fill(fill)));
    }
    pub fn rect_stroke(&mut self, z: ZIndex, rect: Rect, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape::Rect(rect),
            Style::Stroke(stroke),
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

/// 把样式组件解析为**屏幕空间**的 [`Style`]。
///
/// ⚠️ **本任务是唯一换算点**：描边宽度在此经 [`resolve_width`] 结合
/// `camera.scale` 解析为屏幕像素。项目中其它任何地方都**不得**再乘除 `scale`——
/// 否则会出现"某些描边乘过、某些没乘"的不一致。
trait ResolveStyle {
    fn resolve_style(&self, scale: f32) -> Style;
}

impl ResolveStyle for StrokeSolid {
    fn resolve_style(&self, scale: f32) -> Style {
        Style::Stroke(StrokeSolid::new_in(
            resolve_width(self.width, self.space, scale),
            self.color,
            // 已解析为屏幕像素，不再是世界度量。
            MeasureSpace::Screen,
        ))
    }
}

impl ResolveStyle for FillSolid {
    fn resolve_style(&self, _scale: f32) -> Style {
        Style::Fill(self.clone())
    }
}

macro_rules! for_each_scene_primitive {
    ($w:ident $cma:ident $paint:expr; $(($shape:ident, $sty:ty))*) => {
        $(for (s, c) in $w.query::<(&$shape, &$sty)>().iter() {
            $paint(&Shape::$shape(s.to_screen($cma)), &c.resolve_style($cma.scale));
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
        |sh: &Shape, st: &Style| queue.push(DrawCommand::new(ZIndex::new(0), sh.clone(), st.clone()));
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
    use xcal_core::scene::components::*;
    use xcal_core::types::Rect;

    struct MockPainter;

    impl Painter2D for MockPainter {
        fn paint(&self, _: &Shape, _: &Style) {}
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
                xcal_core::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_core::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
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
                xcal_core::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_core::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
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
                xcal_core::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                xcal_core::types::Vec3::new(glam::Vec3::new(1.0, 0.0, 0.0)),
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
        assert_eq!(*painter.calls.borrow(), vec!["line_stroke"]);
    }

    fn cam_with_scale(scale: f32) -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            scale,
            Rect::new((0.0, 0.0).into(), (100.0, 100.0).into()),
        )
    }

    fn queue_stroke_width(cmd: &DrawCommand) -> f32 {
        match &cmd.style {
            Style::Stroke(s) => s.width,
            other => panic!("expected stroke, got {other:?}"),
        }
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
        assert_eq!(
            *painter.calls.borrow(),
            vec!["circle_stroke", "circle_fill", "path_fill", "path_stroke"]
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
}
