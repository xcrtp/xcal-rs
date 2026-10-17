// service.rs — 宿主侧 **Scene Service VM**（原生 Rust，注册 typed 函数）。
//
// # 它在双向对接里的位置
//
// ```text
// 脚本 ──(Lua 全局表 scene.*)──> SceneApi 的 typed fn 指针 ──> 本 VM 的状态
// 宿主 ──Vm::call("xcal.script::run")──> LuaVm ──执行脚本──> 回调本 VM ──> SceneScript
// ```
//
// 本 VM **只被脚本在装配期调用**（不在每帧），故走 typed 快路径只是顺带收益；
// `SceneApi::signatures()` 与这里注册的完整函数名必须逐字一致（见一致性护栏用例）。

use std::any::Any;
use std::cell::RefCell;

use xcal_scene::animation::target::target_ctor_by_name;
use xcal_scene::scene::components::Reveal;
use xcal_scene::scene_script::{Handle as ScriptHandle, SceneEncoder, SceneScript};
use xcal_scene::{AnimValue, Curve, Interp};
use xc_notion::{Color32, Path2D, Vec2};
use xc_object::{Circle, Framing2D};
use xc_render_ir::{MeasureSpace, StrokeSolid};
use xc_scene_contract::{Component, Components};
use xc_vm::{FunctionId, Handle, HandleTable, QualifiedName, RegistryVm, Vm, VmError, VmId};

/// 注入时使用的 Service 名（与 `LuaVm::requirements()` 对应）。
pub const SERVICE_NAME: &str = "scene";

/// 内部交接函数：把录制结果取走并重置（不是脚本可见 API）。
pub const FINISH_PATH: &str = "xcal.scene::__finish";
/// 内部交接函数：丢弃上一轮录制状态（每次 `run` 前调用）。
pub const RESET_PATH: &str = "xcal.scene::__reset";

/// 一个"动画动作"：target 名字 + 曲线（`clip` 的产物）。
#[derive(Clone)]
struct Clip {
    target: String,
    curve: Curve,
}

/// Service 的全部可变状态（`Vm` 只有 `&self` ⇒ 内部可变性）。
struct Recording {
    owner: VmId,
    components: HandleTable<Box<dyn Component>>,
    curves: HandleTable<Curve>,
    clips: HandleTable<Clip>,
    objects: HandleTable<ScriptHandle>,
    encoder: SceneEncoder,
}

impl Recording {
    fn new(owner: VmId) -> Self {
        Self {
            owner,
            components: HandleTable::new(owner),
            curves: HandleTable::new(owner),
            clips: HandleTable::new(owner),
            objects: HandleTable::new(owner),
            encoder: SceneEncoder::new(),
        }
    }
}

/// 宿主侧场景能力 VM。
///
/// 状态归 VM 自己管（`RefCell`）；typed 函数从 `&dyn Vm` 上取回 `Self`。
pub struct SceneServiceVm {
    id: VmId,
    registry: RegistryVm,
    rec: RefCell<Recording>,
}

impl Default for SceneServiceVm {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneServiceVm {
    /// 新建并注册全部 typed 函数。
    pub fn new() -> Self {
        let id = VmId::next();
        let mut registry = RegistryVm::new();

        // ── 组件构造（每个组件一个函数；新增组件只加一个构造函数）──────────
        registry.register_fn3_named::<f32, f32, f32, Handle>(
            "xcal.scene::circle",
            "f32",
            "f32",
            "f32",
            "xc_vm.Handle",
            scene_circle,
        );
        registry.register_fn2_named::<f32, u32, Handle>(
            "xcal.scene::stroke",
            "f32",
            "u32",
            "xc_vm.Handle",
            scene_stroke,
        );
        registry.register_fn3_named::<f32, f32, f32, Handle>(
            "xcal.scene::camera2d",
            "f32",
            "f32",
            "f32",
            "xc_vm.Handle",
            scene_camera2d,
        );
        registry.register_fn2_named::<Vec<f32>, bool, Handle>(
            "xcal.scene::reveal",
            "FloatList",
            "bool",
            "xc_vm.Handle",
            scene_reveal,
        );

        // ── 对象 ────────────────────────────────────────────────────────
        registry.register_fn1_named::<Vec<Handle>, Handle>(
            "xcal.scene::spawn",
            "ComponentList",
            "xc_vm.Handle",
            scene_spawn,
        );
        registry.register_fn1::<Handle, ()>("xcal.scene::despawn", scene_despawn);

        // ── 动画（target 走字符串名，复用持久化集中表）────────────────────
        registry.register_fn2::<String, Handle, Handle>("xcal.scene::clip", scene_clip);
        registry.register_fn3_named::<Handle, Handle, f64, ()>(
            "xcal.scene::animate",
            "xc_vm.Handle",
            "xc_vm.Handle",
            "f64",
            "()",
            scene_animate,
        );
        registry.register_fn2::<Handle, Handle, ()>("xcal.scene::parallel", scene_parallel);
        registry.register_fn1::<f64, ()>("xcal.scene::wait", scene_wait);

        // ── 曲线 ────────────────────────────────────────────────────────
        registry.register_fn3_named::<f32, f32, f32, Handle>(
            "xcal.scene::sin",
            "f32",
            "f32",
            "f32",
            "xc_vm.Handle",
            scene_sin,
        );
        registry.register_fn1::<f32, Handle>("xcal.scene::phase", scene_phase);
        registry.register_fn1::<f32, Handle>("xcal.scene::constant", scene_constant);
        registry.register_fn3_named::<f32, f32, f32, Handle>(
            "xcal.scene::ramp",
            "f32",
            "f32",
            "f32",
            "xc_vm.Handle",
            scene_ramp,
        );
        registry.register_fn2_named::<Vec<f32>, f32, Handle>(
            "xcal.scene::ramp2",
            "FloatList",
            "f32",
            "xc_vm.Handle",
            scene_ramp2,
        );

        // ── 内部交接（不是脚本可见 API；走 erased / `Vm::call`）────────────
        registry.register_erased(FINISH_PATH, &[], "SceneScript", |vm, _| {
            Ok(Box::new(service(vm).finish()))
        });
        registry.register_erased(RESET_PATH, &[], "()", |vm, _| {
            service(vm).reset();
            Ok(Box::new(()))
        });

        Self {
            id,
            registry,
            rec: RefCell::new(Recording::new(id)),
        }
    }

    /// 取走本轮的 `SceneScript`，并重置状态（下一轮从空开始）。
    fn finish(&self) -> SceneScript {
        let mut rec = self.rec.borrow_mut();
        let encoder = std::mem::replace(&mut rec.encoder, SceneEncoder::new());
        let script = encoder.finish();
        // 句柄表同步清空：上一轮的组件 / 曲线 / 动作句柄不得跨 run 复用。
        let owner = rec.owner;
        *rec = Recording::new(owner);
        script
    }

    /// 丢弃当前录制状态。
    fn reset(&self) {
        let mut rec = self.rec.borrow_mut();
        let owner = rec.owner;
        *rec = Recording::new(owner);
    }

    fn with_rec<T>(&self, f: impl FnOnce(&mut Recording) -> T) -> T {
        f(&mut self.rec.borrow_mut())
    }
}

impl Vm for SceneServiceVm {
    fn id(&self) -> VmId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }

    fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        let name = self
            .functions()
            .get(id.get() as usize)
            .ok_or_else(|| VmError::UnknownFunction(format!("函数编号 {id}")))?;
        match name.name() {
            "__finish" => Ok(Box::new(self.finish())),
            "__reset" => {
                self.reset();
                Ok(Box::new(()))
            }
            other => Err(VmError::CallFailed(format!(
                "SceneServiceVm 的 `{other}` 只支持 typed 快路径（脚本经 SceneApi 调用）"
            ))),
        }
    }
}

/// 从 `&dyn Vm` 取回 `SceneServiceVm`（接错 VM 属于装配错误）。
fn service(vm: &dyn Vm) -> &SceneServiceVm {
    vm.as_any()
        .downcast_ref::<SceneServiceVm>()
        .expect("SceneApi 的调用者必须是 SceneServiceVm")
}

// ─── typed 函数实现 ──────────────────────────────────────────────────────

fn scene_circle(vm: &dyn Vm, x: f32, y: f32, r: f32) -> Handle {
    service(vm).with_rec(|rec| rec.components.insert(Box::new(Circle::new((x, y), r))))
}

fn scene_stroke(vm: &dyn Vm, width: f32, rgba: u32) -> Handle {
    let color = Color32::new(
        ((rgba >> 24) & 0xff) as u8,
        ((rgba >> 16) & 0xff) as u8,
        ((rgba >> 8) & 0xff) as u8,
        (rgba & 0xff) as u8,
    );
    service(vm).with_rec(|rec| {
        rec.components.insert(Box::new(StrokeSolid::new_in(
            width,
            color,
            MeasureSpace::Screen,
        )))
    })
}

fn scene_camera2d(vm: &dyn Vm, cx: f32, cy: f32, scale: f32) -> Handle {
    service(vm).with_rec(|rec| {
        rec.components
            .insert(Box::new(Framing2D::new(Vec2::new(cx, cy), scale)))
    })
}

fn scene_reveal(vm: &dyn Vm, flat: Vec<f32>, closed: bool) -> Handle {
    let points = points_of(&flat);
    let path = Path2D::from_polyline(&points, closed);
    service(vm).with_rec(|rec| rec.components.insert(Box::new(Reveal::new(path))))
}

fn scene_spawn(vm: &dyn Vm, handles: Vec<Handle>) -> Handle {
    service(vm).with_rec(|rec| {
        let mut components: Components = Vec::with_capacity(handles.len());
        for h in handles {
            // 句柄只能由本 VM 产出；取不到说明装配/复用出错 —— 跳过而不是 panic。
            if let Ok(c) = rec.components.get(&h) {
                components.push(c.clone());
            }
        }
        let handle = rec.encoder.spawn(components);
        rec.objects.insert(handle)
    })
}

fn scene_despawn(vm: &dyn Vm, object: Handle) {
    service(vm).with_rec(|rec| {
        if let Ok(handle) = rec.objects.get(&object).copied() {
            rec.encoder.despawn(handle);
        }
    });
}

fn scene_clip(vm: &dyn Vm, target: String, curve: Handle) -> Handle {
    service(vm).with_rec(|rec| {
        // 句柄只能由本 VM 产出；取不到时退化为常量曲线（动作仍能被记录）。
        let curve = rec
            .curves
            .get(&curve)
            .ok()
            .cloned()
            .unwrap_or(Curve::Const(AnimValue::Scalar(0.0)));
        rec.clips.insert(Clip { target, curve })
    })
}

fn scene_animate(vm: &dyn Vm, object: Handle, clip: Handle, duration: f64) {
    service(vm).with_rec(|rec| {
        let object = rec.objects.get(&object).copied().ok();
        let clip = rec.clips.get(&clip).ok().map(|c| (c.target.clone(), c.curve.clone()));
        let (Some(object), Some((target, curve))) = (object, clip) else {
            return;
        };
        rec.encoder.animate_named(object, &target, curve, duration);
    });
}

fn scene_parallel(vm: &dyn Vm, object: Handle, clip: Handle) {
    service(vm).with_rec(|rec| {
        let object = rec.objects.get(&object).copied().ok();
        let clip = rec.clips.get(&clip).ok().map(|c| (c.target.clone(), c.curve.clone()));
        let (Some(object), Some((target, curve))) = (object, clip) else {
            return;
        };
        rec.encoder.parallel_named(object, &target, curve);
    });
}

fn scene_wait(vm: &dyn Vm, d: f64) {
    service(vm).with_rec(|rec| {
        rec.encoder.wait(d);
    });
}

fn scene_sin(vm: &dyn Vm, base: f32, amp: f32, omega: f32) -> Handle {
    service(vm).with_rec(|rec| rec.curves.insert(Curve::Sin { base, amp, omega }))
}

fn scene_phase(vm: &dyn Vm, omega: f32) -> Handle {
    service(vm).with_rec(|rec| rec.curves.insert(Curve::Phase { omega }))
}

fn scene_constant(vm: &dyn Vm, value: f32) -> Handle {
    service(vm).with_rec(|rec| {
        rec.curves
            .insert(Curve::Const(AnimValue::Scalar(value)))
    })
}

fn scene_ramp(vm: &dyn Vm, from: f32, to: f32, dur: f32) -> Handle {
    let curve = Curve::Keyframes {
        keys: vec![
            (0.0, AnimValue::Scalar(from)),
            (dur as f64, AnimValue::Scalar(to)),
        ],
        interpolation: Interp::Linear,
    };
    service(vm).with_rec(|rec| rec.curves.insert(curve))
}

fn scene_ramp2(vm: &dyn Vm, flat: Vec<f32>, dur: f32) -> Handle {
    let from = Vec2::new(flat.first().copied().unwrap_or(0.0), flat.get(1).copied().unwrap_or(0.0));
    let to = Vec2::new(flat.get(2).copied().unwrap_or(0.0), flat.get(3).copied().unwrap_or(0.0));
    let curve = Curve::Keyframes {
        keys: vec![
            (0.0, AnimValue::Vec2(from)),
            (dur as f64, AnimValue::Vec2(to)),
        ],
        interpolation: Interp::Linear,
    };
    service(vm).with_rec(|rec| rec.curves.insert(curve))
}

/// 扁平点表 → `Vec2` 表（奇数尾巴被忽略；胶水层已做奇偶校验）。
fn points_of(flat: &[f32]) -> Vec<Vec2> {
    flat.chunks_exact(2)
        .map(|p| Vec2::new(p[0], p[1]))
        .collect()
}

/// 校验 target 名字是否存在（供胶水层在 `clip` 之前给出明确错误）。
pub fn target_exists(name: &str) -> bool {
    target_ctor_by_name(name).is_some()
}
