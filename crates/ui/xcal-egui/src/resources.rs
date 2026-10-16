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

use xcal_render::{FilterSlot, PostFilterChain};

/// **视口操作目标**（K3）：中央视口里 2D / 3D 图层同屏合成、占据同一矩形，
/// "在哪层上拖拽"无法从坐标区分 ⇒ 由本开关显式选择"拖拽/滚轮操作哪台相机"。
///
/// 它同时决定 **3D 相机的模式**（`CameraMode`）与**自动轨道动画是否让位**：
/// - [`TwoD`](ViewportCamera::TwoD)：操作 2D 相机；3D 相机处于 `Path`（动画驱动）
///   —— 自动轨道绑定生效（与改动前逐位一致）。
/// - [`ThreeDOrbit`](ViewportCamera::ThreeDOrbit)：操作 3D 相机（轨道模式）；
///   自动轨道绑定**让位**（否则它每帧写 `eye`，把交互结果立刻覆盖掉）。
/// - [`ThreeDFps`](ViewportCamera::ThreeDFps)：操作 3D 相机（第一人称模式）；同上让位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewportCamera {
    /// 操作 2D 相机（Pan/Zoom）；3D 相机走自动轨道动画。
    #[default]
    TwoD,
    /// 操作 3D 相机：拖拽 = Orbit、滚轮 = Dolly。
    ThreeDOrbit,
    /// 操作 3D 相机：拖拽 = Look、WASD/QE = Move。
    ThreeDFps,
}

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub show_grid: bool,
    pub show_field: bool,
    /// **2D 图层开关**：关闭即"关掉 2D 相机" —— `build_scene_view` 不再声明
    /// `Viewport2D` 图层（网格 / 曲线 / 公式随之全部不画）。
    pub show_2d: bool,
    /// **3D 图层开关**：关闭即"关掉 3D 相机"（线框立方体与曲面不画）。
    pub show_3d: bool,
    /// 视口操作目标（K3，见 [`ViewportCamera`]）。
    pub viewport_camera: ViewportCamera,
    /// **2D 相机是否自由**（K3 补）：
    /// - `false`（默认）= **分镜驱动**：分镜的 `Camera2DFraming` 绑定在位，相机被动画推动；
    /// - `true` = **自由**：把分镜的 2D 相机绑定**移出绑定表**（否则它每帧无条件重写
    ///   `center` / `scale`，把用户的平移缩放同帧抹掉），改由 `Pan` / `Zoom` 命令驱动。
    pub free_2d_camera: bool,
    /// **合成后滤镜（C 线）**：点运算（亮度）。
    pub filter_gain: bool,
    /// **合成后滤镜（C 线）**：几何重映射（桶形畸变）。
    pub filter_distortion: bool,
    /// **合成后滤镜（C 线）**：邻域（3×3 模糊）。
    pub filter_blur: bool,
    /// 三个滤镜共用的强度参数（`0..1`，映射见 [`UiConfig::post_filters`]）。
    pub filter_param: f32,
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
            viewport_camera: ViewportCamera::default(),
            free_2d_camera: false,
            filter_gain: false,
            filter_distortion: false,
            filter_blur: false,
            filter_param: 0.5,
            flip_layer_order: false,
        }
    }
}

impl UiConfig {
    /// 由面板开关构造**合成后滤镜链**（C 线）。
    ///
    /// 顺序固定为 点运算 → 几何重映射 → 邻域（先调色、再位移、最后模糊）——
    /// 这也是"滤镜链是数据"的直接体现：面板改状态 ⇒ 链变 ⇒ 无需碰管线代码。
    /// 三个开关全关 ⇒ **空链** ⇒ 渲染层走直通路径（画面逐位不变）。
    pub fn post_filters(&self) -> PostFilterChain {
        let p = self.filter_param.clamp(0.0, 1.0);
        let mut chain = PostFilterChain::new();
        if self.filter_gain {
            // 1.0 → 1.0（恒等）、0.0 → 0.5（压暗一半）。
            chain.push(FilterSlot::brightness(1.0 - 0.5 * p));
        }
        if self.filter_distortion {
            // 桶形畸变：强度 0 时恒等。
            chain.push(FilterSlot::distortion(1.5 * p, 0.2 * p, 1.0));
        }
        if self.filter_blur {
            chain.push(FilterSlot::blur(p));
        }
        chain
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
