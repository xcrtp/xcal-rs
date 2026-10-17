//! 不透明句柄：大类型只在 VM 内部存数据，跨边界只传"号"。
//!
//! 第一版语义（§2）：
//! - **显式 release**（隐式 GC 在跨进程场景不成立）；
//! - **跨 VM 传递 ⇒ 明确报错**（不是静默错误）；
//! - 句柄只作为号传递，数据留在 VM 内。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::VmError;

/// per-VM 身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VmId(u64);

impl VmId {
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

impl fmt::Display for VmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vm#{}", self.0)
    }
}

/// 不透明句柄 = `(owner, slot, generation)` 三个号。
///
/// `generation` 防 ABA：槽位 release 后复用会递增 generation，旧句柄随即失效。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    owner: VmId,
    slot: u64,
    generation: u32,
}

impl Handle {
    /// 直接构造（自定义 VM 的句柄表可自行生成）。
    pub const fn new(owner: VmId, slot: u64, generation: u32) -> Self {
        Self {
            owner,
            slot,
            generation,
        }
    }

    /// 所属 VM。
    pub const fn owner(&self) -> VmId {
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

    /// 校验句柄属于 `owner`；不属于 ⇒ [`VmError::CrossVmHandle`]。
    pub fn ensure_owner(&self, owner: VmId) -> Result<(), VmError> {
        if self.owner == owner {
            Ok(())
        } else {
            Err(VmError::CrossVmHandle {
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

/// 句柄槽位表：数据留在 VM 内，外部只拿到 [`Handle`]。
///
/// 一个表只属于一个 VM（构造时绑定 [`VmId`]）；跨表使用句柄会得到
/// [`VmError::CrossVmHandle`]。
pub struct HandleTable<T> {
    owner: VmId,
    slots: Vec<Slot<T>>,
    free: Vec<u64>,
}

impl<T> HandleTable<T> {
    /// 为 `owner` 新建空表。
    pub fn new(owner: VmId) -> Self {
        Self {
            owner,
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// 所属 VM id。
    pub fn owner(&self) -> VmId {
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

    /// 取只读引用。跨 VM / 已释放 ⇒ 明确错误。
    pub fn get(&self, handle: &Handle) -> Result<&T, VmError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(VmError::HandleReleased { slot: handle.slot })?;
        entry
            .value
            .as_ref()
            .ok_or(VmError::HandleReleased { slot: handle.slot })
    }

    /// 取可变引用。跨 VM / 已释放 ⇒ 明确错误。
    pub fn get_mut(&mut self, handle: &Handle) -> Result<&mut T, VmError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get_mut(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(VmError::HandleReleased { slot: handle.slot })?;
        entry
            .value
            .as_mut()
            .ok_or(VmError::HandleReleased { slot: handle.slot })
    }

    /// 显式释放。重复释放同样是明确错误。
    pub fn release(&mut self, handle: Handle) -> Result<(), VmError> {
        handle.ensure_owner(self.owner)?;
        let entry = self
            .slots
            .get_mut(handle.slot as usize)
            .filter(|e| e.generation == handle.generation)
            .ok_or(VmError::HandleReleased { slot: handle.slot })?;
        if entry.value.take().is_none() {
            return Err(VmError::HandleReleased { slot: handle.slot });
        }
        self.free.push(handle.slot);
        Ok(())
    }

    /// 句柄当前是否有效（跨 VM 也算无效）。
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
