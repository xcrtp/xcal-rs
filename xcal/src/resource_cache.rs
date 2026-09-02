// resource_cache.rs
//
// 资源缓存管理器：CPU 端数据 → GPU 资源（GpuHandle）的按需上传与淘汰。
//
// 设计要点：
// - CacheKey = 组件类型(TypeId) + 实体 ID(hecs::Entity，含 generation 防复用)
//   + 版本计数(u64，CPU 数据变更时 +1)。版本进 key 意味着"变更 = 换 key"，
//   旧条目成为孤儿，由 evict_unused 在 max_idle_frames 帧后回收。
// - 淘汰策略：帧戳线性扫描（条目通常 < 千级，O(n) 远小于一次 GPU 上传）。
//   wgpu 30 下 GpuHandle 移出 map（drop）即完成资源释放，无需淘汰回调。
// - T 是轻量元数据（如 byte_len/format），不存 CPU 源数据——源数据的
//   owner 是组件本身，缓存里再留一份是双倍浪费。
// - mark_dirty/upload_dirty 是可选的原地更新路径：同一 GpuHandle 内
//   queue.write_buffer 更新（适合高频变更 mesh）。不使用 dirty 流程时，
//   "version 换 key → 新分配 → 旧条目被 evict" 也能正确工作。

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::render_backend::{GpuHandle, RenderBackend};

// ─── CacheKey ─────────────────────────────────────────────────────────────

/// 缓存键：组件类型 + 实体 ID + 版本计数。
///
/// - `type_id`：组件类型（如 Mesh、Texture），
/// - `entity`：hecs 实体（内含 generation，实体回收复用后 key 自然不同），
/// - `version`：CPU 数据版本计数，组件每次变更 +1（由持有组件的 System 维护）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheKey {
    pub type_id: TypeId,
    pub entity: hecs::Entity,
    pub version: u64,
}

impl CacheKey {
    /// 构造指定组件类型的缓存键。
    pub fn of<C: 'static>(entity: hecs::Entity, version: u64) -> Self {
        Self {
            type_id: TypeId::of::<C>(),
            entity,
            version,
        }
    }
}

// ─── 缓存条目 ─────────────────────────────────────────────────────────────

/// 单条缓存：GPU 句柄 + 元数据 + 帧戳。
pub struct CacheEntry<T> {
    pub handle: GpuHandle,
    pub meta: T,
    last_used_frame: u64,
}

impl<T> CacheEntry<T> {
    /// 距当前帧的使用间隔。
    pub fn idle_frames(&self, current_frame: u64) -> u64 {
        current_frame - self.last_used_frame
    }
}

// ─── ResourceCache ────────────────────────────────────────────────────────

/// GPU 资源缓存管理器。
///
/// 帧流程约定：
/// 1. 每帧开头调用 [`begin_frame`](Self::begin_frame) 推进内部帧时钟；
/// 2. Prepare 阶段 System 通过 [`get_or_insert`](Self::get_or_insert)
///    按需上传（miss 时调用 upload 闭包）或 [`upload_dirty`](Self::upload_dirty)
///    原地重建标记过的条目；
/// 3. RenderSubmit 阶段 System 通过 [`get`](Self::get) 取 `&GpuHandle` 下发绘制；
/// 4. 帧末（或 Prepare 开头）调用 [`evict_unused`](Self::evict_unused) 淘汰孤儿。
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

    /// 推进帧时钟（每帧调用一次）。
    pub fn begin_frame(&mut self) {
        self.frame_clock += 1;
    }

    /// 当前帧时钟值。
    pub fn current_frame(&self) -> u64 {
        self.frame_clock
    }

    // ─── 查询 ────────────────────────────────────────────

    /// 读缓存命中（不更新帧戳；RenderSubmit 阶段用）。
    pub fn get(&self, key: &CacheKey) -> Option<&GpuHandle> {
        self.entries.get(key).map(|e| &e.handle)
    }

    /// 缓存是否包含该键。
    pub fn contains(&self, key: &CacheKey) -> bool {
        self.entries.contains_key(key)
    }

    /// 条目总数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// dirty 标记数量。
    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }

    // ─── 上传（Prepare 阶段）─────────────────────────────

    /// 取缓存，miss 时调用 `upload` 上传并插入。自动 mark_used。
    ///
    /// 返回 `&GpuHandle`（handle 不可 Clone，返回引用避免装箱拷贝）。
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

    /// 标记条目为 dirty（原地重建路径）。
    pub fn mark_dirty(&mut self, key: CacheKey) {
        if self.entries.contains_key(&key) {
            self.dirty.insert(key);
        }
    }

    /// 重建所有 dirty 条目：调用 `rebuild` 生成新 handle 替换旧值。
    /// 返回重建数量。跳过 dirty 集合中已不存在的键（如已被淘汰）。
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

    // ─── 帧戳与淘汰 ──────────────────────────────────────

    /// 手动刷新条目的使用帧戳（get_or_insert 之外的使用路径用）。
    /// 返回条目是否存在。
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

    /// 淘汰连续 `max_idle_frames` 帧未使用的条目，返回淘汰数量。
    ///
    /// version 换 key 产生的孤儿条目由此回收；dirty 集合中的死键同步清理。
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

/// 最简 GPU 资源缓存（T = `()`，无元数据）。
pub type GpuResourceCache = ResourceCache<()>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成一个 hecs::Entity 作 key 用（world 立即 drop，值语义无碍）。
    fn dummy_entity(n: u32) -> hecs::Entity {
        let mut world = hecs::World::new();
        let mut e = world.spawn(());
        for _ in 0..n {
            e = world.spawn(());
        }
        e
    }

    // ─── CacheKey ────────────────────────────────────────

    #[test]
    fn test_cachekey_of_uses_typeid() {
        struct Mesh;
        struct Texture;
        let e = dummy_entity(0);
        let k1 = CacheKey::of::<Mesh>(e, 0);
        let k2 = CacheKey::of::<Texture>(e, 0);
        let k3 = CacheKey::of::<Mesh>(e, 1);
        assert_ne!(k1, k2, "组件类型不同 → key 不同");
        assert_ne!(k1, k3, "版本不同 → key 不同");
        assert_eq!(k1, CacheKey::of::<Mesh>(e, 0), "同类型同版本 → key 相同");
    }

    // ─── get_or_insert ───────────────────────────────────

    #[test]
    fn test_get_or_insert_miss_then_hit() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(1);
        let key = CacheKey::of::<u32>(e, 0);

        let mut upload_count = 0;
        // miss：调用 upload
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

        // hit：不再调用 upload，返回同一资源
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
        assert_eq!(upload_count, 1, "命中缓存不应重复上传");
        let h2 = cache.get(&key).map(|h| *h.downcast_ref::<u32>().unwrap());
        assert_eq!(h1, Some(7));
        assert_eq!(h2, Some(7), "命中返回原 handle");
    }

    #[test]
    fn test_version_change_isolates_entries() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(2);

        let key_v0 = CacheKey::of::<u32>(e, 0);
        let key_v1 = CacheKey::of::<u32>(e, 1);

        cache.get_or_insert(key_v0, (), |_, _| GpuHandle::new(10_u32), &mut StubBackend {});
        cache.get_or_insert(key_v1, (), |_, _| GpuHandle::new(20_u32), &mut StubBackend {});

        assert_eq!(cache.len(), 2, "新旧版本是两个独立条目");
        assert_eq!(
            cache.get(&key_v0).and_then(|h| h.downcast_ref::<u32>()),
            Some(&10)
        );
        assert_eq!(
            cache.get(&key_v1).and_then(|h| h.downcast_ref::<u32>()),
            Some(&20)
        );

        // v0 不再使用；v1 每帧继续使用，3 帧后 v0 被淘汰
        for _ in 0..3 {
            cache.begin_frame();
            cache.mark_used(&key_v1);
        }
        let evicted = cache.evict_unused(1);
        assert_eq!(evicted, 1);
        assert!(cache.get(&key_v0).is_none());
        assert!(cache.get(&key_v1).is_some(), "仍被使用的条目保留");
    }

    // ─── mark_used / evict_unused ────────────────────────

    #[test]
    fn test_mark_used_prevents_evict() {
        let mut cache = ResourceCache::<()>::new();
        let e = dummy_entity(3);
        let key = CacheKey::of::<u32>(e, 0);

        cache.begin_frame();
        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        // 3 帧不使用但每帧 mark_used
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
        assert_eq!(cache.dirty_len(), 0, "孤儿条目的 dirty 死键同步清理");
        assert!(!cache.mark_used(&key), "已淘汰条目 mark_used 返回 false");
    }

    // ─── mark_dirty / upload_dirty ───────────────────────

    #[test]
    fn test_mark_dirty_and_upload_dirty() {
        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(5);
        let key = CacheKey::of::<u32>(e, 0);

        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        cache.mark_dirty(key);
        assert_eq!(cache.dirty_len(), 1);

        // 重建：新 handle 值为 42
        let rebuilt = cache.upload_dirty(&mut StubBackend {}, |key, _, _| {
            assert_eq!(key.entity, e);
            GpuHandle::new(42_u32)
        });
        assert_eq!(rebuilt, 1);
        assert_eq!(cache.dirty_len(), 0);
        assert_eq!(
            cache.get(&key).and_then(|h| h.downcast_ref::<u32>()),
            Some(&42),
            "dirty 重建后 handle 被替换"
        );
    }

    #[test]
    fn test_upload_dirty_skips_evicted_keys() {
        let mut cache = ResourceCache::<()>::new();
        let e = dummy_entity(6);
        let key = CacheKey::of::<u32>(e, 0);

        cache.begin_frame();
        cache.get_or_insert(key, (), |_, _| GpuHandle::new(1_u32), &mut StubBackend {});

        // 淘汰条目但不经 mark_dirty 清理（模拟外部 remove 路径不存在的情况：
        // 先 mark_dirty 再淘汰）
        cache.mark_dirty(key);
        cache.begin_frame();
        cache.begin_frame();
        cache.begin_frame();
        cache.evict_unused(1);
        // dirty 里的死键已在 evict 时清理
        assert_eq!(cache.dirty_len(), 0);
        assert_eq!(cache.upload_dirty(&mut StubBackend {}, |_, _, _| GpuHandle::new(0_u32)), 0);
    }

    // ─── headless GPU 集成：真 wgpu Buffer 存入缓存 ──────

    #[test]
    fn test_headless_upload_and_retrieve() {
        let Ok(mut backend) = crate::render_backend::EframeWgpuBackend::headless() else {
            return; // 无 GPU 环境跳过
        };

        let mut cache = ResourceCache::<()>::new();
        cache.begin_frame();
        let e = dummy_entity(7);
        let key = CacheKey::of::<()>(e, 0);

        // Prepare：上传顶点数据为 wgpu::Buffer 并缓存
        let vertices: [f32; 6] = [0.0, 0.5, -0.5, -0.5, 0.5, -0.5];
        let handle = cache.get_or_insert(
            key,
            (),
            |_, backend| {
                let buf = backend.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("cached-mesh"),
                    size: (vertices.len() * 4) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                backend.queue().write_buffer(
                    buf.downcast_ref::<wgpu::Buffer>().unwrap(),
                    0,
                    bytemuck_bytes(&vertices),
                );
                buf
            },
            &mut backend,
        );
        assert!(handle.downcast_ref::<wgpu::Buffer>().is_some());

        // RenderSubmit：命中取回
        let again = cache.get(&key).expect("cached");
        assert!(again.downcast_ref::<wgpu::Buffer>().is_some());
    }

    /// 把 &[f32] 转字节切片（避免引入 bytemuck 依赖）。
    fn bytemuck_bytes(f32s: &[f32]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(f32s.as_ptr() as *const u8, f32s.len() * 4)
        }
    }

    // ─── Stub 后端（纯逻辑测试不依赖 GPU）────────────────

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
        fn create_pipeline(
            &self,
            _desc: &wgpu::RenderPipelineDescriptor<'_>,
        ) -> GpuHandle {
            GpuHandle::new(0_u32)
        }
        fn set_target(&mut self, _view: Option<wgpu::TextureView>) {}
        fn target(&self) -> Option<&wgpu::TextureView> {
            None
        }
    }
}
