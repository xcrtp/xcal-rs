// registry.rs — 后端注册与选择（B4 Step 4）。
//
// # 为什么需要
// B3-0.5 只做了 capability **声明**（`IMPLEMENTED_NAMESPACES`），没有"换后端"的装配点：
// `StandardBackends` 直接持有 `WgpuBackend2D` / `WgpuBackend3D` 两个具体类型，
// 系统能力也只能用某个后端的常量冒充（`RenderCapabilitySet::ALL`，已删除）。
//
// 本模块补上两件事：
// 1. **每个后端自述**自己的 namespace（[`BackendKind::capabilities`]）；
// 2. **系统能力 = 已注册后端之并**（[`BackendRegistry::system_capabilities`]）。
//
// # 不静默
// - 注册一个"尚未实现"的后端（如矢量后端移植前）⇒ 显式错误；
// - 选择未注册的后端 ⇒ 显式错误；
// - 协商缺能力 ⇒ 复用 `ze_render_ir::caps::negotiate` 的全有或全无语义。

use ze_render_ir::caps::{self, CapabilityNegotiationError, RenderCapabilitySet};

use crate::render_backend::IMPLEMENTED_NAMESPACES;

/// 可注册的后端种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// wgpu 三角化 + MSAA 后端（既有实现）。
    Wgpu,
    /// 矢量后端（解析 AA；B4 Step 5 移植后实现 `zela.render.shapes`）。
    Vector,
}

impl BackendKind {
    /// 本后端**自述**实现的 namespace。
    pub fn namespaces(self) -> &'static [&'static str] {
        match self {
            BackendKind::Wgpu => IMPLEMENTED_NAMESPACES,
            // 移植完成后改为 `&[caps::shapes::NAMESPACE]`（Step 5 接线）。
            BackendKind::Vector => &[],
        }
    }

    /// 本后端的能力集合。
    pub fn capabilities(self) -> RenderCapabilitySet {
        RenderCapabilitySet::from_names(self.namespaces())
    }

    /// 本构建里是否真的可实例化（`Vector` 在移植前为 `false`）。
    pub fn is_available(self) -> bool {
        matches!(self, BackendKind::Wgpu)
    }
}

/// 选择 / 注册失败（显式，不静默）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendRegistryError {
    #[error("后端 {kind:?} 已注册")]
    AlreadyRegistered { kind: BackendKind },
    #[error("后端 {kind:?} 尚未实现（不可注册 / 不可选择）")]
    NotAvailable { kind: BackendKind },
    #[error("后端 {kind:?} 未注册（当前已注册：{registered:?}）")]
    NotRegistered {
        kind: BackendKind,
        registered: Vec<BackendKind>,
    },
    #[error(transparent)]
    Capability(#[from] CapabilityNegotiationError),
}

/// 已注册后端集合。
#[derive(Debug, Clone, Default)]
pub struct BackendRegistry {
    kinds: Vec<BackendKind>,
}

impl BackendRegistry {
    /// 空注册表（测试 / 自定义装配）。
    pub fn empty() -> Self {
        Self { kinds: Vec::new() }
    }

    /// 本仓库的标准装配：wgpu 后端。
    pub fn with_standard() -> Self {
        Self {
            kinds: vec![BackendKind::Wgpu],
        }
    }

    /// 注册后端（重复 ⇒ 显式错误；未实现 ⇒ 显式错误）。
    pub fn register(&mut self, kind: BackendKind) -> Result<(), BackendRegistryError> {
        if !kind.is_available() {
            return Err(BackendRegistryError::NotAvailable { kind });
        }
        if self.kinds.contains(&kind) {
            return Err(BackendRegistryError::AlreadyRegistered { kind });
        }
        self.kinds.push(kind);
        Ok(())
    }

    pub fn kinds(&self) -> &[BackendKind] {
        &self.kinds
    }

    /// **系统能力**：已注册后端能力之并（不是任何单个后端的常量）。
    pub fn system_capabilities(&self) -> RenderCapabilitySet {
        let sets: Vec<RenderCapabilitySet> = self.kinds.iter().map(|k| k.capabilities()).collect();
        RenderCapabilitySet::union(&sets)
    }

    /// 按所需 namespace 协商（全有或全无）。
    pub fn negotiate(
        &self,
        required: &[&str],
    ) -> Result<RenderCapabilitySet, BackendRegistryError> {
        let system = self.system_capabilities();
        // 复用 IR 的协商：把"系统能力"展开成 namespace 列表再判定。
        let mut available: Vec<&str> = Vec::new();
        for name in caps::NAMESPACES {
            let present = match name {
                caps::render2d::NAMESPACE => system.render2d,
                caps::render3d::NAMESPACE => system.render3d,
                caps::mixed::NAMESPACE => system.mixed,
                caps::shapes::NAMESPACE => system.shapes,
                _ => false,
            };
            if present {
                available.push(name);
            }
        }
        Ok(caps::negotiate(&available, required)?)
    }

    /// 选定一个后端做实际渲染（未注册 / 未实现 ⇒ 显式错误）。
    pub fn select(&self, kind: BackendKind) -> Result<(), BackendRegistryError> {
        if !kind.is_available() {
            return Err(BackendRegistryError::NotAvailable { kind });
        }
        if !self.kinds.contains(&kind) {
            return Err(BackendRegistryError::NotRegistered {
                kind,
                registered: self.kinds.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系统能力是并集：只注册 wgpu 时没有 `shapes`，协商 shapes ⇒ 显式失败。
    #[test]
    fn system_capabilities_come_from_registered_backends() {
        let registry = BackendRegistry::with_standard();
        let system = registry.system_capabilities();
        assert!(system.render2d && system.render3d && system.mixed);
        assert!(!system.shapes, "wgpu 后端尚未实现 shapes");

        let err = registry.negotiate(&[caps::shapes::NAMESPACE]).unwrap_err();
        assert!(matches!(err, BackendRegistryError::Capability(_)));
    }

    /// 未实现的后端不可注册（Step 5 移植前 `Vector` 就是这种状态）。
    #[test]
    fn unavailable_backend_cannot_be_registered() {
        let mut registry = BackendRegistry::empty();
        let err = registry.register(BackendKind::Vector).unwrap_err();
        assert!(matches!(
            err,
            BackendRegistryError::NotAvailable {
                kind: BackendKind::Vector
            }
        ));
        assert!(registry.kinds().is_empty());
    }

    /// 重复注册 / 未注册选择 ⇒ 显式错误。
    #[test]
    fn duplicate_and_unregistered_are_explicit() {
        let mut registry = BackendRegistry::with_standard();
        assert!(matches!(
            registry.register(BackendKind::Wgpu).unwrap_err(),
            BackendRegistryError::AlreadyRegistered { .. }
        ));
        let empty = BackendRegistry::empty();
        assert!(matches!(
            empty.select(BackendKind::Wgpu).unwrap_err(),
            BackendRegistryError::NotRegistered { .. }
        ));
        assert!(registry.select(BackendKind::Wgpu).is_ok());
    }
}
