// health.rs
//
// 运行时健康指标 overlay（F1 开关）与 World 快照导出（F10）。
//
// - overlay 展示 FPS / 帧耗时（p99 环形缓冲）/ 实体数 / System 数 /
//   draw call 数 / 粗略内存估计，数据来源为 Scheduler 的 FrameReport。
// - F10 把当前 World + Resources 通过 SnapshotRegistry 导出为 pretty JSON，
//   写入当前工作目录 `xcal_snapshot_tick{N}.json`，用于 bug 复现与状态对比。
//
// 本模块全部依赖 xcal-scene telemetry feature（默认开启）。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use xcal_schedule::resource::Resources;
use xcal_schedule::telemetry::{FrameReport, SnapshotRegistry, write_snapshot_json};

use xcal_scene::scene::components::{Material3D, MeshGeometry};
use xcal_scene::TimeState;
use xcal_egui::resources::UiConfig;
use xcal_render::ViewRect;
use xcal_render::components::{Circle, PhaseAnim, StrokeSolid};
use xcal_render::{DrawQueue, DrawQueue3D};
use xcal_render_backends::RenderContext;
use xc_object::{Camera2DHandle, Camera3D, Framing2D};

/// 环形缓冲保留的帧耗时样本数（约 4 秒 @60fps）。
const HISTORY_LEN: usize = 240;

/// 健康指标 overlay 与快照注册表。
pub struct HealthOverlay {
    open: bool,
    registry: SnapshotRegistry,
    frame_times_ms: VecDeque<f32>,
    fps: f32,
    // 最近一次更新的指标，供 overlay 绘制。
    last_frame_ms: f32,
    last_p99_ms: f32,
    entities: usize,
    systems: usize,
    draw_calls: usize,
}

impl Default for HealthOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthOverlay {
    pub fn new() -> Self {
        let mut registry = SnapshotRegistry::new();

        // ── 组件注册（未注册者仍会以 <unregistered> 出现在快照结构中）──
        // 几何组件以 `Arc<MeshGeometry>` 存储（见 `SceneRecord3DSystem` 的查询）。
        registry.register_component::<Arc<MeshGeometry>>("MeshGeometry");
        registry.register_component::<Material3D>("Material3D");
        registry.register_component::<Camera3D>("Camera3D");
        registry.register_component::<Circle>("Circle");
        registry.register_component::<StrokeSolid>("StrokeSolid");
        registry.register_component::<PhaseAnim>("PhaseAnim");

        // ── Resource 注册（仅记录存在性）──
        // P1-1：`AppCamera` 已删除（相机状态归场景对象）；这里登记的是
        // 相机**句柄**与场景里的取景参数组件。
        registry.register_resource::<Camera2DHandle>("Camera2DHandle");
        registry.register_resource::<TimeState>("TimeState");
        registry.register_resource::<UiConfig>("UiConfig");
        registry.register_resource::<ViewRect>("ViewRect");
        registry.register_resource::<DrawQueue>("DrawQueue");
        registry.register_resource::<DrawQueue3D>("DrawQueue3D");
        registry.register_resource::<RenderContext>("RenderContext");

        // 2D 相机取景参数（P1-2：与渲染目标 rect 解绑后的相机对象形态）。
        registry.register_component::<Framing2D>("Framing2D");

        Self {
            open: false,
            registry,
            frame_times_ms: VecDeque::with_capacity(HISTORY_LEN),
            fps: 0.0,
            last_frame_ms: 0.0,
            last_p99_ms: 0.0,
            entities: 0,
            systems: 0,
            draw_calls: 0,
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// 每帧用最新数据更新指标。
    pub fn update(&mut self, dt: f32, report: &FrameReport, entities: usize, draw_calls: usize) {
        let frame_ms = dt * 1000.0;
        if frame_ms > 0.0 {
            // FPS 指数滑动平均。
            let instant = 1000.0 / frame_ms;
            self.fps = if self.fps <= 0.0 {
                instant
            } else {
                self.fps * 0.9 + instant * 0.1
            };
        }
        if self.frame_times_ms.len() == HISTORY_LEN {
            self.frame_times_ms.pop_front();
        }
        self.frame_times_ms.push_back(frame_ms);

        self.last_frame_ms = report.total_duration.as_secs_f32() * 1000.0;
        self.last_p99_ms = self.p99_ms();
        self.entities = entities;
        self.systems = report.system_count();
        self.draw_calls = draw_calls;
    }

    /// 环形缓冲的 99 分位帧耗时（ms）。
    fn p99_ms(&self) -> f32 {
        if self.frame_times_ms.is_empty() {
            return 0.0;
        }
        let mut v: Vec<f32> = self.frame_times_ms.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((v.len() as f32 * 0.99) as usize).min(v.len() - 1);
        v[idx]
    }

    /// 粗略内存估计（MB）：实体 × 平均组件占用 + draw calls 顶点缓冲估值。
    /// 仅作量级参考，非精确统计。
    fn memory_estimate_mb(&self) -> f32 {
        const BYTES_PER_ENTITY: f32 = 256.0;
        const BYTES_PER_DRAW: f32 = 4096.0;
        (self.entities as f32 * BYTES_PER_ENTITY + self.draw_calls as f32 * BYTES_PER_DRAW)
            / (1024.0 * 1024.0)
    }

    /// 绘制 overlay 窗口（F1 开关；关闭时无操作）。
    pub fn show(&self, ui: &egui::Ui) {
        if !self.open {
            return;
        }
        egui::Window::new("Health (F1)")
            .default_pos([12.0, 40.0])
            .auto_sized()
            .show(ui.ctx(), |ui| {
                ui.label(format!("FPS: {:.1}", self.fps));
                ui.label(format!(
                    "Frame: {:.3} ms (systems total: {:.3} ms)",
                    self.frame_times_ms.back().copied().unwrap_or(0.0),
                    self.last_frame_ms
                ));
                ui.label(format!("Frame p99: {:.3} ms", self.last_p99_ms));
                ui.separator();
                ui.label(format!("Entities: {}", self.entities));
                ui.label(format!("Systems: {}", self.systems));
                ui.label(format!("Draw calls: {}", self.draw_calls));
                ui.label(format!("Mem(est): {:.2} MB", self.memory_estimate_mb()));
                ui.separator();
                ui.label(egui::RichText::new("F1 关闭面板 · F10 导出 World 快照 JSON").weak());
            });
    }

    /// 导出当前 World 快照到 `xcal_snapshot_tick{tick}.json`。
    pub fn dump_snapshot(
        &self,
        world: &hecs::World,
        resources: &Resources,
        tick: u64,
    ) -> std::io::Result<PathBuf> {
        let snapshot = self.registry.snapshot(world, resources, tick);
        let path = PathBuf::from(format!("xcal_snapshot_tick{tick}.json"));
        write_snapshot_json(&path, &snapshot)?;
        Ok(path)
    }
}
