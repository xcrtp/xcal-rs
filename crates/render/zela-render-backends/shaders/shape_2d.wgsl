// shape_2d.wgsl
//
// S3-1：2D 图元几何着色器（AaModel::None + MSAA）。R0 起额外承载**书写动画**。
//
// 顶点布局**来自契约**（zela_render::vertex::VertexLayout::standard，R0 后 stride 24）：
//   location 0 = position (F32x2)：**路径上的点**（不含宽度偏移）
//   location 1 = attrib0  (F32x2)：边法线（含 miter 缩放，未归一化）
//   location 2 = attrib1  (F32)  ：边侧符号 ±1
//   location 3 = attrib2  (F32)  ：归一化弧长 s ∈ [0,1]（`aa::AttribSemantic::Advance`）
//
// 描边**几何参数化**（R1）：几何里没有宽度，宽度由这里现场补偿 ——
//   轮廓点 = position + normal * (stroke_width * 0.5)
// lyon 的 `normal` 已含 miter 缩放（"沿它位移 1.0 即把描边撑粗 2.0"，**不得归一化**），
// 故上式正好还原 CPU 侧 lyon 原本烘进几何的轮廓点。
//
// 填充顶点 `normal = [0,0]` → 偏移恒为 0，与参数化之前**逐字节一致**（自动安全）。
//
// # 书写动画（reveal，R0）
//
// 只画 `s ∈ [params.z, params.w]` 的部分（默认 `[0,1]` ⇒ 与改动前逐位一致）。
//
// **做法 = (b) 片元丢弃**（在 `fs_main` 里 `discard`），不是 (a) 顶点塌缩。
// 计划 §7.1 先建议 (a)，但 §10 像素级验收**实测证伪**了它：顶点塌缩到同一个
// 视口外点只对"整个三角形都在区间外"成立；实测描边三角化里存在
// **恰有一个顶点越界**的三角形（miter join 邻域），那个三角形会从塌缩点拉出一条
// 覆盖屏幕的楔形 —— 表现为 `b = 0` 时仍有 747 个不透明像素、且覆盖数不单调
// （`b = 0.75` 比 `b = 1` 还多）。切成片元丢弃后：`b = 0` 零像素、覆盖随 `b` 严格递增、
// `[0,1]` 与不带 reveal 逐位一致（三条均有 GPU 用例锁定）。
//
// 切口精度：`s` 作为**插值量**送到片元（描边四边形两端点的 `s` 恰好是路径推进量，
// 沿路径方向线性、沿宽度方向恒定）⇒ 切口是**准确的直线**，不受顶点间距量化。
// `REVEAL_EPS` 只为吸收插值末位误差，防止 `b = 1` 时误丢路径末端的片元
// （否则 `[0,1]` 无法与既有画面逐位一致）。
//
// 宽度单位：与几何**同一空间**的屏幕像素，由 CPU 侧 `draw.rs::resolve_style` 唯一换算点
// 解析（shader 只做几何偏移，不做空间换算——`MeasureSpace` 语义不变）。
//
// 本阶段仍是"硬边 + MSAA"：偏移后的位置直接光栅化，`side` 暂不解释
// （将来切 AaModel::Analytic 用 `side` 定内外侧，顶点格式不动）。

/// 区间比较的容差：吸收 `s` 插值的末位误差（见上文"切口精度"）。
const REVEAL_EPS: f32 = 1e-6;

struct Uniforms {
    // 绘制空间（图元像素）→ 裁剪空间（含 Y 翻转）。
    transform: mat4x4<f32>,
    // 已预乘 alpha 的颜色。
    color: vec4<f32>,
    // x = 描边宽度（屏幕像素，与几何同空间）；y = 形变参数 t（本 shader 不用）；
    // z = reveal 区间起点 a；w = reveal 区间终点 b（普通绘制为 0.0 / 1.0 ⇒ 全可见）。
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // 归一化弧长：插值到片元后与 `params.z/w` 比较（书写动画的切口）。
    @location(1) advance: f32,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) side: f32,
    @location(3) advance: f32,
) -> VsOut {
    var out: VsOut;
    // 沿法线外扩半个线宽；填充 normal=[0,0] → 偏移为 0。
    let p = position + normal * (u.params.x * 0.5);
    out.clip = u.transform * vec4<f32>(p, 0.0, 1.0);
    out.color = u.color;
    out.advance = advance;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // 书写动画：只保留 `s ∈ [a, b]` 的片元（区间是 uniform ⇒ 同一 draw call 内一致）。
    if (in.advance < u.params.z - REVEAL_EPS || in.advance > u.params.w + REVEAL_EPS) {
        discard;
    }
    return in.color;
}
