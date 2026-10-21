// registry.rs — **组件登记表（唯一来源）**：正向 downcast、类型名反查、克隆、serde、
// 以及 ECS 侧的类型擦除写入分发共用这一份（B3-1b 从 `zela-scene::contract` 下沉）。
//
// # 为什么在核心层
// 登记表登记的全是**核心层的具体组件类型**（图元 / 相机 / 3D 装配 / 组织关系）。
// 它既被 ECS 的 `spawn_dyn` 用（`add_known`），也被脚本数据 `SceneScript` 的
// 持久化用（`persist`）—— 后者必须留在核心层，故登记表也必须在这里。
//
// # 一张表 + 多个展开器
// 核心约束是"新增一个已登记类型只写一处"：`known_type_name` / `clone_known` /
// `serialize_component` / `deserialize_component` 以及 ECS 侧的 `add_known` /
// `SceneWrite` 分发全部由本表展开，各自只需一个极薄的 `macro_rules!` 臂。

use std::sync::Arc;

use ze_scene_contract::{Component, Group, LifetimeTracked, Name, Parent, Visibility};
use ze_render_ir::{FillSolid, Opacity, StrokeSolid};

use crate::components::{ContentTextureItem, Light, Material3D, MeshGeometry, MorphShape, PhaseAnim, Reveal};
use crate::composer::Composer;
use crate::domain::CameraMode;
use crate::surface::{Surface, SurfaceRenderer};
use crate::object::{
    Arrow, Axes2D, AxesLine, Camera2D, Camera3D, Circle, FpsView, Framing2D, FunctionGraph, Line,
    Path, Projection, Rect, Transform, ZOrder,
};

/// **组件登记表（唯一来源）**。
///
/// `clone:` 段的类型必须 `Clone`（`Components` 的元素要求）—— 这是
/// `SceneScript: Clone` 的直接前提。
#[macro_export]
macro_rules! known_component_table {
    ($mac:ident) => {
        $mac! {
            clone: Circle, Line, Path, Rect, Arrow, Transform, ZOrder,
                   Name, Group, Visibility, Opacity,
                   Camera2D, Camera3D, Projection, Framing2D, CameraMode, FpsView,
                   StrokeSolid, FillSolid,
                   Axes2D, AxesLine, FunctionGraph,
                   PhaseAnim, MorphShape, ContentTextureItem, Reveal, Parent,
                   // B3 Step 3：2D/3D 融合边界（Surface 是数据组件，不含 position）。
                   Surface, SurfaceRenderer,
                   // B1-D：3D 装配组件（脚本层入口）。`Arc<MeshGeometry>` 是
                   // 录制系统读取的类型（记录时以 Arc 组件遍历）；Arc 本身 Clone /
                   // Serialize / Deserialize（MeshGeometry 派生 serde）。
                   LifetimeTracked, Composer, Arc<MeshGeometry>, Material3D, Light;
            manual:
        }
    };
}

/// 反向能力 ①：已登记类型的**稳定名字**（即 Rust 类型名）。
pub fn known_type_name<T: 'static>() -> Option<&'static str> {
    macro_rules! name_arms {
        (clone: $($c:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if std::any::TypeId::of::<T>() == std::any::TypeId::of::<$c>() {
                    return Some(stringify!($c));
                }
            )+
            $(
                if std::any::TypeId::of::<T>() == std::any::TypeId::of::<$m>() {
                    return Some(stringify!($m));
                }
            )*
        }};
    }
    known_component_table!(name_arms);
    None
}

/// 反向能力 ②：克隆一个**类型擦除**的已登记组件。
pub fn clone_known(
    c: &(dyn std::any::Any + Send + Sync),
) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    macro_rules! clone_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if let Some(v) = c.downcast_ref::<$t>() {
                    return Some(Box::new(v.clone()));
                }
            )+
            $(
                if let Some(v) = c.downcast_ref::<$m>() {
                    let make = $ctor;
                    return Some(Box::new(make(v)));
                }
            )*
        }};
    }
    known_component_table!(clone_arms);
    None
}

/// **组件持久化（编码）**：按登记表把组件写成 `(稳定类型名, JSON 值)`。
pub fn serialize_component(
    c: &dyn std::any::Any,
) -> Result<(&'static str, serde_json::Value), crate::persist::PersistError> {
    macro_rules! ser_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if let Some(v) = c.downcast_ref::<$t>() {
                    return serde_json::to_value(v)
                        .map(|data| (stringify!($t), data))
                        .map_err(|e| crate::persist::PersistError::SerializeComponent {
                            type_name: stringify!($t),
                            message: e.to_string(),
                        });
                }
            )+
            $(
                if let Some(v) = c.downcast_ref::<$m>() {
                    return serde_json::to_value(v)
                        .map(|data| (stringify!($m), data))
                        .map_err(|e| crate::persist::PersistError::SerializeComponent {
                            type_name: stringify!($m),
                            message: e.to_string(),
                        });
                }
            )*
        }};
    }
    known_component_table!(ser_arms);

    Err(crate::persist::PersistError::UnregisteredComponent(
        std::any::type_name_of_val(c).to_string(),
    ))
}

/// **组件持久化（解码）**：按稳定类型名重建组件。
pub fn deserialize_component(
    type_name: &str,
    data: serde_json::Value,
) -> Result<Box<dyn Component>, crate::persist::PersistError> {
    macro_rules! de_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if type_name == stringify!($t) {
                    return serde_json::from_value::<$t>(data)
                        .map(|v| Box::new(v) as Box<dyn Component>)
                        .map_err(|e| crate::persist::PersistError::DeserializeComponent {
                            type_name: type_name.to_string(),
                            message: e.to_string(),
                        });
                }
            )+
            $(
                if type_name == stringify!($m) {
                    return serde_json::from_value::<$m>(data)
                        .map(|v| Box::new(v) as Box<dyn Component>)
                        .map_err(|e| crate::persist::PersistError::DeserializeComponent {
                            type_name: type_name.to_string(),
                            message: e.to_string(),
                        });
                }
            )*
        }};
    }
    known_component_table!(de_arms);

    Err(crate::persist::PersistError::UnknownComponentType(
        type_name.to_string(),
    ))
}
