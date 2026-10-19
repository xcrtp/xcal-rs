// ze-notion — 理念层（数学元语）
//
// 全仓唯一的数学元语出口：**glam 只在此 crate 被理念层包装/再导出**，
// 上层不再各自"摸 glam"造本地类型（M1 收口目标）。
//
// 依赖方向：ze-notion（仅 glam）← 其余所有 crate。
//
// 模块归属：
//   vec / mat        glam 再导出：Vec2/3/4、U8Vec4/I8Vec4、Mat3/Mat4/Quat
//   color            Color32
//   rect             Rect2D
//   path             Path2D / Path3D（精确边界 + morph / resample / flatten）
//   primitives       Point3D / Line3D / Triangle2D / Circle2D / Ellipse2D
//                    / Line2D / Polyline2D / Polygon2D
//   transform        Transform2D / Transform3D（数学仿射变换值）
//   lerp             Lerp（f32 / Vec2 / Vec3 / Color32 / Path2D）
//   func / curve / sample
//                    FunctionX2Y / ParameterCurveT2XY / ParameterCurveT2XYZ
//                    / Sampler1D / Sampler2D / Interpolator / SurfaceFn / DynFn
//   proj             camera 投影（directx::perspective / orthographic、look_at_mat4）
//   —— 以下两项暂留，注明原因 ——
//   stroke           Stroke2D（被 ze-content 依赖，属 kernel，进不了 render-ir）
//   space            MeasureSpace（呈现层度量，同上）
//
// 约束（本阶段核心判据，见验收 `cargo tree -p ze-notion`）：
//   - 只依赖 glam（+ 可选 serde）
//   - 内不得出现 ECS / hecs / GPU / wgpu / egui / UI
//   - 内不得出现离散化产物（MeshGeometry / indices / topology 那类面向 GPU 的组织方式）
//
// 同名不同物（易踩坑，见 M1 T6②）：本 crate 的 `Transform2D` / `Transform3D` 是
// **数学仿射变换值**；`zela-scene` 的 `Transform { position, rotation, scale }` 是
// **摆放组件**（M3 归场景对象层）。两者不可混用。

pub mod color;
pub mod curve;
pub mod func;
pub mod lerp;
pub mod mat;
pub mod path;
pub mod primitives;
pub mod proj;
pub mod rect;
pub mod sample;
pub mod space;
pub mod stroke;
pub mod transform;
pub mod vec;

pub use color::Color32;
pub use curve::{ParameterCurveT2XY, ParameterCurveT2XYZ};
pub use func::{DynFn, FunctionX2Y, SurfaceFn};
pub use lerp::Lerp;
pub use mat::{Mat3, Mat4, Quat};
pub use path::{DEFAULT_MORPH_SAMPLES, MORPH_TOLERANCE, Path2D, Path3D, PathCommand};
pub use primitives::{
    ARROW_SHAFT_WIDTH_RATIO, Arrow2D, Circle2D, Ellipse2D, Line2D, Line3D, Point3D, Polygon2D,
    Polyline2D, Triangle2D,
};
pub use rect::Rect2D;
pub use sample::{Interpolator, Sampler1D, Sampler2D};
pub use space::{MeasureSpace, resolve_width};
pub use stroke::Stroke2D;
pub use transform::{Transform2D, Transform3D};
pub use vec::{I8Vec4, U8Vec4, Vec2, Vec3, Vec4};
