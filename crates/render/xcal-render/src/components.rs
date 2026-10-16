// components.rs
//
// 场景组件 + 渲染样式的统一出口（facade）。
//
// M1 T4：连续数学对象（FunctionX2Y / ParameterCurve* / Sampler* / Interpolator /
// SyncSendFn）已归位理念层 `xc-notion`，本 facade 直接从理念层转发。
//
// M2：样式组件（StrokeSolid / FillSolid / FillStyle / StrokeStyle）的定义已下沉
// 渲染 IR 层 `xc-render-ir`（纯数据 + 唯一换算点），此处转发以保持
// `xcal_render::components::*` 路径不变。

pub use xcal_scene::scene::components::{
    CPUDevice, PathMesh, PhaseAnim, QuadMesh, TriangleMesh, WGPUDevice, marker, mesh,
};
pub use xc_notion::{
    FunctionX2Y, Interpolator, ParameterCurveT2XY, ParameterCurveT2XYZ, Sampler1D, Sampler2D,
    SyncSendFn,
};
// 图元对象与摆放组件（M3-4/M3-5：定义在场景对象层 `xc-object`，此处转发）。
pub use xc_object::{Circle, Line, Path, Rect, Transform};
pub use xc_render_ir::{FillSolid, FillStyle, StrokeSolid, StrokeStyle};

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::MeasureSpace;

    /// 两参构造默认 `Screen`（保证现有调用点行为不变）。
    #[test]
    fn stroke_solid_default_construction_is_screen() {
        let s = StrokeSolid::new(2.0, (255, 0, 0, 255));
        assert_eq!(s.space, MeasureSpace::Screen);
        assert_eq!(s.width, 2.0);

        let w = StrokeSolid::new_in(1.0, (0, 0, 0, 255), MeasureSpace::World);
        assert_eq!(w.space, MeasureSpace::World);
    }
}
