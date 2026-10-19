// render_context.rs
//
// WGPU 渲染上下文（具体后端层）：device/queue/target_format 句柄。
// - headless 构造仅依赖 wgpu + pollster（feature = "wgpu"）；
// - from_render_state 从 egui_wgpu::RenderState 提取（feature = "egui-wgpu"）。
//
// S2：错误类型本地下沉 —— 后端不再依赖 `xcal-scene::errors::Error`。

/// 渲染上下文构造错误（S2：本层自有的最小错误集）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderContextError {
    /// 无可用 GPU / 设备创建失败。
    #[error("Initialization error: {0}")]
    Initialization(String),
}

pub type Result<T> = std::result::Result<T, RenderContextError>;

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
    /// 直接构造（宿主已拿到 device / queue / format）。
    ///
    /// 浏览器路径：`await` 完 adapter/device 后用本函数构造；
    /// native 路径通常走 [`RenderContext::headless`]。
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            device,
            queue,
            target_format,
        }
    }

    /// 从 egui_wgpu::RenderState 提取上下文。
    #[cfg(feature = "egui-wgpu")]
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            target_format: rs.target_format,
        }
    }

    /// **异步请求 adapter + device**（S2；web 用）。
    ///
    /// 只返回原始句柄（不猜 format）：宿主需先用 adapter 查
    /// `surface.get_capabilities(&adapter)` 选格式，再用 [`RenderContext::new`]
    /// 构造上下文。本函数只 `await`，不依赖 `pollster`，可在 wasm 上用任意
    /// executor（如 `wasm_bindgen_futures`）驱动。
    pub async fn request_adapter_and_device(
        instance: &wgpu::Instance,
        options: wgpu::RequestAdapterOptions<'_, '_>,
    ) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
        let adapter = instance.request_adapter(&options).await.map_err(|e| {
            RenderContextError::Initialization(format!("No suitable GPU adapter: {e}"))
        })?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| {
                RenderContextError::Initialization(format!("Request device failed: {e}"))
            })?;
        Ok((adapter, device, queue))
    }

    /// 创建无 surface 的 headless 上下文（默认格式 [`DEFAULT_TARGET_FORMAT`]）。
    ///
    /// 仅 native `blocking` 构建可用；wasm 构建不编译（避免 `block_on` 静默卡死）。
    #[cfg(feature = "blocking")]
    pub fn headless() -> Result<Self> {
        Self::headless_with_format(DEFAULT_TARGET_FORMAT)
    }

    /// 仅 native `blocking` 构建可用（内部 `pollster::block_on`）。
    #[cfg(feature = "blocking")]
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
                .map_err(|e| {
                    RenderContextError::Initialization(format!("No suitable GPU adapter: {e}"))
                })?;

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .map_err(|e| {
                    RenderContextError::Initialization(format!("Request device failed: {e}"))
                })?;

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

    /// **同步回读**（仅 native `blocking` 构建）：`map_async` → `poll(wait)` → 紧凑像素。
    ///
    /// ⚠️ **字节序 = 目标纹理格式的字节序**，不是无条件 RGBA8：
    /// 默认 headless 格式是 `Bgra8UnormSrgb` ⇒ 返回的是 **BGRA**。
    /// 调用方需按 [`RenderContext::target_format`] 自行转换
    /// （示例见 `xcal_playback::to_rgba`）。
    #[cfg(feature = "blocking")]
    pub fn read_texture(&self, texture: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        let (buffer, height, unpadded, padded) = self.copy_to_readback(texture, size);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll 失败");
        let data = slice.get_mapped_range().expect("回读映射失败");
        let out = compact_rows(&data, height, unpadded, padded);
        drop(data);
        buffer.unmap();
        out
    }

    /// **异步回读**（S3；web 必用）：`map_async` 回调 → oneshot → `await`。
    ///
    /// 不依赖 `pollster`：wasm 上由 WebGPU 自动 poll；native 的 `blocking` 构建
    /// 会在 `await` 前调用一次 `poll(wait)`（行为与同步版一致）。
    ///
    /// 返回字节序同 [`RenderContext::read_texture`]（目标格式，不是无条件 RGBA8）。
    pub async fn read_texture_async(
        &self,
        texture: &wgpu::Texture,
        size: (u32, u32),
    ) -> Vec<u8> {
        let (buffer, height, unpadded, padded) = self.copy_to_readback(texture, size);
        let slice = buffer.slice(..);
        let (tx, rx) = futures_channel::oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        #[cfg(feature = "blocking")]
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll 失败");
        rx.await
            .expect("map_async 回调通道关闭")
            .expect("map_async 失败");
        let data = slice.get_mapped_range().expect("回读映射失败");
        let out = compact_rows(&data, height, unpadded, padded);
        drop(data);
        buffer.unmap();
        out
    }

    /// 建回读 buffer、录制 `copy_texture_to_buffer` 并提交。
    ///
    /// 返回 `(buffer, height, unpadded, padded)`。
    fn copy_to_readback(
        &self,
        texture: &wgpu::Texture,
        size: (u32, u32),
    ) -> (wgpu::Buffer, u32, u32, u32) {
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
        (buffer, height, unpadded, padded)
    }
}

/// 去掉 256 字节行对齐填充，得到紧凑像素。
fn compact_rows(data: &[u8], height: u32, unpadded: u32, padded: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        out.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_target_format_is_bgra8_srgb() {
        assert_eq!(DEFAULT_TARGET_FORMAT, wgpu::TextureFormat::Bgra8UnormSrgb);
    }

    #[cfg(feature = "blocking")]
    #[test]
    fn test_headless_construct_or_error() {
        let result = RenderContext::headless();
        match result {
            Ok(ctx) => {
                assert_eq!(ctx.target_format, DEFAULT_TARGET_FORMAT);
            }
            Err(RenderContextError::Initialization(_)) => {
                // 无 GPU 环境可接受
            }
        }
    }
}
