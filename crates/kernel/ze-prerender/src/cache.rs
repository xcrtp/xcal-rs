// cache.rs — 内容哈希缓存。
//
// key = 归一化源码哈希 + 配置哈希。命中直接克隆 IR 返回。
// 缓存挂在引擎内部；Typst 侧的 World 还会利用 comemo 的记忆化做进一步去重。

use std::collections::HashMap;
use std::sync::RwLock;

use crate::request::{CacheKey, ContentResult};

/// 预渲染结果缓存（线程安全，内部读写锁）。
#[derive(Default)]
pub struct ContentCache {
    inner: RwLock<HashMap<CacheKey, ContentResult>>,
    /// 测试辅助：命中计数（debug only 统计）。
    #[cfg(test)]
    hits: std::sync::Mutex<u64>,
    #[cfg(test)]
    misses: std::sync::Mutex<u64>,
}

impl ContentCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询缓存；命中克隆并返回，同时统计命中。
    pub fn get(&self, key: &CacheKey) -> Option<ContentResult> {
        let r = self.inner.read().expect("cache poisoned").get(key).cloned();
        #[cfg(test)]
        {
            if r.is_some() {
                *self.hits.lock().unwrap() += 1;
            } else {
                *self.misses.lock().unwrap() += 1;
            }
        }
        r
    }

    /// 写入缓存。仅当结果为 Ok 或希望记住错误时才需要。
    pub fn insert(&self, key: CacheKey, value: ContentResult) {
        self.inner.write().expect("cache poisoned").insert(key, value);
    }

    /// 清空缓存。
    pub fn clear(&self) {
        self.inner.write().expect("cache poisoned").clear();
    }

    #[cfg(test)]
    pub fn hits(&self) -> u64 { *self.hits.lock().unwrap() }
    #[cfg(test)]
    pub fn misses(&self) -> u64 { *self.misses.lock().unwrap() }
}
