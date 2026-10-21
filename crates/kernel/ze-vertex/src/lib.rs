// ze-vertex: 顶点格式契约（V 任务）——只声明格式，不生产数据。
//
// 三角化 / AA / Pipeline 三块共享同一个交汇点：顶点格式。本 crate 把这份共享契约
// 钉死为三份声明（§4）：
//   - layout.rs：AttribType / VertexAttrib / VertexLayout（V1 布局描述符 + V4 可哈希/可序列化）
//   - aa.rs：AaModel + 通用槽语义映射表（V2）
//   - cache.rs：CacheStrategy 缓存策略（V3）
//   - check.rs：debug 跨帧一致性校验（V5）
//
// 硬约束：本 crate **不得引用任何 wgpu 类型**。翻译到 wgpu::VertexBufferLayout
// 的代码在 zela-render-backends::vertex_layout（V6）。
//
// 契约消费方：
//   - 三角化器：填写 VertexLayout（声明"我吐出的顶点长什么样"）
//   - Pipeline：读取布局构造 vertex_buffers，并把它作为变体缓存 key 的一部分
//   - 缓存层：按 layout.cache 选择缓存路径（G7 回校对象）
//
// # 顶点布局是不变契约（跨着色风格共享）
// `VertexLayout` 是**跨着色风格共享的契约**：新增着色风格（体素 / 卡通 / PBR …）时
// **不得**改顶点格式，只能加 shader 变体与 pipeline —— 否则每加一种风格都要动格式，
// "换风格 = 换数据"的可替换性随之失效。
// （B2-0a：3D 布局由本 crate 的 [`layout::VertexLayout::standard_3d`] 声明并消费 ——
// `zela-render-backends::wgpu_3d` 的硬编码已改为契约翻译，见 wgpu_3d.rs。）

pub mod aa;
pub mod cache;
pub mod check;
pub mod layout;

pub use aa::{AaModel, AaSlotSemantics, AttribSemantic};
pub use cache::CacheStrategy;
pub use check::{hash_bytes, hash_bytes_sampled, StaticConsistencyChecker};
pub use layout::{AttribType, StepMode, VertexAttrib, VertexInputDesc, VertexLayout, VertexLayoutError};
