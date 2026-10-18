//! 不透明句柄：大类型只在 Pod 内部存数据，跨边界只传"号"。
//!
//! 第一版语义（§2）：
//! - **显式 release**（隐式 GC 在跨进程场景不成立）；
//! - **跨 Pod 传递 ⇒ 明确报错**（不是静默错误）；
//! - 句柄只作为号传递，数据留在 Pod 内。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::PodError;

/// per-Pod 身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PodId(u64);

impl PodId {
    /// 分配一个进程内唯一的新 id（从 1 开始）。
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// 直接构造（测试 / 反序列化等场景）。
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// 原始值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pod#{}", self.0)
    }
}

/// **寻址用的 Pod 身份句柄**（不是资源 [`Handle`]）。
///
/// 与 [`Handle`] 的区别：
///
/// | | [`PodHandle`] | [`Handle`] |
/// |---|---|---|
/// | 指向 | **Pod 本身**（扇出 / 路由 / 排序键的 `source`） | Pod 内部的一个资源槽位 |
/// | owner | 无（它自己就是 owner） | 带 owner，跨 Pod 使用 ⇒ `CrossPodHandle` |
/// | 用途 | topic 成员、`SeqKey::source` | 大对象跨边界传递 |
///
/// **禁止**用资源 `Handle` 做寻址：它带 owner，跨 Pod 使用会触发
/// [`PodError::CrossPodHandle`]。
///
/// 实现 `PodType`（schema 为 `u64`）⇒ 可跨语言/跨边界传递，也可作为
/// 泛型 capability 的参数类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PodHandle(PodId);

impl PodHandle {
    /// 从 Pod 身份构造。
    pub const fn new(id: PodId) -> Self {
        Self(id)
    }

    /// 原始 Pod 身份。
    pub const fn id(self) -> PodId {
        self.0
    }

    /// 原始数值（排序 / 日志）。
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for PodHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<PodId> for PodHandle {
    fn from(id: PodId) -> Self {
        Self(id)
    }
}

impl From<PodHandle> for PodId {
    fn from(handle: PodHandle) -> Self {
        handle.0
    }
}

impl crate::value::PodType for PodHandle {
    const SPEC: &'static crate::typespec::TypeSpec =
        &crate::typespec::TypeSpec::Scalar(crate::typespec::ScalarKind::U64);

    fn to_pod_value(&self) -> crate::value::PodValue {
        crate::value::PodValue::Scalar(crate::value::ScalarValue::U64(self.get()))
    }

    fn from_pod_value(
        value: &crate::value::PodValue,
    ) -> Result<Self, crate::value::PodValueError> {
        match value {
            crate::value::PodValue::Scalar(crate::value::ScalarValue::U64(raw)) => {
                Ok(Self(PodId::new(*raw)))
            }
            other => Err(crate::value::PodValueError::new(format!(
                "期望 u64（xc_pod.PodHandle），实际是 {}",
                crate::value::describe(other)
            ))),
        }
    }
}

impl crate::type_name::IntoPodTypeName for PodHandle {
    const NAME: crate::type_name::PodTypeName = "xc_pod.PodHandle";
}

/// 不透明句柄 = `(owner, slot, generation)` 三个号。
///
/// `generation` 防 ABA：槽位 release 后复用会递增 generation，旧句柄随即失效。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    owner: PodId,
    slot: u64,
    generation: u32,
}

impl Handle {
    /// 直接构造（自定义 Pod 的句柄表可自行生成）。
    pub const fn new(owner: PodId, slot: u64, generation: u32) -> Self {
        Self {
            owner,
            slot,
            generation,
        }
    }

    /// 所属 Pod。
    pub const fn owner(&self) -> PodId {
        self.owner
    }

    /// 槽位号。
    pub const fn slot(&self) -> u64 {
        self.slot
    }

    /// 槽位代数。
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    /// 校验句柄属于 `owner`；不属于 ⇒ [`PodError::CrossPodHandle`]。
    pub fn ensure_owner(&self, owner: PodId) -> Result<(), PodError> {
        if self.owner == owner {
            Ok(())
        } else {
            Err(PodError::CrossPodHandle {
                expected: owner.get(),
                actual: self.owner.get(),
            })
        }
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "handle({} slot {}, gen {})",
            self.owner, self.slot, self.generation
        )
    }
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// 句柄槽位表：数据留在 Pod 内，外部只拿到 [`Handle`]。
///
/// 一个表只属于一个 Pod（构造时绑定 [`PodId`]）；跨表使用句柄会得到
/// [`PodError::CrossPodHandle`]。
pub struct HandleTable<T> {
    owner: PodId,
    slots: Vec<Slot<T>>,
    free: Vec<u64>,
}

impl<T> HandleTable<T> {
    /// 为 `owner` 新建空表。
    pub fn new(owner: PodId) -> Self {
        Self {
            owner,
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// 所属 Pod id。
    pub fn owner(&self) -> PodId {
        self.owner
    }

    /// 存入一个值，返回不透明句柄。
    pub fn insert(&mut self, value: T) -> Handle {
        if let Some(slot) = self.free.pop() {
            let entry = &mut self.slots[slot as usize];
            entry.generation = entry.generation.wrapping_add(1);
            entry.value = Some(value);
            Handle::new(self.owner, slot, entry.generation)
        } else {
            let slot = self.slots.len() as u64;
            self.slots.push(Slot {
                generation: 0,
                value: Some(value),
            });
            Handle::new(self.owner, slot, 0)
        }
    }

    /// 取只读引用。跨 Pod / 已释放 ⇒ 明确错误。
    pub fn get(&self, handle: &Handle) -> Result<&T, PodError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(PodError::HandleReleased { slot: handle.slot })?;
        entry
            .value
            .as_ref()
            .ok_or(PodError::HandleReleased { slot: handle.slot })
    }

    /// 取可变引用。跨 Pod / 已释放 ⇒ 明确错误。
    pub fn get_mut(&mut self, handle: &Handle) -> Result<&mut T, PodError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get_mut(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(PodError::HandleReleased { slot: handle.slot })?;
        entry
            .value
            .as_mut()
            .ok_or(PodError::HandleReleased { slot: handle.slot })
    }

    /// 显式释放。重复释放同样是明确错误。
    pub fn release(&mut self, handle: Handle) -> Result<(), PodError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get_mut(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(PodError::HandleReleased { slot: handle.slot })?;
        if entry.value.take().is_none() {
            return Err(PodError::HandleReleased { slot: handle.slot });
        }
        self.free.push(handle.slot);
        Ok(())
    }

    /// 句柄当前是否有效（跨 Pod 也算无效）。
    pub fn is_live(&self, handle: &Handle) -> bool {
        self.get(handle).is_ok()
    }

    /// 存活值数量。
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
