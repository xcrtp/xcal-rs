// render_context.rs

use xcal_core::errors::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// headless 构造的默认目标格式。
pub const DEFAULT_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// WGPU 渲染上下文。
#[derive(Clone)]
pub struct RenderContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub target_format: wgpu::TextureFormat,
}

impl RenderContext {
    /// 从 egui_wgpu::RenderState 提取上下文。
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            target_format: rs.target_format,
        }
    }

    /// 创建无 surface 的 headless 上下文（默认格式 [`DEFAULT_TARGET_FORMAT`]）。
    pub fn headless() -> Result<Self> {
        Self::headless_with_format(DEFAULT_TARGET_FORMAT)
    }

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

    #[inline]
    pub fn submit(&self, command_buffer: wgpu::CommandBuffer) {
        self.queue.submit(std::iter::once(command_buffer));
    }

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

    #[test]
    fn test_headless_construct_or_error() {
        let result = RenderContext::headless();
        match result {
            Ok(ctx) => {
                assert_eq!(ctx.target_format, DEFAULT_TARGET_FORMAT);
            }
            Err(Error::InitializationError(_)) => {
                // 可接受
            }
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }
}
