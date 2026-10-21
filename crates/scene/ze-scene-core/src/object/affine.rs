// affine.rs — 相机降级仿射上的几何映射（场景对象层的公共实现）。
//
// **为什么需要它**：`ze-render-ir::ToShape2D` 吃的是**调用点降级出的仿射矩阵**
// （见 `ze_render_ir::degrade`），而不是 `Camera2D`。于是"点对点映射"之外的量
// —— `Circle` 的半径、`Path2D::Arc` 的 radii —— 必须从矩阵里把**统一缩放因子**
// 取回来。相机降级矩阵的线性部分恒为 `diag(s, -s)`（`s` = `Camera2D.scale`）。
//
// 这里集中放这份映射，供：
// - `ToShape2D` impl（本层，屏幕方向）；
// - `zela-scene` 的 `Transform2DInto`（相机方向，含反向）—— 单一实现，不各写一份。

use ze_notion::{Path2D, PathCommand, Transform2D};

/// 从相机降级仿射中取回统一缩放因子 `s`（线性部分 = `diag(s, -s)`）。
pub fn uniform_scale(t: &Transform2D) -> f32 {
    t.matrix.x_axis.x
}

/// 相机仿射下的 `Path2D` 映射（**解析改写**，不折线采样）。
///
/// - 点类命令（MoveTo / LineTo / QuadTo / CubicTo）：逐点 `t.apply_point`；
///   —— 仿射变换下贝塞尔保持形状，故控制点逐点映射即精确；
/// - `Close`：原样保留（闭合语义不因变换丢失）；
/// - `Arc`：椭圆弧在 `diag(s, -s)` 下仍是椭圆弧，故**解析改写**：
///   `center' = A(center)`、`radii' = radii * s`、`rotation' = -rotation`、
///   `start/end_angle'` 取负。
///   依据恒等式 `diag(1,-1)·R(rot) = R(-rot)·diag(1,-1)`
///   ⇒ `A·R(rot)·diag(rx, ry)·(cosθ, sinθ) = R(-rot)·diag(s·rx, s·ry)·(cos(-θ), sin(-θ))`
///   —— 精确无信息损失（对比 `Path2D::transformed` 的 32 段折线近似）。
///
/// 重建 `Path2D` 时 `bounds` 重置为 `None`（懒计算缓存随变换失效）。
pub fn map_path_affine(path: &Path2D, t: &Transform2D) -> Path2D {
    let s = uniform_scale(t);
    let commands = path
        .commands
        .iter()
        .copied()
        .map(|cmd| match cmd {
            PathCommand::MoveTo(p) => PathCommand::MoveTo(t.apply_point(p)),
            PathCommand::LineTo(p) => PathCommand::LineTo(t.apply_point(p)),
            PathCommand::QuadTo { ctrl, end } => PathCommand::QuadTo {
                ctrl: t.apply_point(ctrl),
                end: t.apply_point(end),
            },
            PathCommand::CubicTo { ctrl1, ctrl2, end } => PathCommand::CubicTo {
                ctrl1: t.apply_point(ctrl1),
                ctrl2: t.apply_point(ctrl2),
                end: t.apply_point(end),
            },
            PathCommand::Arc {
                center,
                radii,
                rotation,
                start_angle,
                end_angle,
            } => PathCommand::Arc {
                center: t.apply_point(center),
                radii: radii * s,
                rotation: -rotation,
                start_angle: -start_angle,
                end_angle: -end_angle,
            },
            PathCommand::Close => PathCommand::Close,
        })
        .collect();
    Path2D {
        commands,
        bounds: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::{Mat3, Vec2, Vec3};

    /// 相机降级矩阵：`diag(s, -s)` + 平移（与 `Camera2D::to_transform2d` 同式）。
    fn camera_like(s: f32, tx: f32, ty: f32) -> Transform2D {
        Transform2D {
            matrix: Mat3::from_cols(
                Vec3::new(s, 0.0, 0.0),
                Vec3::new(0.0, -s, 0.0),
                Vec3::new(tx, ty, 1.0),
            ),
        }
    }

    #[test]
    fn uniform_scale_reads_diagonal() {
        assert_eq!(uniform_scale(&camera_like(12.0, 100.0, 50.0)), 12.0);
    }

    /// 圆（用 512 边形逼近）在相机仿射下半径按 `s` 缩放，Y 翻转。
    #[test]
    fn path_affine_scales_and_flips() {
        let mut p = Path2D::new();
        p.move_to(Vec2::new(1.0, 0.0)).line_to(Vec2::new(0.0, 1.0));
        let t = camera_like(10.0, 100.0, 200.0);
        let out = map_path_affine(&p, &t);
        assert_eq!(out.commands[0], PathCommand::MoveTo(Vec2::new(110.0, 200.0)));
        assert_eq!(out.commands[1], PathCommand::LineTo(Vec2::new(100.0, 190.0)));
    }

    /// `Close` 保留（闭合语义不丢）。
    #[test]
    fn path_affine_keeps_close() {
        let p = Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(1.0, 0.0)], true);
        let out = map_path_affine(&p, &camera_like(2.0, 0.0, 0.0));
        assert_eq!(out.commands.last(), Some(&PathCommand::Close));
    }

    /// `Arc` 解析改写：radii × s、rotation / 角度取负（不折线采样）。
    #[test]
    fn path_affine_rewrites_arc_analytically() {
        let mut p = Path2D::new();
        p.move_to(Vec2::ZERO)
            .arc_to(Vec2::ZERO, Vec2::new(1.0, 2.0), 0.5, 1.0, 2.0);
        let out = map_path_affine(&p, &camera_like(4.0, 0.0, 0.0));
        match out.commands[1] {
            PathCommand::Arc {
                radii,
                rotation,
                start_angle,
                end_angle,
                ..
            } => {
                assert_eq!(radii, Vec2::new(4.0, 8.0));
                assert_eq!(rotation, -0.5);
                assert_eq!(start_angle, -1.0);
                assert_eq!(end_angle, -2.0);
            }
            other => panic!("Arc 必须保持为 Arc 命令，得到 {other:?}"),
        }
    }
}
