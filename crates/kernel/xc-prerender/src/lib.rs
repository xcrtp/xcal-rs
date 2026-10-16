// xc-prerender: 排版引擎接入层（kernel 重 crate）。
//
// 依赖方向：xc-notion ← xc-content ← xc-prerender。
// 铁律：不能依赖任何 graph / render / ui / app。
// 重引擎依赖（typst 等）必须放在 feature 后；默认 feature 关闭应能独立编译。

pub mod cache;
pub mod engine;
pub mod placeholder;
pub mod request;

#[cfg(feature = "typst")]
pub mod typst;

pub use cache::ContentCache;
pub use engine::{CompileFn, ContentEngine, InMemoryEngine};
pub use placeholder::engine as placeholder_engine;
pub use request::{
    CacheKey, ContentConfig, ContentError, ContentHandle, ContentResult, ContentStatus,
};

// typst 模式下重导出入口。
#[cfg(feature = "typst")]
pub use self::typst::engine as typst_engine;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn placeholder_engine_flow() {
        let eng = placeholder_engine();
        let h = eng.request("x^2", &ContentConfig::default());
        assert!(matches!(eng.poll(h), ContentStatus::Ready));
        let ir = eng.take(h).unwrap().unwrap();
        assert!(!ir.items.is_empty());
        // 锚点能回切。
        for sp in &ir.items {
            let a = sp.anchor.unwrap();
            assert!(!a.slice(&ir.source.text).is_empty());
        }
    }

    #[test]
    fn caching_dedupes_compile() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let counter = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&counter);
        let compile_fn: Arc<CompileFn> = Arc::new(move |s, cfg| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(xc_content::placeholder::generate_placeholder(
                &s.text,
                &xc_content::placeholder::PlaceholderOptions {
                    nominal_font_size: cfg.font_size,
                    fill: cfg.fill,
                    ..Default::default()
                },
            ))
        });
        let eng = InMemoryEngine::new("c", compile_fn);
        let _a = eng.request("abc", &ContentConfig::default());
        let _b = eng.request("abc", &ContentConfig::default());
        let _c = eng.request("abx", &ContentConfig::default());
        assert_eq!(counter.load(Ordering::SeqCst), 2); // abc / abx 各一次
    }
}
