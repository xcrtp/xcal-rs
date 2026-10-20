// 3d.rs
//
// 3D 核心类型（纯逻辑，无 GPU 依赖）：
// - PrimitiveTopology：图元拓扑
// - MeshGeometry：顶点/法线/UV/索引/拓扑 + 稳定 id/version（几何）
// - Material3D：颜色/纹理/UV 投影/着色变体（外观）
// - ShaderVariant：着色变体（纯数据风格声明，渲染端据此查表选 pipeline）
//
// 几何与外观是两类关注点：`MeshGeometry` 只描述"长什么样"，
// `Material3D` 只描述"看起来什么颜色/贴什么纹理"。二者同层（均在 zela-scene）。
//
// M3-2：`Projection` / `Camera3D` 已迁往**场景对象层** `ze-object`
// （相机是用户可见的场景对象；两个相机同居一层）。
//
// W4：向量类型不再直接写理念层之外的路径，改用场景层自己的数学类型出口
// `crate::types`（= 理念层 `ze_notion` 的转发）—— 场景层是"数学概念的定义场所"，
// 收敛在此最值（新增数学相关代码不再制造同型重复）。

use crate::types::{Color32, Vec2, Vec3};

// S2：3D 渲染描述符已下沉 `ze-render-ir`（渲染 IR 层）；此处 re-export
// 保持 `zela_scene::scene::components::*` 路径不变。
pub use ze_render_ir::{PrimitiveTopology, ShaderVariant, TextureHandle, UvProjection};

/// 分配进程内唯一的几何 id。
fn next_geometry_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 3D 网格几何组件：顶点位置 + 可选法线/UV + 可选索引 + 拓扑。
///
/// 只描述"长什么样"，不含颜色等外观信息（外观见 [`Material3D`]）。
/// 每个几何体带一个进程内唯一、跨 `clone` 稳定的 `id`，以及可手动递增的
/// `version`；二者的组合（`ResourceKey::Versioned`）作为 GPU 侧缓存的键：
/// 几何内容变化时调用 [`MeshGeometry::bump_version`]，所有引用者的缓存一起失效。
///
/// # Example
/// ```rust
/// use zela_scene::scene::components::{MeshGeometry, PrimitiveTopology};
/// use zela_scene::types::Vec3;
///
/// // 一个无索引三角形。
/// let mesh = MeshGeometry::new(
///     vec![
///         Vec3::new(-1.0, -1.0, 0.0),
///         Vec3::new(1.0, -1.0, 0.0),
///         Vec3::new(0.0, 1.0, 0.0),
///     ],
///     None,
///     PrimitiveTopology::Triangles,
/// );
/// assert_eq!(mesh.positions.len(), 3);
/// assert!(mesh.indices.is_none());
/// ```
#[derive(Debug, Clone)]
pub struct MeshGeometry {
    /// 顶点位置（模型空间）。
    pub positions: Vec<Vec3>,
    /// 可选逐顶点法线；长度应与 `positions` 一致。
    pub normals: Option<Vec<Vec3>>,
    /// 可选逐顶点 UV；长度应与 `positions` 一致。
    pub uvs: Option<Vec<Vec2>>,
    /// 可选索引；`None` 表示按顶点顺序连续解释。
    pub indices: Option<Vec<u32>>,
    /// 图元拓扑。
    pub topology: PrimitiveTopology,
    /// 进程内唯一且跨 `clone` 稳定的几何 id。
    id: u64,
    /// 几何内容版本号，由 [`MeshGeometry::bump_version`] 递增。
    version: u64,
}

impl MeshGeometry {
    /// 构造几何：`id` 自动分配，`version` 从 0 开始。
    pub fn new(
        positions: Vec<Vec3>,
        indices: Option<Vec<u32>>,
        topology: PrimitiveTopology,
    ) -> Self {
        Self {
            positions,
            normals: None,
            uvs: None,
            indices,
            topology,
            id: next_geometry_id(),
            version: 0,
        }
    }

    /// 附加逐顶点法线（builder 风格）。
    pub fn with_normals(mut self, normals: Vec<Vec3>) -> Self {
        self.normals = Some(normals);
        self
    }

    /// 附加逐顶点 UV（builder 风格）。
    pub fn with_uvs(mut self, uvs: Vec<Vec2>) -> Self {
        self.uvs = Some(uvs);
        self
    }

    /// 稳定几何 id（跨 `clone` 保持不变）。
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 当前几何版本号。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 递增几何版本号，使所有引用者的 GPU 缓存失效。
    pub fn bump_version(&mut self) -> u64 {
        self.version += 1;
        self.version
    }
}

/// 3D 材质组件：颜色 + 可选纹理句柄 + 无 UV 几何的兜底 UV 投影 + 着色变体。
///
/// 只描述"看起来什么颜色/贴什么纹理"，不含几何数据（几何见 [`MeshGeometry`]）。
///
/// # 图集 / 纹理数组（未来能力，无需改动本结构）
/// [`texture`](Self::texture) 是不透明句柄 [`TextureHandle`]，指向资源层的一张纹理。
/// 将来做字形图集或纹理数组时，只需把整张图集注册为一个 [`TextureHandle`]，并让
/// 几何 UV 指向图集内区域即可；**不需要**为本结构或 [`ShaderVariant`] 新增
/// `TextureKind` 之类的枚举（与 [`ContentHandle`](crate::domain::ContentHandle) 同构的不透明句柄设计）。
///
/// # Example
/// ```rust
/// use zela_scene::scene::components::{Material3D, ShaderVariant};
/// use zela_scene::types::Color32;
///
/// let m = Material3D::color(Color32::new(255, 0, 0, 255));
/// assert_eq!(m.color.r, 255);
/// assert!(m.texture.is_none());
/// // 默认着色变体为朗伯漫反射。
/// assert_eq!(m.variant, ShaderVariant::Lambert);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Material3D {
    /// 整体颜色（无纹理时的着色来源，有纹理时作为调制色）。
    pub color: Color32,
    /// 可选纹理句柄。
    pub texture: Option<TextureHandle>,
    /// 无 UV 几何的兜底 UV 投影策略。
    pub uv_projection: UvProjection,
    /// 着色变体（片元着色方程的选择）。
    pub variant: ShaderVariant,
}

impl Material3D {
    /// 以单一颜色构造材质（无纹理、平面投影兜底）。
    pub fn color(color: Color32) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }
}

/// 场景光源组件（B1-A 光照数据化）。
///
/// **单一真源**（B1 铁律 3）：本组件**不带 position** —— 光源的位置由
/// `Transform` → `WorldTransforms`（S1 已接通）提供，避免"位置"出现两份定义。
/// 方向光无位置概念，其 `direction` 直接声明在组件上；点光的 `range` 是衰减
/// 半径（**世界单位**），位置走 `Transform`。
///
/// 光照计算在渲染层统一消费：`SceneRecord3DSystem` 每帧把它录进
/// `LightQueue`，后端据此上传**场景级** light uniform buffer。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// 光源类型（方向光 / 点光）。
    pub kind: LightKind,
    /// 光的颜色（RGB 调制；alpha 恒 1，不参与光照）。
    pub color: Color32,
    /// 强度倍率（默认 1.0）。
    pub intensity: f32,
}

/// 光源类型。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightKind {
    /// 方向光：`direction` 是光传播方向（**世界空间**，从光源指向场景）。
    Directional { direction: Vec3 },
    /// 点光（B1-B）：`range` 是有效半径（世界单位，超出即衰减到 0）。
    /// **不带 position** —— 位置由同一对象的 `Transform`（→ `WorldTransforms`）提供。
    Point { range: f32 },
}

impl Light {
    /// 构造方向光（强度 1.0、纯白）。
    pub fn directional(direction: Vec3) -> Self {
        Self {
            kind: LightKind::Directional { direction },
            color: Color32::WHITE,
            intensity: 1.0,
        }
    }

    /// 构造方向光（自定义颜色与强度）。
    pub fn directional_with(direction: Vec3, color: Color32, intensity: f32) -> Self {
        Self {
            kind: LightKind::Directional { direction },
            color,
            intensity,
        }
    }

    /// 构造点光（强度 1.0、纯白）。位置不在本组件，由 `Transform` 提供。
    pub fn point(range: f32) -> Self {
        Self {
            kind: LightKind::Point { range },
            color: Color32::WHITE,
            intensity: 1.0,
        }
    }

    /// 构造点光（自定义颜色与强度）。
    pub fn point_with(range: f32, color: Color32, intensity: f32) -> Self {
        Self {
            kind: LightKind::Point { range },
            color,
            intensity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Color32;

    #[test]
    fn test_mesh_geometry_and_material3d_independent() {
        // 1) 几何与材质可独立构造。
        let mut geometry = MeshGeometry::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            Some(vec![0, 1, 2]),
            PrimitiveTopology::Triangles,
        );
        let material = Material3D::color(Color32::WHITE);
        let id_before = geometry.id();
        assert_eq!(geometry.version(), 0);

        // 2) 改材质不影响几何 identity/version：拆分后二者关注点天然隔离，
        //    不再需要旧 `hash_mesh` 里手工排除 color 的特例。
        let mut edited_material = material;
        edited_material.color = Color32::RED;
        edited_material.uv_projection = UvProjection::Spherical;
        assert_eq!(geometry.id(), id_before);
        assert_eq!(geometry.version(), 0, "改材质不应改变几何版本");
        assert_ne!(material, edited_material);

        // 3) 只有几何自身内容变更才递增 version，且 id 不变。
        assert_eq!(geometry.bump_version(), 1);
        assert_eq!(geometry.version(), 1);
        assert_eq!(geometry.id(), id_before, "bump_version 不改 id");
    }

    #[test]
    fn test_parametric_surface_uv_boundaries_exact() {
        // 临时构造 1×1 参数网格（u 沿 x 参数、v 沿 y 参数），验证 UV 被
        // 精确保留：边界值恰好 0.0 / 1.0，无浮点漂移。
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        for j in 0..=1 {
            let v = j as f32;
            for i in 0..=1 {
                let u = i as f32;
                positions.push(Vec3::new(u * 2.0 - 1.0, v * 2.0 - 1.0, 0.0));
                uvs.push(Vec2::new(u, v));
            }
        }
        let mesh = MeshGeometry::new(positions, None, PrimitiveTopology::Triangles).with_uvs(uvs);

        let uvs = mesh.uvs.as_ref().expect("uvs 应存在");
        assert_eq!(uvs[0], Vec2::new(0.0, 0.0));
        assert_eq!(uvs[1], Vec2::new(1.0, 0.0));
        assert_eq!(uvs[2], Vec2::new(0.0, 1.0));
        assert_eq!(uvs[3], Vec2::new(1.0, 1.0));
    }
}
