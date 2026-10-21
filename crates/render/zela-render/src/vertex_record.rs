// vertex_record.rs — 顶点**生产端**（B3-0：生产逻辑离开 backend crate）。
//
// # 定位
// 本模块只做 CPU 侧工作：几何 → 交错字节（`VertexBufferData`）。它**不依赖 wgpu**，
// 因此任何后端（wgpu / 未来 WebGL / 软件光栅）都复用同一份生产逻辑：
//
// ```text
// 生产端（本模块）：Shape2D / MeshGeometry / LineDraw → VertexBufferData
// 后端          ：VertexBufferData → create_buffer_init（只会上传）
// ```
//
// # 与相机的关系（不变）
// 2D 三角化的容差依赖相机 scale（`tolerance_for_scale`）。本模块只把"调用 + 打包"
// 从后端搬出来，**调用时机仍在渲染阶段**（相机已确定），不提前到场景录制。
//
// # 单一真源
// - 2D：顶点格式由 `tessellate` 产出的 `VertexLayout` 声明（stride 24）；
// - 3D：`VertexLayout::standard_3d()`（stride 32）；
// - 打包后的字节必须满足 `VertexBufferData::validate()`（长度 / 索引范围）。

use ze_notion::{Path2D, Transform2D, Vec2, Vec3};
use ze_render_ir::{Shape2D, Style2D, UvProjection};
use ze_vertex::{bytes_of, VertexBufferData, VertexBufferError, VertexLayout};
use ze_scene_core::components::MeshGeometry;

use crate::tessellate::{TessKind, Tessellation2D, TessellateError, tessellate, tessellate_reveal};

/// 生产端错误：三角化失败与打包失败都**显式**冒泡（不静默）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VertexRecordError {
    #[error("三角化失败：{0}")]
    Tessellation(#[from] TessellateError),
    #[error("顶点缓冲打包失败：{0}")]
    Buffer(#[from] VertexBufferError),
    #[error(
        "形变两端拓扑不一致（等点重采样后不应发生）：顶点 {from_vertices} vs {to_vertices}、索引 {from_indices} vs {to_indices}"
    )]
    MorphTopologyMismatch {
        from_vertices: usize,
        to_vertices: usize,
        from_indices: usize,
        to_indices: usize,
    },
}

/// 一次三角化 + 打包的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TessellatedBuffer {
    pub data: VertexBufferData,
    pub kind: TessKind,
}

fn pack(t: Tessellation2D) -> Result<TessellatedBuffer, VertexRecordError> {
    let data = VertexBufferData::new(t.layout, bytes_of(&t.vertices).to_vec(), t.indices)?;
    Ok(TessellatedBuffer {
        data,
        kind: t.kind,
    })
}

/// 2D 图元 → 顶点缓冲（填充 / 描边各至多一项；`Group` 递归成多项）。
pub fn tessellate_to_buffers(
    shape: &Shape2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
) -> Result<Vec<TessellatedBuffer>, VertexRecordError> {
    let mut out = Vec::new();
    for item in tessellate(shape, transform, style, tolerance)? {
        out.push(pack(item)?);
    }
    Ok(out)
}

/// 形变端点：路径 → 顶点缓冲（与 [`tessellate_to_buffers`] 同一份打包）。
pub fn tessellate_path_to_buffers(
    path: &Path2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
) -> Result<Vec<TessellatedBuffer>, VertexRecordError> {
    tessellate_to_buffers(&Shape2D::Path(path.clone()), transform, style, tolerance)
}

/// 书写动画：路径按 `step` 预细分 → 顶点缓冲（`None` = 非描边/无几何）。
pub fn tessellate_reveal_to_buffer(
    path: &Path2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
    step: f32,
) -> Result<Option<TessellatedBuffer>, VertexRecordError> {
    match tessellate_reveal(path, transform, style, tolerance, step) {
        Some(item) => Ok(Some(pack(item)?)),
        None => Ok(None),
    }
}

/// 形变两端的顶点缓冲（拓扑一致 ⇒ 共用索引表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphEndpointBuffers {
    pub from: TessellatedBuffer,
    pub to: TessellatedBuffer,
}

/// 形变端点生产：`from` / `to` 两条路径 ⇒ 两端缓冲。
///
/// - 两侧都无几何 ⇒ `Ok(None)`（调用方跳过该项）；
/// - 仅一侧有几何 ⇒ 用**可三角化的一侧**的拓扑合成退化侧（顶点塌到该路径首点、
///   法线归零，`t = 0` 时三角形退化不可见）；
/// - 拓扑不一致 ⇒ **显式错误**（等点重采样后不应发生，不静默跳过）。
pub fn morph_endpoint_buffers(
    from_path: &Path2D,
    to_path: &Path2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
) -> Result<Option<MorphEndpointBuffers>, VertexRecordError> {
    let from_tess = tessellate(&Shape2D::Path(from_path.clone()), transform, style, tolerance)?
        .into_iter()
        .next();
    let to_tess = tessellate(&Shape2D::Path(to_path.clone()), transform, style, tolerance)?
        .into_iter()
        .next();

    let (from, to) = match (from_tess, to_tess) {
        (Some(f), Some(t)) => (f, t),
        (None, Some(t)) => {
            let c = collapsed_like(&t, from_path);
            (c, t)
        }
        (Some(f), None) => {
            let c = collapsed_like(&f, to_path);
            (f, c)
        }
        (None, None) => return Ok(None),
    };

    if from.indices.len() != to.indices.len() || from.vertices.len() != to.vertices.len() {
        return Err(VertexRecordError::MorphTopologyMismatch {
            from_vertices: from.vertices.len(),
            to_vertices: to.vertices.len(),
            from_indices: from.indices.len(),
            to_indices: to.indices.len(),
        });
    }

    Ok(Some(MorphEndpointBuffers {
        from: pack(from)?,
        to: pack(to)?,
    }))
}

/// 按模板拓扑合成"退化侧"：顶点全部塌到 `zero_path` 的首点，法线归零。
fn collapsed_like(template: &Tessellation2D, zero_path: &Path2D) -> Tessellation2D {
    let pivot = zero_path
        .flatten(0.0)
        .first()
        .copied()
        .unwrap_or(Vec2::ZERO);
    let mut out = template.clone();
    for v in &mut out.vertices {
        v.position = [pivot.x, pivot.y];
        v.normal = [0.0, 0.0];
    }
    out
}

// ─── 3D 生产（B3-0b）────────────────────────────────────────────────────────

/// 3D 顶点格式：`[pos.xyz, uv.xy, normal.xyz]`（8 个 f32 = 32 字节）。
pub const VERTEX_3D_FLOATS: usize = 8;

/// 3D 标准布局（stride 32）—— 单一真源是 `ze-vertex`。
pub fn layout_3d() -> VertexLayout {
    VertexLayout::standard_3d()
}

/// **交错顶点数据**：每顶点 `[pos.xyz, uv.xy, normal.xyz]`。
///
/// `uvs` 必须与 `positions` 等长（几何自带或 [`fallback_uvs`] 兜底）——不等长时
/// 对缺失 UV 以 (0, 0) 补齐。`normals` 为 `None`（如线框立方体）或长度不足时，
/// 对应顶点以 `(0, 0, 1)` 补齐：配合默认头顶光照（`L = (0, 0, 1)`）得 `lit = 1`。
pub fn interleaved_pos_uv_normal(
    positions: &[Vec3],
    uvs: &[Vec2],
    normals: Option<&[Vec3]>,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(positions.len() * VERTEX_3D_FLOATS);
    for (i, p) in positions.iter().enumerate() {
        let uv = uvs.get(i).copied().unwrap_or(Vec2::ZERO);
        let n = normals.and_then(|ns| ns.get(i).copied()).unwrap_or(Vec3::Z);
        out.extend_from_slice(&[p.x, p.y, p.z, uv.x, uv.y, n.x, n.y, n.z]);
    }
    out
}

/// 几何未携带 UV 时的兜底 UV（包围盒归一化到 [0, 1]）。
///
/// - [`UvProjection::Planar`]：XY 平面投影；
/// - [`UvProjection::Spherical`]：围绕包围盒中心的方向向量球面映射；
/// - [`UvProjection::Triplanar`]：按相对中心偏移最大的分量选主轴投影。
///
/// 退化包围盒（任一轴跨度为 0）按 1e-6 处理，避免除零。
pub fn fallback_uvs(positions: &[Vec3], projection: UvProjection) -> Vec<Vec2> {
    if positions.is_empty() {
        return Vec::new();
    }
    let mut min = positions[0];
    let mut max = positions[0];
    for p in positions {
        min = min.min(*p);
        max = max.max(*p);
    }
    let size = (max - min).max(Vec3::splat(1e-6));
    let center = (min + max) * 0.5;

    positions
        .iter()
        .map(|p| {
            let local = *p - min;
            match projection {
                UvProjection::Planar => Vec2::new(local.x / size.x, local.y / size.y),
                UvProjection::Spherical => {
                    let d = (*p - center).normalize_or_zero();
                    Vec2::new(d.x * 0.5 + 0.5, d.y * 0.5 + 0.5)
                }
                UvProjection::Triplanar => {
                    let rel = *p - center;
                    let (ax, ay) = if rel.x.abs() >= rel.y.abs() && rel.x.abs() >= rel.z.abs() {
                        (local.y / size.y, local.z / size.z)
                    } else if rel.y.abs() >= rel.z.abs() {
                        (local.x / size.x, local.z / size.z)
                    } else {
                        (local.x / size.x, local.y / size.y)
                    };
                    Vec2::new(ax, ay)
                }
            }
        })
        .collect()
}

/// 几何 → 顶点缓冲（UV 选择 + 法线补齐 + 交错打包，全部在生产端完成）。
pub fn mesh_vertex_buffer(
    geometry: &MeshGeometry,
    projection: UvProjection,
) -> Result<VertexBufferData, VertexRecordError> {
    let uvs = match &geometry.uvs {
        Some(uvs) => uvs.clone(),
        None => fallback_uvs(&geometry.positions, projection),
    };
    let floats = interleaved_pos_uv_normal(&geometry.positions, &uvs, geometry.normals.as_deref());
    let indices = geometry.indices.clone().unwrap_or_default();
    Ok(VertexBufferData::new(layout_3d(), bytes_of(&floats).to_vec(), indices)?)
}

/// 3D 线段（两点）→ 顶点缓冲。
///
/// line 无材质纹理 ⇒ UV 恒 0；法线 `(0,0,1)`（配合默认头顶光 `lit = 1`，
/// 颜色不受光照影响）。**不再内联**（B7：内联是第二份真源）。
pub fn line_vertex_buffer(start: Vec3, end: Vec3) -> Result<VertexBufferData, VertexRecordError> {
    let floats: [f32; VERTEX_3D_FLOATS * 2] = [
        start.x, start.y, start.z, 0.0, 0.0, 0.0, 0.0, 1.0, end.x, end.y, end.z, 0.0, 0.0, 0.0,
        0.0, 1.0,
    ];
    Ok(VertexBufferData::new(
        layout_3d(),
        bytes_of(&floats).to_vec(),
        Vec::new(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::Color32;
    use ze_scene_core::mesh_primitives::wireframe_cube;

    /// 3D 顶点缓冲：stride 32、顶点数正确、字节序与布局一致。
    #[test]
    fn mesh_vertex_buffer_matches_layout() {
        let geometry = wireframe_cube(1.0);
        let data = mesh_vertex_buffer(&geometry, UvProjection::Planar).unwrap();
        assert_eq!(data.layout.stride, 32);
        assert_eq!(data.vertex_count as usize, geometry.positions.len());
        data.validate().unwrap();
    }

    /// line 顶点缓冲：2 个顶点、无索引、法线 +Z。
    #[test]
    fn line_vertex_buffer_is_two_vertices() {
        let data = line_vertex_buffer(Vec3::ZERO, Vec3::X).unwrap();
        assert_eq!(data.vertex_count, 2);
        assert!(data.indices.is_empty());
        let words: Vec<f32> = data
            .vertices
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(&words[5..8], &[0.0, 0.0, 1.0]);
        assert_eq!(&words[13..16], &[0.0, 0.0, 1.0]);
    }

    /// 2D 生产：矩形填充 → stride 24、validate 通过（格式未变，D1）。
    #[test]
    fn tessellated_buffer_packs_2d_stride_24() {
        let shape = Shape2D::Rect(ze_notion::Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 10.0)));
        let style = Style2D {
            fill: Some(Color32::WHITE),
            ..Style2D::default()
        };
        let buffers = tessellate_to_buffers(
            &shape,
            &Transform2D::identity(),
            &style,
            1.0,
        )
        .unwrap();
        assert!(!buffers.is_empty());
        for b in buffers {
            assert_eq!(b.data.layout.stride, 24);
            b.data.validate().unwrap();
        }
    }
}
