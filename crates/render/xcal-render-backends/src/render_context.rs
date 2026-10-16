// render_context.rs
//
// WGPU 渲染上下文（具体后端层）：device/queue/target_format 句柄。
// - headless 构造仅依赖 wgpu + pollster（feature = "wgpu"）；
// - from_render_state 从 egui_wgpu::RenderState 提取（feature = "egui-wgpu"）。

use xcal_scene::errors::Error;

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
    #[cfg(feature = "egui-wgpu")]
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
    pub fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        self.queue.write_buffer(buffer, offset, data);
    }

    /// **回读纹理为紧凑 RGBA8 像素**（行距 = `width * 4`，不含 wgpu 的 256 对齐填充）。
    ///
    /// W1：此前这是三份重复的测试 helper（`wgpu_2d` / `compositor` / `scene_callback`），
    /// 提升为公开 API 后三处改为调用它 —— 导出（序列帧抓帧）与测试共用同一条路径。
    ///
    /// `bytes_per_row` 的 **256 字节对齐**是 wgpu 硬要求（`copy_texture_to_buffer`），
    /// 故内部按 256 补行距、回读后再按行剥离填充。
    ///
    /// 要求纹理带 `COPY_SRC`（离屏纹理与合成目标都已带上）。
    pub fn read_texture(&self, texture: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xcal-readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xcal-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll 失败");
        let data = slice.get_mapped_range().expect("回读映射失败");
        let mut out = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + unpadded as usize]);
        }
        drop(data);
        buffer.unmap();
        out
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
                // 无 GPU 环境可接受
            }
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }
}
