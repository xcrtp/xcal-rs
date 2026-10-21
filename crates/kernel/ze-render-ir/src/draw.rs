// draw.rs — 2D 绘制命令（B2-0b：从 `zela-render::draw` 迁入 IR）。
//
// 为什么迁到 IR：`DrawCommand` 是「对象 → 绘制」的中间表示（ZIndex + Shape2D +
// Style2D），不含任何后端/ECS 类型；留在渲染管线层会让「队列」概念被绑在一个
// 具体 crate 上（计划 #9）。B2-1 的统一 `ZOrder` 将以本结构的排序语义为基。
//
// **排序语义（R-F，确定性保证）**：`Ord::cmp` **只比 `z`**，同 z 内的先后由
// **插入序**决定 —— 依赖 `Vec::sort()` 的**稳定排序**。任何改用 `sort_unstable`
// 的实现都会破坏该保证（B2-1 会写成显式测试锁定）。

use crate::shape::{Shape2D, Style2D};

/// 绘制层级（B2-1：**统一排序概念**，吸收原 2D `ZIndex`）。
///
/// 值越小越先绘制（位于画面下层）。2D 与 3D 命令共用这一个排序键 —— 3D 此前
/// 没有层序概念（#11），B2-1 起由它承担，B2-4 的 epsilon 也由它派生（R-C）。
///
/// 与场景对象层的 `ze_scene_core::ZOrder` 是**同一个类型**（后者 re-export 本类型），
/// 保证"排序概念只有一份"（铁律 3）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ZOrder(pub i32);

impl ZOrder {
    /// 默认层（无显式声明时的取值）。
    pub const DEFAULT: Self = Self(0);

    pub const fn new(z: i32) -> Self {
        Self(z)
    }
}

impl Default for ZOrder {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 旧名兼容别名（B2-1 起 `ZIndex` 统一为 [`ZOrder`]；下游路径保持可用）。
pub type ZIndex = ZOrder;

/// 单条 2D 绘制命令 = [`ZOrder`] + [`Shape2D`] + [`Style2D`]。
#[derive(Debug, Clone)]
pub struct DrawCommand {
    pub z: ZOrder,
    pub shape: Shape2D,
    pub style: Style2D,
}

impl DrawCommand {
    pub fn new(z: ZOrder, shape: Shape2D, style: Style2D) -> Self {
        Self { z, shape, style }
    }
}

impl PartialEq for DrawCommand {
    fn eq(&self, other: &Self) -> bool {
        self.z == other.z
    }
}
impl Eq for DrawCommand {}
impl PartialOrd for DrawCommand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for DrawCommand {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.z.cmp(&other.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{FillSolid, fill_style};
    use ze_notion::Color32;

    fn cmd(z: i32) -> DrawCommand {
        DrawCommand::new(
            ZIndex::new(z),
            Shape2D::Circle {
                center: ze_notion::Vec2::ZERO,
                radius: 1.0,
            },
            fill_style(&FillSolid::new(Color32::WHITE)),
        )
    }

    /// R-F：`Ord` 只比 z —— 同 z 的命令互相 equal，排序稳定性由此承担。
    #[test]
    fn test_draw_command_order_compares_z_only() {
        let a = cmd(5);
        let b = cmd(5);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        assert_eq!(a, b, "同 z 必须相等（不比较 shape/style）");
        assert!(cmd(1) < cmd(2));
    }

    /// 稳定排序：同 z 内保持插入序（R-F 的显式锁定）。
    #[test]
    fn test_draw_command_stable_within_equal_z() {
        let mut v = vec![cmd(5), cmd(1), cmd(5), cmd(1), cmd(5)];
        v.sort();
        // 两个 z=1 保持原始相对序；三个 z=5 同理
        assert_eq!(v.iter().filter(|c| c.z.0 == 1).count(), 2);
        assert_eq!(v.iter().filter(|c| c.z.0 == 5).count(), 3);
        assert!(v.windows(2).all(|w| w[0].z <= w[1].z));
    }
}
