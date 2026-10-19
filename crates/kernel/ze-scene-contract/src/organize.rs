// organize.rs — 对象的**组织与可见性**（N1–N3）。
//
// 本模块回答"对象怎么被组织、怎么被引用、怎么被看见"里的后两问：
// - [`Name`]：对象可被**声明式引用**（按名字找对象，而不是手递 `ObjectId`）；
// - [`Group`] / [`GroupId`]：对象可被**分组**（纯标签，一组可整体操作）；
// - [`Visibility`] / [`VisibilityTag`]：可见性是**"对谁可见"**（关系而非布尔）——
//   同一对象在不同相机下可以不同。
//
// # 为什么在契约层（`ze-scene-contract`）而不是场景对象层
//
// 这四者都是**场景的组织语义**（谁叫什么、谁属于哪组、谁对谁可见），不含任何
// 数学值、也不含任何渲染手段 —— 与同层的 [`crate::Parent`] 同一判据
// （"描述场景中的关系，不是数学概念"）。任何 `Scene` 实现都能用，**不绑 hecs**。
//
// # 与"可缺席组件 = 默认值"惯例的关系
//
// 照 [`ZOrder`]（在 `ze-object`）的既有先例：**缺席即默认**
// - `Name` 缺席 = 匿名对象（绝大多数对象不需要名字）
// - `Group` 缺席 = 不属于任何组
// - `Visibility` 缺席 = **全局可见**（`visible_for` 的 `None` 分支）
//
// # 本轮明确不做（记账）
//
// - **嵌套组**（`Group` 里再含 `Group`）：嵌套是"看起来通用、实际没人用"的典型；
// - **组实体**（不为组建对象）⇒ [`GroupId`] 用 `u32` 而非 `ObjectId`：
//   组是纯标签，不存在生命周期，也不需要 generation 失效检测；
// - **`Name` 全局唯一性强制**：`find_by_name` 返回**第一个**命中（见 `zela-scene` 的实现）；
// - **按名字的可见性 tag**（`VisibilityTag` 只认相机与组）。

use std::collections::HashSet;

use crate::id::ObjectId;

// ─── N1：Name ─────────────────────────────────────────────────────────────

/// 对象名字：**声明式引用**的载体（脚本按名字拿对象，不必手递 `ObjectId`）。
///
/// 缺席 = 匿名。**不强制唯一**：查找返回第一个命中（场景规模为几十个对象，
/// O(n) 遍历足够；引入全局注册表不划算）。查重是装配层策略，不是契约的职责。
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Name(pub String);

impl Name {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─── N2：Group ────────────────────────────────────────────────────────────

/// 组标识（纯标签）。
///
/// **为什么不是 `ObjectId`**：本轮**不建组实体**（[`crate::Parent`] 那套父子关系
/// 牵扯"父子变换累乘"这个未接线项，组刻意绕开它）。没有实体就没有生命周期，
/// 因而不需要 generation 失效检测 —— `u32` 正是这一事实的诚实表达。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct GroupId(pub u32);

impl GroupId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

/// 组标签组件：标记"这个对象属于某组"。
///
/// **只含标识，不含变换** —— 这是与 [`crate::Parent`] 的根本区别：组不做父子
/// 变换累乘，只是"一批对象"的标签，用于整体查询与批量控制。
///
/// 缺席 = 不属于任何组。**不支持嵌套**（组里不能再含组）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Group(pub GroupId);

impl Group {
    pub const fn new(id: GroupId) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> GroupId {
        self.0
    }
}

// ─── N3：Visibility ───────────────────────────────────────────────────────

/// 可见性 tag：一个"对谁可见"的条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VisibilityTag {
    /// 全局可见（等价于"不挂组件"）。
    Global,
    /// 在**指定相机**下可见（复用相机的 [`ObjectId`]，不另建标识体系）。
    Camera(ObjectId),
    /// 由**指定组的开关**决定（见 [`GroupVisibility`]）。
    Group(GroupId),
}

/// 可见性组件：**非空 tag 集合**，**任一命中即可见（并，非交）**。
///
/// 为什么是"一个组件内含集合"而不是"挂多个 `Visibility` 组件"：多组件的组合
/// 语义（并还是交？）没有自然答案，而集合的并语义是显式的。
///
/// # 缺席语义 = 全局可见
///
/// 照 [`ZOrder`] 惯例：绝大多数对象不必挂本组件 ——
/// [`Visibility::visible_for`] 的调用方在"查不到组件"时按 `true` 处理
/// （`zela-render` 的点查询用 `.unwrap_or(true)`）。
///
/// # 空 tag 集合
///
/// 空集**不**被区分对待，一律按"全局可见"处理：空集在语义上只有两种可能
/// （"什么都不命中 ⇒ 永不可见"或"没声明 ⇒ 全局可见"），而后者与缺席一致、
/// 不与任何调用点冲突。**空集 = 缺席 = 全局可见**（单一口径，避免
/// "挂了组件反而隐形的"这种反直觉陷阱）。
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Visibility {
    pub tags: Vec<VisibilityTag>,
}

impl Visibility {
    /// 全局可见（与缺席等价）。
    pub fn global() -> Self {
        Self {
            tags: vec![VisibilityTag::Global],
        }
    }

    /// 只在指定相机下可见。
    pub fn for_camera(camera: ObjectId) -> Self {
        Self {
            tags: vec![VisibilityTag::Camera(camera)],
        }
    }

    /// 由指定组的开关决定。
    pub fn of_group(group: GroupId) -> Self {
        Self {
            tags: vec![VisibilityTag::Group(group)],
        }
    }

    /// 追加一个 tag（builder 风格；用于"在多台相机 + 某组下都可见"）。
    pub fn with_tag(mut self, tag: VisibilityTag) -> Self {
        self.tags.push(tag);
        self
    }

    /// 该对象是否在 `camera` 下可见。
    ///
    /// - 空 `tags` ⇒ `true`（空集 = 全局可见，见类型文档）；
    /// - 否则**任一 tag 命中**即 `true`：
    ///   全局 tag 恒命中；相机 tag 要求 `camera` 与它**同一个 [`ObjectId`]**；
    ///   组 tag 要求该组**未被 [`GroupVisibility`] 隐藏**。
    ///
    /// `camera = None`（调用方拿不到相机 ID）⇒ 只有 `Global` / 未隐藏的 `Group`
    /// 能命中；`Camera(_)` tag 一律不命中 —— "不知道是谁在看"时不假装是它。
    pub fn visible_for(&self, camera: Option<ObjectId>, groups: &GroupVisibility) -> bool {
        if self.tags.is_empty() {
            return true;
        }
        self.tags.iter().any(|tag| match tag {
            VisibilityTag::Global => true,
            VisibilityTag::Camera(c) => camera == Some(*c),
            VisibilityTag::Group(g) => !groups.is_hidden(*g),
        })
    }
}

/// **组级开关**：被隐藏的组集合（默认空 ⇒ 所有组可见）。
///
/// 这是 `VisibilityTag::Group` 的"另一半"—— tag 只声明"我随组走"，
/// 而"组现在是开还是关"必须有一个**集中**的地方（不能存在每个成员身上，
/// 否则"组级控制"退化成 N 次逐对象写入，且没有真相来源）。
///
/// 它是**纯数据**（契约层不关心谁改它）：宿主（UI 复选框 / 脚本 / 测试）
/// 写，录制路径读。本轮不做 UI 面板（记账），但录制路径已消费它。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupVisibility {
    hidden: HashSet<GroupId>,
}

impl GroupVisibility {
    pub fn new() -> Self {
        Self::default()
    }

    /// 隐藏一组（组内所有"随组走"的对象随之不可见）。
    pub fn hide(&mut self, group: GroupId) -> &mut Self {
        self.hidden.insert(group);
        self
    }

    /// 让一组恢复可见。
    pub fn show(&mut self, group: GroupId) -> &mut Self {
        self.hidden.remove(&group);
        self
    }

    pub fn is_hidden(&self, group: GroupId) -> bool {
        self.hidden.contains(&group)
    }

    /// 是否有任何组处于隐藏状态（录制路径的快速旁路：全可见 ⇒ 跳过组判定）。
    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> ObjectId {
        ObjectId::new(index, 1)
    }

    /// **可见性缺席**（无组件）与**空 tag 集合**同口径：都视为全局可见。
    #[test]
    fn empty_tags_mean_global_visible() {
        let groups = GroupVisibility::new();
        let empty = Visibility::default();
        assert!(empty.visible_for(None, &groups), "空集 = 全局可见");
        assert!(empty.visible_for(Some(id(1)), &groups));
        assert_eq!(Visibility::global(), {
            let mut v = Visibility::default();
            v.tags.push(VisibilityTag::Global);
            v
        });
    }

    /// 相机 tag：**同一 `ObjectId` 才命中**（"不知道是谁在看"时不命中）。
    #[test]
    fn camera_tag_matches_exact_camera_only() {
        let groups = GroupVisibility::new();
        let v = Visibility::for_camera(id(7));
        assert!(v.visible_for(Some(id(7)), &groups), "相机 A 下可见");
        assert!(!v.visible_for(Some(id(8)), &groups), "相机 B 下不可见");
        assert!(!v.visible_for(None, &groups), "不知道相机 ⇒ 不假装可见");
    }

    /// 组 tag：由 `GroupVisibility` 决定（组级控制生效）。
    #[test]
    fn group_tag_follows_group_switch() {
        let mut groups = GroupVisibility::new();
        let v = Visibility::of_group(GroupId::new(3));
        assert!(v.visible_for(None, &groups), "组默认开 ⇒ 可见");
        groups.hide(GroupId::new(3));
        assert!(!v.visible_for(None, &groups), "组关闭 ⇒ 不可见");
        assert!(groups.is_hidden(GroupId::new(3)));
        groups.show(GroupId::new(3));
        assert!(v.visible_for(None, &groups), "再打开 ⇒ 恢复可见");
        assert!(groups.is_empty(), "全部恢复后集合为空");
    }

    /// **并（非交）**：多 tag 任一命中即可见 —— 组被关掉但相机 tag 命中 ⇒ 仍可见。
    #[test]
    fn tags_are_union_not_intersection() {
        let mut groups = GroupVisibility::new();
        groups.hide(GroupId::new(1));
        let v = Visibility::of_group(GroupId::new(1)).with_tag(VisibilityTag::Camera(id(5)));

        assert!(
            v.visible_for(Some(id(5)), &groups),
            "相机 tag 命中 ⇒ 可见（不因组关闭而消失）"
        );
        assert!(
            !v.visible_for(Some(id(6)), &groups),
            "两个 tag 都不命中 ⇒ 不可见"
        );
        assert!(
            v.visible_for(Some(id(6)), &GroupVisibility::new()),
            "组打开时组 tag 命中"
        );
    }

    /// `Name` / `Group` 是纯数据载体（构造与取值）。
    #[test]
    fn name_and_group_carry_identity() {
        assert_eq!(Name::new("curve").as_str(), "curve");
        let g = Group::new(GroupId::new(9));
        assert_eq!(g.id(), GroupId::new(9));
        assert_ne!(GroupId::new(1), GroupId::new(2));
    }
}
