// service.rs — 宿主侧 **Scene Service Pod**（原生 Rust，注册 typed 函数）。
//
// 函数签名由**结构化类型**构成（`Circle` / `Stroke` / `Curve` / `TargetKind` …）：
// 绑定层按 `TypeSpec` 转换参数，这里只做"结构化值 → 场景原语"的落点转换。
//
// # 组件组（W2）
//
// 一个组件句柄可以对应**多个组件**（`RevealPath` = `Reveal` + `StrokeSolid`），
// `spawn` 时展开 —— 内容路径的样式（颜色 / 线宽）不必在脚本里重述。

use std::any::Any;
use std::sync::{Mutex, MutexGuard};

use ze_time::Curve as NativeCurve;
use ze_scene_core::animation::TargetKind;
use ze_scene_core::components::Reveal as NativeReveal;
use ze_scene_core::scene_script::{
    Handle as ScriptHandle, SceneEncoder, SceneScript, handle_ref,
};
use ze_notion::{Color32, Quat, Rect2D, Vec2 as NotionVec2, Vec3 as NotionVec3};
use ze_scene_core::object::{
    Arrow as NativeArrow, Axes2D as NativeAxes2D, Camera3D as NativeCamera3D, Circle as NativeCircle,
    Framing2D, Path as NativePath, Projection, TickSpec, Transform as NativeTransform,
};
use ze_pod_core::{FunctionId, Handle, HandleTable, Pod, PodError, PodId, QualifiedName, RegistryPod};
use ze_render_ir::{FillSolid, MeasureSpace, Opacity as NativeOpacity, StrokeSolid};
use ze_scene_contract::{Components, Name as NativeName, Parent as NativeParent};

use crate::content_service::PathStore;
use crate::types::{
    Arrow, Axes2D, Camera2D, Camera3D, Circle, Cube, Curve, Fill, Light, LightKind, LineStyle,
    Material3D, Name, Opacity, Parent, Path, PathShape, Polyline, Reveal, RevealPath, ShaderVariant,
    Stroke, Surface, SurfaceMapping, SurfaceRenderer, Texture, TextureFilter, Transform, Transform3D,
    TransformChannel,
};
use ze_scene_core::components::{Light as NativeLight, Material3D as NativeMaterial3D};
use ze_scene_core::surface::{
    Surface as NativeSurface, SurfaceMapping as NativeSurfaceMapping,
    SurfaceRenderer as NativeSurfaceRenderer, SurfaceTexture as NativeSurfaceTexture,
    SurfaceTextureRef, TextureFilter as NativeTextureFilter,
};
use ze_render_ir::ShaderVariant as NativeShaderVariant;

/// 注入时使用的 Service 名（与 `LuaPod::requirements()` 对应）。
pub const SERVICE_NAME: &str = "scene";
/// Service 版本（写入 ZSB Meta，用于可还原性检查）。
pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 内部交接函数：把录制结果取走并重置（不是脚本可见 API）。
pub const FINISH_PATH: &str = "zela.scene::__finish";
/// 内部交接函数：丢弃上一轮录制状态（每次 `run` 前调用）。
pub const RESET_PATH: &str = "zela.scene::__reset";

/// 一个"动画动作"：**目标**（枚举或 Transform 通道）+ 已转换的曲线。
#[derive(Clone)]
struct Clip {
    target: ClipTarget,
    curve: NativeCurve,
}

/// 脚本侧动作目标的两种来源。
#[derive(Clone, Copy)]
enum ClipTarget {
    /// 既有集中枚举（CircleRadius / Phase / CameraEye / MorphT / Camera2DFraming /
    /// RevealRange / OpacityValue）。
    Kind(TargetKind),
    /// S3：显式 Transform 通道（B7 起没有 `Auto`）。
    Transform(TransformChannel),
}

impl ClipTarget {
    fn make(self) -> fn(ze_scene_contract::ObjectId) -> std::sync::Arc<dyn ze_scene_core::animation::Target> {
        use ze_scene_core::animation::target::{
            make_transform_position, make_transform_rotation, make_transform_scale,
        };
        match self {
            ClipTarget::Kind(kind) => kind.ctor(),
            ClipTarget::Transform(channel) => match channel {
                TransformChannel::Position => make_transform_position,
                TransformChannel::Rotation => make_transform_rotation,
                TransformChannel::Scale => make_transform_scale,
            },
        }
    }
}

/// Service 的全部可变状态（`Pod` 只有 `&self` ⇒ 内部可变性）。
struct Recording {
    /// 一个句柄 → 一组组件（组件组；`spawn` 时展开）。
    ///
    /// 三张表各自持有**独立的 owner `PodId`**：句柄因此带"种类"，
    /// 把对象句柄当组件传给 `spawn` 会得到 `CrossPodHandle`（明确错误），
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
            components: HandleTable::new(PodId::next()),
            clips: HandleTable::new(PodId::next()),
            objects: HandleTable::new(PodId::next()),
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

/// 录制状态的**线程安全**容器（P1：让 `SceneServicePod` 登记 `SyncPod` 徽章）。
///
/// 保留原 `borrow_mut()` 的调用形态，内部是 `Mutex` —— 调用点无需改动；
/// 锁只在装配期（脚本执行）使用，不在渲染热路径上。
struct RecordingCell(Mutex<Recording>);

impl RecordingCell {
    fn new(recording: Recording) -> Self {
        Self(Mutex::new(recording))
    }

    fn borrow_mut(&self) -> MutexGuard<'_, Recording> {
        self.0.lock().expect("场景录制状态锁中毒")
    }
}

/// 宿主侧场景能力 Pod。
pub struct SceneServicePod {
    id: PodId,
    registry: RegistryPod,
    rec: RecordingCell,
    paths: PathStore,
}

impl Default for SceneServicePod {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneServicePod {
    /// 新建（自带独立路径表；需要与内容服务共享时用 [`Self::with_paths`]）。
    pub fn new() -> Self {
        Self::with_paths(PathStore::new())
    }

    /// 与内容服务**共享**路径表（`RevealPath` / `PathShape` 读内容路径）。
    pub fn with_paths(paths: PathStore) -> Self {
        let id = PodId::next();
        let mut registry = RegistryPod::new();

        // ── 组件构造（单结构体参数 ⇒ 自动生成 `Circle { ... }` 等构造器）────
        registry.register_fn1_named::<Circle, Handle>(
            "zela.scene::circle",
            "zela.Circle",
            "ze_pod_core.Handle",
            scene_circle,
        );
        registry.register_fn1_named::<Stroke, Handle>(
            "zela.scene::stroke",
            "zela.Stroke",
            "ze_pod_core.Handle",
            scene_stroke,
        );
        registry.register_fn1_named::<Camera2D, Handle>(
            "zela.scene::camera2d",
            "zela.Camera2D",
            "ze_pod_core.Handle",
            scene_camera2d,
        );
        // ── 3D（B1-D）──────────────────────────────────────────────────
        registry.register_fn1_named::<Camera3D, Handle>(
            "zela.scene::camera3d",
            "zela.Camera3D",
            "ze_pod_core.Handle",
            scene_camera3d,
        );
        registry.register_fn1_named::<Cube, Handle>(
            "zela.scene::cube",
            "zela.Cube",
            "ze_pod_core.Handle",
            scene_cube,
        );
        registry.register_fn1_named::<Material3D, Handle>(
            "zela.scene::material3d",
            "zela.Material3D",
            "ze_pod_core.Handle",
            scene_material3d,
        );
        registry.register_fn1_named::<Light, Handle>(
            "zela.scene::light",
            "zela.Light",
            "ze_pod_core.Handle",
            scene_light,
        );
        // B3 Step 0：Surface 组件组（Surface + Renderer + Texture）。
        registry.register_fn1_named::<Surface, Handle>(
            "zela.scene::surface",
            "zela.Surface",
            "ze_pod_core.Handle",
            scene_surface,
        );
        registry.register_fn1_named::<Transform3D, Handle>(
            "zela.scene::transform3d",
            "zela.Transform3D",
            "ze_pod_core.Handle",
            scene_transform3d,
        );
        registry.register_fn1_named::<Reveal, Handle>(
            "zela.scene::reveal",
            "zela.Reveal",
            "ze_pod_core.Handle",
            scene_reveal,
        );
        registry.register_fn1_named::<Polyline, Handle>(
            "zela.scene::polyline",
            "zela.Polyline",
            "ze_pod_core.Handle",
            scene_polyline,
        );
        registry.register_fn1::<Parent, Handle>("zela.scene::parent", scene_parent);
        registry.register_fn1::<Name, Handle>("zela.scene::name", scene_name);
        registry.register_fn1::<Opacity, Handle>("zela.scene::opacity", scene_opacity);
        registry.register_fn1_named::<Axes2D, Handle>(
            "zela.scene::axes2d",
            "zela.Axes2D",
            "ze_pod_core.Handle",
            scene_axes2d,
        );
        registry.register_fn1_named::<Arrow, Handle>(
            "zela.scene::arrow",
            "zela.Arrow",
            "ze_pod_core.Handle",
            scene_arrow,
        );
        registry.register_fn1_named::<Fill, Handle>(
            "zela.scene::fill",
            "zela.Fill",
            "ze_pod_core.Handle",
            scene_fill,
        );
        registry.register_fn1_named::<Transform, Handle>(
            "zela.scene::transform",
            "zela.Transform",
            "ze_pod_core.Handle",
            scene_transform,
        );
        registry.register_fn1_named::<RevealPath, Handle>(
            "zela.scene::reveal_path",
            "zela.RevealPath",
            "ze_pod_core.Handle",
            scene_reveal_path,
        );
        registry.register_fn1_named::<PathShape, Handle>(
            "zela.scene::path_shape",
            "zela.PathShape",
            "ze_pod_core.Handle",
            scene_path_shape,
        );

        // ── 对象 ────────────────────────────────────────────────────────
        registry.register_fn1::<Vec<Handle>, Handle>("zela.scene::spawn", scene_spawn);
        registry.register_fn1::<Handle, ()>("zela.scene::despawn", scene_despawn);

        // ── 动画：目标枚举 + 曲线枚举 ───────────────────────────────────
        registry.register_fn2::<TargetKind, Curve, Handle>("zela.scene::clip", scene_clip);
        registry.register_fn2::<TransformChannel, Curve, Handle>(
            "zela.scene::clip_transform",
            scene_clip_transform,
        );
        registry.register_fn3_named::<Handle, Handle, f64, ()>(
            "zela.scene::animate",
            "ze_pod_core.Handle",
            "ze_pod_core.Handle",
            "f64",
            "()",
            scene_animate,
        );
        registry.register_fn2::<Handle, Handle, ()>("zela.scene::parallel", scene_parallel);
        registry.register_fn1::<f64, ()>("zela.scene::wait", scene_wait);

        // ── 内部交接（不是脚本可见 API；走 erased / `Pod::call`）────────────
        registry.register_erased(
            FINISH_PATH,
            &[],
            "Result<Box<dyn Any>, String>",
            |pod, _| {
                Ok(Box::new(
                    service(pod)
                        .finish()
                        .map(|script| Box::new(script) as Box<dyn Any>),
                ))
            },
        );
        registry.register_erased(RESET_PATH, &[], "()", |pod, _| {
            service(pod).reset();
            Ok(Box::new(()))
        });

        Self {
            id,
            registry,
            rec: RecordingCell::new(Recording::new()),
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

impl Pod for SceneServicePod {
    fn id(&self) -> PodId {
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

    fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        let name = self
            .functions()
            .get(id.get() as usize)
            .ok_or_else(|| PodError::UnknownFunction(format!("函数编号 {id}")))?;
        match name.name() {
            "__finish" => Ok(Box::new(
                self.finish().map(|script| Box::new(script) as Box<dyn Any>),
            )),
            "__reset" => {
                self.reset();
                Ok(Box::new(()))
            }
            other => Err(PodError::CallFailed(format!(
                "SceneServicePod 的 `{other}` 只支持 typed 快路径（脚本经 SceneApi 调用）"
            ))),
        }
    }
}

fn service(pod: &dyn Pod) -> &SceneServicePod {
    pod.as_any()
        .downcast_ref::<SceneServicePod>()
        .expect("SceneApi 的调用者必须是 SceneServicePod")
}

fn color32(color: crate::types::Color) -> Color32 {
    Color32::new(color.r, color.g, color.b, color.a)
}

fn screen_line(style: LineStyle) -> StrokeSolid {
    StrokeSolid::new_in(style.width, color32(style.color), MeasureSpace::Screen)
}

fn world_stroke(path: Path) -> StrokeSolid {
    StrokeSolid::new_in(path.width, color32(path.color), MeasureSpace::World)
}

// ─── typed 函数实现：结构化值 → 场景原语 ─────────────────────────────────

fn scene_circle(pod: &dyn Pod, shape: Circle) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeCircle::new(
            (shape.x, shape.y),
            shape.r,
        ))])
}

fn scene_stroke(pod: &dyn Pod, style: Stroke) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(StrokeSolid::new_in(
            style.width,
            color32(style.color),
            MeasureSpace::Screen,
        ))])
}

fn scene_camera2d(pod: &dyn Pod, camera: Camera2D) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(Framing2D::new(
            NotionVec2::new(camera.center.x, camera.center.y),
            camera.scale,
        ))])
}

// ─── 3D（B1-D）：结构化值 → 场景组件 ──────────────────────────────────

fn scene_camera3d(pod: &dyn Pod, camera: Camera3D) -> Handle {
    // `aspect` 固定 16:9：渲染时被视口宽高比覆盖（`view_projection(viewport.aspect())`）。
    let native = NativeCamera3D::new(
        NotionVec3::new(camera.eye.x, camera.eye.y, camera.eye.z),
        NotionVec3::new(camera.target.x, camera.target.y, camera.target.z),
        NotionVec3::new(camera.up.x, camera.up.y, camera.up.z),
        Projection::perspective(camera.fov_y, 16.0 / 9.0, 0.1, 100.0),
    );
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(native)])
}

fn scene_cube(pod: &dyn Pod, cube: Cube) -> Handle {
    // **参数化构造器**：顶点由宿主生成（B1-D 硬约束），脚本不传顶点数组。
    let mesh = std::sync::Arc::new(if cube.solid {
        ze_scene_core::mesh_primitives::solid_cube(cube.half_extent)
    } else {
        ze_scene_core::mesh_primitives::wireframe_cube(cube.half_extent)
    });
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(mesh)])
}

fn scene_material3d(pod: &dyn Pod, material: Material3D) -> Handle {
    let this = service(pod);
    let mut rec = this.rec.borrow_mut();
    let mut native = NativeMaterial3D::color(color32(material.color));
    native.variant = match material.variant {
        ShaderVariant::Flat => NativeShaderVariant::Flat,
        ShaderVariant::Lambert => NativeShaderVariant::Lambert,
        ShaderVariant::Pbr => NativeShaderVariant::Pbr,
    };
    native.metallic = material.metallic;
    native.roughness = material.roughness;
    // B2-7（D19）：意图层字段原样透传；后端负责翻译成 cull_mode / blend。
    native.double_sided = material.double_sided;
    native.transparent = material.transparent;
    // B3 Step 0：纹理引用以**组件**承载（`SurfaceTextureRef` 的占位句柄在 commit 解析），
    // 材质本身不接触 ObjectId / u64 句柄。
    let mut components: Vec<Box<dyn ze_scene_contract::Component>> = vec![Box::new(native)];
    match material.texture {
        Texture::None => {}
        Texture::Surface { surface } => match rec.objects.get(&surface) {
            Ok(script) => components.push(Box::new(SurfaceTextureRef(handle_ref(*script)))),
            Err(error) => rec.fail(format!("材质引用的 Surface 句柄无效：{error:?}")),
        },
    }
    rec.components.insert(components)
}

/// `zela.Surface` → `Surface + SurfaceRenderer + SurfaceTexture` 组件组。
fn scene_surface(pod: &dyn Pod, desc: Surface) -> Handle {
    let mapping = match desc.mapping {
        SurfaceMapping::World => NativeSurfaceMapping::World,
        SurfaceMapping::Screen => NativeSurfaceMapping::Screen,
    };
    let renderer = match desc.renderer {
        SurfaceRenderer::Renderer2D => NativeSurfaceRenderer::Renderer2D,
        SurfaceRenderer::Renderer3D => NativeSurfaceRenderer::Renderer3D,
    };
    let filter = match desc.filter {
        TextureFilter::Nearest => NativeTextureFilter::Nearest,
        TextureFilter::Linear => NativeTextureFilter::Linear,
    };
    let surface = NativeSurface::new(
        Rect2D::new(
            NotionVec2::new(desc.min_x, desc.min_y),
            NotionVec2::new(desc.max_x, desc.max_y),
        ),
        mapping,
        NotionVec2::new(desc.anchor_x, desc.anchor_y),
    );
    let mut texture = NativeSurfaceTexture::new(
        [desc.texel_width, desc.texel_height],
        filter,
        desc.mipmap,
    )
    .with_composite(desc.composite);
    if desc.fit_margin > 0.0 {
        texture = texture.with_fit_margin(desc.fit_margin);
    }
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![
            Box::new(surface),
            Box::new(renderer),
            Box::new(texture),
        ])
}

fn scene_light(pod: &dyn Pod, light: Light) -> Handle {
    let color = color32(light.color);
    let native = match light.kind {
        LightKind::Directional { direction } => NativeLight::directional_with(
            NotionVec3::new(direction.x, direction.y, direction.z),
            color,
            light.intensity,
        ),
        LightKind::Point { range } => NativeLight::point_with(range, color, light.intensity),
    };
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(native)])
}

/// `Transform3D` → 原生 `Transform`（同一摆放组件，单一真源）。
fn scene_transform3d(pod: &dyn Pod, transform: Transform3D) -> Handle {
    let native = NativeTransform {
        position: NotionVec3::new(
            transform.position.x,
            transform.position.y,
            transform.position.z,
        ),
        // pod 字段顺序即 glam 四元数 xyzw。
        rotation: Quat::from_xyzw(
            transform.rotation.x,
            transform.rotation.y,
            transform.rotation.z,
            transform.rotation.w,
        ),
        scale: NotionVec3::new(transform.scale.x, transform.scale.y, transform.scale.z),
    };
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(native)])
}

fn scene_reveal(pod: &dyn Pod, reveal: Reveal) -> Handle {
    let points: Vec<NotionVec2> = reveal
        .points
        .iter()
        .map(|p| NotionVec2::new(p.x, p.y))
        .collect();
    let path = ze_notion::Path2D::from_polyline(&points, reveal.closed);
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeReveal::new(path))])
}

fn scene_polyline(pod: &dyn Pod, polyline: Polyline) -> Handle {
    let points: Vec<NotionVec2> = polyline
        .points
        .iter()
        .map(|p| NotionVec2::new(p.x, p.y))
        .collect();
    let path = ze_notion::Path2D::from_polyline(&points, polyline.closed);
    let mut reveal = NativeReveal::new(path);
    // S2 / G13：静态折线**可见初态**。仍可再挂 `RevealRange` 做二次书写。
    reveal.start = 0.0;
    reveal.end = 1.0;
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(reveal)])
}

fn scene_parent(pod: &dyn Pod, parent: Parent) -> Handle {
    let mut rec = service(pod).rec.borrow_mut();
    // 脚本句柄 → Encoder 局部句柄（`handle_ref` 的编码空间）；非法句柄明确失败。
    let resolved = match rec.objects.get(&parent.parent) {
        Ok(handle) => Some(*handle),
        Err(_) => {
            rec.fail(format!("父对象句柄无效：{}", parent.parent));
            None
        }
    };
    match resolved {
        Some(handle) => rec
            .components
            .insert(vec![Box::new(NativeParent(handle_ref(handle)))]),
        None => rec.components.insert(Vec::new()),
    }
}

fn scene_name(pod: &dyn Pod, name: Name) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeName(name.value))])
}

fn scene_opacity(pod: &dyn Pod, opacity: Opacity) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(NativeOpacity::new(opacity.value))])
}

fn scene_axes2d(pod: &dyn Pod, axes: Axes2D) -> Handle {
    let axes = NativeAxes2D {
        x_range: (f32::NEG_INFINITY, f32::INFINITY),
        y_range: (f32::NEG_INFINITY, f32::INFINITY),
        tick: TickSpec::new(axes.min_px),
        grid_stroke: screen_line(axes.grid),
        axis_stroke: screen_line(axes.axis),
    };
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(axes)])
}

fn scene_arrow(pod: &dyn Pod, arrow: Arrow) -> Handle {
    let arrow = NativeArrow::new(
        (arrow.from.x, arrow.from.y),
        (arrow.to.x, arrow.to.y),
        arrow.head_length,
        arrow.head_width,
    );
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(arrow)])
}

fn scene_fill(pod: &dyn Pod, fill: Fill) -> Handle {
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(FillSolid::new(color32(fill.color)))])
}

fn scene_transform(pod: &dyn Pod, transform: Transform) -> Handle {
    let transform = NativeTransform {
        position: NotionVec3::new(transform.x, transform.y, transform.z),
        ..Default::default()
    };
    service(pod)
        .rec
        .borrow_mut()
        .components
        .insert(vec![Box::new(transform)])
}

fn scene_reveal_path(pod: &dyn Pod, shape: RevealPath) -> Handle {
    let this = service(pod);
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

fn scene_path_shape(pod: &dyn Pod, shape: PathShape) -> Handle {
    let this = service(pod);
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

fn scene_spawn(pod: &dyn Pod, handles: Vec<Handle>) -> Handle {
    let mut rec = service(pod).rec.borrow_mut();
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

fn scene_despawn(pod: &dyn Pod, object: Handle) {
    let mut rec = service(pod).rec.borrow_mut();
    if let Ok(handle) = rec.objects.get(&object).copied() {
        rec.encoder.despawn(handle);
    }
}

fn scene_clip(pod: &dyn Pod, target: TargetKind, curve: Curve) -> Handle {
    service(pod).rec.borrow_mut().clips.insert(Clip {
        target: ClipTarget::Kind(target),
        curve: curve.to_native(),
    })
}

fn scene_clip_transform(pod: &dyn Pod, channel: TransformChannel, curve: Curve) -> Handle {
    service(pod).rec.borrow_mut().clips.insert(Clip {
        target: ClipTarget::Transform(channel),
        curve: curve.to_native(),
    })
}

fn scene_animate(pod: &dyn Pod, object: Handle, clip: Handle, duration: f64) {
    let mut rec = service(pod).rec.borrow_mut();
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
        .animate_with(object, target.make(), curve, duration);
}

fn scene_parallel(pod: &dyn Pod, object: Handle, clip: Handle) {
    let mut rec = service(pod).rec.borrow_mut();
    let object = rec.objects.get(&object).copied().ok();
    let clip = rec
        .clips
        .get(&clip)
        .ok()
        .map(|c| (c.target, c.curve.clone()));
    let (Some(object), Some((target, curve))) = (object, clip) else {
        return;
    };
    rec.encoder.parallel_with(object, target.make(), curve);
}

fn scene_wait(pod: &dyn Pod, seconds: f64) {
    service(pod).rec.borrow_mut().encoder.wait(seconds);
}
