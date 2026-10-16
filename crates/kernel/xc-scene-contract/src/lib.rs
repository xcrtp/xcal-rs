// xc-scene-contract — 契约层
//
// 场景抽象：**身份 / 组织 / 关系 / 样式描述符**。上层（渲染管线、宿主 UI、场景对象层）
// 只依赖本层，不依赖具体 ECS（hecs 只出现在实现方 `xcal-scene`）。
//
// 分层位置（M2 建立）：
//
// ```text
// xc-notion          理念层      数学值 + 代数运算
// xc-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle
// xc-scene-contract  契约层      Scene / ObjectId / Name / Group / Visibility / Parent
//                                / Describable                                  ← 本 crate
// xc-object          场景对象层  Circle / Camera2D / Transform / …（M3）
// xcal-scene         ECS 实现    `impl Scene for hecs::World`
// ```
//
// 硬约束（验收口径见 M2 计划 §6.2）：
//   - 不得出现 ECS / hecs / GPU / wgpu / egui / UI
//   - 不得依赖 `xcal-*` / `xc-object`
//     （`cargo tree -p xc-scene-contract | grep -E "xcal-|xc-object"` = 0）
//
// ⚠️ M2 结束时 `Scene` trait **尚无实现**（`xc-object` 也还不存在）—— 这是计划
// 明确认可的已知中间态，M3 补上实现（`impl Scene for hecs::World` 在 xcal-scene）。

pub mod descriptor;
pub mod id;
pub mod organize;
pub mod relation;
pub mod scene;

pub use descriptor::{
    Describable, FillDesc, MaterialDesc, SLOT_FILL, SLOT_STROKE, StrokeDesc, apply_fill_desc,
    apply_stroke_desc, theme_style,
};
pub use id::ObjectId;
pub use organize::{Group, GroupId, GroupVisibility, Name, Visibility, VisibilityTag};
pub use relation::{Children, Parent};
pub use scene::{Components, Scene, SceneRead};
