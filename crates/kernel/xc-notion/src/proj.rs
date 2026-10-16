// proj.rs — 相机投影/视图辅助（纯数学）。
//
// 收口动机（M1 T6②）：`xcal-scene::scene::components::3d` 原本直接
// `use glam::camera::rh::proj::directx;` / `use glam::camera::rh::view::look_at_mat4;`，
// 让场景层直接摸 glam 的相机子模块。这两个是**纯数学投影/视图函数**，属理念层，
// 故在此转发再导出，由理念层统一充当 glam 的唯一出口。
//
// 边界（用户已定）：**只收这两个函数，不收 `Camera3D` / `Projection`** ——
// 相机本身是场景对象，归场景对象层（M3）；本模块只有矩阵构造函数。
//
// 约定：右手系 + wgpu/D3D NDC（深度 [0, 1]、Y-up），与 `xcal-scene` 的
// `Projection::Perspective/Orthographic` 文档一致。

/// 投影矩阵构造（右手系 + [0, 1] 深度 + Y-up；即 wgpu / DirectX 约定）。
pub use glam::camera::rh::proj::directx;
/// 视图矩阵：右手 look-at（世界空间 → 相机空间）。
pub use glam::camera::rh::view::look_at_mat4;
