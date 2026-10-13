// xcal-geometry: 纯几何数据层
//
// 依赖方向：xcal-geometry（仅 glam）← xcal-render ← xcal-render-backends
//
// 本 crate 只包含数据结构与纯函数算法：
//   - vec / mat：glam 向量/矩阵类型的直接再导出
//   - color：Color32（RGBA u8）
//   - rect：Rect2D（轴对齐矩形 + 相交/包含）
//   - path：Path2D / PathCommand（2D 路径 + 折线采样 flatten）
//   - transform：Transform2D / Transform3D（仿射变换包装）
//   - primitives：Line3D / Point3D / Triangle2D 基础图元数据
//
// 约束：不依赖 egui / wgpu / hecs，不定义 trait object。

pub mod color;
pub mod lerp;
pub mod mat;
pub mod path;
pub mod primitives;
pub mod rect;
pub mod stroke;
pub mod transform;
pub mod vec;

pub use color::Color32;
pub use lerp::Lerp;
pub use mat::{Mat3, Mat4, Quat};
pub use path::{DEFAULT_MORPH_SAMPLES, MORPH_TOLERANCE, Path2D, PathCommand};
pub use primitives::{Line3D, Point3D, Triangle2D};
pub use rect::Rect2D;
pub use stroke::Stroke2D;
pub use transform::{Transform2D, Transform3D};
pub use vec::{Vec2, Vec3, Vec4};
