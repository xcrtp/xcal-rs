// lua_vm.rs — 第一个**非原生 VM**（Lua 5.4，解释器经 `vendored` 编进二进制）。
//
// # 两个方向（不要混为一谈）
//
// ```text
// 脚本 → 宿主：装配期。宿主把 SceneServiceVm 以 Service 为单位经 Injector 注入；
//              Lua 侧 `scene` 表由 `binding.rs` **从 SceneApi::calls() 自动生成**。
// 宿主 → 脚本：Vm::call("xcal.script::run", [source])（erased 慢路径）。
// ```
//
// # 重入约束
//
// 脚本只**产出** `SceneScript`（装配期一次），不在每帧回调宿主改场景。

use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

use mlua::{Lua, Table, Value};
use xcal_scene::scene_script::SceneScript;
use xc_vm::{
    Capability, FunctionId, Injector, MissingRequirements, QualifiedName, Service,
    ServiceRequirement, Vm, VmError, VmId, negotiate_dyn, validate_requirements,
};

use crate::api::SceneApi;
use crate::service::{FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServiceVm};

/// `Injector` 的两个函数（与 `xc_vm::Injector::signatures()` 逐字一致）。
pub const REQUIREMENTS_PATH: &str = "xc_vm::injector::requirements";
pub const INJECT_PATH: &str = "xc_vm::injector::inject";
/// 宿主 → 脚本的唯一入口（erased）。
pub const RUN_PATH: &str = "xcal.script::run";

type RequirementsFn = fn(&dyn Vm) -> Vec<ServiceRequirement>;
type InjectFn = fn(&dyn Vm, String, Arc<dyn Vm>) -> Result<(), VmError>;
type RunFn = fn(&dyn Vm, String) -> Result<SceneScript, VmError>;

/// VM 的可变状态（`Vm` 只有 `&self`，故用内部可变性）。
struct ScriptState {
    lua: Lua,
    /// 注入后的场景 Service VM（`run` 结束时向它索取 `SceneScript`）。
    scene: Option<Arc<dyn Vm>>,
}

/// Lua 脚本 VM。
pub struct LuaVm {
    id: VmId,
    names: Vec<QualifiedName>,
    state: RefCell<ScriptState>,
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
            QualifiedName::from_path(RUN_PATH, vec!["String".to_string()], "SceneScript"),
        ];
        Self {
            id: VmId::next(),
            names,
            state: RefCell::new(ScriptState {
                lua: Lua::new(),
                scene: None,
            }),
            requirements_fn: requirements_impl,
            inject_fn: inject_impl,
            run_fn: run_impl,
        }
    }

    /// 当前 Lua 全局表 `scene` 注册的函数名（排序后）。
    ///
    /// 一致性护栏用例使用：**实际注册名** vs `SceneApi::signatures()` 的名字集合。
    pub fn registered_scene_names(&self) -> Vec<String> {
        let state = self.state.borrow();
        let Ok(table) = state.lua.globals().get::<Table>("scene") else {
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

    /// 宿主 → 脚本：执行一段 Lua 源码并取回 `SceneScript`。
    pub fn run(&self, source: &str) -> Result<SceneScript, VmError> {
        let id = self
            .function_id(&QualifiedName::from_path(
                RUN_PATH,
                vec!["String".to_string()],
                "SceneScript",
            ))
            .ok_or_else(|| VmError::UnknownFunction(RUN_PATH.to_string()))?;
        let owned = source.to_string();
        let arg: &dyn Any = &owned;
        let out = self.call(id, &[arg])?;
        out.downcast::<SceneScript>()
            .map(|b| *b)
            .map_err(|_| VmError::CallFailed("run 没有返回 SceneScript".to_string()))
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
                Ok(Box::new((self.run_fn)(self, source)?))
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

fn requirements_impl(_vm: &dyn Vm) -> Vec<ServiceRequirement> {
    SceneApi::signatures()
        .iter()
        .map(|sig| ServiceRequirement::required(SERVICE_NAME, &sig.qualified().to_string()))
        .collect()
}

fn inject_impl(vm: &dyn Vm, service: String, service_vm: Arc<dyn Vm>) -> Result<(), VmError> {
    let this = lua_vm(vm);
    if service != SERVICE_NAME {
        return Err(VmError::CallFailed(format!(
            "未知 Service `{service}`（脚本 VM 只接受 `{SERVICE_NAME}`）"
        )));
    }
    // 先协商一次：服务没提供完整 SceneApi ⇒ 明确失败（绑定层不再逐函数检查）。
    let api: SceneApi<'_> = negotiate_dyn(&*service_vm).ok_or_else(|| {
        VmError::CallFailed("scene 服务未提供完整的 SceneApi（协商失败）".to_string())
    })?;
    drop(api);

    let mut state = this.state.borrow_mut();
    // 全部 Lua 胶水由 `calls()` + `TypeSpec` 生成（这里没有逐函数闭包）。
    crate::binding::bind(&state.lua, Arc::clone(&service_vm)).map_err(lua_err)?;
    state.scene = Some(service_vm);
    Ok(())
}

// ─── 宿主 → 脚本：run ──────────────────────────────────────────────────

fn run_impl(vm: &dyn Vm, source: String) -> Result<SceneScript, VmError> {
    let this = lua_vm(vm);
    let state = this.state.borrow();
    let scene = state.scene.as_ref().ok_or_else(|| {
        VmError::CallFailed("脚本 VM 尚未注入 scene 服务（先经 Injector 注入）".to_string())
    })?;

    scene.call(function_id(scene, RESET_PATH, "()")?, &[])?;

    state
        .lua
        .load(&source)
        .exec()
        .map_err(|e| VmError::CallFailed(format!("Lua 执行失败：{e}")))?;

    let out = scene.call(function_id(scene, FINISH_PATH, "SceneScript")?, &[])?;
    out.downcast::<SceneScript>()
        .map(|b| *b)
        .map_err(|_| VmError::CallFailed("scene 服务没有返回 SceneScript".to_string()))
}

fn function_id(vm: &Arc<dyn Vm>, path: &str, ret: &str) -> Result<FunctionId, VmError> {
    vm.function_id(&QualifiedName::from_path(path, vec![], ret))
        .ok_or_else(|| VmError::UnknownFunction(path.to_string()))
}

// ─── 宿主侧装配入口（双向各一条）────────────────────────────────────────

/// 装配好的脚本宿主：Lua VM + 注入进去的场景 Service。
pub struct ScriptHost {
    pub lua: Arc<LuaVm>,
    pub scene: Arc<SceneServiceVm>,
}

impl ScriptHost {
    /// 装配：声明需求 → 校验 → 注入（任一环节失败都返回明确错误）。
    pub fn new() -> Result<Self, VmError> {
        let lua = Arc::new(LuaVm::new());
        let scene = Arc::new(SceneServiceVm::new());
        let injector: Injector = negotiate_dyn(&*lua).ok_or_else(|| {
            VmError::CallFailed("LuaVm 未暴露 Injector（默认拒绝）".to_string())
        })?;
        let service = Service::new(SERVICE_NAME, Arc::clone(&scene) as Arc<dyn Vm>);
        validate_requirements(&injector.requirements(), std::slice::from_ref(&service))
            .map_err(|missing: MissingRequirements| VmError::CallFailed(missing.to_string()))?;
        injector
            .inject_service(&service)
            .map_err(|e| VmError::CallFailed(format!("注入失败：{e}")))?;
        Ok(Self { lua, scene })
    }

    /// 宿主 → 脚本（erased 入口）。
    pub fn run(&self, source: &str) -> Result<SceneScript, VmError> {
        self.lua.run(source)
    }
}
