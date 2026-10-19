// cube.rs
//
// 立方体 MeshGeometry 生成器：线框（12 条边，Lines 拓扑）与实心（12 个三角形，Triangles 拓扑）。

use ze_notion::Vec3;

use crate::scene::components::{MeshGeometry, PrimitiveTopology};

/// 立方体 8 个角点（h = half_extent）：
/// 0..=3 为后面（z=-h），4..=7 为前面（z=+h）。
fn cube_vertices(h: f32) -> Vec<Vec3> {
    vec![
        Vec3::new(-h, -h, -h), // 0 后-左下
        Vec3::new(h, -h, -h),  // 1 后-右下
        Vec3::new(h, h, -h),   // 2 后-右上
        Vec3::new(-h, h, -h),  // 3 后-左上
        Vec3::new(-h, -h, h),  // 4 前-左下
        Vec3::new(h, -h, h),   // 5 前-右下
        Vec3::new(h, h, h),    // 6 前-右上
        Vec3::new(-h, h, h),   // 7 前-左上
    ]
}

/// 生成线框立方体：8 顶点 + 24 索引（12 条边），[`PrimitiveTopology::Lines`]。
///
/// 每 2 个索引一条线段。
pub fn wireframe_cube(half_extent: f32) -> MeshGeometry {
    let indices: Vec<u32> = vec![
        // 后面
        0, 1, 1, 2, 2, 3, 3, 0, // 前面
        4, 5, 5, 6, 6, 7, 7, 4, // 前后连接
        0, 4, 1, 5, 2, 6, 3, 7,
    ];
    MeshGeometry::new(
        cube_vertices(half_extent),
        Some(indices),
        PrimitiveTopology::Lines,
    )
}

/// 生成立方体实体：8 顶点 + 36 索引（12 个三角形），[`PrimitiveTopology::Triangles`]。
///
/// 三角形绕序为 CCW（从外部观察），配合默认背面剔除。
pub fn solid_cube(half_extent: f32) -> MeshGeometry {
    let indices: Vec<u32> = vec![
        // 后面 (z=-h，法线 -z)
        0, 3, 2, 0, 2, 1, // 前面 (z=+h，法线 +z)
        4, 5, 6, 4, 6, 7, // 顶面 (y=+h，法线 +y)
        3, 7, 6, 3, 6, 2, // 底面 (y=-h，法线 -y)
        0, 1, 5, 0, 5, 4, // 右面 (x=+h，法线 +x)
        1, 2, 6, 1, 6, 5, // 左面 (x=-h，法线 -x)
        0, 4, 7, 0, 7, 3,
    ];
    MeshGeometry::new(
        cube_vertices(half_extent),
        Some(indices),
        PrimitiveTopology::Triangles,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wireframe_cube_topology_and_counts() {
        let cube = wireframe_cube(1.0);
        assert_eq!(cube.positions.len(), 8);
        assert_eq!(cube.topology, PrimitiveTopology::Lines);
        let indices = cube.indices.as_ref().expect("wireframe has indices");
        assert_eq!(indices.len(), 24);
        assert!(indices.iter().all(|&i| i < 8));
    }

    #[test]
    fn test_solid_cube_topology_and_counts() {
        let cube = solid_cube(1.0);
        assert_eq!(cube.positions.len(), 8);
        assert_eq!(cube.topology, PrimitiveTopology::Triangles);
        let indices = cube.indices.as_ref().expect("solid cube has indices");
        assert_eq!(indices.len(), 36);
        assert_eq!(indices.len() % 3, 0);
        assert!(indices.iter().all(|&i| i < 8));
    }

    #[test]
    fn test_wireframe_cube_half_extent() {
        let cube = wireframe_cube(2.0);
        for v in &cube.positions {
            assert!(v.x.abs() <= 2.0 && v.y.abs() <= 2.0 && v.z.abs() <= 2.0);
        }
    }
}
