// render_target.rs
//
// 渲染目标：纹理视图 + 格式/尺寸信息（wgpu 抽象）。
// 仅在 feature = "wgpu" 下编译。

/// 渲染目标：纹理视图 + 格式/尺寸信息。
///
/// 离屏目标拥有纹理（[`Option::Some`]）；surface 交换链纹理由帧宿主
/// （如 winit/eframe）持有并保证在 render pass 结束前存活，此处仅借用
/// 其视图（[`Option::None`]）。
pub struct RenderTarget {
    /// 拥有的纹理；surface 目标为 `None`（纹理由帧宿主持有）。
    pub texture: Option<wgpu::Texture>,
    /// 纹理视图（render pass 挂载点）。
    pub view: wgpu::TextureView,
    /// 纹理格式。
    pub format: wgpu::TextureFormat,
    /// 纹理尺寸。
    pub size: wgpu::Extent3d,
}

impl RenderTarget {
    /// 从已创建的纹理构造（取得所有权，自动创建视图并读取格式/尺寸）。
    pub fn from_texture(texture: wgpu::Texture) -> Self {
        let format = texture.format();
        let size = texture.size();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture: Some(texture),
            view,
            format,
            size,
        }
    }

    /// 从 surface 当前帧纹理借用视图（`SurfaceTexture` 由调用方持有至提交后）。
    pub fn from_surface(surface_texture: &wgpu::SurfaceTexture) -> Self {
        let texture = &surface_texture.texture;
        let format = texture.format();
        let size = texture.size();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture: None,
            view,
            format,
            size,
        }
    }

    /// 创建离屏渲染目标（`RENDER_ATTACHMENT | COPY_SRC | TEXTURE_BINDING`）。
    ///
    /// - `COPY_SRC`：便于回读/调试；
    /// - `TEXTURE_BINDING`：S3-2 起离屏纹理要作为合成器的采样源。
    pub fn new_offscreen(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-offscreen-target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self::from_texture(texture)
    }

    /// render pass 挂载的纹理视图。
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}
