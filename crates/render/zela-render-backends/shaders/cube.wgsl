// cube.wgsl — 3D 默认着色器（RC.1：纹理 over 颜色；RC 补充：头顶 Lambert 着色；
// B1-A：光照数据化 —— 光源从硬编码常量改为场景级 uniform 驱动；
// B1-B：点光 —— `model` 入 uniform，片元取世界位置做距离衰减；
// B1-C：PBR —— 金属-粗糙度 GGX/Smith/Schlick，线性域计算，法线贴图走**屏幕
// 空间 TBN**（不改顶点格式）；相机位置经 `Lights.camera` 传入（视线向量用）。
//
// 绑定（group 0）：
//   binding 0 = Uniforms（mvp + model + color + params，dynamic offset）
//   binding 1 = texture_2d<f32>（decal 纹理）
//   binding 2 = sampler
//   binding 3 = Lights（场景级光源，非 dynamic —— 所有 draw 共享一份）
//   binding 4 = texture_2d<f32>（法线贴图，B1-C；缺省 = 平坦法线兜底纹理）
//
// 纹理合成：**墨迹 over 已受光材质色**（`tex` 是 decal，不参与光照）
//   albedo = tex.rgb + color.rgb * lit * (1 - tex.a)
// 纹理是**预乘 alpha**（2D 离屏以 alpha=0 透明清屏、笔画按 One/OneMinusSrcAlpha
// 累积），所以：
//   - 透明处 `tex = (0,0,0,0)` → albedo = color * lit（材质色，保留明暗起伏）
//     —— 曲面本体可见，且与"纯色 3D + Lambert"完全一致；
//   - 墨迹处 `tex = (1,1,1,1)`（白墨）→ albedo = white —— 公式是**标签**，
//     恒为纯白，不被曲面明暗压暗（ambient=0.2 时不会变深灰）。
// 材质未指定纹理时，后端绑定 1×1 **透明黑**兜底纹理（`tex = (0,0,0,0)`），
// 上式退化为 `color * lit`，因此既有"纯色 3D"画面的结果不变。
//
// 光照（B1-A/B）：方向光与点光数组都来自 binding 3 的场景级 uniform
// （`counts.x` 个方向光 + `counts.z` 个点光，各上限 `MAX_LIGHTS`）。默认场景
// （无 `Light` 组件）时后端兜底写入**单方向光 `(0,0,1)` + ambient 0.2**，
// 与本文件 B1-A 之前的硬编码 `LIGHT_DIR=(0,0,1)` / `AMBIENT=0.2` 逐位一致。
// 方向光方向为世界空间（物体无旋转时等于模型空间，见后端注释）；未携带法线
// 的几何（线框立方体等）在打包时法线填 `(0,0,1)`，与默认光点积为 1 → lit = 1，
// 外观逐像素不变；只有真实曲面（法线随位置变化）才产生明暗起伏。
// 点光（B1-B）位置为**世界空间**，片元位置 `world_pos = model * pos` 也按世界
// 空间计算（`model` 是 mesh 的模型矩阵）；衰减为 **range 窗口**
// `att = clamp(1 - dist²/range², 0, 1)`，避免 1/d² 在 d→0 爆炸。
//
// 着色变体（对应 ShaderVariant）：同一 shader module 通过不同 @fragment 入口提供
// 多种着色方程，后端按 (topology, variant) 预建 pipeline 并查表选择：
//   - fs_flat    ← ShaderVariant::Flat（无光照：纹理 over 颜色）
//   - fs_lambert ← ShaderVariant::Lambert（朗伯漫反射，默认）
//   - fs_pbr     ← ShaderVariant::Pbr（金属-粗糙度 PBR，B1-C）
// 顶点着色阶段（vs_main）与顶点布局对所有变体共享，因此新增风格**不得**改顶点格式。

struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    // B1-C：`x = metallic, y = roughness`（PBR 变体消费；其余变体忽略）。
    params: vec4<f32>,
};

// 场景级光源（B1-A/B，binding 3，非 dynamic）。
// counts.x = 方向光数量；counts.y = 环境光占比（ambient）；counts.z = 点光数量。
// dirs[i].xyz = 方向光方向（世界空间，从光源指向场景）；dirs[i].w = 强度。
// colors[i] = 方向光颜色（rgb；alpha 不参与光照）。
// points[i].xyz = 点光位置（世界空间）；points[i].w = 点光强度。
// point_colors[i] = 点光颜色（rgb；w = range 有效半径）。
// camera.xyz = 相机世界位置（B1-C，PBR 视线向量用）。
const MAX_LIGHTS: u32 = 4u;
struct Lights {
    counts: vec4<f32>,
    dirs: array<vec4<f32>, 4>,
    colors: array<vec4<f32>, 4>,
    points: array<vec4<f32>, 4>,
    point_colors: array<vec4<f32>, 4>,
    camera: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;
@group(0) @binding(3) var<uniform> lights: Lights;
@group(0) @binding(4) var nt: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) world_pos: vec3<f32>,
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
    // B1-B：世界位置（点光衰减用）。line 的 model = 恒等 ⇒ world_pos = pos。
    out.world_pos = (u.model * vec4<f32>(pos, 1.0)).xyz;
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
    // 环境光 + 全部方向光的朗伯项累加（B1-A）。默认场景 = 单方向光 (0,0,1)
    // 纯白强度 1，逐位等于旧硬编码 `lit = AMBIENT + (1-AMBIENT)*max(dot(n, L), 0.0)`。
    var lit = vec3<f32>(lights.counts.y);
    for (var i = 0u; i < u32(lights.counts.x); i = i + 1u) {
        let lam = max(dot(n, lights.dirs[i].xyz), 0.0);
        lit += (1.0 - lights.counts.y) * lights.dirs[i].w * lam * lights.colors[i].rgb;
    }
    // B1-B：点光 —— range 窗口衰减 `att = clamp(1 - dist²/range², 0, 1)`；
    // `max(dist2, 1e-6)` 护 normalize（光源贴片元时 d 近零）。
    for (var i = 0u; i < u32(lights.counts.z); i = i + 1u) {
        let d = lights.points[i].xyz - in.world_pos;
        let dist2 = dot(d, d);
        let inv = 1.0 / sqrt(max(dist2, 1e-6));
        let lam = max(dot(n, d * inv), 0.0);
        let att = clamp(1.0 - dist2 / (lights.point_colors[i].w * lights.point_colors[i].w), 0.0, 1.0);
        lit += (1.0 - lights.counts.y) * lights.points[i].w * att * lam * lights.point_colors[i].rgb;
    }
    // `color * lit * (1 - tex.a)`：光照只调制材质色；墨迹（预乘 alpha）直接盖上，
    // 不被 `lit` 压暗（否则最暗处公式会变成 0.2 的深灰）。
    let albedo = tex.rgb + in.color.rgb * lit * (1.0 - tex.a);
    return vec4<f32>(albedo, in.color.a);
}

// sRGB 编码的 8-bit 材质色 → 线性域（PBR 的 GGX/Smith/Schlick 假设线性亮度；
// 3D 离屏目标是 sRGB 格式，shader 输出线性值、硬件写入时编码，与 Lambert
// 变体"直接乘 sRGB 色"的既有行为刻意不同 —— PBR 是新增变体，无旧基线约束）。
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(c / 12.92, pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c > vec3<f32>(0.04045));
}

// ShaderVariant::Pbr（B1-C）：金属-粗糙度 PBR —— GGX（D）/ Smith（G）/
// Schlick（F），全部在线性域计算；法线贴图走**屏幕空间 TBN**（Mikkelsen 式，
// 不改顶点格式）：
//   det = uv1.x·uv2.y − uv2.x·uv1.y
//   t   = (p1·uv2.y − p2·uv1.y) / det
//   b   = (p2·uv1.x − p1·uv2.x) / det
//   n   = normalize(mat3(t, b, n_g) · (nt.rgb·2 − 1))
// `det` 符号无关：det 为负时 t/b 同号翻转，cross(t, b) 不变。缺省（无法线贴图）
// = 平坦法线兜底 (128,128,255) → nts = (0,0,1) → 恒等变换。
//
// 公式（每盏光，`intensity·ndl·lightColor`，点光乘 range 窗口 `att`）：
//   a2 = roughness⁴
//   D  = a2 / (π·(ndh²·(a2−1)+1)²)
//   G  = G_ndl · G_ndv，其中 G_x = 2·x / (x + √(a2 + (1−a2)·x²))
//   F  = f0 + (1−f0)·(1−hdv)⁵，f0 = mix(0.04, albedo, metallic)
//   spec = D·G·F / (4·ndl·ndv + 1e-4)；kd = (1−F)·(1−metallic)
//   direct += (kd·albedo/π + spec)·intensity·ndl·lightColor
//   ambient = counts.y·albedo；brdf = ambient + (1−counts.y)·direct
// 输出：`tex.rgb + brdf·(1−tex.a)`，alpha = 材质色（与 Lambert 同一 decal 合成口径）。
@fragment
fn fs_pbr(in: VertexOutput) -> @location(0) vec4<f32> {
    // ── 法线：几何法线 + 屏幕空间 TBN 变换法线贴图 ──
    let n_g = normalize(in.normal);
    let p1 = dpdx(in.world_pos);
    let p2 = dpdy(in.world_pos);
    let uv1 = dpdx(in.uv);
    let uv2 = dpdy(in.uv);
    let det = uv1.x * uv2.y - uv2.x * uv1.y;
    let inv_det = 1.0 / max(abs(det), 1e-6);
    // 注意：局部切线变量不得叫 `t` —— 会遮蔽 binding 1 的纹理 `t`（WGSL 不允许
    // 函数内遮蔽全局后仍被 textureSample 正确解析）。
    let tangent = normalize((p1 * uv2.y - p2 * uv1.y) * inv_det);
    let bitangent = normalize((p2 * uv1.x - p1 * uv2.x) * inv_det);
    let nts = textureSample(nt, s, in.uv).rgb * 2.0 - 1.0;
    let n = normalize(mat3x3(tangent, bitangent, n_g) * nts);

    // ── 材质参数（线性域 albedo）──
    let albedo = srgb_to_linear(in.color.rgb);
    let metallic = clamp(u.params.x, 0.0, 1.0);
    let roughness = clamp(u.params.y, 0.04, 1.0);
    let a2 = roughness * roughness * roughness * roughness;
    let v = normalize(lights.camera.xyz - in.world_pos);
    let ndv = max(dot(n, v), 1e-4);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);

    var brdf = vec3<f32>(lights.counts.y) * albedo;

    // 方向光。
    for (var i = 0u; i < u32(lights.counts.x); i = i + 1u) {
        let l = lights.dirs[i].xyz;
        let ndl = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let ndh = max(dot(n, h), 0.0);
        let hdv = max(dot(h, v), 0.0);
        let d_ggx = a2 / (3.14159265 * pow(ndh * ndh * (a2 - 1.0) + 1.0, 2.0));
        let g_ndl = 2.0 * ndl / max(ndl + sqrt(a2 + (1.0 - a2) * ndl * ndl), 1e-4);
        let g_ndv = 2.0 * ndv / max(ndv + sqrt(a2 + (1.0 - a2) * ndv * ndv), 1e-4);
        let f_schlick = f0 + (1.0 - f0) * pow(1.0 - hdv, 5.0);
        let spec = d_ggx * g_ndl * g_ndv * f_schlick / (4.0 * ndl * ndv + 1e-4);
        let kd = (1.0 - f_schlick) * (1.0 - metallic);
        brdf += (1.0 - lights.counts.y) * lights.dirs[i].w * (kd * albedo / 3.14159265 + spec)
            * ndl * lights.colors[i].rgb;
    }

    // 点光（range 窗口衰减同 Lambert；`world_pos` 世界空间）。
    for (var i = 0u; i < u32(lights.counts.z); i = i + 1u) {
        let dvec = lights.points[i].xyz - in.world_pos;
        let dist2 = dot(dvec, dvec);
        let inv = 1.0 / sqrt(max(dist2, 1e-6));
        let l = dvec * inv;
        let ndl = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let ndh = max(dot(n, h), 0.0);
        let hdv = max(dot(h, v), 0.0);
        let d_ggx = a2 / (3.14159265 * pow(ndh * ndh * (a2 - 1.0) + 1.0, 2.0));
        let g_ndl = 2.0 * ndl / max(ndl + sqrt(a2 + (1.0 - a2) * ndl * ndl), 1e-4);
        let g_ndv = 2.0 * ndv / max(ndv + sqrt(a2 + (1.0 - a2) * ndv * ndv), 1e-4);
        let f_schlick = f0 + (1.0 - f0) * pow(1.0 - hdv, 5.0);
        let spec = d_ggx * g_ndl * g_ndv * f_schlick / (4.0 * ndl * ndv + 1e-4);
        let kd = (1.0 - f_schlick) * (1.0 - metallic);
        let att = clamp(1.0 - dist2 / (lights.point_colors[i].w * lights.point_colors[i].w), 0.0, 1.0);
        brdf += (1.0 - lights.counts.y) * lights.points[i].w * att
            * (kd * albedo / 3.14159265 + spec) * ndl * lights.point_colors[i].rgb;
    }

    let tex = textureSample(t, s, in.uv);
    let out_rgb = tex.rgb + brdf * (1.0 - tex.a);
    return vec4<f32>(out_rgb, in.color.a);
}
