// shape.rs
//
// 2D 形状定义（渲染 IR）：基于 ze-notion 的纯数据描述。
//
// 设计要点：
// - Shape2D 是"逻辑图元"枚举，后端可选择光栅化路径（triangulate / flatten）；
// - Shape2D::Custom 是开放扩展点：宿主（zela-egui / 插件）通过**可寻址的 SpecId**
//   引用一个具名几何规格，使 render 层无需为每个宿主图元增加变体即可统一处理
//   裁剪 / 命中测试 / 离屏后端；
// - 本模块不依赖 wgpu/egui，纯几何 + 可序列化的规格标识。
//
// M2：本文件自 `zela-render/src/shape.rs` 迁入（渲染 IR 层）。
// ZSB：`CustomShapeData` 不再是"函数指针 + TypeId"，而是 `SpecId + bounds` ——
// 图元必须可序列化，且不得包含匿名闭包（见 D1/D2）。
//
// ⚠️ 变体语义**冻结**：`Circle { center, radius }` 等扁平字段形态是后端
// （wgpu_2d / tessellate）源级依赖的契约，改名 / 改形会连带改后端源码 —— 禁止。

use std::borrow::Cow;

use ze_notion::{Color32, MeasureSpace, Path2D, Rect2D, Transform2D, Triangle2D, Vec2};

/// 自定义图元的**稳定规格标识**。
///
/// 为什么不是注册序号：编译器与回放端是两个独立二进制；若用序号，两侧注册顺序
/// 稍有不同，同一个 ID 就会指向不同函数 ⇒ 画面错乱且不报错。
///
/// 构造：`SpecId::new("my.glyph")`（编译期常量）或 `SpecId::from_name(...)`
/// （反序列化 / 动态来源）。序列化只写名字。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpecId(Cow<'static, str>);

impl SpecId {
    /// 编译期常量规格名（推荐）。
    pub const fn new(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// 运行期来源（反序列化等）。
    pub fn from_name(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl std::fmt::Display for SpecId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for SpecId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for SpecId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(name))
    }
}

/// 一个自定义图元的**具名几何规格**（注册表条目）。
///
/// 这是函数指针唯一的合法归宿：它们住在**具名常量表**里，而不是每个图元实例里。
/// 编译器与回放端各自链接同一张表 ⇒ 同名即同函数。
pub struct CustomSpec {
    /// 稳定规格名。
    pub id: SpecId,
    /// 展开为路径。
    pub to_path: Option<fn(&CustomShapeData, transform: &Transform2D) -> Path2D>,
    /// 直接三角化（坐标已应用 transform）。
    pub to_triangles: Option<fn(&CustomShapeData, transform: &Transform2D) -> Vec<Triangle2D>>,
}

impl CustomSpec {
    /// 空规格（两种降级都未提供 ⇒ 解析时明确报错）。
    pub const fn new(id: &'static str) -> Self {
        Self {
            id: SpecId::new(id),
            to_path: None,
            to_triangles: None,
        }
    }

    pub const fn with_path(
        mut self,
        f: fn(&CustomShapeData, &Transform2D) -> Path2D,
    ) -> Self {
        self.to_path = Some(f);
        self
    }

    pub const fn with_triangles(
        mut self,
        f: fn(&CustomShapeData, &Transform2D) -> Vec<Triangle2D>,
    ) -> Self {
        self.to_triangles = Some(f);
        self
    }

    /// 该规格是否至少提供一种几何降级。
    pub const fn has_geometry(&self) -> bool {
        self.to_path.is_some() || self.to_triangles.is_some()
    }
}

impl std::fmt::Debug for CustomSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSpec")
            .field("id", &self.id)
            .field("to_path", &self.to_path.is_some())
            .field("to_triangles", &self.to_triangles.is_some())
            .finish()
    }
}

/// **注册表**：编译期生成的具名规格表（`static SPECS: &[CustomSpec]`）。
#[derive(Debug, Clone, Copy, Default)]
pub struct CustomSpecs {
    specs: &'static [CustomSpec],
}

impl CustomSpecs {
    /// 空表（没有自定义图元的运行时）。
    pub const EMPTY: Self = Self { specs: &[] };

    pub const fn new(specs: &'static [CustomSpec]) -> Self {
        Self { specs }
    }

    /// 按名字精确查找（**不做前缀 / 模糊匹配**）。
    pub fn get(&self, id: &SpecId) -> Option<&'static CustomSpec> {
        self.specs.iter().find(|spec| spec.id == *id)
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }
}

/// 自定义图元的几何降级数据。
///
/// 只携带**可序列化**的载荷：规格名 + 包围盒。几何函数在
/// [`CustomSpec`] 注册表里按名字查找。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomShapeData {
    /// 具名几何规格。
    pub spec: SpecId,
    /// 逻辑包围盒（用于裁剪 / 排序 / 视锥剔除）。
    pub bounds: Rect2D,
}

impl CustomShapeData {
    /// 创建自定义图元降级数据（bounds 为必填）。
    pub fn new(spec: impl Into<SpecId>, bounds: Rect2D) -> Self {
        Self {
            spec: spec.into(),
            bounds,
        }
    }
}

impl From<&'static str> for SpecId {
    fn from(name: &'static str) -> Self {
        Self::new(name)
    }
}

impl From<String> for SpecId {
    fn from(name: String) -> Self {
        Self::from_name(name)
    }
}

/// 2D 绘制样式（新管线：描边 + 填充）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style2D {
    /// 填充色（None = 不填充）。
    pub fill: Option<Color32>,
    /// 描边色（None = 不描边）。
    pub stroke: Option<Color32>,
    /// 描边宽度（单位由 [`Style2D::space`] 声明；默认屏幕像素）。
    pub stroke_width: f32,
    /// 描边宽度所属空间（声明式）。IR 转换层（`content_ir`）在此声明意图，
    /// 由消费端把它转成 `StrokeSolid.space`，最终在 `record_scene`
    /// （**唯一换算点**）结合相机 scale 解析为屏幕像素。
    pub space: MeasureSpace,
    /// 描边形态（cap / join）：**已解析**的载荷，后端唯一消费点（B5 V2-10）。
    /// 默认 = butt + miter（= B5 之前 lyon 的隐式默认 ⇒ 逐位不变）。
    pub stroke_shape: crate::style::StrokeShape,
}

impl Default for Style2D {
    fn default() -> Self {
        Self {
            fill: Some(Color32::WHITE),
            stroke: None,
            stroke_width: 1.0,
            space: MeasureSpace::Screen,
            stroke_shape: crate::style::StrokeShape::DEFAULT,
        }
    }
}

/// 2D 逻辑图元（**可序列化**：ZSB 产物的硬前提）。
///
/// 所有坐标均为逻辑坐标（本地空间），由渲染层的 transform 统一映射。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Shape2D {
    /// 空图元（不产生任何绘制；用于占位/布局）。
    Empty,
    /// 线段：起点 → 终点。
    Line { start: Vec2, end: Vec2 },
    /// 折线：点序列（不闭合）。
    Polyline { points: Vec<Vec2> },
    /// 路径（含贝塞尔/弧线命令，后端按需 flatten）。
    Path(Path2D),
    /// 圆：圆心 + 半径。
    Circle { center: Vec2, radius: f32 },
    /// 椭圆：中心 + 两轴半径。
    Ellipse { center: Vec2, rx: f32, ry: f32 },
    /// 轴对齐矩形。
    Rect(Rect2D),
    /// 多边形（闭合点序列）。
    Polygon { points: Vec<Vec2> },
    /// 图元分组（子图元列表；transform 作用于整组）。
    Group { children: Vec<Shape2D> },
    /// 宿主自定义图元（按 [`SpecId`] 查注册表）。
    Custom(CustomShapeData),
}

impl Shape2D {
    /// 结构指纹 / 诊断用的**稳定变体名**。
    ///
    /// 本匹配在定义 crate 内**穷举**（无 `_` 兜底）：新增 `Shape2D` 变体必须在这里
    /// 显式归类，否则**编译失败**。下游（如结构层对拍）只调用本方法，不自己 match ——
    /// 因为 `Shape2D` 是 `#[non_exhaustive]`，跨 crate 无法真正穷举。
    pub fn kind_name(&self) -> &'static str {
        match self {
            Shape2D::Empty => "empty",
            Shape2D::Line { .. } => "line",
            Shape2D::Polyline { .. } => "polyline",
            Shape2D::Path(_) => "path",
            Shape2D::Circle { .. } => "circle",
            Shape2D::Ellipse { .. } => "ellipse",
            Shape2D::Rect(_) => "rect",
            Shape2D::Polygon { .. } => "polygon",
            Shape2D::Group { .. } => "group",
            Shape2D::Custom(_) => "custom",
        }
    }

    /// 图元的逻辑包围盒（None 表示无几何/空图元）。
    ///
    /// Custom 图元直接使用其声明的 bounds；Circle/Ellipse 按参数计算。
    pub fn bounds(&self) -> Option<Rect2D> {
        match self {
            Shape2D::Empty => None,
            Shape2D::Line { start, end } => Some(Rect2D {
                min: Vec2::new(start.x.min(end.x), start.y.min(end.y)),
                max: Vec2::new(start.x.max(end.x), start.y.max(end.y)),
            }),
            Shape2D::Polyline { points } | Shape2D::Polygon { points } => bounds_of_points(points),
            Shape2D::Path(path) => path.compute_bounds(),
            // from_center 接收半尺寸：圆/椭圆包围盒的半尺寸即半径。
            Shape2D::Circle { center, radius } => {
                Some(Rect2D::from_center(*center, Vec2::splat(*radius)))
            }
            Shape2D::Ellipse { center, rx, ry } => {
                Some(Rect2D::from_center(*center, Vec2::new(*rx, *ry)))
            }
            Shape2D::Rect(rect) => Some(*rect),
            Shape2D::Group { children } => {
                let mut acc: Option<Rect2D> = None;
                for child in children {
                    acc = match (acc, child.bounds()) {
                        (Some(a), Some(b)) => Some(a.union(&b)),
                        (a, None) => a,
                        (None, b) => b,
                    };
                }
                acc
            }
            Shape2D::Custom(data) => Some(data.bounds),
        }
    }
}

fn bounds_of_points(points: &[Vec2]) -> Option<Rect2D> {
    let mut iter = points.iter();
    let first = *iter.next()?;
    let (mut min, mut max) = (first, first);
    for p in iter {
        min = min.min(*p);
        max = max.max(*p);
    }
    Some(Rect2D { min, max })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_bounds() {
        let s = Shape2D::Line {
            start: Vec2::new(1.0, 5.0),
            end: Vec2::new(4.0, 2.0),
        };
        let b = s.bounds().unwrap();
        assert_eq!(b.min, Vec2::new(1.0, 2.0));
        assert_eq!(b.max, Vec2::new(4.0, 5.0));
    }

    #[test]
    fn test_circle_bounds() {
        let s = Shape2D::Circle {
            center: Vec2::new(10.0, 20.0),
            radius: 3.0,
        };
        let b = s.bounds().unwrap();
        assert_eq!(b.center(), Vec2::new(10.0, 20.0));
        assert_eq!(b.size(), Vec2::new(6.0, 6.0));
    }

    #[test]
    fn test_empty_and_group_bounds() {
        assert_eq!(Shape2D::Empty.bounds(), None);

        let g = Shape2D::Group {
            children: vec![
                Shape2D::Rect(Rect2D::from_min_size(Vec2::ZERO, Vec2::new(2.0, 2.0))),
                Shape2D::Empty,
                Shape2D::Rect(Rect2D::from_min_size(
                    Vec2::new(1.0, 1.0),
                    Vec2::new(3.0, 3.0),
                )),
            ],
        };
        let b = g.bounds().unwrap();
        assert_eq!(b.min, Vec2::new(0.0, 0.0));
        assert_eq!(b.max, Vec2::new(4.0, 4.0));
    }

    #[test]
    fn test_custom_shape_bounds_and_debug() {
        let data = CustomShapeData::new(
            "test.custom",
            Rect2D::from_min_size(Vec2::new(0.0, 0.0), Vec2::new(5.0, 5.0)),
        );
        let shape = Shape2D::Custom(data);
        let b = shape.bounds().unwrap();
        assert_eq!(b.size(), Vec2::new(5.0, 5.0));
        let dbg = format!("{shape:?}");
        assert!(dbg.contains("test.custom"));
    }

    /// ZSB D1：`Shape2D` 全部变体可序列化，且 Custom 只写规格名。
    #[test]
    fn shape2d_is_serializable_and_custom_writes_spec_name() {
        let shape = Shape2D::Custom(CustomShapeData::new(
            "test.glyph",
            Rect2D::from_min_size(Vec2::ZERO, Vec2::new(1.0, 1.0)),
        ));
        let json = serde_json::to_string(&shape).expect("Shape2D 可序列化");
        assert!(json.contains("test.glyph"), "json = {json}");
        let back: Shape2D = serde_json::from_str(&json).expect("可反序列化");
        assert_eq!(back, shape);
    }

    /// SpecId 按名字查表：未注册 ⇒ `None`（调用方必须显式报错）。
    #[test]
    fn custom_specs_lookup_is_exact() {
        static SPECS: &[CustomSpec] = &[CustomSpec::new("a.b").with_path(|_, t| {
            let mut p = Path2D::new();
            p.move_to(t.apply_point(Vec2::ZERO));
            p
        })];
        let specs = CustomSpecs::new(SPECS);
        assert!(specs.get(&SpecId::new("a.b")).is_some());
        assert!(specs.get(&SpecId::new("a.bc")).is_none());
        assert!(CustomSpecs::EMPTY.get(&SpecId::new("a.b")).is_none());
    }
}
