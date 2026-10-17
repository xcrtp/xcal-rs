// lua_vm.rs — 第一个**非原生 VM**（Lua 5.4，解释器经 `vendored` 编进二进制）。
//
// # 两个方向
//
// ```text
// 脚本 → 宿主：装配期。两个 Service（scene / content）经 Injector 注入；
//              Lua 侧 `scene.*` / `content.*` 与类型构造器由 `calls()` 自动生成。
// 宿主 → 脚本：Vm::call("xcal.script::run", [source])（erased 慢路径）。
// ```
//
// # Injector 的第二次真实使用
//
// 第一次是场景装配（`SceneApi`），第二次是内容编译（`ContentApi`）。
// 同一套机制服务两种完全不同的能力 —— 这才是"通用"的判据。

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use mlua::{Lua, Table, Value};
use xc_vm::{Capability, FunctionId, QualifiedName, ServiceRequirement, Vm, VmError, VmId};

/// `Injector` 的两个函数（与 `xc_vm::Injector::signatures()` 逐字一致）。
pub const REQUIREMENTS_PATH: &str = "xc_vm::injector::requirements";
pub const INJECT_PATH: &str = "xc_vm::injector::inject";
/// 宿主 → 脚本的唯一入口（erased）。
pub const RUN_PATH: &str = "xcal.script::run";
/// `run` 的完整函数名（含参数 / 返回类型；`Broker::run_with` 用）。
pub const RUN_ENTRY: &str = "xcal.script::run(String) -> Any";
/// 入口 Service 的约定名（解释器与"场景录制器"之间的协议，不是 crate 依赖）。
pub const ENTRY_SERVICE: &str = "scene";
/// 入口 Service 的内部交接函数（不是脚本可见 API）。
pub const RESET_PATH: &str = "xcal.scene::__reset";
pub const FINISH_PATH: &str = "xcal.scene::__finish";

type RequirementsFn = fn(&dyn Vm) -> Vec<ServiceRequirement>;
type InjectFn = fn(&dyn Vm, String, Arc<dyn Vm>) -> Result<(), VmError>;
type RunFn = fn(&dyn Vm, String) -> Result<Box<dyn Any>, VmError>;

/// VM 的可变状态（`Vm` 只有 `&self`，故用内部可变性）。
struct ScriptState {
    lua: Lua,
    /// 场景装配 Service（`run` 的内部交接需要它）。
    scene: Option<Arc<dyn Vm>>,
}

/// 一个已注册能力的注入端点。
struct BinderEntry {
    /// 协商 + 建语言侧表。
    bind: fn(&Lua, &str, Arc<dyn Vm>) -> Result<(), VmError>,
    /// 自述需求（`required` 由注册时决定）。
    requirements: Vec<ServiceRequirement>,
}

/// Lua 脚本 VM。
pub struct LuaVm {
    id: VmId,
    names: Vec<QualifiedName>,
    state: RefCell<ScriptState>,
    /// Service 名 → 能力绑定器（**不写死任何 domain 类型**）。
    binders: RefCell<BTreeMap<String, BinderEntry>>,
    requirements_fn: RequirementsFn,
    inject_fn: InjectFn,
    run_fn: RunFn,
}

impl Default for LuaVm {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaVm {
    /// 新建脚本 VM（尚未注入任何宿主能力）。
    pub fn new() -> Self {
        let names = vec![
            QualifiedName::from_path(REQUIREMENTS_PATH, vec![], "ServiceRequirements"),
            QualifiedName::from_path(
                INJECT_PATH,
                vec!["ServiceName".to_string(), "ServiceVm".to_string()],
                "InjectResult",
            ),
            QualifiedName::from_path(RUN_PATH, vec!["String".to_string()], "Any"),
        ];
        Self {
            id: VmId::next(),
            names,
            state: RefCell::new(ScriptState {
                lua: Lua::new(),
                scene: None,
            }),
            binders: RefCell::new(BTreeMap::new()),
            requirements_fn: requirements_impl,
            inject_fn: inject_impl,
            run_fn: run_impl,
        }
    }

    /// 注册一个能力的绑定器（必须在 Broker 装配前调用）。
    ///
    /// `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配，
    /// 只是 Lua 全局表中不会出现对应的表（B4 的 `python` 走这条路径）。
    pub fn register_capability<C: Capability<'static>>(&self, service: &str, required: bool) {
        let requirements = C::signatures()
            .iter()
            .map(|sig| ServiceRequirement::new(service, &sig.qualified().to_string(), required))
            .collect();
        self.binders.borrow_mut().insert(
            service.to_string(),
            BinderEntry {
                bind: bind_capability::<C>,
                requirements,
            },
        );
    }

    /// 当前 Lua 全局表 `scene` 注册的函数名（排序后）。
    ///
    /// 一致性护栏用例使用：**实际注册名** vs `SceneApi::signatures()` 的名字集合。
    pub fn registered_scene_names(&self) -> Vec<String> {
        self.registered_names("scene")
    }

    /// 当前 Lua 全局表 `content` 注册的函数名（排序后）。
    pub fn registered_content_names(&self) -> Vec<String> {
        self.registered_names("content")
    }

    /// 任意全局表注册的函数名（排序后；表不存在 ⇒ 空）。
    pub fn registered_global_names(&self, global: &str) -> Vec<String> {
        self.registered_names(global)
    }

    fn registered_names(&self, global: &str) -> Vec<String> {
        let state = self.state.borrow();
        let Ok(table) = state.lua.globals().get::<Table>(global) else {
            return Vec::new();
        };
        let mut names: Vec<String> = table
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .map(|(name, _)| name)
            .collect();
        names.sort();
        names
    }

    /// 宿主 → 脚本：执行一段 Lua 源码，取回入口 Service 的产物 `T`。
    ///
    /// 解释器不知道 `T` 是什么（`SceneScript` / 任意录制结果），
    /// 因此这里对 `T` 完全泛型 —— 这是 Lua 适配器能保持"纯"的关键。
    pub fn run<T: 'static>(&self, source: &str) -> Result<T, VmError> {
        let id = self
            .function_id(&QualifiedName::from_path(
                RUN_PATH,
                vec!["String".to_string()],
                "Any",
            ))
            .ok_or_else(|| VmError::UnknownFunction(RUN_PATH.to_string()))?;
        let owned = source.to_string();
        let arg: &dyn Any = &owned;
        let out = self.call(id, &[arg])?;
        out.downcast::<T>().map(|boxed| *boxed).map_err(|_| {
            VmError::CallFailed(format!("run 的返回值不是 `{}`", std::any::type_name::<T>()))
        })
    }
}

impl Vm for LuaVm {
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
        if *name == self.names[0] {
            Some(&self.requirements_fn)
        } else if *name == self.names[1] {
            Some(&self.inject_fn)
        } else if *name == self.names[2] {
            Some(&self.run_fn)
        } else {
            None
        }
    }

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        match id.get() {
            0 => Ok(Box::new((self.requirements_fn)(self))),
            1 => {
                let service = arg::<String>(args, 0)?.clone();
                let vm = Arc::clone(arg::<Arc<dyn Vm>>(args, 1)?);
                (self.inject_fn)(self, service, vm)?;
                Ok(Box::new(()))
            }
            2 => {
                let source = arg::<String>(args, 0)?.clone();
                (self.run_fn)(self, source)
            }
            _ => Err(VmError::UnknownFunction(format!("函数编号 {id}"))),
        }
    }
}

fn arg<'a, T: 'static>(args: &'a [&'a dyn Any], index: usize) -> Result<&'a T, VmError> {
    args.get(index)
        .and_then(|a| a.downcast_ref::<T>())
        .ok_or_else(|| VmError::CallFailed(format!("第 {index} 个实参类型不匹配")))
}

fn lua_vm(vm: &dyn Vm) -> &LuaVm {
    vm.as_any()
        .downcast_ref::<LuaVm>()
        .expect("LuaVm 的函数只能由 LuaVm 自己调用")
}

fn lua_err(e: mlua::Error) -> VmError {
    VmError::CallFailed(format!("Lua：{e}"))
}

// ─── Injector：VM 自述需求 + 接收注入 ───────────────────────────────────

/// 一个能力的语言侧绑定器（泛型在注册时单态化，运行时只留 fn 指针）。
fn bind_capability<C: Capability<'static>>(
    lua: &Lua,
    global: &str,
    vm: Arc<dyn Vm>,
) -> Result<(), VmError> {
    // Broker 已在装配期做函数级校验；这里保留手动注入的最后一道闸。
    let missing: Vec<&str> = C::signatures()
        .iter()
        .filter(|sig| !vm.functions().contains(&sig.qualified()))
        .map(|sig| sig.name)
        .collect();
    if !missing.is_empty() {
        return Err(VmError::CallFailed(format!(
            "`{global}` 缺少函数：{}",
            missing.join(", ")
        )));
    }
    crate::binding::bind::<C>(lua, global, vm).map_err(lua_err)
}

fn requirements_impl(vm: &dyn Vm) -> Vec<ServiceRequirement> {
    let this = lua_vm(vm);
    this.binders
        .borrow()
        .values()
        .flat_map(|entry| entry.requirements.iter().cloned())
        .collect()
}

fn inject_impl(vm: &dyn Vm, service: String, service_vm: Arc<dyn Vm>) -> Result<(), VmError> {
    let this = lua_vm(vm);
    let binder = this.binders.borrow().get(&service).map(|entry| entry.bind);
    let Some(binder) = binder else {
        return Err(VmError::CallFailed(format!(
            "未知 Service `{service}`（能力未注册；先调用 `LuaVm::register_capability`）"
        )));
    };
    let mut state = this.state.borrow_mut();
    binder(&state.lua, &service, Arc::clone(&service_vm))?;
    if service == ENTRY_SERVICE {
        state.scene = Some(service_vm);
    }
    Ok(())
}

// ─── 宿主 → 脚本：run ──────────────────────────────────────────────────

fn run_impl(vm: &dyn Vm, source: String) -> Result<Box<dyn Any>, VmError> {
    let this = lua_vm(vm);
    let state = this.state.borrow();
    let scene = state.scene.as_ref().ok_or_else(|| {
        VmError::CallFailed("脚本 VM 尚未注入入口服务（先经 Injector 注入 scene）".to_string())
    })?;

    scene.call(function_id(scene, RESET_PATH, "()")?, &[])?;

    state
        .lua
        .load(&source)
        .exec()
        .map_err(|e| VmError::CallFailed(format!("Lua 执行失败：{e}")))?;

    // 入口服务把产物装箱返回：`Result<Box<dyn Any>, String>`。
    let out = scene.call(
        function_id(scene, FINISH_PATH, "Result<Box<dyn Any>, String>")?,
        &[],
    )?;
    match out.downcast::<Result<Box<dyn Any>, String>>() {
        Ok(result) => match *result {
            Ok(value) => Ok(value),
            Err(message) => Err(VmError::CallFailed(message)),
        },
        Err(_) => Err(VmError::CallFailed(
            "入口服务没有返回 Result<Box<dyn Any>, String>".to_string(),
        )),
    }
}

fn function_id(vm: &Arc<dyn Vm>, path: &str, ret: &str) -> Result<FunctionId, VmError> {
    vm.function_id(&QualifiedName::from_path(path, vec![], ret))
        .ok_or_else(|| VmError::UnknownFunction(path.to_string()))
}
