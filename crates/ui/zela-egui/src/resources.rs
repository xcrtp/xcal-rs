// resources.rs: egui 层的全局 Resource 类型
//
// B 阶段（UI 解耦）后本文件不得出现任何 egui 类型。
//
// A4 后：`UiConfig` / `ViewportCamera` 的**定义**已归位 `zela_scene_ecs::domain`
// （`CameraDriveSystem` 是场景 system，必须能读到这两个开关；场景不得反向依赖 UI）。
// 本层只保留 re-export 与"依赖渲染层的派生函数"（`post_filters`）。
//
// P1-1（相机归位）：`AppCamera` **已删除** —— 相机状态是**场景对象**（持 `Framing2D`，
// 见 `ze-object`），UI 只发 `CameraCommand`（控制器纯函数在 `zela_scene_ecs::camera_input`），
// 场景里的目标相机由 `Camera2DHandle`（引用型句柄）指认。
// 本层不再持有任何相机状态。
//
// P4：`ViewRect` 已随三个录制 System 下沉到渲染层（`zela_render::ViewRect`）——
// 录制 System 不得读 UI 定义的类型；本层宿主只负责每帧把布局结果**写入**它。

// ─── TimeState ─────────────────────────────────────────────────────────────

pub use zela_scene_ecs::TimeState;

// ─── UiConfig / ViewportCamera（定义在场景 domain）──────────────────────────

pub use zela_scene_ecs::domain::{UiConfig, ViewportCamera};

use zela_render::{FilterSlot, PostFilterChain};

/// 由面板开关构造**合成后滤镜链**（C 线）。
///
/// 顺序固定为 点运算 → 几何重映射 → 邻域（先调色、再位移、最后模糊）——
/// 这也是"滤镜链是数据"的直接体现：面板改状态 ⇒ 链变 ⇒ 无需碰管线代码。
/// 三个开关全关 ⇒ **空链** ⇒ 渲染层走直通路径（画面逐位不变）。
pub fn post_filters(cfg: &UiConfig) -> PostFilterChain {
    let p = cfg.filter_param.clamp(0.0, 1.0);
    let mut chain = PostFilterChain::new();
    if cfg.filter_gain {
        // 1.0 → 1.0（恒等）、0.0 → 0.5（压暗一半）。
        chain.push(FilterSlot::brightness(1.0 - 0.5 * p));
    }
    if cfg.filter_distortion {
        // 桶形畸变：强度 0 时恒等。
        chain.push(FilterSlot::distortion(1.5 * p, 0.2 * p, 1.0));
    }
    if cfg.filter_blur {
        chain.push(FilterSlot::blur(p));
    }
    chain
}

// ─── ViewRect ──────────────────────────────────────────────────────────────
//
// 见文件头：`ViewRect` 已下沉到 `zela_render::ViewRect`（P4），本层不再定义。

#[cfg(test)]
mod tests {
    use super::*;
    use zela_schedule::resource::Resources;

    #[test]
    fn test_ui_config_default() {
        let cfg = UiConfig::default();
        assert!(cfg.show_grid);
        assert!(!cfg.show_field);
        // 两个图层默认都开（关掉即"关掉对应相机"）。
        assert!(cfg.show_2d);
        assert!(cfg.show_3d);
        // S3-2：默认保持声明顺序（不翻转）。
        assert!(!cfg.flip_layer_order);
    }

    #[test]
    fn test_resources_remove() {
        let mut r = Resources::new();
        r.insert(42_i32);
        assert!(r.get::<i32>().is_some());
        let removed = r.remove::<i32>();
        assert_eq!(removed, Some(42));
        assert!(r.get::<i32>().is_none());
    }

    /// 三个滤镜全关 ⇒ 空链（渲染层直通）。
    #[test]
    fn post_filters_empty_when_all_off() {
        assert!(post_filters(&UiConfig::default()).is_empty());
    }
}
