// lua_vm.rs — 第一个**非原生 VM**（Lua 5.4，解释器经 `vendored` 编进二进制）。
//
// # 两个方向（不要混为一谈）
//
// ```text
// 脚本 → 宿主：装配期。宿主把 SceneServiceVm 以 Service 为单位经 Injector 注入；
//              Lua 侧注册全局表 `scene`，每个函数捕获 **typed fn 指针**（快路径）。
// 宿主 → 脚本：Vm::call("xcal.script::run", [source])（erased 慢路径）。
//              `run` 执行脚本、回调控宿主、最后取回 SceneScript。
// ```
//
// 计划 §L4 的先核查结论：Scene Service 是**原生 Rust + typed 注册** ⇒
// `negotiate_dyn::<SceneApi>()` 可用 ⇒ 胶水层拿到 typed 快路径。脚本 VM 自己
// 暴露给宿主的 `run` 只有 erased（宿主走 `Vm::call`）—— 两条方向互不影响。
//
// # 重入约束
//
// 脚本只**产出** `SceneScript`（装配期一次），不在每帧回调宿主改场景
// （见 `xc-vm` 的重入约束：回调只读查询，写场景走命令）。

use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

use mlua::{AnyUserData, Lua, Table, UserData, Value};
use xc_vm::{
    Capability, FunctionId, Injector, MissingRequirements, QualifiedName, Service,
    ServiceRequirement, Vm, VmError, VmId, negotiate_dyn, validate_requirements,
};

use crate::api::SceneApi;
use crate::service::{
    FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServiceVm, target_exists,
};
use xcal_scene::scene_script::SceneScript;

/// `Injector` 的两个函数（完整函数名与 `xc_vm::Injector::signatures()` 逐字一致）。
pub const REQUIREMENTS_PATH: &str = "xc_vm::injector::requirements";
pub const INJECT_PATH: &str = "xc_vm::injector::inject";
/// 宿主 → 脚本的唯一入口（erased）。
pub const RUN_PATH: &str = "xcal.script::run";

type RequirementsFn = fn(&dyn Vm) -> Vec<ServiceRequirement>;
type InjectFn = fn(&dyn Vm, String, Arc<dyn Vm>) -> Result<(), VmError>;
type RunFn = fn(&dyn Vm, String) -> Result<SceneScript, VmError>;

/// Lua 侧的句柄令牌（`xc_vm::Handle` 的 userdata 包装）。
///
/// 泛型 / 域类型不出内核：`xc-vm` 只出"号"，脚本语言怎么表示它由本层决定。
#[derive(Clone, Copy)]
struct LuaHandle(xc_vm::Handle);

impl UserData for LuaHandle {}

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
    ///
    /// Lua 解释器在 `Lua::new()` 里创建；此时**没有** `scene` 全局表 ——
    /// 注入成功后才出现（默认拒绝）。
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
    /// 供一致性护栏用例使用：**实际注册名** vs `SceneApi::signatures()` 的名字集合。
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
    ///
    /// 内部走 `Vm::call`（erased）；这是宿主侧唯一需要的入口。
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
    // 逐条要求 `SceneApi::signatures()`（全有或全无；缺一个就不装配脚本）。
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
    let api: SceneApi<'_> = negotiate_dyn(&*service_vm).ok_or_else(|| {
        VmError::CallFailed("scene 服务未提供完整的 SceneApi（协商失败）".to_string())
    })?;
    // 把 typed fn 指针复制出来：闭包要 `'static`，不能持有借用 VM 的句柄。
    let fns = SceneFns {
        circle: api.circle,
        stroke: api.stroke,
        camera2d: api.camera2d,
        reveal: api.reveal,
        spawn: api.spawn,
        despawn: api.despawn,
        clip: api.clip,
        animate: api.animate,
        parallel: api.parallel,
        wait: api.wait,
        sin: api.sin,
        phase: api.phase,
        constant: api.constant,
        ramp: api.ramp,
        ramp2: api.ramp2,
    };
    drop(api);

    let mut state = this.state.borrow_mut();
    let lua = &state.lua;
    let table = lua.create_table().map_err(lua_err)?;

    // ── 组件构造 ────────────────────────────────────────────────────────
    let svc = Arc::clone(&service_vm);
    let f = fns.circle;
    table
        .set(
            "circle",
            lua.create_function(move |_, (x, y, r): (f32, f32, f32)| {
                Ok(LuaHandle(f(&*svc, x, y, r)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.stroke;
    table
        .set(
            "stroke",
            lua.create_function(move |_, (width, r, g, b, a): (f32, u32, u32, u32, u32)| {
                // Lua 侧是 (r, g, b, a) 四个分量；宿主侧收打包的 0xRRGGBBAA。
                let rgba = ((r & 0xff) << 24) | ((g & 0xff) << 16) | ((b & 0xff) << 8) | (a & 0xff);
                Ok(LuaHandle(f(&*svc, width, rgba)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.camera2d;
    table
        .set(
            "camera2d",
            lua.create_function(move |_, (cx, cy, scale): (f32, f32, f32)| {
                Ok(LuaHandle(f(&*svc, cx, cy, scale)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.reveal;
    table
        .set(
            "reveal",
            lua.create_function(move |_, (points, closed): (Table, bool)| {
                let flat = flatten_floats(&points)?;
                if flat.len() % 2 != 0 {
                    return Err(mlua::Error::RuntimeError(
                        "reveal：点表长度必须是偶数（[x0, y0, x1, y1, ...]）".to_string(),
                    ));
                }
                Ok(LuaHandle(f(&*svc, flat, closed)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    // ── 对象 ────────────────────────────────────────────────────────────
    let svc = Arc::clone(&service_vm);
    let f = fns.spawn;
    table
        .set(
            "spawn",
            lua.create_function(move |_, components: Table| {
                Ok(LuaHandle(f(&*svc, flatten_handles(&components)?)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.despawn;
    table
        .set(
            "despawn",
            lua.create_function(move |_, object: AnyUserData| {
                f(&*svc, handle_of(&object)?);
                Ok(())
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    // ── 动画 ────────────────────────────────────────────────────────────
    let svc = Arc::clone(&service_vm);
    let f = fns.clip;
    table
        .set(
            "clip",
            lua.create_function(move |_, (target, curve): (String, AnyUserData)| {
                if !target_exists(&target) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "未知动画目标 `{target}`（见 target_ctor_by_name 表）"
                    )));
                }
                Ok(LuaHandle(f(&*svc, target, handle_of(&curve)?)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.animate;
    table
        .set(
            "animate",
            lua.create_function(
                move |_, (object, clip, duration): (AnyUserData, AnyUserData, f64)| {
                    f(&*svc, handle_of(&object)?, handle_of(&clip)?, duration);
                    Ok(())
                },
            )
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.parallel;
    table
        .set(
            "parallel",
            lua.create_function(move |_, (object, clip): (AnyUserData, AnyUserData)| {
                f(&*svc, handle_of(&object)?, handle_of(&clip)?);
                Ok(())
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.wait;
    table
        .set(
            "wait",
            lua.create_function(move |_, d: f64| {
                f(&*svc, d);
                Ok(())
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    // ── 曲线 ────────────────────────────────────────────────────────────
    let svc = Arc::clone(&service_vm);
    let f = fns.sin;
    table
        .set(
            "sin",
            lua.create_function(move |_, (base, amp, omega): (f32, f32, f32)| {
                Ok(LuaHandle(f(&*svc, base, amp, omega)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.phase;
    table
        .set(
            "phase",
            lua.create_function(move |_, omega: f32| Ok(LuaHandle(f(&*svc, omega))))
                .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.constant;
    table
        .set(
            "constant",
            lua.create_function(move |_, value: f32| Ok(LuaHandle(f(&*svc, value))))
                .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.ramp;
    table
        .set(
            "ramp",
            lua.create_function(move |_, (from, to, dur): (f32, f32, f32)| {
                Ok(LuaHandle(f(&*svc, from, to, dur)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    let svc = Arc::clone(&service_vm);
    let f = fns.ramp2;
    table
        .set(
            "ramp2",
            lua.create_function(move |_, (points, dur): (Table, f32)| {
                let flat = flatten_floats(&points)?;
                if flat.len() != 4 {
                    return Err(mlua::Error::RuntimeError(
                        "ramp2：点表必须是 [from_x, from_y, to_x, to_y]".to_string(),
                    ));
                }
                Ok(LuaHandle(f(&*svc, flat, dur)))
            })
            .map_err(lua_err)?,
        )
        .map_err(lua_err)?;

    lua.globals().set("scene", table).map_err(lua_err)?;
    state.scene = Some(service_vm);
    Ok(())
}

/// 复制出来的 `SceneApi` typed fn 指针（闭包需要 `'static`）。
struct SceneFns {
    circle: fn(&dyn Vm, f32, f32, f32) -> xc_vm::Handle,
    stroke: fn(&dyn Vm, f32, u32) -> xc_vm::Handle,
    camera2d: fn(&dyn Vm, f32, f32, f32) -> xc_vm::Handle,
    reveal: fn(&dyn Vm, Vec<f32>, bool) -> xc_vm::Handle,
    spawn: fn(&dyn Vm, Vec<xc_vm::Handle>) -> xc_vm::Handle,
    despawn: fn(&dyn Vm, xc_vm::Handle),
    clip: fn(&dyn Vm, String, xc_vm::Handle) -> xc_vm::Handle,
    animate: fn(&dyn Vm, xc_vm::Handle, xc_vm::Handle, f64),
    parallel: fn(&dyn Vm, xc_vm::Handle, xc_vm::Handle),
    wait: fn(&dyn Vm, f64),
    sin: fn(&dyn Vm, f32, f32, f32) -> xc_vm::Handle,
    phase: fn(&dyn Vm, f32) -> xc_vm::Handle,
    constant: fn(&dyn Vm, f32) -> xc_vm::Handle,
    ramp: fn(&dyn Vm, f32, f32, f32) -> xc_vm::Handle,
    ramp2: fn(&dyn Vm, Vec<f32>, f32) -> xc_vm::Handle,
}

fn handle_of(value: &AnyUserData) -> mlua::Result<xc_vm::Handle> {
    value
        .borrow::<LuaHandle>()
        .map(|h| h.0)
        .map_err(|_| mlua::Error::RuntimeError("期望 xcal 场景句柄（userdata）".to_string()))
}

fn flatten_floats(table: &Table) -> mlua::Result<Vec<f32>> {
    table.sequence_values::<f32>().collect()
}

fn flatten_handles(table: &Table) -> mlua::Result<Vec<xc_vm::Handle>> {
    let mut out = Vec::new();
    for value in table.sequence_values::<AnyUserData>() {
        out.push(handle_of(&value?)?);
    }
    Ok(out)
}

// ─── 宿主 → 脚本：run ──────────────────────────────────────────────────

fn run_impl(vm: &dyn Vm, source: String) -> Result<SceneScript, VmError> {
    let this = lua_vm(vm);
    let state = this.state.borrow();
    let scene = state.scene.as_ref().ok_or_else(|| {
        VmError::CallFailed("脚本 VM 尚未注入 scene 服务（先经 Injector 注入）".to_string())
    })?;

    // 每轮从空状态开始（上一轮的组件 / 曲线 / 动作句柄不得跨 run 复用）。
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
