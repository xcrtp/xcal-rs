// cube.wgsl
//
// MVE 3D 管线着色器：
// - uniform：mvp 矩阵（mat4x4）+ 颜色（vec4）
// - 顶点：location(0) = vec3 位置
// - 片元：直接输出 uniform 颜色
// 拓扑（triangle list / line list）由 pipeline 状态决定，着色器本身不区分。

struct Uniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = u.mvp * vec4<f32>(pos, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
