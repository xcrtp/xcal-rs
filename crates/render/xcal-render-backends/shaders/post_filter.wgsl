// post_filter.wgsl — 合成后滤镜链（C1：**uber shader + 固定槽位**）
//
// 一次全屏 pass：输入是**中间全屏纹理**（= 全部图层合成后的结果，**预乘 alpha**），
// 输出到主 pass。滤镜**不是管线维度** —— 4 个固定槽位走 uniform，shader 内按
// `kind` 分支（uniform 分支：同一 draw call 内所有像素走同一分支，非 divergent）。
// ⇒ 新增滤镜**不需要改管线代码**（只需在此加一个分支 + 在 Rust 侧加一个 kind 码）。
//
// # 预乘 alpha 硬约束（§10.5）
//
// 输入是预乘值（`rgb ≤ a`）。所有涉及插值的滤镜都必须在预乘空间做，
// 否则透明黑 (0,0,0,0) 会渗进 rgb，半透明边缘出现黑边 / halo。
// - `KIND_GAIN`：每通道增益 —— `a` 的增益与 `rgb` 各自独立，但**rgb 增益须 ≤ 1**
//   （Rust 侧 `FilterSlot::color_gain` 的约定），以保持 `rgb ≤ a`；
// - `KIND_DISTORTION`：只改**采样坐标**，不参与插值 ⇒ 天然安全；
// - `KIND_BLUR`：对**预乘值**做加权平均 ⇒ 安全（非预乘空间做则出黑边）。
//
// # 数值码必须与 Rust 侧 `FilterKind::code()` 一致
//
//   None = 0 / Gain = 1 / Distortion = 2 / Blur = 3（见 `xcal_render::filters`）。

struct Slot {
    // x = kind 码（见上表）；yzw 预留。（不能叫 `meta`：WGSL 保留字）
    kind_code: vec4<u32>,
    // 参数（含义由 kind 决定）。
    params: vec4<f32>,
};

struct PostUniforms {
    // x = 有效槽位数（≤ 4）
    header: vec4<u32>,
    // xy = 中间纹理的**纹素步长**（1/宽, 1/高）—— 邻域滤镜用。
    global: vec4<f32>,
    slots: array<Slot, 4>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> u: PostUniforms;

const KIND_NONE: u32 = 0u;
const KIND_GAIN: u32 = 1u;
const KIND_DISTORTION: u32 = 2u;
const KIND_BLUR: u32 = 3u;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // 与 composite_2d.wgsl 同源：attrib0 存 UV（全 NDC 四边形上即 0..1）。
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) _side: f32,
) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

/// 几何重映射（畸变）：**逆向映射** —— 对目标像素问"我从源图哪里来"。
/// 正向映射会产生空洞与重叠。`k1/k2` 为 0 时恒等。
fn remap_uv(uv: vec2<f32>, params: vec4<f32>) -> vec2<f32> {
    let center = vec2<f32>(0.5, 0.5);
    let d = uv - center;
    let r2 = dot(d, d);
    // 归一化半径（把 [0,1]² 的对角半径拉回 1）⇒ k1 的量级与直觉一致。
    let r2n = r2 / 0.5;
    let k = 1.0 + params.x * r2n + params.y * r2n * r2n;
    return center + d * params.z * k;
}

/// 3×3 高斯模糊（**在预乘空间**做 ⇒ 无黑边）。`strength = 0` 时恒等。
///
/// 权重：中心 4、四邻 2、四对角 1（和 = 16）。**完全展开**（不用循环 + 动态数组索引：
/// WGSL 的 `let` 数组不支持运行时下标，展开同时省掉一次循环开销）。
fn blur9(uv: vec2<f32>, strength: f32, texel: vec2<f32>) -> vec4<f32> {
    let c = textureSample(src_tex, src_sampler, uv);
    let e = texel.x;
    let f = texel.y;
    var sum = c * 4.0;
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(e, 0.0)) * 2.0;
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(-e, 0.0)) * 2.0;
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(0.0, f)) * 2.0;
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(0.0, -f)) * 2.0;
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(e, f));
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(-e, f));
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(-e, -f));
    sum += textureSample(src_tex, src_sampler, uv + vec2<f32>(e, -f));
    return mix(c, sum / 16.0, clamp(strength, 0.0, 1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let count = min(u.header.x, 4u);
    // 逐槽应用：先采样（畸变改坐标）、再做邻域 / 点运算。
    var uv = in.uv;
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var sampled = false;

    for (var i = 0u; i < count; i = i + 1) {
        let slot = u.slots[i];
        let kind = slot.kind_code.x;
        if (kind == KIND_NONE) {
            continue;
        } else if (kind == KIND_DISTORTION) {
            uv = remap_uv(uv, slot.params);
        } else if (kind == KIND_BLUR) {
            // 邻域需要相邻样点 ⇒ 用中间纹理的纹素步长（uniform 提供）。
            color = blur9(uv, slot.params.x, u.global.xy);
            sampled = true;
        } else if (kind == KIND_GAIN) {
            if (!sampled) {
                color = textureSample(src_tex, src_sampler, uv);
                sampled = true;
            }
            color = color * slot.params;
        }
    }

    if (!sampled) {
        color = textureSample(src_tex, src_sampler, uv);
    }
    return color;
}
