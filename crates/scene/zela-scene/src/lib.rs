// zela-scene — 兼容壳层（B3-1d）
//
// 实现已改名为 `zela-scene-ecs`（只留 ECS：world / resources / System 接线）。
// 本 crate 只做 re-export，保持下游 `zela_scene::*` 路径不变。
//
// 分层（B3-1）：
// ```text
// ze-scene-contract   契约
//        ↑
// ze-scene-core       对象 / 组件 / 纯逻辑（零 hecs、零 zela-*）
//        ↑
// zela-scene-ecs      ECS 实现（hecs / System / 调度接线）
// ze-scene-api        Pod 能力声明（零 hecs、零 zela-*）
// ```

pub use zela_scene_ecs::*;
