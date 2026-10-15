// resource_cache.rs（过渡 shim）
//
// GPU 资源缓存的纯数据部分（ResourceKey / DeferredDestroyer / ResourceCache / GpuResourceCache）
// 已下沉 kernel/xc-resource。本模块只保留仍需 hecs 的兼容层：
//   G2: 旧 CacheKey 兼容（impl From<CacheKey> for ResourceKey）
//
// 保留 glob 重导出以维持 `xcal_render::resource_cache::*` 路径不变。

pub use xc_resource::resource_cache::*;

use std::any::TypeId;

// ─── 旧 CacheKey 兼容（实现 Into<ResourceKey>） ──────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheKey {
    pub type_id: TypeId,
    pub entity: hecs::Entity,
    pub version: u64,
}

impl CacheKey {
    pub fn of<C: 'static>(entity: hecs::Entity, version: u64) -> Self {
        Self {
            type_id: TypeId::of::<C>(),
            entity,
            version,
        }
    }
}

impl From<CacheKey> for ResourceKey {
    fn from(k: CacheKey) -> Self {
        let bits = k.entity.to_bits().get();
        let eid = bits as u32;
        let egen = (bits >> 32) as u32;
        let id = (eid as u64) | ((egen as u64) << 32);
        Self::Versioned {
            type_id: k.type_id,
            id,
            version: k.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_backend::{AccessError, GpuHandle, RenderBackend};

    fn dummy_entity(n: u32) -> hecs::Entity {
        let mut world = hecs::World::new();
        let mut e = world.spawn(());
        for _ in 0..n {
            e = world.spawn(());
        }
        e
    }

    // ── 回归测试（7 个） ───────────────────────────────────────────────

    #[test]
    fn test_cachekey_of_uses_typeid() {
        struct Mesh;
        struct Texture;
        let e = dummy_entity(0);
        let k1 = CacheKey::of::<Mesh>(e, 0);
        let k2 = CacheKey::of::<Texture>(e, 0);
        let k3 = CacheKey::of::<Mesh>(e, 1);
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_eq!(k1, CacheKey::of::<Mesh>(e, 0));
    }

    #[test]
    fn test_get_or_insert_miss_then_hit() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(1);
        let key = CacheKey::of::<u32>(e, 0);

        let mut upload_count = 0;
        cache.get_or_insert(
            key,
            (),
            |_, _| {
                upload_count += 1;
                GpuHandle::new(7_u32)
            },
            &mut StubBackend {},
        );
        assert_eq!(upload_count, 1);
        assert_eq!(cache.len(), 1);

        let h1 = cache.get(key).map(|h| *h.downcast_ref::<u32>().unwrap());
        cache.get_or_insert(
            key,
            (),
            |_, _| {
                upload_count += 1;
                GpuHandle::new(999_u32)
            },
            &mut StubBackend {},
        );
        assert_eq!(upload_count, 1);
        let h2 = cache.get(key).map(|h| *h.downcast_ref::<u32>().unwrap());
        assert_eq!(h1, Some(7));
        assert_eq!(h2, Some(7));
    }

    #[test]
    fn test_version_change_isolates_entries() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(2);

        let key_v0 = CacheKey::of::<u32>(e, 0);
        let key_v1 = CacheKey::of::<u32>(e, 1);

        cache.get_or_insert(
            key_v0,
            (),
            |_, _| GpuHandle::new(10_u32),
            &mut StubBackend {},
        );
        cache.get_or_insert(
            key_v1,
            (),
            |_, _| GpuHandle::new(20_u32),
            &mut StubBackend {},
        );

        assert_eq!(cache.len(), 2);
        assert_eq!(
            cache.get(key_v0).and_then(|h| h.downcast_ref::<u32>()),
            Some(&10)
        );
        assert_eq!(
            cache.get(key_v1).and_then(|h| h.downcast_ref::<u32>()),
            Some(&20)
        );

        for _ in 0..3 {
            cache.begin_frame();
            cache.mark_used(&key_v1);
        }
        let evicted = cache.evict_unused(1);
        assert_eq!(evicted, 1);
        assert!(cache.get(key_v0).is_none());
        assert!(cache.get(key_v1).is_some());
    }

    #[test]
    fn test_mark_used_prevents_evict() {
        let mut cache = ResourceCache::<()>::new();
        let e = dummy_entity(3);
        let key = CacheKey::of::<u32>(e, 0);

        cache.begin_frame();
        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        for _ in 0..3 {
            cache.begin_frame();
            assert!(cache.mark_used(&key));
        }
        assert_eq!(cache.evict_unused(1), 0);
        assert!(cache.contains(key));
    }

    #[test]
    fn test_evict_unused_returns_count_and_cleans_dirty() {
        let mut cache = ResourceCache::<()>::new();
        let e = dummy_entity(4);
        let key = CacheKey::of::<u32>(e, 0);

        cache.begin_frame();
        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});
        cache.mark_dirty(key);
        assert_eq!(cache.dirty_len(), 1);

        cache.begin_frame();
        cache.begin_frame();
        cache.begin_frame();
        let evicted = cache.evict_unused(1);
        assert_eq!(evicted, 1);
        assert_eq!(cache.dirty_len(), 0);
        assert!(!cache.mark_used(&key));
    }

    #[test]
    fn test_mark_dirty_and_upload_dirty() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(5);
        let key = CacheKey::of::<u32>(e, 0);

        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        cache.mark_dirty(key);
        assert_eq!(cache.dirty_len(), 1);

        let rebuilt = cache.upload_dirty(&mut StubBackend {}, |rk, _, _| {
            // 新 ResourceKey: 校验 Versioned 分支 type_id 与 version
            match rk {
                ResourceKey::Versioned {
                    type_id, version, ..
                } => {
                    assert_eq!(*type_id, std::any::TypeId::of::<u32>());
                    assert_eq!(*version, 0);
                }
                _ => panic!("expected Versioned ResourceKey"),
            }
            GpuHandle::new(42_u32)
        });
        assert_eq!(rebuilt, 1);
        assert_eq!(cache.dirty_len(), 0);
        assert_eq!(
            cache.get(key).and_then(|h| h.downcast_ref::<u32>()),
            Some(&42)
        );
    }

    #[test]
    fn test_upload_dirty_skips_evicted_keys() {
        let mut cache = ResourceCache::<()>::new();
        let e = dummy_entity(6);
        let key = CacheKey::of::<u32>(e, 0);

        cache.begin_frame();
        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        cache.mark_dirty(key);
        cache.begin_frame();
        cache.begin_frame();
        cache.begin_frame();
        cache.evict_unused(1);
        assert_eq!(cache.dirty_len(), 0);
        assert_eq!(
            cache.upload_dirty(&mut StubBackend {}, |_, _, _| GpuHandle::new(0_u32)),
            0
        );
    }

    // ── G1/G4 单测：stale id 返回 AccessError::Stale ──────────────────

    #[test]
    fn test_stale_id_returns_access_error() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(0);
        let key: ResourceKey = CacheKey::of::<u32>(e, 0).into();

        let id = {
            let mut id_out = None;
            cache.get_or_insert(key, (), |_, _| GpuHandle::new(7_u32), &mut StubBackend {});
            // 通过 key 获取 id
            if let Some(&rid) = cache.key_id_of(&key) {
                id_out = Some(rid);
            }
            // evict 它：手动走 destroy（让它空闲并 evict）
            cache.begin_frame();
            cache.begin_frame();
            cache.begin_frame();
            let n = cache.evict_unused(1);
            assert_eq!(n, 1);
            id_out.unwrap()
        };
        // 旧 id 访问：generation 应为 stale
        assert_eq!(cache.get_by_id(id), Err(AccessError::Stale { id }));
    }

    // ── Stub backend ───────────────────────────────────────────────────

    struct StubBackend;

    impl RenderBackend for StubBackend {
        fn device(&self) -> &wgpu::Device {
            unreachable!("StubBackend 不应触发 GPU 操作")
        }
        fn queue(&self) -> &wgpu::Queue {
            unreachable!("StubBackend 不应触发 GPU 操作")
        }
        fn target_format(&self) -> wgpu::TextureFormat {
            wgpu::TextureFormat::Bgra8UnormSrgb
        }
        fn create_buffer(&self, _desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle {
            GpuHandle::new(0_u32)
        }
        fn create_texture(&self, _desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle {
            GpuHandle::new(0_u32)
        }
        fn create_pipeline(&self, _desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle {
            GpuHandle::new(0_u32)
        }
        fn set_target(&mut self, _view: Option<wgpu::TextureView>) {}
        fn target(&self) -> Option<&wgpu::TextureView> {
            None
        }
    }
}
