// register.rs — **场景侧 System 注册点**（A6：注册序列从 app 归位到场景层）。
//
// 为什么归位：若"注册哪些场景 system"留在 app，app 就会把场景细节吸回来
// （瘦完还会胖回去）。这里只注册**场景自己拥有的** System；渲染录制与 UI 接线
// 分别由 `xcal-render` / `xcal-egui` 各自的注册函数负责，避免反向依赖。
//
// Phase 分配（Director 是唯一时间源）：
// - `Input`：DirectorAdvance（W=DirectorState/TimeState）、
//   CameraDrive（R=UiConfig/Camera*Handle，W=BindingTable）、
//   CameraInput（R=InputEvents/Camera*Handle，写场景对象组件）；
//   相机驱动必须在 `CameraInput` **之前** ⇒ 同帧生效。
// - `SceneSync`：BindingApply（R=BindingTable）。
// - `Prepare`：Replay（重播清理，在录制之前）。
// - `FrameEnd`：DeferredApply（帧末统一应用延迟命令，避免录制遍历中删对象）。

use xcal_schedule::scheduler::{Phase, Scheduler};

use crate::animation::{Binding, BindingApplySystem, CameraDriveSystem};
use crate::camera_input::CameraInputSystem;
use crate::deferred::{DeferredApplySystem, ReplaySystem};
use crate::director_state::DirectorAdvanceSystem;

/// 注册场景侧全部 System。
///
/// `cam2d_bindings`：分镜里指向 2D 相机的绑定（`center` + `scale`）；
/// `orbit_binding`：3D 自动轨道绑定。两者由装配层构造，`CameraDriveSystem` 托管让位。
pub fn register_systems(
    scheduler: &mut Scheduler,
    cam2d_bindings: Vec<Binding>,
    orbit_binding: Binding,
) {
    scheduler.add_system(Phase::Input, DirectorAdvanceSystem);
    // K3 + 2D 自由开关：按「2D 相机自由」与「视口操作目标」决定两台相机由分镜还是交互
    // 驱动，并让对应绑定让位/复位。放在 `CameraInputSystem` 之前：同帧内先改模式，
    // 交互才开始按新模式解释。
    scheduler.add_system(
        Phase::Input,
        CameraDriveSystem::new(cam2d_bindings, orbit_binding),
    );
    // P1：相机输入由场景层消费（相机状态归场景对象，UI 只发命令）。
    scheduler.add_system(Phase::Input, CameraInputSystem);

    scheduler.add_system(Phase::SceneSync, BindingApplySystem);

    // P6/B4：重播清理 —— 运行时创建的对象在重播时销毁（`Prepare`，在录制之前；
    // 该相位当前无其它 Resource 写者）。
    scheduler.add_system(Phase::Prepare, ReplaySystem);

    // P6/B3：延迟命令应用 —— 帧末统一应用（避免录制遍历中删对象）。
    scheduler.add_system(Phase::FrameEnd, DeferredApplySystem);
}
