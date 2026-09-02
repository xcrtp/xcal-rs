// access.rs
//
// Access：声明 System 对 Resource 的访问模式（read / write）。
//
// 用途：
// - `System::Reads` / `System::Writes` 关联类型通过 `ResourceTuple::ids()`
//   展开为 `TypeId` 列表，再构造对应的 `Access`。
// - `Scheduler::add_system` 在 debug 模式下检测同一 phase 内
//   `read/write` 冲突，冲突时 panic 带系统名。
// - 通过 `Access::union` 可以合并一个 phase 内所有 system 的访问模式，
//   实现增量冲突检测。

use std::any::TypeId;
use std::collections::HashSet;

use crate::resource::ResourceTuple;

/// 描述单个 System 声明对一组 Resource 的访问模式。
///
/// 不区分 `&T` 与 `&mut T`，而是把：
/// - 只读访问放入 `reads`
/// - 写入访问放入 `writes`
///
/// `writes` 隐含包含对同名资源的读访问（写者也需要先读）；
/// `conflicts_with` 按以下规则判断：
/// - 任意一方的 `writes` 与另一方的 `reads` 或 `writes` 重叠 → 冲突
/// - 两方都只在 `reads` 中重叠 → 不冲突（多读者允许）
#[derive(Debug, Default, Clone)]
pub struct Access {
    pub reads: HashSet<TypeId>,
    pub writes: HashSet<TypeId>,
}

impl Access {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从两个 `ResourceTuple` 构造 `Access`：
    /// - `reads` 元组中的每个 TypeId 进入 `reads`
    /// - `writes` 元组中的每个 TypeId 进入 `writes`
    pub fn from_tuples<R, W>() -> Self
    where
        R: ResourceTuple,
        W: ResourceTuple,
    {
        let mut access = Self::new();
        for id in R::ids() {
            access.reads.insert(id);
        }
        for id in W::ids() {
            access.writes.insert(id);
        }
        access
    }

    /// 当前 `Access` 与 `other` 是否存在冲突。
    ///
    /// 冲突规则：
    /// - `self.writes` ∩ (`other.reads` ∪ `other.writes`) ≠ ∅
    /// - `other.writes` ∩ (`self.reads` ∪ `self.writes`) ≠ ∅
    /// - 仅有 `reads` 重叠时不算冲突（多读者允许）
    pub fn conflicts_with(&self, other: &Access) -> bool {
        // self 写与 other 任何访问冲突
        if self.writes.iter().any(|w| other.reads.contains(w) || other.writes.contains(w)) {
            return true;
        }
        // other 写与 self 任何访问冲突
        if other.writes.iter().any(|w| self.reads.contains(w) || self.writes.contains(w)) {
            return true;
        }
        false
    }

    /// 将 `other` 的访问模式合并到 `self`（用于增量检查）。
    pub fn union(&mut self, other: &Access) {
        for r in &other.reads {
            self.reads.insert(*r);
        }
        for w in &other.writes {
            self.writes.insert(*w);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.writes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid<T: 'static>() -> TypeId {
        TypeId::of::<T>()
    }

    #[test]
    fn test_empty_access_no_conflict() {
        let a = Access::new();
        let b = Access::new();
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn test_double_read_no_conflict() {
        let mut a = Access::new();
        let mut b = Access::new();
        a.reads.insert(tid::<i32>());
        b.reads.insert(tid::<i32>());
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn test_write_vs_read_conflict() {
        let mut a = Access::new();
        let mut b = Access::new();
        a.writes.insert(tid::<i32>());
        b.reads.insert(tid::<i32>());
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn test_write_vs_write_conflict() {
        let mut a = Access::new();
        let mut b = Access::new();
        a.writes.insert(tid::<i32>());
        b.writes.insert(tid::<i32>());
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn test_disjoint_no_conflict() {
        let mut a = Access::new();
        let mut b = Access::new();
        a.writes.insert(tid::<i32>());
        b.writes.insert(tid::<f64>());
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn test_union_combines() {
        let mut a = Access::new();
        let mut b = Access::new();
        a.reads.insert(tid::<i32>());
        b.writes.insert(tid::<f64>());
        a.union(&b);
        assert!(a.reads.contains(&tid::<i32>()));
        assert!(a.writes.contains(&tid::<f64>()));
    }

    #[test]
    fn test_from_tuples_constructs_access() {
        let access = Access::from_tuples::<(i32,), (f64,)>();
        assert!(access.reads.contains(&tid::<i32>()));
        assert!(access.writes.contains(&tid::<f64>()));
    }

    #[test]
    fn test_nil_access_is_empty() {
        let access = Access::from_tuples::<(), ()>();
        assert!(access.is_empty());
    }
}
