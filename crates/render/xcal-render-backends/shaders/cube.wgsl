// cube.wgsl — 3D 默认着色器（RC.1：颜色 × 纹理调制；RC 补充：头顶 Lambert 着色）。
//
// 绑定（group 0）：
//   binding 0 = Uniforms（mvp + color，dynamic offset）
//   binding 1 = texture_2d<f32>
//   binding 2 = sampler
//
// 材质未指定纹理时，后端绑定 1×1 白色兜底纹理，`color * white == color`，
// 因此既有"纯色 3D"画面的结果不变。
//
// 光照：以固定头顶方向 L = (0, 0, 1)（模型空间）做 Lambert。未携带法线的几何
// （线框立方体等）在打包时法线填 (0, 0, 1)，与 L 点积为 1 → lit = 1，外观逐像素
// 不变；只有真实曲面（法线随位置变化）才产生明暗起伏。故无需额外开关。
//
// 着色变体（对应 ShaderVariant）：同一 shader module 通过不同 @fragment 入口提供
// 多种着色方程，后端按 (topology, variant) 预建 pipeline 并查表选择：
//   - fs_flat    ← ShaderVariant::Flat（无光照：color * tex）
//   - fs_lambert ← ShaderVariant::Lambert（朗伯漫反射，默认）
// 顶点着色阶段（vs_main）与顶点布局对所有变体共享，因此新增风格**不得**改顶点格式。

struct Uniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;

// 头顶平行光方向（模型空间）与环境光占比。
const LIGHT_DIR: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);
const AMBIENT: f32 = 0.2;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = u.mvp * vec4<f32>(pos, 1.0);
    out.color = u.color;
    out.uv = uv;
    out.normal = normal;
    return out;
}

// ShaderVariant::Flat：无光照，仅颜色 × 纹理（加光前的行为）。
@fragment
fn fs_flat(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t, s, in.uv);
    return vec4<f32>(in.color.rgb * tex.rgb, in.color.a * tex.a);
}

// ShaderVariant::Lambert：朗伯漫反射 × 颜色 × 纹理（默认变体）。
@fragment
fn fs_lambert(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t, s, in.uv);
    let n = normalize(in.normal);
    let lit = AMBIENT + (1.0 - AMBIENT) * max(dot(n, LIGHT_DIR), 0.0);
    return vec4<f32>(in.color.rgb * lit * tex.rgb, in.color.a * tex.a);
}
