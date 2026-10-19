// descriptor.rs — 样式描述符（descriptor 体系）。
//
// **决策（M2-1 ④）**：descriptor 体系归契约层 —— 因为 `Describable` 派生宏展开的
// 代码在 `ze-object`/下游插件里，trait 与 `StrokeDesc` 等**必须能被它们引用**。
// 这让契约层厚一点，但这是"能被引用的最小公共集"。
//
// descriptor 是**增量覆盖**：字段全为 `Option`（外层）+ `with_*` 派生，
// 缺失取主题默认值。这是"主题"能存在的前提，也是"不用继承"在值层面的兑现。
//
// ⚠️ **本轮的 `apply` 只做「声明合并」**：它产出**未换算**的 `Style2D`
// （`stroke_width` + `space` 原样带过去）。世界→屏幕换算仍只发生在
// `record_scene`（唯一换算点）；"record_scene 消费 Descriptors"留待 M4 ——
// 与 `Parent` 同类，属**已知未接线项**（记账见计划 §7）。

use ze_notion::{Color32, MeasureSpace};
use ze_render_ir::{StrokeSolid, Style2D};

/// 可被样式描述符驱动降级的对象。
///
/// `Descriptors` 由 `#[derive(Describable)]` + `#[describable(..)]` 生成：
/// **多个独立类型组合成元组**（不是总结构体），于是"线段不能填色"这类约束在
/// **类型层面**成立 —— `Line::Descriptors` 里根本没有 `FillDesc` 槽位。
pub trait Describable {
    /// 本对象接受的描述符元组。
    type Descriptors;

    /// 把描述符合并为绘制样式（未换算；缺失槽位取主题默认值）。
    fn apply(&self, d: &Self::Descriptors) -> Style2D;
}

/// 描边描述符：颜色 + 宽度 + 宽度所属空间。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeDesc {
    pub color: Color32,
    pub width: f32,
    pub space: MeasureSpace,
}

impl StrokeDesc {
    /// 两参构造：`space` 取默认 [`MeasureSpace::Screen`]。
    pub const fn new(color: Color32, width: f32) -> Self {
        Self {
            color,
            width,
            space: MeasureSpace::Screen,
        }
    }

    /// 显式指定度量空间。
    pub const fn new_in(color: Color32, width: f32, space: MeasureSpace) -> Self {
        Self {
            color,
            width,
            space,
        }
    }

    /// 转成既有的样式组件（宽度**仍未解析**，换算留给唯一换算点）。
    pub fn to_solid(&self) -> StrokeSolid {
        StrokeSolid::new_in(self.width, self.color, self.space)
    }
}

/// 主题默认：黑色 1px 屏幕空间描边。
impl Default for StrokeDesc {
    fn default() -> Self {
        Self::new(Color32::BLACK, 1.0)
    }
}

/// 填充描述符：单一颜色（与 `FillSolid` 同形，但属"声明"而非"样式实例"）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillDesc {
    pub color: Color32,
}

impl FillDesc {
    pub const fn new(color: Color32) -> Self {
        Self { color }
    }
}

/// 主题默认：白色填充。
impl Default for FillDesc {
    fn default() -> Self {
        Self::new(Color32::WHITE)
    }
}

/// 3D 材质描述符。
///
/// **M2-4**：本轮**不引用 `ShaderVariant`** —— 它在 `zela-scene`（场景层），
/// 契约层引它就会成环。变体选择暂留 `Material3D`；本字段待 M4 随材质系统
/// 一并下沉后再补。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MaterialDesc {
    pub color: Color32,
}

impl MaterialDesc {
    pub const fn new(color: Color32) -> Self {
        Self { color }
    }

    pub const fn color(color: Color32) -> Self {
        Self { color }
    }
}

// ─── 合并辅助（供 `#[derive(Describable)]` 展开调用）────────────────────────

/// 槽位掩码位：描边。
pub const SLOT_STROKE: u8 = 1 << 0;
/// 槽位掩码位：填充。
pub const SLOT_FILL: u8 = 1 << 1;

/// 主题默认样式：从 [`Style2D::default`] 起手，但**只保留对象声明的槽位**。
///
/// 为什么必须按槽位裁剪：`Style2D::default()` 是"有填充、无描边"的主题默认。
/// 若原样用于 `Line`（无 Fill 槽位），会得到一个 `fill = Some(WHITE)` 的样式
/// —— "线段不能填色"的**类型层保证**就会被运行时的主题默认偷偷破坏。
/// 故未被声明的槽位在此显式关闭（`fill = None` / `stroke = None`）。
pub fn theme_style(slot_mask: u8) -> Style2D {
    let mut style = Style2D::default();
    if slot_mask & SLOT_FILL == 0 {
        style.fill = None;
    }
    if slot_mask & SLOT_STROKE == 0 {
        style.stroke = None;
        style.stroke_width = 0.0;
    }
    style
}

/// 把描边描述符合并进 `Style2D`（**不换算**：`width` / `space` 原样带入）。
pub fn apply_stroke_desc(d: &StrokeDesc, style: &mut Style2D) {
    style.stroke = Some(d.color);
    style.stroke_width = d.width;
    style.space = d.space;
}

/// 把填充描述符合并进 `Style2D`。
pub fn apply_fill_desc(d: &FillDesc, style: &mut Style2D) {
    style.fill = Some(d.color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_desc_defaults_to_screen() {
        let d = StrokeDesc::new(Color32::RED, 2.0);
        assert_eq!(d.space, MeasureSpace::Screen);
        assert_eq!(StrokeDesc::default().width, 1.0);
        assert_eq!(
            StrokeDesc::new_in(Color32::RED, 2.0, MeasureSpace::World).space,
            MeasureSpace::World
        );
    }

    /// 声明合并不做换算：`World` 宽度原样保留（换算点唯一，在 record_scene）。
    #[test]
    fn apply_stroke_desc_does_not_resolve_width() {
        let mut style = Style2D::default();
        apply_stroke_desc(
            &StrokeDesc::new_in(Color32::RED, 3.0, MeasureSpace::World),
            &mut style,
        );
        assert_eq!(style.stroke, Some(Color32::RED));
        assert_eq!(style.stroke_width, 3.0);
        assert_eq!(style.space, MeasureSpace::World);
    }

    #[test]
    fn apply_fill_desc_sets_fill() {
        let mut style = Style2D {
            fill: None,
            ..Style2D::default()
        };
        apply_fill_desc(&FillDesc::new(Color32::GREEN), &mut style);
        assert_eq!(style.fill, Some(Color32::GREEN));
    }

    /// **证伪点**：只声明 Stroke 的对象（线段），其主题默认样式里**不得**有填充
    /// —— 否则"线段不能填色"的类型层保证会被主题默认破坏。
    #[test]
    fn theme_style_clears_undeclared_slots() {
        let line = theme_style(SLOT_STROKE);
        assert_eq!(line.fill, None, "未声明 Fill 槽位 ⇒ 不得有填充");
        assert_eq!(line.stroke, None, "主题默认无描边（由 descriptor 提供）");

        let circle = theme_style(SLOT_STROKE | SLOT_FILL);
        assert_eq!(circle.fill, Some(Color32::WHITE), "声明的槽位保留主题默认");
    }
}
