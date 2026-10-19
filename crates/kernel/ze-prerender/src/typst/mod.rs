// typst/ — Typst 引擎接入。
//
// 模块结构：
//   - mod.rs    ：暴露引擎入口 `engine()` + 顶层 compile 函数（对齐 CompileFn）。
//   - world.rs  ：`TypstWorld`（实现 typst::World）+ font/Source 管理。
//   - convert.rs：Frame → ContentIR（Text/Shape/Group 三类 FrameItem 的递归转换、锚点、样式提取）。

pub mod convert;
pub mod world;

use self::world::TypstWorld;

use crate::engine::{CompileFn, InMemoryEngine};
use crate::request::{ContentConfig, ContentError, ContentResult};
use std::sync::Arc;
use ze_content::ContentSource;

/// Typst 编译一次请求：归一化源码 → 包入 `$ $` 数学块设定字号 → 编译为 Frame → 转换 IR。
///
/// 数学模式是我们主场景。如果调用者明确传入了 Typst 正文（含 `$`）则按正文原样渲染。
/// 为了简单、统一行为：源码如不以 `$` 开头，则自动用 `$ <源码> $` 包装为 inline math。
pub fn compile(
    world_provider: &Arc<dyn Fn() -> TypstWorld + Send + Sync>,
    src: &ContentSource,
    cfg: &ContentConfig,
) -> ContentResult {
    let raw_body = wrap_if_math(&src.text);
    let font_size_pt = cfg.font_size.max(1.0);
    let rgb_hex = format!(
        "\"#{:02x}{:02x}{:02x}\"",
        cfg.fill.r, cfg.fill.g, cfg.fill.b
    );
    let fs_pt = format!("{}pt", font_size_pt);
    // 前缀 = #set page + #set text 行；body 紧随其后。
    let prefix = format!(
        "#set page(width: auto, height: auto, margin: 0pt)\n\
         #set text(size: {fs}, fill: rgb({color}))\n",
        fs = fs_pt,
        color = rgb_hex,
    );
    // body_offset = 用户原始归一化源码在 wrapped_src 中的字节偏移。
    // wrap_if_math 非数学包装时加 "$ " 前缀（2 字节），需加上。
    let body_inner_offset = if src.text.starts_with('$') { 0 } else { 2 };
    let body_offset = prefix.len() + body_inner_offset;
    let wrapped_src = format!("{}{}\n", prefix, raw_body);

    // 生成 TypstWorld：给 main source 包进去。
    let world = world_provider();
    world.set_main_source(wrapped_src.clone());
    world.set_original_source(src.text.clone());
    world.set_body_offset(body_offset);

    if std::env::var_os("XC_PRERENDER_DUMP_SRC").is_some() {
        tracing::debug!(
            "[ze-prerender::typst] wrapped source:\n----------\n{}\n----------",
            wrapped_src
        );
    }

    let warned = typst::compile::<typst::layout::PagedDocument>(&world);
    let doc = warned
        .output
        .map_err(|errs| ContentError::CompileFailed(format_diags(&errs)))?;

    let page = doc
        .pages
        .into_iter()
        .next()
        .ok_or_else(|| ContentError::CompileFailed("typst 输出为空".into()))?;

    convert::frame_to_ir(&world, page.frame, src, cfg)
}

fn wrap_if_math(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('$') {
        s.to_string()
    } else {
        format!("$ {} $", t)
    }
}

fn format_diags(errs: &ecow::EcoVec<typst::diag::SourceDiagnostic>) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for e in errs {
        // Span::number() 为 pub(crate)；用 Debug 格式化，可近似定位。
        let _ = writeln!(s, "[{:?}] {:?}: {}", e.severity, e.span, e.message);
    }
    s
}

/// 构造 Typst 引擎实例。字体与 Library 会随引擎长期持有以复用。
///
/// 注：同一引擎实例内多次编译共享同一个 TypstWorld，
/// 但每次请求会替换其 main source（内部用 RwLock，代价可控）。
/// Typst 的 comemo 会自动做记忆化，相同源码重编译近乎免费。
pub fn engine() -> Result<InMemoryEngine, ContentError> {
    let world_builder: Arc<dyn Fn() -> TypstWorld + Send + Sync> = Arc::new(|| TypstWorld::new());
    // 预先构建一次做健康检查：Library/FontBook 是否可以构造。
    let probe = world_builder();
    use typst::World as _;
    if probe.book().families().next().is_none() {
        // 仍允许继续——只是复杂数学公式可能缺字形。
        // 不返回 Err，避免用户机器无 Latin Modern Math 就起不来。
    }
    let shared = Arc::clone(&world_builder);
    let compile_fn: Arc<CompileFn> = Arc::new(move |src, cfg| compile(&shared, src, cfg));
    Ok(InMemoryEngine::new("typst", compile_fn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentEngine;
    use crate::request::ContentConfig;
    use ze_content::ContentSource;
    use ze_notion::Color32;

    fn cfg(font_size: f32) -> ContentConfig {
        ContentConfig {
            font_size,
            fill: Color32::BLACK,
            scale_factor: 1.0,
        }
    }

    /// 能编译出 x^2 + 1，并且 items 非空、bounds 有限。
    #[test]
    fn typst_compiles_x2_plus_1() {
        let engine = engine().expect("typst engine");
        let h = engine.request("x^2 + 1", &cfg(24.0));
        let ir = engine.take(h).expect("ready").expect("ok");
        assert!(
            !ir.items.is_empty(),
            "预期至少一个 glyph path；实际 items=0"
        );
        let b = ir.bounds;
        assert!(
            b.min.x.is_finite() && b.max.x > b.min.x,
            "x bounds 应当有限: {:?}",
            b
        );
        assert!(
            b.min.y.is_finite() && b.max.y > b.min.y,
            "y bounds 应当有限: {:?}",
            b
        );
        assert!(ir.nominal_font_size > 0.0);
    }

    /// 复杂公式（Typst 数学语法，含求和、上下标）可编译且 items 非空。
    #[test]
    fn typst_compiles_complex_sum_formula() {
        let engine = engine().expect("typst engine");
        let formula = "sum_(i=1)^n i = (n (n + 1)) / 2";
        let h = engine.request(formula, &cfg(20.0));
        let ir = engine.take(h).expect("ready").expect("ok");
        assert!(!ir.items.is_empty(), "复杂公式预期多个 glyph；实际 0");
        let size = ir.bounds.size();
        assert!(size.x > 0.0 && size.y > 0.0, "bounds 尺寸: {:?}", size);
    }

    /// 锚点回切：`x^2 + 1` 的 item.anchor 中至少 2 个能切回归一化原文。
    #[test]
    fn typst_anchors_slice_back_to_source() {
        let engine = engine().expect("typst engine");
        let h = engine.request("x^2 + 1", &cfg(24.0));
        let ir = engine.take(h).unwrap().unwrap();
        let norm = &ir.source.text;

        let mut hits: Vec<String> = Vec::new();
        for it in &ir.items {
            let Some(a) = it.anchor else { continue };
            if let Some(slice) = norm.get(a.start as usize..a.end as usize) {
                if slice.chars().any(|c| !c.is_whitespace()) {
                    hits.push(slice.to_string());
                }
            }
        }
        let unique: std::collections::BTreeSet<String> = hits.into_iter().collect();
        assert!(
            unique.len() >= 2,
            "锚点回切唯一 token 过少：{:?}（norm={:?}）",
            unique,
            norm
        );
    }

    /// 相同源码两次 request：handle 不同；ir items 长度与归一化 hash 相同。
    #[test]
    fn typst_engine_cache_dedupes_requests() {
        let engine = engine().expect("typst engine");
        let h1 = engine.request("1 + 2 = 3", &cfg(18.0));
        let h2 = engine.request("1 + 2 = 3", &cfg(18.0));
        assert_ne!(h1, h2);
        let ir1 = engine.take(h1).unwrap().unwrap();
        let ir2 = engine.take(h2).unwrap().unwrap();
        assert_eq!(ir1.items.len(), ir2.items.len());
        assert_eq!(ir1.source.hash, ir2.source.hash);
        assert!(!ir1.items.is_empty());
    }

    /// compile 函数幂等 + 输出 SVG 快照到临时目录。
    #[test]
    fn compile_fn_idempotent_and_writes_svg_snapshot() {
        let world_provider: Arc<dyn Fn() -> TypstWorld + Send + Sync> =
            Arc::new(|| TypstWorld::new());
        let src = ContentSource::from_raw("a/b");
        let conf = cfg(14.0);
        let ir1 = compile(&world_provider, &src, &conf).expect("compile 1");
        let ir2 = compile(&world_provider, &src, &conf).expect("compile 2");
        assert_eq!(ir1.items.len(), ir2.items.len());
        assert!(!ir1.items.is_empty());
        let svg = ze_content::svg::to_svg(&ir1, &Default::default());
        let path = std::env::temp_dir().join("ze_prerender_typst_snapshot_a_div_b.svg");
        std::fs::write(&path, svg).expect("写 SVG 快照失败");
        eprintln!("[typst test] SVG snapshot: {}", path.display());
    }
}
