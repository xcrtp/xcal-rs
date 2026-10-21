// cube.rs
//
// 立方体 MeshGeometry 生成器：线框（12 条边，Lines 拓扑）与实心（12 个三角形，Triangles 拓扑）。

use ze_notion::{Vec2, Vec3};

use crate::components::{MeshGeometry, PrimitiveTopology};

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

/// 生成立方体实体：24 顶点（每面 4 个，UV / 法线不共享）+ 36 索引，[`PrimitiveTopology::Triangles`]。
///
/// **为什么是 24 顶点**：贴图与光照都需要每面独立的 UV / 法线（8 个共享角点无法表达
/// 六个面各自的 0..1 映射与朝向）；显式 UV 也避免走 Planar 兜底 —— 兜底对"面平行于 Z"
/// 退化成 0 面积，会让 PBR 法线 TBN 除以 `det≈0` 产生 NaN（旧基线右脸的随机噪点即由此而来）。
///
/// UV 约定：每个面从外部看为 **左下 → 右下 → 右上 → 左上**，`v = 0` 是图像行首（上沿）
/// ⇒ 竖直面上 `+y` 取 `v = 0`。与渲染侧"纹理行首在 +y"的口径一致
/// （见 `zela_render::vertex_record::fallback_uvs`）。
///
/// 三角形绕序为 CCW（从外部观察），配合默认背面剔除。
pub fn solid_cube(half_extent: f32) -> MeshGeometry {
    let h = half_extent;
    let uv = [
        Vec2::new(0.0, 1.0), // 左下
        Vec2::new(1.0, 1.0), // 右下
        Vec2::new(1.0, 0.0), // 右上
        Vec2::new(0.0, 0.0), // 左上
    ];

    // 每面：4 个角点（左下/右下/右上/左上）+ 面法线。
    let faces: [([Vec3; 4], Vec3); 6] = [
        // 前面 (z=+h，法线 +z)：右 = +x，上 = +y
        (
            [
                Vec3::new(-h, -h, h),
                Vec3::new(h, -h, h),
                Vec3::new(h, h, h),
                Vec3::new(-h, h, h),
            ],
            Vec3::Z,
        ),
        // 后面 (z=-h，法线 -z)：右 = -x，上 = +y
        (
            [
                Vec3::new(h, -h, -h),
                Vec3::new(-h, -h, -h),
                Vec3::new(-h, h, -h),
                Vec3::new(h, h, -h),
            ],
            Vec3::new(0.0, 0.0, -1.0),
        ),
        // 右面 (x=+h，法线 +x)：右 = -z，上 = +y
        (
            [
                Vec3::new(h, -h, h),
                Vec3::new(h, -h, -h),
                Vec3::new(h, h, -h),
                Vec3::new(h, h, h),
            ],
            Vec3::X,
        ),
        // 左面 (x=-h，法线 -x)：右 = +z，上 = +y
        (
            [
                Vec3::new(-h, -h, -h),
                Vec3::new(-h, -h, h),
                Vec3::new(-h, h, h),
                Vec3::new(-h, h, -h),
            ],
            Vec3::new(-1.0, 0.0, 0.0),
        ),
        // 顶面 (y=+h，法线 +y)：右 = +x，图"上"= -z
        (
            [
                Vec3::new(-h, h, h),
                Vec3::new(h, h, h),
                Vec3::new(h, h, -h),
                Vec3::new(-h, h, -h),
            ],
            Vec3::Y,
        ),
        // 底面 (y=-h，法线 -y)：右 = +x，图"上"= +z
        (
            [
                Vec3::new(-h, -h, -h),
                Vec3::new(h, -h, -h),
                Vec3::new(h, -h, h),
                Vec3::new(-h, -h, h),
            ],
            Vec3::new(0.0, -1.0, 0.0),
        ),
    ];

    let mut positions = Vec::with_capacity(24);
    let mut uvs = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (index, (face, normal)) in faces.into_iter().enumerate() {
        let base = (index * 4) as u32;
        positions.extend_from_slice(&face);
        uvs.extend_from_slice(&uv);
        normals.extend_from_slice(&[normal; 4]);
        // 每个面：左下 → 右下 → 右上 / 左下 → 右上 → 左上（CCW）。
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    MeshGeometry::new(positions, Some(indices), PrimitiveTopology::Triangles)
        .with_uvs(uvs)
        .with_normals(normals)
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
        // 每面 4 个独立顶点（UV 不共享）。
        assert_eq!(cube.positions.len(), 24);
        assert_eq!(cube.topology, PrimitiveTopology::Triangles);
        let indices = cube.indices.as_ref().expect("solid cube has indices");
        assert_eq!(indices.len(), 36);
        assert_eq!(indices.len() % 3, 0);
        assert!(indices.iter().all(|&i| i < 24));
        let uvs = cube.uvs.as_ref().expect("实心立方体必须带每面 UV");
        assert_eq!(uvs.len(), 24);
        // 前面（第 0 面）四个角：左下/右下/右上/左上 ⇒ v = 1/1/0/0。
        assert_eq!(uvs[0], Vec2::new(0.0, 1.0));
        assert_eq!(uvs[1], Vec2::new(1.0, 1.0));
        assert_eq!(uvs[2], Vec2::new(1.0, 0.0));
        assert_eq!(uvs[3], Vec2::new(0.0, 0.0));
    }

    /// 每个面的 UV 必须张开（非退化）：否则 PBR 法线 TBN 会除以 `det≈0` 产生 NaN。
    #[test]
    fn test_solid_cube_faces_have_non_degenerate_uv() {
        let cube = solid_cube(1.0);
        let uvs = cube.uvs.as_ref().expect("uvs");
        for face in 0..6 {
            let quad = &uvs[face * 4..face * 4 + 4];
            let du = (quad[1].x - quad[0].x).abs();
            let dv = (quad[3].y - quad[0].y).abs();
            assert!(du > 0.5 && dv > 0.5, "第 {face} 面 UV 退化：{quad:?}");
        }
    }

    #[test]
    fn test_wireframe_cube_half_extent() {
        let cube = wireframe_cube(2.0);
        for v in &cube.positions {
            assert!(v.x.abs() <= 2.0 && v.y.abs() <= 2.0 && v.z.abs() <= 2.0);
        }
    }
}
