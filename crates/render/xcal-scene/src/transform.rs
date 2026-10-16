// transform.rs
//
// 数学部分：Transform2D / Transform2DInto trait
// + glam 底层类型的 Transform2DInto 实现
// + Circle 的手动 Transform2DInto 实现（需要 radius 按 scale 放缩）
// + Path2D 的手动 Transform2DInto 实现（逐命令映射 + 椭圆弧解析改写）
//
// 注：Painter2D/DrawQueue/SceneRender 等"绘制"部分属于 xcal-render，
// 不放在这里。

use crate::scene::components::Circle;
use crate::types::{Camera2D, Vec2};
use xc_notion::{Path2D, PathCommand};

/// 2D 坐标系转换 Trait（非消耗版本）。
///
/// 实现此 Trait 的对象可接受 [`Camera2D`] 相机参数，将自身在屏幕坐标系与
/// 逻辑坐标系之间相互转换。
///
/// - 屏幕坐标系：视口左上角为原点，y 向下，单位为像素。
/// - 逻辑坐标系：[`Camera2D::center`] 为中心，y 向上，单位为世界单位。
///
/// 对实现 [`Transform2DInto`] 且 `Clone` 的类型，本 Trait 由 blanket impl 自动提供，
/// 不需要手动实现。
pub trait Transform2D {
    /// 从屏幕坐标系转换到逻辑坐标系。
    fn to_logical(&self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系。
    fn to_screen(&self, camera: &Camera2D) -> Self;
}

/// 2D 坐标系转换 Trait（消耗 `self` 版本）。
///
/// 与 [`Transform2D`] 的区别：方法消耗 `self`，可以避免额外的 `clone`。
/// 派生方式：在结构体上添加 `#[derive(xc_macro::Transform2DInto)]`。
pub trait Transform2DInto {
    /// 从屏幕坐标系转换到逻辑坐标系（消耗 self）。
    fn to_logical_into(self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系（消耗 self）。
    fn to_screen_into(self, camera: &Camera2D) -> Self;
}

/// 自动为实现了 [`Transform2DInto`] 且 `Clone` 的类型派生 [`Transform2D`]。
impl<T: Transform2DInto + Clone> Transform2D for T {
    fn to_logical(&self, camera: &Camera2D) -> Self {
        self.clone().to_logical_into(camera)
    }
    fn to_screen(&self, camera: &Camera2D) -> Self {
        self.clone().to_screen_into(camera)
    }
}

// ─── 底层 glam 类型的 Transform2DInto 实现 ─────────────────────────────────
//
// Y 翻转单点约定：世界空间统一 Y-up（数学惯例），屏幕空间 Y-down。
// 两者之间的翻转（rect_center.y - (y - center.y) * scale）只在下面
// glam::Vec2 的实现中存在**一份**；Vec3 / Circle / 其他类型一律委托到
// Vec2（取 xy 分量），不得各自再写翻转。Camera2D::screen_to_world /
// world_to_screen 也只是薄封装，不另造换算。

impl Transform2DInto for glam::Vec2 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            (self.x - rect_center.x) / camera.scale + camera.center.x,
            (rect_center.y - self.y) / camera.scale + camera.center.y,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            rect_center.x + (self.x - camera.center.x) * camera.scale,
            rect_center.y - (self.y - camera.center.y) * camera.scale,
        )
    }
}

impl Transform2DInto for glam::Vec3 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_logical_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_screen_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }
}

// ─── Circle 的 Transform2DInto 手动实现 ─────────────────────────────────────

impl Transform2DInto for Circle {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_logical_into(camera),
            self.radius / camera.scale,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_screen_into(camera),
            self.radius * camera.scale,
        )
    }
}

// ─── Path2D 的 Transform2DInto 手动实现 ─────────────────────────────────────
//
// 相机变换是仿射 `A = diag(s, -s)`（s = scale 用于 to_screen，1/scale 用于
// to_logical）加平移。逐命令映射：
// - 点类命令（MoveTo / LineTo / QuadTo / CubicTo）：逐点委托 `glam::Vec2`
//   （Y 翻转的唯一实现点），与旧 `Path.vertices` 经 `Vec3::to_screen_into`
//   再取 xy 的语义完全一致；
// - `Close`：原样保留（闭合语义不因变换丢失）；
// - `Arc`：椭圆弧在该仿射下仍是椭圆弧，可**解析改写**而非折线采样：
//     center' = A(center)，radii' = radii * s，rotation' = -rotation，
//     start_angle' = -start_angle，end_angle' = -end_angle。
//   依据恒等式 `diag(1,-1)·R(rot) = R(-rot)·diag(1,-1)`
//   ⇒ `A·R(rot)·diag(radii.x, radii.y)·(cosθ, sinθ)`
//     `= R(-rot)·diag(s·radii.x, s·radii.y)·(cos(-θ), sin(-θ))`，精确无信息损失。
//
// 重建 `Path2D` 时 `bounds` 自然重置为 `None`（懒计算缓存随变换失效）。
fn map_path2d(path: Path2D, camera: &Camera2D, to_screen: bool) -> Path2D {
    let s = if to_screen {
        camera.scale
    } else {
        1.0 / camera.scale
    };
    let map_pt = |p: glam::Vec2| {
        if to_screen {
            p.to_screen_into(camera)
        } else {
            p.to_logical_into(camera)
        }
    };
    let commands = path
        .commands
        .into_iter()
        .map(|cmd| match cmd {
            PathCommand::MoveTo(p) => PathCommand::MoveTo(map_pt(p)),
            PathCommand::LineTo(p) => PathCommand::LineTo(map_pt(p)),
            PathCommand::QuadTo { ctrl, end } => PathCommand::QuadTo {
                ctrl: map_pt(ctrl),
                end: map_pt(end),
            },
            PathCommand::CubicTo { ctrl1, ctrl2, end } => PathCommand::CubicTo {
                ctrl1: map_pt(ctrl1),
                ctrl2: map_pt(ctrl2),
                end: map_pt(end),
            },
            PathCommand::Arc {
                center,
                radii,
                rotation,
                start_angle,
                end_angle,
            } => PathCommand::Arc {
                center: map_pt(center),
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

impl Transform2DInto for Path2D {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        map_path2d(self, camera, false)
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        map_path2d(self, camera, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::Path;
    use crate::types::Rect;

    /// 非平凡相机（非零中心 + 非零平移视口），放大数值差异。
    fn camera() -> Camera2D {
        Camera2D::new(
            (-3.5, 7.25).into(),
            12.0,
            Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        )
    }

    /// RB.3 #4：`Path` → `Path2D`（屏幕）→ `Path`（逻辑）往返不丢信息——
    /// 闭合语义（末尾 `Close`）保留，命令数不变，各点数值可逆。
    #[test]
    fn path2d_closed_roundtrip_preserves_semantics() {
        let pts = [
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(2.0, 0.0),
            glam::Vec2::new(2.0, 1.5),
            glam::Vec2::new(0.0, 1.5),
        ];
        let original = Path::new(Path2D::from_polyline(&pts, true));
        let cam = camera();

        let back = original.to_screen(&cam).to_logical(&cam);

        // 命令序列逐条对齐（数量与闭合语义都不丢）。
        assert_eq!(back.path.commands.len(), original.path.commands.len());
        assert!(
            matches!(back.path.commands.last(), Some(PathCommand::Close)),
            "闭合语义必须往返保留"
        );
        // 各点数值可逆（容差 1e-4）。
        for (a, b) in original.path.commands.iter().zip(&back.path.commands) {
            match (a, b) {
                (PathCommand::MoveTo(p0), PathCommand::MoveTo(p1))
                | (PathCommand::LineTo(p0), PathCommand::LineTo(p1)) => {
                    assert!((*p0 - *p1).length() < 1e-4, "点未往返: {p0:?} vs {p1:?}");
                }
                (PathCommand::Close, PathCommand::Close) => {}
                _ => panic!("命令类型不匹配: {a:?} vs {b:?}"),
            }
        }
    }
}
