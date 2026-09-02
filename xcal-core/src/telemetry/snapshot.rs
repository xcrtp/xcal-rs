// telemetry/snapshot.rs
//
// World 状态快照：把某一帧的 ECS 实体/组件与 Resources 导出为可序列化的
// `WorldSnapshot`（JSON），用于 bug 复现、节点连线前后状态对比、动画帧对比。
//
// 设计：
// - hecs 不提供按 `TypeId` 动态取组件值的能力，因此由 [`SnapshotRegistry`]
//   在启动时显式注册关心的组件类型（`register_component::<T>("Name")`）；
//   注册只需 `T: Debug`，不强制 `Serialize` 派生（Debug 兜底）。
// - 未注册的组件类型仍会出现在快照中（type_name = "<unregistered>"），
//   保证实体结构完整可见。
// - 快照数据结构全部为 owned（String），支持 Serialize + Deserialize 双向。

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::resource::Resources;

/// 未注册类型在快照中的占位名。
pub const UNREGISTERED: &str = "<unregistered>";

/// 一帧的 World 快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub tick: u64,
    pub entities: Vec<EntitySnapshot>,
    pub resources: Vec<ResourceSnapshot>,
}

/// 单个实体的快照：实体 id + 其全部组件信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub id: u64,
    pub components: Vec<ComponentInfo>,
}

/// 单个组件的类型名与简短摘要（Debug 文本，截断到 200 字符）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub type_name: String,
    pub summary: String,
}

/// 单个 Resource 的类型名。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub type_name: String,
}

/// 类型注册表：`TypeId` → （显示名, 取值函数）。
pub struct SnapshotRegistry {
    components: HashMap<TypeId, ComponentEntry>,
    resources: HashMap<TypeId, &'static str>,
}

struct ComponentEntry {
    name: &'static str,
    /// 从 World 中取出指定实体的该组件并 Debug 格式化。
    extract: fn(&hecs::World, hecs::Entity) -> Option<String>,
}

impl Default for SnapshotRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotRegistry {
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
            resources: HashMap::new(),
        }
    }

    /// 注册一个组件类型。`T` 只需满足 `hecs::Component + Debug`，
    /// 无需 `Serialize`。
    pub fn register_component<T: hecs::Component + Debug>(&mut self, name: &'static str) {
        self.components.insert(
            TypeId::of::<T>(),
            ComponentEntry {
                name,
                extract: extract_component::<T>,
            },
        );
    }

    /// 注册一个 Resource 类型名（Resource 不导出值，仅记录存在性）。
    pub fn register_resource<T: 'static>(&mut self, name: &'static str) {
        self.resources.insert(TypeId::of::<T>(), name);
    }

    /// 采集当前 World + Resources 的快照。
    pub fn snapshot(&self, world: &hecs::World, resources: &Resources, tick: u64) -> WorldSnapshot {
        let mut entities = Vec::new();
        for entity_ref in world.iter() {
            // hecs 0.11：World::iter() 直接产出 EntityRef（非 (Entity, EntityRef) 元组）。
            let entity = entity_ref.entity();
            let mut components = Vec::new();
            for ty in entity_ref.component_types() {
                let (type_name, summary) = match self.components.get(&ty) {
                    Some(entry) => {
                        let summary = (entry.extract)(world, entity).unwrap_or_default();
                        (entry.name.to_string(), summary)
                    }
                    None => (UNREGISTERED.to_string(), String::new()),
                };
                components.push(ComponentInfo { type_name, summary });
            }
            entities.push(EntitySnapshot {
                id: entity.to_bits().get(),
                components,
            });
        }

        let resources = resources
            .type_ids()
            .map(|ty| {
                let name = self
                    .resources
                    .get(&ty)
                    .copied()
                    .unwrap_or(UNREGISTERED)
                    .to_string();
                ResourceSnapshot { type_name: name }
            })
            .collect();

        WorldSnapshot {
            tick,
            entities,
            resources,
        }
    }
}

/// 泛型取值函数：monomorphize 后以 fn 指针存入注册表。
fn extract_component<T: hecs::Component + Debug>(
    world: &hecs::World,
    entity: hecs::Entity,
) -> Option<String> {
    let value = world.entity(entity).ok()?.get::<&T>()?;
    Some(truncate_summary(&format!("{value:?}")))
}

/// 组件摘要可能很长（如采样点数组），截断避免快照膨胀。
fn truncate_summary(text: &str) -> String {
    const MAX: usize = 200;
    if text.len() <= MAX {
        text.to_string()
    } else {
        let mut end = MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...(truncated)", &text[..end])
    }
}

/// 把快照序列化为 pretty JSON 并写入文件。
pub fn write_snapshot_json(
    path: impl AsRef<Path>,
    snapshot: &WorldSnapshot,
) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, snapshot).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试夹具：字段仅用于 Debug 格式化快照摘要，不直接读取。
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Position(f32, f32);

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Velocity(f32, f32);

    fn test_world() -> hecs::World {
        let mut world = hecs::World::new();
        world.spawn((Position(1.0, 2.0), Velocity(0.5, -0.5)));
        world.spawn((Position(3.0, 4.0),));
        world.spawn(("untagged",)); // 未注册组件
        world
    }

    fn test_registry() -> SnapshotRegistry {
        let mut reg = SnapshotRegistry::new();
        reg.register_component::<Position>("Position");
        reg.register_component::<Velocity>("Velocity");
        reg
    }

    #[test]
    fn test_snapshot_lists_all_entities_and_registered_components() {
        let world = test_world();
        let reg = test_registry();
        let snap = reg.snapshot(&world, &Resources::new(), 7);

        assert_eq!(snap.tick, 7);
        assert_eq!(snap.entities.len(), 3);

        // 第一个实体：Position + Velocity 均已注册
        let first = &snap.entities[0];
        let names: Vec<&str> = first
            .components
            .iter()
            .map(|c| c.type_name.as_str())
            .collect();
        assert!(names.contains(&"Position"));
        assert!(names.contains(&"Velocity"));

        // 第三个实体：未注册组件以占位名出现
        let untagged = &snap.entities[2];
        assert_eq!(untagged.components[0].type_name, UNREGISTERED);
    }

    #[test]
    fn test_snapshot_summary_uses_debug() {
        let world = test_world();
        let reg = test_registry();
        let snap = reg.snapshot(&world, &Resources::new(), 0);
        let pos_info = snap.entities[0]
            .components
            .iter()
            .find(|c| c.type_name == "Position")
            .unwrap();
        assert!(pos_info.summary.contains("1.0"));
        assert!(pos_info.summary.contains("2.0"));
    }

    #[test]
    fn test_snapshot_json_roundtrip() {
        let world = test_world();
        let reg = test_registry();
        let snap = reg.snapshot(&world, &Resources::new(), 3);

        let json = serde_json::to_string_pretty(&snap).unwrap();
        assert!(json.contains("\"tick\""));
        assert!(json.contains("Position"));

        // 可反序列化还原
        let restored: WorldSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.tick, 3);
        assert_eq!(restored.entities.len(), 3);
    }

    #[test]
    fn test_truncate_long_summary() {
        let long = "x".repeat(500);
        let t = truncate_summary(&long);
        assert!(t.len() < 220);
        assert!(t.ends_with("...(truncated)"));
    }
}
