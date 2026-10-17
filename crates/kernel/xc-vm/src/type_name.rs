//! 稳定类型名（与 `xc-node::TypeTag::type_name()` 形态同构）。
//!
//! `xc-node` 的 `TypeTag::type_name() -> &'static str` 是"稳定人类可读名"的既有先例，
//! `NamedTypeTag` 是它的字符串实现。本模块把同一形态内建到 kernel 层：
//!
//! - [`VmTypeName`] = `&'static str`（稳定类型名）；
//! - [`IntoVmTypeName`] = 编译期映射（关联常量 `NAME`）。
//!
//! **为什么不能直接复用 `xc-node::TypeTag`**：`xc-vm` 是零依赖 kernel crate，
//! 不得依赖任何 `xcal-*` / `xc-*`（见 Cargo.toml 依赖铁律）。故只照其**形态**内建。

use crate::handle::Handle;

/// 稳定类型名。
///
/// 只描述"逻辑类型"，不含泛型参数或生命周期；同一类型在所有 VM 间必须给出同一字符串。
pub type VmTypeName = &'static str;

/// `Into` 风格的编译期类型名映射。
///
/// # 推断规则（由 `#[capability]` 宏实现）
///
/// - 类型 `T` 实现了本 trait ⇒ 宏以 `<T as IntoVmTypeName>::NAME` **自动推断**；
/// - 未实现 ⇒ 必须在字段上用 `#[vm_type(...)]` **显式标注**；两者都没有 ⇒ **编译错误**
///   （生成代码里 `<T as IntoVmTypeName>::NAME` 无法解析）。
///
/// 不做隐式推导：内建映射只覆盖 std 小类型；`Vec2` / `Path2D` 等域类型
/// 因 kernel 不能依赖域 crate，一律走显式标注（或由类型所属 crate 实现本 trait）。
pub trait IntoVmTypeName {
    /// 稳定类型名（编译期常量）。
    const NAME: VmTypeName;

    /// 以值形式取类型名（便于非宏场景使用）。
    #[inline]
    fn into_vm_type_name(&self) -> VmTypeName {
        Self::NAME
    }
}

macro_rules! impl_vm_type_name {
    ($($ty:ty => $name:literal),* $(,)?) => {
        $(
            impl IntoVmTypeName for $ty {
                const NAME: VmTypeName = $name;
            }
        )*
    };
}

// 内建小类型映射（§1.2）。刻意保持最小：其余类型必须显式标注 ——
// 隐式推导出错时是"静默的类型名错配"，最难查。
impl_vm_type_name! {
    () => "()",
    bool => "bool",
    char => "char",
    f32 => "f32",
    f64 => "f64",
    i8 => "i8",
    i16 => "i16",
    i32 => "i32",
    i64 => "i64",
    i128 => "i128",
    isize => "isize",
    u8 => "u8",
    u16 => "u16",
    u32 => "u32",
    u64 => "u64",
    u128 => "u128",
    usize => "usize",
    String => "String",
}

// 本 crate 自己的大类型句柄：方法语义的 receiver 参数就是它。
impl IntoVmTypeName for Handle {
    const NAME: VmTypeName = "xc_vm.Handle";
}

/// `Vec<Handle>` 的协商名与 **schema 形态**一致（`Array<Opaque>`）。
///
/// 只为这一具体类型实现（`Vec<T>` 的通用名需要 const 字符串拼接，stable 不可用）。
/// 有了它，capability 字段不再需要 `#[vm_type(param0 = "ComponentList")]` 这类
/// 与 schema 脱节的字符串 override。
impl IntoVmTypeName for Vec<Handle> {
    const NAME: VmTypeName = "Array<xc_vm.Handle>";
}
