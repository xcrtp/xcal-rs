// persist.rs — 场景组件 / 脚本的持久化（tagged JSON）。
//
// # 为什么需要自描述格式
//
// `Components` 是**异构动态集合**（每对象组件数、类型都不同）⇒ 不能直接
// derive；每个元素写成：
//
// ```json
// { "type": "Circle", "data": { ... } }
// ```
//
// `type` 直接复用 `known_component_table!` 的 `stringify!` 名字
// （与 `known_type_name` 同一来源），类型名 ↔ 构造器 ↔ 序列化器三件事合一。
//
// # 显式错误，不降级
//
// 未登记类型 / 数据不合法 / 匿名函数 ⇒ [`PersistError`]。
// **绝不**静默丢组件（那会重演"少了一半组件"的坑）。

use std::fmt;

use serde::{Deserialize, Serialize};
use xc_scene_contract::Components;

/// 持久化错误（全部是明确错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistError {
    /// 组件类型未在登记表里（`std::any::type_name` 形式，便于诊断）。
    UnregisteredComponent(String),
    /// 组件编码失败（最常见：匿名函数 —— 序列化只写名字，匿名没有名字）。
    SerializeComponent {
        type_name: &'static str,
        message: String,
    },
    /// 反序列化遇到未登记的类型名。
    UnknownComponentType(String),
    /// 组件解码失败（数据与该类型不匹配）。
    DeserializeComponent { type_name: String, message: String },
    /// 动画目标只有 `Ready`（已解析的 trait object）—— 不可持久化。
    UnsupportedTarget(String),
    /// 动画目标名字未在构造器表中。
    UnknownTarget(String),
    /// JSON 文本层错误。
    Json(String),
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnregisteredComponent(name) => {
                write!(f, "未登记的组件类型：{name}")
            }
            Self::SerializeComponent { type_name, message } => {
                write!(f, "组件 {type_name} 序列化失败：{message}")
            }
            Self::UnknownComponentType(name) => {
                write!(f, "未知的组件类型名：{name}")
            }
            Self::DeserializeComponent { type_name, message } => {
                write!(f, "组件 {type_name} 反序列化失败：{message}")
            }
            Self::UnsupportedTarget(name) => {
                write!(
                    f,
                    "动画目标 {name} 不可持久化（只支持场景脚本的 Pending 目标）"
                )
            }
            Self::UnknownTarget(name) => {
                write!(f, "未知的动画目标名：{name}")
            }
            Self::Json(message) => write!(f, "JSON 错误：{message}"),
        }
    }
}

impl std::error::Error for PersistError {}

/// 一个自描述组件：`{ "type": "...", "data": ... }`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaggedComponent {
    /// 稳定类型名（`known_component_table!` 的 `stringify!`）。
    #[serde(rename = "type")]
    pub type_name: String,
    /// 组件载荷。
    pub data: serde_json::Value,
}

/// 把 `Components` 编码为 tagged 列表。
pub fn serialize_components(components: &Components) -> Result<Vec<TaggedComponent>, PersistError> {
    components
        .iter()
        .map(|c| {
            // 显式解引用到 `dyn Component`：`Box<dyn Component>` 本身也满足
            // blanket `Component` impl，直接 `c.as_any()` 会拿到 Box 的 Any 而不是组件的。
            let (type_name, data) = crate::contract::serialize_component((**c).as_any())?;
            Ok(TaggedComponent {
                type_name: type_name.to_string(),
                data,
            })
        })
        .collect()
}

/// 把 tagged 列表解码为 `Components`（任一失败 ⇒ 整个失败，不静默丢组件）。
///
/// **默认绑定**：函数类组件的"名字壳"由 [`crate::math_vm::math_vm`] 按名反查
/// （内置数学函数无需调用方再手动绑定）。未知名字保持未绑定。
pub fn deserialize_components(
    tagged: Vec<TaggedComponent>,
) -> Result<Components, PersistError> {
    let mut components: Components = tagged
        .into_iter()
        .map(|t| crate::contract::deserialize_component(&t.type_name, t.data))
        .collect::<Result<_, _>>()?;
    crate::math_vm::rebind_components(&mut components);
    Ok(components)
}
