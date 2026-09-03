// resource_cache.rs
//
// GPU 资源缓存（G2–G4）：
//   G2: ResourceKey + 旧 CacheKey 兼容（From<CacheKey> for ResourceKey）
//   G3: DeferredDestroyer（graveyard 3 帧延迟销毁）
//   G4: ResourceCache 重写：generation-aware slot + key_to_id 索引
//       + 所有 GPU 资源释放都走 schedule_destroy，绝不直接 drop。
//
// 仅在 feature = "wgpu" 下编译（upload 回调通过 dyn RenderBackend 创建 GPU 资源）。
// 具体后端（EframeWgpuBackend）位于 xcal-render-backends。

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::render_backend::{AccessError, GpuHandle, GpuResourceId, RenderBackend};

// ─── ResourceKey (G2) ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResourceKey {
    Content { type_id: TypeId, hash: u64 },
    Versioned {
        type_id: TypeId,
        id: u64,
        version: u64,
    },
}

impl ResourceKey {
    pub fn content<C: 'static>(hash: u64) -> Self {
        Self::Content {
            type_id: TypeId::of::<C>(),
            hash,
        }
    }

    pub fn versioned<C: 'static>(id: u64, version: u64) -> Self {
        Self::Versioned {
            type_id: TypeId::of::<C>(),
            id,
            version,
        }
    }

    pub fn type_id(&self) -> TypeId {
        match self {
            Self::Content { type_id, .. } | Self::Versioned { type_id, .. } => *type_id,
        }
    }
}

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

// ─── DeferredDestroyer (G3) ───────────────────────────────────────────────

#[derive(Debug)]
pub struct DeferredDestroyer {
    pending: Vec<(u64, GpuHandle)>, // (destroyed_at_submitted_frame, handle)
    graveyard_period: u64,          // 保守值 = 3
    submitted_frame: u64,
}

impl DeferredDestroyer {
    pub const DEFAULT_GRAVEYARD_PERIOD: u64 = 3;

    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            graveyard_period: Self::DEFAULT_GRAVEYARD_PERIOD,
            submitted_frame: 0,
        }
    }

    pub fn with_period(period: u64) -> Self {
        Self {
            pending: Vec::new(),
            graveyard_period: period,
            submitted_frame: 0,
        }
    }

    pub fn schedule_destroy(&mut self, handle: GpuHandle) {
        self.pending.push((self.submitted_frame, handle));
    }

    pub fn on_submit(&mut self) {
        self.submitted_frame = self.submitted_frame.saturating_add(1);
    }

    /// 回收已经安全的 pending，返回实际 drop 的数量。
    ///
    /// 当「提交帧号 + graveyard_period <= 当前 submitted_frame」时认为可以安全销毁：
    /// 即资源在被 schedule_destroy 之后必须再经过 graveyard_period 次 submit 计数推进才会真正 drop。
    /// 测试语义：提交帧 0 的资源在 graveyard=3 时，在 submitted_frame 抵达 3 后（3 次 on_submit）开始回收。
    pub fn begin_frame(&mut self) -> usize {
        let submitted = self.submitted_frame;
        let graveyard = self.graveyard_period;
        let mut count = 0usize;
        self.pending.retain(|(submitted_at, _)| {
            let safe_frame = submitted_at.saturating_add(graveyard);
            if safe_frame <= submitted {
                count += 1;
                false
            } else {
                true
            }
        });
        count
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn submitted_frame(&self) -> u64 {
        self.submitted_frame
    }

    pub fn graveyard_period(&self) -> u64 {
        self.graveyard_period
    }
}

impl Default for DeferredDestroyer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── 缓存条目 + Slot (G4) ─────────────────────────────────────────────────

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

struct Slot {
    generation: u32,
    free: bool,
}

// ─── ResourceCache (G4) ───────────────────────────────────────────────────

pub struct ResourceCache<T> {
    key_to_id: HashMap<ResourceKey, GpuResourceId>,
    slots: Vec<Slot>,
    free: Vec<u32>,
    entries: HashMap<GpuResourceId, CacheEntry<T>>,
    destroyer: DeferredDestroyer,
    dirty: HashSet<ResourceKey>,
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
            key_to_id: HashMap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            entries: HashMap::new(),
            destroyer: DeferredDestroyer::new(),
            dirty: HashSet::new(),
            frame_clock: 0,
        }
    }

    // ── slot management ────────────────────────────────────────────────

    fn alloc_slot(&mut self) -> GpuResourceId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.generation = slot.generation.wrapping_add(1);
            if slot.generation == 0 {
                slot.generation = 1; // 避免 generation=0 与哨兵混淆
            }
            slot.free = false;
            GpuResourceId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 1,
                free: false,
            });
            GpuResourceId {
                index,
                generation: 1,
            }
        }
    }

    /// 释放 slot：把 handle 移入 destroyer，然后标记 slot free。
    fn release_slot(&mut self, id: GpuResourceId) {
        if id.is_sentinel() {
            return;
        }
        let idx = id.index() as usize;
        if idx >= self.slots.len() {
            return;
        }

        // 先把 handle 从 entries 取出来并投入 destroyer（延迟销毁），
        // 绝对不能让 CacheEntry 直接 drop 内部的 GpuHandle。
        if let Some(entry) = self.entries.remove(&id) {
            self.destroyer.schedule_destroy(entry.handle);
            // meta 直接丢弃（它不是 GPU 资源）
        }

        let slot = &mut self.slots[idx];
        slot.free = true;
        self.free.push(id.index());
    }

    // ── public API ─────────────────────────────────────────────────────

    pub fn begin_frame(&mut self) {
        self.frame_clock = self.frame_clock.saturating_add(1);
        // 每帧开始，收割 graveyard 中已安全的 pending GPU 资源
        let _dropped = self.destroyer.begin_frame();
    }

    pub fn notify_submit(&mut self) {
        self.destroyer.on_submit();
    }

    pub fn current_frame(&self) -> u64 {
        self.frame_clock
    }

    pub fn get<K: Into<ResourceKey>>(&self, key: K) -> Option<&GpuHandle> {
        let rk = key.into();
        let id = *self.key_to_id.get(&rk)?;
        self.entries.get(&id).map(|e| &e.handle)
    }

    pub fn key_id_of(&self, key: &ResourceKey) -> Option<&GpuResourceId> {
        self.key_to_id.get(key)
    }

    pub fn contains<K: Into<ResourceKey>>(&self, key: K) -> bool {
        let rk = key.into();
        self.key_to_id.contains_key(&rk)
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

    /// Generation 校验的句柄访问入口。
    pub fn get_by_id(&self, id: GpuResourceId) -> Result<&GpuHandle, AccessError> {
        if id.is_sentinel() {
            return Err(AccessError::Dropped);
        }
        let idx = id.index() as usize;
        let slot = self.slots.get(idx).ok_or(AccessError::Dropped)?;
        if slot.free || slot.generation != id.generation() {
            return Err(AccessError::Stale { id });
        }
        match self.entries.get(&id) {
            Some(entry) => Ok(&entry.handle),
            None => Err(AccessError::Dropped),
        }
    }

    pub fn get_or_insert<K>(
        &mut self,
        key: K,
        meta: T,
        upload: impl FnOnce(&T, &mut dyn RenderBackend) -> GpuHandle,
        backend: &mut dyn RenderBackend,
    ) -> &GpuHandle
    where
        K: Into<ResourceKey> + Copy,
    {
        let rk = key.into();
        let now = self.frame_clock;
        if !self.key_to_id.contains_key(&rk) {
            let mut handle = upload(&meta, backend);
            let id = self.alloc_slot();
            handle.assign_id(id);
            self.key_to_id.insert(rk, id);
            self.entries.insert(
                id,
                CacheEntry {
                    handle,
                    meta,
                    last_used_frame: now,
                },
            );
        }

        let id = *self.key_to_id.get(&rk).expect("inserted above");
        let entry = self.entries.get_mut(&id).expect("inserted above");
        entry.last_used_frame = now;
        &entry.handle
    }

    /// 直接插入一个已经构造好的 [`GpuHandle`]，不走 upload 回调，也不依赖 [`RenderBackend`]。
    ///
    /// 当 key 已经存在时，不替换已有条目，直接返回现有 handle 并更新 last_used_frame。
    /// 当 key 未命中时，为传入的 handle 分配 generation slot、写入 entries、记录 key→id 映射。
    pub fn insert_handle<K: Into<ResourceKey>>(
        &mut self,
        key: K,
        meta: T,
        mut handle: GpuHandle,
    ) -> &GpuHandle {
        let rk = key.into();
        let now = self.frame_clock;
        if !self.key_to_id.contains_key(&rk) {
            let id = self.alloc_slot();
            handle.assign_id(id);
            self.key_to_id.insert(rk, id);
            self.entries.insert(
                id,
                CacheEntry {
                    handle,
                    meta,
                    last_used_frame: now,
                },
            );
        }
        let id = *self.key_to_id.get(&rk).expect("inserted above");
        let entry = self.entries.get_mut(&id).expect("inserted above");
        entry.last_used_frame = now;
        &entry.handle
    }

    pub fn mark_dirty<K: Into<ResourceKey>>(&mut self, key: K) {
        let rk = key.into();
        if self.key_to_id.contains_key(&rk) {
            self.dirty.insert(rk);
        }
    }

    pub fn upload_dirty(
        &mut self,
        backend: &mut dyn RenderBackend,
        mut rebuild: impl FnMut(&ResourceKey, &T, &mut dyn RenderBackend) -> GpuHandle,
    ) -> usize {
        let keys: Vec<ResourceKey> = self.dirty.drain().collect();
        let now = self.frame_clock;
        let mut count = 0;
        for rk in keys {
            let Some(&id) = self.key_to_id.get(&rk) else {
                continue;
            };
            let Some(entry) = self.entries.get_mut(&id) else {
                continue;
            };
            let mut new_handle = rebuild(&rk, &entry.meta, backend);
            new_handle.assign_id(id);
            // 旧句柄投入延迟销毁，绝不直接 drop
            let old_handle = std::mem::replace(&mut entry.handle, new_handle);
            self.destroyer.schedule_destroy(old_handle);
            entry.last_used_frame = now;
            count += 1;
        }
        count
    }

    pub fn mark_used<K>(&mut self, key: &K) -> bool
    where
        K: Into<ResourceKey> + Copy,
    {
        let rk = (*key).into();
        let now = self.frame_clock;
        let Some(&id) = self.key_to_id.get(&rk) else {
            return false;
        };
        match self.entries.get_mut(&id) {
            Some(entry) => {
                entry.last_used_frame = now;
                true
            }
            None => false,
        }
    }

    pub fn evict_unused(&mut self, max_idle_frames: u64) -> usize {
        let now = self.frame_clock;
        // 找到需要 evict 的 id 列表
        let mut evict_ids: Vec<(ResourceKey, GpuResourceId)> = Vec::new();
        for (rk, id) in &self.key_to_id {
            if let Some(entry) = self.entries.get(id) {
                if entry.idle_frames(now) > max_idle_frames {
                    evict_ids.push((*rk, *id));
                }
            }
        }
        let count = evict_ids.len();
        for (rk, id) in evict_ids {
            self.key_to_id.remove(&rk);
            self.dirty.remove(&rk);
            // release_slot 内部会 schedule_destroy(handle) + 回收 slot
            self.release_slot(id);
        }
        count
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

    // ── G3 单测：graveyard 3 帧 ────────────────────────────────────────

    #[test]
    fn test_deferred_destroyer_graveyard_3() {
        let mut d = DeferredDestroyer::new();
        let h = GpuHandle::new(1_u32);
        d.schedule_destroy(h); // submitted_frame = 0
        for frame in 0..3 {
            // 模拟每帧 1 次 begin_frame + submit
            let dropped = d.begin_frame();
            d.on_submit();
            assert_eq!(dropped, 0, "frame {frame} should not drop");
        }
        // 第 4 次 begin_frame：completed = 3-3 = 0，submitted_at 0 <= 0，drop
        let dropped = d.begin_frame();
        assert_eq!(dropped, 1);
        assert_eq!(d.pending_count(), 0);
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
            cache.get_or_insert(
                key,
                (),
                |_, _| GpuHandle::new(7_u32),
                &mut StubBackend {},
            );
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
