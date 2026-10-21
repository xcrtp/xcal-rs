// render_pod_probe.rs — **渲染契约探针**（B3-2）。
//
// # 目的：契约证伪，不是生产接线
//
// `ze_render_ir::caps::render2d` 的四条签名（`capabilities` / `create_target` /
// `upload` / `draw_2d`）此前**从未被任何代码实现**，却已被 `PINNED_FINGERPRINTS`
// 钉住挂 CI 门禁 —— 等于"没有任何消费方验证过的形状被冻结"。
// 本模块用一个**真实实现**（真 wgpu 后端、真像素回读）回答：
// 这四条签名够不够用、哪里缺语义、要不要改。
//
// # 边界（铁律）
//
// - **不改生产路径**：`StandardBackends` / `SceneCompositorCallback` / `assemble_runtime`
//   一律不碰；本模块整体 `#[cfg(test)]`，只存在于测试构建。
// - **不改 `caps.rs` 签名**：探针的结论是"够用（单帧）但缺生命周期"，见 GAPS I 段；
//   签名未改 ⇒ 指纹不变。
// - **不给 IR 补 serde**：载荷用探针内自定义的可序列化类型。`DrawCommand` / `Style2D`
//   是否要 serde 是**独立决策**（`Shape2D` 已有 serde）。
//
// # 三条边界事实（写进 GAPS）
//
// 1. `Pod::resolve` 一律 `None` ⇒ 只走 `call` 慢路径（`pod.rs:63-66`：None 合法）；
// 2. `RenderTargetDesc` 比后端能力**宽**：`format` / `sample_count` 在 wgpu 2D 上被
//    忽略（离屏固定 `OFFSCREEN_FORMAT` + `MSAA_SAMPLES`）⇒ 探针**显式收窄**，不静默；
// 3. `Handle` 只能表达**一个**实体，而 `draw_2d(Handle, PodBytes)` 需要"目标 + 批次"
//    两个 ⇒ 第二个必须编码进 `PodBytes`（`ProbeFrame` 的注释）。

#![allow(dead_code)]

use std::any::Any;
use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use ze_notion::{Color32, MeasureSpace, Rect2D, Transform2D, Vec2};
use ze_pod_core::{
    FunctionId, Handle, HandleTable, Pod, PodBytes, PodError, PodId, QualifiedName,
};
use ze_render_ir::caps::{render2d, RenderCapabilitySet};
use ze_render_ir::{ColorFormat, LayerId, RenderTargetDesc, Shape2D, Style2D};

use crate::wgpu_2d::{Frame2DGeometry, MSAA_SAMPLES, OFFSCREEN_FORMAT, WgpuBackend2D};
use crate::RenderContext;

// ─── 探针载荷（**本文件私有**，与 IR 的 serde 决策解耦）────────────────────

/// 一次 `upload` 的载荷：一组绘制项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeBatch {
    pub draws: Vec<ProbeDraw>,
}

/// 一条探针绘制项（`Shape2D` 的极小子集 + 样式 + 变换）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeDraw {
    pub z: i32,
    pub shape: ProbeShape,
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<[u8; 4]>,
    pub stroke_width: f32,
    /// 平移（世界 / 屏幕坐标由宿主决定；探针按屏幕点处理）。
    pub translate: [f32; 2],
}

/// 探针图元子集。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProbeShape {
    Circle { center: [f32; 2], radius: f32 },
    Rect { min: [f32; 2], max: [f32; 2] },
    Line { start: [f32; 2], end: [f32; 2] },
}

/// `draw_2d` 的载荷 = **帧描述**（不是绘制数据）。
///
/// ⚠️ 契约缺口实证：`draw_2d(Handle, PodBytes)` 只有一个 `Handle`，但一帧需要
/// "渲染目标" + "已上传的批次"两个实体 ⇒ **批次只能编码进 `PodBytes`**
/// （这里用 `batch_slot` / `batch_generation`，即 `Handle` 的两个号）。
/// 换句话说：`PodBytes` 里没有 `Handle` 的表示规则，跨边界只能自己约定编码。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeFrame {
    pub batch_slot: u64,
    pub batch_generation: u32,
}

impl ProbeDraw {
    /// 探针载荷 → IR（唯一转换点）。
    pub fn to_ir(&self) -> (Shape2D, Style2D, Transform2D) {
        let shape = match &self.shape {
            ProbeShape::Circle { center, radius } => Shape2D::Circle {
                center: Vec2::new(center[0], center[1]),
                radius: *radius,
            },
            ProbeShape::Rect { min, max } => Shape2D::Rect(Rect2D::new(
                Vec2::new(min[0], min[1]),
                Vec2::new(max[0], max[1]),
            )),
            ProbeShape::Line { start, end } => Shape2D::Line {
                start: Vec2::new(start[0], start[1]),
                end: Vec2::new(end[0], end[1]),
            },
        };
        let style = Style2D {
            fill: self.fill.map(|[r, g, b, a]| Color32::from_rgba(r, g, b, a)),
            stroke: self.stroke.map(|[r, g, b, a]| Color32::from_rgba(r, g, b, a)),
            stroke_width: self.stroke_width,
            space: MeasureSpace::Screen,
        };
        let transform = Transform2D::translate(Vec2::new(self.translate[0], self.translate[1]));
        (shape, style, transform)
    }
}

// ─── 探针 Pod ──────────────────────────────────────────────────────────────

/// `zela.render2d` 的**真实实现**（测试探针）。
///
/// 内部状态是 `RefCell<ProbeState>`（后端 `&mut self` 要求内部可变性）。
/// `RefCell` 让 Pod `!Sync`，但**不是**硬阻塞：`ProbeState` 在 native 上是
/// `Send + Sync`（后端带 S1 徽章）⇒ 换 `Mutex` 即可登记 `SyncPod`；
/// 平台对照见 `examples/send_probe.rs` 与 `probe_state_is_send_and_sync_on_native`。
pub struct RenderProbePod {
    id: PodId,
    names: Vec<QualifiedName>,
    state: RefCell<ProbeState>,
}

/// 探针内部状态。
///
/// # 句柄映射规则（契约 R-05 / R-06 的落地形态）
///
/// - **目标**：`Handle.slot` 直接当离屏 `LayerId` 用
///   （`backend.offscreen_texture(LayerId(handle.slot()))`），`generation` 由
///   `HandleTable` 校验；`RenderTargetDesc` 与 `Frame2DGeometry` 存在 `targets`。
/// - **批次**：`Handle` ↔ `batches` 的槽位，世代 / ABA 由 `HandleTable` 负责。
/// - **`GpuHandle` 不出 Pod**：离屏纹理只在本结构内被提交 / 回读，跨边界只有
///   `Handle`（两个整数 + PodId）。
/// - 生产化时的通用对齐（GAPS R-06）：`Handle.slot` ↔ `GpuResourceId.index`、
///   `Handle.generation` ↔ `GpuResourceId.generation`。探针目前用 `LayerId` 代替
///   资源 id（离屏纹理由后端按 layer 持有），所以这条对齐**尚未真正启用** ——
///   写在这里是为了让后续接线不必重新推导。
struct ProbeState {
    ctx: RenderContext,
    backend: WgpuBackend2D,
    targets: HandleTable<TargetSlot>,
    batches: HandleTable<ProbeBatch>,
}

struct TargetSlot {
    desc: RenderTargetDesc,
    geometry: Frame2DGeometry,
}

impl RenderProbePod {
    /// 在给定上下文上建探针（真设备；`RenderContext::headless()` 的产物）。
    pub fn new(ctx: RenderContext) -> Self {
        let id = PodId::next();
        let backend = WgpuBackend2D::new(ctx.device(), ctx.queue());
        let names = render2d::SIGNATURES
            .iter()
            .map(|signature| signature.qualified())
            .collect();
        Self {
            id,
            names,
            state: RefCell::new(ProbeState {
                ctx,
                backend,
                targets: HandleTable::new(id),
                batches: HandleTable::new(id),
            }),
        }
    }

    /// `capabilities`（第 0 条）：**自述集合**只含 render2d。
    pub fn capabilities(&self) -> RenderCapabilitySet {
        RenderCapabilitySet::from_names(&[render2d::NAMESPACE])
    }

    /// 读回某个 target 的像素（测试专用；真实宿主应由合成器消费）。
    ///
    /// 字节序 = 离屏纹理格式（[`OFFSCREEN_FORMAT`] = `Rgba8Unorm`）⇒ RGBA。
    pub fn read_target(&self, handle: &Handle) -> Vec<u8> {
        let state = self.state.borrow();
        let slot = state.targets.get(handle).expect("target 句柄有效");
        let texture = state
            .backend
            .offscreen_texture(LayerId(handle.slot() as u32))
            .expect("draw_2d 已创建离屏纹理");
        state
            .ctx
            .read_texture(texture, (slot.desc.width, slot.desc.height))
    }

    fn create_target(&self, desc: RenderTargetDesc) -> Result<Handle, PodError> {
        desc.validate()
            .map_err(|e| PodError::CallFailed(format!("create_target：{e}")))?;
        // **显式收窄**：后端离屏格式 / 采样数是常量，契约却允许任意值。
        // 静默忽略会让调用方以为"格式生效了"，故这里直接拒绝（铁律 3）。
        if desc.format != ColorFormat::Rgba8Unorm {
            return Err(PodError::CallFailed(format!(
                "create_target：2D 离屏格式固定为 Rgba8Unorm（OFFSCREEN_FORMAT），\
                 收到 {:?}；契约未声明该限制",
                desc.format
            )));
        }
        if desc.sample_count != MSAA_SAMPLES {
            return Err(PodError::CallFailed(format!(
                "create_target：2D 离屏固定 {}x MSAA，收到 sample_count={}；\
                 契约未声明该限制",
                MSAA_SAMPLES, desc.sample_count
            )));
        }
        let geometry = Frame2DGeometry {
            // 缺口实证：`RenderTargetDesc` 没有视口矩形 / pixel ratio ⇒ 探针只能
            // 约定"整目标 = 视口、1 point = 1 px"。
            viewport_points: Rect2D::new(
                Vec2::ZERO,
                Vec2::new(desc.width as f32, desc.height as f32),
            ),
            pixels_per_point: 1.0,
            screen_size_px: [desc.width, desc.height],
        };
        let mut state = self.state.borrow_mut();
        Ok(state.targets.insert(TargetSlot { desc, geometry }))
    }

    fn upload(&self, payload: &PodBytes) -> Result<Handle, PodError> {
        let batch: ProbeBatch = serde_json::from_slice(payload.as_slice())
            .map_err(|e| PodError::CallFailed(format!("upload：载荷不是 ProbeBatch：{e}")))?;
        let mut state = self.state.borrow_mut();
        // 缺口实证：`upload(PodBytes) -> Handle` **每次调用都新开槽**，
        // 契约里没有 release / 原地更新 ⇒ 每帧 upload 会持续泄漏槽位。
        Ok(state.batches.insert(batch))
    }

    fn draw_2d(&self, target: Handle, payload: &PodBytes) -> Result<(), PodError> {
        let frame: ProbeFrame = serde_json::from_slice(payload.as_slice())
            .map_err(|e| PodError::CallFailed(format!("draw_2d：载荷不是 ProbeFrame：{e}")))?;
        let mut state = self.state.borrow_mut();

        // 先把借用取完（geometry 是 Copy；batch 复制一份），再拿 `&mut backend`。
        let geometry = state.targets.get(&target)?.geometry;
        let batch_handle = Handle::new(self.id, frame.batch_slot, frame.batch_generation);
        let batch = state.batches.get(&batch_handle)?.clone();
        let layer = LayerId(target.slot() as u32);

        // 缺口实证：清屏 / 背景**不在契约里**（2D 离屏固定清成透明，
        // 背景由合成器负责）⇒ `draw_2d` 无法表达"先把画面铺满底色"。
        state.backend.begin_frame_at(geometry);
        for draw in &batch.draws {
            let (shape, style, transform) = draw.to_ir();
            state.backend.push(&shape, &transform, &style, draw.z);
        }
        let mut encoder =
            state
                .ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-render-pod-probe"),
                });
        state.backend.finish_frame(&mut encoder, layer, &geometry);
        state.ctx.submit(encoder.finish());
        Ok(())
    }
}

impl Pod for RenderProbePod {
    fn id(&self) -> PodId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        &self.names
    }

    /// **一律 None**：本探针只提供 erased 慢路径。
    ///
    /// `pod.rs:63-66` 明写 None = "没有（或只有 erased 实现）" ⇒ 合法。
    /// 这条也为 B3-2d ④ 取证：`Pod` 不要求 `Send + Sync`，瓶颈在别处。
    fn resolve(&self, _name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        None
    }

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        match id.get() {
            0 => Ok(Box::new(self.capabilities())),
            1 => {
                let desc = arg::<RenderTargetDesc>(args, 0)?;
                Ok(Box::new(self.create_target(*desc)?))
            }
            2 => {
                let payload = arg::<PodBytes>(args, 0)?;
                Ok(Box::new(self.upload(payload)?))
            }
            3 => {
                let target = arg::<Handle>(args, 0)?;
                let payload = arg::<PodBytes>(args, 1)?;
                self.draw_2d(*target, payload)?;
                Ok(Box::new(()))
            }
            _ => Err(PodError::UnknownFunction(format!("函数编号 {id}"))),
        }
    }
}

fn arg<'a, T: 'static>(args: &'a [&'a dyn Any], index: usize) -> Result<&'a T, PodError> {
    args.get(index)
        .and_then(|value| value.downcast_ref::<T>())
        .ok_or_else(|| PodError::CallFailed(format!("第 {index} 个实参类型不匹配")))
}

// ─── 对拍：typed 直调 vs Pod 边界 ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 160;
    const H: u32 = 120;

    /// 无 GPU（CI 软件适配器缺失）时跳过；与仓库既有 headless 约定一致。
    fn headless() -> Option<RenderContext> {
        RenderContext::headless_with_format(OFFSCREEN_FORMAT).ok()
    }

    fn target_desc() -> RenderTargetDesc {
        RenderTargetDesc::new(W, H, ColorFormat::Rgba8Unorm).with_sample_count(MSAA_SAMPLES)
    }

    /// 同一段场景的**声明式**描述（两条路径共用同一份数据，避免手写两份）。
    fn scene() -> Vec<ProbeDraw> {
        vec![
            ProbeDraw {
                z: 0,
                shape: ProbeShape::Rect {
                    min: [10.0, 10.0],
                    max: [150.0, 110.0],
                },
                fill: Some([30, 60, 120, 255]),
                stroke: None,
                stroke_width: 0.0,
                translate: [0.0, 0.0],
            },
            ProbeDraw {
                z: 1,
                shape: ProbeShape::Circle {
                    center: [60.0, 60.0],
                    radius: 30.0,
                },
                fill: Some([255, 0, 0, 255]),
                stroke: None,
                stroke_width: 0.0,
                translate: [0.0, 0.0],
            },
            ProbeDraw {
                z: 2,
                shape: ProbeShape::Line {
                    start: [20.0, 100.0],
                    end: [140.0, 100.0],
                },
                fill: None,
                stroke: Some([0, 255, 0, 255]),
                stroke_width: 4.0,
                translate: [0.0, 0.0],
            },
            ProbeDraw {
                z: 3,
                shape: ProbeShape::Circle {
                    center: [100.0, 60.0],
                    radius: 20.0,
                },
                fill: Some([255, 255, 255, 128]),
                stroke: None,
                stroke_width: 0.0,
                translate: [4.0, -2.0],
            },
        ]
    }

    fn geometry() -> Frame2DGeometry {
        Frame2DGeometry {
            viewport_points: Rect2D::new(Vec2::ZERO, Vec2::new(W as f32, H as f32)),
            pixels_per_point: 1.0,
            screen_size_px: [W, H],
        }
    }

    /// FNV-1a 64：与后端无关的帧指纹（对拍只要求"同机同后端"）。
    fn fingerprint(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// 路径 A：typed 直调后端（**不经过任何 Pod 边界**）。
    fn render_typed(ctx: &RenderContext, draws: &[ProbeDraw]) -> Vec<u8> {
        let mut backend = WgpuBackend2D::new(ctx.device(), ctx.queue());
        let g = geometry();
        backend.begin_frame_at(g);
        for draw in draws {
            let (shape, style, transform) = draw.to_ir();
            backend.push(&shape, &transform, &style, draw.z);
        }
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-render-probe-typed"),
            });
        backend.finish_frame(&mut encoder, LayerId(0), &g);
        ctx.submit(encoder.finish());
        let texture = backend
            .offscreen_texture(LayerId(0))
            .expect("typed 路径已创建离屏纹理");
        ctx.read_texture(texture, (W, H))
    }

    /// 路径 B：只经 `Pod::call` 慢路径。
    fn render_via_pod(ctx: RenderContext, draws: &[ProbeDraw]) -> Vec<u8> {
        let pod = RenderProbePod::new(ctx);

        // capabilities
        let caps = pod
            .call(FunctionId::new(0), &[])
            .expect("capabilities")
            .downcast::<RenderCapabilitySet>()
            .expect("返回类型是 RenderCapabilitySet")
            .as_ref()
            .to_owned();
        assert!(caps.render2d, "探针自述必须实现 render2d");

        // create_target
        let desc = target_desc();
        let target = *pod
            .call(FunctionId::new(1), &[&desc])
            .expect("create_target")
            .downcast::<Handle>()
            .expect("返回 Handle");

        // upload
        let batch = ProbeBatch {
            draws: draws.to_vec(),
        };
        let uploaded = PodBytes::new(serde_json::to_vec(&batch).expect("序列化批次"));
        let uploaded = *pod
            .call(FunctionId::new(2), &[&uploaded])
            .expect("upload")
            .downcast::<Handle>()
            .expect("返回 Handle");

        // draw_2d
        let frame = PodBytes::new(
            serde_json::to_vec(&ProbeFrame {
                batch_slot: uploaded.slot(),
                batch_generation: uploaded.generation(),
            })
            .expect("序列化帧描述"),
        );
        pod.call(FunctionId::new(3), &[&target, &frame])
            .expect("draw_2d");

        pod.read_target(&target)
    }

    /// **B3 核心断据**：同一场景两条路径逐字节相同 ⇒ 边界不引入语义差异。
    #[test]
    fn pod_boundary_matches_typed_path() {
        let Some(ctx) = headless() else {
            eprintln!("[render-pod-probe] 无可用适配器，跳过");
            return;
        };

        let h1_bytes = render_typed(&ctx, &scene());
        let h2_bytes = render_via_pod(ctx, &scene());

        let h1 = fingerprint(&h1_bytes);
        let h2 = fingerprint(&h2_bytes);
        eprintln!("[render-pod-probe] H1(typed) = {h1:016x}");
        eprintln!("[render-pod-probe] H2(pod)   = {h2:016x}");

        assert_eq!(h1, h2, "Pod 边界引入了像素差异");
        assert_eq!(h1_bytes, h2_bytes, "逐字节相同（比指纹更强）");
        assert_eq!(h1_bytes.len(), (W * H * 4) as usize);

        // 防空过：至少中心圆真的画出来了（红通道高、绿蓝低）。
        let at = |x: u32, y: u32| {
            let i = ((y * W + x) * 4) as usize;
            [h1_bytes[i], h1_bytes[i + 1], h1_bytes[i + 2], h1_bytes[i + 3]]
        };
        let center = at(60, 60);
        assert!(
            center[0] > 200 && center[1] < 60 && center[2] < 60,
            "圆心 (60,60) 应为红色，实得 {center:?}"
        );
        let lit = h1_bytes
            .chunks_exact(4)
            .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30)
            .count();
        eprintln!("[render-pod-probe] 亮像素 = {lit}");
        assert!(lit > 0, "帧不得为空（否则 H1==H2 是平凡成立）");
    }

    /// **B1 验收**：`resolve` 对四条签名一律返回 `None`（只走慢路径）。
    #[test]
    fn resolve_is_always_none() {
        let Some(ctx) = headless() else {
            eprintln!("[render-pod-probe] 无可用适配器，跳过");
            return;
        };
        let pod = RenderProbePod::new(ctx);
        assert_eq!(pod.functions().len(), render2d::SIGNATURES.len());
        for signature in render2d::SIGNATURES {
            let name = signature.qualified();
            assert!(
                pod.functions().contains(&name),
                "探针必须自述契约函数 `{name}`"
            );
            assert!(
                pod.resolve(&name).is_none(),
                "`{name}` 必须走慢路径（resolve = None）"
            );
        }
    }

    /// **B3-2d ④ 实证（native）**：`Send + Sync` **不是**硬阻塞。
    ///
    /// `WgpuBackend2D` 自带 S1 徽章（`wgpu_2d.rs:352-362`：`#[cfg(not(wasm32))]
    /// unsafe impl Send/Sync`），本探针的状态因此也是 `Send + Sync` ⇒
    /// 把 `RefCell` 换成 `Mutex` 就能登记 `SyncPod`。
    ///
    /// 反证在 `examples/send_probe.rs`：同一断言在 `--target wasm32` 下**必须失败**
    /// （徽章按 cfg 关掉，wgpu 资源在 wasm 上含 `Rc<RefCell<..>>`）。
    #[test]
    fn probe_state_is_send_and_sync_on_native() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProbeState>();
    }

    /// `Pod` 本身不要求 `Send + Sync`（`PodNode::new` 合法）⇒ 探针可被 Broker 持有。
    #[test]
    fn pod_runs_without_sync_badge() {
        let Some(ctx) = headless() else {
            eprintln!("[render-pod-probe] 无可用适配器，跳过");
            return;
        };
        let pod = RenderProbePod::new(ctx);
        assert!(!pod.functions().is_empty());
        let shared: std::sync::Arc<dyn Pod> = std::sync::Arc::new(pod);
        assert!(shared.as_any().downcast_ref::<RenderProbePod>().is_some());
    }

    /// **B3-2d ① 实证**：契约比后端窄 —— `format` / `sample_count` 必须显式拒绝，
    /// 不得静默忽略。
    #[test]
    fn target_desc_is_narrowed_explicitly() {
        let Some(ctx) = headless() else {
            eprintln!("[render-pod-probe] 无可用适配器，跳过");
            return;
        };
        let pod = RenderProbePod::new(ctx);

        let wrong_format = RenderTargetDesc::new(8, 8, ColorFormat::Bgra8Unorm)
            .with_sample_count(MSAA_SAMPLES);
        let err = pod
            .call(FunctionId::new(1), &[&wrong_format])
            .expect_err("格式不匹配必须显式失败");
        assert!(err.to_string().contains("Rgba8Unorm"), "实际：{err}");

        let wrong_samples =
            RenderTargetDesc::new(8, 8, ColorFormat::Rgba8Unorm).with_sample_count(1);
        let err = pod
            .call(FunctionId::new(1), &[&wrong_samples])
            .expect_err("采样数不匹配必须显式失败");
        assert!(err.to_string().contains("MSAA"), "实际：{err}");
    }

    /// **B3-2d ② 实证**：`upload` 每次调用都新开槽（契约无 release / 原地更新）。
    ///
    /// 这条用例是**证据**而不是期望行为：两次 upload 得到两个不同的 handle，
    /// 且旧句柄仍然存活 —— 生产化必须补 `release`（GAPS I-02）。
    #[test]
    fn upload_allocates_a_new_slot_per_call() {
        let Some(ctx) = headless() else {
            eprintln!("[render-pod-probe] 无可用适配器，跳过");
            return;
        };
        let pod = RenderProbePod::new(ctx);
        let payload = PodBytes::new(
            serde_json::to_vec(&ProbeBatch { draws: Vec::new() }).expect("序列化批次"),
        );
        let first = *pod
            .call(FunctionId::new(2), &[&payload])
            .expect("第一次 upload")
            .downcast::<Handle>()
            .expect("Handle");
        let second = *pod
            .call(FunctionId::new(2), &[&payload])
            .expect("第二次 upload")
            .downcast::<Handle>()
            .expect("Handle");
        assert_ne!(first, second, "每次 upload 都新开槽（契约无 release）");
        eprintln!(
            "[render-pod-probe] upload 槽位 {} → {}（无 release ⇒ 每帧泄漏）",
            first.slot(),
            second.slot()
        );
    }
}
