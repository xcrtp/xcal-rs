// buffer.rs — 顶点缓冲的**中间产物**（B3-0a / B1）。
//
// 生产端（渲染无关层）把几何按 `VertexLayout` **交错了字节**，产出本结构；
// 后端只做 `vertices → create_buffer_init`，不再理解顶点结构（B3：后端只会上传）。
//
// 单一真源（铁律 3）：`layout` 是唯一的格式声明；`vertices` 必须恰好是
// `vertex_count × layout.stride` 字节，`validate()` 显式校验（不静默）。

use crate::layout::{VertexLayout, VertexLayoutError};

/// 顶点缓冲数据（已按 `layout` 交错打包）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexBufferData {
    /// 本缓冲按哪个布局打包（后端据此建 `VertexBufferLayout`）。
    pub layout: VertexLayout,
    /// 已按 `layout` 交错的顶点字节。
    pub vertices: Vec<u8>,
    /// 索引（可为空 ⇒ 非索引绘制）。
    pub indices: Vec<u32>,
    /// 顶点数（= `vertices.len() / layout.stride`）。
    pub vertex_count: u32,
}

/// 顶点缓冲自洽性错误（显式报错，不静默）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VertexBufferError {
    #[error("顶点布局非法：{0}")]
    Layout(#[from] VertexLayoutError),
    #[error("布局 stride 为 0，无法推导顶点数")]
    ZeroStride,
    #[error("顶点字节数 {bytes} 不是 stride {stride} 的整数倍")]
    PartialVertex { bytes: usize, stride: u32 },
    #[error("索引（第 {index} 个）{value} 越界：顶点数 {vertex_count}")]
    IndexOutOfBounds {
        index: usize,
        value: u32,
        vertex_count: u32,
    },
}

impl VertexBufferData {
    /// 按字节与索引构造（`vertex_count` 由 stride 推导）。
    pub fn new(
        layout: VertexLayout,
        vertices: Vec<u8>,
        indices: Vec<u32>,
    ) -> Result<Self, VertexBufferError> {
        let stride = layout.stride;
        if stride == 0 {
            return Err(VertexBufferError::ZeroStride);
        }
        if vertices.len() % stride as usize != 0 {
            return Err(VertexBufferError::PartialVertex {
                bytes: vertices.len(),
                stride,
            });
        }
        let vertex_count = (vertices.len() / stride as usize) as u32;
        let data = Self {
            layout,
            vertices,
            indices,
            vertex_count,
        };
        data.validate()?;
        Ok(data)
    }

    /// 校验布局、字节长度与索引范围。
    pub fn validate(&self) -> Result<(), VertexBufferError> {
        self.layout.validate()?;
        let stride = self.layout.stride;
        if stride == 0 {
            return Err(VertexBufferError::ZeroStride);
        }
        if self.vertices.len() != self.vertex_count as usize * stride as usize {
            return Err(VertexBufferError::PartialVertex {
                bytes: self.vertices.len(),
                stride,
            });
        }
        for (index, value) in self.indices.iter().enumerate() {
            if *value >= self.vertex_count {
                return Err(VertexBufferError::IndexOutOfBounds {
                    index,
                    value: *value,
                    vertex_count: self.vertex_count,
                });
            }
        }
        Ok(())
    }

    /// 是否为空（无顶点）。
    pub fn is_empty(&self) -> bool {
        self.vertex_count == 0
    }
}

/// 把 `Copy` 切片视为字节切片（**纯 CPU**，无 GPU 依赖）。
///
/// 生产端用它把 `[Vertex2D]` / `[f32]` 等交错数组转成 `Vec<u8>`；
/// 后端同样用它打包索引 / uniform（B3：只有顶点字节移到生产端）。
pub fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    // SAFETY: `T: Copy` 无 Drop；`size_of_val` 精确覆盖切片；只读视图，
    // 生命周期与输入切片绑定。
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aa::AaModel;
    use crate::cache::CacheStrategy;

    /// **已知向量**（B3-0a 验收）：3D 标准布局（stride 32）下，
    /// 一个顶点的字节序必须是 `[pos.xyz, uv.xy, normal.xyz]` 的 f32 小端串联。
    ///
    /// 锁定生产端与后端之间的字节契约（wgsl 的 `@location` 顺序同此）。
    #[test]
    fn known_vector_3d_vertex_bytes() {
        let layout = VertexLayout::standard_3d();
        assert_eq!(layout.stride, 32);
        let floats = [1.5f32, -2.25, 3.0, 0.25, 0.75, 0.0, 0.0, 1.0];
        let data = VertexBufferData::new(layout, bytes_of(&floats).to_vec(), vec![])
            .expect("一个 8-f32 顶点满足 stride 32");
        assert_eq!(data.vertex_count, 1);
        assert_eq!(data.vertices.len(), 32);
        // 逐 f32 回读，顺序必须与 `[pos, uv, normal]` 一致。
        let words: Vec<f32> = data
            .vertices
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(words, floats.to_vec());
    }

    /// 字节长度不整 ⇒ 显式错误（不静默截断）。
    #[test]
    fn partial_vertex_is_explicit_error() {
        let layout = VertexLayout::standard_3d();
        let err = VertexBufferData::new(layout, vec![0u8; 31], vec![]).unwrap_err();
        assert!(matches!(err, VertexBufferError::PartialVertex { .. }));
    }

    /// 索引越界 ⇒ 显式错误。
    #[test]
    fn index_out_of_bounds_is_explicit_error() {
        let layout = VertexLayout::standard( AaModel::None, CacheStrategy::Static);
        let vertices = bytes_of(&[0u32; 6]).to_vec(); // 24 字节 = 1 个 2D 顶点
        let err = VertexBufferData::new(layout, vertices, vec![0, 1]).unwrap_err();
        assert!(matches!(
            err,
            VertexBufferError::IndexOutOfBounds { value: 1, .. }
        ));
    }

    /// 2D 标准布局仍是 stride 24（D1：不改格式）。
    #[test]
    fn two_d_stride_stays_24() {
        let layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
        assert_eq!(layout.stride, 24);
        let vertices = bytes_of(&[0u32; 6]).to_vec();
        let data = VertexBufferData::new(layout, vertices, vec![0]).unwrap();
        assert_eq!(data.vertex_count, 1);
        data.validate().unwrap();
    }
}
