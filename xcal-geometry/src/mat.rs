// mat.rs
//
// 矩阵/四元数类型：直接再导出 glam 类型。
// - Mat3：2D 仿射变换（3x3，见 Transform2D）
// - Mat4：3D 仿射/投影变换（见 Transform3D）
// - Quat：3D 旋转

pub use glam::{Mat3, Mat4, Quat};
