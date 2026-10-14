// components.rs
//
// 从 xcal_core::scene::components 重导出 marker/mesh + Circle/Path/Line/PhaseAnim，
// 并新增 render 子模块（StrokeSolid / FillSolid），因为这些组件依赖 Color32，
// 而 Color32 在 xcal_core::types 中。因此渲染样式组件归入 xcal-render。

pub use xcal_core::scene::components::{
    CPUDevice, Circle, FunctionX2Y, Interpolator, Line, ParameterCurveT2XY, ParameterCurveT2XYZ,
    Parent, Path, PathMesh, PhaseAnim, QuadMesh, Sampler1D, Sampler2D, SyncSendFn, Transform,
    TriangleMesh, WGPUDevice, marker, mesh,
};

pub mod render {
    use xcal_core::types::Color32;
    use xc_geometry::MeasureSpace;

    /// 描边样式组件：颜色 + 宽度 + 宽度所属空间。
    ///
    /// `space` 声明宽度的空间归属（默认 [`MeasureSpace::Screen`]）；
    /// **唯一换算点**是 [`crate::draw::record_scene`]（结合 `camera.scale` 解析）。
    #[derive(Debug, Clone)]
    pub struct StrokeSolid {
        pub width: f32,
        pub color: Color32,
        pub space: MeasureSpace,
    }

    impl StrokeSolid {
        /// 两参构造：`space` 取默认 [`MeasureSpace::Screen`]（既有调用点行为不变）。
        pub fn new(width: f32, color: impl Into<Color32>) -> Self {
            Self {
                width,
                color: color.into(),
                space: MeasureSpace::Screen,
            }
        }

        /// 显式指定度量空间的构造。
        pub fn new_in(width: f32, color: impl Into<Color32>, space: MeasureSpace) -> Self {
            Self {
                width,
                color: color.into(),
                space,
            }
        }
    }

    #[derive(Debug, Clone, derive_new::new)]
    pub struct FillSolid {
        #[new(into)]
        pub color: Color32,
    }

    pub enum FillStyle {
        Solid(FillSolid),
    }
    pub enum StrokeStyle {
        Solid(StrokeSolid),
    }
}

pub use render::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// 两参构造默认 `Screen`（保证现有调用点行为不变）。
    #[test]
    fn stroke_solid_default_construction_is_screen() {
        let s = StrokeSolid::new(2.0, (255, 0, 0, 255));
        assert_eq!(s.space, xc_geometry::MeasureSpace::Screen);
        assert_eq!(s.width, 2.0);

        let w = StrokeSolid::new_in(1.0, (0, 0, 0, 255), xc_geometry::MeasureSpace::World);
        assert_eq!(w.space, xc_geometry::MeasureSpace::World);
    }
}
