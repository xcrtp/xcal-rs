// id.rs — 场景对象的身份。
//
// **决策（M2-1 ②）**：`ObjectId` 带 `generation`。这是唯一能防止"句柄指向已销毁
// 对象"的机制 —— 销毁并复用 index 后，旧句柄的 generation 与当前值不再相同，
// 比对即可判定失效。hecs 的 `Entity` 自带 `{ id, generation }`，此处照搬同一模型
// （契约层不含 hecs，只借用它的模型）。

/// 场景对象的稳定句柄：`index`（槽位）+ `generation`（代）。
///
/// - `index` 在对象销毁后可被复用；
/// - 每次复用 `generation` 递增 ⇒ 旧句柄不会"意外指向新对象"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    /// 槽位下标（可复用）。
    pub index: u32,
    /// 代：槽位每被复用一次 +1。**不变量**：恒 ≥ 1（0 保留给"永不匹配"）。
    pub generation: u32,
}

impl ObjectId {
    /// 永不匹配任何对象的哨兵句柄（`index` / `generation` 取最大值）。
    pub const DANGLING: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
    };

    /// 构造句柄（`generation` 由分配方保证 ≥ 1）。
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// 句柄是否已失效：`self` 与**同一 index 处当前句柄** `current` 不同代。
    ///
    /// - 不同 index ⇒ 不是同一个槽位，按"已失效"处理（调用方可先用 `index` 自检）；
    /// - 同 index 同代 ⇒ 仍指向活对象。
    pub fn is_stale(&self, current: ObjectId) -> bool {
        self.index != current.index || self.generation != current.generation
    }
}

impl Default for ObjectId {
    fn default() -> Self {
        Self::DANGLING
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **可证伪**：销毁 → 复用同一 index 后，旧句柄必须被判为失效。
    #[test]
    fn object_id_generation_detects_stale_handle() {
        let before_despawn = ObjectId::new(7, 1);

        // 销毁后该槽位复用，generation 递增：
        let after_respawn = ObjectId::new(7, 2);

        assert!(
            before_despawn.is_stale(after_respawn),
            "旧句柄必须被判为失效（generation 不同）"
        );
        assert!(
            !after_respawn.is_stale(after_respawn),
            "当前句柄对自身必须仍有效"
        );
        assert!(
            before_despawn.is_stale(ObjectId::new(8, 1)),
            "不同槽位不得视为同一对象"
        );
    }

    #[test]
    fn dangling_never_matches() {
        assert!(ObjectId::DANGLING.is_stale(ObjectId::new(0, 1)));
        assert_eq!(ObjectId::default(), ObjectId::DANGLING);
    }
}
