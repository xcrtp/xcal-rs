// render_context.rs

use crate::{Error, Result};

/// headless 构造的默认目标格式。
pub const DEFAULT_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// WGPU 渲染上下文。
///
/// 薄包装：仅持有 wgpu 句柄（内部 Arc，Clone 廉价）与屏幕目标纹理格式。
/// 不持有 surface、不持有 egui 资源，可在任意线程/闭包中传递。
///
/// 构造路径：
/// - App 启动时由 [`RenderContext::from_render_state`] 从 `egui_wgpu::RenderState` 提取；
/// - 测试/离屏场景由 [`RenderContext::headless`]（默认格式）或
///   [`RenderContext::headless_with_format`]（可配置格式）构造。
#[derive(Clone)]
pub struct RenderContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub target_format: wgpu::TextureFormat,
}

impl RenderContext {
    // ─── 构造 ───────────────────────────────────────────────

    /// 从 egui_wgpu::RenderState 提取上下文。
    ///
    /// 仅复制 device/queue/target_format；adapter/instance/renderer 不携带。
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            target_format: rs.target_format,
        }
    }

    /// 创建无 surface 的 headless 上下文（默认格式 [`DEFAULT_TARGET_FORMAT`]）。
    ///
    /// 用于单元测试与离屏渲染。阻塞至 device 就绪（内部 `pollster::block_on`）。
    pub fn headless() -> Result<Self> {
        Self::headless_with_format(DEFAULT_TARGET_FORMAT)
    }

    /// 同 [`headless`]，但指定目标纹理格式（可配置入口）。
    pub fn headless_with_format(target_format: wgpu::TextureFormat) -> Result<Self> {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::None,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    ..Default::default()
                })
                .await
                .map_err(|e| Error::InitializationError(format!("No suitable GPU adapter: {e}")))?;

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .map_err(|e| Error::InitializationError(format!("Request device failed: {e}")))?;

            Ok(Self {
                device,
                queue,
                target_format,
            })
        })
    }

    // ─── 只读访问器 ────────────────────────────────────────

    #[inline]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[inline]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    #[inline]
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    // ─── 便捷工具 ──────────────────────────────────────────

    /// 提交单条命令缓冲到队列。
    #[inline]
    pub fn submit(&self, command_buffer: wgpu::CommandBuffer) {
        self.queue.submit(std::iter::once(command_buffer));
    }

    /// 写 buffer 数据（透传 `Queue::write_buffer`）。
    #[inline]
    pub fn write_buffer(
        &self,
        buffer: &wgpu::Buffer,
        offset: wgpu::BufferAddress,
        data: &[u8],
    ) {
        self.queue.write_buffer(buffer, offset, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_target_format_is_bgra8_srgb() {
        assert_eq!(DEFAULT_TARGET_FORMAT, wgpu::TextureFormat::Bgra8UnormSrgb);
    }

    /// headless 构造在 CI/无 GPU 环境可能失败，故仅断言"能跑通或返回初始化错误"。
    #[test]
    fn test_headless_construct_or_error() {
        let result = RenderContext::headless();
        match result {
            Ok(ctx) => {
                assert_eq!(ctx.target_format, DEFAULT_TARGET_FORMAT);
            }
            Err(Error::InitializationError(_)) => {
                // 无可用 adapter/device，可接受
            }
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }
}
