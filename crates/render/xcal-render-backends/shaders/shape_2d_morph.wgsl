// shape_2d_morph.wgsl
//
// S4.3：2D **形变**几何着色器。与 `shape_2d.wgsl` 同源，唯一区别是顶点来自
// **两个** vertex buffer，逐帧插值参数 `t` 走 uniform（几何本身帧间不变）。
//
// 顶点布局**不动契约**（`VertexLayout::standard` 两个字面复用，只是绑到两个
// buffer slot）：wgpu 的 shader location 跨 buffer **全局连续**编号，R0 后每个
// buffer 有**四**个属性，于是
//   buffer slot 0 → location 0/1/2/3 = from 的 position / normal / side / advance
//   buffer slot 1 → location 4/5/6/7 = to   的 position / normal / side / advance
// （见 `xcal_render::vertex::VertexInputDesc` 与
//  `xcal-render-backends::vertex_layout::to_wgpu_buffers`）。
// location 3 / 7（两侧的 `advance`）**本 shader 不消费**：reveal 只作用于
// 单几何路径（`shape_2d.wgsl`），形变不做书写动画（计划 §12.3）。
//
// 两端几何由 CPU 侧**弧长等点重采样**（`MorphItem::sampled_endpoints`）保证
// 顶点一一对应，因此这里逐点 `mix` 才是合法插值；描边法线也一并插值（A1），
// 形变全程描边宽度准确。
//
// uniform 复用 `shape_2d.wgsl` 的同一块：`params.x` = 描边宽度、
// **`params.y` = t**（形变参数）、`params.z/w` = reveal 区间（本 shader 不用，
// CPU 侧恒写 `[0,1]`）——块大小不变（96B），无需改 uniform 契约。

struct Uniforms {
    // 绘制空间（图元像素）→ 裁剪空间（含 Y 翻转）。
    transform: mat4x4<f32>,
    // 已预乘 alpha 的颜色。
    color: vec4<f32>,
    // x = 描边宽度（屏幕像素，与几何同空间）；y = 形变参数 t；zw = reveal 区间（不用）。
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) from_position: vec2<f32>,
    @location(1) from_normal: vec2<f32>,
    @location(2) from_side: f32,
    @location(3) from_advance: f32,
    @location(4) to_position: vec2<f32>,
    @location(5) to_normal: vec2<f32>,
    @location(6) to_side: f32,
    @location(7) to_advance: f32,
) -> VsOut {
    let t = clamp(u.params.y, 0.0, 1.0);
    // A1：位置与法线都插值（描边宽度在形变全程准确）。
    let position = mix(from_position, to_position, t);
    let normal = mix(from_normal, to_normal, t);
    let _side = mix(from_side, to_side, t); // 本阶段硬边 + MSAA，side 暂不解释
    // 沿法线外扩半个线宽（R1 参数化描边，与 shape_2d.wgsl 同式）。
    let p = position + normal * (u.params.x * 0.5);
    var out: VsOut;
    out.clip = u.transform * vec4<f32>(p, 0.0, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
