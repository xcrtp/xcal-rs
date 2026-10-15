// shape_2d.wgsl
//
// S3-1：2D 图元几何着色器（AaModel::None + MSAA）。
//
// 顶点布局**来自契约**（xcal_render::vertex::VertexLayout::standard，stride 20）：
//   location 0 = position (F32x2)：轮廓点（宽度已由 CPU 侧 lyon 落进几何）
//   location 1 = attrib0  (F32x2)：边法线（Analytic AA 用；本阶段 shader 不解释）
//   location 2 = attrib1  (F32)  ：边侧符号 ±1（同上）
//
// 本阶段是"硬边 + MSAA"：直接使用几何顶点位置，不解释 normal/side
// （与计划 §5.3 一致；将来切 AaModel::Analytic 只换本 shader，顶点格式不动）。

struct Uniforms {
    // 绘制空间（图元像素）→ 裁剪空间（含 Y 翻转）。
    transform: mat4x4<f32>,
    // 已预乘 alpha 的颜色。
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) side: f32,
) -> VsOut {
    var out: VsOut;
    out.clip = u.transform * vec4<f32>(position, 0.0, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
