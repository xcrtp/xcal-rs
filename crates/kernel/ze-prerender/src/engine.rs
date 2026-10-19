// engine.rs — ContentEngine trait（异步形状、同步实现）。
//
// 为什么使用这个形状：将来 LaTeX / SVG 必然异步。如果先给同步 render()，
// 改异步时每个调用点都要动。现在形状对了，同步偷懒即可。

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::cache::ContentCache;
use crate::request::{
    CacheKey, ContentConfig, ContentError, ContentHandle, ContentResult, ContentStatus,
};
use ze_content::{ContentIR, ContentSource, normalize_source};

/// 预渲染引擎 trait。
///
/// 调用流程：
/// 1. `request(source, config)` → 立刻拿到句柄，同步引擎内部此刻就完成编译写入槽位。
/// 2. `poll(handle)` → 查询是否就绪（当前实现在 request 时就 Ready）。
/// 3. `take(handle)` → 消费结果（Gone 语义，第二次调用返回 None）。
pub trait ContentEngine: Send + Sync {
    fn name(&self) -> &str;

    /// 提交请求，立即返回句柄；不阻塞。
    fn request(&self, source: &str, config: &ContentConfig) -> ContentHandle;

    /// 查询是否就绪。
    fn poll(&self, handle: ContentHandle) -> ContentStatus;

    /// 取结果（未就绪 / 已取走 → None）。
    fn take(&self, handle: ContentHandle) -> Option<ContentResult>;
}

/// 完成的请求槽（内部用）。
///
/// 当前实现是同步的（`request` 直接写入 `Ready`），`Pending` / `Gone` 是给
/// 未来异步引擎预留的状态：它们已有匹配分支与转移逻辑，但尚无构造点，
/// 故保留 `allow(dead_code)`（体检第 11 项：确认是"接口预留"而非漏接线）。
#[allow(dead_code)]
enum Slot {
    Pending,
    Ready(ContentResult),
    Gone,
}

/// 「编译一个请求」函数签名：输入归一化源码 + 配置，返回 IR 或错误。
///
/// 具体引擎（Placeholder / Typst / 未来 LaTeX）实现此函数后注入
/// `InMemoryEngine`，后者统一负责句柄、缓存、槽位管理。
pub type CompileFn = dyn Fn(&ContentSource, &ContentConfig) -> Result<ContentIR, ContentError>
    + Send
    + Sync
    + 'static;

/// 函数驱动的内存引擎：同步编译、请求级缓存、句柄槽位。
///
/// - Placeholder 引擎：注入 `placeholder::generate_placeholder`。
/// - Typst 引擎：注入 `typst::compile_with_world`（背后挂 TypstWorld，含字体与缓存）。
pub struct InMemoryEngine {
    name: String,
    compile: Arc<CompileFn>,
    handle_counter: AtomicU64,
    slots: RwLock<std::collections::BTreeMap<NonZeroU64, Slot>>,
    cache: Arc<ContentCache>,
}

impl InMemoryEngine {
    pub fn new(name: impl Into<String>, compile: Arc<CompileFn>) -> Self {
        Self {
            name: name.into(),
            compile,
            handle_counter: AtomicU64::new(1),
            slots: RwLock::new(std::collections::BTreeMap::new()),
            cache: Arc::new(ContentCache::new()),
        }
    }

    /// 挂外部缓存（便于 Typst 侧与 World 共享同一个缓存实例）。
    pub fn with_cache(mut self, cache: Arc<ContentCache>) -> Self {
        self.cache = cache;
        self
    }

    pub fn cache(&self) -> &Arc<ContentCache> {
        &self.cache
    }
}

impl ContentEngine for InMemoryEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn request(&self, source: &str, config: &ContentConfig) -> ContentHandle {
        let handle_val = self.handle_counter.fetch_add(1, Ordering::Relaxed);
        // 按计划 1.3：所有引擎使用同一套归一化。
        let normalized = normalize_source(source);
        let src = ContentSource::from_normalized(normalized);
        let key = CacheKey::new(&src, config);

        // 先查缓存。
        let cached = self.cache.get(&key);
        let result = match cached {
            Some(r) => r,
            None => {
                let r = (self.compile)(&src, config);
                self.cache.insert(key, r.clone());
                r
            }
        };

        let handle = ContentHandle(NonZeroU64::new(handle_val).expect("handle overflow"));
        self.slots
            .write()
            .expect("slots poisoned")
            .insert(handle.0, Slot::Ready(result));
        handle
    }

    fn poll(&self, handle: ContentHandle) -> ContentStatus {
        let slots = self.slots.read().expect("slots poisoned");
        match slots.get(&handle.0) {
            Some(Slot::Pending) => ContentStatus::Pending,
            Some(Slot::Ready(_)) => ContentStatus::Ready,
            None | Some(Slot::Gone) => ContentStatus::Gone,
        }
    }

    fn take(&self, handle: ContentHandle) -> Option<ContentResult> {
        let mut slots = self.slots.write().expect("slots poisoned");
        let Some(slot) = slots.remove(&handle.0) else {
            return None;
        };
        match slot {
            Slot::Ready(r) => Some(r),
            Slot::Pending => {
                slots.insert(handle.0, Slot::Pending);
                None
            }
            Slot::Gone => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder_compile(
        src: &ContentSource,
        cfg: &ContentConfig,
    ) -> Result<ContentIR, ContentError> {
        Ok(ze_content::placeholder::generate_placeholder(
            &src.text,
            &ze_content::placeholder::PlaceholderOptions {
                nominal_font_size: cfg.font_size,
                fill: cfg.fill,
                ..Default::default()
            },
        ))
    }

    #[test]
    fn request_then_take_is_ready() {
        let eng = InMemoryEngine::new("test", Arc::new(placeholder_compile));
        let h = eng.request("abc", &ContentConfig::default());
        assert!(matches!(eng.poll(h), ContentStatus::Ready));
        let ir = eng.take(h).unwrap().unwrap();
        assert_eq!(ir.items.len(), 3);
        // Gone 语义。
        assert!(eng.take(h).is_none());
        assert!(matches!(eng.poll(h), ContentStatus::Gone));
    }

    #[test]
    fn duplicate_request_hits_cache() {
        let eng = InMemoryEngine::new("t", Arc::new(placeholder_compile));
        let _h1 = eng.request("abc", &ContentConfig::default());
        assert_eq!(eng.cache().misses(), 1);
        let _h2 = eng.request("abc", &ContentConfig::default());
        assert_eq!(eng.cache().hits(), 1);
    }

    #[test]
    fn unknown_handle_returns_none() {
        let eng = InMemoryEngine::new("t", Arc::new(placeholder_compile));
        let bad = ContentHandle(NonZeroU64::new(9999).unwrap());
        assert!(eng.take(bad).is_none());
        assert!(matches!(eng.poll(bad), ContentStatus::Gone));
    }
}
