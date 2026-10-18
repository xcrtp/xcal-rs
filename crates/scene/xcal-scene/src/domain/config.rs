// config.rs — 视图 / 相机控制的纯数据配置（UI 与场景共用）。
//
// 归位理由：`CameraDriveSystem` 是**场景 system**，它必须读到"视口操作目标 / 2D 相机
// 是否自由"这两个开关。把配置留在 UI 层会让场景反向依赖 UI；这里只搬数据，
// 不新增机制。滤镜链（`post_filters`）依赖渲染层的 `PostFilterChain`，留在
// `xcal-egui` 作为宿主侧派生函数。

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
    /// 三个滤镜共用的强度参数（`0..1`，映射见宿主侧的 `post_filters`）。
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
