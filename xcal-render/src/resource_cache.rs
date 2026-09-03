// resource_cache.rs
//
// GPU 资源缓存：按 (TypeId, Entity, version) 索引 GpuHandle，
// 支持脏标记重建、帧时钟、空闲淘汰。仅在 feature = "wgpu" 下编译
// （upload 回调通过 dyn RenderBackend 创建 GPU 资源）。
//
// 具体后端（EframeWgpuBackend）位于 xcal-render-backends。

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::render_backend::{GpuHandle, RenderBackend};

// ─── CacheKey ─────────────────────────────────────────────────────────────

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

// ─── 缓存条目 ─────────────────────────────────────────────────────────────

pub struct CacheEntry<T> {
    pub handle: GpuHandle,
    pub meta: T,
    last_used_frame: u64,
}

impl<T> CacheEntry<T> {
    pub fn idle_frames(&self, current_frame: u64) -> u64 {
        current_frame - self.last_used_frame
    }
}

// ─── ResourceCache ────────────────────────────────────────────────────────

pub struct ResourceCache<T> {
    entries: HashMap<CacheKey, CacheEntry<T>>,
    dirty: HashSet<CacheKey>,
    frame_clock: u64,
}

impl<T> Default for ResourceCache<T>
where
    T: 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ResourceCache<T>
where
    T: 'static,
{
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            dirty: HashSet::new(),
            frame_clock: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_clock += 1;
    }

    pub fn current_frame(&self) -> u64 {
        self.frame_clock
    }

    pub fn get(&self, key: &CacheKey) -> Option<&GpuHandle> {
        self.entries.get(key).map(|e| &e.handle)
    }

    pub fn contains(&self, key: &CacheKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }

    pub fn get_or_insert(
        &mut self,
        key: CacheKey,
        meta: T,
        upload: impl FnOnce(&T, &mut dyn RenderBackend) -> GpuHandle,
        backend: &mut dyn RenderBackend,
    ) -> &GpuHandle {
        let now = self.frame_clock;
        self.entries.entry(key).or_insert_with(|| CacheEntry {
            handle: upload(&meta, backend),
            meta,
            last_used_frame: now,
        });
        let entry = self.entries.get_mut(&key).expect("just inserted");
        entry.last_used_frame = now;
        &entry.handle
    }

    pub fn mark_dirty(&mut self, key: CacheKey) {
        if self.entries.contains_key(&key) {
            self.dirty.insert(key);
        }
    }

    pub fn upload_dirty(
        &mut self,
        backend: &mut dyn RenderBackend,
        mut rebuild: impl FnMut(&CacheKey, &T, &mut dyn RenderBackend) -> GpuHandle,
    ) -> usize {
        let keys: Vec<CacheKey> = self.dirty.drain().collect();
        let now = self.frame_clock;
        let mut count = 0;
        for key in keys {
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.handle = rebuild(&key, &entry.meta, backend);
                entry.last_used_frame = now;
                count += 1;
            }
        }
        count
    }

    pub fn mark_used(&mut self, key: &CacheKey) -> bool {
        let now = self.frame_clock;
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.last_used_frame = now;
                true
            }
            None => false,
        }
    }

    pub fn evict_unused(&mut self, max_idle_frames: u64) -> usize {
        let now = self.frame_clock;
        let mut evicted = Vec::new();
        self.entries.retain(|key, entry| {
            let keep = entry.idle_frames(now) <= max_idle_frames;
            if !keep {
                evicted.push(*key);
            }
            keep
        });
        for key in &evicted {
            self.dirty.remove(key);
        }
        evicted.len()
    }
}

/// 无元数据的 GPU 资源缓存（最常用形态）。
pub type GpuResourceCache = ResourceCache<()>;

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_entity(n: u32) -> hecs::Entity {
        let mut world = hecs::World::new();
        let mut e = world.spawn(());
        for _ in 0..n {
            e = world.spawn(());
        }
        e
    }

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

        let h1 = cache.get(&key).map(|h| *h.downcast_ref::<u32>().unwrap());
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
        let h2 = cache.get(&key).map(|h| *h.downcast_ref::<u32>().unwrap());
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
            cache.get(&key_v0).and_then(|h| h.downcast_ref::<u32>()),
            Some(&10)
        );
        assert_eq!(
            cache.get(&key_v1).and_then(|h| h.downcast_ref::<u32>()),
            Some(&20)
        );

        for _ in 0..3 {
            cache.begin_frame();
            cache.mark_used(&key_v1);
        }
        let evicted = cache.evict_unused(1);
        assert_eq!(evicted, 1);
        assert!(cache.get(&key_v0).is_none());
        assert!(cache.get(&key_v1).is_some());
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
        assert!(cache.contains(&key));
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

        let rebuilt = cache.upload_dirty(&mut StubBackend {}, |key, _, _| {
            assert_eq!(key.entity, e);
            GpuHandle::new(42_u32)
        });
        assert_eq!(rebuilt, 1);
        assert_eq!(cache.dirty_len(), 0);
        assert_eq!(
            cache.get(&key).and_then(|h| h.downcast_ref::<u32>()),
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
