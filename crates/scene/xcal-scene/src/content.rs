// content.rs — 内容编译的**宿主侧适配**（排版引擎 → IR → 世界坐标轮廓）。
//
// 纯逻辑（IR → 展平子路径 → 世界坐标 / 描边回退 / 阅读顺序）已下沉
// `xc-content::stroke`，只剩一个真源；本模块只把 `xc-prerender` 的
// `request` / `take` 包成函数，供场景作者（storyboard）与 Pod 内容服务
// （`pod::content_service`）共用，避免两处各写一遍引擎调用。

use xc_content::{ContentIR, StrokeOutline};
use xc_notion::{Color32, Vec2};
use xc_prerender::{ContentConfig, ContentEngine, InMemoryEngine};

/// 用**标称字号**（`xc_content::DEFAULT_NOMINAL_FONT_SIZE`）编译源码为 IR。
///
/// 失败（编译错误）返回 `None`；调用方据此优雅降级。
pub fn build_ir(engine: &InMemoryEngine, source: &str) -> Option<ContentIR> {
    let config = ContentConfig {
        font_size: xc_content::DEFAULT_NOMINAL_FONT_SIZE,
        fill: Color32::WHITE,
        scale_factor: 1.0,
    };
    let handle = engine.request(source, &config);
    engine.take(handle).and_then(|result| result.ok())
}

/// 源码 → 世界坐标描边轮廓（编译 + `xc-content` 纯变换）。
///
/// `font_size` 是**目标显示**字号（世界单位）；`world_pos` 是 IR 原点的落点。
/// `sort_reading_order` 为 `true` 时按包围盒 `min_x` 升序（书写顺序）。
pub fn compile(
    engine: &InMemoryEngine,
    source: &str,
    font_size: f32,
    world_pos: Vec2,
    sort_reading_order: bool,
) -> Vec<StrokeOutline> {
    build_ir(engine, source)
        .map(|ir| xc_content::stroke_outlines(&ir, font_size, world_pos, sort_reading_order))
        .unwrap_or_default()
}
