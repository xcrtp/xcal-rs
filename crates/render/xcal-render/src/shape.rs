// shape.rs
//
// 2D 形状定义（新渲染管线）：基于 xc-notion 的纯数据描述。
//
// 设计要点：
// - Shape2D 是"逻辑图元"枚举，后端可选择光栅化路径（triangulate / flatten）；
// - Shape2D::Custom 是开放扩展点：宿主（xcal-egui / 插件）可携带自己的
//   类型标识与几何降级函数（to_path / to_triangles），使 render 层无需
//   为每个宿主图元增加变体即可统一处理裁剪/命中测试/离屏后端；
// - 本模块不依赖 wgpu/egui，纯几何 + 函数指针。

use xc_notion::{Color32, MeasureSpace, Path2D, Rect2D, Transform2D, Triangle2D, Vec2};

/// 自定义图元携带的类型标识（用于调试 / 宿主侧反查）。
pub type CustomTypeId = std::any::TypeId;

/// 自定义图元的几何降级数据。
///
/// 后端不认识的宿主图元（如节点编辑器的贝塞尔连接、图表符号）可通过
/// 此结构向 render 层提供两种光栅化降级：
/// - `to_path`：把图元展开为 [`Path2D`]（描边/填充后端通用）；
/// - `to_triangles`：直接提供三角化结果（宿主已预三角化时可省一次展平）。
#[derive(Clone)]
pub struct CustomShapeData {
    /// 宿主侧类型标识。
    pub type_id: CustomTypeId,
    /// 逻辑包围盒（用于裁剪 / 排序 / 视锥剔除）。
    pub bounds: Rect2D,
    /// 展开为路径（可选）。函数指针而非闭包：保证 Send + Sync + 'static，
    /// 避免为每个图元做 boxed closure 分配。
    pub to_path: Option<fn(&CustomShapeData, transform: &Transform2D) -> Path2D>,
    /// 展开为三角形（可选；坐标已应用 transform）。
    pub to_triangles: Option<fn(&CustomShapeData, transform: &Transform2D) -> Vec<Triangle2D>>,
}

impl std::fmt::Debug for CustomShapeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomShapeData")
            .field("type_id", &self.type_id)
            .field("bounds", &self.bounds)
            .field("to_path", &self.to_path.is_some())
            .field("to_triangles", &self.to_triangles.is_some())
            .finish()
    }
}

impl CustomShapeData {
    /// 创建自定义图元降级数据（bounds 为必填，几何函数可选）。
    pub fn new(type_id: CustomTypeId, bounds: Rect2D) -> Self {
        Self {
            type_id,
            bounds,
            to_path: None,
            to_triangles: None,
        }
    }

    /// 设置路径展开函数。
    pub fn with_to_path(mut self, f: fn(&CustomShapeData, &Transform2D) -> Path2D) -> Self {
        self.to_path = Some(f);
        self
    }

    /// 设置三角化函数。
    pub fn with_to_triangles(
        mut self,
        f: fn(&CustomShapeData, &Transform2D) -> Vec<Triangle2D>,
    ) -> Self {
        self.to_triangles = Some(f);
        self
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
    /// （**唯一换算点**）结合 `camera.scale` 解析为屏幕像素。
    pub space: MeasureSpace,
}

impl Default for Style2D {
    fn default() -> Self {
        Self {
            fill: Some(Color32::WHITE),
            stroke: None,
            stroke_width: 1.0,
            space: MeasureSpace::Screen,
        }
    }
}

/// 2D 逻辑图元。
///
/// 所有坐标均为逻辑坐标（本地空间），由渲染层的 transform 统一映射。
#[derive(Debug, Clone)]
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
    /// 宿主自定义图元（通过 [`CustomShapeData`] 提供几何降级）。
    Custom(CustomShapeData),
}

impl Shape2D {
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
            std::any::TypeId::of::<u32>(),
            Rect2D::from_min_size(Vec2::new(0.0, 0.0), Vec2::new(5.0, 5.0)),
        )
        .with_to_path(|_, t| {
            let mut p = Path2D::new();
            p.move_to(t.apply_point(Vec2::ZERO));
            p
        });
        let shape = Shape2D::Custom(data);
        let b = shape.bounds().unwrap();
        assert_eq!(b.size(), Vec2::new(5.0, 5.0));
        let dbg = format!("{shape:?}");
        assert!(dbg.contains("Custom"));
    }
}
