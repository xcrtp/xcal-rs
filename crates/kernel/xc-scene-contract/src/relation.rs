// relation.rs — 场景对象之间的关系。
//
// **决策（M2-1 ③）**：`Parent` 归契约层。它描述"场景中的关系"，不是数学概念，
// 故不进理念层；此前它定义在 `xcal-scene/src/scene/components.rs` 且**全仓零使用**，
// 本轮是它第一次有明确归属。
//
// ⚠️ **本轮的 `Parent` 仍无消费方**（父子变换累乘 System 延后到 M4）。这是
// "先建机制后接线"的**已知未接线项**，与 `UvProjection` 同类 —— 只要用途明确
// （M4 的 `Axes2D::build` 会用）而非凭空设计，就不是问题，但必须记账。

use crate::id::ObjectId;

/// 父对象引用（组件形态：挂在**子**对象上）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Parent(pub ObjectId);

impl Parent {
    pub const fn new(parent: ObjectId) -> Self {
        Self(parent)
    }

    pub const fn id(&self) -> ObjectId {
        self.0
    }
}

/// 子对象列表（组件形态：挂在**父**对象上）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Children(pub Vec<ObjectId>);

impl Children {
    pub fn new(children: Vec<ObjectId>) -> Self {
        Self(children)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 追加子对象（不查重 —— 去重是装配层策略）。
    pub fn push(&mut self, child: ObjectId) {
        self.0.push(child);
    }

    /// 移除子对象，返回是否命中。
    pub fn remove(&mut self, child: ObjectId) -> bool {
        let before = self.0.len();
        self.0.retain(|c| *c != child);
        self.0.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_push_remove() {
        let a = ObjectId::new(1, 1);
        let b = ObjectId::new(2, 1);
        let mut c = Children::default();
        assert!(c.is_empty());
        c.push(a);
        c.push(b);
        assert_eq!(c.len(), 2);
        assert!(c.remove(a));
        assert!(!c.remove(a));
        assert_eq!(c.0, vec![b]);
    }

    #[test]
    fn parent_roundtrip() {
        let id = ObjectId::new(3, 5);
        assert_eq!(Parent::new(id).id(), id);
    }
}
