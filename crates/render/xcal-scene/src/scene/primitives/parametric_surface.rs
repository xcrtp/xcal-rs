// parametric_surface.rs
//
// 参数曲面生成器：把 `z = f(x, y)` 形式的标量函数离散成规则三角网格。
//
// 与 `cube.rs` 的差异：本模块产出**真实 UV**（参数域归一化）与**法线**，
// 是"公式贴在曲面上"的最小载体；UV 边界精确为 0 / 1，便于纹理对位。
// v 轴按纹理约定反向（`v = 0` 对齐 `y = v_max`），使贴图在 Y-up 曲面上正立。

use glam::{Vec2, Vec3};

use crate::scene::components::{MeshGeometry, PrimitiveTopology};

/// 参数曲面描述：参数矩形域 + 离散步数 + 标量高度函数。
///
/// - `u_range` / `v_range`：参数域（同时也是 x / y 的定义域）。
/// - `u_steps` / `v_steps`：两方向的**格子数**（顶点数 = 步数 + 1）。
/// - `f`：`z = f(x, y)`，高度函数；常数函数即平面。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::primitives::{SurfaceSpec, parametric_surface};
///
/// let spec = SurfaceSpec::new((-1.0, 1.0), (-1.0, 1.0), 4, 4, |x, y| x * y);
/// let mesh = parametric_surface(spec);
/// // (4 + 1) * (4 + 1) 个顶点，4 * 4 * 6 个索引。
/// assert_eq!(mesh.positions.len(), 25);
/// assert_eq!(mesh.indices.as_ref().unwrap().len(), 96);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SurfaceSpec {
    /// u 方向（x）参数范围 `(min, max)`。
    pub u_range: (f32, f32),
    /// v 方向（y）参数范围 `(min, max)`。
    pub v_range: (f32, f32),
    /// u 方向格子数。
    pub u_steps: usize,
    /// v 方向格子数。
    pub v_steps: usize,
    /// 高度函数 `z = f(x, y)`。
    pub f: fn(f32, f32) -> f32,
}

impl SurfaceSpec {
    /// 构造参数曲面描述。
    pub fn new(
        u_range: (f32, f32),
        v_range: (f32, f32),
        u_steps: usize,
        v_steps: usize,
        f: fn(f32, f32) -> f32,
    ) -> Self {
        Self {
            u_range,
            v_range,
            u_steps,
            v_steps,
            f,
        }
    }
}

/// 生成参数曲面网格：规则网格顶点 + 逐顶点 UV / 法线 + 三角索引。
///
/// 顶点按行主序（外层 v、内层 u）排列，行列间距即参数步长；
/// 每格拆成两个逆时针三角形（从 +z 侧观察）。
///
/// UV 的 u 随 x 递增（`u_min → 0`，`u_max → 1`）；v 随 y 递减（`v_max → 0`，
/// `v_min → 1`），即纹理 v = 0 对齐世界 y 的**上方**，与 wgpu「纹理左上为原点」
/// 一致，使贴图在 Y-up 曲面上正立。
///
/// 法线由中心差分近似 `(-∂f/∂x, -∂f/∂y, 1)` 后单位化，
/// 与解析法线在足够细分下一致，且对任意 `f` 都成立。
pub fn parametric_surface(spec: SurfaceSpec) -> MeshGeometry {
    let (u_min, u_max) = spec.u_range;
    let (v_min, v_max) = spec.v_range;
    let cols = spec.u_steps + 1;
    let rows = spec.v_steps + 1;

    let mut positions = Vec::with_capacity(cols * rows);
    let mut uvs = Vec::with_capacity(cols * rows);
    let mut normals = Vec::with_capacity(cols * rows);

    // 中心差分步长：取各自方向步长的一半（退化时回落到固定小量）。
    let h_u = half_step(u_min, u_max, spec.u_steps);
    let h_v = half_step(v_min, v_max, spec.v_steps);

    for j in 0..rows {
        let v = j as f32 / spec.v_steps.max(1) as f32;
        let y = lerp(v_min, v_max, v);
        for i in 0..cols {
            let u = i as f32 / spec.u_steps.max(1) as f32;
            let x = lerp(u_min, u_max, u);

            positions.push(Vec3::new(x, y, (spec.f)(x, y)));
            // UV 直接取归一化参数：u = i / steps，v = 1 - j / steps。
            // v 取「1 - 」是为了对齐纹理坐标约定：wgpu 纹理 v = 0 在图像**顶部**，
            // 而世界 Y 向上 ⇒ y 越大 v 越小（`v_max` ↔ v = 0），贴图在 Y-up 曲面上
            // 正立。`i = 0 / steps` 与 `j = 0 / steps` 仍精确映射到 0.0 / 1.0。
            uvs.push(Vec2::new(u, 1.0 - v));

            let dx = ((spec.f)(x + h_u, y) - (spec.f)(x - h_u, y)) / (2.0 * h_u);
            let dy = ((spec.f)(x, y + h_v) - (spec.f)(x, y - h_v)) / (2.0 * h_v);
            normals.push(Vec3::new(-dx, -dy, 1.0).normalize());
        }
    }

    let mut indices = Vec::with_capacity(spec.u_steps * spec.v_steps * 6);
    for j in 0..spec.v_steps {
        for i in 0..spec.u_steps {
            let i0 = (j * cols + i) as u32;
            let i1 = i0 + 1;
            let i2 = i0 + cols as u32;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i1, i3, i0, i3, i2]);
        }
    }

    MeshGeometry::new(positions, Some(indices), PrimitiveTopology::Triangles)
        .with_uvs(uvs)
        .with_normals(normals)
}

/// 参数域上的线性插值；端点分别精确等于 `min` / `max`。
fn lerp(min: f32, max: f32, t: f32) -> f32 {
    min * (1.0 - t) + max * t
}

/// 中心差分步长：格子数非零时取步长一半，否则回落固定小量。
fn half_step(min: f32, max: f32, steps: usize) -> f32 {
    if steps == 0 {
        1e-3
    } else {
        ((max - min) / steps as f32 * 0.5).abs().max(f32::EPSILON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uv_boundaries_are_exact() {
        let mesh = parametric_surface(SurfaceSpec::new((-2.0, 3.0), (0.0, 5.0), 7, 5, |_, _| 0.0));
        let uvs = mesh.uvs.as_ref().expect("参数曲面必须带 UV");

        // u 方向：第 0 列恒为 0.0，最后一列恒为 1.0；v 随 j 反向（j=0 → 1.0）。
        for j in 0..=5 {
            let v = 1.0 - j as f32 / 5.0;
            assert_eq!(uvs[j * 8], Vec2::new(0.0, v));
            assert_eq!(uvs[j * 8 + 7], Vec2::new(1.0, v));
        }
        // v 方向：第 0 行（y = v_min）恒为 1.0，最后一行（y = v_max）恒为 0.0。
        for i in 0..=7 {
            assert_eq!(uvs[i], Vec2::new(i as f32 / 7.0, 1.0));
            assert_eq!(uvs[5 * 8 + i], Vec2::new(i as f32 / 7.0, 0.0));
        }
        // 边界仍精确（无浮点漂移），只是 v 的两个极点对调。
        assert_eq!(uvs[0], Vec2::new(0.0, 1.0));
        assert_eq!(uvs[uvs.len() - 1], Vec2::new(1.0, 0.0));
    }

    #[test]
    fn test_vertex_and_index_counts() {
        let mesh = parametric_surface(SurfaceSpec::new((0.0, 1.0), (0.0, 1.0), 4, 3, |x, y| x + y));
        assert_eq!(mesh.positions.len(), (4 + 1) * (3 + 1));
        let indices = mesh.indices.as_ref().expect("参数曲面必须带索引");
        assert_eq!(indices.len(), 4 * 3 * 6);
        assert_eq!(indices.len() % 3, 0);
        assert!(
            indices.iter().all(|&i| (i as usize) < mesh.positions.len()),
            "索引必须落在顶点范围内"
        );
    }

    #[test]
    fn test_constant_function_is_a_flat_plane() {
        let mesh = parametric_surface(SurfaceSpec::new((-3.0, 3.0), (-2.0, 2.0), 6, 6, |_, _| 0.0));
        assert!(mesh.positions.iter().all(|p| p.z == 0.0));
    }

    #[test]
    fn test_normals_are_unit_length() {
        let mesh =
            parametric_surface(SurfaceSpec::new((-1.0, 1.0), (-1.0, 1.0), 8, 8, |x, y| x * y));
        let normals = mesh.normals.as_ref().expect("参数曲面必须带法线");
        assert_eq!(normals.len(), mesh.positions.len());
        for n in normals {
            assert!(
                (n.length() - 1.0).abs() < 1e-5,
                "法线必须为单位向量，得到 {:?}",
                n
            );
        }
    }
}
