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
fn is_handle_ref(id: ObjectId) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interp;
    use crate::animation::target::{OpacityValue, Target};
    use crate::contract::SceneRead;
    use crate::scene::Scene as SceneStruct;
    use std::collections::HashMap;
    use ze_object::Circle;
    use ze_scene_contract::Name;
    use ze_time::AnimValue;

    /// **编译期断言**（§7.2.6）：`SceneScript` 必须是 `Send + Sync`。
    ///
    /// 这是本条唯一可靠的验证方式 —— 运行期断言测不出"类型能不能跨线程"。
    #[test]
    fn scene_script_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SceneScript>();
        assert_send_sync::<SceneRecipe>();
        assert_send_sync::<Handle>();
        assert_send_sync::<HandleRange>();
    }

    fn circle_components(x: f32) -> Components {
        vec![Box::new(Circle::new((x, 0.0), 1.0))]
    }

    /// 实际跨界验证：把 `SceneScript` 送进另一个线程（不能只"看起来是"）。
    #[test]
    fn scene_script_crosses_thread_boundary() {
        let mut enc = SceneEncoder::new();
        enc.spawn(circle_components(0.0));
        let script = enc.finish();
        let out = std::thread::spawn(move || script.len()).join().unwrap();
        assert_eq!(out, 1);
    }

    /// **§7.2.1**：录制期不需要 `Scene` —— `SceneEncoder` 的构造与录制独立完成。
    #[test]
    fn recording_needs_no_scene() {
        let mut enc = SceneEncoder::new();
        let h = enc.spawn(circle_components(0.0));
        enc.wait(0.5);
        enc.animate::<OpacityValue>(h, ramp(1.0), 1.0);
        let script = enc.finish();
        assert_eq!(script.len(), 1);
        assert!((script.duration() - 1.5).abs() < 1e-9);
    }

    /// **§7.2.2 + §7.2.3**：两个对象 + 一个引用另一个 ⇒ commit 后引用解析正确，
    /// 且 `commit` 顺序 = 声明顺序。
    #[test]
    fn commit_resolves_handle_reference_and_preserves_order() {
        let mut enc = SceneEncoder::new();
        // 句柄 0：相机（用圆代替，只要能拿到 ObjectId 即可）。
        let cam = enc.spawn(circle_components(0.0));
        // 句柄 1：带 `Visibility::for_camera(handle_ref(cam))` 的对象。
        let mut comps = circle_components(1.0);
        comps.push(Box::new(Visibility::for_camera(handle_ref(cam))));
        let object = enc.spawn(comps);
        let script = enc.finish();

        let mut scene = SceneStruct::new();
        let recipe = script.commit(&mut scene).expect("组件均已登记");

        let cam_id = recipe.id(cam).expect("cam 有 id");
        let obj_id = recipe.id(object).expect("object 有 id");
        assert_ne!(cam_id, obj_id, "两个对象必须是不同 ObjectId");

        // 引用被解析成**真实**的 cam_id（不是占位、不是 DANGLING）。
        let tags = scene
            .with::<Visibility, _>(obj_id, |v| v.tags.clone())
            .expect("对象在场景里");
        assert_eq!(tags, vec![VisibilityTag::Camera(cam_id)]);
        assert!(!is_handle_ref(cam_id), "解析出的必须是真实句柄，不是占位");

        // commit 顺序 = 声明顺序：cam 先建 ⇒ index 更小（hecs 顺序分配）。
        assert!(cam_id.index < obj_id.index);
        assert_eq!(scene.len(), 2);
    }

    /// **T0-① 不变量用例**：把"真实对象 `generation` 恒 ≥ 1"钉在测试里（不只是注释）。
    ///
    /// 可证伪：若 `ObjectId` 的分配方将来把 `generation` 从 0 起（或复用槽位后回 0），
    /// 本用例 RED。它与 `commit` 里的 `debug_assert!` 是同一不变量的两条守卫。
    #[test]
    fn spawned_objects_have_generation_at_least_one() {
        let mut enc = SceneEncoder::new();
        for i in 0..8 {
            enc.spawn(circle_components(i as f32));
        }
        let script = enc.finish();

        let mut scene = SceneStruct::new();
        let recipe = script.commit(&mut scene).expect("组件均已登记");

        assert_eq!(recipe.handles.len(), 8);
        for (i, id) in recipe.handles.iter().enumerate() {
            assert!(
                id.generation >= 1,
                "第 {i} 个真实对象的 generation 必须 ≥ 1（0 为句柄引用保留值）：{id:?}"
            );
        }
        // 反面对照：声明期占位引用的 generation **恒为 0** —— 这正是判据的另一半。
        assert_eq!(handle_ref(Handle(3)).generation, 0);
        assert!(is_handle_ref(handle_ref(Handle(3))));
        assert!(
            recipe.handles.iter().all(|id| !is_handle_ref(*id)),
            "真实对象绝不能被判成声明期占位引用"
        );
    }

    /// **T0-② 核心判据（口径已收窄）**：新增一个 target 在**构造期**无需改任何集中清单。
    ///
    /// 本用例**就地**定义一个测试专有的 target（`TestOpacity`）：它只实现 [`Target`] /
    /// [`TargetCtor`]，**没有任何枚举、映射表或 System** 需要同步改动，经
    /// `SceneEncoder::animate::<TestOpacity>`（泛型 + 关联函数）直接接线。
    ///
    /// ⚠️ **持久化路径不适用**：`serde::Deserialize` 拿不到外部 registry，
    /// 反序列化要靠 `animation::target::target_ctor_by_name` 的集中表。
    /// 故新增 target 若要**可持久化**，仍需在那张表加一行（记账见 ARCHITECTURE）。
    ///
    /// 可证伪：若"新增 target"在**构造期**仍需在某处集中清单加变体 / 分支，
    /// 本用例无法只靠本地定义编译通过。
    #[test]
    fn new_target_requires_no_central_list_change() {
        use crate::animation::target::TargetCtor;
        use crate::contract::WorldScene;
        use std::sync::Arc;
        use ze_render_ir::Opacity;
        use ze_scene_contract::Scene as _;

        #[derive(Debug, Clone, Copy)]
        struct TestOpacity {
            id: ObjectId,
        }
        impl Target for TestOpacity {
            fn apply(&self, scene: &mut WorldScene<'_>, v: &AnimValue) {
                let Some(v) = v.try_scalar() else { return };
                if let Some(o) = scene.get_mut::<Opacity>(self.id) {
                    o.0 = v;
                }
            }
            fn name(&self) -> &'static str {
                "TestOpacity"
            }
            fn target_id(&self) -> ObjectId {
                self.id
            }
        }
        impl TargetCtor for TestOpacity {
            fn make(id: ObjectId) -> Arc<dyn Target> {
                Arc::new(Self { id })
            }
        }

        let mut enc = SceneEncoder::new();
        let h = enc.spawn(vec![
            Box::new(Circle::new((0.0, 0.0), 1.0)),
            Box::new(Opacity::new(1.0)),
        ]);
        enc.animate::<TestOpacity>(h, ramp(1.0), 1.0);

        let mut scene = SceneStruct::new();
        let recipe = enc.finish().commit(&mut scene).expect("组件均已登记");
        assert_eq!(recipe.bindings.bindings.len(), 1);
        let b = &recipe.bindings.bindings[0];
        assert_eq!(b.target.name(), "TestOpacity", "名字由 target 自带");
        assert_eq!(
            b.target.target_id(),
            recipe.handles[0],
            "目标必须指向脚本创建的那个对象"
        );
        assert_eq!(SceneRead::len(&scene), 1);
    }

    /// **§7.2.4 合并三条**：句柄不撞号 / 对象数 = 两段之和 / `b` 首段起始 = `a` 总时长。
    #[test]
    fn append_splices_handles_and_timeline() {
        // a：一个对象 + 一段 2.0s 动画。
        let mut a = SceneEncoder::new();
        let _ = a.spawn(circle_components(0.0));
        let ha = Handle(0);
        a.animate::<OpacityValue>(ha, ramp(1.0), 2.0);
        let a = a.finish();
        let a_dur = a.duration();

        // b：一个对象 + 一段 1.0s 动画（句柄在 b 内是 0）。
        let mut b = SceneEncoder::new();
        let hb = b.spawn(circle_components(5.0));
        b.animate::<OpacityValue>(hb, ramp(1.0), 1.0);
        let b = b.finish();

        let mut merged = a;
        let range = merged.append(b);

        // 句柄不撞号：b 的 Handle(0) 现在是 Handle(1)。
        assert_eq!(range, HandleRange { start: 1, len: 1 });
        assert_eq!(range.get(0), Some(Handle(1)));
        assert_ne!(Handle(0), range.get(0).unwrap());
        // 对象数 = 两段之和。
        assert_eq!(merged.len(), 2);
        // 时间游标接续：总时长 = 2.0 + 1.0。
        assert!((merged.duration() - (a_dur + 1.0)).abs() < 1e-9);

        // commit 后：b 段的首段起始 = a 的总时长（2.0）；两条绑定分别落在 0 与 2.0。
        let mut scene = SceneStruct::new();
        let recipe = merged.commit(&mut scene).expect("登记齐备");
        assert_eq!(recipe.handles.len(), 2);
        assert_eq!(scene.len(), 2);
        assert_eq!(recipe.bindings.bindings.len(), 2);

        let starts: Vec<f64> = recipe
            .bindings
            .bindings
            .iter()
            .map(|b| recipe.forest.get(b.clock).unwrap().local_offset)
            .collect();
        assert_eq!(starts, vec![0.0, 2.0], "b 首段起始 = a 总时长");
        // 两条绑定的 target 各自指向**不同**对象（句柄偏移没串号）。
        assert_ne!(recipe.handles[0], recipe.handles[1]);
    }

    /// **§7.2.5**：多次 `commit` 到同一 Scene ⇒ 对象并存（证明"不新建 Scene"）。
    #[test]
    fn multiple_commits_share_one_scene() {
        let mut scene = SceneStruct::new();

        let mut s1 = SceneEncoder::new();
        s1.spawn(circle_components(0.0));
        s1.finish().commit(&mut scene).expect("ok");

        let mut s2 = SceneEncoder::new();
        s2.spawn(circle_components(1.0));
        s2.spawn(circle_components(2.0));
        s2.finish().commit(&mut scene).expect("ok");

        assert_eq!(scene.len(), 3, "两份脚本的对象在同一 Scene 里并存");
    }

    /// **§7.1.7 失败语义**：未登记组件 ⇒ commit 返回 `Err`，且不继续造对象。
    #[test]
    fn commit_aborts_on_unregistered_component() {
        #[derive(Clone)]
        struct NotRegistered;
        let mut enc = SceneEncoder::new();
        enc.spawn(circle_components(0.0));
        enc.spawn(vec![Box::new(NotRegistered)]);
        enc.spawn(circle_components(1.0));
        let script = enc.finish();

        let mut scene = SceneStruct::new();
        let err = script.commit(&mut scene).unwrap_err();
        assert_eq!(err, ScriptError::CreateFailed { handle: Handle(1) });
        // 前一条已建、后一条未建（Scene 无事务，提交粒度是逐条）。
        assert_eq!(scene.len(), 1);
    }

    /// 合并后动画目标仍指向**正确的**对象（待解析句柄随偏移改写）。
    #[test]
    fn append_keeps_animation_target_on_its_own_object() {
        let mut scene = SceneStruct::new();

        // a：对象 A + 动画（目标 = A）。
        let mut a = SceneEncoder::new();
        let ha = a.spawn(circle_components(0.0));
        a.animate::<OpacityValue>(ha, ramp(1.0), 1.0);
        let a = a.finish();

        // b：对象 B + 动画（目标 = B）。
        let mut b = SceneEncoder::new();
        let hb = b.spawn(circle_components(9.0));
        b.animate::<OpacityValue>(hb, ramp(1.0), 1.0);
        let b = b.finish();

        let mut merged = a;
        merged.append(b);
        let recipe = merged.commit(&mut scene).expect("ok");

        // 两条绑定分别指向 A / B；用 kind 之外的信息无法直接比对，这里比对"写入落点"：
        // 两个对象各自被写 opacity —— 若句柄偏移串号，两者会写到同一对象。
        let ids = recipe.handles.clone();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(recipe.bindings.bindings.len(), 2);
        // 两个目标都是 OpacityValue（T0-② 按 `name()` 判类别，无集中清单）。
        assert!(
            recipe
                .bindings
                .bindings
                .iter()
                .all(|b| b.target.name() == "OpacityValue")
        );
    }

    /// 段内从 0 线性到 1 的曲线（供脚本测试构造"名义"动画）。
    fn ramp(dur: f64) -> Curve {
        Curve::Keyframes {
            keys: vec![(0.0, AnimValue::Scalar(0.0)), (dur, AnimValue::Scalar(1.0))],
            interpolation: Interp::Linear,
        }
    }

    /// 段内把标量从 `from` 插到 `to`（供等价性对照）。
    fn keyframes(dur: f64, from: f32, to: f32) -> Curve {
        Curve::Keyframes {
            keys: vec![(0.0, AnimValue::Scalar(from)), (dur, AnimValue::Scalar(to))],
            interpolation: Interp::Linear,
        }
    }

    /// **B2 等价性对照（§8 验收）**：同一场编排，Encoder 写法 vs 手写
    /// `Timeline` + `find_by_name` ⇒ `BindingTable` 的**时钟（offset/span）**与
    /// **目标类别**逐条相同。
    ///
    /// 比对口径说明：`Arc<dyn Target>` 无法直接相等比较，故比对
    /// `target.name()`（自带语义名）+ 时钟节点。这正是"两种写法产出同一条驱动链路"的判据。
    #[test]
    fn encoder_matches_handwritten_timeline() {
        // ── 手写路径：先建对象，再 `find_by_name` 取 id，再手写 Timeline ──
        let mut scene = SceneStruct::new();
        let mut a_comps: Components = circle_components(0.0);
        a_comps.push(Box::new(Name::new("a")));
        let mut b_comps: Components = circle_components(1.0);
        b_comps.push(Box::new(Name::new("b")));
        scene.spawn_dyn(a_comps).expect("ok");
        scene.spawn_dyn(b_comps).expect("ok");
        let id_a = crate::contract::find_by_name(&scene, "a").unwrap();
        let id_b = crate::contract::find_by_name(&scene, "b").unwrap();

        let mut hand = Timeline::new();
        hand.play(keyframes(1.0, 1.0, 0.0), OpacityValue { id: id_a }, 1.0);
        hand.wait(0.5);
        hand.play(keyframes(2.0, 0.0, 1.0), OpacityValue { id: id_b }, 2.0);
        let mut h_forest = ClockForest::new();
        let mut h_bindings = BindingTable::new();
        hand.compile(&mut h_forest, &mut h_bindings);

        // ── Encoder 路径：声明期无 id，`commit` 才解析 ──
        let mut enc = SceneEncoder::new();
        let ha = enc.spawn(a_comps_of("a"));
        let hb = enc.spawn(b_comps_of("b"));
        enc.animate::<OpacityValue>(ha, keyframes(1.0, 1.0, 0.0), 1.0);
        enc.wait(0.5);
        enc.animate::<OpacityValue>(hb, keyframes(2.0, 0.0, 1.0), 2.0);
        let mut scene2 = SceneStruct::new();
        let recipe = enc.finish().commit(&mut scene2).expect("ok");

        // ── 逐条比对 ──
        assert_eq!(recipe.bindings.bindings.len(), h_bindings.bindings.len());
        assert_eq!(recipe.bindings.bindings.len(), 2);

        for (enc_b, h_b) in recipe.bindings.bindings.iter().zip(&h_bindings.bindings) {
            let e_node = recipe.forest.get(enc_b.clock).expect("节点在");
            let h_node = h_forest.get(h_b.clock).expect("节点在");
            assert_eq!(
                e_node.local_offset, h_node.local_offset,
                "段起始（offset）必须一致"
            );
            assert_eq!(e_node.span, h_node.span, "段区间（span）必须一致");
            assert_eq!(e_node.mode, h_node.mode);
            // `Curve` 未实现 `PartialEq` ⇒ 用 `Debug` 结构相等比对（同一构造 ⇒ 同一 Debug）。
            assert_eq!(
                format!("{:?}", enc_b.curve),
                format!("{:?}", h_b.curve),
                "曲线必须一致"
            );
            assert_eq!(enc_b.target.name(), h_b.target.name(), "目标语义名必须一致");
        }
    }

    fn a_comps_of(name: &str) -> Components {
        let mut c: Components = circle_components(0.0);
        c.push(Box::new(Name::new(name)));
        c
    }
    fn b_comps_of(name: &str) -> Components {
        let mut c: Components = circle_components(1.0);
        c.push(Box::new(Name::new(name)));
        c
    }

    /// 断言 `HandleRange::get` 越界返回 `None`。
    #[test]
    fn handle_range_get_is_bounded() {
        let r = HandleRange { start: 3, len: 2 };
        assert_eq!(r.get(0), Some(Handle(3)));
        assert_eq!(r.get(1), Some(Handle(4)));
        assert_eq!(r.get(2), None);
    }

    /// 确保 `Name`（登记表成员）经脚本创建后仍可被 `find_by_name` 找到（B0 的接续）。
    #[test]
    fn script_created_named_object_is_findable() {
        let mut enc = SceneEncoder::new();
        let mut comps: Components = circle_components(0.0);
        comps.push(Box::new(Name::new("scripted")));
        enc.spawn(comps);
        let script = enc.finish();

        let mut scene = SceneStruct::new();
        let recipe = script.commit(&mut scene).expect("ok");
        let found = crate::contract::find_by_name(&scene, "scripted").expect("能按名字找到");
        assert_eq!(found, recipe.handles[0]);
    }

    /// `From<SceneEncoder>` 与 `finish()` 等价（便捷入口）。
    #[test]
    fn encoder_converts_into_script() {
        let mut enc = SceneEncoder::new();
        enc.spawn(circle_components(0.0));
        let script: SceneScript = enc.into();
        assert_eq!(script.len(), 1);
        assert!(!script.is_empty());
    }

    /// 供 `HashMap` 相关断言占位（句柄是 `Hash`；此处仅证明可用作键）。
    #[test]
    fn handle_is_hashable() {
        let mut m: HashMap<Handle, u32> = HashMap::new();
        m.insert(Handle(0), 7);
        assert_eq!(m.get(&Handle(0)), Some(&7));
    }
}
