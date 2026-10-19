// mesh.rs — 3D 渲染描述符（S2：从 `zela_scene::scene::components` 下沉）。
//
// 这些是**纯数据声明**（拓扑 / 着色变体 / UV 兜底 / 纹理句柄），渲染后端按它们
// 查表选 pipeline；不属于场景逻辑，也不含任何 GPU 类型。
// `MeshGeometry` / `Material3D`（场景组件）继续留在 `zela-scene`，并 re-export
// 本模块的类型以保持既有路径。

/// 3D 图元拓扑方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PrimitiveTopology {
    /// 三角形列表：每 3 个顶点构成一个三角形。
    Triangles,
    /// 线段列表：每 2 个顶点构成一条线段。
    Lines,
    /// 点列表：每个顶点一个点。
    Points,
}

/// 纹理句柄：不透明标识符，仅用于在资源层定位纹理。
///
/// 句柄本身不持有 GPU 资源，具体纹理由后端按 id 解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TextureHandle(u64);

impl TextureHandle {
    /// 以给定 id 构造句柄。
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// 句柄 id。
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// 无 UV 几何的兜底 UV 投影策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum UvProjection {
    /// 沿主视轴平面投影（默认）。
    #[default]
    Planar,
    /// 球面投影。
    Spherical,
    /// 三平面投影。
    Triplanar,
}

/// 3D 着色变体：选择片元着色方程（即"风格"）。
///
/// 纯数据声明——只表达"想要哪种着色"，不引用 wgpu/egui。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum ShaderVariant {
    /// 无光照：`color * tex`。
    Flat,
    /// 朗伯漫反射（默认）。
    #[default]
    Lambert,
}
