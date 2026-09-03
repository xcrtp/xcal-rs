// text_engine.rs
//
// 文本渲染引擎抽象（预留）：公式/文本后端（Typst、LaTeX）实现 TextRenderEngine，
// 将 TextInput 排版为 TextRenderOutput（位图覆盖层 或 三角化矢量）。
//
// MVE 阶段只定义 trait 与数据类型，具体实现在 xcal-render-backends（feature = "text"）。

use xc_geometry::{Rect2D, Vec2};

/// 文本渲染错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum TextRenderError {
    #[error("文本引擎编译失败: {0}")]
    CompileFailed(String),
    #[error("文本引擎未就绪: {0}")]
    NotReady(String),
    #[error("不支持的输出格式: {0}")]
    UnsupportedFormat(String),
}

/// 位图像素格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitmapFormat {
    /// 8-bit 灰度（仅 alpha）。
    A8,
    /// RGBA 8-bit/通道。
    Rgba8,
}

/// 位图覆盖层：排版结果作为一张 2D 位图，叠加到画布的 `offset` 位置。
#[derive(Debug, Clone)]
pub struct BitmapOverlay {
    /// 像素数据（按 `format` 解释）。
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: BitmapFormat,
    /// 在画布逻辑坐标中的左上角偏移。
    pub offset: Vec2,
}

/// 排版配置。
#[derive(Debug, Clone)]
pub struct TextConfig {
    /// 字体大小（逻辑像素）。
    pub font_size: f32,
    /// 目标显示缩放（HiDPI）。
    pub scale_factor: f32,
    /// 文本颜色（sRGB）。
    pub color: xc_geometry::Color32,
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            scale_factor: 1.0,
            color: xc_geometry::Color32::BLACK,
        }
    }
}

/// 文本/公式输入。
#[derive(Debug, Clone)]
pub struct TextInput {
    /// 源码（Typst/LaTeX 语法，由具体引擎解释）。
    pub source: String,
    /// 排版配置。
    pub config: TextConfig,
}

impl TextInput {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            config: TextConfig::default(),
        }
    }

    pub fn with_config(mut self, config: TextConfig) -> Self {
        self.config = config;
        self
    }
}

/// 排版输出。
#[derive(Debug, Clone)]
pub enum TextRenderOutput {
    /// 位图覆盖（栅格化路径）。
    Bitmap(BitmapOverlay),
    /// 矢量路径（路径化文字/公式，由 2D 后端光栅化）。
    Paths {
        paths: Vec<xc_geometry::Path2D>,
        bounds: Rect2D,
    },
}

/// 文本渲染引擎 trait（Typst / LaTeX 后端实现）。
///
/// 引擎负责把 [`TextInput`] 编译为 [`TextRenderOutput`]；调用方
/// （渲染层）负责把输出提交给 2D 后端绘制。
pub trait TextRenderEngine: Send + Sync {
    /// 引擎名称（调试/日志用）。
    fn name(&self) -> &str;

    /// 同步排版（MVE 阶段足够；后续可扩展异步缓存版本）。
    fn render(&self, input: &TextInput) -> Result<TextRenderOutput, TextRenderError>;
}
