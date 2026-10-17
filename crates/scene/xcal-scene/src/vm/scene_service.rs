// service.rs — 宿主侧 **Scene Service VM**（原生 Rust，注册 typed 函数）。
//
// 函数签名由**结构化类型**构成（`Circle` / `Stroke` / `Curve` / `TargetKind` …）：
// 绑定层按 `TypeSpec` 转换参数，这里只做"结构化值 → 场景原语"的落点转换。
//
// # 组件组（W2）
//
// 一个组件句柄可以对应**多个组件**（`RevealPath` = `Reveal` + `StrokeSolid`），
// `spawn` 时展开 —— 内容路径的样式（颜色 / 线宽）不必在脚本里重述。

use std::any::Any;
use std::cell::RefCell;

use crate::Curve as NativeCurve;
use crate::animation::TargetKind;
use crate::scene::components::Reveal as NativeReveal;
use crate::scene_script::{Handle as ScriptHandle, SceneEncoder, SceneScript};
use xc_notion::{Color32, Vec2 as NotionVec2, Vec3 as NotionVec3};
use xc_object::{
    Arrow as NativeArrow, Axes2D as NativeAxes2D, Circle as NativeCircle, Framing2D,
    Path as NativePath, TickSpec, Transform as NativeTransform,
};
use xc_render_ir::{FillSolid, MeasureSpace, StrokeSolid};
use xc_scene_contract::Components;
use xc_vm::{FunctionId, Handle, HandleTable, QualifiedName, RegistryVm, Vm, VmError, VmId};

use crate::vm::content_service::PathStore;
use crate::vm::types::{
    Arrow, Axes2D, Camera2D, Circle, Curve, Fill, LineStyle, Path, PathShape, Reveal, RevealPath,
    Stroke, Transform,
};

/// 注入时使用的 Service 名（与 `LuaVm::requirements()` 对应）。
pub const SERVICE_NAME: &str = "scene";

/// 内部交接函数：把录制结果取走并重置（不是脚本可见 API）。
pub const FINISH_PATH: &str = "xcal.scene::__finish";
/// 内部交接函数：丢弃上一轮录制状态（每次 `run` 前调用）。
pub const RESET_PATH: &str = "xcal.scene::__reset";

/// 一个"动画动作"：**目标枚举** + 已转换的曲线。
#[derive(Clone)]
struct Clip {
    target: TargetKind,
    curve: NativeCurve,
}

/// Service 的全部可变状态（`Vm` 只有 `&self` ⇒ 内部可变性）。
struct Recording {
    /// 一个句柄 → 一组组件（组件组；`spawn` 时展开）。
    ///
    /// 三张表各自持有**独立的 owner `VmId`**：句柄因此带"种类"，
    /// 把对象句柄当组件传给 `spawn` 会得到 `CrossVmHandle`（明确错误），
    /// 而不是撞上同槽位的组件。
    components: HandleTable<Components>,
    clips: HandleTable<Clip>,
    objects: HandleTable<ScriptHandle>,
    encoder: SceneEncoder,
    /// 装配期错误（非法句柄等）：在 `finish` 时作为 `run` 的明确失败返回。
    error: Option<String>,
}

impl Recording {
    fn new() -> Self {
        Self {
            components: HandleTable::new(VmId::next()),
            clips: HandleTable::new(VmId::next()),
            objects: HandleTable::new(VmId::next()),
            encoder: SceneEncoder::new(),
            error: None,
        }
    }

    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
    }
}

/// 宿主侧场景能力 VM。
pub struct SceneServiceVm {
    id: VmId,
    registry: RegistryVm,
    rec: RefCell<Recording>,
    paths: PathStore,
}

impl Default for SceneServiceVm {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneServiceVm {
    /// 新建（自带独立路径表；需要与内容服务共享时用 [`Self::with_paths`]）。
    pub fn new() -> Self {
        Self::with_paths(PathStore::new())
    }

    /// 与内容服务**共享**路径表（`RevealPath` / `PathShape` 读内容路径）。
    pub fn with_paths(paths: PathStore) -> Self {
        let id = VmId::next();
        let mut registry = RegistryVm::new();

        // ── 组件构造（单结构体参数 ⇒ 自动生成 `Circle { ... }` 等构造器）────
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
        registry.register_fn1_named::<Axes2D, Handle>(
            "xcal.scene::axes2d",
            "xcal.Axes2D",
            "xc_vm.Handle",
            scene_axes2d,
        );
        registry.register_fn1_named::<Arrow, Handle>(
            "xcal.scene::arrow",
            "xcal.Arrow",
            "xc_vm.Handle",
            scene_arrow,
        );
        registry.register_fn1_named::<Fill, Handle>(
            "xcal.scene::fill",
            "xcal.Fill",
            "xc_vm.Handle",
            scene_fill,
        );
        registry.register_fn1_named::<Transform, Handle>(
            "xcal.scene::transform",
            "xcal.Transform",
            "xc_vm.Handle",
            scene_transform,
        );
        registry.register_fn1_named::<RevealPath, Handle>(
            "xcal.scene::reveal_path",
            "xcal.RevealPath",
            "xc_vm.Handle",
            scene_reveal_path,
        );
        registry.register_fn1_named::<PathShape, Handle>(
            "xcal.scene::path_shape",
            "xcal.PathShape",
            "xc_vm.Handle",
            scene_path_shape,
        );

        // ── 对象 ────────────────────────────────────────────────────────
        registry.register_fn1::<Vec<Handle>, Handle>("xcal.scene::spawn", scene_spawn);
        registry.register_fn1::<Handle, ()>("xcal.scene::despawn", scene_despawn);

        // ── 动画：目标枚举 + 曲线枚举 ───────────────────────────────────
        registry.register_fn2::<TargetKind, Curve, Handle>("xcal.scene::clip", scene_clip);
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
        registry.register_erased(FINISH_PATH, &[], "Result<Box<dyn Any>, String>", |vm, _| {
            Ok(Box::new(
                service(vm)
                    .finish()
                    .map(|script| Box::new(script) as Box<dyn Any>),
            ))
        });
        registry.register_erased(RESET_PATH, &[], "()", |vm, _| {
            service(vm).reset();
            Ok(Box::new(()))
        });

        Self {
            id,
            registry,
            rec: RefCell::new(Recording::new()),
            paths,
        }
    }

    /// 取走本轮的 `SceneScript`，并重置状态。
    ///
    /// 装配期错误（非法句柄等）在此变成 `run` 的**明确失败**，
    /// 而不是"少一个组件"的静默结果。
    fn finish(&self) -> Result<SceneScript, String> {
        let mut rec = self.rec.borrow_mut();
        let error = rec.error.take();
        let encoder = std::mem::replace(&mut rec.encoder, SceneEncoder::new());
        let script = encoder.finish();
        self.paths.reset();
        *rec = Recording::new();
        match error {
            Some(message) => Err(message),
            None => Ok(script),
        }
    }

    /// 丢弃当前录制状态。
    fn reset(&self) {
        let mut rec = self.rec.borrow_mut();
        *rec = Recording::new();
        self.paths.reset();
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
            "__finish" => Ok(Box::new(
                self.finish().map(|script| Box::new(script) as Box<dyn Any>),
            )),
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

fn service(vm: &dyn Vm) -> &SceneServiceVm {
    vm.as_any()
        .downcast_ref::<SceneServiceVm>()
        .expect("SceneApi 的调用者必须是 SceneServiceVm")
}

fn color32(color: crate::vm::types::Color) -> Color32 {
    Color32::new(color.r, color.g, color.b, color.a)
}

fn screen_line(style: LineStyle) -> StrokeSolid {
    StrokeSolid::new_in(style.width, color32(style.color), MeasureSpace::Screen)
}

fn world_stroke(path: Path) -> StrokeSolid {
    StrokeSolid::new_in(path.width, color32(path.color), MeasureSpace::World)
}

// ─── typed 函数实现：结构化值 → 场景原语 ─────────────────────────────────

fn scene_circle(vm: &dyn Vm, shape: Circle) -> Handle {
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeCircle::new(
            (shape.x, shape.y),
            shape.r,
        ))])
}

fn scene_stroke(vm: &dyn Vm, style: Stroke) -> Handle {
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(StrokeSolid::new_in(
            style.width,
            color32(style.color),
            MeasureSpace::Screen,
        ))])
}

fn scene_camera2d(vm: &dyn Vm, camera: Camera2D) -> Handle {
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(Framing2D::new(
            NotionVec2::new(camera.center.x, camera.center.y),
            camera.scale,
        ))])
}

fn scene_reveal(vm: &dyn Vm, reveal: Reveal) -> Handle {
    let points: Vec<NotionVec2> = reveal
        .points
        .iter()
        .map(|p| NotionVec2::new(p.x, p.y))
        .collect();
    let path = xc_notion::Path2D::from_polyline(&points, reveal.closed);
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeReveal::new(path))])
}

fn scene_axes2d(vm: &dyn Vm, axes: Axes2D) -> Handle {
    let axes = NativeAxes2D {
        x_range: (f32::NEG_INFINITY, f32::INFINITY),
        y_range: (f32::NEG_INFINITY, f32::INFINITY),
        tick: TickSpec::new(axes.min_px),
        grid_stroke: screen_line(axes.grid),
        axis_stroke: screen_line(axes.axis),
    };
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(axes)])
}

fn scene_arrow(vm: &dyn Vm, arrow: Arrow) -> Handle {
    let arrow = NativeArrow::new(
        (arrow.from.x, arrow.from.y),
        (arrow.to.x, arrow.to.y),
        arrow.head_length,
        arrow.head_width,
    );
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(arrow)])
}

fn scene_fill(vm: &dyn Vm, fill: Fill) -> Handle {
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(FillSolid::new(color32(fill.color)))])
}

fn scene_transform(vm: &dyn Vm, transform: Transform) -> Handle {
    let transform = NativeTransform {
        position: NotionVec3::new(transform.x, transform.y, transform.z),
        ..Default::default()
    };
    service(vm)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(transform)])
}

fn scene_reveal_path(vm: &dyn Vm, shape: RevealPath) -> Handle {
    let this = service(vm);
    let mut rec = this.rec.borrow_mut();
    match this.paths.get(&shape.path.path) {
        Ok(path) => rec.components.insert(vec![
            Box::new(NativeReveal::new(path)),
            Box::new(world_stroke(shape.path)),
        ]),
        Err(e) => {
            rec.fail(format!("内容路径句柄无效：{e}"));
            rec.components.insert(Vec::new())
        }
    }
}

fn scene_path_shape(vm: &dyn Vm, shape: PathShape) -> Handle {
    let this = service(vm);
    let mut rec = this.rec.borrow_mut();
    match this.paths.get(&shape.path.path) {
        Ok(path) => rec.components.insert(vec![
            Box::new(NativePath::new(path)),
            Box::new(world_stroke(shape.path)),
        ]),
        Err(e) => {
            rec.fail(format!("内容路径句柄无效：{e}"));
            rec.components.insert(Vec::new())
        }
    }
}

fn scene_spawn(vm: &dyn Vm, handles: Vec<Handle>) -> Handle {
    let mut rec = service(vm).rec.borrow_mut();
    let mut components: Components = Vec::new();
    for handle in handles {
        match rec.components.get(&handle) {
            Ok(group) => components.extend(group.iter().cloned()),
            Err(_) => rec.fail(format!("组件句柄无效：{handle}")),
        }
    }
    let object = rec.encoder.spawn(components);
    rec.objects.insert(object)
}

fn scene_despawn(vm: &dyn Vm, object: Handle) {
    let mut rec = service(vm).rec.borrow_mut();
    if let Ok(handle) = rec.objects.get(&object).copied() {
        rec.encoder.despawn(handle);
    }
}

fn scene_clip(vm: &dyn Vm, target: TargetKind, curve: Curve) -> Handle {
    service(vm).rec.borrow_mut().clips.insert(Clip {
        target,
        curve: curve.to_native(),
    })
}

fn scene_animate(vm: &dyn Vm, object: Handle, clip: Handle, duration: f64) {
    let mut rec = service(vm).rec.borrow_mut();
    let object = rec.objects.get(&object).copied().ok();
    let clip = rec
        .clips
        .get(&clip)
        .ok()
        .map(|c| (c.target, c.curve.clone()));
    let (Some(object), Some((target, curve))) = (object, clip) else {
        return;
    };
    rec.encoder
        .animate_named(object, target.name(), curve, duration);
}

fn scene_parallel(vm: &dyn Vm, object: Handle, clip: Handle) {
    let mut rec = service(vm).rec.borrow_mut();
    let object = rec.objects.get(&object).copied().ok();
    let clip = rec
        .clips
        .get(&clip)
        .ok()
        .map(|c| (c.target, c.curve.clone()));
    let (Some(object), Some((target, curve))) = (object, clip) else {
        return;
    };
    rec.encoder.parallel_named(object, target.name(), curve);
}

fn scene_wait(vm: &dyn Vm, seconds: f64) {
    service(vm).rec.borrow_mut().encoder.wait(seconds);
}
