// composite_2d.wgsl
//
// S3-1：把 2D 离屏纹理（Rgba8Unorm，存的是 **gamma 值**）合成进 eframe 主 pass。
// S3-2/C4：泛化为**通用图层合成器**——源纹理可以是任意图层（2D 离屏 / 3D 离屏），
// 逐层带自己的 `opacity`；混合方式由 pipeline 的 BlendState 决定（不在这里分支）。
//
// 顶点布局同样**来自契约**（stride 20 的三槽）：
//   location 0 = 裁剪空间位置（F32x2）
//   location 1 = 纹理 UV（F32x2）
//   location 2 = 未使用（F32）
//
// 三种 fragment 入口，按**源纹理域 → 主 pass 目标域**选择（合成器在 prepare 时按
// (blend, 域转换) 选预建管线）：
//   - `fs_identity`        ：两侧同域（源非 sRGB + 目标非 sRGB，或源 sRGB + 目标 sRGB）
//     —— 采样值原样输出，逐字节保持图层内容；
//   - `fs_gamma_to_linear` ：源存 gamma 值（2D 离屏 Rgba8Unorm）、目标 sRGB
//     —— 硬件写入时会做 linear→sRGB 编码，故先 gamma→linear；
//   - `fs_linear_to_gamma` ：源是 sRGB 格式（3D 离屏，采样已由硬件解码为 linear）、
//     目标非 sRGB —— 目标不做编码，故先 linear→gamma。
// 这样三种组合下最终帧缓冲内容都与"该层直接画进主 pass"一致。

@group(0) @binding(0) var r_tex: texture_2d<f32>;
@group(0) @binding(1) var r_sampler: sampler;
/// 每层 uniform：`x` = 该层不透明度 [0,1]（其余分量预留）。
@group(0) @binding(2) var<uniform> r_layer: vec4<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) clip: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) unused: f32,
) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(clip, 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn gamma_from_linear_rgb(linear: vec3<f32>) -> vec3<f32> {
    let cutoff = linear < vec3<f32>(0.0031308);
    let lower = linear * vec3<f32>(12.92);
    let higher = vec3<f32>(1.055) * pow(linear, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, cutoff);
}

/// 预乘 alpha 的 opacity 缩放：rgb 与 a 同乘，保持"预乘"这一不变量
/// （Alpha 混合的 `src: One` 才能直接成立）。opacity=1 时恒等。
fn apply_opacity(c: vec4<f32>) -> vec4<f32> {
    return c * r_layer.x;
}

@fragment
fn fs_identity(in: VsOut) -> @location(0) vec4<f32> {
    return apply_opacity(textureSample(r_tex, r_sampler, in.uv));
}

@fragment
fn fs_gamma_to_linear(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(r_tex, r_sampler, in.uv);
    return apply_opacity(vec4<f32>(linear_from_gamma_rgb(c.rgb), c.a));
}

@fragment
fn fs_linear_to_gamma(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(r_tex, r_sampler, in.uv);
    return apply_opacity(vec4<f32>(gamma_from_linear_rgb(c.rgb), c.a));
}
