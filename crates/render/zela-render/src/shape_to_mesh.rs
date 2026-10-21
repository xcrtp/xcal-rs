// shape_to_mesh.rs — 2D 图元 → 3D 网格（B2-3 派生桥；B3 Step 1 复用三角化器）。
//
// # 为什么需要
// B2 的目标之一是让 2D 内容参与 3D 的深度/层序（融合）。`Shape2D` 是屏幕/世界
// XY 平面上的二维图元，本模块把它**降级为一个平面网格**（法线恒 +Z），交给 3D
// 管线绘制：
//   - `double_sided = true`（D19 意图层）—— 2D 派生片是"纸片"，从背面也必须可见；
//   - `transparent` 由填充/描边颜色 alpha 推导（`< 255`）—— 透明桶排序见后端；
//   - `variant = Flat`：二维图元不参与光照，颜色即终值。
//
// # 单一真源（B3 Step 1）
// 三角化只走 [`crate::tessellate`]（lyon）：Rect / Circle / Ellipse / Polygon /
// Line / **Path** / **Group** 七种图元在这里统一派生。原先手写的 ellipse /
// polygon / line_ribbon 三个近似三角化函数已删除（第二份真源）。
//
// # 明确限制（不静默）
// - `Polyline` / `Custom` 仍**显式报错**（Direct 路径未声明支持；不静默丢弃）；
// - **不携带 2D 的边法线槽**：`attrib0` 是 AA 边法线，不是着色法线。转换时丢弃，
//   着色法线恒 `(0,0,1)`（3D 布局 stride 32 三槽已满）；
// - **Reveal 的 advance 槽无处安放** ⇒ Direct 路径不支持 Reveal（由调用方显式拒绝）；
// - Direct 是单材质：`fill` 优先；两者同时存在时描边不进入本网格（与 B2-3 行为一致）。

use std::sync::Arc;

use ze_notion::{Color32, Transform2D, Vec2, Vec3};
use ze_render_ir::{CustomSpecs, Shape2D, ShaderVariant, Style2D};
use ze_scene_core::components::{Material3D, MeshGeometry, PrimitiveTopology};

use crate::tessellate::{tessellate_with, tolerance_for_scale};

/// 2D → 3D 派生失败的原因（显式错误，铁律 2）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShapeMeshError {
    #[error("2D 图元 {kind} 暂不支持派生为 3D 网格（Direct 路径未声明；不静默丢弃）")]
    Unsupported { kind: &'static str },
    #[error("2D 图元 {kind} 的几何退化（点数/半径/宽度不足）")]
    Degenerate { kind: &'static str },
    #[error("2D 图元 {kind} 三角化失败：{message}")]
    Tessellate {
        kind: &'static str,
        message: String,
    },
}

/// 派生结果：平面网格 + 意图化材质 + 是否透明。
pub struct DerivedMesh {
    pub geometry: Arc<MeshGeometry>,
    pub material: Material3D,
    pub transparent: bool,
}

/// 把 2D 图元派生为**朝向 +Z 的平面网格**（B2-3 / B3 Step 1）。
///
/// `style` 只消费颜色与描边宽度：填充色优先，否则描边色；两者都缺席 ⇒ 白色。
pub fn derive_mesh(shape: &Shape2D, style: &Style2D) -> Result<DerivedMesh, ShapeMeshError> {
    // Direct 路径未声明支持的两类：显式拒绝（铁律 1）。
    match shape {
        Shape2D::Polyline { .. } => {
            return Err(ShapeMeshError::Unsupported { kind: "Polyline" })
        }
        Shape2D::Custom(_) => return Err(ShapeMeshError::Unsupported { kind: "Custom" }),
        Shape2D::Empty => return Err(ShapeMeshError::Degenerate { kind: "Empty" }),
        _ => {}
    }

    let color = style.fill.or(style.stroke).unwrap_or(Color32::WHITE);

    // 单材质：fill 优先；只有描边时走 `TessKind::Stroke`（替换旧的粗糙矩形带）。
    let tess_style = Style2D {
        fill: style.fill,
        stroke: if style.fill.is_none() {
            style.stroke
        } else {
            None
        },
        stroke_width: style.stroke_width,
        space: style.space,
    };

    // 1 point = 1 px（Direct 路径的约定；相机 scale 在 Step 3 的 Surface 映射里）。
    let parts = tessellate_with(
        shape,
        &Transform2D::identity(),
        &tess_style,
        tolerance_for_scale(1.0),
        &CustomSpecs::EMPTY,
    )
    .map_err(|error| ShapeMeshError::Tessellate {
        kind: kind_of(shape),
        message: error.to_string(),
    })?;

    let vertex_count: usize = parts.iter().map(|part| part.vertices.len()).sum();
    let index_count: usize = parts.iter().map(|part| part.indices.len()).sum();
    if vertex_count == 0 || index_count == 0 {
        return Err(ShapeMeshError::Degenerate { kind: kind_of(shape) });
    }

    let mut positions = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(index_count);
    for part in &parts {
        let base = positions.len() as u32;
        for vertex in &part.vertices {
            let [x, y] = vertex.position;
            positions.push(Vec3::new(x, y, 0.0));
            // 平面片无纹理语义：UV 直接取 XY（0..1 的归一化留给纹理路径）。
            uvs.push(Vec2::new(x, y));
        }
        indices.extend(part.indices.iter().map(|index| index + base));
    }

    // 2D 的 `attrib0`（边法线）在这里**丢弃**：着色法线恒 +Z。
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

fn kind_of(shape: &Shape2D) -> &'static str {
    match shape {
        Shape2D::Rect(_) => "Rect",
        Shape2D::Circle { .. } => "Circle",
        Shape2D::Ellipse { .. } => "Ellipse",
        Shape2D::Polygon { .. } => "Polygon",
        Shape2D::Line { .. } => "Line",
        Shape2D::Path(_) => "Path",
        Shape2D::Polyline { .. } => "Polyline",
        Shape2D::Group { .. } => "Group",
        Shape2D::Custom(_) => "Custom",
        Shape2D::Empty => "Empty",
        // `Shape2D` 是 `#[non_exhaustive]`：新增变体必须在这里显式决定（不静默）。
        _ => "unknown-variant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::{Path2D, Rect2D};

    fn style(color: Color32) -> Style2D {
        Style2D {
            fill: Some(color),
            ..Style2D::default()
        }
    }

    fn assert_plane_geometry(derived: &DerivedMesh) {
        assert!(!derived.geometry.positions.is_empty());
        let indices = derived.geometry.indices.as_ref().unwrap();
        assert!(!indices.is_empty());
        assert_eq!(indices.len() % 3, 0, "必须输出三角形列表");
        // 着色法线恒 +Z；2D 边法线已被丢弃。
        assert!(derived
            .geometry
            .normals
            .as_ref()
            .unwrap()
            .iter()
            .all(|n| *n == Vec3::new(0.0, 0.0, 1.0)));
        // UV = XY（无 2D 槽语义）。
        let uvs = derived.geometry.uvs.as_ref().unwrap();
        assert_eq!(uvs.len(), derived.geometry.positions.len());
        for (uv, position) in uvs.iter().zip(derived.geometry.positions.iter()) {
            assert_eq!((uv.x, uv.y), (position.x, position.y));
        }
    }

    /// Rect → 平面三角形列表；材质走 Flat + 双面；不透明色 ⇒ transparent=false。
    #[test]
    fn rect_derives_plane_triangles_and_flat_double_sided_material() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0)));
        let d = derive_mesh(&shape, &style(Color32::WHITE)).unwrap();
        assert_plane_geometry(&d);
        assert_eq!(d.geometry.indices.as_ref().unwrap().len(), 6);
        assert_eq!(d.material.variant, ShaderVariant::Flat);
        assert!(d.material.double_sided, "2D 派生片必须双面可见");
        assert!(!d.transparent);
    }

    /// alpha < 255 ⇒ 透明桶标记。
    #[test]
    fn translucent_color_marks_transparent() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(1.0, 1.0)));
        let d = derive_mesh(&shape, &style(Color32::new(255, 0, 0, 128))).unwrap();
        assert!(d.transparent);
        assert!(d.material.transparent);
    }

    /// Circle 走 lyon 三角化（不再手写扇形）。
    #[test]
    fn circle_derives_tessellated_fan() {
        let shape = Shape2D::Circle {
            center: Vec2::ZERO,
            radius: 1.0,
        };
        let d = derive_mesh(&shape, &style(Color32::WHITE)).unwrap();
        assert_plane_geometry(&d);
        assert!(d.geometry.positions.len() >= 3);
    }

    /// **Step 1 核心**：Path 不再 Unsupported，复用 tessellate。
    #[test]
    fn path_is_no_longer_unsupported() {
        let path = Path2D::from_polyline(
            &[
                Vec2::ZERO,
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
            true,
        );
        let d = derive_mesh(&Shape2D::Path(path), &style(Color32::WHITE)).unwrap();
        assert_plane_geometry(&d);
    }

    /// Group 递归展开为多项，再合并为一份网格。
    #[test]
    fn group_derives_all_children() {
        let group = Shape2D::Group {
            children: vec![
                Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(1.0, 1.0))),
                Shape2D::Circle {
                    center: Vec2::new(3.0, 3.0),
                    radius: 1.0,
                },
            ],
        };
        let d = derive_mesh(&group, &style(Color32::WHITE)).unwrap();
        assert_plane_geometry(&d);
    }

    /// 只有描边的 Line 走 `TessKind::Stroke`（旧实现是手写矩形带）。
    #[test]
    fn stroke_only_line_uses_tessellated_stroke() {
        let shape = Shape2D::Line {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 0.0),
        };
        let style = Style2D {
            fill: None,
            stroke: Some(Color32::WHITE),
            stroke_width: 2.0,
            ..Style2D::default()
        };
        let d = derive_mesh(&shape, &style).unwrap();
        assert_plane_geometry(&d);
        assert!(d.geometry.positions.len() >= 4);
    }

    /// Direct 路径未声明的图元必须**显式报错**（不静默）。
    #[test]
    fn undeclared_shapes_error_explicitly() {
        let polyline = Shape2D::Polyline {
            points: vec![Vec2::ZERO, Vec2::new(1.0, 0.0)],
        };
        assert!(matches!(
            derive_mesh(&polyline, &style(Color32::WHITE)),
            Err(ShapeMeshError::Unsupported { kind: "Polyline" })
        ));

        let custom = Shape2D::Custom(ze_render_ir::CustomShapeData::new(
            "test.custom",
            Rect2D::new(Vec2::ZERO, Vec2::new(1.0, 1.0)),
        ));
        assert!(matches!(
            derive_mesh(&custom, &style(Color32::WHITE)),
            Err(ShapeMeshError::Unsupported { kind: "Custom" })
        ));
    }

    /// 退化图元显式报错（零尺寸矩形）。
    #[test]
    fn degenerate_shape_errors_explicitly() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::ZERO));
        assert!(matches!(
            derive_mesh(&shape, &style(Color32::WHITE)),
            Err(ShapeMeshError::Degenerate { .. })
        ));
    }
}
