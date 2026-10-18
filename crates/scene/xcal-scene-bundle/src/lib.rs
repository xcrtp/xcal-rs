// xcal-scene-bundle — XSB（Xcal Scene Bundle）格式 crate。
//
// # 它是什么
//
// 「脚本 → 已装配场景」的**规范产物**：脚本在 Native 侧执行完，产出纯数据的 `.xsb`；
// 回放端只做「读产物 → 可还原性检查 → 还原 Scene → 调度推进 → 渲染」。
// 它不依赖任何语言适配器（mlua / pyo3），因此可以单独带进浏览器。
//
// # 两段布局
//
// ```text
// ┌──────────────────────────────┐
// │ Header（24 B）                │ 魔数 + 容器版本 + 段长
// ├──────────────────────────────┤
// │ Payload（hash 只覆盖这段）    │ SceneScript 的规范 JSON
// ├──────────────────────────────┤
// │ Meta（不进 hash）             │ cap / pod 版本 + 降级记录 + 源脚本 hash
// └──────────────────────────────┘
// ```
//
// # 两个断言必须分开跑（见执行计划 §2）
//
// - **内容 hash**：只覆盖 payload 字节 ⇒ 跨运行时比对渲染确定性；
// - **可还原性检查**：依据 Meta 的 cap / pod 版本 ⇒ 逐项确认当前运行时能解释。
//   hash 对得上**不代表**能还原；缺失项必须**完整列出**，不得静默降级。
//
// # 字节确定性
//
// payload 是规范 JSON：`serde_json` 的对象键按序（默认 `BTreeMap`）输出，
// 且不存在 `HashMap` 迭代；`encode` 会做一次 **round-trip 等价**检查
// （反序列化 → 再序列化必须逐字节相同），NaN / Infinity 会在这一步暴露为错误。

use serde::{Deserialize, Serialize};

pub mod sha256;

use xcal_scene::scene_script::SceneScript;

/// 文件魔数（4 字节）。
pub const MAGIC: &[u8; 4] = b"XSB1";
/// 当前容器版本（决定"怎么读字节"，不属于内容 hash）。
pub const CONTAINER_VERSION: u16 = 1;
/// payload 的格式标识（写入 Meta）。
pub const PAYLOAD_FORMAT: &str = "xsb.script/1";
const HEADER_LEN: usize = 24;

/// 一条能力版本声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    /// `None` = 该能力的实现没有声明版本（仍要求名字存在）。
    pub version: Option<String>,
}

impl Capability {
    pub fn new(name: impl Into<String>, version: Option<&str>) -> Self {
        Self {
            name: name.into(),
            version: version.map(str::to_string),
        }
    }
}

/// 一条 Pod 版本声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pod {
    pub name: String,
    pub version: String,
}

impl Pod {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// 一条**降级记录**：哪个 Custom 规格走了哪条降级路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Downgrade {
    pub spec: String,
    pub path: String,
}

/// Meta 段（不进内容 hash）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// 容器版本（冗余写入，便于诊断；读取以 Header 为准）。
    pub container: u16,
    /// payload 格式标识。
    pub payload_format: String,
    /// 能力版本清单（可还原性检查的依据）。
    pub capabilities: Vec<Capability>,
    /// Pod 版本清单。
    pub pods: Vec<Pod>,
    /// 降级记录（影响"一比一还原"能力）。
    pub downgrades: Vec<Downgrade>,
    /// 源脚本 SHA-256（诊断用，不进内容 hash）。
    pub source_sha256: Option<String>,
    /// 场景时长（秒；从 SceneScript 重算，作为交叉校验）。
    pub duration: f64,
    /// 对象数（交叉校验）。
    pub object_count: usize,
    /// payload 的 SHA-256（读取时校验；不参与自身的计算）。
    pub payload_sha256: String,
}

/// `encode` 的元信息输入（payload 侧的信息由实现自行填充）。
#[derive(Debug, Clone, Default)]
pub struct MetaInput {
    pub capabilities: Vec<Capability>,
    pub pods: Vec<Pod>,
    pub downgrades: Vec<Downgrade>,
    pub source_sha256: Option<String>,
}

/// 当前运行时**能解释什么**（可还原性检查的输入）。
#[derive(Debug, Clone, Default)]
pub struct Available {
    pub capabilities: Vec<Capability>,
    pub pods: Vec<Pod>,
}

impl Available {
    pub fn new(capabilities: Vec<Capability>, pods: Vec<Pod>) -> Self {
        Self {
            capabilities,
            pods,
        }
    }
}

/// 写入 / 读取失败的**显式**错误。
#[derive(Debug, Clone, PartialEq)]
pub enum BundleError {
    /// 字节数不足一个 Header。
    TooShort { len: usize },
    /// 魔数不匹配。
    BadMagic { found: [u8; 4] },
    /// 容器版本不受支持。
    UnsupportedContainer { found: u16 },
    /// 段长越界 / 文件被截断 / 尾部多余字节。
    MalformedLayout { detail: String },
    /// payload / meta 不是合法 JSON。
    Json { section: &'static str, message: String },
    /// `SceneScript` 还原失败（组件未登记 / 类型不匹配等）。
    Script { message: String },
    /// 序列化不具确定性（round-trip 不是逐字节相同，通常是 NaN / Infinity）。
    NonCanonical { message: String },
    /// payload hash 与 Meta 记录不一致。
    HashMismatch { stored: String, actual: String },
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "XSB 太短（{len} B < {HEADER_LEN} B）"),
            Self::BadMagic { found } => {
                write!(f, "XSB 魔数不匹配（找到 {found:?}，期望 {MAGIC:?}）")
            }
            Self::UnsupportedContainer { found } => write!(
                f,
                "不支持的 XSB 容器版本 {found}（本实现支持 {CONTAINER_VERSION}）"
            ),
            Self::MalformedLayout { detail } => write!(f, "XSB 布局非法：{detail}"),
            Self::Json { section, message } => write!(f, "XSB {section} 段 JSON 非法：{message}"),
            Self::Script { message } => write!(f, "XSB payload 还原失败：{message}"),
            Self::NonCanonical { message } => write!(f, "XSB payload 不具确定性：{message}"),
            Self::HashMismatch { stored, actual } => {
                write!(f, "XSB payload hash 不一致（记录 {stored}，实际 {actual}）")
            }
        }
    }
}

impl std::error::Error for BundleError {}

/// 可还原性检查失败：**完整清单**（缺哪个 cap / 版本不匹配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreError {
    pub missing: Vec<String>,
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "XSB 无法在当前运行时还原（共 {} 项）：",
            self.missing.len()
        )?;
        for item in &self.missing {
            writeln!(f, "  - {item}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RestoreError {}

/// 一个已解析的 XSB 产物（payload 保持原始字节，hash 因此可复算）。
#[derive(Debug, Clone)]
pub struct Bundle {
    payload: Vec<u8>,
    meta: Meta,
}

impl Bundle {
    /// 从装配产物 + 元信息编码。
    ///
    /// - payload = `SceneScript` 的规范 JSON；
    /// - 会做一次 round-trip 等价检查（防止 NaN / Infinity / 非确定序列化）；
    /// - `duration` 必须有限。
    pub fn encode(script: &SceneScript, input: MetaInput) -> Result<Self, BundleError> {
        let text = script
            .to_json()
            .map_err(|e| BundleError::Script {
                message: e.to_string(),
            })?;

        // 字节确定性 / 有限性：反序列化再序列化必须逐字节相同。
        // NaN / Infinity 在 serde_json 中会退化成 `null`，round-trip 会在
        // 反序列化时报错或被下面的比较抓住。
        let back = SceneScript::from_json(&text).map_err(|e| BundleError::NonCanonical {
            message: e.to_string(),
        })?;
        let again = back.to_json().map_err(|e| BundleError::NonCanonical {
            message: e.to_string(),
        })?;
        if again != text {
            return Err(BundleError::NonCanonical {
                message: "反序列化后再序列化的字节不同（含 NaN / Infinity 或非确定顺序）"
                    .to_string(),
            });
        }

        let duration = script.duration();
        if !duration.is_finite() {
            return Err(BundleError::NonCanonical {
                message: format!("时长不是有限数：{duration}"),
            });
        }

        let payload = text.into_bytes();
        let payload_sha256 = sha256::hex(&sha256::sha256(&payload));
        let meta = Meta {
            container: CONTAINER_VERSION,
            payload_format: PAYLOAD_FORMAT.to_string(),
            capabilities: input.capabilities,
            pods: input.pods,
            downgrades: input.downgrades,
            source_sha256: input.source_sha256,
            duration,
            object_count: script.len(),
            payload_sha256,
        };
        Ok(Self { payload, meta })
    }

    /// 二进制布局：`Header(24) | payload | meta`。
    pub fn to_bytes(&self) -> Vec<u8> {
        let meta = serde_json::to_vec(&self.meta).expect("Meta 只含可序列化字段");
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len() + meta.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // flags（保留）
        out.extend_from_slice(&(self.payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&(meta.len() as u64).to_le_bytes());
        debug_assert_eq!(out.len(), HEADER_LEN);
        out.extend_from_slice(&self.payload);
        out.extend_from_slice(&meta);
        out
    }

    /// 解析二进制布局并**校验 payload hash**。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BundleError> {
        if bytes.len() < HEADER_LEN {
            return Err(BundleError::TooShort { len: bytes.len() });
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[0..4]);
        if &magic != MAGIC {
            return Err(BundleError::BadMagic { found: magic });
        }
        let container = u16::from_le_bytes([bytes[4], bytes[5]]);
        if container != CONTAINER_VERSION {
            return Err(BundleError::UnsupportedContainer { found: container });
        }
        let payload_len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
        let meta_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
        let total = HEADER_LEN
            .checked_add(payload_len)
            .and_then(|v| v.checked_add(meta_len))
            .ok_or_else(|| BundleError::MalformedLayout {
                detail: "段长溢出".to_string(),
            })?;
        if bytes.len() != total {
            return Err(BundleError::MalformedLayout {
                detail: format!("文件长 {} B ≠ header 声明 {} B", bytes.len(), total),
            });
        }

        let payload = bytes[HEADER_LEN..HEADER_LEN + payload_len].to_vec();
        let meta_bytes = &bytes[HEADER_LEN + payload_len..];
        let meta: Meta =
            serde_json::from_slice(meta_bytes).map_err(|e| BundleError::Json {
                section: "Meta",
                message: e.to_string(),
            })?;

        let actual = sha256::hex(&sha256::sha256(&payload));
        if meta.payload_sha256 != actual {
            return Err(BundleError::HashMismatch {
                stored: meta.payload_sha256.clone(),
                actual,
            });
        }

        // 容器版本以 Header 为准；Meta 里的副本只做诊断。
        if meta.container != container {
            return Err(BundleError::MalformedLayout {
                detail: format!("Meta 容器版本 {} ≠ Header {container}", meta.container),
            });
        }
        Ok(Self { payload, meta })
    }

    /// 还原装配产物（纯数据，不需要脚本运行时）。
    pub fn script(&self) -> Result<SceneScript, BundleError> {
        let text = std::str::from_utf8(&self.payload).map_err(|e| BundleError::Script {
            message: e.to_string(),
        })?;
        SceneScript::from_json(text).map_err(|e| BundleError::Script {
            message: e.to_string(),
        })
    }

    /// payload 原始字节（hash 的输入）。
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// 内容 hash（只覆盖 payload 段）。
    pub fn payload_hash(&self) -> [u8; 32] {
        sha256::sha256(&self.payload)
    }

    pub fn payload_hash_hex(&self) -> String {
        sha256::hex(&self.payload_hash())
    }

    /// **可还原性检查**：逐项确认当前运行时能解释 Meta 声明的 cap / pod。
    ///
    /// 失败时返回**完整清单**（不是遇到第一个就返回）。
    pub fn check_restorable(&self, available: &Available) -> Result<(), RestoreError> {
        let mut missing = Vec::new();

        for required in &self.meta.capabilities {
            match available
                .capabilities
                .iter()
                .find(|c| c.name == required.name)
            {
                None => missing.push(format!("缺少能力 `{}`", required.name)),
                Some(found) => {
                    if required.version.is_some() && found.version != required.version {
                        missing.push(format!(
                            "能力 `{}` 版本不匹配（产物要求 {:?}，当前 {:?}）",
                            required.name, required.version, found.version
                        ));
                    }
                }
            }
        }

        for required in &self.meta.pods {
            match available.pods.iter().find(|p| p.name == required.name) {
                None => missing.push(format!("缺少 Pod `{}`", required.name)),
                Some(found) => {
                    if found.version != required.version {
                        missing.push(format!(
                            "Pod `{}` 版本不匹配（产物要求 {}，当前 {}）",
                            required.name, required.version, found.version
                        ));
                    }
                }
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(RestoreError { missing })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::Circle;
    use xcal_scene::scene_script::SceneEncoder;
    use xcal_scene::scene::Scene;

    fn tiny_script() -> SceneScript {
        let mut enc = SceneEncoder::new();
        let circle = enc.spawn(vec![Box::new(Circle::new((0.0, 0.0), 1.0))]);
        enc.animate_named(
            circle,
            "CircleRadius",
            xcal_scene::Curve::Keyframes {
                keys: vec![
                    (0.0, xcal_scene::AnimValue::Scalar(1.0)),
                    (2.0, xcal_scene::AnimValue::Scalar(3.0)),
                ],
                interpolation: xcal_scene::Interp::Linear,
            },
            2.0,
        );
        enc.finish()
    }

    fn meta_input() -> MetaInput {
        MetaInput {
            capabilities: vec![
                Capability::new("scene", Some("4")),
                Capability::new("content", Some("1")),
            ],
            pods: vec![Pod::new("scene", "0.1.0"), Pod::new("content", "0.1.0")],
            downgrades: Vec::new(),
            source_sha256: Some("deadbeef".to_string()),
        }
    }

    #[test]
    fn roundtrip_preserves_payload_and_hash() {
        let script = tiny_script();
        let bundle = Bundle::encode(&script, meta_input()).expect("encode");
        let bytes = bundle.to_bytes();
        let read = Bundle::from_bytes(&bytes).expect("decode");
        assert_eq!(read.payload_hash(), bundle.payload_hash());
        assert_eq!(read.script().expect("script").to_json().unwrap(), script.to_json().unwrap());
        assert_eq!(read.meta().object_count, script.len());
    }

    #[test]
    fn meta_changes_do_not_change_content_hash() {
        let script = tiny_script();
        let a = Bundle::encode(&script, meta_input()).expect("encode a");
        let mut other = meta_input();
        other.source_sha256 = Some("cafebabe".to_string());
        other.downgrades.push(Downgrade {
            spec: "x".to_string(),
            path: "to_path".to_string(),
        });
        let b = Bundle::encode(&script, other).expect("encode b");
        assert_eq!(a.payload_hash_hex(), b.payload_hash_hex());
        assert_ne!(a.to_bytes(), b.to_bytes(), "Meta 段应不同");
    }

    #[test]
    fn determinism_is_byte_stable() {
        let script = tiny_script();
        let a = Bundle::encode(&script, meta_input()).expect("a");
        let b = Bundle::encode(&script, meta_input()).expect("b");
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn missing_capability_is_listed_with_pod() {
        let script = tiny_script();
        let bundle = Bundle::encode(&script, meta_input()).expect("encode");
        let available = Available::new(
            vec![Capability::new("scene", Some("4"))],
            vec![Pod::new("scene", "0.1.0")],
        );
        let error = bundle
            .check_restorable(&available)
            .expect_err("缺少 content");
        assert_eq!(error.missing.len(), 2, "必须列出全部缺失项：{error}");
        assert!(error.missing.iter().any(|m| m.contains("content")));
        assert!(error.missing.iter().any(|m| m.contains("Pod")));
    }

    #[test]
    fn version_mismatch_is_explicit() {
        let script = tiny_script();
        let bundle = Bundle::encode(&script, meta_input()).expect("encode");
        let available = Available::new(
            vec![
                Capability::new("scene", Some("3")),
                Capability::new("content", Some("1")),
            ],
            vec![Pod::new("scene", "0.1.0"), Pod::new("content", "0.1.0")],
        );
        let error = bundle.check_restorable(&available).expect_err("版本不匹配");
        assert!(error.missing[0].contains("版本不匹配"));
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let script = tiny_script();
        let bundle = Bundle::encode(&script, meta_input()).expect("encode");
        let mut bytes = bundle.to_bytes();
        let last = bytes.len() - 1;
        // 改 payload 一个字节（payload 在 Header 之后、Meta 之前）。
        bytes[HEADER_LEN] ^= 0x01;
        let error = Bundle::from_bytes(&bytes).expect_err("hash 必须不一致");
        assert!(matches!(error, BundleError::HashMismatch { .. }), "error={error}");
        let _ = last;
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = Bundle::encode(&tiny_script(), meta_input())
            .expect("encode")
            .to_bytes();
        bytes[0] = b'Z';
        assert!(matches!(
            Bundle::from_bytes(&bytes),
            Err(BundleError::BadMagic { .. })
        ));
    }

    /// 还原后的场景可提交（纯数据 → Scene）。
    #[test]
    fn restored_script_commits_into_scene() {
        let bundle = Bundle::encode(&tiny_script(), meta_input()).expect("encode");
        let script = bundle.script().expect("script");
        let mut scene = Scene::new();
        let recipe = script.clone().commit(&mut scene).expect("commit");
        assert_eq!(recipe.handles.len(), 1);
        assert_eq!(recipe.bindings.bindings.len(), 1);
        assert_eq!(script.duration(), 2.0);
    }
}
