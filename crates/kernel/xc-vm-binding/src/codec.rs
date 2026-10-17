//! 值与语言对象的双向转换 + 一次调用的通用编排。
//!
//! 适配器只实现 [`ValueCodec`]（Lua 的 userdata / Python 的 PyObject），
//! 参数个数校验、`VmValue` 转换、返回值回写都由 [`invoke`] 统一完成。

use std::sync::Arc;

use xc_vm::{ErasedFn, TypeSpec, Vm, VmValue};

/// 语言侧的值编解码器。
pub trait ValueCodec {
    /// 语言侧的值类型（如 Lua 的 userdata / Python 的 PyObject）。
    type Value: Clone;
    /// 语言侧的错误类型。
    type Error;

    /// 语言值 → `VmValue`（按 schema）。
    fn to_vm(
        &self,
        value: &Self::Value,
        spec: &'static TypeSpec,
    ) -> Result<VmValue, Self::Error>;

    /// `VmValue` → 语言值。
    fn from_vm(&self, value: &VmValue) -> Result<Self::Value, Self::Error>;

    /// 构造一个语言侧错误（参数个数 / 宿主错误）。
    fn error(&self, message: String) -> Self::Error;
}

/// 一次 capability 调用的通用编排：**所有语言共用**。
///
/// 1. 参数个数必须等于 schema；
/// 2. 逐参 `codec.to_vm`；
/// 3. 调 erased trampoline（一次间接寻址）；
/// 4. `codec.from_vm` 回写。
pub fn invoke<C: ValueCodec>(
    codec: &C,
    service: &Arc<dyn Vm>,
    item: ErasedFn,
    args: &[C::Value],
) -> Result<C::Value, C::Error> {
    let name = item.signature.name;
    let io = item.io;
    if args.len() != io.params.len() {
        return Err(codec.error(format!(
            "`{name}` 需要 {} 个参数，实际 {}",
            io.params.len(),
            args.len()
        )));
    }
    let mut values = Vec::with_capacity(args.len());
    for (arg, param) in args.iter().zip(io.params) {
        values.push(codec.to_vm(arg, param.ty)?);
    }
    let out = (item.call)(&**service, &values).map_err(|e| codec.error(e.to_string()))?;
    codec.from_vm(&out)
}
