// surface_matrix.js — surface_matrix.lua 的逐语句翻译（B7 Step 3，JS 首次落地）。
//
// 目的：证明「同一场景、两种语言、同一像素」。与 .lua 的对应关系是**一对一**的：
// 每一条语句、每一个字段、每一个枚举都与 .lua 相同，不重排、不合并。
// 判据：帧序列哈希与 surface_matrix.lua 逐位相同（不符即 fail，不放宽）。
//
// JS 侧口径（ze-pod-js 绑定层的显式差异，见 binding.rs 头注释）：
// - 组件 / 值 / 枚举都走构造器：`Surface({ ... })`、`Color({ ... })`、`SurfaceMapping.World`
//   （unit 变体是值，带字段变体是构造器）；
// - 数组是普通 `Array`：`scene.spawn([...])`（Lua 侧是表）；
// - 数字只有 f64 ⇒ 整数字段（如 texel_width）必须是整值，否则绑定层显式报错。

const surf = scene.spawn([
  Surface({
    min_x: -8.0, min_y: -6.0, max_x: 8.0, max_y: 6.0,
    mapping: SurfaceMapping.World,
    anchor_x: 0.5, anchor_y: 0.5,
    renderer: SurfaceRenderer.Renderer2D,
    texel_width: 1024, texel_height: 768,
    filter: TextureFilter.Linear,
    mipmap: false,
    fit_margin: 0.9,
    composite: true,
  }),
]);

const belong = Parent({ parent: surf });
const fillWarm = Fill({ color: Color({ r: 255, g: 90, b: 90, a: 255 }) });
const fillCool = Fill({ color: Color({ r: 255, g: 220, b: 90, a: 255 }) });
const strokeCool = Stroke({ width: 6.0, color: Color({ r: 80, g: 200, b: 255, a: 255 }) });
const strokeGreen = Stroke({ width: 4.0, color: Color({ r: 120, g: 255, b: 160, a: 255 }) });

// Circle × Fill / Stroke（空间错开，避免互相污染）。
scene.spawn([Circle({ x: -5.5, y: 3.0, r: 1.6 }), fillWarm, belong]);
scene.spawn([Circle({ x: -5.5, y: -3.0, r: 1.6 }), strokeCool, belong]);

// Arrow × Fill / Stroke。
scene.spawn([
  Arrow({
    from: Vec2({ x: -2.0, y: 3.0 }),
    to: Vec2({ x: 1.5, y: 3.0 }),
    head_length: 0.9,
    head_width: 0.6,
  }),
  fillCool,
  belong,
]);
scene.spawn([
  Arrow({
    from: Vec2({ x: -2.0, y: -3.0 }),
    to: Vec2({ x: 1.5, y: -3.0 }),
    head_length: 0.9,
    head_width: 0.6,
  }),
  strokeGreen,
  belong,
]);

// Path × Stroke：文本编译出的描边路径（每条形一条对象）。
const paths = content.text(Text({ source: "AB", font_size: 3.0, x: 3.0, y: -0.5 }));
for (const path of paths) {
  scene.spawn([PathShape({ path }), belong]);
}

// Path × Fill：同一份字形轮廓走填充（另一种样式组合，空间上不重叠）。
const fills = content.text(Text({ source: "CD", font_size: 2.5, x: 3.0, y: 4.0 }));
for (const path of fills) {
  scene.spawn([PathShape({ path }), fillWarm, belong]);
}

// 运行时要求存在 2D 相机（内容层是纹理源；此相机仅占位）。
scene.spawn([Camera2D({ center: Vec2({ x: 0.0, y: 0.0 }), scale: 30.0 })]);
