// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: B4 Step 5 场景适配（V2-06）—— zela 的 2D 图元 → 矢量后端的填充记录。

//! 场景适配：`Shape2D` + `Transform2D` + `Style2D` → 上游的填充记录。
//!
//! **本批只映射填充族**（B4 验收 §四 V2-06 的第一段）：
//! 圆 / 椭圆 / 矩形 / 多边形 / 折线 / 路径（含贝塞尔与椭圆弧）。
//!
//! 明确**不支持**（返回 [`SceneMapError::Unsupported`]，不静默降级）：
//! 描边（cap / joint / 宽度语义未与上游 `Paint` 对齐）、渐变（上游笔刷行未接）、
//! `Custom` 图元（需要宿主的规格注册表）。
//!
//! # 坐标系
//!
//! 输入是 zela 的**屏幕坐标**（原点左上、y 向下、单位像素）；上游 2D 视图的世界坐标
//! 是**以画面中心为原点、y 向上**（见 `api::view_2d`）。换算是
//! `world = (placed − canvas/2)` 且 y 取负，在 CPU 侧一次完成并烘进控制点。

use ze_notion::{Color32, Path2D, PathCommand, Transform2D, Vec2};
use ze_render_ir::{Shape2D, Style2D};

use crate::api::{render_2d, FilledPath};

/// 一条待映射的图元（与后端的 `DrawItem` 同构，但不依赖 `zela-render`）。
#[derive(Debug, Clone)]
pub struct SceneItem<'a> {
    pub shape: &'a Shape2D,
    pub transform: Transform2D,
    pub style: Style2D,
}

/// 映射失败（显式，不静默）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SceneMapError {
    #[error("矢量后端暂不支持 {what}（B4 只映射填充族，见 GAPS V2-06）")]
    Unsupported { what: String },
}

/// 屏幕坐标（经 `transform` 放置后）→ 上游 2D 世界坐标。
fn to_world(p: Vec2, m: &Transform2D, center: Vec2) -> [f64; 3] {
    let placed = m.matrix.transform_point2(p) - center;
    [placed.x as f64, -(placed.y as f64), 0.0]
}

/// 直线段 → 三次曲线（四等分点）。
fn line_curve(a: Vec2, b: Vec2, m: &Transform2D, center: Vec2) -> [[f64; 3]; 4] {
    let lerp = |t: f32| a + (b - a) * t;
    [
        to_world(a, m, center),
        to_world(lerp(1.0 / 3.0), m, center),
        to_world(lerp(2.0 / 3.0), m, center),
        to_world(b, m, center),
    ]
}

/// 二次贝塞尔 → 三次（de Casteljau 升阶）。
fn quad_curve(a: Vec2, ctrl: Vec2, b: Vec2, m: &Transform2D, center: Vec2) -> [[f64; 3]; 4] {
    let c1 = a + (ctrl - a) * (2.0 / 3.0);
    let c2 = b + (ctrl - b) * (2.0 / 3.0);
    [
        to_world(a, m, center),
        to_world(c1, m, center),
        to_world(c2, m, center),
        to_world(b, m, center),
    ]
}

/// 椭圆弧 → 若干 ≤90° 的三次曲线（标准 `4/3·tan(Δ/4)` 控制点）。
fn arc_curves(
    at: Vec2,
    radii: Vec2,
    rotation: f32,
    start: f32,
    end: f32,
    m: &Transform2D,
    center: Vec2,
) -> Vec<[[f64; 3]; 4]> {
    let (sin_r, cos_r) = rotation.sin_cos();
    let point = |t: f32| {
        let (s, c) = t.sin_cos();
        Vec2::new(
            at.x + radii.x * c * cos_r - radii.y * s * sin_r,
            at.y + radii.x * c * sin_r + radii.y * s * cos_r,
        )
    };
    let derivative = |t: f32| {
        let (s, c) = t.sin_cos();
        Vec2::new(
            -radii.x * s * cos_r - radii.y * c * sin_r,
            -radii.x * s * sin_r + radii.y * c * cos_r,
        )
    };
    let sweep = end - start;
    let segments = ((sweep.abs() / std::f32::consts::FRAC_PI_2).ceil() as usize).max(1);
    let step = sweep / segments as f32;
    let alpha = 4.0 / 3.0 * (step / 4.0).tan();
    (0..segments)
        .map(|i| {
            let (t0, t1) = (start + step * i as f32, start + step * (i + 1) as f32);
            let (p0, p3) = (point(t0), point(t1));
            let p1 = p0 + derivative(t0) * alpha;
            let p2 = p3 - derivative(t1) * alpha;
            [
                to_world(p0, m, center),
                to_world(p1, m, center),
                to_world(p2, m, center),
                to_world(p3, m, center),
            ]
        })
        .collect()
}

/// 收集一条路径的全部子路径（曲线段 + `[首曲线, 末曲线, 是否闭合, 0]`）。
fn path_subpaths(
    path: &Path2D,
    m: &Transform2D,
    center: Vec2,
) -> (Vec<[f64; 3]>, Vec<[u32; 4]>) {
    let mut points: Vec<[f64; 3]> = Vec::new();
    let mut ranges: Vec<[u32; 4]> = Vec::new();
    let push = |curve: [[f64; 3]; 4], points: &mut Vec<[f64; 3]>| {
        points.extend_from_slice(&curve);
    };
    let flush = |points: &Vec<[f64; 3]>, ranges: &mut Vec<[u32; 4]>, from: u32, closed: bool| {
        let to = (points.len() / 4) as u32;
        if to > from {
            ranges.push([from, to, u32::from(closed), 0]);
        }
    };
    let mut start: Option<Vec2> = None;
    let mut current: Option<Vec2> = None;
    let mut range_start = 0u32;
    let mut closed = false;

    for command in &path.commands {
        match command {
            PathCommand::MoveTo(p) => {
                if let Some(previous) = current {
                    if previous != *p {
                        // 未闭合的上一段：先登记，再开新段。
                        flush(&points, &mut ranges, range_start, closed);
                        range_start = (points.len() / 4) as u32;
                    }
                }
                start = Some(*p);
                current = Some(*p);
                closed = false;
            }
            PathCommand::LineTo(p) => {
                if let Some(from) = current {
                    push(line_curve(from, *p, m, center), &mut points);
                }
                current = Some(*p);
            }
            PathCommand::QuadTo { ctrl, end } => {
                if let Some(from) = current {
                    push(quad_curve(from, *ctrl, *end, m, center), &mut points);
                }
                current = Some(*end);
            }
            PathCommand::CubicTo { ctrl1, ctrl2, end } => {
                if let Some(from) = current {
                    push(
                        [
                            to_world(from, m, center),
                            to_world(*ctrl1, m, center),
                            to_world(*ctrl2, m, center),
                            to_world(*end, m, center),
                        ],
                        &mut points,
                    );
                }
                current = Some(*end);
            }
            PathCommand::Arc {
                center: arc_at,
                radii,
                rotation,
                start_angle,
                end_angle,
            } => {
                for curve in arc_curves(
                    *arc_at,
                    *radii,
                    *rotation,
                    *start_angle,
                    *end_angle,
                    m,
                    center,
                ) {
                    push(curve, &mut points);
                }
                current = Some(*arc_at);
            }
            PathCommand::Close => {
                if let (Some(s), Some(c)) = (start, current) {
                    if s != c {
                        push(line_curve(c, s, m, center), &mut points);
                    }
                    current = Some(s);
                }
                closed = true;
                flush(&points, &mut ranges, range_start, closed);
                range_start = (points.len() / 4) as u32;
                closed = false;
            }
        }
    }
    // 尾部未闭合的子路径也要登记（填充按隐式闭合处理）。
    flush(&points, &mut ranges, range_start, closed);
    (points, ranges)
}

/// 闭多边形 → 一条闭合子路径。
fn polygon_path(points: &[Vec2], m: &Transform2D, center: Vec2) -> FilledPath {
    let n = points.len();
    let mut curves = Vec::with_capacity(n * 4);
    for i in 0..n {
        curves.extend_from_slice(&line_curve(points[i], points[(i + 1) % n], m, center));
    }
    FilledPath {
        points: curves,
        ranges: vec![[0, n as u32, 1, 0]],
        fill: [0.0; 4],
        model: IDENTITY,
    }
}

const IDENTITY: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// 圆弧常量 `4/3·(√2−1)`：把一个四分之一圆用三次曲线逼近。
const KAPPA: f32 = 0.552_284_75;

/// 圆 / 椭圆 → 4 条三次曲线。
fn ellipse_path(at: Vec2, rx: f32, ry: f32, m: &Transform2D, center: Vec2) -> FilledPath {
    let (cx, cy) = (at.x, at.y);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let curves = [
        // 右 → 下
        [
            Vec2::new(cx + rx, cy),
            Vec2::new(cx + rx, cy + ky),
            Vec2::new(cx + kx, cy + ry),
            Vec2::new(cx, cy + ry),
        ],
        // 下 → 左
        [
            Vec2::new(cx, cy + ry),
            Vec2::new(cx - kx, cy + ry),
            Vec2::new(cx - rx, cy + ky),
            Vec2::new(cx - rx, cy),
        ],
        // 左 → 上
        [
            Vec2::new(cx - rx, cy),
            Vec2::new(cx - rx, cy - ky),
            Vec2::new(cx - kx, cy - ry),
            Vec2::new(cx, cy - ry),
        ],
        // 上 → 右
        [
            Vec2::new(cx, cy - ry),
            Vec2::new(cx + kx, cy - ry),
            Vec2::new(cx + rx, cy - ky),
            Vec2::new(cx + rx, cy),
        ],
    ];
    FilledPath {
        points: curves
            .iter()
            .flat_map(|curve| curve.iter().map(|p| to_world(*p, m, center)))
            .collect(),
        ranges: vec![[0, 4, 1, 0]],
        fill: [0.0; 4],
        model: IDENTITY,
    }
}

fn rgba(color: Color32) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ]
}

fn push_shape(
    shape: &Shape2D,
    transform: &Transform2D,
    fill: [f32; 4],
    center: Vec2,
    out: &mut Vec<FilledPath>,
) -> Result<(), SceneMapError> {
    match shape {
        Shape2D::Empty => {}
        // 线段没有面积：填充语义为空（与 wgpu 侧三角化一致），不是降级。
        Shape2D::Line { .. } => {}
        Shape2D::Circle { center: at, radius } => {
            out.push(ellipse_path(*at, *radius, *radius, transform, center));
        }
        Shape2D::Ellipse { center: at, rx, ry } => {
            out.push(ellipse_path(*at, *rx, *ry, transform, center));
        }
        Shape2D::Rect(rect) => {
            let (min, max) = (rect.min, rect.max);
            let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
            out.push(polygon_path(&corners, transform, center));
        }
        Shape2D::Polygon { points } | Shape2D::Polyline { points } => {
            if points.len() >= 3 {
                out.push(polygon_path(points, transform, center));
            }
        }
        Shape2D::Path(path) => {
            let (points, ranges) = path_subpaths(path, transform, center);
            if !ranges.is_empty() {
                out.push(FilledPath {
                    points,
                    ranges,
                    fill: [0.0; 4],
                    model: IDENTITY,
                });
            }
        }
        Shape2D::Group { children } => {
            for child in children {
                push_shape(child, transform, fill, center, out)?;
            }
        }
        Shape2D::Custom(_) => {
            return Err(SceneMapError::Unsupported {
                what: "Custom 图元（需要宿主的规格注册表）".to_string(),
            });
        }
        // `Shape2D` 是 `#[non_exhaustive]`：新增变体落在这里 ⇒ 显式失败，不静默丢图元。
        other => {
            return Err(SceneMapError::Unsupported {
                what: format!("图元 `{}`", other.kind_name()),
            });
        }
    }
    Ok(())
}

/// 把一批图元映射成填充路径（顺序原样保留 = 绘制序）。
///
/// `canvas`：画布像素尺寸 —— 世界坐标以画面中心为原点，故必须知道中心。
///
/// 规则：
/// - `style.stroke` 有值 ⇒ [`SceneMapError::Unsupported`]（描边未映射，不静默丢弃）；
/// - `style.fill` 为空 ⇒ 该图元不产生绘制（`.stroke()` 专用图元会被上面的规则先拦下）；
/// - `Group` 递归展开（共享同一 `transform`）。
pub fn fill_paths(
    items: &[SceneItem<'_>],
    canvas: (u32, u32),
) -> Result<Vec<FilledPath>, SceneMapError> {
    let center = Vec2::new(canvas.0 as f32 * 0.5, canvas.1 as f32 * 0.5);
    let mut out = Vec::new();
    for item in items {
        if item.style.stroke.is_some() {
            return Err(SceneMapError::Unsupported {
                what: format!(
                    "描边（width = {}；cap / joint 语义尚未与上游 Paint 对齐）",
                    item.style.stroke_width
                ),
            });
        }
        let Some(fill) = item.style.fill else {
            continue;
        };
        let before = out.len();
        push_shape(item.shape, &item.transform, rgba(fill), center, &mut out)?;
        for path in &mut out[before..] {
            path.fill = rgba(fill);
        }
    }
    Ok(out)
}

/// 离屏渲染一帧 zela 的 2D 图元（`fill_paths` + [`render_2d`]）。
pub fn render_scene_2d(
    width: u32,
    height: u32,
    samples: u32,
    background: [f32; 4],
    items: &[SceneItem<'_>],
) -> Result<Vec<u8>, String> {
    let paths = fill_paths(items, (width, height)).map_err(|error| error.to_string())?;
    render_2d(width, height, samples, background, &paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_render_ir::FillSolid;

    const CANVAS: (u32, u32) = (128, 128);

    fn style(fill: Color32) -> Style2D {
        Style2D {
            fill: Some(fill),
            stroke: None,
            stroke_width: 0.0,
            space: ze_notion::MeasureSpace::Screen,
        }
    }

    fn translated(x: f32, y: f32) -> Transform2D {
        Transform2D::translate(Vec2::new(x, y))
    }

    #[test]
    fn rect_maps_to_one_closed_subpath() {
        let shape = Shape2D::Rect(ze_notion::Rect2D::new(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 4.0),
        ));
        let item = SceneItem {
            shape: &shape,
            transform: Transform2D::identity(),
            style: style(Color32::RED),
        };
        let paths = fill_paths(&[item], CANVAS).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].points.len(), 16, "4 段 × 4 控制点");
        assert_eq!(paths[0].ranges.len(), 1);
        assert_eq!(paths[0].ranges[0][2], 1, "闭合");
        assert_eq!(paths[0].fill, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn circle_maps_to_four_cubics_and_centers_the_canvas() {
        let shape = Shape2D::Circle {
            center: Vec2::new(64.0, 64.0),
            radius: 10.0,
        };
        let item = SceneItem {
            shape: &shape,
            transform: Transform2D::identity(),
            style: style(Color32::RED),
        };
        let paths = fill_paths(&[item], CANVAS).unwrap();
        assert_eq!(paths[0].points.len(), 16);
        // 画布中心的圆 ⇒ 世界坐标原点为中心的半径 10。
        let max_x = paths[0].points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        assert!((max_x - 10.0).abs() < 1e-3, "最右点应为 10：{max_x}");
        let min_y = paths[0].points.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!((min_y - -10.0).abs() < 1e-3, "最下点应为 -10：{min_y}");
    }

    #[test]
    fn y_axis_is_flipped_relative_to_screen() {
        // 屏幕 (72, 64) ⇒ 世界 (8, 0)；屏幕 y 更大 ⇒ 世界 y 更小（向上）。
        let top = Shape2D::Circle {
            center: Vec2::new(64.0, 44.0),
            radius: 2.0,
        };
        let bottom = Shape2D::Circle {
            center: Vec2::new(64.0, 84.0),
            radius: 2.0,
        };
        let items = [
            SceneItem {
                shape: &top,
                transform: Transform2D::identity(),
                style: style(Color32::RED),
            },
            SceneItem {
                shape: &bottom,
                transform: Transform2D::identity(),
                style: style(Color32::RED),
            },
        ];
        let paths = fill_paths(&items, CANVAS).unwrap();
        let mean_y = |path: &FilledPath| {
            path.points.iter().map(|p| p[1]).sum::<f64>() / path.points.len() as f64
        };
        assert!(mean_y(&paths[0]) > 0.0, "屏幕上方 ⇒ 世界 +y");
        assert!(mean_y(&paths[1]) < 0.0, "屏幕下方 ⇒ 世界 -y");
    }

    #[test]
    fn transform_is_baked_into_control_points() {
        let shape = Shape2D::Circle {
            center: Vec2::ZERO,
            radius: 5.0,
        };
        let item = SceneItem {
            shape: &shape,
            transform: translated(100.0, 0.0),
            style: style(Color32::RED),
        };
        let paths = fill_paths(&[item], CANVAS).unwrap();
        let max_x = paths[0].points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        // 100 − 64（中心）+ 5 = 41。
        assert!((max_x - 41.0).abs() < 1e-3, "平移 + 居中应烘进控制点：{max_x}");
        // model 保持单位阵（不用上游的 GPU 插值路径，见 GAPS V2-03）。
        assert_eq!(paths[0].model, IDENTITY);
    }

    #[test]
    fn quad_becomes_an_equivalent_cubic() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(64.0, 64.0));
        path.quad_to(Vec2::new(74.0, 74.0), Vec2::new(84.0, 64.0));
        let shape = Shape2D::Path(path);
        let item = SceneItem {
            shape: &shape,
            transform: Transform2D::identity(),
            style: style(Color32::RED),
        };
        let paths = fill_paths(&[item], CANVAS).unwrap();
        let curve = &paths[0].points[..4];
        // c1 = p0 + 2/3(ctrl − p0) = (70.667, 70.667) ⇒ 世界 (6.667, −6.667)。
        assert!((curve[1][0] - 20.0 / 3.0).abs() < 1e-3, "{curve:?}");
        assert!((curve[1][1] - -20.0 / 3.0).abs() < 1e-3, "y 已翻转为负");
        assert!((curve[2][0] - 40.0 / 3.0).abs() < 1e-3);
    }

    #[test]
    fn stroke_only_items_fail_explicitly() {
        let shape = Shape2D::Circle {
            center: Vec2::ZERO,
            radius: 1.0,
        };
        let item = SceneItem {
            shape: &shape,
            transform: Transform2D::identity(),
            style: Style2D {
                fill: None,
                stroke: Some(Color32::RED),
                stroke_width: 2.0,
                space: ze_notion::MeasureSpace::Screen,
            },
        };
        let error = fill_paths(&[item], CANVAS).unwrap_err();
        assert!(matches!(error, SceneMapError::Unsupported { .. }));
        assert!(error.to_string().contains("描边"), "{error}");
    }

    #[test]
    fn custom_shapes_fail_explicitly() {
        let shape = Shape2D::Custom(ze_render_ir::CustomShapeData::new(
            "test.spec",
            ze_notion::Rect2D::new(Vec2::ZERO, Vec2::splat(1.0)),
        ));
        let item = SceneItem {
            shape: &shape,
            transform: Transform2D::identity(),
            style: style(Color32::RED),
        };
        let error = fill_paths(&[item], CANVAS).unwrap_err();
        assert!(error.to_string().contains("Custom"), "{error}");
    }

    #[test]
    fn fill_style_becomes_straight_rgba() {
        let solid = FillSolid::new(Color32::RED);
        assert_eq!(solid.color.r, 255);
    }
}
