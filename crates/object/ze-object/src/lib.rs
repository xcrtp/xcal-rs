// ze-object — 场景对象层（**兼容壳层**，B3-1f 起）
//
// 内容已并入 `ze-scene-core::object`（B3-1b）；本 crate 只做 re-export，
// 让 123 处 `ze_object::*` 下游 import 在分批迁移期间保持不变（D6）。
//
// 迁移完成后本 crate 会被删除；新代码请直接用 `ze_scene_core::`。

pub use ze_scene_core::object::*;
