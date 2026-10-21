// surface.rs — 2D/3D 融合的**归属边界**与渲染策略（B3 Step 3）。
//
// # 设计要点
// - `Surface` 只描述**边界与映射**，**不带 position**（位置仍由 `Transform` 单一真源）；
// - `SurfaceRenderer` 是正交维度：`Renderer2D`（离屏 → 纹理 → 代理平面）/
//   `Renderer3D`（内容直接派生为 3D 网格）；
// - **不存 `surface_id`**：归属沿 `Parent` 链向上找最近的 `Surface` 祖先，
//   避免"关系"出现第二份真源；
// - 找不到 `SurfaceRenderer` ⇒ **显式错误**（不静默取默认策略）；
// - 沿 `Parent` 链用 visited 集合做**树的环检测**（不需要图算法依赖）。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ze_notion::{Rect2D, Vec2};
use ze_scene_contract::{ObjectId, Parent, SceneRead};

use crate::object::Camera2D;

/// 2D 内容到 3D 的边界（marker：只描述边界，不带位置）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// 2D 内容所在的矩形（逻辑坐标；映射方式见 [`Surface::mapping`]）。
    pub bounds: Rect2D,
    /// 坐标映射语义。
    pub mapping: SurfaceMapping,
    /// 锚点（归一化到 `bounds`：0,0 = 左上，1,1 = 右下）。
    pub anchor: Vec2,
}

impl Surface {
    pub fn new(bounds: Rect2D, mapping: SurfaceMapping, anchor: Vec2) -> Self {
        Self {
            bounds,
            mapping,
            anchor,
        }
    }
}

/// `Surface.bounds` 的坐标语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceMapping {
    /// 屏幕空间（像素 / 点）。
    Screen,
    /// 世界空间（与 3D 同一坐标系）。
    World,
}

/// Surface 的渲染策略（**必填**；缺席 ⇒ [`SurfaceError::MissingRenderer`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceRenderer {
    /// 2D 离屏渲染 → 纹理 → 代理平面（能力无损，多一次采样）。
    Renderer2D,
    /// 内容直接派生为 3D 网格（零额外 pass，能力受限）。
    Renderer3D,
}

/// 纹理采样过滤（**显式**；不靠后端默认值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureFilter {
    Nearest,
    Linear,
}

/// `Renderer2D` 的纹理契约（Fixed 尺寸；用户拍板不随视口/DPI 变化）。
///
/// 对应 `ContentTextureSpec` 的 `size_px`（同样的物理像素口径），
/// 但**不新建第二套尺寸语义**：这里只是把它挂到 Surface 上。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceTexture {
    /// 纹理尺寸（物理像素，**固定**）。
    pub texel_size: [u32; 2],
    /// 采样过滤（显式，不用隐含默认）。
    pub filter: TextureFilter,
    /// 是否生成/采样 mipmap（显式；默认值不隐藏）。
    pub mipmap: bool,
}

impl SurfaceTexture {
    pub fn new(texel_size: [u32; 2], filter: TextureFilter, mipmap: bool) -> Self {
        Self {
            texel_size,
            filter,
            mipmap,
        }
    }

    /// 固定尺寸 → 内容相机：`bounds`（世界/屏幕矩形）映射到整张纹理。
    ///
    /// - 尺寸必须非零；
    /// - `bounds` 必须非退化；
    /// - x / y 缩放必须一致（否则是静默拉伸）⇒ 显式错误。
    pub fn camera_for(&self, bounds: Rect2D) -> Result<Camera2D, SurfaceError> {
        let [width, height] = self.texel_size;
        if width == 0 || height == 0 {
            return Err(SurfaceError::InvalidTexelSize {
                width,
                height,
            });
        }
        let size = bounds.size();
        if !(size.x > 0.0 && size.y > 0.0) {
            return Err(SurfaceError::DegenerateBounds);
        }
        let scale_x = width as f32 / size.x;
        let scale_y = height as f32 / size.y;
        let relative = (scale_x - scale_y).abs() / scale_x.max(scale_y);
        if relative > 1e-3 {
            return Err(SurfaceError::AspectMismatch {
                bounds: [size.x, size.y],
                texel_size: self.texel_size,
                scale_x,
                scale_y,
            });
        }
        Ok(Camera2D::new(
            bounds.center(),
            scale_x,
            Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
        ))
    }
}

/// 归属结果：最近的 Surface 祖先 + 它的策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceOwner {
    pub surface: ObjectId,
    pub renderer: SurfaceRenderer,
}

/// Surface 归属失败（显式错误）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SurfaceError {
    #[error("Surface 对象 {0:?} 未声明 SurfaceRenderer（不静默取默认策略）")]
    MissingRenderer(ObjectId),
    #[error("Surface 对象 {0:?} 的 Renderer2D 未声明 SurfaceTexture（尺寸/过滤/mipmap 必须显式）")]
    MissingTexture(ObjectId),
    #[error("Parent 链存在环（归属判定在 {0:?} 处中止）")]
    ParentCycle(ObjectId),
    #[error("SurfaceTexture 尺寸非法：{width}x{height}（必须非零）")]
    InvalidTexelSize { width: u32, height: u32 },
    #[error("Surface.bounds 退化（宽高必须 > 0）")]
    DegenerateBounds,
    #[error(
        "SurfaceTexture 与 bounds 宽高比不一致（bounds={bounds:?}, texel={texel_size:?}, \
         scale_x={scale_x}, scale_y={scale_y}）：会静默拉伸，故拒绝"
    )]
    AspectMismatch {
        bounds: [f32; 2],
        texel_size: [u32; 2],
        scale_x: f32,
        scale_y: f32,
    },
}

/// 读取 Surface 的纹理契约（`Renderer2D` 必需）。
///
/// 未声明 ⇒ [`SurfaceError::MissingTexture`]（不在后端取默认值）。
pub fn resolve_surface_texture(
    scene: &impl SceneRead,
    owner: SurfaceOwner,
) -> Result<SurfaceTexture, SurfaceError> {
    scene
        .with::<SurfaceTexture, _>(owner.surface, |texture| *texture)
        .ok_or(SurfaceError::MissingTexture(owner.surface))
}

/// 沿 `Parent` 链向上找**最近的** `Surface`（含 `id` 自身）。
///
/// - 无 `Surface` 祖先 ⇒ `Ok(None)`（对象走默认路径）；
/// - 找到 `Surface` 但缺 `SurfaceRenderer` ⇒ [`SurfaceError::MissingRenderer`]；
/// - `Parent` 链成环 ⇒ [`SurfaceError::ParentCycle`]；
/// - `Parent(DANGLING)` / 父对象已不存在 ⇒ 视为链结束（`Ok(None)`）。
pub fn resolve_surface(
    scene: &impl SceneRead,
    id: ObjectId,
) -> Result<Option<SurfaceOwner>, SurfaceError> {
    let mut current = Some(id);
    let mut visited: HashSet<ObjectId> = HashSet::new();
    while let Some(node) = current {
        if !visited.insert(node) {
            return Err(SurfaceError::ParentCycle(node));
        }
        if !scene.contains(node) {
            return Ok(None);
        }
        if scene.with::<Surface, _>(node, |_| ()).is_some() {
            let renderer = scene
                .with::<SurfaceRenderer, _>(node, |renderer| *renderer)
                .ok_or(SurfaceError::MissingRenderer(node))?;
            return Ok(Some(SurfaceOwner {
                surface: node,
                renderer,
            }));
        }
        let parent = scene.with::<Parent, _>(node, |parent| parent.0);
        current = match parent {
            Some(ObjectId::DANGLING) | None => None,
            other => other,
        };
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Node {
        surface: Option<Surface>,
        renderer: Option<SurfaceRenderer>,
        texture: Option<SurfaceTexture>,
        parent: Option<Parent>,
    }

    #[derive(Default)]
    struct MockScene {
        nodes: HashMap<ObjectId, Node>,
    }

    impl MockScene {
        fn spawn(&mut self, id: ObjectId) -> &mut Node {
            self.nodes.entry(id).or_default()
        }
    }

    impl SceneRead for MockScene {
        fn contains(&self, id: ObjectId) -> bool {
            self.nodes.contains_key(&id)
        }

        fn len(&self) -> usize {
            self.nodes.len()
        }

        fn for_each<T: 'static + Send + Sync>(&self, _f: impl FnMut(ObjectId, &T)) {}

        fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
            &self,
            _f: impl FnMut(ObjectId, &A, &B),
        ) {
        }

        fn for_each_pair_opt<
            A: 'static + Send + Sync,
            B: 'static + Send + Sync,
            C: 'static + Send + Sync,
        >(
            &self,
            _f: impl FnMut(ObjectId, &A, &B, Option<&C>),
        ) {
        }

        fn with<T: 'static + Send + Sync, R>(
            &self,
            id: ObjectId,
            f: impl FnOnce(&T) -> R,
        ) -> Option<R> {
            let node = self.nodes.get(&id)?;
            if let Some(value) = &node.surface {
                if let Some(value) = (value as &dyn Any).downcast_ref::<T>() {
                    return Some(f(value));
                }
            }
            if let Some(value) = &node.renderer {
                if let Some(value) = (value as &dyn Any).downcast_ref::<T>() {
                    return Some(f(value));
                }
            }
            if let Some(value) = &node.texture {
                if let Some(value) = (value as &dyn Any).downcast_ref::<T>() {
                    return Some(f(value));
                }
            }
            if let Some(value) = &node.parent {
                if let Some(value) = (value as &dyn Any).downcast_ref::<T>() {
                    return Some(f(value));
                }
            }
            None
        }
    }

    fn id(n: u32) -> ObjectId {
        ObjectId::new(n, 1)
    }

    fn bounds() -> Rect2D {
        Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 100.0))
    }

    /// 自身带 Surface ⇒ 归属自身。
    #[test]
    fn surface_resolves_to_itself() {
        let mut scene = MockScene::default();
        let s = id(1);
        let node = scene.spawn(s);
        node.surface = Some(Surface::new(bounds(), SurfaceMapping::World, Vec2::ZERO));
        node.renderer = Some(SurfaceRenderer::Renderer3D);

        let owner = resolve_surface(&scene, s).unwrap().unwrap();
        assert_eq!(owner.surface, s);
        assert_eq!(owner.renderer, SurfaceRenderer::Renderer3D);
    }

    /// 内容对象沿 Parent 链找到最近的 Surface（不存 surface_id）。
    #[test]
    fn content_resolves_to_nearest_ancestor() {
        let mut scene = MockScene::default();
        let outer = id(1);
        let inner = id(2);
        let content = id(3);
        {
            let node = scene.spawn(outer);
            node.surface = Some(Surface::new(bounds(), SurfaceMapping::Screen, Vec2::ZERO));
            node.renderer = Some(SurfaceRenderer::Renderer2D);
        }
        {
            let node = scene.spawn(inner);
            node.surface = Some(Surface::new(bounds(), SurfaceMapping::World, Vec2::ZERO));
            node.renderer = Some(SurfaceRenderer::Renderer3D);
            node.parent = Some(Parent(outer));
        }
        scene.spawn(content).parent = Some(Parent(inner));

        let owner = resolve_surface(&scene, content).unwrap().unwrap();
        assert_eq!(owner.surface, inner, "必须取最近的 Surface 祖先");
        assert_eq!(owner.renderer, SurfaceRenderer::Renderer3D);
    }

    /// 没有 Surface 祖先 ⇒ None（默认路径）。
    #[test]
    fn no_surface_ancestor_is_none() {
        let mut scene = MockScene::default();
        let root = id(1);
        let child = id(2);
        scene.spawn(root);
        scene.spawn(child).parent = Some(Parent(root));
        assert_eq!(resolve_surface(&scene, child).unwrap(), None);
    }

    /// Surface 缺 SurfaceRenderer ⇒ 显式错误（不静默取默认）。
    #[test]
    fn missing_renderer_errors_explicitly() {
        let mut scene = MockScene::default();
        let s = id(1);
        scene.spawn(s).surface = Some(Surface::new(
            bounds(),
            SurfaceMapping::Screen,
            Vec2::ZERO,
        ));
        assert_eq!(
            resolve_surface(&scene, s),
            Err(SurfaceError::MissingRenderer(s))
        );
    }

    /// Parent 链成环 ⇒ 显式错误。
    #[test]
    fn parent_cycle_errors_explicitly() {
        let mut scene = MockScene::default();
        let a = id(1);
        let b = id(2);
        scene.spawn(a).parent = Some(Parent(b));
        scene.spawn(b).parent = Some(Parent(a));
        assert!(matches!(
            resolve_surface(&scene, a),
            Err(SurfaceError::ParentCycle(_))
        ));
    }

    /// `Parent(DANGLING)` 视为链结束。
    #[test]
    fn dangling_parent_ends_chain() {
        let mut scene = MockScene::default();
        let child = id(2);
        scene.spawn(child).parent = Some(Parent(ObjectId::DANGLING));
        assert_eq!(resolve_surface(&scene, child).unwrap(), None);
    }

    /// 固定尺寸 → 内容相机：bounds 映射到整张纹理。
    #[test]
    fn texture_camera_maps_bounds_to_full_texture() {
        let texture = SurfaceTexture::new([200, 100], TextureFilter::Linear, false);
        let bounds = Rect2D::new(Vec2::new(-10.0, -5.0), Vec2::new(90.0, 45.0));
        let camera = texture.camera_for(bounds).unwrap();
        assert_eq!(camera.scale, 2.0);
        assert_eq!(camera.center, Vec2::new(40.0, 20.0));
        assert_eq!(camera.rect.size(), Vec2::new(200.0, 100.0));
    }

    /// 尺寸非法 / bounds 退化 / 宽高比不一致 ⇒ 显式错误（不静默拉伸）。
    #[test]
    fn texture_contract_rejects_invalid_shapes() {
        let zero = SurfaceTexture::new([0, 100], TextureFilter::Nearest, false);
        assert!(matches!(
            zero.camera_for(bounds()),
            Err(SurfaceError::InvalidTexelSize { .. })
        ));

        let degenerate = SurfaceTexture::new([100, 100], TextureFilter::Nearest, false);
        assert!(matches!(
            degenerate.camera_for(Rect2D::new(Vec2::ZERO, Vec2::ZERO)),
            Err(SurfaceError::DegenerateBounds)
        ));

        let stretched = SurfaceTexture::new([200, 200], TextureFilter::Nearest, false);
        assert!(matches!(
            stretched.camera_for(Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 50.0))),
            Err(SurfaceError::AspectMismatch { .. })
        ));
    }

    /// `Renderer2D` 未声明 SurfaceTexture ⇒ 显式错误。
    #[test]
    fn missing_texture_errors_explicitly() {
        let mut scene = MockScene::default();
        let s = id(1);
        {
            let node = scene.spawn(s);
            node.surface = Some(Surface::new(bounds(), SurfaceMapping::World, Vec2::ZERO));
            node.renderer = Some(SurfaceRenderer::Renderer2D);
        }
        let owner = resolve_surface(&scene, s).unwrap().unwrap();
        assert_eq!(
            resolve_surface_texture(&scene, owner),
            Err(SurfaceError::MissingTexture(s))
        );

        scene.nodes.get_mut(&s).unwrap().texture =
            Some(SurfaceTexture::new([100, 100], TextureFilter::Linear, false));
        let owner = resolve_surface(&scene, s).unwrap().unwrap();
        assert_eq!(
            resolve_surface_texture(&scene, owner).unwrap().texel_size,
            [100, 100]
        );
    }
}
