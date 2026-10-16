// draw.rs
//
// 2D 绘制抽象：Painter2D trait + Shape2D/Style2D + ZIndex + DrawQueue +
// SceneRender + record_drawables / record_scene。
// 数学部分（Transform2D/Transform2DInto/Circle 手动 impl）在 xcal_scene::transform。
//
// M0（图元体系迁移）：载荷已统一为 [`Shape2D`](crate::shape::Shape2D) /
// [`Style2D`](crate::shape::Style2D)，旧 `Shape` / `Style` 枚举已删除。
// 录制语义保持不变：
// - 场景图元在 [`record_scene`] 里经 `to_screen(camera)` 落到**屏幕空间**后
//   再转成 `Shape2D`（`Shape2D::Path` 承载折线/闭合语义）；
// - 描边宽度在 [`record_scene`] 里经 [`resolve_width`] 结合 `camera.scale`
//   解析为屏幕像素（**唯一换算点**）。
//
// SA（绘制录制解耦）：
// - 录制入口拆成两层——[`record_drawables`] 只接受 [`Drawable`]（**不知道 ECS 存在**），
//   [`record_scene`] 负责把 hecs 组件组合成 `Drawable` 后调用它；
// - 旧「逐组件穷举宏」已删除。新增图元只需实现 [`ToShape2D`]
//   （形状）+ 复用既有 [`ResolveStyle`]（样式），**不必改动本模块的录制逻辑**。

use crate::components::{Circle, FillSolid, Line, Path, StrokeSolid};
use crate::shape::{Shape2D, Style2D};
use xcal_scene::transform::Transform2D;
use xcal_scene::types::{Camera2D, Rect};
use xc_geometry::{MeasureSpace, Rect2D, Vec2 as GeoVec2, resolve_width};

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
        // `Path` 直接持有 `Path2D`（自带闭合语义）；变换后在屏幕空间仍是
        // 同一条命令序列，仅逐命令映射坐标（含 `Close` 保留）。
        Shape2D::Path(self.to_screen(camera).path)
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

    /// 路径填充：`path` 已在屏幕空间。
    pub fn path_fill(&mut self, z: ZIndex, path: Path, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Path(path.path),
            fill_style(&fill),
        ));
    }

    /// 路径描边：`path` 已在屏幕空间。
    pub fn path_stroke(&mut self, z: ZIndex, path: Path, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Path(path.path),
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

/// 内容纹理层（S3-3）的当帧录制结果。
///
/// 内容层是**纹理源**而非显示层，其录制结果与主 2D 队列分开持有，
/// 由 `SceneCompositorCallback` 渲染到独立纹理后交给 3D 材质采样。
///
/// ⚠️ 单份资源 → 当前只支持**一个**内容纹理层（见 `ContentTextureSpec` 的 doc）。
#[derive(Default)]
pub struct ContentTextureQueue(pub DrawQueue);

// ─── 可绘制对象（ECS 无关的录制入口）────────────────────────────────────────

/// 已降级的绘制描述：能把自己转换成**屏幕空间**的 [`Shape2D`] / [`Style2D`]。
///
/// [`record_drawables`] 只依赖本 trait，**不知道 ECS 存在**——任何类型只要实现它
/// 即可进 [`DrawQueue`]。返回 `None` 表示本帧剔除（不产生绘制命令）。
///
/// ⚠️ 世界→屏幕换算只允许发生在实现体内（经 [`ToShape2D`] / [`ResolveStyle`]），
/// 不得在别处乘除 `camera.scale`（唯一换算点纪律，见 [`ResolveStyle`]）。
pub trait Drawable {
    fn to_primitive(&self, camera: &Camera2D) -> Option<(Shape2D, Style2D)>;
}

/// `(形状组件, 样式组件)` 组合即一个可绘制图元。
///
/// 泛型实现把"新增图元"的成本压到**一个 [`ToShape2D`] impl**：形状实现
/// [`ToShape2D`]、样式实现 [`ResolveStyle`] 即自动获得 `Drawable`，
/// 无需改动本模块其它任何地方（旧「逐组件穷举宏」已删除）。
///
/// 之所以是"引用对"而非裸组件：`to_primitive` 必须同时产出形状与样式，
/// 而二者在 ECS 里是两个独立组件类型。
#[allow(private_bounds)] // ToShape2D / ResolveStyle 是模块内实现细节，不外泄为公开 API
impl<S: ToShape2D, T: ResolveStyle> Drawable for (&S, &T) {
    fn to_primitive(&self, camera: &Camera2D) -> Option<(Shape2D, Style2D)> {
        Some((self.0.to_shape2d(camera), self.1.resolve_style(camera.scale)))
    }
}

/// 把一组**已降级的绘制描述**录制进 [`DrawQueue`]（ECS 无关）。
///
/// `items` 每项 = `(可绘制对象, z)`。`z` 是绘制层级（越小越先画），写入
/// `DrawCommand` 时取整为 [`ZIndex`]；同一 `z` 内保持 `items` 的迭代顺序
/// （[`DrawQueue`] 的排序是稳定的）。`to_primitive` 返回 `None` 的项被跳过（剔除）。
pub fn record_drawables<D: Drawable, I>(queue: &mut DrawQueue, camera: &Camera2D, items: I)
where
    I: IntoIterator<Item = (D, f32)>,
{
    for (item, z) in items {
        if let Some((shape, style)) = item.to_primitive(camera) {
            queue.push(DrawCommand::new(ZIndex::new(z as i32), shape, style));
        }
    }
}

/// 场景渲染器：遍历 World 中图元组件，使用 Camera2D 转换到屏幕坐标后交给 Painter2D。
pub struct SceneRender<T: Painter2D> {
    painter: T,
}

impl<T: Painter2D> SceneRender<T> {
    pub fn new(painter: T) -> Self {
        Self { painter }
    }

    /// 录制 → 按 [`ZIndex`] 排序 → 绘制。
    ///
    /// 录制复用 [`record_scene`]（与生产路径同一份组件枚举，不再有第二份列表）；
    /// [`record_scene`] 写入的 `z` 恒为 0，排序稳定 ⇒ 绘制顺序与逐组合的查询顺序
    /// 逐位一致，与重构前行为相同。
    pub fn render(&self, world: &hecs::World, camera: &Camera2D) {
        let mut queue = DrawQueue::new();
        record_scene(world, camera, &mut queue);
        queue.execute(&self.painter);
    }
}

/// 把场景实体录制进 [`DrawQueue`]（z 恒为 0）。
///
/// **组件组合的枚举只在此处**：每种组合构造一个 `(形状, 样式)` 引用对交给
/// [`record_drawables`]——录制逻辑本身不认识具体组件类型。
///
/// 描边宽度在此按 `camera.scale` 解析（**唯一换算点**，见 [`ResolveStyle`]）。
///
/// 下列组合的顺序即同 `z` 下的绘制顺序（[`DrawQueue`] 稳定排序），
/// 与重构前逐位一致。
pub fn record_scene(world: &hecs::World, camera: &Camera2D, queue: &mut DrawQueue) {
    // 场景图元的绘制层级：同层（保持查询顺序）。
    const Z: f32 = 0.0;

    record_drawables(
        queue,
        camera,
        world
            .query::<(&Circle, &StrokeSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Circle, &FillSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Path, &StrokeSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Path, &FillSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Line, &StrokeSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Rect, &FillSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
    record_drawables(
        queue,
        camera,
        world
            .query::<(&Rect, &StrokeSolid)>()
            .iter()
            .map(|(s, t)| ((s, t), Z)),
    );
}

/// 录制**单个**路径描边实体到队列（z=0，与 [`record_scene`] 同口径）。
///
/// S3-3 内容纹理层按需录制（只录带 `ContentTextureItem` 标记的实体）时用它：
/// 位置经 `to_screen(camera)` 落到相机屏幕空间，描边宽度经 [`ResolveStyle`]
/// 按 `camera.scale` 解析为屏幕像素——与 [`record_scene`] 完全一致，换算
/// 仍只发生在这里（唯一换算点不外泄）。
pub fn record_path_stroke(
    queue: &mut DrawQueue,
    path: &Path,
    stroke: &StrokeSolid,
    camera: &Camera2D,
) {
    queue.push(DrawCommand::new(
        ZIndex::new(0),
        path.to_shape2d(camera),
        stroke.resolve_style(camera.scale),
    ));
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use xcal_scene::scene::components::*;
    use xc_geometry::Path2D;

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
        let path = Path::new(Path2D::from_polyline(
            &[glam::Vec2::new(0.0, 0.0), glam::Vec2::new(1.0, 1.0)],
            false,
        ));
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
        let path = Path::new(Path2D::from_polyline(
            &[glam::Vec2::new(0.0, 0.0), glam::Vec2::new(1.0, 1.0)],
            false,
        ));
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
        Path::new(Path2D::from_polyline(
            &[glam::Vec2::new(0.0, 0.0), glam::Vec2::new(1.0, 0.0)],
            false,
        ))
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

    /// 闭合语义（M0 方案 b）：`closed` 经 `Path2D::from_polyline` 追加 `Close`，
    /// 变换后 `Shape2D::Path` 仍保留 `Close`。
    #[test]
    fn closed_path_keeps_close_command() {
        let mut world = hecs::World::new();
        world.spawn((
            Path::new(Path2D::from_polyline(
                &[
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(1.0, 0.0),
                    glam::Vec2::new(1.0, 1.0),
                ],
                true,
            )),
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
