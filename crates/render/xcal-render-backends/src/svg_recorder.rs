// svg_recorder.rs
//
// SVG 录制后端（预留空壳）：
// 后续实现 Painter2D，把 Shape/Path2D 录制为 SVG 元素输出。
// 当前仅提供占位类型，待 2D 重构阶段填充。

use xc_geometry::Rect2D;

/// SVG 录制器（预留）。
///
/// 计划职责：接收 2D 绘制命令，序列化为 SVG 文档字符串/文件。
#[derive(Debug, Default)]
pub struct SvgRecorder {
    /// 输出画布范围（逻辑坐标）。
    pub viewport: Option<Rect2D>,
}

impl SvgRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置输出视口（预留）。
    pub fn set_viewport(&mut self, viewport: Rect2D) {
        self.viewport = Some(viewport);
    }
}
