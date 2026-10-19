// gpu_util.rs
//
// 2D / 3D 后端共用的 GPU 工具：
// - `bytes_of`：Copy 切片 → 字节切片（顶点 / 索引 / uniform 上传）；
// - `UniformArena`：256B 对齐的 dynamic uniform staging。
//
// 这两个工具在语义上不属于任何一侧后端，独立成模块以避免 2D 反向依赖 3D。

// ─── bytes_of ──────────────────────────────────────────────────────────────

/// 把 Copy 切片视为字节切片（顶点/索引/uniform 上传用）。
pub fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

// ─── G5：UniformArena ──────────────────────────────────────────────────────

/// Dynamic uniform arena。stride 强制 256 字节对齐（wgpu dynamic uniform offset 的规范）。
///
/// 典型使用：
/// 1. `begin_frame()` 重置 used_blocks（staging 底层容量不释放，避免抖动）；
/// 2. `push(data)` 追加一帧内每个 draw 的 uniform 字节，返回块 index（0..used_blocks）；
/// 3. `flush(device, queue)`：必要时扩容 buffer，一次性 `write_buffer` 上传 staging
///    的全部数据。返回 (&buffer, stride_bytes)。
///
/// RC.1 起 arena **不再持有 bind group**：bind group 还需绑定材质纹理视图，
/// 由后端按纹理 id 缓存（见 `WgpuBackend3D` 的 bind group 缓存）。`generation()`
/// 在每次重建 buffer 时递增，供后端判断其缓存的 bind group 是否已失效。
pub struct UniformArena {
    buffer: Option<wgpu::Buffer>,
    capacity_blocks: u32,
    used_blocks: u32,
    stride: u64,
    uniform_size: u64,
    staging: Vec<u8>,
    generation: u64,
}

impl UniformArena {
    pub const MIN_BLOCKS: u32 = 16;

    pub fn new(uniform_size: u64, stride: u64) -> Self {
        assert!(
            stride >= uniform_size,
            "stride ({stride}) must be >= uniform_size ({uniform_size})"
        );
        assert_eq!(
            stride & 0xFF,
            0,
            "stride must be 256-byte aligned (wgpu dynamic uniform offset), got {stride}"
        );
        Self {
            buffer: None,
            capacity_blocks: 0,
            used_blocks: 0,
            stride,
            uniform_size,
            staging: Vec::new(),
            generation: 0,
        }
    }

    /// 新帧开始：重置 used_blocks（staging 容量保留，避免每帧 realloc）。
    pub fn begin_frame(&mut self) {
        self.used_blocks = 0;
    }

    /// 追加 uniform 数据到 staging。返回本块的 block index（0..used_blocks），
    /// 调用方随后通过 `index * stride` 得到 dynamic uniform 的 byte offset。
    pub fn push(&mut self, data: &[u8]) -> u32 {
        assert_eq!(
            data.len() as u64,
            self.uniform_size,
            "push() data size mismatch"
        );
        let slot = self.used_blocks;
        self.used_blocks = slot.checked_add(1).expect("too many draws in frame");
        let needed = (self.used_blocks as u64 * self.stride) as usize;
        if self.staging.len() < needed {
            // 按 256 B 填充，让 staging 的已用区间完全填满，避免 write_buffer
            // 读到未初始化数据（wgpu 对 &[u8] 没有特殊要求，但保持干净）。
            self.staging.resize(needed, 0u8);
        }
        let off = (slot as u64 * self.stride) as usize;
        self.staging[off..off + data.len()].copy_from_slice(data);
        slot
    }

    /// 保证 arena 的 GPU 缓冲容量 ≥ needed_blocks（MIN_BLOCKS 下限），
    /// 写回 staging 数据。返回 (&buffer, stride_bytes)。
    ///
    /// buffer 扩容（身份变化）时 `generation` 递增 —— 后端据此判定其按纹理缓存的
    /// bind group 已失效并整体重建。
    pub fn flush<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> (&'a wgpu::Buffer, u32) {
        let needed_blocks = self.used_blocks.max(Self::MIN_BLOCKS);

        if needed_blocks > self.capacity_blocks {
            let new_blocks = needed_blocks.next_power_of_two();
            let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zela-3d-uniform-arena"),
                size: new_blocks as u64 * self.stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // 缓冲身份切换 -> 引用它的 bind group 必须重建。
            self.generation += 1;
            self.buffer = Some(new_buf);
            self.capacity_blocks = new_blocks;
        }

        let buffer = self.buffer.as_ref().expect("buffer must be created");
        let used_bytes = (self.used_blocks as u64 * self.stride) as usize;
        if used_bytes > 0 {
            queue.write_buffer(buffer, 0, &self.staging[..used_bytes]);
        }

        (buffer, self.stride as u32)
    }

    /// uniform buffer 的版本号：每次重建（扩容）递增。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[inline]
    pub fn capacity_blocks(&self) -> u32 {
        self.capacity_blocks
    }
    #[inline]
    pub fn used_blocks(&self) -> u32 {
        self.used_blocks
    }
    #[inline]
    pub fn stride(&self) -> u64 {
        self.stride
    }
}
