// scene_script.rs — 场景脚本（Encoder / Script）：照抄 wgpu 的**两级结构**（P6/B1+B2）。
//
// # 为什么是 wgpu 形态（§1）
//
// ```text
// wgpu                                    本模块
// ────────────────────────────────────────────────────────────
// device.create_command_encoder()    →    SceneEncoder::new()
//   单线程录制、句柄是局部的               SceneEncoder（录制命令，句柄是**局部**的）
// encoder.finish()                   →    SceneEncoder::finish()
//   → CommandBuffer（不可变）              → SceneScript（不可变）
// queue.submit([cmd])                →    SceneScript::commit(&mut scene)
//   提交到已有资源                         提交到**已有 Scene**（不新建）
// ```
//
// 这个模型天然解决三件事：
//
// 1. **句柄** —— Encoder 内是局部下标 [`Handle`]，`commit` 时才映射为 [`ObjectId`]
//    ⇒ 声明期**不需要 Scene、也不需要 ObjectId**；
// 2. **合并** —— `SceneScript` 是命令序列 ⇒ `append` 就是"拼接 + 句柄偏移"（§3.3）；
// 3. **重播** —— Script 可再生（不依赖"状态由 t 唯一决定"，见 B4）。
//
// # 线程安全语义（§1.1）
//
// | 类型 | Send + Sync | 为什么 |
// |---|---|---|
// | `SceneEncoder` | 是（实现上无借用，故顺带 Send） | 录制中；本设计命令**持有**数据（不借用栈） |
// | `SceneScript`  | **是** | 成品、不可变；可跨线程传递、可合并 —— 有编译期断言钉住 |
//
// # 命令的载体：复用既有类型（§1.2）
//
// - 创建载荷直接用契约层既有的 `Components`（`Vec<Box<dyn Any + Send + Sync>>`）；
// - 动画目标用 [`TargetCtor`]（`commit` 时按真实 id 生成 trait object）；
// - 时序复用既有 [`Timeline`]（`play` / `wait` / `parallel` 的待解析目标版本）。
//
// # 句柄引用（"组件里存另一个对象的句柄"）
//
// 契约层的 `Visibility::for_camera` / `Parent` 里的 `ObjectId` 在**声明期**还不存在。
// 这里用 `generation == 0`（`ObjectId` 的**保留值**，真实对象恒 ≥ 1）编码"这是句柄引用"：
// 装配时写 [`handle_ref(h)`](handle_ref)，`commit` / `append` 在[**唯一改写点**](resolve_component_refs)
// 里统一解析 / 偏移。新增"携带句柄引用的组件"只需在那个函数里加一条 —— 不散落。

use std::sync::Arc;

use ze_scene_contract::{
    Components, ObjectId, Parent, Scene as SceneContract, Visibility, VisibilityTag,
};
use ze_time::{ClockForest, Curve};

use crate::animation::binding::BindingTable;
use crate::animation::target::{TargetCtor, target_ctor_by_name};
use crate::animation::timeline::Timeline;

/// **Encoder 内的局部句柄**：声明期索引，`commit` 时才映射到 [`ObjectId`]。
///
/// 与 wgpu 的录制期句柄同构：它只在**一段录制**内有意义，不可跨 Encoder 混用
/// （合并时由 [`SceneScript::append`] 的整体偏移负责对齐）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Handle(pub usize);

/// 声明期"引用另一个 Encoder 句柄"的占位 [`ObjectId`]。
///
/// `generation == 0` 是 `ObjectId` 的**保留值**（`id.rs`：真实对象 `generation` 恒 ≥ 1），
/// 故该编码与任何真实句柄都不冲突；`commit` / `append` 在
/// [`resolve_component_refs`] / [`shift_component_refs`] 处解析 / 偏移。
pub const fn handle_ref(h: Handle) -> ObjectId {
    ObjectId::new(h.0 as u32, 0)
}

/// 该 `ObjectId` 是否为声明期占位引用（见 [`handle_ref`]）。
///
/// `pub`（非 `pub(crate)`）：ECS 侧的 `commit` 用例要断言"解析后不再有占位"。
pub fn is_handle_ref(id: ObjectId) -> bool {
    id.generation == 0
}

/// **句柄改写唯一落点（解析）**：把组件集合里的声明期占位引用换成真实 `ObjectId`。
///
/// 新增"携带句柄引用的组件"时**只改这里**（一次遍历，不散落）。
fn resolve_component_refs(components: &mut Components, resolve: impl Fn(ObjectId) -> ObjectId) {
    for c in components.iter_mut() {
        // 显式到 `dyn Component`（`Box<dyn Component>` 也满足 blanket impl，
        // 直接 `c.as_any_mut()` 会拿到 Box 的 Any）。
        if let Some(v) = (**c).as_any_mut().downcast_mut::<Visibility>() {
            for tag in &mut v.tags {
                if let VisibilityTag::Camera(id) = tag {
                    if is_handle_ref(*id) {
                        *id = resolve(*id);
                    }
                }
            }
        } else if let Some(p) = (**c).as_any_mut().downcast_mut::<Parent>() {
            if is_handle_ref(p.0) {
                p.0 = resolve(p.0);
            }
        }
    }
}

/// **句柄改写唯一落点（偏移）**：合并（[`SceneScript::append`]）时把占位引用整体 `+ base`。
fn shift_component_refs(components: &mut Components, base: usize) {
    for c in components.iter_mut() {
        if let Some(v) = (**c).as_any_mut().downcast_mut::<Visibility>() {
            for tag in &mut v.tags {
                if let VisibilityTag::Camera(id) = tag {
                    if is_handle_ref(*id) {
                        id.index += base as u32;
                    }
                }
            }
        } else if let Some(p) = (**c).as_any_mut().downcast_mut::<Parent>() {
            if is_handle_ref(p.0) {
                p.0.index += base as u32;
            }
        }
    }
}

/// 命令录制器（wgpu 的 `CommandEncoder`）。
///
/// 声明期的一切（创建 / 销毁 / 时序）都只记在**局部下标**上；`finish` 产出不可变的
/// [`SceneScript`]。构造与录制**不需要 `Scene`，也不需要 `ObjectId`**（§7.2.1）。
#[derive(Default)]
pub struct SceneEncoder {
    creates: Vec<Components>,
    despawns: Vec<Handle>,
    timeline: Timeline,
}

impl SceneEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 录一条创建命令，返回**局部句柄**。
    ///
    /// 不接住返回值也可以（与 `Scene::spawn` 一样）—— 后续要引用它就接住。
    pub fn spawn(&mut self, components: Components) -> Handle {
        let h = Handle(self.creates.len());
        self.creates.push(components);
        h
    }

    /// 录一条销毁命令（`commit` 时按映射到的 `ObjectId` 执行）。
    pub fn despawn(&mut self, handle: Handle) {
        self.despawns.push(handle);
    }

    /// 录一段动画：`T` 是目标**类型**（`commit` 时按 `handle` 的真实 id 生成）。
    pub fn animate<T: TargetCtor>(&mut self, handle: Handle, curve: Curve, dur: f64) -> &mut Self {
        self.timeline.play_pending(curve, T::make, handle.0, dur);
        self
    }

    /// 与上一段 `animate` **同区间**的额外动作。
    pub fn parallel<T: TargetCtor>(&mut self, handle: Handle, curve: Curve) -> &mut Self {
        self.timeline.parallel_pending(curve, T::make, handle.0);
        self
    }

    /// 按 target **名字**录一段动画（脚本层入口：target 在 Lua 侧是字符串）。
    ///
    /// 名字 → 构造器复用持久化路径的集中表 [`target_ctor_by_name`]（该表因此获得
    /// 第二个真实消费方）。未知名字 ⇒ 返回 `false`，且**不录制任何东西**。
    pub fn animate_named(&mut self, handle: Handle, target: &str, curve: Curve, dur: f64) -> bool {
        let Some((make, _)) = target_ctor_by_name(target) else {
            return false;
        };
        self.timeline.play_pending(curve, make, handle.0, dur);
        true
    }

    /// [`SceneEncoder::animate_named`] 的"与上一段同区间"版本。
    pub fn parallel_named(&mut self, handle: Handle, target: &str, curve: Curve) -> bool {
        let Some((make, _)) = target_ctor_by_name(target) else {
            return false;
        };
        self.timeline.parallel_pending(curve, make, handle.0);
        true
    }

    /// 按**构造器函数**录一段动画（S3：Transform 通道用）。
    ///
    /// 与 [`SceneEncoder::animate_named`] 的差别只在目标来源：调用方已持有
    /// `fn(ObjectId) -> Arc<dyn Target>`（通道已闭包在函数里），不必再查字符串表。
    pub fn animate_with(
        &mut self,
        handle: Handle,
        make: fn(ObjectId) -> Arc<dyn crate::animation::Target>,
        curve: Curve,
        dur: f64,
    ) -> &mut Self {
        self.timeline.play_pending(curve, make, handle.0, dur);
        self
    }

    /// [`SceneEncoder::animate_with`] 的"与上一段同区间"版本。
    pub fn parallel_with(
        &mut self,
        handle: Handle,
        make: fn(ObjectId) -> Arc<dyn crate::animation::Target>,
        curve: Curve,
    ) -> &mut Self {
        self.timeline.parallel_pending(curve, make, handle.0);
        self
    }

    /// 推进时序游标（不产生节点）。
    pub fn wait(&mut self, d: f64) -> &mut Self {
        self.timeline.wait(d);
        self
    }

    /// 收尾：产出一份不可变的 [`SceneScript`]。
    pub fn finish(self) -> SceneScript {
        SceneScript {
            creates: self.creates,
            despawns: self.despawns,
            timeline: self.timeline,
        }
    }
}

/// 合并（[`SceneScript::append`]）后 `other` 的句柄落位区间。
///
/// `other` 原来的 `Handle(i)` 现在等于 `Handle(range.start + i)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleRange {
    pub start: usize,
    pub len: usize,
}

impl HandleRange {
    /// `other` 的第 `i` 个句柄在本脚本里的新句柄。
    pub fn get(&self, i: usize) -> Option<Handle> {
        (i < self.len).then(|| Handle(self.start + i))
    }
}

/// 脚本的编译产物：时钟树 + 绑定表 + 句柄映射（提交到调度器所需的三样）。
///
/// 与 Scene 的**对象**分开：`commit` 只把对象写进 Scene，时钟树 / 绑定表是 Resource，
/// 由调用方取出后插入（"提交到已有 Scene，不新建"）。
#[derive(Debug)]
pub struct SceneRecipe {
    pub forest: ClockForest,
    pub bindings: BindingTable,
    /// `handles[i]` = 本脚本第 `i` 个 [`Handle`] 对应的 `ObjectId`。
    pub handles: Vec<ObjectId>,
}

impl SceneRecipe {
    /// 句柄 → `ObjectId`（越界 ⇒ `None`）。
    pub fn id(&self, h: Handle) -> Option<ObjectId> {
        self.handles.get(h.0).copied()
    }
}

/// `commit` 的失败语义（§7.1.7）：`Create` 失败 ⇒ **中止整个 commit**，与
/// `spawn_dyn` 既有的"不造半成品"口径一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    /// 第 `handle` 条创建命令被 Scene 拒绝（组件未登记 / 句柄非法）。
    CreateFailed { handle: Handle },
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::CreateFailed { handle } => {
                write!(f, "场景脚本 commit 失败：第 {} 条创建命令被拒绝", handle.0)
            }
        }
    }
}

impl std::error::Error for ScriptError {}

/// 不可变的命令序列（wgpu 的 `CommandBuffer`）。
///
/// - **不可变且可 Clone** ⇒ 可跨线程传递、可合并、可**克隆后重新 commit**；
/// - `commit(&mut scene)` ⇒ **提交到已有 Scene，不新建**；可多次提交到同一场景。
///
/// # `commit` 为什么仍消耗 `self`
///
/// `Scene::spawn_dyn` 按值接收 `Components`。复用同一份脚本靠 **Clone**
/// （`Components` 现在可 Clone），而不是把 `commit` 改成 `&self`——
/// 后者会迫使 `spawn_dyn` 也改成借用，波及既有全部调用点。
#[derive(Clone)]
pub struct SceneScript {
    creates: Vec<Components>,
    despawns: Vec<Handle>,
    timeline: Timeline,
}

impl serde::Serialize for SceneScript {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
        struct Repr<'a> {
            creates: Vec<Vec<crate::persist::TaggedComponent>>,
            despawns: &'a [Handle],
            timeline: &'a Timeline,
        }

        let creates = self
            .creates
            .iter()
            .map(|components| {
                crate::persist::serialize_components(components).map_err(serde::ser::Error::custom)
            })
            .collect::<Result<Vec<_>, S::Error>>()?;

        Repr {
            creates,
            despawns: &self.despawns,
            timeline: &self.timeline,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SceneScript {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Repr {
            creates: Vec<Vec<crate::persist::TaggedComponent>>,
            despawns: Vec<Handle>,
            timeline: Timeline,
        }

        let repr = <Repr as serde::Deserialize>::deserialize(deserializer)?;
        let mut creates = Vec::with_capacity(repr.creates.len());
        for tagged in repr.creates {
            creates.push(
                crate::persist::deserialize_components(tagged).map_err(serde::de::Error::custom)?,
            );
        }
        Ok(Self {
            creates,
            despawns: repr.despawns,
            timeline: repr.timeline,
        })
    }
}

impl SceneScript {
    /// 序列化为 JSON 文本（持久化入口）。
    pub fn to_json(&self) -> Result<String, crate::persist::PersistError> {
        serde_json::to_string(self).map_err(|e| crate::persist::PersistError::Json(e.to_string()))
    }

    /// 从 JSON 文本反序列化。
    ///
    /// 函数类组件（`FunctionGraph`）得到的是**只有名字的壳**，调用前必须按名字
    /// 重新绑定（`ze-notion` 的 `FunctionX2Y::name`）。
    pub fn from_json(text: &str) -> Result<Self, crate::persist::PersistError> {
        serde_json::from_str(text).map_err(|e| crate::persist::PersistError::Json(e.to_string()))
    }
}

impl SceneScript {
    /// 本脚本声明的对象数（句柄数）。
    pub fn len(&self) -> usize {
        self.creates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.creates.is_empty()
    }

    /// 编排总时长（秒）—— 合并时 `other` 的首段起点即落到这里之后（§3.3）。
    pub fn duration(&self) -> f64 {
        self.timeline.total_duration()
    }

    /// **合并**：把 `other` 整体接到本脚本之后（拼接命令序列，§3.3）。
    ///
    /// 三条规则：
    /// 1. **句柄偏移**：`other` 的 `Handle(n)` → `Handle(base + n)`（`base = self.len()`），
    ///    `other` 内部**所有引用**（创建载荷里的占位引用 + 销毁 + 待解析动画目标）同步改写；
    /// 2. **时间游标**：`other` 的时序接在 `self.duration()` 之后；
    /// 3. **返回 [`HandleRange`]**：告诉调用方 `other` 原句柄的新落位。
    ///
    /// 不做并行合并（两段时间轴对齐）—— 需要额外语义，等真实需求（§11.1）。
    pub fn append(&mut self, mut other: SceneScript) -> HandleRange {
        let base = self.creates.len();
        let other_len = other.creates.len();

        // 1. 创建载荷：先偏移 other 的占位引用，再整体搬过来（句柄 = 数组下标 ⇒ 自然对齐）。
        for components in &mut other.creates {
            shift_component_refs(components, base);
        }
        self.creates.append(&mut other.creates);

        // 2. 销毁命令：句柄偏移。
        for h in &mut other.despawns {
            h.0 += base;
        }
        self.despawns.append(&mut other.despawns);

        // 3. 时序：时间游标接续 + 待解析目标句柄偏移。
        self.timeline.append_shifted(&other.timeline, base);

        HandleRange {
            start: base,
            len: other_len,
        }
    }

    /// **提交到已有 Scene，不新建**：按声明顺序 `spawn_dyn` ⇒ 解析引用 ⇒ 编译时序。
    ///
    /// - `Create` 失败 ⇒ 中止并返回 [`ScriptError`]（前面已创建的对象**保留**，
    ///   与 `Scene` 自身的提交粒度一致 —— Scene 无事务）；
    /// - 成功 ⇒ 产出 [`SceneRecipe`]（时钟树 + 绑定表 + 句柄映射），由调用方插入调度器。
    pub fn commit<S: SceneContract>(self, scene: &mut S) -> Result<SceneRecipe, ScriptError> {
        let mut handles: Vec<ObjectId> = Vec::with_capacity(self.creates.len());

        for (i, mut components) in self.creates.into_iter().enumerate() {
            // 声明期占位引用在此解析（引用**先前**创建的对象；未解析的落 `DANGLING`）。
            resolve_component_refs(&mut components, |refid| {
                handles
                    .get(refid.index as usize)
                    .copied()
                    .unwrap_or(ObjectId::DANGLING)
            });
            match scene.spawn_dyn(components) {
                Some(id) => {
                    // **T0-① 不变量防御**：`handle_ref` 用 `generation == 0` 编码"声明期占位
                    // 引用"（见上），其前提是**真实对象的 `generation` 恒 ≥ 1**。该前提若被
                    // 破坏，`is_handle_ref` 会把真实对象误判成占位引用 ⇒ 解析时改写 ⇒ 指向
                    // 错误对象（不报错、不崩溃，只画面错乱）。这里把不变量钉在**解析路径**上。
                    debug_assert!(
                        id.generation >= 1,
                        "真实对象的 generation 必须 ≥ 1（0 被 handle_ref 保留作占位引用）：{id:?}"
                    );
                    handles.push(id)
                }
                None => return Err(ScriptError::CreateFailed { handle: Handle(i) }),
            }
        }

        for h in &self.despawns {
            if let Some(id) = handles.get(h.0) {
                scene.despawn(*id);
            }
        }

        let mut forest = ClockForest::new();
        let mut bindings = BindingTable::new();
        self.timeline.compile_with(
            &|idx| handles.get(idx).copied().unwrap_or(ObjectId::DANGLING),
            &mut forest,
            &mut bindings,
        );

        Ok(SceneRecipe {
            forest,
            bindings,
            handles,
        })
    }
}

impl From<SceneEncoder> for SceneScript {
    fn from(enc: SceneEncoder) -> Self {
        enc.finish()
    }
}
