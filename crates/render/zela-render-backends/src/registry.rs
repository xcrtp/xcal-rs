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

/// adapter 的两项事实（B11 Step 0）。
///
/// 抽出来是为了让"能力判定"可单测：`wgpu::Adapter` 不能在测试里伪造，
/// 但 `(features, R32Float 是否可混合)` 这组输入可以。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterFacts {
    pub features: wgpu::Features,
    /// `adapter.get_texture_format_features(R32Float).flags` 是否含 `BLENDABLE`。
    pub blends_float32: bool,
}

impl AdapterFacts {
    /// 从真实 adapter 取事实（`get_texture_format_features` 只有 `Adapter` 有）。
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        Self {
            features: adapter.features(),
            blends_float32: adapter
                .get_texture_format_features(wgpu::TextureFormat::R32Float)
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE),
        }
    }
}

/// 后端自述的 device features 需求（B11 Step 0）。
///
/// 分两类，**不得混用**：
/// - `required`：不满足 ⇒ 该后端不可用（[`BackendKind::is_available_on`] = false）；
/// - `preferred`：有则更好（例如更高的 MSAA），没有也能跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendFeatures {
    pub required: wgpu::Features,
    pub preferred: wgpu::Features,
}

impl BackendFeatures {
    /// 空需求（wgpu 后端显式自述的值，见 [`BackendKind::required_features`]）。
    pub const NONE: Self = Self {
        required: wgpu::Features::empty(),
        preferred: wgpu::Features::empty(),
    };
}

impl BackendKind {
    /// 本后端**自述**实现的 namespace。
    pub fn namespaces(self) -> &'static [&'static str] {
        match self {
            BackendKind::Wgpu => IMPLEMENTED_NAMESPACES,
            // 矢量的**填充族**映射已就绪（`zela-render-vector::scene`），但描边 /
            // 渐变 / 纹理尚未映射 ⇒ 仍不自述 `zela.render2d`（见 GAPS V2-06）。
            BackendKind::Vector => &[],
        }
    }

    /// 本后端的能力集合。
    ///
    /// B10 Step 1：除 namespace 外还要自述**接受的网格形态**。两个后端都只吃
    /// 三角形面（wgpu：`standard_3d()` 交错顶点 + 索引；矢量：`Upload::mesh` 的
    /// `triangles`）；`MeshForm::Line` 目前无人接受 ⇒ 请求它必须显式失败。
    pub fn capabilities(self) -> RenderCapabilitySet {
        RenderCapabilitySet::from_names(self.namespaces())
            .with_mesh(ze_render_ir::caps::MeshForms::tri())
    }

    /// 本后端的**后处理能力**（B6 Step 1.3）：着色栈是矢量路径的固有部分。
    ///
    /// 与 [`Self::capabilities`] 分开：前者是 4 个 namespace 指纹（`zela.render*`），
    /// 后者是 `Capabilities::post` 的四维开关。`RenderParams::default_for(&caps)`
    /// 按它决定"默认开着色栈还是全关"（B6 §0.3）。
    pub fn post_caps(self) -> ze_render_ir::PostCaps {
        match self {
            // wgpu 三角化路径没有后处理 pass（B6 §0.4：本批不给它新写）。
            BackendKind::Wgpu => ze_render_ir::PostCaps::none(),
            // 矢量路径：tone / bloom / ao / env 都由上游着色栈实现。
            BackendKind::Vector => ze_render_ir::PostCaps::all(),
        }
    }

    /// 本后端需要 device 具备哪些 features（B11 Step 0）。
    ///
    /// 口径与 [`Self::capabilities`] / [`Self::post_caps`] 相同：**后端自述**，宿主不替它猜。
    /// - wgpu 显式返回 [`BackendFeatures::NONE`]（不是"没实现"——C4 禁止静默默认）；
    /// - 矢量对应 `zela-render-vector/src/render.rs` 的两条硬前置：R32Float 可混合
    ///   （`FLOAT32_BLENDABLE`，覆盖率在 float32 目标里累加）与
    ///   `DEPTH32FLOAT_STENCIL8`（reversed-Z）；`TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`
    ///   只影响 MSAA 档位，属 preferred。
    pub fn required_features(self, adapter: &wgpu::Adapter) -> BackendFeatures {
        self.required_features_for(AdapterFacts::of(adapter))
    }

    /// 同 [`Self::required_features`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn required_features_for(self, facts: AdapterFacts) -> BackendFeatures {
        match self {
            BackendKind::Wgpu => BackendFeatures::NONE,
            BackendKind::Vector => BackendFeatures {
                // 只请求 adapter 自述的 feature：请求它没有的会让 request_device 直接失败。
                required: wgpu::Features::DEPTH32FLOAT_STENCIL8
                    | (facts.features & wgpu::Features::FLOAT32_BLENDABLE),
                preferred: facts.features & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            },
        }
    }

    /// 本后端是否要求 device 按 **adapter 上限**申请（B11 Step 1）。
    ///
    /// 矢量后端的存储缓冲 / 纹理规模随场景走，按默认 limits 申请会不够；
    /// wgpu 三角化沿用 wgpu 默认 limits（与改动前一致）。
    pub fn requires_adapter_limits(self) -> bool {
        match self {
            BackendKind::Wgpu => false,
            BackendKind::Vector => true,
        }
    }

    /// 本后端在这块 adapter 上是否可用（B11 Step 0/2）。
    ///
    /// 判定只看**能力**，不看枚举值：`BackendKind` 从此只是标识。
    pub fn is_available_on(self, adapter: &wgpu::Adapter) -> bool {
        self.is_available_on_facts(AdapterFacts::of(adapter))
    }

    /// 同 [`Self::is_available_on`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn is_available_on_facts(self, facts: AdapterFacts) -> bool {
        match self {
            // wgpu 三角化只依赖 device 本身；它的 features 需求是显式空集。
            BackendKind::Wgpu => true,
            BackendKind::Vector => {
                // B15：浏览器上也能出帧（异步回读 + GPU 内目标）⇒ 只按**能力**判定，
                // 不再按目标平台拒绝（X2-03 关闭）。缺 required ⇒ 由
                // `unavailable_reason_for` 给出具体理由。
                facts.blends_float32
                    && facts
                        .features
                        .contains(wgpu::Features::DEPTH32FLOAT_STENCIL8)
            }
        }
    }

    /// **被拒理由**（可用 ⇒ `None`）。C4：拒绝必须带理由，不得只说"不可用"。
    pub fn unavailable_reason(self, adapter: &wgpu::Adapter) -> Option<String> {
        self.unavailable_reason_for(AdapterFacts::of(adapter))
    }

    /// 同 [`Self::unavailable_reason`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn unavailable_reason_for(self, facts: AdapterFacts) -> Option<String> {
        if self.is_available_on_facts(facts) {
            return None;
        }
        match self {
            BackendKind::Wgpu => None, // 恒可用，不会走到这里
            BackendKind::Vector => {
                let mut why: Vec<&str> = Vec::new();
                if !facts.blends_float32 {
                    why.push("adapter 不能混合 R32Float（覆盖率在 float32 目标里累加）");
                }
                if !facts
                    .features
                    .contains(wgpu::Features::DEPTH32FLOAT_STENCIL8)
                {
                    why.push("adapter 缺 DEPTH32FLOAT_STENCIL8（reversed-Z 需要浮点深度带模板）");
                }
                Some(why.join("；"))
            }
        }
    }

}

/// 选择 / 注册失败（显式，不静默）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendRegistryError {
    #[error("后端 {kind:?} 已注册")]
    AlreadyRegistered { kind: BackendKind },
    #[error("后端 {kind:?} 在这块 adapter 上不可用：{reason}")]
    NotAvailable { kind: BackendKind, reason: String },
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

/// 一个后端在给定 adapter 上的可用性（B11 Step 1/4 的可观测面）。
///
/// `available = false` 时 `reason` **必须非空**（C4：拒绝不得无理由）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendAvailability {
    pub kind: BackendKind,
    pub available: bool,
    pub reason: Option<String>,
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

    /// 注册后端（重复 ⇒ 显式错误）。
    ///
    /// 注册 = "装配期想用它"，**不**判断 adapter 能力：能力判定在运行期由
    /// [`Self::select_on`] / [`Self::availability`] 完成（B11 Step 2：可用性只由能力决定）。
    pub fn register(&mut self, kind: BackendKind) -> Result<(), BackendRegistryError> {
        if self.kinds.contains(&kind) {
            return Err(BackendRegistryError::AlreadyRegistered { kind });
        }
        self.kinds.push(kind);
        Ok(())
    }

    pub fn kinds(&self) -> &[BackendKind] {
        &self.kinds
    }

    /// 逐个后端汇报"在这块 adapter 上能不能用、为什么"（B11 Step 1）。
    pub fn availability(&self, adapter: &wgpu::Adapter) -> Vec<BackendAvailability> {
        self.availability_for(AdapterFacts::of(adapter))
    }

    /// 同 [`Self::availability`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn availability_for(&self, facts: AdapterFacts) -> Vec<BackendAvailability> {
        self.kinds
            .iter()
            .map(|kind| match kind.unavailable_reason_for(facts) {
                None => BackendAvailability {
                    kind: *kind,
                    available: true,
                    reason: None,
                },
                Some(reason) => BackendAvailability {
                    kind: *kind,
                    available: false,
                    reason: Some(reason),
                },
            })
            .collect()
    }

    /// **device features = 已注册且可用后端的 required 之并**（B11 Step 1 / C2）。
    ///
    /// - 只对 `is_available_on` 通过的后端取并集：不可用的后端**不请求**它的 features
    ///   （否则会造出"以前能起 device 的机器现在起不来"的回归）；
    /// - 一个可用后端都没有 ⇒ 显式错误，并列出每个后端的拒绝理由。
    pub fn required_device_features(
        &self,
        adapter: &wgpu::Adapter,
    ) -> Result<wgpu::Features, String> {
        self.required_device_features_for(AdapterFacts::of(adapter))
    }

    /// 同 [`Self::required_device_features`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn required_device_features_for(
        &self,
        facts: AdapterFacts,
    ) -> Result<wgpu::Features, String> {
        let mut union = wgpu::Features::empty();
        let mut available = 0usize;
        let mut rejected: Vec<String> = Vec::new();
        for entry in self.availability_for(facts) {
            match entry.reason {
                None => {
                    available += 1;
                    union |= entry.kind.required_features_for(facts).required;
                }
                Some(reason) => rejected.push(format!("{:?}：{reason}", entry.kind)),
            }
        }
        if available == 0 {
            return Err(format!(
                "没有可用后端（已注册：{:?}）。拒绝理由：{}",
                self.kinds,
                rejected.join("；")
            ));
        }
        Ok(union)
    }

    /// device limits：有**可用**后端要求 adapter 上限时取 `adapter.limits()`，
    /// 否则用 wgpu 默认（仅 wgpu 注册时与改动前一致）。
    pub fn required_device_limits(&self, adapter: &wgpu::Adapter) -> wgpu::Limits {
        let facts = AdapterFacts::of(adapter);
        if self
            .availability_for(facts)
            .iter()
            .any(|entry| entry.available && entry.kind.requires_adapter_limits())
        {
            adapter.limits()
        } else {
            wgpu::Limits::default()
        }
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

    /// 选定一个后端做实际渲染（未注册 / 该 adapter 上不可用 ⇒ 显式错误）。
    ///
    /// B11 Step 2：可用性由 `is_available_on(adapter)` 决定，**不再**看 `BackendKind` 枚举值。
    pub fn select_on(
        &self,
        kind: BackendKind,
        adapter: &wgpu::Adapter,
    ) -> Result<(), BackendRegistryError> {
        self.select_on_facts(kind, AdapterFacts::of(adapter))
    }

    /// 同 [`Self::select_on`]，输入换成可单测的 [`AdapterFacts`]。
    pub fn select_on_facts(
        &self,
        kind: BackendKind,
        facts: AdapterFacts,
    ) -> Result<(), BackendRegistryError> {
        if !self.kinds.contains(&kind) {
            return Err(BackendRegistryError::NotRegistered {
                kind,
                registered: self.kinds.clone(),
            });
        }
        match kind.unavailable_reason_for(facts) {
            None => Ok(()),
            Some(reason) => Err(BackendRegistryError::NotAvailable { kind, reason }),
        }
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

    /// 一块自述"能混合 float32、有 float32 深度带模板"的 adapter。
    fn capable_facts() -> AdapterFacts {
        AdapterFacts {
            features: wgpu::Features::FLOAT32_BLENDABLE
                | wgpu::Features::DEPTH32FLOAT_STENCIL8
                | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            blends_float32: true,
        }
    }

    /// B11 Step 0：wgpu **显式**自述空需求（不是"没实现" ⇒ C4）。
    #[test]
    fn wgpu_declares_an_explicitly_empty_feature_set() {
        for facts in [
            AdapterFacts {
                features: wgpu::Features::empty(),
                blends_float32: false,
            },
            capable_facts(),
        ] {
            let features = BackendKind::Wgpu.required_features_for(facts);
            assert!(
                features.required.is_empty(),
                "wgpu 的 required 必须是空集，实测 {:?}",
                features.required
            );
            assert!(
                features.preferred.is_empty(),
                "wgpu 的 preferred 必须是空集，实测 {:?}",
                features.preferred
            );
        }
    }

    /// B11 Step 0：矢量缺 `FLOAT32_BLENDABLE`（R32Float 不可混合）⇒ 不可用，且**带理由**。
    #[test]
    fn vector_is_unavailable_without_float32_blending() {
        let facts = AdapterFacts {
            features: wgpu::Features::DEPTH32FLOAT_STENCIL8,
            blends_float32: false,
        };
        assert!(!BackendKind::Vector.is_available_on_facts(facts));
        let reason = BackendKind::Vector
            .unavailable_reason_for(facts)
            .expect("不可用必须给理由");
        assert!(reason.contains("R32Float"), "理由应点明 R32Float：{reason}");
    }

    /// B11 Step 0：矢量缺 `DEPTH32FLOAT_STENCIL8` ⇒ 不可用，理由点明该 feature。
    #[test]
    fn vector_is_unavailable_without_float32_depth_stencil() {
        let facts = AdapterFacts {
            features: wgpu::Features::FLOAT32_BLENDABLE,
            blends_float32: true,
        };
        assert!(!BackendKind::Vector.is_available_on_facts(facts));
        let reason = BackendKind::Vector
            .unavailable_reason_for(facts)
            .expect("不可用必须给理由");
        assert!(
            reason.contains("DEPTH32FLOAT_STENCIL8"),
            "理由应点明 DEPTH32FLOAT_STENCIL8：{reason}"
        );
    }

    /// B11 Step 0：矢量的 required 永远不会超出 adapter 自述（请求它没有的会让
    /// `request_device` 直接失败）；preferred 同理。
    #[test]
    fn vector_required_features_never_exceed_the_adapter() {
        let facts = capable_facts();
        let features = BackendKind::Vector.required_features_for(facts);
        assert!(facts.features.contains(features.required));
        assert!(facts.features.contains(features.preferred));
        assert!(features.required.contains(wgpu::Features::DEPTH32FLOAT_STENCIL8));
        // 缺 float32 混合能力的 adapter：required 里不得出现它（否则 request_device 直接失败）。
        let weak = AdapterFacts {
            features: wgpu::Features::DEPTH32FLOAT_STENCIL8,
            blends_float32: false,
        };
        let weak_features = BackendKind::Vector.required_features_for(weak);
        assert!(!weak_features
            .required
            .contains(wgpu::Features::FLOAT32_BLENDABLE));
        assert!(weak.features.contains(weak_features.required));
    }

    /// B11 Step 1（C2 的守护）：**仅 wgpu 注册 ⇒ 并集为空集**，与改动前逐位相同。
    #[test]
    fn union_is_empty_when_only_wgpu_is_registered() {
        let registry = BackendRegistry::with_standard();
        let features = registry
            .required_device_features_for(capable_facts())
            .expect("wgpu 恒可用");
        assert_eq!(features, wgpu::Features::empty());
    }

    /// B11 Step 1：并集只覆盖**可用**后端 —— 矢量被拒时不得把它的 features 带进来。
    #[test]
    fn union_only_covers_available_backends() {
        let mut registry = BackendRegistry::with_standard();
        registry.register(BackendKind::Vector).unwrap();

        let weak = AdapterFacts {
            features: wgpu::Features::DEPTH32FLOAT_STENCIL8,
            blends_float32: false,
        };
        assert_eq!(
            registry.required_device_features_for(weak).unwrap(),
            wgpu::Features::empty(),
            "矢量不可用时不得请求它的 features"
        );

        let strong = registry.required_device_features_for(capable_facts()).unwrap();
        assert!(strong.contains(wgpu::Features::DEPTH32FLOAT_STENCIL8));
    }

    /// B11 Step 1：一个可用后端都没有 ⇒ 显式报错，并列出每个后端的拒绝理由。
    #[test]
    fn no_available_backend_is_an_explicit_error_with_reasons() {
        let mut registry = BackendRegistry::empty();
        registry.register(BackendKind::Vector).unwrap();
        let weak = AdapterFacts {
            features: wgpu::Features::empty(),
            blends_float32: false,
        };
        let error = registry.required_device_features_for(weak).unwrap_err();
        assert!(error.contains("R32Float"), "错误应带拒绝理由：{error}");
        assert!(
            error.contains("DEPTH32FLOAT_STENCIL8"),
            "错误应列出全部理由：{error}"
        );
    }

    /// 注册 = 装配期意图（不再被构建级开关拦）；可用性在运行期按 adapter 判定。
    #[test]
    fn registration_is_assembly_time_and_selection_is_capability_based() {
        let mut registry = BackendRegistry::empty();
        assert!(matches!(
            registry
                .select_on_facts(BackendKind::Vector, capable_facts())
                .unwrap_err(),
            BackendRegistryError::NotRegistered { .. }
        ));

        registry.register(BackendKind::Vector).unwrap();
        assert!(matches!(
            registry.register(BackendKind::Vector).unwrap_err(),
            BackendRegistryError::AlreadyRegistered { .. }
        ));
        assert!(registry
            .select_on_facts(BackendKind::Vector, capable_facts())
            .is_ok());

        registry.register(BackendKind::Wgpu).unwrap();
        assert!(registry
            .select_on_facts(BackendKind::Wgpu, capable_facts())
            .is_ok());

        let mut with_vector = BackendRegistry::with_standard();
        with_vector.register(BackendKind::Vector).unwrap();
        assert!(with_vector
            .select_on_facts(BackendKind::Vector, capable_facts())
            .is_ok());
        let weak = AdapterFacts {
            features: wgpu::Features::empty(),
            blends_float32: false,
        };
        assert!(matches!(
            with_vector
                .select_on_facts(BackendKind::Vector, weak)
                .unwrap_err(),
            BackendRegistryError::NotAvailable { .. }
        ));
    }

    /// B6 Step 1.3：后处理能力各自自述 —— wgpu 全关、矢量全开。
    /// `RenderParams::default_for(&caps)` 靠它决定默认值（B6 §0.3）。
    #[test]
    fn post_caps_are_self_described_per_backend() {
        let none = BackendKind::Wgpu.post_caps();
        assert!(!none.tone && !none.bloom && !none.ao && !none.env);

        let all = BackendKind::Vector.post_caps();
        assert!(all.tone && all.bloom && all.ao && all.env);

        // 默认参数按能力取：wgpu 保持全关，矢量默认带着色栈。
        let caps = |post| ze_render_ir::Capabilities {
            max_sample_count: 4,
            color_format: ze_render_ir::ColorFormat::Bgra8UnormSrgb,
            depth_format: ze_render_ir::DepthFormat::Depth32Float,
            post,
        };
        let wgpu = ze_render_ir::RenderParams::default_for(&caps(BackendKind::Wgpu.post_caps()));
        assert!(wgpu.tone.is_none() && wgpu.bloom.is_none() && wgpu.ao.is_none());
        let vector = ze_render_ir::RenderParams::default_for(&caps(BackendKind::Vector.post_caps()));
        assert!(vector.tone.is_some() && vector.bloom.is_some() && vector.ao.is_some());
    }
}
