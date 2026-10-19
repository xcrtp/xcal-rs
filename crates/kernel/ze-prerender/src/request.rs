// request.rs — 请求 / 句柄 / 就绪查询 / 错误 / 配置。
//
// API 按异步形状设计：request() 立即返回句柄，poll() 查询状态，take() 消费结果。
// 当前实现内部可以同步完成（首次 poll 即已就绪），将来换真正调度无需改调用点。

use std::num::NonZeroU64;
use ze_content::ContentIR;
use ze_notion::Color32;

/// 不透明句柄。内部单调递增整数，跨引擎进程内唯一（同一引擎下）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, derive_more::From)]
pub struct ContentHandle(pub NonZeroU64);

/// 排版请求配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ContentConfig {
    /// 标称字号（pt）。
    pub font_size: f32,
    /// 基础文本颜色。
    pub fill: Color32,
    /// 显示 DPI 缩放（当前主要给后端渲染层用；IR 始终以 pt 为单位）。
    pub scale_factor: f32,
}

impl Default for ContentConfig {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            fill: Color32::BLACK,
            scale_factor: 1.0,
        }
    }
}

/// 句柄对应的请求状态。
#[derive(Debug, Clone)]
pub enum ContentStatus {
    /// 仍在处理中。
    Pending,
    /// 结果就绪，可调用 take() 取出。
    Ready,
    /// 句柄已失效（已被 take 或未分配）。
    Gone,
}

/// 预渲染错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ContentError {
    #[error("引擎编译失败: {0}")]
    CompileFailed(String),
    #[error("未知句柄")]
    UnknownHandle,
    #[error("引擎实现错误: {0}")]
    Internal(String),
}

/// 缓存键：归一化源码哈希 + 配置哈希（字号、颜色等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub source_hash: u64,
    pub config_hash: u64,
}

impl CacheKey {
    pub fn new(source: &ze_content::ContentSource, config: &ContentConfig) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        config.font_size.to_bits().hash(&mut h);
        config.fill.to_array().hash(&mut h);
        config.scale_factor.to_bits().hash(&mut h);
        let config_hash = h.finish();
        Self { source_hash: source.hash, config_hash }
    }
}

/// 已完成的结果（缓存中或已 resolve）。
pub type ContentResult = Result<ContentIR, ContentError>;
