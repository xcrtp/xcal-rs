// primitives.rs
//
//! 内置 3D 图元生成器。

pub mod cube;
pub mod parametric_surface;

pub use cube::{solid_cube, wireframe_cube};
pub use parametric_surface::{SurfaceSpec, parametric_surface};
