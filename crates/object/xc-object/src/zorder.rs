// zorder.rs — 绘制层级（场景对象层的"摆放"信息，M4）。
//
// 为什么在对象层而不是渲染层：绘制层级是**对象画在哪一层**的摆放语义，
// 与 `Transform`（位置 / 旋转 / 缩放）同源 —— 它必须由装配方（`Axes2D` /
// `FunctionGraph` / 未来脚本）在**创建对象时**声明。渲染层只做翻译：
// `record_scene` 把它转成 [`crate::ZOrder`] → `DrawCommand::z`（`ZIndex`）。
//
// 为什么是**可缺席**的组件：绝大多数对象不需要显式分层（用录制默认层），
// 只有坐标轴 / 网格这类"观测工具"必须压在内容之下。因此契约层提供
// `for_each_pair_opt` 而不是要求所有对象都带它。

/// 绘制层级：值越小越先绘制（位于画面下层）。
///
/// 与迁移前 `GridBuildSystem` 直写队列的 z 值语义完全一致
/// （见 [`crate::axes::GRID_Z`] / [`crate::axes::AXIS_Z`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, derive_new::new)]
pub struct ZOrder(pub i32);
