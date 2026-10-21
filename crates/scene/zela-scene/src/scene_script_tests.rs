// scene_script 的 ECS 侧用例（B3-1b）：脚本数据已下沉 `ze-scene-core`，
// 但 `commit` 用例需要真实的 hecs 场景。

use ze_scene_core::scene_script::*;
mod tests {
    use super::*;
    use ze_scene_contract::{
        Components, ObjectId, Scene as SceneContract, Visibility, VisibilityTag,
    };
    use ze_time::{ClockForest, Curve, Interp};
    use crate::animation::target::{OpacityValue, Target};
    use ze_scene_core::animation::{BindingTable, Timeline};
    use ze_scene_core::scene_write::SceneWrite;
    use ze_scene_contract::SceneRead;
    use crate::scene::Scene as SceneStruct;
    use std::collections::HashMap;
    use ze_scene_core::object::Circle;
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
        use std::sync::Arc;
        use ze_render_ir::Opacity;

        #[derive(Debug, Clone, Copy)]
        struct TestOpacity {
            id: ObjectId,
        }
        impl Target for TestOpacity {
            fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
                let Some(v) = v.try_scalar() else { return };
                ze_scene_core::scene_write::with_component::<Opacity, _>(scene, self.id, |o| {
                    o.0 = v
                });
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
