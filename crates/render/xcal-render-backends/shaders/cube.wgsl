// cube.wgsl — 3D 默认着色器（RC.1：纹理 over 颜色；RC 补充：头顶 Lambert 着色）。
//
// 绑定（group 0）：
//   binding 0 = Uniforms（mvp + color，dynamic offset）
//   binding 1 = texture_2d<f32>
//   binding 2 = sampler
//
// 纹理合成：**墨迹 over 已受光材质色**（`tex` 是 decal，不参与光照）
//   albedo = tex.rgb + color.rgb * lit * (1 - tex.a)
// 纹理是**预乘 alpha**（2D 离屏以 alpha=0 透明清屏、笔画按 One/OneMinusSrcAlpha
// 累积），所以：
//   - 透明处 `tex = (0,0,0,0)` → albedo = color * lit（材质色，保留明暗起伏）
//     —— 曲面本体可见，且与"纯色 3D + Lambert"完全一致；
//   - 墨迹处 `tex = (1,1,1,1)`（白墨）→ albedo = white —— 公式是**标签**，
//     恒为纯白，不被曲面明暗压暗（AMBIENT=0.2 时不会变深灰）。
// 材质未指定纹理时，后端绑定 1×1 **透明黑**兜底纹理（`tex = (0,0,0,0)`），
// 上式退化为 `color * lit`，因此既有"纯色 3D"画面的结果不变。
//
// 光照：以固定头顶方向 L = (0, 0, 1)（模型空间）做 Lambert。未携带法线的几何
// （线框立方体等）在打包时法线填 (0, 0, 1)，与 L 点积为 1 → lit = 1，外观逐像素
// 不变；只有真实曲面（法线随位置变化）才产生明暗起伏。故无需额外开关。
//
// 着色变体（对应 ShaderVariant）：同一 shader module 通过不同 @fragment 入口提供
// 多种着色方程，后端按 (topology, variant) 预建 pipeline 并查表选择：
//   - fs_flat    ← ShaderVariant::Flat（无光照：纹理 over 颜色）
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

// ShaderVariant::Flat：无光照，纹理预乘 over 颜色（加光前的行为）。
@fragment
fn fs_flat(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t, s, in.uv);
    // 预乘 over：`tex` 已是预乘 alpha，故直接相加；输出 alpha 取材质色。
    let albedo = tex.rgb + in.color.rgb * (1.0 - tex.a);
    return vec4<f32>(albedo, in.color.a);
}

// ShaderVariant::Lambert：朗伯漫反射只作用于材质色，纹理作为 decal 叠在其上（默认变体）。
@fragment
fn fs_lambert(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t, s, in.uv);
    let n = normalize(in.normal);
    let lit = AMBIENT + (1.0 - AMBIENT) * max(dot(n, LIGHT_DIR), 0.0);
    // `color * lit * (1 - tex.a)`：光照只调制材质色；墨迹（预乘 alpha）直接盖上，
    // 不被 `lit` 压暗（否则最暗处公式会变成 0.2 的深灰）。
    let albedo = tex.rgb + in.color.rgb * lit * (1.0 - tex.a);
    return vec4<f32>(albedo, in.color.a);
}
