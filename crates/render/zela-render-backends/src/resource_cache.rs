// resource_cache.rs（过渡 shim）
//
// GPU 资源缓存的纯数据部分（ResourceKey / DeferredDestroyer / ResourceCache / GpuResourceCache）
// 已下沉 kernel/ze-resource。本模块保留 glob 重导出以维持
// `zela_render::resource_cache::*` 路径不变。
//
// M3-7：原先的旧 `CacheKey { entity: hecs::Entity, .. }` 兼容 shim 已删除 ——
// 它在本仓**零外部消费方**（只有自己的测试在用），却把 hecs 拖进了本 crate。
// 测试改为直接构造 `ResourceKey::Versioned`，语义不变。

pub use ze_resource::resource_cache::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_backend::{AccessError, GpuHandle, RenderBackend};
    use std::any::TypeId;

    /// 测试用键：与旧 `CacheKey` → `ResourceKey` 的转换逐字段等价
    /// （`id` 扮演对象身份、`version` 扮演几何版本）。
    fn key<C: 'static>(object: u64, version: u64) -> ResourceKey {
        ResourceKey::Versioned {
            type_id: TypeId::of::<C>(),
            id: object,
            version,
        }
    }

    // ── 回归测试（7 个） ───────────────────────────────────────────────

    #[test]
    fn test_cachekey_of_uses_typeid() {
        struct Mesh;
        struct Texture;
        let k1 = key::<Mesh>(1, 0);
        let k2 = key::<Texture>(1, 0);
        let k3 = key::<Mesh>(1, 1);
        assert_ne!(k1, k2, "同对象不同类型 ⇒ 不同键");
        assert_ne!(k1, k3, "同对象不同版本 ⇒ 不同键");
        assert_eq!(k1, key::<Mesh>(1, 0));
    }

    #[test]
    fn test_get_or_insert_miss_then_hit() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let k = key::<u32>(1, 0);

        let mut upload_count = 0;
        cache.get_or_insert(
            k,
            (),
            |_, _| {
                upload_count += 1;
                GpuHandle::new(7_u32)
            },
            &mut StubBackend {},
        );
        assert_eq!(upload_count, 1);
        assert_eq!(cache.len(), 1);

        let h1 = cache.get(k).map(|h| *h.downcast_ref::<u32>().unwrap());
        cache.get_or_insert(
            k,
            (),
            |_, _| {
                upload_count += 1;
                GpuHandle::new(999_u32)
            },
            &mut StubBackend {},
        );
        assert_eq!(upload_count, 1);
        let h2 = cache.get(k).map(|h| *h.downcast_ref::<u32>().unwrap());
        assert_eq!(h1, Some(7));
        assert_eq!(h2, Some(7));
    }

    #[test]
    fn test_version_change_isolates_entries() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();

        let key_v0 = key::<u32>(1, 0);
        let key_v1 = key::<u32>(1, 1);

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
        let k = key::<u32>(1, 0);

        cache.begin_frame();
        cache.get_or_insert(k, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        for _ in 0..3 {
            cache.begin_frame();
            assert!(cache.mark_used(&k));
        }
        assert_eq!(cache.evict_unused(1), 0);
        assert!(cache.contains(k));
    }

    #[test]
    fn test_evict_unused_returns_count_and_cleans_dirty() {
        let mut cache = ResourceCache::<()>::new();
        let k = key::<u32>(1, 0);

        cache.begin_frame();
        cache.get_or_insert(k, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});
        cache.mark_dirty(k);
        assert_eq!(cache.dirty_len(), 1);

        cache.begin_frame();
        cache.begin_frame();
        cache.begin_frame();
        let evicted = cache.evict_unused(1);
        assert_eq!(evicted, 1);
        assert_eq!(cache.dirty_len(), 0);
        assert!(!cache.mark_used(&k));
    }

    #[test]
    fn test_mark_dirty_and_upload_dirty() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let k = key::<u32>(1, 0);

        cache.get_or_insert(k, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        cache.mark_dirty(k);
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
            cache.get(k).and_then(|h| h.downcast_ref::<u32>()),
            Some(&42)
        );
    }

    #[test]
    fn test_upload_dirty_skips_evicted_keys() {
        let mut cache = ResourceCache::<()>::new();
        let k = key::<u32>(1, 0);

        cache.begin_frame();
        cache.get_or_insert(k, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        cache.mark_dirty(k);
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
        let k: ResourceKey = key::<u32>(0, 0);

        let id = {
            let mut id_out = None;
            cache.get_or_insert(k, (), |_, _| GpuHandle::new(7_u32), &mut StubBackend {});
            // 通过 key 获取 id
            if let Some(&rid) = cache.key_id_of(&k) {
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
