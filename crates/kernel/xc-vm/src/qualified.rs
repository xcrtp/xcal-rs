//! 完整函数名（内部链接器）：`"ns::path::name(A, B) -> R"`。
//!
//! 支持 namespace 与重载 ⇒ 不能只用短名：
//!
//! ```text
//! "xcal.math.sin(f32) -> f32"
//! "ns::path::overloaded(f32) -> f32"
//! "ns::path::overloaded(Vec2) -> Vec2"     ← 重载：靠签名区分
//! ```
//!
//! 短名与限定名**互不匹配**：`sin` 与 `xcal.math::sin` 是两个不同的名字。

use std::fmt;

use crate::error::VmError;

/// 完整函数名：namespace + 函数名 + 参数类型名 + 返回类型名。
///
/// 相等性 = 逐字段相等（namespace 按整段字符串保留，`ns::path` 不会被拆散）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedName {
    namespace: String,
    name: String,
    params: Vec<String>,
    ret: String,
}

impl QualifiedName {
    /// 逐字段构造。
    pub fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        params: Vec<String>,
        ret: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            params,
            ret: ret.into(),
        }
    }

    /// 从 `"ns::name"`（path）构造：最后一段是函数名，其余是 namespace。
    pub fn from_path(path: &str, params: Vec<String>, ret: impl Into<String>) -> Self {
        match path.rsplit_once("::") {
            Some((namespace, name)) => Self::new(namespace, name, params, ret),
            None => Self::new("", path, params, ret),
        }
    }

    /// 解析 `"ns::path::name(A, B) -> R"`。
    pub fn parse(input: &str) -> Result<Self, VmError> {
        let bad = || VmError::BadQualifiedName(input.to_string());

        let (lhs, ret) = input.split_once("->").ok_or_else(bad)?;
        let lhs = lhs.trim();
        let ret = ret.trim();
        if lhs.is_empty() || ret.is_empty() {
            return Err(bad());
        }

        let (path, params) = lhs.split_once('(').ok_or_else(bad)?;
        let params = params.strip_suffix(')').ok_or_else(bad)?;
        let path = path.trim();
        if path.is_empty() {
            return Err(bad());
        }

        let params = params
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        Ok(Self::from_path(path, params, ret))
    }

    /// namespace（可为空串）。
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// 函数名（不含 namespace）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 参数类型名（按声明顺序）。
    pub fn params(&self) -> &[String] {
        &self.params
    }

    /// 返回类型名。
    pub fn ret(&self) -> &str {
        &self.ret
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.namespace.is_empty() {
            write!(
                f,
                "{}({}) -> {}",
                self.name,
                self.params.join(", "),
                self.ret
            )
        } else {
            write!(
                f,
                "{}::{}({}) -> {}",
                self.namespace,
                self.name,
                self.params.join(", "),
                self.ret
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_round_trip() {
        let inputs = [
            "xcal.math::sin(f32) -> f32",
            "ns::path::overloaded(Vec2) -> Vec2",
            "sin(f32) -> f32",
            "no_args() -> bool",
        ];
        for input in inputs {
            let parsed = QualifiedName::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(parsed.to_string(), input, "round-trip 失败：{input}");
        }
    }

    #[test]
    fn overloads_are_distinct() {
        let a = QualifiedName::parse("ns::path::overloaded(f32) -> f32").unwrap();
        let b = QualifiedName::parse("ns::path::overloaded(Vec2) -> Vec2").unwrap();
        assert_ne!(a, b, "同名不同参数 ⇒ 不同的完整函数名");
    }

    #[test]
    fn short_name_does_not_match_qualified_name() {
        let short = QualifiedName::parse("sin(f32) -> f32").unwrap();
        let qualified = QualifiedName::parse("xcal.math::sin(f32) -> f32").unwrap();
        assert_ne!(short, qualified, "短名与限定名不得互配");
        assert_eq!(short.namespace(), "");
        assert_eq!(qualified.namespace(), "xcal.math");
        assert_eq!(qualified.name(), "sin");
    }

    #[test]
    fn multi_segment_namespace_is_preserved() {
        let qn = QualifiedName::parse("ns::path::name(i32, f32) -> bool").unwrap();
        assert_eq!(qn.namespace(), "ns::path");
        assert_eq!(qn.name(), "name");
        assert_eq!(qn.params(), &["i32".to_string(), "f32".to_string()]);
        assert_eq!(qn.ret(), "bool");
    }

    #[test]
    fn bad_inputs_are_rejected() {
        for input in ["", "sin", "sin(f32)", "-> f32", "sin(f32) -> "] {
            assert!(
                QualifiedName::parse(input).is_err(),
                "应拒绝非法输入：{input:?}"
            );
        }
    }
}
