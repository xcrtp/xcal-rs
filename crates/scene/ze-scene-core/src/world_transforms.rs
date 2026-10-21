// world_transforms.rs — 每帧派生的**世界矩阵**（Resource；不进持久化）。
//
// B3-1 收尾：数据结构下沉核心层（渲染层只读它，不该依赖 ECS 实现）。
// **生产**（父子累乘）在 `zela-scene-ecs::transform_hierarchy::TransformHierarchySystem`；
// 本模块只提供容器与读取口径（3D 唯一真源 + 2D 投影视图）。
//
// 组合约定（与生成侧一致）：`scale → rotation → translation`；父子
// `world = parent ∘ local`。无 `Parent` 的对象其世界矩阵与 local 逐位相同。

use std::collections::HashMap;

use ze_notion::{Transform2D, Transform3D};
use ze_scene_contract::ObjectId;

/// 每帧派生的**世界矩阵**（Resource；不进持久化）。
#[derive(Debug, Clone, Default)]
pub struct WorldTransforms {
    world: HashMap<ObjectId, Transform3D>,
}

impl WorldTransforms {
    /// 由"对象 → 世界矩阵"映射构造（生成侧每帧重建时用）。
    pub fn from_map(world: HashMap<ObjectId, Transform3D>) -> Self {
        Self { world }
    }

    /// 写入 / 覆盖一个对象的世界矩阵（测试与增量构建用）。
    pub fn insert(&mut self, id: ObjectId, transform: Transform3D) {
        self.world.insert(id, transform);
    }

    /// 取对象的**世界矩阵**（3D 唯一真源；缺席 ⇒ 该对象没有 Transform，
    /// 渲染侧回退 local / 恒等）。
    pub fn get(&self, id: ObjectId) -> Option<&Transform3D> {
        self.world.get(&id)
    }

    /// 取对象的 **2D 投影视图**（D1/D4）：3D 世界矩阵的 xy 子块。
    /// `z` 平移与 xy↔z 耦合在此被丢弃（投影，非同构）。
    pub fn get2d(&self, id: ObjectId) -> Option<Transform2D> {
        self.world.get(&id).map(|t| t.to_affine2d())
    }

    pub fn len(&self) -> usize {
        self.world.len()
    }

    pub fn is_empty(&self) -> bool {
        self.world.is_empty()
    }
}
