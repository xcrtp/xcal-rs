// resources.rs: egui 层的全局 Resource 类型
//
// B 阶段（UI 解耦）后本文件不得出现任何 egui 类型。
//
// P1-1（相机归位）：`AppCamera` **已删除** —— 相机状态是**场景对象**（持 `Framing2D`，
// 见 `xc-object`），UI 只发 `CameraCommand`（控制器纯函数在 `xcal_scene::camera_input`），
// 场景里的目标相机由 `Camera2DHandle`（引用型句柄）指认。
// 本层不再持有任何相机状态。
//
// P4：`ViewRect` 已随三个录制 System 下沉到渲染层（`xcal_render::ViewRect`）——
// 录制 System 不得读 UI 定义的类型；本层宿主只负责每帧把布局结果**写入**它。

// ─── TimeState ─────────────────────────────────────────────────────────────

pub use xcal_scene::TimeState;

// ─── UiConfig ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub show_grid: bool,
    pub show_field: bool,
    /// **2D 图层开关**：关闭即"关掉 2D 相机" —— `build_scene_view` 不再声明
    /// `Viewport2D` 图层（网格 / 曲线 / 公式随之全部不画）。
    pub show_2d: bool,
    /// **3D 图层开关**：关闭即"关掉 3D 相机"（线框立方体与曲面不画）。
    pub show_3d: bool,
    /// S3-2 验收开关：**声明的图层顺序**（`false`）= 先声明的在**下**；
    /// 翻转后 = 先声明的在**上**（2D 盖住 3D）。
    ///
    /// 顺序的语义来源是 `SceneView::layers` 的索引序，本开关只做展示期翻转，
    /// 不改动场景声明（用于肉眼验证"叠放顺序确实由声明决定"）。
    pub flip_layer_order: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_grid: true,
            show_field: false,
            show_2d: true,
            show_3d: true,
            flip_layer_order: false,
        }
    }
}

// ─── ViewRect ──────────────────────────────────────────────────────────────
//
// 见文件头：`ViewRect` 已下沉到 `xcal_render::ViewRect`（P4），本层不再定义。

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_schedule::resource::Resources;

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
}
