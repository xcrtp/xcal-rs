// draw.rs
//
// 2D 绘制抽象：Painter2D trait + Shape2D/Style2D + ZIndex + DrawQueue +
// SceneRender + record_drawables / record_scene。
// 数学部分（Transform2D/Transform2DInto/Circle 手动 impl）在 ze_scene_core::transform。
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
//   [`record_scene`] 负责把**契约层 `Scene`** 里的组件组合成 `Drawable` 后调用它
//   （M3-7 起不认识 hecs —— `impl Scene for hecs::World` 在 zela-scene）；
// - 旧「逐组件穷举宏」已删除。新增图元只需实现 [`ToShape2D`]
//   （形状）+ 复用既有 [`ResolveStyle`]（样式），**不必改动本模块的录制逻辑**。
//
// S4.3（形变上 GPU）：
// - 形变不走 [`DrawCommand`]（那是冻结结构，装不下 `to` / `t`），而是经
//   [`MorphQueue`] 以"两端几何 + `t`"的原样形态直达 2D 后端；
// - 两端几何与 `t` 无关 ⇒ 三角化与 GPU 缓冲都帧间命中，每帧只写一个 `t` uniform。

use crate::components::{Arrow, Circle, FillSolid, Line, Path, Rect, StrokeSolid};
use crate::shape::{Shape2D, Style2D};
use ze_notion::Transform2D as Affine2D;
use ze_scene_core::Camera2D;
// 对象层的绘制层级（M4）：可缺席 —— 缺席即用录制默认层。
// T1：逐对象摆放（`Transform`，可缺席 ⇒ 恒等）。
// `ZOrder` 由对象层 re-export（唯一声明在 IR，B2-1 单一真源）。
use ze_scene_core::{Transform, ZOrder};
// `Path::to_screen(camera)`（世界 → 屏幕）由 zela-scene 的场景层 trait 提供。
use ze_scene_core::transform::Transform2D as _;
// 契约层 `SceneRead`：`record_scene` 的唯一入参抽象（M3-7 —— 本 crate 不认识 hecs，
// 且**只需要读能力**）。
use ze_scene_contract::{GroupVisibility, ObjectId, SceneRead, Visibility};
// 降级 trait 与样式组件定义在渲染 IR 层（M2）：
// - `ToShape2D` 吃**仿射矩阵**（相机在调用点降级），`ResolveStyle` 吃标量 scale
//   ⇒ IR 层不认识相机类型，不成环（见 ze_render_ir::degrade）。
// - `Camera2D::to_transform2d()` 由场景对象层的 `Camera2D` 提供（M3 迁往 ze-object）。
// - `ToShape2D` 的图元 impl 在场景对象层 `ze-object`（孤儿规则：实现里必须有本地类型）。
use ze_render_ir::{
    DrawCommand, Opacity, ResolveStyle, ToShape2D, ZIndex, fill_style, stroke_style_raw,
};

/// 2D 绘制器 Trait。
///
/// # 当前状态（S4.1 后）
///
/// 本 trait **零生产实现**——实际 2D 渲染路径是 [`DrawQueue`] 录制 + 后端直消费
/// （`zela-render-backends::wgpu_2d` 走 lyon 三角化）。旧 CPU 回放实现已在 S4.1
/// 随其编译期 feature 一并删除；本 trait 与
/// [`DrawQueue::execute`] 仅由测试 mock 使用。按
/// [`crate::Painter3D`] 的先例**保留 + 标注状态**：去留待后续阶段决定
/// （删掉后 `DrawQueue` 只剩 GPU 一条消费路径，将来加非 GPU 后端要重建）。
pub trait Painter2D {
    fn paint(&self, shape: &Shape2D, style: &Style2D);
}

/// 队列中的一条绘制项（B2-1：**一个 DrawQueue**，2D / 3D 命令共存，D2）。
///
/// 三种载荷共享同一个排序键 [`ZOrder`]；消费方按载荷过滤（2D 后端只取
/// `Shape`，3D 后端只取 `Mesh` / `Line`）。
#[derive(Debug, Clone)]
pub enum DrawItem {
    /// 2D 形状命令（屏幕空间）。
    Shape(DrawCommand),
    /// 3D 网格绘制命令。
    Mesh(crate::draw_queue_3d::MeshDraw),
    /// 3D 线段绘制命令。
    Line(crate::draw_queue_3d::LineDraw),
}

impl DrawItem {
    /// 统一排序键。
    pub fn zorder(&self) -> ZOrder {
        match self {
            Self::Shape(cmd) => cmd.z,
            Self::Mesh(mesh) => mesh.z,
            Self::Line(line) => line.z,
        }
    }
}

/// 绘制命令队列（B2-1 统一队列）：2D 形状与 3D 网格/线段共存，
/// 按 [`ZOrder`] **稳定**排序（延迟排序）。
///
/// # 确定性保证（R-F）
/// 排序是**稳定**的（`Vec::sort_by_key`）：同 `ZOrder` 内严格保持**插入序**。
/// 这是对拍逐位一致的隐含前提，必须先写成显式测试（`test_queue_stable_within_equal_zorder`），
/// 任何改用非稳定排序的改动都会让它变红。
pub struct DrawQueue {
    items: Vec<DrawItem>,
    sorted: bool,
    /// B3 Step 2：Surface 路由 / Direct 派生的**显式错误**（每帧由 2D 录制清空）。
    ///
    /// 不静默：归属环、缺 renderer、Direct 不支持的能力、派生失败都记在这里，
    /// 由宿主读取（`surface_errors()`）；不影响队列载荷。
    surface_errors: Vec<crate::surface_route::SurfaceRecordError>,
}

// ─── 组件 → Shape2D 转换（屏幕空间） ────────────────────────────────────────
//
// M3-5：图元 → `Shape2D` 的映射（`ToShape2D` impl 与相机仿射映射）已随图元
// 迁往场景对象层 `ze-object` —— 那里才是"实现里含本地类型"的合法落点
// （孤儿规则）。本模块只保留"队列便捷方法"用的**已是屏幕空间**的构造。

fn rect_shape(r: &Rect) -> Shape2D {
    Shape2D::Rect(r.0)
}

impl DrawQueue {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            sorted: true,
            surface_errors: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            sorted: true,
            surface_errors: Vec::new(),
        }
    }

    /// 本帧的 Surface 路由 / Direct 派生错误（显式，不静默）。
    pub fn surface_errors(&self) -> &[crate::surface_route::SurfaceRecordError] {
        &self.surface_errors
    }

    /// 追加一条 Surface 路由错误（crate 内录制使用）。
    pub(crate) fn push_surface_error(
        &mut self,
        error: crate::surface_route::SurfaceRecordError,
    ) {
        self.surface_errors.push(error);
    }

    /// 清空 Surface 路由错误（每帧 2D 录制开始时调用）。
    pub fn clear_surface_errors(&mut self) {
        self.surface_errors.clear();
    }

    /// 追加一条 2D 命令。O(1)，不立即排序。
    pub fn push(&mut self, cmd: DrawCommand) {
        self.items.push(DrawItem::Shape(cmd));
        self.sorted = false;
    }

    /// 追加任意队列项（3D 载荷经此入队；crate 内使用）。
    pub(crate) fn push_item(&mut self, item: DrawItem) {
        self.items.push(item);
        self.sorted = false;
    }

    /// 圆填充：`circle.center` 已在屏幕空间。
    pub fn circle_fill(&mut self, z: ZIndex, circle: Circle, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Circle {
                center: circle.center,
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
                center: circle.center,
                radius: circle.radius,
            },
            stroke_style_raw(&stroke),
        ));
    }

    /// 路径填充：`path` 已在屏幕空间。
    pub fn path_fill(&mut self, z: ZIndex, path: Path, fill: FillSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Path(path.0),
            fill_style(&fill),
        ));
    }

    /// 路径描边：`path` 已在屏幕空间。
    pub fn path_stroke(&mut self, z: ZIndex, path: Path, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Path(path.0),
            stroke_style_raw(&stroke),
        ));
    }

    /// 线段描边：端点已在屏幕空间。
    pub fn line_stroke(&mut self, z: ZIndex, line: Line, stroke: StrokeSolid) {
        self.push(DrawCommand::new(
            z,
            Shape2D::Line {
                start: line.a,
                end: line.b,
            },
            stroke_style_raw(&stroke),
        ));
    }

    /// 矩形填充：`rect` 已在屏幕空间。
    ///
    /// 接受 `impl Into<Rect>`：场景层的视口矩形（`ze_scene_core::types::Rect`）与
    /// 绘制用矩形对象（`ze_scene_core::Rect`）同名不同物，转换由 `ze-object` 的
    /// `From` 实现承担（在 `zela-scene` 侧提供，避免对象层反向依赖）。
    pub fn rect_fill(&mut self, z: ZIndex, rect: impl Into<Rect>, fill: FillSolid) {
        let rect: Rect = rect.into();
        self.push(DrawCommand::new(z, rect_shape(&rect), fill_style(&fill)));
    }

    /// 矩形描边：`rect` 已在屏幕空间（`Into<Rect>` 语义同 [`DrawQueue::rect_fill`]）。
    pub fn rect_stroke(&mut self, z: ZIndex, rect: impl Into<Rect>, stroke: StrokeSolid) {
        let rect: Rect = rect.into();
        self.push(DrawCommand::new(
            z,
            rect_shape(&rect),
            stroke_style_raw(&stroke),
        ));
    }

    /// 稳定排序（同 `ZOrder` 内保持插入序 —— R-F 的确定性保证）。
    pub fn ensure_sorted(&mut self) {
        if !self.sorted {
            // `sort_by_key` 是**稳定**排序；不得改用 `sort_unstable_by_key`。
            self.items.sort_by_key(DrawItem::zorder);
            self.sorted = true;
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn is_sorted(&self) -> bool {
        self.sorted
    }

    /// 全部队列项（结构判据 / 测试用；2D 消费方用 [`Self::commands`]）。
    pub fn items(&self) -> &[DrawItem] {
        &self.items
    }

    /// 2D 形状命令（按当前队列顺序；未排序时即插入序）。
    pub fn commands(&self) -> Vec<&DrawCommand> {
        self.items
            .iter()
            .filter_map(|item| match item {
                DrawItem::Shape(cmd) => Some(cmd),
                _ => None,
            })
            .collect()
    }

    /// [`Self::commands`] 的语义化别名（2D 后端用）。
    pub fn shapes(&self) -> Vec<&DrawCommand> {
        self.commands()
    }

    /// 归并两个队列，结果保持按 [`ZOrder`] 升序（稳定：self 在 other 之前）。
    pub fn merge(mut self, other: Self) -> Self {
        self.items.extend(other.items);
        self.sorted = false;
        self.ensure_sorted();
        self
    }

    /// 排序后逐条 `paint`（只回放 2D 形状；3D 项由后端消费）。
    ///
    /// ⚠️ S4.1 后**零生产调用者**（仅测试 mock）——生产路径是后端直接消费
    /// [`DrawQueue::commands`]；保留原因见 [`Painter2D`] 的"当前状态"。
    pub fn execute<P: Painter2D>(&mut self, painter: &P) {
        self.ensure_sorted();
        for item in &self.items {
            if let DrawItem::Shape(cmd) = item {
                painter.paint(&cmd.shape, &cmd.style);
            }
        }
    }

    /// 清空整个队列（2D + 3D）；帧末统一清理用。
    pub fn clear(&mut self) {
        self.items.clear();
        self.surface_errors.clear();
        self.sorted = true;
    }

    /// 只清空 3D 项（2D 项保留）。
    ///
    /// 录制 System 各自清自己的载荷：2D / 3D 录制同相位执行、顺序不保证，
    /// 用整队 `clear()` 会互相擦除（B2-1 合并队列后的必守约束）。
    pub fn clear_meshes_and_lines(&mut self) {
        self.items
            .retain(|item| !matches!(item, DrawItem::Mesh(_) | DrawItem::Line(_)));
    }
}

impl Default for DrawQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 内容纹理层（S3-3）的当帧录制结果（**按 `LayerId` 索引的容器**）。
///
/// 内容层是**纹理源**而非显示层，其录制结果与主 2D 队列分开持有，
/// 由 `SceneCompositorCallback` 渲染到独立纹理后交给 3D 材质采样。
///
/// **B3 Step 4c-2**：单份 → 多份；键是 `ContentTextureSpec::layer_id()`
/// （由 Surface 的 `ObjectId` 稳定派生，见 `ze_scene_core::domain::content_layer_id`）。
#[derive(Default)]
pub struct ContentTextureQueue(pub std::collections::BTreeMap<ze_render_ir::LayerId, DrawQueue>);

impl ContentTextureQueue {
    /// 取某一内容层的当帧队列（不存在 ⇒ `None`）。
    pub fn get(&self, id: ze_render_ir::LayerId) -> Option<&DrawQueue> {
        self.0.get(&id)
    }

    /// 取或建某一内容层的队列（录制 System 用）。
    pub fn queue_mut(&mut self, id: ze_render_ir::LayerId) -> &mut DrawQueue {
        self.0.entry(id).or_default()
    }
}

// ─── 形变绘制项（S4.3） ───────────────────────────────────────────────────

/// 形变绘制项：形变的两端 `from` / `to` 是**常量**，逐帧变的只有 `t`。
///
/// 因此几何可一次性三角化并缓存（帧间零重算、零上传），每帧只写一个 `t`
/// uniform —— 与 G7「描边宽度参数化」完全同一模式（几何静态、变量走 uniform）。
///
/// 为什么不走 [`Shape2D`] / [`DrawCommand`]：那两者是**冻结结构**，承载不了
/// `to` / `t` 这对额外信息；形变以"两端几何 + `t`"的原样形态直达 2D 后端。
///
/// ⚠️ 两端都会被**弧长等点重采样**到 `samples` 个点（见
/// [`MorphItem::sampled_endpoints`]），保证两条几何的顶点/索引一一对应，
/// 于是 GPU 的 `mix(from, to, t)` 是逐点插值而不是错位连接。
#[derive(Debug, Clone)]
pub struct MorphItem {
    /// 形变起点路径（世界/屏幕空间由 `style.space` 与相机决定，与 [`Path`] 同口径）。
    pub from: ze_notion::Path2D,
    /// 形变终点路径。
    pub to: ze_notion::Path2D,
    /// 形变参数 ∈ [0, 1]（由 `BindingApplySystem` 写 `MorphShape.t`）。
    pub t: f32,
    /// 等点采样数（顶点数与该值无关，只决定两端几何的点数）。
    pub samples: u32,
    /// 已解析为屏幕空间的样式（唯一换算点仍是 [`ResolveStyle`]）。
    pub style: Style2D,
}

impl MorphItem {
    /// 两端经**弧长等点重采样**后的折线（点数恒为 `samples`；闭合与否见下）。
    ///
    /// 与 [`ze_notion::Path2D::morph_points`] 的采样口径一致（同样的 `n` /
    /// `MORPH_TOLERANCE`），但**闭合性由两端共同决定**、且**与 `t` 无关**
    /// ⇒ 结果可逐帧缓存，是"几何帧间不变"的关键。
    ///
    /// # 闭合性（实测修）
    ///
    /// 早期版本**恒按闭合环**重采样：开口曲线（如 `y = sin x`）会因此多出一条
    /// **首尾相接的弦** —— 在分镜里表现为 x 轴上一条可见的青色直线。
    /// 现在改为：**任一端自带闭合语义（末命令 `Close`）即闭合，否则开口**
    /// （两端必须同口径，逐点插值才成立；闭合是超集：闭合端按环采、开口端按折线采
    /// 都会得到 `n` 个点，但当两端都开口时不再补弦）。
    ///
    /// 退化处理：任一侧为空时用另一侧补齐两端。
    ///
    /// ⚠️ **零长度但不为空的一侧**（"退化点"起点，全点重合）**不在这里处理** ——
    /// lyon 对零长度路径不产出几何（实测 `tess_len = 0`），2D 后端会在
    /// `prepare` 阶段用另一侧的拓扑合成它（见 `wgpu_2d.rs` 的 `collapsed_like`）；
    /// 本函数只负责"点数对齐 + 闭合口径一致"这两件事。
    pub fn sampled_endpoints(&self) -> (ze_notion::Path2D, ze_notion::Path2D) {
        use ze_notion::{MORPH_TOLERANCE, Path2D};
        let n = self.samples.max(1) as usize;
        let closed = is_closed_path(&self.from) || is_closed_path(&self.to);
        let a = self.from.resample_arc_length(n, MORPH_TOLERANCE, closed);
        let b = self.to.resample_arc_length(n, MORPH_TOLERANCE, closed);
        match (a.is_empty(), b.is_empty()) {
            (true, true) => (Path2D::new(), Path2D::new()),
            (true, false) => {
                let p = Path2D::from_polyline(&b, closed);
                (p.clone(), p)
            }
            (false, true) => {
                let p = Path2D::from_polyline(&a, closed);
                (p.clone(), p)
            }
            (false, false) => (
                Path2D::from_polyline(&a, closed),
                Path2D::from_polyline(&b, closed),
            ),
        }
    }
}

/// 路径是否自带闭合语义（末命令为 [`ze_notion::PathCommand::Close`]）。
fn is_closed_path(p: &ze_notion::Path2D) -> bool {
    matches!(p.commands.last(), Some(ze_notion::PathCommand::Close))
}

/// 形变绘制队列（S4.3）：当帧的形变项，与 [`DrawQueue`] 并行持有。
///
/// 渲染顺序说明见 `zela-render-backends::wgpu_2d::finish_frame`：形变项排在
/// `DrawQueue` 的全部几何之后（二者同为 z = 0 的内容；网格/轴线的 z < 0 不受影响）。
#[derive(Default)]
pub struct MorphQueue(pub Vec<MorphItem>);

// ─── 书写动画项（R0 reveal） ──────────────────────────────────────────────

/// 书写动画（reveal）绘制项：**一条描边路径 + 一个可见区间 `[start, end]`**。
///
/// 与 [`MorphItem`] 同一设计口径（几何静态、变量走 uniform）：
/// - 几何 = 路径按弧长预细分 + 每顶点归一化弧长 `s`（`advance`）⇒ **帧间不变**；
/// - 逐帧变的只有 `[start, end]`（走 2D uniform 的 `params.z / params.w`）。
/// ⇒ 三角化与 GPU 缓冲都命中缓存，稳态零上传；也**不进合批**（区间是 uniform，
/// 同批只能共享一个区间），每项一次 `draw_indexed`。
///
/// 与 [`MorphItem`] 的差别：morph 是"两端几何插值"，reveal 是"沿一条几何推进"，
/// 二者可作用于同一条路径的不同实体，但**同一路径不宜同时挂两者**（计划 §12.3）。
#[derive(Debug, Clone)]
pub struct RevealItem {
    /// 待书写的路径（世界/屏幕空间由 `style.space` 与相机决定，与 [`Path`] 同口径）。
    pub path: ze_notion::Path2D,
    /// 可见区间起点（归一化弧长 ∈ [0,1]）。
    pub start: f32,
    /// 可见区间终点（归一化弧长 ∈ [0,1]）。
    pub end: f32,
    /// 已解析为屏幕空间的**描边**样式（唯一换算点仍是 [`ResolveStyle`]）。
    pub style: Style2D,
}

/// 书写动画队列（R0）：当帧的 reveal 项，与 [`DrawQueue`] / [`MorphQueue`] 并行持有。
///
/// 渲染顺序与 `MorphQueue` 相同（排在 `DrawQueue` 全部几何之后）。
#[derive(Default)]
pub struct RevealQueue(pub Vec<RevealItem>);

// ─── 可绘制对象（ECS 无关的录制入口）────────────────────────────────────────

/// 已降级的绘制描述：能把自己转换成**屏幕空间**的 [`Shape2D`] / [`Style2D`]。
///
/// [`record_drawables`] 只依赖本 trait，**不知道 ECS 存在**——任何类型只要实现它
/// 即可进 [`DrawQueue`]。返回 `None` 表示本帧剔除（不产生绘制命令）。
///
/// ⚠️ 世界→屏幕换算只允许发生在实现体内（经 [`ToShape2D`] / [`ResolveStyle`]），
/// 不得在别处乘除 `scale`（唯一换算点纪律，见 [`ResolveStyle`]）。
pub trait Drawable {
    /// `t`：相机降级出的仿射矩阵（见 [`ToShape2D`]）；
    /// `scale`：`Camera2D.scale`，供 [`ResolveStyle`] 解析世界/屏幕宽度。
    fn to_primitive(&self, t: &Affine2D, scale: f32) -> Option<(Shape2D, Style2D)>;
}

/// `(形状组件, 样式组件)` 组合即一个可绘制图元。
///
/// 泛型实现把"新增图元"的成本压到**一个 [`ToShape2D`] impl**：形状实现
/// [`ToShape2D`]、样式实现 [`ResolveStyle`] 即自动获得 `Drawable`，
/// 无需改动本模块其它任何地方（旧「逐组件穷举宏」已删除）。
///
/// 之所以是"引用对"而非裸组件：`to_primitive` 必须同时产出形状与样式，
/// 而二者在 ECS 里是两个独立组件类型。
impl<S: ToShape2D, T: ResolveStyle> Drawable for (&S, &T) {
    fn to_primitive(&self, t: &Affine2D, scale: f32) -> Option<(Shape2D, Style2D)> {
        Some((self.0.to_shape2d(t), self.1.resolve_style(scale)))
    }
}

/// 把一组**已降级的绘制描述**录制进 [`DrawQueue`]（ECS 无关）。
///
/// `items` 每项 = `(可绘制对象, z)`。`z` 是绘制层级（越小越先画），写入
/// `DrawCommand` 时取整为 [`ZIndex`]；同一 `z` 内保持 `items` 的迭代顺序
/// （[`DrawQueue`] 的排序是稳定的）。`to_primitive` 返回 `None` 的项被跳过（剔除）。
pub fn record_drawables<D: Drawable, I>(queue: &mut DrawQueue, t: &Affine2D, scale: f32, items: I)
where
    I: IntoIterator<Item = (D, f32)>,
{
    for (item, z) in items {
        if let Some((shape, style)) = item.to_primitive(t, scale) {
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
    pub fn render<S: SceneRead>(&self, scene: &S, camera: &Camera2D) {
        let mut queue = DrawQueue::new();
        record_scene(scene, camera, &mut queue);
        queue.execute(&self.painter);
    }
}

// ─── 对象级可见性与不透明度（N3 / N4） ─────────────────────────────────────
//
// 两者都是**可缺席组件 = 默认值**（照 `ZOrder` 惯例）：
// - `Visibility` 缺席 ⇒ 全局可见；
// - `Opacity` 缺席 ⇒ 1.0（不调制）。
//
// 查询方式：`for_each_pair_opt` 的闭包**第一个参数就是 `ObjectId`**，故在闭包内用
// `scene.with::<T, _>(id, ..)` 点查询即可 —— **契约层不必新增四元组访客**
// （`for_each_pair_opt` 的三个位置已被 `(形状, 样式, ZOrder)` 占满）。
// `with` 与 `for_each*` 同为 `&self` 不可变借用 ⇒ 闭包内可再借用 `scene`。

/// 对象级可见性判定（N3）：**缺席 = 全局可见**。
pub fn visible_for<S: SceneRead>(
    scene: &S,
    id: ObjectId,
    camera_tag: Option<ObjectId>,
    groups: &GroupVisibility,
) -> bool {
    scene
        .with::<Visibility, _>(id, |v| v.visible_for(camera_tag, groups))
        .unwrap_or(true)
}

/// 对象级不透明度（N4）：**缺席 = 1.0**。
pub fn opacity_of<S: SceneRead>(scene: &S, id: ObjectId) -> Opacity {
    scene
        .with::<Opacity, _>(id, |o| *o)
        .unwrap_or(Opacity::FULL)
}

/// **把不透明度施加到已解析的样式上**（N4：全仓**唯一**的施加点）。
///
/// - 只缩放颜色的 **alpha**（`Opacity::apply` 的唯一乘法实现），不做别的改动；
/// - `Opacity::FULL`（= 缺席）时**直接返回** ⇒ 画面逐位不变；
/// - 描边与填充都乘 —— 淡入淡出对"填充 + 描边"同时生效（这正是独立 `Opacity`
///   组件相对"改 `color` 的 alpha"的收益，见 `ze_render_ir::Opacity` 的说明）。
pub fn apply_opacity(style: &mut Style2D, opacity: Opacity) {
    if opacity.is_full() {
        return;
    }
    style.fill = style.fill.map(|c| opacity.apply(c));
    style.stroke = style.stroke.map(|c| opacity.apply(c));
}

/// 录制**一组** `(图元, 样式)` 组合：契约层访客形态 → [`DrawQueue`]。
///
/// 拆出来是为了让 [`record_scene_tagged`] 的 9 组调用保持"一组一行"的可读性，
/// 同时把"**可见性过滤** + 降级 + **不透明度施加** + 剔除 + 入队"的逻辑只写一遍。
///
/// 层级（M4）：对象上**可选**的 [`ZOrder`] 优先（网格 / 坐标轴靠它压在内容之下），
/// 缺席则用 `z_default`（内容默认层）。可缺席是必须的：绝大多数对象不分层。
///
/// 顺序（N5）：**先判可见性**（不可见 ⇒ 连降级都不做），再降级，最后施加不透明度。
///
/// 摆放（T1）：对象上**可选**的 [`Transform`] 提供逐对象本地变换；
/// 世界 → 屏幕 = `相机 ∘ 对象` ⇒ 逐对象仿射**先**作用于几何，相机仿射**后**作用
/// （`Transform2D::compose` 的"先 self 后 other"语义，见 `ze_scene_core::Transform::to_affine2d`）。
/// **缺席 `Transform` ⇒ 直接用相机仿射（逐位不变）** —— 不引入"乘恒等"的浮点噪声。
fn record_group<S, A, B>(
    scene: &S,
    t: &Affine2D,
    scale: f32,
    camera_tag: Option<ObjectId>,
    groups: &GroupVisibility,
    worlds: Option<&ze_scene_core::WorldTransforms>,
    queue: &mut DrawQueue,
    z_default: f32,
    surface_filter: Option<ObjectId>,
) where
    S: SceneRead,
    A: ToShape2D + Send + Sync + 'static,
    B: ResolveStyle + Send + Sync + 'static,
{
    scene.for_each_pair_opt::<A, B, ZOrder>(|id, a, b, z| {
        if !visible_for(scene, id, camera_tag, groups) {
            return;
        }
        // B3 Step 1：内容层录制按 Surface 归属过滤（Parent 链，唯一条件）。
        if let Some(surface) = surface_filter {
            match ze_scene_core::resolve_surface(scene, id) {
                Ok(Some(owner)) if owner.surface == surface => {}
                Ok(_) => return,
                Err(source) => {
                    queue.push_surface_error(crate::surface_route::SurfaceRecordError::Route {
                        id,
                        source,
                    });
                    return;
                }
            }
        }
        // B3 Step 2：Surface 显式路由。Renderer3D ⇒ 派生为网格（不再走 2D）；
        // Renderer2D / 无 Surface ⇒ 默认 2D 路径（像素逐位不变）。
        if crate::surface_route::try_record_direct(scene, id, a, b, z.copied(), worlds, queue) {
            return;
        }
        // S5：优先用父子累乘的**世界矩阵**（S1：2D 读投影视图）；缺席 WorldTransforms / 该对象 ⇒ local。
        // 无 `Parent` 时 WorldTransforms 存的就是 local 的逐位拷贝 ⇒ 默认行为不变。
        let local = scene.with::<Transform, _>(id, |tr| tr.to_affine2d());
        let obj = worlds
            .and_then(|w| w.get2d(id))
            .or(local);
        let world_t = match obj {
            Some(obj) => obj.compose(*t),
            None => *t,
        };
        if let Some((shape, mut style)) = (a, b).to_primitive(&world_t, scale) {
            apply_opacity(&mut style, opacity_of(scene, id));
            let z = z.map_or(z_default as i32, |z| z.0);
            queue.push(DrawCommand::new(ZIndex::new(z), shape, style));
        }
    });
}

/// 把场景实体录制进 [`DrawQueue`]（z 恒为 0）。
///
/// **签名冻结**（多次重申，不得改）：本函数 = [`record_scene_tagged`] 的
/// "**不过滤**"形态（`camera_tag = None`、无隐藏组）⇒ **行为逐位不变**。
/// 需要按相机/组过滤的调用方（`SceneRecordSystem`）请用
/// [`record_scene_tagged`]（N5 新增，见那里对"为什么拆一个函数"的说明）。
pub fn record_scene<S: SceneRead>(scene: &S, camera: &Camera2D, queue: &mut DrawQueue) {
    record_scene_tagged(scene, camera, None, &GroupVisibility::default(), queue);
}

/// 带**对象级可见性**判定的录制入口（N5）。
///
/// # 为什么拆一个新函数而不是给 `record_scene` 加参数
///
/// `record_scene` 的签名是**冻结契约**（多次重申）；而且它只拿得到 `&Camera2D`，
/// **拿不到相机的 `ObjectId`** —— 而 `VisibilityTag::Camera(..)` 的判定恰恰需要它。
/// ⇒ 新增本函数承载"tag 感知"的录制，`record_scene` 以 `None` 调它 ⇒
/// 冻结签名不动、行为不变。
///
/// - `camera_tag`：当前 2D 相机的 [`ObjectId`]（`None` = 不过滤相机 tag）；
/// - `groups`：组级开关（[`GroupVisibility`]，默认空 ⇒ 所有组可见）。
///
/// **组件组合的枚举只在此处**：每种组合交给 [`record_group`]——录制逻辑本身
/// 不认识具体组件类型。描边宽度也在此按 `camera.scale` 解析
/// （**唯一换算点**，见 [`ResolveStyle`]）。
///
/// 下列组合的顺序即同 `z` 下的绘制顺序（[`DrawQueue`] 稳定排序），
/// 与重构前逐位一致。
///
/// **M3-7：不认识 hecs。** 参数是契约层 [`Scene`]（`for_each_pair_opt` 完成 9 组
/// `(图元, 样式)` 组合查询）；hecs 的实现（适配器 `WorldScene`）在 `zela-scene`，
/// 本 crate 的依赖里没有 hecs。
///
/// **M4：层级来自对象。** 带 [`ZOrder`] 的对象用它自己的层级（网格 z = -100、
/// 坐标轴 z = -90，见 `ze_scene_core::axes`），其余对象落在 `Z`（内容层）。
///
/// **N5：可见性来自对象。** 带 [`Visibility`] 的对象按"对谁可见"过滤
/// （缺席 = 全局可见）；不透明度见 [`apply_opacity`]。
///
/// ⚠️ **总闸优先（N3）**：图层级总闸（`UiConfig::show_2d`，在 `build_scene_view`
/// 门控）关掉时**根本不会走到这里**（整层不声明 ⇒ 不录制）——本函数的对象级
/// `Visibility` 判定只在总闸开启后生效，二者是"整层 / 逐对象"两级，不互相取消。
pub fn record_scene_tagged<S: SceneRead>(
    scene: &S,
    camera: &Camera2D,
    camera_tag: Option<ObjectId>,
    groups: &GroupVisibility,
    queue: &mut DrawQueue,
) {
    record_scene_tagged_with(scene, camera, camera_tag, groups, None, queue);
}

/// [`record_scene_tagged`] 的 **S5 版本**：额外消费父子累乘后的世界仿射。
///
/// `worlds = None` ⇒ 逐位等于旧行为（只用 local `Transform`）。
/// `worlds = Some(..)` ⇒ 有世界仿射的对象用世界仿射，其余仍回退 local。
pub fn record_scene_tagged_with<S: SceneRead>(
    scene: &S,
    camera: &Camera2D,
    camera_tag: Option<ObjectId>,
    groups: &GroupVisibility,
    worlds: Option<&ze_scene_core::WorldTransforms>,
    queue: &mut DrawQueue,
) {
    // 内容对象的默认绘制层级：同层（保持查询顺序）。
    const Z: f32 = 0.0;
    // B3 Step 2：每帧重新收集 Surface 路由错误（显式，不静默）。
    queue.clear_surface_errors();
    // 相机 → 屏幕的仿射（唯一降级点）。
    let t = camera.to_transform2d();
    let scale = camera.scale;

    record_group::<S, Circle, StrokeSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Circle, FillSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Path, StrokeSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Path, FillSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Line, StrokeSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Rect, FillSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Rect, StrokeSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    // T2：箭头是**一等图元**，两种样式各一组（`FillSolid` ⇒ 实心箭头，`StrokeSolid` ⇒ 空心）。
    // 加在既有 7 组之后 ⇒ 同 `z` 下的绘制顺序向后追加，**不影响**已有对象之间的相对顺序
    // （`DrawQueue` 稳定排序）。
    record_group::<S, Arrow, StrokeSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
    record_group::<S, Arrow, FillSolid>(scene, &t, scale, camera_tag, groups, worlds, queue, Z, None);
}

/// **内容层录制**（B3 Step 1）：只录归属某 `Surface` 的 2D 对象。
///
/// 复用 [`record_scene_tagged_with`] 的同一套 `(图元, 样式)` 组合与唯一换算点；
/// 唯一差别是按 `Parent` 链归属过滤（**不再要求 `ContentTextureItem` 标记**
/// —— 归属本身就是唯一真源）。
///
/// `camera` 是该 Surface 的内容相机（见 `SurfaceTexture::camera_for`）。
pub fn record_scene_for_surface<S: SceneRead>(
    scene: &S,
    surface: ObjectId,
    camera: &Camera2D,
    worlds: Option<&ze_scene_core::WorldTransforms>,
    queue: &mut DrawQueue,
) {
    const Z: f32 = 0.0;
    let t = camera.to_transform2d();
    let scale = camera.scale;
    let groups = GroupVisibility::default();

    record_group::<S, Circle, StrokeSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Circle, FillSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Path, StrokeSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Path, FillSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Line, StrokeSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Rect, FillSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Rect, StrokeSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Arrow, StrokeSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
    record_group::<S, Arrow, FillSolid>(scene, &t, scale, None, &groups, worlds, queue, Z, Some(surface));
}

/// 录制**单个**路径描边实体到队列（z=0，与 [`record_scene`] 同口径）。
///
/// S3-3 内容纹理层按需录制（只录带 `ContentTextureItem` 标记的实体）时用它：
/// 位置经相机降级仿射落到屏幕空间，描边宽度经 [`ResolveStyle`] 按 `camera.scale`
/// 解析为屏幕像素——与 [`record_scene`] 完全一致，换算仍只发生在这里
/// （唯一换算点不外泄）。
pub fn record_path_stroke(
    queue: &mut DrawQueue,
    path: &Path,
    stroke: &StrokeSolid,
    camera: &Camera2D,
) {
    queue.push(DrawCommand::new(
        ZIndex::new(0),
        path.to_shape2d(&camera.to_transform2d()),
        stroke.resolve_style(camera.scale),
    ));
}

/// 录制**一条形变项**到 [`MorphQueue`]（z 与形变管线一致，见 [`MorphQueue`]）。
///
/// 为什么把它放在本 crate 而不是调用点（M4-4）：形变项的两端几何要经相机
/// 落到屏幕空间、描边宽度要经 [`ResolveStyle`] 按 `camera.scale` 解析 ——
/// 这两件事都属于**唯一换算点**纪律的范围，必须留在渲染层内，
/// 不得散到 UI 层（否则"某些描边乘过 scale、某些没乘"）。
pub fn push_morph_item(
    queue: &mut MorphQueue,
    from: &ze_notion::Path2D,
    to: &ze_notion::Path2D,
    t: f32,
    samples: u32,
    stroke: &StrokeSolid,
    opacity: Opacity,
    camera: &Camera2D,
) {
    let mut style = stroke.resolve_style(camera.scale);
    // N4：不透明度在此施加（与 `record_group` 同一函数 ⇒ 唯一的施加点）。
    apply_opacity(&mut style, opacity);
    queue.0.push(MorphItem {
        from: Path::new(from.clone()).to_screen(camera).0,
        to: Path::new(to.clone()).to_screen(camera).0,
        t,
        samples,
        style,
    });
}

/// 录制**一条书写动画项**到 [`RevealQueue`]（与形变项同层，见 [`RevealQueue`]）。
///
/// 与 [`push_morph_item`] 同一纪律：路径经相机落到**屏幕空间**、描边宽度经
/// [`ResolveStyle`] 按 `camera.scale` 解析 —— 唯一换算点不外泄（否则"某些描边
/// 乘过 scale、某些没乘"）。归一化区间 `[start, end]` 原样带过。
pub fn push_reveal_item(
    queue: &mut RevealQueue,
    path: &ze_notion::Path2D,
    start: f32,
    end: f32,
    stroke: &StrokeSolid,
    opacity: Opacity,
    camera: &Camera2D,
) {
    let mut style = stroke.resolve_style(camera.scale);
    // N4：不透明度在此施加（与 `record_group` 同一函数 ⇒ 唯一的施加点）。
    apply_opacity(&mut style, opacity);
    queue.0.push(RevealItem {
        path: Path::new(path.clone()).to_screen(camera).0,
        start,
        end,
        style,
    });
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use std::any::Any;
    use ze_notion::{MeasureSpace, Path2D};
    use ze_scene_contract::{Component, Components, ObjectId, Parent, Scene, SceneRead};
    use ze_scene_core::components::*;

    // ─── 非 ECS 的测试 Scene（M3-7 判据：`record_scene` 不依赖 hecs）──────────
    //
    // 这是契约层抽象的**第二个实现**：一个纯 `Vec` 存储的场景。它同时证明
    // "可替换"不是纸面承诺 —— `record_scene` 能被非 ECS 场景驱动。

    #[derive(Default)]
    struct TestScene {
        items: Vec<(ObjectId, Vec<Box<dyn Any + Send + Sync>>)>,
    }

    /// 让 `TestScene::spawn((A, B))` 与 hecs 的元组写法保持一致。
    trait IntoComponents {
        fn into_components(self) -> Vec<Box<dyn Any + Send + Sync>>;
    }

    macro_rules! impl_into_components {
        ($($name:ident),+) => {
            impl<$($name: Send + Sync + 'static),+> IntoComponents for ($($name,)+) {
                #[allow(non_snake_case)]
                fn into_components(self) -> Vec<Box<dyn Any + Send + Sync>> {
                    let ($($name,)+) = self;
                    vec![$(Box::new($name)),+]
                }
            }
        };
    }
    impl_into_components!(A);
    impl_into_components!(A, B);
    impl_into_components!(A, B, C);
    impl_into_components!(A, B, C, D);

    impl TestScene {
        fn new() -> Self {
            Self::default()
        }

        fn spawn(&mut self, bundle: impl IntoComponents) -> ObjectId {
            let id = ObjectId::new(self.items.len() as u32, 1);
            self.items.push((id, bundle.into_components()));
            id
        }
    }

    impl SceneRead for TestScene {
        fn contains(&self, id: ObjectId) -> bool {
            self.items.iter().any(|(i, _)| *i == id)
        }

        fn len(&self) -> usize {
            self.items.len()
        }

        fn for_each<T: 'static + Send + Sync>(&self, mut f: impl FnMut(ObjectId, &T)) {
            for (id, comps) in &self.items {
                if let Some(t) = comps.iter().find_map(|c| c.downcast_ref::<T>()) {
                    f(*id, t);
                }
            }
        }

        fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
            &self,
            mut f: impl FnMut(ObjectId, &A, &B),
        ) {
            for (id, comps) in &self.items {
                let a = comps.iter().find_map(|c| c.downcast_ref::<A>());
                let b = comps.iter().find_map(|c| c.downcast_ref::<B>());
                if let (Some(a), Some(b)) = (a, b) {
                    f(*id, a, b);
                }
            }
        }

        fn for_each_pair_opt<
            A: 'static + Send + Sync,
            B: 'static + Send + Sync,
            C: 'static + Send + Sync,
        >(
            &self,
            mut f: impl FnMut(ObjectId, &A, &B, Option<&C>),
        ) {
            for (id, comps) in &self.items {
                let a = comps.iter().find_map(|c| c.downcast_ref::<A>());
                let b = comps.iter().find_map(|c| c.downcast_ref::<B>());
                if let (Some(a), Some(b)) = (a, b) {
                    let c = comps.iter().find_map(|c| c.downcast_ref::<C>());
                    f(*id, a, b, c);
                }
            }
        }

        fn with<T: 'static + Send + Sync, R>(
            &self,
            id: ObjectId,
            f: impl FnOnce(&T) -> R,
        ) -> Option<R> {
            let comps = &self.items.iter().find(|(i, _)| *i == id)?.1;
            let t = comps.iter().find_map(|c| c.downcast_ref::<T>())?;
            Some(f(t))
        }
    }

    impl Scene for TestScene {
        fn spawn_dyn(&mut self, components: Components) -> Option<ObjectId> {
            let id = ObjectId::new(self.items.len() as u32, 1);
            // 测试场景内部仍用 `Any` 存储（它能装任意测试类型）；这里把
            // `Component` 重新擦除回 `Any`（`into_any`）。
            self.items
                .push((id, components.into_iter().map(|c| c.into_any()).collect()));
            Some(id)
        }

        fn despawn(&mut self, id: ObjectId) -> bool {
            let before = self.items.len();
            self.items.retain(|(i, _)| *i != id);
            self.items.len() != before
        }

        fn attach(&mut self, child: ObjectId, parent: Parent) -> bool {
            self.set(child, parent)
        }

        fn get_mut<T: 'static + Send + Sync>(&mut self, id: ObjectId) -> Option<&mut T> {
            self.items
                .iter_mut()
                .find(|(i, _)| *i == id)?
                .1
                .iter_mut()
                .find_map(|c| c.downcast_mut::<T>())
        }

        fn set<T: Component>(&mut self, id: ObjectId, component: T) -> bool {
            let Some(slot) = self.items.iter_mut().find(|(i, _)| *i == id) else {
                return false;
            };
            match slot
                .1
                .iter_mut()
                .find(|c| (**c).type_id() == std::any::TypeId::of::<T>())
            {
                Some(existing) => *existing = Box::new(component),
                None => slot.1.push(Box::new(component)),
            }
            true
        }
    }

    struct MockPainter;

    impl Painter2D for MockPainter {
        fn paint(&self, _: &Shape2D, _: &Style2D) {}
    }

    fn test_camera() -> Camera2D {
        Camera2D::new(
            (0.0, 0.0).into(),
            1.0,
            // 视口矩形 = 理念层 `Rect2D`（M3-2 后场景层的视口 `Rect` 已删除）。
            ze_notion::Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::new(100.0, 100.0)),
        )
    }

    #[test]
    fn test_scene_render_circle_stroke() {
        let mut world = TestScene::new();
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
        let mut world = TestScene::new();
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
        let mut world = TestScene::new();
        let path = Path::new(Path2D::from_polyline(
            &[ze_notion::Vec2::new(0.0, 0.0), ze_notion::Vec2::new(1.0, 1.0)],
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
        let world = TestScene::new();
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_mixed_entities() {
        let mut world = TestScene::new();
        world.spawn((
            Circle::new((0.0, 0.0), 10.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        world.spawn((
            Circle::new((5.0, 5.0), 20.0),
            FillSolid::new((0, 255, 0, 128)),
        ));
        let path = Path::new(Path2D::from_polyline(
            &[ze_notion::Vec2::new(0.0, 0.0), ze_notion::Vec2::new(1.0, 1.0)],
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
            let style_kind = if style.fill.is_some() {
                "fill"
            } else {
                "stroke"
            };
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
            &[ze_notion::Vec2::new(0.0, 0.0), ze_notion::Vec2::new(1.0, 0.0)],
            false,
        ))
    }
    fn mk_line(x: f32) -> Line {
        Line::new((x, 0.0), (x, 10.0))
    }
    fn mk_rect() -> Rect {
        Rect::new((0.0_f32, 0.0_f32), (10.0_f32, 10.0_f32))
    }

    #[test]
    fn test_scene_render_line_stroke() {
        let mut world = TestScene::new();
        world.spawn((mk_line(5.0), mk_stroke()));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_rect_fill_and_stroke() {
        let mut world = TestScene::new();
        world.spawn((mk_rect(), mk_fill()));
        world.spawn((mk_rect(), mk_stroke()));
        let renderer = SceneRender {
            painter: MockPainter,
        };
        renderer.render(&world, &test_camera());
    }

    #[test]
    fn test_scene_render_line_to_screen() {
        let mut world = TestScene::new();
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
            ze_notion::Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::new(100.0, 100.0)),
        )
    }

    fn queue_stroke_width(cmd: &DrawCommand) -> f32 {
        assert!(cmd.style.stroke.is_some(), "expected stroke style");
        cmd.style.stroke_width
    }

    /// `World` 描边：屏幕宽度随 camera.scale 线性变化。
    #[test]
    fn world_stroke_scales_with_camera_scale() {
        let mut world = TestScene::new();
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
        let mut world = TestScene::new();
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
        let mut world = TestScene::new();
        world.spawn((
            mk_line(1.0),
            StrokeSolid::new_in(0.01, (0, 0, 0, 255), MeasureSpace::WorldWithMinPx(1.5)),
        ));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(1.0), &mut q);
        // 0.01 * 1 = 0.01 < 1.5 → 钳到 1.5
        assert!((queue_stroke_width(&q.commands()[0]) - 1.5).abs() < 1e-6);
    }

    /// M4：`ZOrder` 决定绘制层级 —— 网格线段（-100）排在内容（0）之前。
    #[test]
    fn zorder_component_overrides_default_layer() {
        let mut world = TestScene::new();
        world.spawn((mk_line(1.0), mk_stroke()));
        world.spawn((mk_line(2.0), mk_stroke(), ZOrder::new(-100)));
        let mut q = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut q);
        q.ensure_sorted();
        let zs: Vec<i32> = q.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![-100, 0], "网格线段应压在内容之下");
    }

    #[test]
    fn test_record_scene_into_queue() {
        let mut world = TestScene::new();
        world.spawn((mk_circle(1.0), mk_fill()));
        world.spawn((mk_line(2.0), mk_stroke()));
        let mut queue = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut queue);
        assert_eq!(queue.len(), 2);
        let kinds: Vec<String> = queue
            .commands()
            .iter()
            .map(|c| {
                let style_kind = if c.style.fill.is_some() {
                    "fill"
                } else {
                    "stroke"
                };
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
        let mut world = TestScene::new();
        world.spawn((
            Path::new(Path2D::from_polyline(
                &[
                    ze_notion::Vec2::new(0.0, 0.0),
                    ze_notion::Vec2::new(1.0, 0.0),
                    ze_notion::Vec2::new(1.0, 1.0),
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
            matches!(p.commands.last(), Some(ze_notion::PathCommand::Close)),
            "闭合路径必须以 Close 结尾"
        );
    }

    /// 圆半径随 camera.scale 缩放（`Circle::to_screen` 语义经 `Shape2D` 保留）。
    #[test]
    fn circle_radius_scales_with_camera() {
        let mut world = TestScene::new();
        world.spawn((Circle::new((0.0, 0.0), 2.0), mk_stroke()));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);
        let Shape2D::Circle { radius, .. } = &q.commands()[0].shape else {
            panic!("expected Shape2D::Circle");
        };
        assert!((radius - 20.0).abs() < 1e-4, "radius = {radius}");
    }

    // ─── T1：逐对象 `Transform`（2D 摆放通道）────────────────────────────

    fn transform_at(x: f32, y: f32) -> ze_scene_core::Transform {
        ze_scene_core::Transform {
            position: ze_notion::Vec3::new(x, y, 0.0),
            ..Default::default()
        }
    }

    fn circle_center(cmd: &DrawCommand) -> ze_notion::Vec2 {
        match &cmd.shape {
            Shape2D::Circle { center, .. } => *center,
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    /// **缺席（或恒等）`Transform` ⇒ 逐位不变**：实心写入恒等摆放与完全不写摆放
    /// 得到**完全相同**的几何（证明"未使用新能力时画面逐位不变"）。
    #[test]
    fn absent_or_identity_transform_is_bit_identical() {
        let mut world = TestScene::new();
        world.spawn((mk_circle(3.0), mk_stroke()));
        world.spawn((mk_circle(3.0), mk_stroke(), ze_scene_core::Transform::default()));
        // spawn 顺序 = 查询顺序 ⇒ 两条命令一一对应。
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);
        assert_eq!(q.len(), 2);
        assert_eq!(
            q.commands()[0].shape.bounds(),
            q.commands()[1].shape.bounds()
        );
    }

    /// **T1**：摆放 `position = (1, 0)` ⇒ 几何整体右移 `1 × camera.scale` 像素。
    #[test]
    fn object_transform_translates_geometry() {
        let mut world = TestScene::new();
        world.spawn((mk_circle(0.0), mk_stroke()));
        world.spawn((mk_circle(0.0), mk_stroke(), transform_at(1.0, 0.0)));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);

        let base = circle_center(&q.commands()[0]);
        let moved = circle_center(&q.commands()[1]);
        assert!((base.x - 50.0).abs() < 1e-4, "基准圆心 = {base:?}");
        assert!(
            (moved.x - base.x - 10.0).abs() < 1e-4,
            "右移 1 世界单位 × scale 10 = 10px（{base:?} → {moved:?}）"
        );
        assert!((moved.y - base.y).abs() < 1e-6, "y 不变");
    }

    /// **T1**：摆放 `scale = 2` ⇒ 半径（覆盖像素数）增大（半径 × 2）。
    #[test]
    fn object_transform_scales_geometry() {
        let mut world = TestScene::new();
        world.spawn((mk_circle(0.0), mk_stroke()));
        world.spawn((
            mk_circle(0.0),
            mk_stroke(),
            ze_scene_core::Transform {
                scale: ze_notion::Vec3::splat(2.0),
                ..Default::default()
            },
        ));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);

        let radius = |cmd: &DrawCommand| match &cmd.shape {
            Shape2D::Circle { radius, .. } => *radius,
            other => panic!("expected Circle, got {other:?}"),
        };
        let (r0, r1) = (radius(&q.commands()[0]), radius(&q.commands()[1]));
        assert!((r1 - r0 * 2.0).abs() < 1e-4, "{r0} × 2 ≠ {r1}");
    }

    /// **T1**：摆放绕 Z 旋转 90° ⇒ 端点位置按旋转后落点（可证伪：顺序错则落到别处）。
    #[test]
    fn object_transform_rotates_geometry_about_z() {
        let mut world = TestScene::new();
        world.spawn((Line::new((0.0, 0.0), (1.0, 0.0)), mk_stroke()));
        world.spawn((
            Line::new((0.0, 0.0), (1.0, 0.0)),
            mk_stroke(),
            ze_scene_core::Transform {
                rotation: ze_notion::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                ..Default::default()
            },
        ));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);

        let Shape2D::Line { start: s0, end: e0 } = &q.commands()[0].shape else {
            panic!("expected Line");
        };
        // 基准（未摆放）：世界 (0,0)→(1,0) ⇒ 屏幕 (50,50)→(60,50)。
        assert_eq!(
            (*s0, *e0),
            (ze_notion::Vec2::new(50.0, 50.0), ze_notion::Vec2::new(60.0, 50.0))
        );

        let Shape2D::Line { start: s1, end: e1 } = &q.commands()[1].shape else {
            panic!("expected Line");
        };
        // 旋转 90°：世界 (1,0) → (0,1) ⇒ 屏幕 (50, 40)（Y-down 翻转）。
        assert!(
            (s1.x - 50.0).abs() < 1e-4 && (s1.y - 50.0).abs() < 1e-4,
            "start = {s1:?}"
        );
        assert!(
            (e1.x - 50.0).abs() < 1e-4 && (e1.y - 40.0).abs() < 1e-4,
            "旋转后终点应为 (50, 40)，实际 {e1:?}"
        );
    }

    // ─── T2：箭头（一等图元，`Group` 降解）─────────────────────────────────

    /// **T2**：`Arrow` 两种样式都进队列；均降解为 `Group { [杆, 头] }`（2 个多边形）。
    #[test]
    fn arrow_records_as_group_in_both_styles() {
        let a = Arrow::new((0.0, 0.0), (1.0, 0.0), 0.2, 0.2);
        let mut world = TestScene::new();
        // 组顺序：`record_scene_tagged` 先 (Arrow, StrokeSolid) 后 (Arrow, FillSolid)。
        world.spawn((a, mk_stroke()));
        world.spawn((a, mk_fill()));
        let mut q = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut q);
        assert_eq!(q.len(), 2, "描边 + 填充各一条");

        for (i, cmd) in q.commands().iter().enumerate() {
            match &cmd.shape {
                Shape2D::Group { children } => {
                    assert_eq!(children.len(), 2, "第 {i} 条：杆 + 头");
                    for c in children {
                        assert!(
                            matches!(c, Shape2D::Polygon { .. }),
                            "子图元必须是闭合多边形"
                        );
                    }
                }
                other => panic!("第 {i} 条应为 Group，实际 {other:?}"),
            }
        }
        assert!(q.commands()[0].style.fill.is_none(), "第一条是描边箭头");
        assert!(q.commands()[1].style.stroke.is_none(), "第二条是实心箭头");
    }

    /// **T2**：方向可证伪 —— 把 `from` / `to` 对调后，头部像素/几何分布镜像。
    #[test]
    fn arrow_direction_is_mirrored_when_reversed() {
        let mut world = TestScene::new();
        world.spawn((Arrow::new((0.0, 0.0), (1.0, 0.0), 0.2, 0.2), mk_fill()));
        world.spawn((Arrow::new((1.0, 0.0), (0.0, 0.0), 0.2, 0.2), mk_fill()));
        let mut q = DrawQueue::new();
        record_scene(&world, &test_camera(), &mut q);

        let head_tip_x = |cmd: &DrawCommand| match &cmd.shape {
            Shape2D::Group { children } => match &children[1] {
                // 头的第二个顶点就是箭尖 `to`。
                Shape2D::Polygon { points } => points[1].x,
                other => panic!("expected head polygon, got {other:?}"),
            },
            other => panic!("expected Group, got {other:?}"),
        };
        // 世界 x = 1 → 屏幕 51（test_camera scale = 1）；世界 x = 0 → 屏幕 50。
        assert!((head_tip_x(&q.commands()[0]) - 51.0).abs() < 1e-4);
        assert!((head_tip_x(&q.commands()[1]) - 50.0).abs() < 1e-4);
    }

    /// `Arrow` 也受 T1 的逐对象 `Transform` 影响（两个能力可组合）。
    #[test]
    fn arrow_honors_object_transform() {
        let a = Arrow::new((0.0, 0.0), (1.0, 0.0), 0.2, 0.2);
        let mut world = TestScene::new();
        world.spawn((a, mk_fill()));
        world.spawn((a, mk_fill(), transform_at(1.0, 0.0)));
        let mut q = DrawQueue::new();
        record_scene(&world, &cam_with_scale(10.0), &mut q);
        let bounds = |cmd: &DrawCommand| cmd.shape.bounds().unwrap();
        let (b0, b1) = (bounds(&q.commands()[0]), bounds(&q.commands()[1]));
        assert!(
            (b1.min.x - b0.min.x - 10.0).abs() < 1e-4,
            "Transform 应把整条箭头右移 10px（{b0:?} → {b1:?}）"
        );
    }

    /// 形变端点的**闭合口径**（实测修）：两端都开口 ⇒ 不得补弦。
    ///
    /// 早期版本恒按闭合环重采样，开口曲线（分镜的 `y = sin x`）会因此多出一条
    /// **首尾相接的弦** —— 在画面上就是 x 轴上一条额外的直线。
    #[test]
    fn open_morph_endpoints_stay_open_without_chord() {
        let pts: Vec<ze_notion::Vec2> = (0..=16)
            .map(|i| {
                let x = -1.0 + (i as f32) * 0.125;
                ze_notion::Vec2::new(x, x.sin())
            })
            .collect();
        let item = MorphItem {
            from: Path2D::from_polyline(&[pts[0], pts[0]], false),
            to: Path2D::from_polyline(&pts, false),
            t: 1.0,
            samples: 32,
            style: Style2D::default(),
        };
        let (from, to) = item.sampled_endpoints();
        for (name, p) in [("from", &from), ("to", &to)] {
            assert_eq!(p.flatten(0.0).len(), 32, "{name} 重采样点数应恒为 samples");
            assert!(
                !matches!(p.commands.last(), Some(ze_notion::PathCommand::Close)),
                "{name} 应保持开口（否则首尾会多出一条弦）"
            );
        }
    }

    /// 反向：两端都闭合（如 S1a 的圆→方）⇒ 仍闭合，行为不变。
    #[test]
    fn closed_morph_endpoints_stay_closed() {
        let ring = |r: f32| {
            let pts: Vec<ze_notion::Vec2> = (0..16)
                .map(|i| {
                    let a = std::f32::consts::TAU * (i as f32) / 16.0;
                    ze_notion::Vec2::new(r * a.cos(), r * a.sin())
                })
                .collect();
            Path2D::from_polyline(&pts, true)
        };
        let item = MorphItem {
            from: ring(1.0),
            to: ring(2.0),
            t: 0.5,
            samples: 32,
            style: Style2D::default(),
        };
        let (from, to) = item.sampled_endpoints();
        for (name, p) in [("from", &from), ("to", &to)] {
            assert!(
                matches!(p.commands.last(), Some(ze_notion::PathCommand::Close)),
                "{name} 应保持闭合"
            );
        }
    }

    // ─── B2-1：统一队列 + 统一 ZOrder（R-F 显式锁定）───────────────────────

    /// 🔴 **R-F**：同 `ZOrder` 内**必须保持插入序**（稳定排序）。
    ///
    /// 这条保证原先是隐式的（`cmp` 只比 z + `Vec::sort` 稳定）。B2-1 重构队列后
    /// 把它写成显式测试：任何 `sort_unstable_*` 的实现都会让本用例变红。
    #[test]
    fn test_queue_stable_within_equal_zorder() {
        let mut q = DrawQueue::new();
        // 交错插入：z=5 三条、z=1 两条，各自保持相对顺序
        q.circle_stroke(ZIndex::new(5), mk_circle(1.0), mk_stroke()); // A
        q.circle_fill(ZIndex::new(1), mk_circle(2.0), mk_fill()); // B
        q.path_stroke(ZIndex::new(5), mk_path(), mk_stroke()); // C
        q.line_stroke(ZIndex::new(1), mk_line(3.0), mk_stroke()); // D
        q.circle_fill(ZIndex::new(5), mk_circle(4.0), mk_fill()); // E

        q.ensure_sorted();
        let kinds: Vec<&str> = q
            .commands()
            .iter()
            .map(|c| {
                if c.style.fill.is_some() {
                    "fill"
                } else {
                    "stroke"
                }
            })
            .collect();
        // z=1 的两条（B 填充、D 描边）保持插入序；z=5 的三条（A 描边、C 描边、E 填充）
        // 保持插入序 —— 若排序不稳定，同 z 组内顺序可能被重排。
        assert_eq!(kinds, vec!["fill", "stroke", "stroke", "stroke", "fill"]);
        let zs: Vec<i32> = q.commands().iter().map(|c| c.z.0).collect();
        assert_eq!(zs, vec![1, 1, 5, 5, 5]);

        // 幂等：再排一次结果不变（同 z 序不被二次扰动）
        let first: Vec<i32> = q
            .commands()
            .iter()
            .map(|c| match &c.shape {
                Shape2D::Circle { radius, .. } => *radius as i32,
                _ => -1,
            })
            .collect();
        q.ensure_sorted();
        let second: Vec<i32> = q
            .commands()
            .iter()
            .map(|c| match &c.shape {
                Shape2D::Circle { radius, .. } => *radius as i32,
                _ => -1,
            })
            .collect();
        assert_eq!(first, second, "重复排序必须幂等");
    }

    /// B2-1：2D / 3D 命令共处一个队列，共享同一 `ZOrder` 排序。
    #[test]
    fn test_unified_queue_mixes_2d_and_3d_by_zorder() {
        use std::sync::Arc;

        use ze_scene_core::components::Material3D;
        use ze_scene_core::mesh_primitives::wireframe_cube;

        let mut q = DrawQueue::new();
        q.draw_line(
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::X,
            ze_notion::Color32::RED,
        ); // z=0
        q.circle_fill(ZIndex::new(-1), mk_circle(1.0), mk_fill()); // z=-1
        q.draw_mesh(
            Arc::new(wireframe_cube(1.0)),
            ze_notion::Mat4::IDENTITY,
            Material3D::color(ze_notion::Color32::WHITE),
        ); // z=0

        assert_eq!(q.len(), 3, "一个队列容纳 2D 与 3D");
        assert_eq!(q.commands().len(), 1, "2D 视图只看到 Shape");
        assert_eq!(q.mesh_count(), 1);
        assert_eq!(q.line_count(), 1);

        q.ensure_sorted();
        let zs: Vec<i32> = q.items().iter().map(|i| i.zorder().0).collect();
        assert_eq!(zs, vec![-1, 0, 0], "混合队列按 ZOrder 升序");
        // 同 z（0）内：先插入的 line 在 mesh 之前
        match (&q.items()[1], &q.items()[2]) {
            (DrawItem::Line(_), DrawItem::Mesh(_)) => {}
            other => panic!("同 z 内应保持插入序，实际 {other:?}"),
        }
    }

    /// B2-1：3D 录制只清 3D 项，不擦掉同帧已录的 2D 形状。
    #[test]
    fn test_clear_meshes_and_lines_keeps_shapes() {
        use std::sync::Arc;

        use ze_scene_core::components::Material3D;
        use ze_scene_core::mesh_primitives::wireframe_cube;

        let mut q = DrawQueue::new();
        q.circle_fill(ZIndex::new(0), mk_circle(1.0), mk_fill());
        q.draw_mesh(
            Arc::new(wireframe_cube(1.0)),
            ze_notion::Mat4::IDENTITY,
            Material3D::color(ze_notion::Color32::WHITE),
        );
        q.clear_meshes_and_lines();
        assert_eq!(q.commands().len(), 1);
        assert_eq!(q.mesh_count(), 0);
    }
}
