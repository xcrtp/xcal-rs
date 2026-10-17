//! 第一 VM：闭包注册表（内置小 VM）。
//!
//! 注册 `(完整函数名, 可调用)`，同时暴露：
//! - **typed**：裸 fn 指针 ⇒ 协商出快路径句柄（每次调用一次间接寻址）；
//! - **erased**：`Fn(&dyn Vm, &[&dyn Any])` ⇒ 只有慢路径（纯解释型 VM 的形态）。
//!
//! typed 注册会**自动**同时生成 erased 版本 —— 两条路径调用的是同一个函数，
//! 故结果必然一致（这是"快慢一致"最有力的实现方式）。

use std::any::Any;

use crate::error::VmError;
use crate::handle::VmId;
use crate::qualified::QualifiedName;
use crate::type_name::{IntoVmTypeName, VmTypeName};
use crate::vm::{FunctionId, Vm};

/// 类型擦除的可调用对象（慢路径）。
///
/// 对裸 fn 指针有 blanket 实现（arity 0..=3）。自定义 VM 可直接持有
/// `Box<dyn ErasedCall>` 来复用慢路径。
pub trait ErasedCall: Send + Sync {
    /// 以类型擦除实参调用。
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError>;
}

fn expect_arity(args: &[&dyn Any], expected: usize) -> Result<(), VmError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(VmError::ArityMismatch {
            expected,
            got: args.len(),
        })
    }
}

fn arg<'b, T: 'static>(args: &[&'b dyn Any], index: usize) -> Result<&'b T, VmError> {
    let value: &'b dyn Any = *args.get(index).ok_or(VmError::ArityMismatch {
        expected: index + 1,
        got: args.len(),
    })?;
    value.downcast_ref::<T>().ok_or(VmError::ArgTypeMismatch {
        index,
        expected: std::any::type_name::<T>(),
    })
}

impl<R: 'static> ErasedCall for fn(&dyn Vm) -> R {
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        expect_arity(args, 0)?;
        Ok(Box::new((*self)(vm)))
    }
}

impl<A1: Clone + 'static, R: 'static> ErasedCall for fn(&dyn Vm, A1) -> R {
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        expect_arity(args, 1)?;
        let a1 = arg::<A1>(args, 0)?.clone();
        Ok(Box::new((*self)(vm, a1)))
    }
}

impl<A1: Clone + 'static, A2: Clone + 'static, R: 'static> ErasedCall for fn(&dyn Vm, A1, A2) -> R {
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        expect_arity(args, 2)?;
        let a1 = arg::<A1>(args, 0)?.clone();
        let a2 = arg::<A2>(args, 1)?.clone();
        Ok(Box::new((*self)(vm, a1, a2)))
    }
}

impl<A1: Clone + 'static, A2: Clone + 'static, A3: Clone + 'static, R: 'static> ErasedCall
    for fn(&dyn Vm, A1, A2, A3) -> R
{
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        expect_arity(args, 3)?;
        let a1 = arg::<A1>(args, 0)?.clone();
        let a2 = arg::<A2>(args, 1)?.clone();
        let a3 = arg::<A3>(args, 2)?.clone();
        Ok(Box::new((*self)(vm, a1, a2, a3)))
    }
}

/// 适配器：把 `Fn(&dyn Vm, &[&dyn Any]) -> Result<..>` 包成 [`ErasedCall`]。
///
/// （不能对闭包做 blanket impl —— 那会与上面的 fn 指针 impl 冲突。）
struct ErasedFn<F>(F);

impl<F> ErasedCall for ErasedFn<F>
where
    F: Fn(&dyn Vm, &[&dyn Any]) -> Result<Box<dyn Any>, VmError> + Send + Sync + 'static,
{
    fn invoke(&self, vm: &dyn Vm, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        (self.0)(vm, args)
    }
}

/// 闭包注册表 VM。
///
/// `id` 在构造时分配；注册发生在装配期（`&mut self`），调用期只读（`&self`）。
pub struct RegistryVm {
    id: VmId,
    names: Vec<QualifiedName>,
    typed: Vec<Option<Box<dyn Any + Send + Sync>>>,
    erased: Vec<Option<Box<dyn ErasedCall>>>,
}

impl Default for RegistryVm {
    fn default() -> Self {
        Self::new()
    }
}

impl RegistryVm {
    /// 新建空 VM。
    pub fn new() -> Self {
        Self {
            id: VmId::next(),
            names: Vec::new(),
            typed: Vec::new(),
            erased: Vec::new(),
        }
    }

    fn put(
        &mut self,
        name: QualifiedName,
        typed: Option<Box<dyn Any + Send + Sync>>,
        erased: Box<dyn ErasedCall>,
    ) {
        if let Some(i) = self.names.iter().position(|n| *n == name) {
            self.typed[i] = typed;
            self.erased[i] = Some(erased);
        } else {
            self.names.push(name);
            self.typed.push(typed);
            self.erased.push(Some(erased));
        }
    }

    /// 注册 arity 0 的 typed 函数（类型名由 [`IntoVmTypeName`] 推断）。
    pub fn register_fn0<R>(&mut self, path: &str, f: fn(&dyn Vm) -> R)
    where
        R: IntoVmTypeName + 'static,
    {
        self.register_fn0_named::<R>(path, R::NAME, f);
    }

    /// 注册 arity 0 的 typed 函数（显式类型名）。
    pub fn register_fn0_named<R>(&mut self, path: &str, ret: VmTypeName, f: fn(&dyn Vm) -> R)
    where
        R: 'static,
    {
        let name = QualifiedName::from_path(path, Vec::new(), ret);
        self.put(name, Some(Box::new(f)), Box::new(f));
    }

    /// 注册 arity 1 的 typed 函数（类型名由 [`IntoVmTypeName`] 推断）。
    pub fn register_fn1<A1, R>(&mut self, path: &str, f: fn(&dyn Vm, A1) -> R)
    where
        A1: IntoVmTypeName + Clone + 'static,
        R: IntoVmTypeName + 'static,
    {
        self.register_fn1_named::<A1, R>(path, A1::NAME, R::NAME, f);
    }

    /// 注册 arity 1 的 typed 函数（显式类型名）。
    pub fn register_fn1_named<A1, R>(
        &mut self,
        path: &str,
        param0: VmTypeName,
        ret: VmTypeName,
        f: fn(&dyn Vm, A1) -> R,
    ) where
        A1: Clone + 'static,
        R: 'static,
    {
        let name = QualifiedName::from_path(path, vec![param0.to_string()], ret);
        self.put(name, Some(Box::new(f)), Box::new(f));
    }

    /// 注册 arity 2 的 typed 函数（类型名由 [`IntoVmTypeName`] 推断）。
    pub fn register_fn2<A1, A2, R>(&mut self, path: &str, f: fn(&dyn Vm, A1, A2) -> R)
    where
        A1: IntoVmTypeName + Clone + 'static,
        A2: IntoVmTypeName + Clone + 'static,
        R: IntoVmTypeName + 'static,
    {
        self.register_fn2_named::<A1, A2, R>(path, A1::NAME, A2::NAME, R::NAME, f);
    }

    /// 注册 arity 2 的 typed 函数（显式类型名）。
    pub fn register_fn2_named<A1, A2, R>(
        &mut self,
        path: &str,
        param0: VmTypeName,
        param1: VmTypeName,
        ret: VmTypeName,
        f: fn(&dyn Vm, A1, A2) -> R,
    ) where
        A1: Clone + 'static,
        A2: Clone + 'static,
        R: 'static,
    {
        let name =
            QualifiedName::from_path(path, vec![param0.to_string(), param1.to_string()], ret);
        self.put(name, Some(Box::new(f)), Box::new(f));
    }

    /// 注册 arity 3 的 typed 函数（显式类型名）。
    pub fn register_fn3_named<A1, A2, A3, R>(
        &mut self,
        path: &str,
        param0: VmTypeName,
        param1: VmTypeName,
        param2: VmTypeName,
        ret: VmTypeName,
        f: fn(&dyn Vm, A1, A2, A3) -> R,
    ) where
        A1: Clone + 'static,
        A2: Clone + 'static,
        A3: Clone + 'static,
        R: 'static,
    {
        let name = QualifiedName::from_path(
            path,
            vec![param0.to_string(), param1.to_string(), param2.to_string()],
            ret,
        );
        self.put(name, Some(Box::new(f)), Box::new(f));
    }

    /// 注册只有慢路径（erased）的函数。
    ///
    /// 它的签名仍进入 [`Vm::functions`]，但 [`Vm::resolve`] 返回 `None`
    /// ⇒ 需要该函数的 capability **协商失败**（全有或全无），只能走 [`Vm::call`]。
    pub fn register_erased<F>(&mut self, path: &str, params: &[VmTypeName], ret: VmTypeName, f: F)
    where
        F: Fn(&dyn Vm, &[&dyn Any]) -> Result<Box<dyn Any>, VmError> + Send + Sync + 'static,
    {
        let name =
            QualifiedName::from_path(path, params.iter().map(|s| (*s).to_string()).collect(), ret);
        self.put(name, None, Box::new(ErasedFn(f)));
    }

    /// 注册的完整函数名数量。
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl Vm for RegistryVm {
    fn id(&self) -> VmId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        &self.names
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        let i = self.names.iter().position(|n| n == name)?;
        self.typed[i].as_deref()
    }

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        let entry = self
            .erased
            .get(id.get() as usize)
            .and_then(|e| e.as_deref())
            .ok_or_else(|| VmError::UnknownFunction(format!("函数编号 {id}")))?;
        entry.invoke(self, args)
    }
}
