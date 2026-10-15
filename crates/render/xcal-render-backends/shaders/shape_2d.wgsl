// shape_2d.wgsl
//
// S3-1：2D 图元几何着色器（AaModel::None + MSAA）。
//
// 顶点布局**来自契约**（xcal_render::vertex::VertexLayout::standard，stride 20）：
//   location 0 = position (F32x2)：**路径上的点**（不含宽度偏移）
//   location 1 = attrib0  (F32x2)：边法线（含 miter 缩放，未归一化）
//   location 2 = attrib1  (F32)  ：边侧符号 ±1
//
// 描边**几何参数化**（R1）：几何里没有宽度，宽度由这里现场补偿 ——
//   轮廓点 = position + normal * (stroke_width * 0.5)
// lyon 的 `normal` 已含 miter 缩放（"沿它位移 1.0 即把描边撑粗 2.0"，**不得归一化**），
// 故上式正好还原 CPU 侧 lyon 原本烘进几何的轮廓点。
//
// 填充顶点 `normal = [0,0]` → 偏移恒为 0，与参数化之前**逐字节一致**（自动安全）。
//
// 宽度单位：与几何**同一空间**的屏幕像素，由 CPU 侧 `draw.rs::resolve_style` 唯一换算点
// 解析（shader 只做几何偏移，不做空间换算——`MeasureSpace` 语义不变）。
//
// 本阶段仍是"硬边 + MSAA"：偏移后的位置直接光栅化，`side` 暂不解释
// （将来切 AaModel::Analytic 用 `side` 定内外侧，顶点格式不动）。

struct Uniforms {
    // 绘制空间（图元像素）→ 裁剪空间（含 Y 翻转）。
    transform: mat4x4<f32>,
    // 已预乘 alpha 的颜色。
    color: vec4<f32>,
    // x = 描边宽度（屏幕像素，与几何同空间）；yzw 预留。
    params: vec4<f32>,
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
    // 沿法线外扩半个线宽；填充 normal=[0,0] → 偏移为 0。
    let p = position + normal * (u.params.x * 0.5);
    out.clip = u.transform * vec4<f32>(p, 0.0, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
