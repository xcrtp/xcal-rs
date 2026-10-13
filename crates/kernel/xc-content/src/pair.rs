// pair.rs — 语义配对：按源码锚点把两个 ContentIR 的路径配对成 `(from, to)`。
//
// 路线甲：语义配对的知识留在外层（xc-content），xc-time 不知情。
// 产出的路径对交给下层（`Path2D::morph` / `Curve::PathMorph`）逐对插值。
//
// 为什么按锚点而非弧长/索引：`x^2 → x^3` 里的 `x` 应该变到 `x^3` 的 `x`，
// 而不是变到 `3`。锚点是"身份"（同一段源码），弧长是"位置"。

use xc_geometry::Path2D;

use crate::ir::ContentIR;

/// 按源码锚点配对 `from` → `to` 的路径，产出 `(from_path, to_path)` 对。
///
/// 规则：
/// 1. 锚点 `Span` 相同 → 配对（身份对齐，与 items 顺序无关）；
/// 2. 仅 `from` 有 → 淡出：`to` 退化为一个点（[`degenerate_at`]）；
/// 3. 仅 `to` 有 → 淡入：`from` 退化为一个点；
/// 4. 无锚点 / 未匹配的剩余项 → 按索引顺序配对（zip），多余者再按 2/3 淡入淡出。
///
/// 不 panic：锚点缺失、items 为空都安全退化。
pub fn pair_by_anchor(from: &ContentIR, to: &ContentIR) -> Vec<(Path2D, Path2D)> {
    let mut pairs: Vec<(Path2D, Path2D)> = Vec::new();
    let mut to_used = vec![false; to.items.len()];
    let mut from_leftover: Vec<usize> = Vec::new();

    // 1) 锚点相同 → 配对（`to` 侧每个只被消费一次）。
    for (fi, f) in from.items.iter().enumerate() {
        let matched = f.anchor.and_then(|fa| {
            to.items
                .iter()
                .enumerate()
                .position(|(ti, t)| !to_used[ti] && t.anchor == Some(fa))
        });
        match matched {
            Some(ti) => {
                to_used[ti] = true;
                pairs.push((f.path.clone(), to.items[ti].path.clone()));
            }
            None => from_leftover.push(fi),
        }
    }

    // 4) 剩余项按索引顺序 zip（无锚点时的退化路径）。
    let to_leftover: Vec<usize> = (0..to.items.len()).filter(|ti| !to_used[*ti]).collect();
    let zipped = from_leftover.len().min(to_leftover.len());
    for k in 0..zipped {
        pairs.push((
            from.items[from_leftover[k]].path.clone(),
            to.items[to_leftover[k]].path.clone(),
        ));
    }

    // 2) 仅 from 有 → 淡出（目标退化为点）。
    for &fi in &from_leftover[zipped..] {
        let p = from.items[fi].path.clone();
        let target = degenerate_at(&p);
        pairs.push((p, target));
    }
    // 3) 仅 to 有 → 淡入（起点退化为点）。
    for &ti in &to_leftover[zipped..] {
        let p = to.items[ti].path.clone();
        let source = degenerate_at(&p);
        pairs.push((source, p));
    }

    pairs
}

/// 退化路径：位于 `path` 包围盒中心的**一个点**（供淡入 / 淡出目标）。
/// 无几何（空路径）时返回空路径。
pub fn degenerate_at(path: &Path2D) -> Path2D {
    match path.compute_bounds() {
        Some(b) => Path2D::from_polyline(&[b.center()], true),
        None => Path2D::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ContentSource, Span, StyledPath};
    use xc_geometry::{Color32, Vec2};

    /// half 半宽的正方形（以 (cx, cy) 为中心）。
    fn square(cx: f32, cy: f32, half: f32) -> Path2D {
        Path2D::from_polyline(
            &[
                Vec2::new(cx - half, cy - half),
                Vec2::new(cx + half, cy - half),
                Vec2::new(cx + half, cy + half),
                Vec2::new(cx - half, cy + half),
            ],
            true,
        )
    }

    fn make_ir(items: Vec<(Option<Span>, Path2D)>) -> ContentIR {
        let mut ir = ContentIR::empty(ContentSource::from_raw("t"), 1.0);
        ir.items = items
            .into_iter()
            .map(|(anchor, path)| StyledPath {
                path,
                fill: Some(Color32::WHITE),
                stroke: None,
                anchor,
            })
            .collect();
        ir.recompute_bounds();
        ir
    }

    /// 锚点配对优先于索引配对：即使 items 顺序相反，`x` 也必须配到 `x`。
    #[test]
    fn pairs_by_anchor_not_by_index() {
        // from: [x@(0,1) 半宽 1, 2@(2,3) 半宽 1]
        let from = make_ir(vec![
            (Some(Span::new(0, 1)), square(0.0, 0.0, 1.0)),
            (Some(Span::new(2, 3)), square(2.0, 0.0, 1.0)),
        ]);
        // to: [3@(2,3) 半宽 3, x@(0,1) 半宽 1] —— 注意顺序被打乱
        let to = make_ir(vec![
            (Some(Span::new(2, 3)), square(2.0, 0.0, 3.0)),
            (Some(Span::new(0, 1)), square(0.0, 0.0, 1.0)),
        ]);

        let pairs = pair_by_anchor(&from, &to);
        assert_eq!(pairs.len(), 2);

        // 第一条 from 是 x@(0,1)；锚点配对 → 应配到 to 中锚点 (0,1) 的那条（半宽 1）。
        // 若误用索引配对，会配到 to[0]（半宽 3）。
        let b0 = pairs[0].1.compute_bounds().unwrap();
        assert!(
            (b0.max.x - 1.0).abs() < 1e-6,
            "x 应配到 x（半宽 1），实际半宽 {}",
            b0.max.x
        );
        // 第二条 from 是 2@(2,3) → 配到 3@(2,3)（半宽 3）。
        let b1 = pairs[1].1.compute_bounds().unwrap();
        assert!((b1.max.x - 5.0).abs() < 1e-6, "max.x={}", b1.max.x);
    }

    /// 仅 to 有 → 淡入：from 侧退化为目标中心的一个点。
    #[test]
    fn only_to_fades_in_from_degenerate_point() {
        let from = make_ir(vec![(Some(Span::new(0, 1)), square(0.0, 0.0, 1.0))]);
        let to = make_ir(vec![
            (Some(Span::new(0, 1)), square(0.0, 0.0, 1.0)),
            (Some(Span::new(2, 3)), square(5.0, 0.0, 1.0)),
        ]);

        let pairs = pair_by_anchor(&from, &to);
        assert_eq!(pairs.len(), 2);
        let b = pairs[1].0.compute_bounds().unwrap();
        assert!(b.size().length() < 1e-6, "淡入起点应为退化点: {b:?}");
        assert!((b.center().x - 5.0).abs() < 1e-6, "退化点应在目标中心");
    }

    /// 仅 from 有 → 淡出：to 侧退化为源中心的一个点。
    #[test]
    fn only_from_fades_out_to_degenerate_point() {
        let from = make_ir(vec![
            (Some(Span::new(0, 1)), square(0.0, 0.0, 1.0)),
            (Some(Span::new(2, 3)), square(-4.0, 0.0, 1.0)),
        ]);
        let to = make_ir(vec![(Some(Span::new(0, 1)), square(0.0, 0.0, 1.0))]);

        let pairs = pair_by_anchor(&from, &to);
        assert_eq!(pairs.len(), 2);
        let b = pairs[1].1.compute_bounds().unwrap();
        assert!(b.size().length() < 1e-6, "淡出终点应为退化点: {b:?}");
        assert!((b.center().x + 4.0).abs() < 1e-6);
    }

    /// 无锚点 → 按索引顺序配对。
    #[test]
    fn no_anchors_falls_back_to_index_pairing() {
        let from = make_ir(vec![
            (None, square(0.0, 0.0, 1.0)),
            (None, square(2.0, 0.0, 1.0)),
        ]);
        let to = make_ir(vec![
            (None, square(0.0, 0.0, 3.0)),
            (None, square(2.0, 0.0, 3.0)),
        ]);
        let pairs = pair_by_anchor(&from, &to);
        assert_eq!(pairs.len(), 2);
        // 索引配对：pairs[0] = (from[0], to[0])（半宽 3）
        assert!((pairs[0].1.compute_bounds().unwrap().max.x - 3.0).abs() < 1e-6);
        assert!((pairs[1].1.compute_bounds().unwrap().min.x - (-1.0)).abs() < 1e-6);
    }

    /// 空 IR 不 panic。
    #[test]
    fn empty_irs_do_not_panic() {
        let empty = make_ir(vec![]);
        assert!(pair_by_anchor(&empty, &empty).is_empty());
        let one = make_ir(vec![(None, square(0.0, 0.0, 1.0))]);
        // 只有 from → 淡出
        assert_eq!(pair_by_anchor(&one, &empty).len(), 1);
        // 只有 to → 淡入
        assert_eq!(pair_by_anchor(&empty, &one).len(), 1);
    }
}
