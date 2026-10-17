// components.rs — 场景组件与渲染样式的统一出口（**有意的兼容 facade，不是死代码**）
//
// 本文件只做**转发**，四路来源：
//   - `xcal_scene::scene::components`：纯逻辑场景组件（`PhaseAnim` / `marker` / `mesh` …）
//   - `xc_notion`：连续数学元语（`FunctionX2Y` / `Sampler1D/2D` / `Interpolator` …）
//   - `xc_object`：图元 newtype 与摆放（`Circle` / `Line` / `Path` / `Rect` / `Transform`）
//   - `xc_render_ir`：样式组件（`StrokeSolid` / `FillSolid` / `StrokeStyle` / `FillStyle`）
//
// 目的：保持 `xcal_render::components::*` 这一路径稳定 —— M1 / M2 / M3 三轮下沉
// （notion ← scene ← object ← render_ir）都靠它做到**下游零改动**。
//
// ⚠️ **不是** 死代码，**请勿删除**：下游改用各层原生路径（如 `xc_object::Circle`）后
// 本模块即可整体移除，但那是一次独立的下游 churn（不属于任何单点重构的范围）。
//
// M1 T4：连续数学对象（FunctionX2Y / ParameterCurve* / Sampler* / Interpolator /
// DynFn）已归位理念层 `xc-notion`，本 facade 直接从理念层转发。
//
// M2：样式组件（StrokeSolid / FillSolid / FillStyle / StrokeStyle）的定义已下沉
// 渲染 IR 层 `xc-render-ir`（纯数据 + 唯一换算点），此处转发以保持
// `xcal_render::components::*` 路径不变。

pub use xc_notion::{
    DynFn, FunctionX2Y, Interpolator, ParameterCurveT2XY, ParameterCurveT2XYZ, Sampler1D, Sampler2D,
};
pub use xcal_scene::scene::components::{
    CPUDevice, PathMesh, PhaseAnim, QuadMesh, TriangleMesh, WGPUDevice, marker, mesh,
};
// 图元对象与摆放组件（M3-4/M3-5：定义在场景对象层 `xc-object`，此处转发）。
pub use xc_object::{Arrow, Circle, Line, Path, Rect, Transform};
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
