// types.rs
//
// 数学包装类型：Vec2, Vec3, Vec4, Color32, Rect, Camera2D。
//
// M1 T6①：`I8Vec4` / `U8Vec4` 已收回理念层 `xc-notion::vec`（全仓零外部消费方）。
// 二者此前是本文件的 `export_vec!` newtype，是"每层各自依赖 glam"的重复定义；
// 收回后（含 tuple 转换）零下游影响。

macro_rules! export_type {
    ($(#[$m:meta])? $v:vis $name:ident($ty:ty)) => {
        #[derive(derive_more::From, derive_more::Deref, derive_more::DerefMut)]
        $(#[$m])?
        $v struct $name($ty);
    };
}
macro_rules! export_vec {
    ($id:ident) => {
        export_type! {
            #[derive(Clone,Copy,derive_new::new,Debug)]
           pub $id (glam::$id)
        }
    };
    ($id:ident, $($extra:meta),*) => {
        export_type! {
            #[derive(Clone,Copy,derive_new::new,Debug, $($extra),*)]
           pub $id (glam::$id)
        }
    };
}

export_vec!(Vec2, xc_macro::Transform2DInto);

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Self {
        glam::Vec2::new(x, y).into()
    }
}

export_vec!(Vec3, xc_macro::Transform2DInto);

impl From<(f32, f32, f32)> for Vec3 {
    fn from((x, y, z): (f32, f32, f32)) -> Self {
        glam::Vec3::new(x, y, z).into()
    }
}

export_vec!(Vec4);

impl From<(f32, f32, f32, f32)> for Vec4 {
    fn from((x, y, z, w): (f32, f32, f32, f32)) -> Self {
        glam::Vec4::new(x, y, z, w).into()
    }
}

// Color32 的规范定义位于 xc-notion（理念层），此处再导出，使 ECS 组件、
// 2D 变换体系与渲染层共用同一类型。egui/wgpu 等后端转换由各后端 crate 负责
// （孤儿规则：xcal-scene 不能为外部类型实现外部 trait）。
pub use xc_notion::Color32;

#[derive(Debug, Clone, Copy, derive_new::new, xc_macro::Transform2DInto)]
pub struct Rect {
    min: Vec2,
    max: Vec2,
}

// 公开访问器以便 Rect 字段在私有情况下仍可使用
impl Rect {
    pub fn min(&self) -> Vec2 {
        self.min
    }
    pub fn max(&self) -> Vec2 {
        self.max
    }
}

impl Rect {
    pub fn size(&self) -> Vec2 {
        (*self.max - *self.min).abs().into()
    }
    pub fn center(&self) -> Vec2 {
        ((*self.max + *self.min) / 2.0).into()
    }
}

/// 视口矩形（本层）→ 绘制用矩形对象（`xc-object`）。
///
/// 二者**同名不同物**：`xcal_scene::types::Rect` 是相机视口矩形（场景层）；
/// `xc_object::Rect` 是可绘制的矩形图元（场景对象层）。转换实现放在本层 ——
/// 场景层依赖对象层，反向不允许，故对象层不能知道视口矩形。
impl From<Rect> for xc_object::Rect {
    fn from(r: Rect) -> Self {
        xc_object::Rect::new(*r.min, *r.max)
    }
}

/// 2D 相机参数：用于在屏幕坐标系和逻辑坐标系之间转换。
///
/// - 屏幕坐标系：以视口左上角为原点，y 向下（Y-down），单位为像素。
/// - 逻辑坐标系：以 `center` 为相机中心，y 向上（Y-up，数学惯例），单位为世界单位。
///
/// `scale` 方向约定（易踩坑）：**1 世界单位 = `scale` 像素**，scale 越大
/// 物体越大；这与未来统一相机的正交投影 `extent`（垂直半高，越大看到越多、
/// 物体**越小**）方向**相反**——薄壳层（Camera2D = Camera3D + Orthographic）
/// 负责两者换算。屏幕 Y-down ↔ 世界 Y-up 的翻转只在 Transform2D 的
/// glam::Vec2 实现中存在一份，本类型不另造换算。
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera2D {
    /// 逻辑坐标系中相机中心位置
    pub center: Vec2,
    /// 缩放系数：1 世界单位 = `scale` 像素（越大物体越大）
    pub scale: f32,
    /// 屏幕视口矩形（像素）
    pub rect: Rect,
}

impl Camera2D {
    /// 屏幕坐标 → 场景（逻辑）坐标。UI 适配器把原生指针事件换算为
    /// 场景语义事件时使用。
    pub fn screen_to_world(&self, p: Vec2) -> Vec2 {
        use crate::transform::Transform2D;
        p.to_logical(self)
    }

    /// 场景（逻辑）坐标 → 屏幕坐标。
    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        use crate::transform::Transform2D;
        p.to_screen(self)
    }

    /// 把相机**降级为「世界 → 屏幕」的仿射矩阵**（M2-3）。
    ///
    /// 这是渲染 IR 层唯一被允许看到的相机形态：`ToShape2D` 只吃
    /// `xc_notion::Transform2D`，不吃 `Camera2D`（否则 IR 层要依赖场景对象层，成环）。
    ///
    /// 精确形式（与 [`Camera2D::world_to_screen`] 同式）：
    ///
    /// ```text
    /// screen.x = rect.center.x + (world.x - center.x) * scale
    /// screen.y = rect.center.y - (world.y - center.y) * scale      // Y 翻转
    /// ```
    ///
    /// 作为 `Mat3`（列主序）：
    ///
    /// ```text
    /// col0 = [ s,  0, 0 ]
    /// col1 = [ 0, -s, 0 ]
    /// col2 = [ rc.x - c.x·s,  rc.y + c.y·s,  1 ]
    /// ```
    ///
    /// 回归基线：`camera_transform2d_matches_world_to_screen` 断言两式一致
    /// （同时保留既有的 `screen_to_world` / `world_to_screen` 往返测试）。
    pub fn to_transform2d(&self) -> xc_notion::Transform2D {
        let rc = self.rect.center();
        let s = self.scale;
        let c = self.center;
        xc_notion::Transform2D {
            matrix: glam::Mat3::from_cols(
                glam::Vec3::new(s, 0.0, 0.0),
                glam::Vec3::new(0.0, -s, 0.0),
                glam::Vec3::new(rc.x - c.x * s, rc.y + c.y * s, 1.0),
            ),
        }
    }
}

#[cfg(test)]
mod tests {

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_vec2_from_tuple() {
        let v: Vec2 = (1.0_f32, 2.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
    }

    #[test]
    fn test_vec3_from_tuple() {
        let v: Vec3 = (1.0_f32, 2.0, 3.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
    }

    #[test]
    fn test_vec4_from_tuple() {
        let v: Vec4 = (1.0_f32, 2.0, 3.0, 4.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
        assert_eq!(v.w, 4.0);
    }

    #[test]
    fn test_color32_from_tuple() {
        let c: Color32 = (10_u8, 20, 30, 40).into();
        assert_eq!(c.r(), 10);
        assert_eq!(c.g(), 20);
        assert_eq!(c.b(), 30);
        assert_eq!(c.a(), 40);
        assert_eq!(c.r, 10);
    }

    #[test]
    fn test_rect_size() {
        let min: Vec2 = (0.0, 0.0).into();
        let max: Vec2 = (10.0, 20.0).into();
        let r = Rect::new(min, max);
        let size = r.size();
        assert_eq!(size.x, 10.0);
        assert_eq!(size.y, 20.0);
    }

    #[test]
    fn test_rect_center() {
        let min: Vec2 = (0.0, 0.0).into();
        let max: Vec2 = (10.0, 20.0).into();
        let r = Rect::new(min, max);
        let center = r.center();
        assert_eq!(center.x, 5.0);
        assert_eq!(center.y, 10.0);
    }

    #[test]
    fn test_rect_size_negative_coords() {
        let min: Vec2 = (-5.0, -10.0).into();
        let max: Vec2 = (5.0, 10.0).into();
        let r = Rect::new(min, max);
        let size = r.size();
        assert_eq!(size.x, 10.0);
        assert_eq!(size.y, 20.0);
    }

    /// 坐标换算往返：screen_to_world(to_screen(p)) == p（误差 < 1e-5）。
    #[test]
    fn test_camera2d_screen_world_roundtrip() {
        let camera = Camera2D::new(
            (-3.5, 7.25).into(),
            12.0,
            Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        );

        for wp in [
            (0.0, 0.0).into(),
            (-100.0, 42.0).into(),
            (3.7, -9.9).into(),
        ] {
            let sp = camera.world_to_screen(wp);
            let back = camera.screen_to_world(sp);
            assert!(
                (back.x - wp.x).abs() < 1e-5 && (back.y - wp.y).abs() < 1e-5,
                "world roundtrip failed: {wp:?} -> {sp:?} -> {back:?}"
            );
        }

        for sp in [
            (10.0, 590.0).into(),
            (400.0, 300.0).into(),
            (795.0, 3.0).into(),
        ] {
            let wp = camera.screen_to_world(sp);
            let back = camera.world_to_screen(wp);
            assert!(
                (back.x - sp.x).abs() < 1e-5 && (back.y - sp.y).abs() < 1e-5,
                "screen roundtrip failed: {sp:?} -> {wp:?} -> {back:?}"
            );
        }
    }

    /// M2-3 回归基线：相机降级矩阵 `to_transform2d()` 与 `world_to_screen` 逐点一致。
    ///
    /// 容差 1e-2 像素：两式只差浮点结合律（`rc.x + (wx - cx)*s` vs
    /// `s*wx + (rc.x - cx*s)`），量级 ~1e4 时 fp32 的 ulp ≈ 1e-3。
    #[test]
    fn camera_transform2d_matches_world_to_screen() {
        let camera = Camera2D::new(
            (-3.5, 7.25).into(),
            12.0,
            Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        );
        let t = camera.to_transform2d();

        for wp in [
            (0.0, 0.0).into(),
            (-100.0, 42.0).into(),
            (3.7, -9.9).into(),
            (800.0, -600.0).into(),
        ] {
            let a = camera.world_to_screen(wp);
            let b = t.apply_point(*wp);
            assert!(
                (a.x - b.x).abs() < 1e-2 && (a.y - b.y).abs() < 1e-2,
                "仿射与 world_to_screen 不一致: {wp:?} -> {a:?} vs {b:?}"
            );
        }
    }
}
