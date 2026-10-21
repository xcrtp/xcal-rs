// light_queue.rs — 本帧场景光源快照（渲染层，后端 uniform 友好）。
//
// 与 `zela_scene_ecs::scene::components::Light` 组件一一对应，但已解析成后端消费
// 的形状（颜色归一化到 [0,1]、含强度）：
// - 方向光：`direction`（世界空间）+ 颜色 + 强度；
// - 点光（B1-B）：`position` 已由录制侧从 `Transform` → `WorldTransforms` 解析
//   （单一真源：`Light` 组件本身不带位置）+ 颜色 + 强度 + range。
//
// `LightQueue` 是 `SceneRecord3DSystem` 的写集资源；后端每帧按它上传
// **场景级** light uniform buffer（所有 draw 共享一份，铁律 3）。

use ze_notion::{Color32, Vec3};

/// 方向光（渲染层形态，uniform 友好）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLightRecord {
    /// 光传播方向（世界空间，从光源指向场景）。
    pub direction: Vec3,
    /// 光的颜色（rgba 归一化；alpha 恒 1.0，不参与光照）。
    pub color: [f32; 4],
    /// 强度倍率（默认 1.0）。
    pub intensity: f32,
}

impl DirectionalLightRecord {
    /// 由场景组件字段构造（颜色归一化到 [0,1]）。
    pub fn new(direction: Vec3, color: Color32, intensity: f32) -> Self {
        Self {
            direction,
            color: [
                color.r as f32 / 255.0,
                color.g as f32 / 255.0,
                color.b as f32 / 255.0,
                1.0,
            ],
            intensity,
        }
    }
}

/// 点光（渲染层形态，uniform 友好；B1-B）。
///
/// `position` 是**世界空间**光源位置 —— 录制侧由 `Light` 组件所在对象的
/// `Transform` → `WorldTransforms` 解析（`Light` 组件本身不带位置，单一真源）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLightRecord {
    /// 光源位置（世界空间）。
    pub position: Vec3,
    /// 光的颜色（rgba 归一化；alpha 恒 1.0，不参与光照）。
    pub color: [f32; 4],
    /// 强度倍率（默认 1.0）。
    pub intensity: f32,
    /// 有效半径（世界单位）：超出即衰减到 0（窗口衰减）。
    pub range: f32,
}

impl PointLightRecord {
    /// 由场景组件字段构造（颜色归一化到 [0,1]）。
    pub fn new(position: Vec3, color: Color32, intensity: f32, range: f32) -> Self {
        Self {
            position,
            color: [
                color.r as f32 / 255.0,
                color.g as f32 / 255.0,
                color.b as f32 / 255.0,
                1.0,
            ],
            intensity,
            range,
        }
    }
}

/// 本帧场景光源快照：每帧由 `SceneRecord3DSystem` 从 `Light` 组件录制。
///
/// 空队列 = **默认光照**（后端兜底：单方向光 `(0,0,1)` 纯白 + ambient 0.2），
/// 与 B1-A 之前的硬编码行为逐位一致。
#[derive(Default)]
pub struct LightQueue {
    /// 方向光列表。
    pub directional: Vec<DirectionalLightRecord>,
    /// 点光列表（B1-B）。
    pub points: Vec<PointLightRecord>,
}

impl LightQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空队列（每帧录制前调用）。
    pub fn clear(&mut self) {
        self.directional.clear();
        self.points.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.directional.is_empty() && self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.directional.len() + self.points.len()
    }
}
