// service.rs — 宿主侧 **Scene Service VM**（原生 Rust，注册 typed 函数）。
//
// 函数签名由**结构化类型**构成（`Circle` / `Stroke` / `Curve` …）：
// 绑定层按 `TypeSpec` 转换参数，这里只做"结构化值 → 场景原语"的落点转换。
//
// `SceneApi::signatures()` 与这里注册的完整函数名必须逐字一致（一致性护栏用例）。

use std::any::Any;
use std::cell::RefCell;

use xcal_scene::scene::components::Reveal as NativeReveal;
use xcal_scene::scene_script::{Handle as ScriptHandle, SceneEncoder, SceneScript};
use xcal_scene::Curve as NativeCurve;
use xc_notion::{Color32, Path2D, Vec2 as NotionVec2};
use xc_object::{Circle as NativeCircle, Framing2D};
use xc_render_ir::{MeasureSpace, StrokeSolid};
use xc_scene_contract::{Component, Components};
use xc_vm::{FunctionId, Handle, HandleTable, QualifiedName, RegistryVm, Vm, VmError, VmId};

use crate::types::{Camera2D, Circle, Curve, Reveal, Stroke};

/// 注入时使用的 Service 名（与 `LuaVm::requirements()` 对应）。
pub const SERVICE_NAME: &str = "scene";

/// 内部交接函数：把录制结果取走并重置（不是脚本可见 API）。
pub const FINISH_PATH: &str = "xcal.scene::__finish";
/// 内部交接函数：丢弃上一轮录制状态（每次 `run` 前调用）。
pub const RESET_PATH: &str = "xcal.scene::__reset";

/// 一个"动画动作"：target 名字 + 已转换的曲线。
#[derive(Clone)]
struct Clip {
    target: String,
    curve: NativeCurve,
}

/// Service 的全部可变状态（`Vm` 只有 `&self` ⇒ 内部可变性）。
struct Recording {
    owner: VmId,
    components: HandleTable<Box<dyn Component>>,
    clips: HandleTable<Clip>,
    objects: HandleTable<ScriptHandle>,
    encoder: SceneEncoder,
}

impl Recording {
    fn new(owner: VmId) -> Self {
        Self {
            owner,
            components: HandleTable::new(owner),
            clips: HandleTable::new(owner),
            objects: HandleTable::new(owner),
            encoder: SceneEncoder::new(),
        }
    }
}

/// 宿主侧场景能力 VM。
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

        // ── 组件构造：参数是**结构化组件描述** ────────────────────────────
        registry.register_fn1_named::<Circle, Handle>(
            "xcal.scene::circle",
            "xcal.Circle",
            "xc_vm.Handle",
            scene_circle,
        );
        registry.register_fn1_named::<Stroke, Handle>(
            "xcal.scene::stroke",
            "xcal.Stroke",
            "xc_vm.Handle",
            scene_stroke,
        );
        registry.register_fn1_named::<Camera2D, Handle>(
            "xcal.scene::camera2d",
            "xcal.Camera2D",
            "xc_vm.Handle",
            scene_camera2d,
        );
        registry.register_fn1_named::<Reveal, Handle>(
            "xcal.scene::reveal",
            "xcal.Reveal",
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

        // ── 动画：target 字符串 + 曲线枚举 ──────────────────────────────
        registry.register_fn2::<String, Curve, Handle>("xcal.scene::clip", scene_clip);
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

    /// 取走本轮的 `SceneScript`，并重置状态。
    fn finish(&self) -> SceneScript {
        let mut rec = self.rec.borrow_mut();
        let encoder = std::mem::replace(&mut rec.encoder, SceneEncoder::new());
        let script = encoder.finish();
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

// ─── typed 函数实现：结构化值 → 场景原语 ─────────────────────────────────

fn scene_circle(vm: &dyn Vm, shape: Circle) -> Handle {
    service(vm).with_rec(|rec| {
        rec.components
            .insert(Box::new(NativeCircle::new((shape.x, shape.y), shape.r)))
    })
}

fn scene_stroke(vm: &dyn Vm, style: Stroke) -> Handle {
    let color = Color32::new(style.color.r, style.color.g, style.color.b, style.color.a);
    service(vm).with_rec(|rec| {
        rec.components.insert(Box::new(StrokeSolid::new_in(
            style.width,
            color,
            MeasureSpace::Screen,
        )))
    })
}

fn scene_camera2d(vm: &dyn Vm, camera: Camera2D) -> Handle {
    service(vm).with_rec(|rec| {
        rec.components.insert(Box::new(Framing2D::new(
            NotionVec2::new(camera.center.x, camera.center.y),
            camera.scale,
        )))
    })
}

fn scene_reveal(vm: &dyn Vm, reveal: Reveal) -> Handle {
    let points: Vec<NotionVec2> = reveal
        .points
        .iter()
        .map(|p| NotionVec2::new(p.x, p.y))
        .collect();
    let path = Path2D::from_polyline(&points, reveal.closed);
    service(vm).with_rec(|rec| rec.components.insert(Box::new(NativeReveal::new(path))))
}

fn scene_spawn(vm: &dyn Vm, handles: Vec<Handle>) -> Handle {
    service(vm).with_rec(|rec| {
        let mut components: Components = Vec::with_capacity(handles.len());
        for h in handles {
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

fn scene_clip(vm: &dyn Vm, target: String, curve: Curve) -> Handle {
    service(vm).with_rec(|rec| {
        rec.clips.insert(Clip {
            target,
            curve: curve.to_native(),
        })
    })
}

fn scene_animate(vm: &dyn Vm, object: Handle, clip: Handle, duration: f64) {
    service(vm).with_rec(|rec| {
        let object = rec.objects.get(&object).copied().ok();
        let clip = rec
            .clips
            .get(&clip)
            .ok()
            .map(|c| (c.target.clone(), c.curve.clone()));
        let (Some(object), Some((target, curve))) = (object, clip) else {
            return;
        };
        rec.encoder.animate_named(object, &target, curve, duration);
    });
}

fn scene_parallel(vm: &dyn Vm, object: Handle, clip: Handle) {
    service(vm).with_rec(|rec| {
        let object = rec.objects.get(&object).copied().ok();
        let clip = rec
            .clips
            .get(&clip)
            .ok()
            .map(|c| (c.target.clone(), c.curve.clone()));
        let (Some(object), Some((target, curve))) = (object, clip) else {
            return;
        };
        rec.encoder.parallel_named(object, &target, curve);
    });
}

fn scene_wait(vm: &dyn Vm, seconds: f64) {
    service(vm).with_rec(|rec| {
        rec.encoder.wait(seconds);
    });
}
