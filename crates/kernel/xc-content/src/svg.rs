// svg.rs — IR → SVG 文本序列化（导出 + 测试锚点）。
// 要求：输出确定性（固定浮点精度 + 稳定迭代顺序）；锚点写入 data-span 属性。

use crate::ir::{ContentIR, StyledPath};
use xc_notion::{PathCommand, Vec2};

/// SVG 序列化选项。
#[derive(Debug, Clone)]
pub struct SvgOptions {
    /// 浮点精度（小数位，默认 3）。
    pub precision: usize,
    /// 是否输出 data-span 锚点属性。
    pub include_anchors: bool,
    /// 是否输出 <svg> 根与 viewBox（false = 仅路径集合）。
    pub include_viewbox: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self { precision: 3, include_anchors: true, include_viewbox: true }
    }
}

pub fn to_svg(ir: &ContentIR, opts: &SvgOptions) -> String {
    let mut out = String::new();
    let p = opts.precision.min(10);
    if opts.include_viewbox {
        let b = ir.bounds;
        let w = (b.max.x - b.min.x).abs();
        let h = (b.max.y - b.min.y).abs();
        let pad = 1.0;
        out.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{:.p$} {:.p$} {:.p$} {:.p$}\">\n",
            b.min.x - pad, b.min.y - pad, w + pad * 2.0, h + pad * 2.0, p = p
        ));
    }
    for item in &ir.items {
        write_styled_path(&mut out, item, opts);
    }
    if opts.include_viewbox {
        out.push_str("</svg>\n");
    }
    out
}

fn write_styled_path(out: &mut String, sp: &StyledPath, opts: &SvgOptions) {
    out.push_str("  <path d=\"");
    write_path_data(out, &sp.path.commands, opts.precision);
    out.push('"');
    match sp.fill {
        Some(c) if c.a > 0 => {
            out.push_str(" fill=\"#");
            write_hex(out, c);
            out.push('"');
            if c.a < 255 {
                out.push_str(&format!(" fill-opacity=\"{}\"", c.a as f32 / 255.0));
            }
        }
        _ => out.push_str(" fill=\"none\""),
    }
    if let Some(s) = sp.stroke {
        if s.is_visible() {
            out.push_str(" stroke=\"#");
            write_hex(out, s.color);
            out.push('"');
            out.push_str(&format!(" stroke-width=\"{:.p$}\"", s.width, p = opts.precision));
        }
    }
    if opts.include_anchors {
        if let Some(span) = sp.anchor {
            out.push_str(&format!(" data-span=\"{}:{}\"", span.start, span.end));
        }
    }
    out.push_str("/>\n");
}

fn write_path_data(out: &mut String, cmds: &[PathCommand], p: usize) {
    let mut first = true;
    for cmd in cmds {
        if !first { out.push(' '); }
        first = false;
        match *cmd {
            PathCommand::MoveTo(v) => out.push_str(&format!("M {:.p$},{:.p$}", v.x, v.y, p = p)),
            PathCommand::LineTo(v) => out.push_str(&format!("L {:.p$},{:.p$}", v.x, v.y, p = p)),
            PathCommand::QuadTo { ctrl, end } => out.push_str(&format!(
                "Q {:.p$},{:.p$} {:.p$},{:.p$}", ctrl.x, ctrl.y, end.x, end.y, p = p)),
            PathCommand::CubicTo { ctrl1, ctrl2, end } => out.push_str(&format!(
                "C {:.p$},{:.p$} {:.p$},{:.p$} {:.p$},{:.p$}",
                ctrl1.x, ctrl1.y, ctrl2.x, ctrl2.y, end.x, end.y, p = p)),
            PathCommand::Arc { center, radii, rotation, start_angle, end_angle } => {
                write_svg_arc(out, center, radii, rotation, start_angle, end_angle, p);
            }
            PathCommand::Close => out.push('Z'),
        }
    }
}

/// 参数化弧（中心+半径+起止角）→ SVG A 命令（终点+标志位形式）。
fn write_svg_arc(out: &mut String, center: Vec2, radii: Vec2, rot: f32, sa: f32, ea: f32, p: usize) {
    let end = arc_point(center, radii, rot, ea);
    let delta = ea - sa;
    let large_arc = if delta.abs() > std::f32::consts::PI { 1 } else { 0 };
    let sweep = if delta > 0.0 { 1 } else { 0 };
    out.push_str(&format!("A {:.p$} {:.p$} {:.p$} {} {} {:.p$},{:.p$}",
        radii.x, radii.y, rot.to_degrees(), large_arc, sweep, end.x, end.y, p = p));
}

fn arc_point(center: Vec2, radii: Vec2, rot: f32, angle: f32) -> Vec2 {
    let (s, c) = rot.sin_cos();
    let l = Vec2::new(radii.x * angle.cos(), radii.y * angle.sin());
    Vec2::new(center.x + c * l.x - s * l.y, center.y + s * l.x + c * l.y)
}

fn write_hex(out: &mut String, c: xc_notion::Color32) {
    out.push_str(&format!("{:02x}{:02x}{:02x}", c.r, c.g, c.b));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ContentSource, Span};
    use xc_notion::{Color32, Path2D, Stroke2D, Vec2};

    fn sample_ir() -> ContentIR {
        let src = ContentSource::from_normalized("ab");
        let mut ir = ContentIR::empty(src, 12.0);
        let mut p1 = Path2D::new();
        p1.move_to(Vec2::ZERO)
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close();
        ir.items.push(StyledPath::filled(p1, Color32::from_rgb(0xFF0000)).with_anchor(Span::new(0, 1)));
        let mut p2 = Path2D::new();
        p2.move_to(Vec2::new(20.0, 20.0)).quad_to(Vec2::new(25.0, 30.0), Vec2::new(30.0, 20.0));
        ir.items.push(StyledPath::stroked(p2, Stroke2D::new(Color32::from_rgb(0x0000FF), 1.5))
            .with_anchor(Span::new(1, 2)));
        ir.recompute_bounds();
        ir.baseline = 10.0;
        ir
    }

    #[test]
    fn deterministic_output() {
        let ir = sample_ir();
        let o = SvgOptions::default();
        assert_eq!(to_svg(&ir, &o), to_svg(&ir, &o), "同一 IR 两次序列化必须逐字节相等");
    }

    #[test]
    fn path_count_and_attributes() {
        let svg = to_svg(&sample_ir(), &SvgOptions::default());
        assert_eq!(svg.matches("<path").count(), 2);
        assert!(svg.contains("data-span=\"0:1\""));
        assert!(svg.contains("data-span=\"1:2\""));
        assert!(svg.contains("fill=\"#ff0000\""));
        assert!(svg.contains("stroke=\"#0000ff\""));
        assert!(svg.contains("stroke-width=\"1.500\""));
    }

    #[test]
    fn anchor_toggle() {
        let ir = sample_ir();
        let a = to_svg(&ir, &SvgOptions { include_anchors: true, ..Default::default() });
        let b = to_svg(&ir, &SvgOptions { include_anchors: false, ..Default::default() });
        assert!(a.contains("data-span"));
        assert!(!b.contains("data-span"));
    }

    #[test]
    fn precision_effect() {
        let ir = sample_ir();
        let coarse = to_svg(&ir, &SvgOptions { precision: 1, ..Default::default() });
        assert!(!coarse.contains("1.500"));
        assert!(coarse.contains("1.5"));
    }

    #[test]
    fn transparent_fill_is_none() {
        let mut ir = ContentIR::empty(ContentSource::from_normalized("x"), 12.0);
        let mut p = Path2D::new();
        p.move_to(Vec2::ZERO).line_to(Vec2::new(5.0, 5.0));
        ir.items.push(StyledPath::filled(p, Color32::TRANSPARENT));
        ir.recompute_bounds();
        assert!(to_svg(&ir, &SvgOptions::default()).contains("fill=\"none\""));
    }
}