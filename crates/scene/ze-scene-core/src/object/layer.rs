// layer.rs — 图层类型（S2：从 `zela_scene::domain` 下沉）。
//
// `LayerKind` 携带相机与纹理句柄 ⇒ 需要场景对象层的类型；放在本层使
// `zela-render-backends` 可以只依赖 `ze-object`（它本来就需要相机类型），
// 而不必依赖 `zela-scene`。
//
// `LayerId` / `LayerBlend`（纯合成语义）在 `ze-render-ir`；本模块 re-export
// 它们，使消费方只需一个入口。

pub use ze_render_ir::{LayerBlend, LayerId, TextureHandle};

use crate::camera::{Camera2D, Camera3D};

/// 图层类型：一个视口 + 其相机。
#[derive(Debug, Clone, Copy)]
pub enum LayerKind {
    /// 2D 视口。
    Viewport2D { camera: Camera2D },
    /// 3D 视口。
    Viewport3D { camera: Camera3D },
    /// 内容纹理层：渲染到**独立纹理**，供 3D 材质采样。
    ContentTexture {
        /// 内容相机：`rect` 为**纹理矩形**（决定世界 → 纹理坐标的映射）。
        camera: Camera2D,
        /// 纹理尺寸（物理像素）。
        size_px: [u32; 2],
        /// 该层渲染到的纹理句柄；3D 材质用**同一句柄**引用它。
        texture: TextureHandle,
        /// 是否参与合成。默认 `false`（仅作纹理源）。
        composite: bool,
    },
}
