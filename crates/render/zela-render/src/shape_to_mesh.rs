// shape_to_mesh.rs — 2D 图元 → 3D 网格（B2-3：2D 派生网格进 3D）。
//
// # 为什么需要
// B2 的目标之一是让 2D 内容参与 3D 的深度/层序（融合）。`Shape2D` 是屏幕/世界
// XY 平面上的二维图元，本模块把它**降级为一个平面网格**（法线恒 +Z），交给 3D
// 管线绘制：
//   - `double_sided = true`（D19 意图层）—— 2D 派生片是"纸片"，从背面也必须可见
//     （B2-2 验收项之一）；
//   - `transparent` 由填充/描边颜色 alpha 推导（`< 255`）—— 透明桶排序见后端；
//   - `variant = Flat`：二维图元不参与光照，颜色即终值。
//
// # 明确限制（不静默）
// 仅支持可直接三角化的图元：`Rect` / `Circle` / `Ellipse` / `Polygon` / `Line`。
// `Path`（需要 lyon 三角化）与 `Group` / `Custom` **显式报错**，不静默丢弃 ——
// 后续批次若要支持，在此扩展而不要在后端做隐式兜底。

use std::sync::Arc;

use ze_notion::{Color32, Vec2, Vec3};
use ze_render_ir::{Shape2D, ShaderVariant, Style2D};
use zela_scene::scene::components::{Material3D, MeshGeometry, PrimitiveTopology};

/// 2D → 3D 派生失败的原因（显式错误，铁律 2）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShapeMeshError {
    #[error("2D 图元 {kind} 暂不支持派生为 3D 网格（需要三角化器；不静默丢弃）")]
    Unsupported { kind: &'static str },
    #[error("2D 图元 {kind} 的几何退化（点数/半径/宽度不足）")]
    Degenerate { kind: &'static str },
}

/// 派生结果：平面网格 + 意图化材质 + 是否透明。
pub struct DerivedMesh {
    pub geometry: Arc<MeshGeometry>,
    pub material: Material3D,
    pub transparent: bool,
}

/// 把 2D 图元派生为**朝向 +Z 的平面网格**（B2-3）。
///
/// `style` 只消费颜色与描边宽度：填充色优先，否则描边色；两者都缺席 ⇒ 白色。
pub fn derive_mesh(shape: &Shape2D, style: &Style2D) -> Result<DerivedMesh, ShapeMeshError> {
    let color = style.fill.or(style.stroke).unwrap_or(Color32::WHITE);
    let (positions, uvs, indices) = match shape {
        Shape2D::Rect(r) => {
            let (min, max) = (r.min, r.max);
            if !(max.x > min.x && max.y > min.y) {
                return Err(ShapeMeshError::Degenerate { kind: "Rect" });
            }
            (
                vec![
                    Vec3::new(min.x, min.y, 0.0),
                    Vec3::new(max.x, min.y, 0.0),
                    Vec3::new(max.x, max.y, 0.0),
                    Vec3::new(min.x, max.y, 0.0),
                ],
                vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(1.0, 1.0),
                    Vec2::new(0.0, 1.0),
                ],
                vec![0, 1, 2, 0, 2, 3],
            )
        }
        Shape2D::Circle { center, radius } => ellipse(*center, *radius, *radius, 32, "Circle")?,
        Shape2D::Ellipse { center, rx, ry } => ellipse(*center, *rx, *ry, 32, "Ellipse")?,
        Shape2D::Polygon { points } => polygon(points)?,
        Shape2D::Line { start, end } => {
            // 2D 线段 → 有宽度的矩形带（宽度取描边宽度，至少 1）；否则在 3D 里不可见。
            line_ribbon(*start, *end, style.stroke_width.max(1.0))?
        }
        Shape2D::Path(_) => {
            return Err(ShapeMeshError::Unsupported { kind: "Path" });
        }
        Shape2D::Polyline { .. } => {
            return Err(ShapeMeshError::Unsupported { kind: "Polyline" });
        }
        Shape2D::Group { .. } => {
            return Err(ShapeMeshError::Unsupported { kind: "Group" });
        }
        Shape2D::Custom(_) => {
            return Err(ShapeMeshError::Unsupported { kind: "Custom" });
        }
        // `Shape2D` 是 `#[non_exhaustive]`：新增变体必须在这里显式决定（不静默）。
        _ => {
            return Err(ShapeMeshError::Unsupported {
                kind: "unknown-variant",
            });
        }
    };

    let normals = vec![Vec3::new(0.0, 0.0, 1.0); positions.len()];
    let mut geometry = MeshGeometry::new(positions, Some(indices), PrimitiveTopology::Triangles);
    geometry.normals = Some(normals);
    geometry.uvs = Some(uvs);

    let transparent = color.a < 255;
    let material = Material3D {
        color,
        // 二维图元不参与光照：颜色即终值。
        variant: ShaderVariant::Flat,
        // B2-3 / D19：派生片是纸片，双面可见（背面不剔除）。
        double_sided: true,
        transparent,
        ..Material3D::default()
    };

    Ok(DerivedMesh {
        geometry: Arc::new(geometry),
        material,
        transparent,
    })
}

fn ellipse(
    center: Vec2,
    rx: f32,
    ry: f32,
    segments: usize,
    kind: &'static str,
) -> Result<(Vec<Vec3>, Vec<Vec2>, Vec<u32>), ShapeMeshError> {
    if !(rx > 0.0 && ry > 0.0) || segments < 3 {
        return Err(ShapeMeshError::Degenerate { kind });
    }
    let mut positions = Vec::with_capacity(segments + 1);
    let mut uvs = Vec::with_capacity(segments + 1);
    positions.push(Vec3::new(center.x, center.y, 0.0));
    uvs.push(Vec2::new(0.5, 0.5));
    for i in 0..segments {
        let a = std::f32::consts::TAU * (i as f32) / (segments as f32);
        let (s, c) = a.sin_cos();
        positions.push(Vec3::new(center.x + rx * c, center.y + ry * s, 0.0));
        uvs.push(Vec2::new(0.5 + 0.5 * c, 0.5 + 0.5 * s));
    }
    let mut indices = Vec::with_capacity(segments * 3);
    for i in 0..segments {
        let a = 1 + i as u32;
        let b = 1 + ((i + 1) % segments) as u32;
        indices.extend_from_slice(&[0, a, b]);
    }
    Ok((positions, uvs, indices))
}

fn polygon(points: &[Vec2]) -> Result<(Vec<Vec3>, Vec<Vec2>, Vec<u32>), ShapeMeshError> {
    if points.len() < 3 {
        return Err(ShapeMeshError::Degenerate { kind: "Polygon" });
    }
    let positions: Vec<Vec3> = points.iter().map(|p| Vec3::new(p.x, p.y, 0.0)).collect();
    let uvs: Vec<Vec2> = points
        .iter()
        .map(|p| Vec2::new(p.x - points[0].x, p.y - points[0].y))
        .collect();
    // 扇形三角化（凸多边形成立；非凸多边形在后续批次换 ear-clipping 并记账）。
    let mut indices = Vec::with_capacity((points.len() - 2) * 3);
    for i in 1..points.len() - 1 {
        indices.extend_from_slice(&[0, i as u32, (i + 1) as u32]);
    }
    Ok((positions, uvs, indices))
}

fn line_ribbon(
    start: Vec2,
    end: Vec2,
    width: f32,
) -> Result<(Vec<Vec3>, Vec<Vec2>, Vec<u32>), ShapeMeshError> {
    let d = end - start;
    let len = d.length();
    if len <= f32::EPSILON || !(width > 0.0) {
        return Err(ShapeMeshError::Degenerate { kind: "Line" });
    }
    let n = Vec2::new(-d.y, d.x) / len * (width * 0.5);
    let (a, b) = (start + n, start - n);
    let (c, e) = (end - n, end + n);
    Ok((
        vec![
            Vec3::new(a.x, a.y, 0.0),
            Vec3::new(b.x, b.y, 0.0),
            Vec3::new(c.x, c.y, 0.0),
            Vec3::new(e.x, e.y, 0.0),
        ],
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::Rect2D;

    fn style(color: Color32) -> Style2D {
        Style2D {
            fill: Some(color),
            ..Style2D::default()
        }
    }

    /// Rect → 2 三角形；材质走 Flat + 双面；不透明色 ⇒ transparent=false。
    #[test]
    fn rect_derives_two_triangles_and_flat_double_sided_material() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0)));
        let d = derive_mesh(&shape, &style(Color32::WHITE)).unwrap();
        assert_eq!(d.geometry.positions.len(), 4);
        assert_eq!(d.geometry.indices.as_ref().unwrap().len(), 6);
        assert_eq!(d.material.variant, ShaderVariant::Flat);
        assert!(d.material.double_sided, "2D 派生片必须双面可见");
        assert!(!d.transparent);
        // 法线恒 +Z（平面）
        assert!(d
            .geometry
            .normals
            .as_ref()
            .unwrap()
            .iter()
            .all(|n| *n == Vec3::new(0.0, 0.0, 1.0)));
    }

    /// alpha < 255 ⇒ 透明桶标记。
    #[test]
    fn translucent_color_marks_transparent() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(1.0, 1.0)));
        let d = derive_mesh(&shape, &style(Color32::new(255, 0, 0, 128))).unwrap();
        assert!(d.transparent);
        assert!(d.material.transparent);
    }

    /// Circle → 扇形（32 段）。
    #[test]
    fn circle_derives_fan() {
        let shape = Shape2D::Circle {
            center: Vec2::ZERO,
            radius: 1.0,
        };
        let d = derive_mesh(&shape, &style(Color32::WHITE)).unwrap();
        assert_eq!(d.geometry.positions.len(), 33);
        assert_eq!(d.geometry.indices.as_ref().unwrap().len(), 32 * 3);
    }

    /// 不支持的图元必须**显式报错**（不静默）。
    #[test]
    fn unsupported_shapes_error_explicitly() {
        let path = Shape2D::Path(ze_notion::Path2D::from_polyline(
            &[Vec2::ZERO, Vec2::new(1.0, 0.0)],
            false,
        ));
        assert!(matches!(
            derive_mesh(&path, &style(Color32::WHITE)),
            Err(ShapeMeshError::Unsupported { kind: "Path" })
        ));
    }
}
