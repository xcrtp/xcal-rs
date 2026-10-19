// primitives.rs
//
//! 内置 3D 图元生成器。
//!
//! M1 T5：参数曲面的数学定义（`SurfaceFn`）在理念层 `ze-notion`，此处只有
//! **生成器**（产出三角化结果，属离散产物，不进理念层）。

pub mod cube;
pub mod parametric_surface;

pub use cube::{solid_cube, wireframe_cube};
pub use parametric_surface::parametric_surface;
