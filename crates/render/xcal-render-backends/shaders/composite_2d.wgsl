// composite_2d.wgsl
//
// S3-1：把 2D 离屏纹理（Rgba8Unorm，存的是 **gamma 值**）合成进 eframe 主 pass。
//
// 顶点布局同样**来自契约**（stride 20 的三槽）：
//   location 0 = 裁剪空间位置（F32x2）
//   location 1 = 纹理 UV（F32x2）
//   location 2 = 未使用（F32）
//
// 两个 fragment 入口，与 egui-wgpu 自身的做法一致（`egui.wgsl`）：
//   - 主 pass 目标为**非 sRGB**（Rgba8Unorm/Bgra8Unorm）：直接输出 gamma 值；
//   - 主 pass 目标为 **sRGB**：硬件写入时会做 linear→sRGB 编码，故先 gamma→linear。
// 这样两种目标格式下最终帧缓冲内容都与旧 egui painter 管线一致。

@group(0) @binding(0) var r_tex: texture_2d<f32>;
@group(0) @binding(1) var r_sampler: sampler;

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

@fragment
fn fs_gamma(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(r_tex, r_sampler, in.uv);
}

@fragment
fn fs_linear(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(r_tex, r_sampler, in.uv);
    return vec4<f32>(linear_from_gamma_rgb(c.rgb), c.a);
}
