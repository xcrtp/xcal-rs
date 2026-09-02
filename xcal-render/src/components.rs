// components.rs
//
// 从 xcal_core::scene::components 重导出 marker/mesh/animation + Circle/Path/Line，
// 并新增 render 子模块（StrokeSolid / FillSolid），因为这些组件依赖 Color32，
// 而 Color32 在 xcal_core::types 中。因此渲染样式组件归入 xcal-render。

pub use xcal_core::scene::components::{
    Circle, CircleAnim, CPUDevice, ClockState, FunctionX2Y, Interpolator, Line, LocalClock,
    ParameterCurveT2XY, ParameterCurveT2XYZ, Parent, Path, PathMesh, PhaseAnim, QuadMesh,
    Sampler1D, Sampler2D, SyncSendFn, Transform, TriangleMesh, WGPUDevice, animation, marker,
    mesh,
};

pub mod render {
    use xcal_core::types::Color32;

    #[derive(Debug, Clone, derive_new::new)]
    pub struct StrokeSolid {
        pub width: f32,
        #[new(into)]
        pub color: Color32,
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
