//! GPU 时间归因（B12 Step 4）：timestamp query 的**唯一**入口。
//!
//! # 为什么需要它
//!
//! B8 的分级计时全是 CPU wall-clock：它能测出"回读这行代码阻塞了多久"，
//! **测不出 GPU 实际执行时间**（CPU 阻塞包含等待：GPU 忙时与闲时表现完全不同）。
//!
//! # 硬约束（B12 §2 约束一）
//!
//! 后端不支持 ⇒ **显式 `unsupported` + 理由**，不得静默给 0 —— "GPU 0ms"看起来像
//! "GPU 不忙"，比没有更糟。
//!
//! # 做法（不改任何 pass）
//!
//! 在**主帧提交前后各提交一个空的 compute pass**，各自写一个 timestamp；队列按提交
//! 顺序执行 ⇒ 两个 timestamp 之间正好覆盖主帧的全部 GPU 工作。因此本模块不需要
//! 侵入 compositor / wgpu_2d / wgpu_3d 的 24 处 `timestamp_writes: None`。
//!
//! 已知边界：若主帧内部有多次 submit，中间的空隙（GPU 空闲等待 CPU）也被算进 span。
//! 这是"帧级 GPU 时间"而非"逐 pass GPU 时间"——后者需要改每一处 pass，本批不做。
//!
//! # 为什么默认关闭
//!
//! `TIMESTAMP_QUERY` 必须显式请求 device feature，而默认 device features 必须保持
//! "仅 wgpu 注册 ⇒ 空集"（B11 C2）。⇒ 由 `ZELA_GPU_TIMING=1` 显式开启。

use std::sync::mpsc;

/// 需要显式请求的 device feature。
pub const TIMING_FEATURE: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY;

/// 一次运行里 GPU 计时的三态（**必填**：缺省也要写清楚为什么没有数字）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTimingState {
    /// `ZELA_GPU_TIMING` 未开启。
    Disabled,
    /// 开启了，但本机 / 本次装配不可用（理由在 [`GpuTimingSupport::reason`]）。
    Unsupported,
    /// 已测出数字。
    Measured,
}

impl GpuTimingState {
    pub fn name(self) -> &'static str {
        match self {
            GpuTimingState::Disabled => "disabled",
            GpuTimingState::Unsupported => "unsupported",
            GpuTimingState::Measured => "measured",
        }
    }
}

/// 支持性结论（`state != Measured` 时 `reason` 必须非空）。
#[derive(Debug, Clone)]
pub struct GpuTimingSupport {
    pub state: GpuTimingState,
    pub reason: String,
}

impl GpuTimingSupport {
    pub fn disabled() -> Self {
        Self {
            state: GpuTimingState::Disabled,
            reason: "未启用（ZELA_GPU_TIMING=1 开启）".to_string(),
        }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            state: GpuTimingState::Unsupported,
            reason: reason.into(),
        }
    }

    pub fn measured() -> Self {
        Self {
            state: GpuTimingState::Measured,
            reason: String::new(),
        }
    }

    /// 一行 JSON（B12 Step 3 的报告即用它）。
    pub fn json(&self) -> String {
        format!(
            "{{\"state\":\"{}\",\"reason\":\"{}\"}}",
            self.state.name(),
            escape_json(&self.reason)
        )
    }
}

/// 探测 **adapter** 能力（不改 device）。浏览器侧也用它做显式探测。
pub fn probe(adapter: &wgpu::Adapter) -> GpuTimingSupport {
    let info = adapter.get_info();
    if !adapter.features().contains(TIMING_FEATURE) {
        return GpuTimingSupport::unsupported(format!(
            "adapter（{} / {:?}）没有 TIMESTAMP_QUERY",
            info.name, info.backend
        ));
    }
    GpuTimingSupport::measured()
}

/// ticks → 毫秒。`period_ns` 是 `queue.get_timestamp_period()`。
///
/// `end < begin` ⇒ 报错（而不是返回负数，负的 GPU 时间一定是错的）。
pub fn ticks_to_ms(period_ns: f32, begin: u64, end: u64) -> Result<f64, String> {
    if end < begin {
        return Err(format!("timestamp 倒退（begin={begin} end={end}）"));
    }
    Ok((end - begin) as f64 * period_ns as f64 / 1_000_000.0)
}

/// 帧级 GPU span：`begin()` / `finish()` 成对使用。
///
/// 每次 `finish` 都会等 map 完成（`device.poll(Wait)`）—— 这是**诊断路径**，
/// 默认关闭，不进入生产门禁的计时。
pub struct GpuSpan {
    device: wgpu::Device,
    queue: wgpu::Queue,
    query_set: wgpu::QuerySet,
    /// `QUERY_RESOLVE | COPY_DST`（wgpu 30：`MAP_READ` 只能与 `COPY_DST` 组合，
    /// 不能与 `QUERY_RESOLVE` 同在一个 buffer 上）。
    resolve: wgpu::Buffer,
    /// 回读暂存：`COPY_DST | MAP_READ`。
    staging: wgpu::Buffer,
    period_ns: f32,
}

impl GpuSpan {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, adapter: &wgpu::Adapter) -> Result<Self, String> {
        let support = probe(adapter);
        if support.state != GpuTimingState::Measured {
            return Err(support.reason);
        }
        if !device.features().contains(TIMING_FEATURE) {
            return Err(
                "device 未请求 TIMESTAMP_QUERY（adapter 支持，但创建 device 时没带上这枚 feature）"
                    .to_string(),
            );
        }
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu-timing"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-timing-resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-timing-staging"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            query_set,
            resolve,
            staging,
            period_ns: queue.get_timestamp_period(),
        })
    }

    /// 提交"起始"标记（空 compute pass 只写 timestamp）。
    pub fn begin(&self) -> Result<(), String> {
        self.marker(0)
    }

    /// 提交"结束"标记 + resolve + 回读，返回本帧 GPU span（ms）。
    pub fn finish(&self) -> Result<f64, String> {
        self.marker(1)?;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu-timing-resolve"),
        });
        encoder.resolve_query_set(&self.query_set, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.staging, 0, 16);
        self.queue.submit(Some(encoder.finish()));

        let (sender, receiver) = mpsc::channel();
        self.staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("等待 GPU 回读失败：{error}"))?;
        receiver
            .recv()
            .map_err(|_| "GPU 回读回调丢失".to_string())?
            .map_err(|error| format!("map 失败：{error}"))?;
        let (begin, end) = {
            let data = self
                .staging
                .slice(..)
                .get_mapped_range()
                .map_err(|error| format!("读取 resolve buffer 失败：{error}"))?;
            if data.len() < 16 {
                return Err(format!("resolve buffer 只有 {} 字节", data.len()));
            }
            let begin = u64::from_le_bytes(data[0..8].try_into().expect("8 字节"));
            let end = u64::from_le_bytes(data[8..16].try_into().expect("8 字节"));
            (begin, end)
        };
        self.staging.unmap();
        ticks_to_ms(self.period_ns, begin, end)
    }

    fn marker(&self, index: u32) -> Result<(), String> {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu-timing-mark"),
        });
        {
            // 空的 compute pass：只为携带 timestamp_writes，不做任何工作。
            let _pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gpu-timing-mark"),
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                    query_set: &self.query_set,
                    beginning_of_pass_write_index: Some(index),
                    end_of_pass_write_index: None,
                }),
            });
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }
}

fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 tick = 1ns ⇒ 1e6 ticks = 1ms。
    #[test]
    fn ticks_convert_through_the_timestamp_period() {
        assert_eq!(ticks_to_ms(1.0, 0, 1_000_000).unwrap(), 1.0);
        // 1 tick = 2ns ⇒ 1000 ticks = 2µs = 0.002ms。
        assert_eq!(ticks_to_ms(2.0, 0, 1_000).unwrap(), 0.002);
    }

    /// 倒退的时间戳必须报错（负 GPU 时间一定是错的）。
    #[test]
    fn backward_timestamps_are_an_error() {
        assert!(ticks_to_ms(1.0, 10, 9).unwrap_err().contains("倒退"));
    }

    /// 未测出数字时理由**不得为空**（静默 0 是禁止的）。
    #[test]
    fn non_measured_states_carry_a_reason() {
        for support in [
            GpuTimingSupport::disabled(),
            GpuTimingSupport::unsupported("no TIMESTAMP_QUERY"),
        ] {
            assert!(!support.reason.is_empty(), "{support:?}");
            assert!(support.json().contains("\"state\":\""));
        }
        assert!(GpuTimingSupport::measured().reason.is_empty());
    }
}
