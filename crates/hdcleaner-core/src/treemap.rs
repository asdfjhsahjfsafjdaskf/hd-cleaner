//! Squarified treemap layout computed in Rust; the UI only draws rectangles.
//!
//! Output is a compact binary buffer (24 bytes per rectangle) sent to the
//! frontend as an `ArrayBuffer`:
//! `x f32, y f32, w f32, h f32, node u32, category u8, depth u8, kind u8, flags u8`.

use crate::scan::model::{NodeId, ScanTree};
use serde::Deserialize;
use std::collections::HashSet;

pub const RECT_BYTES: usize = 24;
pub const KIND_FILE: u8 = 0;
/// Directory drawn as a solid block (too small to subdivide / limit reached).
pub const KIND_DIR_BLOCK: u8 = 1;
/// Directory frame: outline + hit target, children drawn inside.
pub const KIND_DIR_FRAME: u8 = 2;
pub const FLAG_HIGHLIGHT: u8 = 1;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SizeMetric {
    #[default]
    Allocated,
    Logical,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapOptions {
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub metric: SizeMetric,
    #[serde(default = "default_max_rects")]
    pub max_rects: usize,
    /// Nodes whose subtrees are highlighted (App Storage Map).
    #[serde(default)]
    pub highlight: Vec<NodeId>,
}

fn default_max_rects() -> usize {
    120_000
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

pub struct Layout {
    pub bytes: Vec<u8>,
    pub count: usize,
}

struct Ctx<'a> {
    tree: &'a ScanTree,
    metric: SizeMetric,
    max_rects: usize,
    highlight: HashSet<NodeId>,
    out: Vec<u8>,
    count: usize,
}

impl Ctx<'_> {
    fn weight(&self, id: NodeId) -> f64 {
        let n = self.tree.node(id);
        if !n.counts_toward_totals() {
            return 0.0;
        }
        match self.metric {
            SizeMetric::Allocated => n.alloc as f64,
            SizeMetric::Logical => n.size as f64,
        }
    }

    fn emit(&mut self, r: Rect, id: NodeId, depth: u32, kind: u8, hl: bool) {
        let cat = self.tree.category(id) as u8;
        self.out.extend_from_slice(&r.x.to_le_bytes());
        self.out.extend_from_slice(&r.y.to_le_bytes());
        self.out.extend_from_slice(&r.w.to_le_bytes());
        self.out.extend_from_slice(&r.h.to_le_bytes());
        self.out.extend_from_slice(&id.to_le_bytes());
        self.out.push(cat);
        self.out.push(depth.min(255) as u8);
        self.out.push(kind);
        self.out.push(if hl { FLAG_HIGHLIGHT } else { 0 });
        self.count += 1;
    }

    fn layout_dir(&mut self, id: NodeId, r: Rect, depth: u32, hl: bool) {
        let hl = hl || self.highlight.contains(&id);
        if depth > 0 {
            self.emit(r, id, depth, KIND_DIR_FRAME, hl);
        }
        // Inset so nested directories remain visually distinguishable.
        let pad = if depth == 0 { 0.0 } else if r.w > 12.0 && r.h > 12.0 { 1.0 } else { 0.0 };
        let inner = Rect { x: r.x + pad, y: r.y + pad, w: (r.w - 2.0 * pad).max(0.0), h: (r.h - 2.0 * pad).max(0.0) };
        let area = (inner.w * inner.h) as f64;
        if area < 1.0 {
            return;
        }
        let total: f64 = self.weight(id).max(0.0);
        if total <= 0.0 {
            return;
        }
        let mut kids: Vec<(NodeId, f64)> = self
            .tree
            .children(id)
            .iter()
            .map(|&c| (c, self.weight(c)))
            .filter(|&(_, w)| w > 0.0)
            .collect();
        if self.metric == SizeMetric::Logical {
            kids.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        }
        let scale = area / total;
        // Drop sub-pixel items (sorted desc, so everything after is smaller).
        let cut = kids.iter().position(|&(_, w)| w * scale < 0.5).unwrap_or(kids.len());
        kids.truncate(cut);
        let items: Vec<(NodeId, f64)> = kids.into_iter().map(|(c, w)| (c, w * scale)).collect();
        let mut placed = Vec::with_capacity(items.len());
        squarify(&items, inner, &mut placed);
        for (child, cr) in placed {
            let n = self.tree.node(child);
            let child_hl = hl || self.highlight.contains(&child);
            if n.is_dir() {
                if n.child_count > 0 && cr.w >= 3.0 && cr.h >= 3.0 && self.count < self.max_rects {
                    self.layout_dir(child, cr, depth + 1, hl);
                } else {
                    self.emit(cr, child, depth + 1, KIND_DIR_BLOCK, child_hl);
                }
            } else {
                self.emit(cr, child, depth + 1, KIND_FILE, child_hl);
            }
        }
    }
}

/// Worst aspect ratio of a row (Bruls, Huizing, van Wijk).
fn worst(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

fn squarify(items: &[(NodeId, f64)], mut r: Rect, out: &mut Vec<(NodeId, Rect)>) {
    let mut i = 0;
    while i < items.len() {
        let side = r.w.min(r.h) as f64;
        if side <= 0.0 {
            break;
        }
        let mut end = i + 1;
        let mut sum = items[i].1;
        let mut cur = worst(sum, items[i].1, items[i].1, side);
        while end < items.len() {
            let s = sum + items[end].1;
            let w = worst(s, items[i].1, items[end].1, side);
            if w > cur {
                break;
            }
            sum = s;
            cur = w;
            end += 1;
        }
        let row = &items[i..end];
        if r.w >= r.h {
            // Column on the left, items stacked vertically.
            let cw = (sum / r.h as f64) as f32;
            let mut y = r.y;
            for (k, &(id, a)) in row.iter().enumerate() {
                let h = if k + 1 == row.len() { r.y + r.h - y } else { (a / cw as f64) as f32 };
                out.push((id, Rect { x: r.x, y, w: cw.min(r.w), h }));
                y += h;
            }
            r.x += cw;
            r.w = (r.w - cw).max(0.0);
        } else {
            let rh = (sum / r.w as f64) as f32;
            let mut x = r.x;
            for (k, &(id, a)) in row.iter().enumerate() {
                let w = if k + 1 == row.len() { r.x + r.w - x } else { (a / rh as f64) as f32 };
                out.push((id, Rect { x, y: r.y, w, h: rh.min(r.h) }));
                x += w;
            }
            r.y += rh;
            r.h = (r.h - rh).max(0.0);
        }
        i = end;
    }
}

pub fn layout(tree: &ScanTree, root: NodeId, opts: &TreemapOptions) -> Layout {
    let w = opts.width.clamp(1.0, 16384.0);
    let h = opts.height.clamp(1.0, 16384.0);
    let mut ctx = Ctx {
        tree,
        metric: opts.metric,
        max_rects: opts.max_rects.clamp(100, 1_000_000),
        highlight: opts.highlight.iter().copied().collect(),
        out: Vec::with_capacity(64 * 1024),
        count: 0,
    };
    let r = Rect { x: 0.0, y: 0.0, w, h };
    if tree.get(root).is_some() {
        if tree.node(root).is_dir() {
            ctx.layout_dir(root, r, 0, false);
        } else {
            let hl = ctx.highlight.contains(&root);
            ctx.emit(r, root, 0, KIND_FILE, hl);
        }
    }
    Layout { bytes: ctx.out, count: ctx.count }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::{ScanMeta, ROOT};

    #[test]
    fn areas_are_proportional_and_inside() {
        let mut b = TreeBuilder::new("C:\\", 8);
        let sizes = [600u64, 300, 60, 30, 10];
        for (i, s) in sizes.iter().enumerate() {
            b.add(ROOT, &format!("f{i}.bin"), &EntryInfo { size: *s, alloc: *s, ..Default::default() });
        }
        let t = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });
        let l = layout(
            &t,
            ROOT,
            &TreemapOptions { width: 100.0, height: 50.0, metric: SizeMetric::Allocated, max_rects: 1000, highlight: vec![] },
        );
        assert_eq!(l.count, 5);
        let mut total = 0.0f32;
        for i in 0..l.count {
            let r = &l.bytes[i * RECT_BYTES..(i + 1) * RECT_BYTES];
            let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            let (x, y, w, h) = (f(0), f(4), f(8), f(12));
            assert!(x >= -0.01 && y >= -0.01 && x + w <= 100.01 && y + h <= 50.01);
            let id = u32::from_le_bytes(r[16..20].try_into().unwrap());
            let expected = t.node(id).alloc as f32 / 1000.0 * 5000.0;
            assert!((w * h - expected).abs() < 1.0, "area {} vs {}", w * h, expected);
            total += w * h;
        }
        assert!((total - 5000.0).abs() < 1.0);
    }

    #[test]
    fn nested_and_highlight() {
        let mut b = TreeBuilder::new("C:\\", 8);
        let d = b.add(ROOT, "d", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(d, "a", &EntryInfo { size: 500, alloc: 500, ..Default::default() });
        b.add(ROOT, "b", &EntryInfo { size: 500, alloc: 500, ..Default::default() });
        let t = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });
        let l = layout(
            &t,
            ROOT,
            &TreemapOptions { width: 200.0, height: 100.0, metric: SizeMetric::Allocated, max_rects: 1000, highlight: vec![d] },
        );
        assert_eq!(l.count, 3); // frame d, file a, file b
        let hl: Vec<u8> = (0..l.count).map(|i| l.bytes[i * RECT_BYTES + 23]).collect();
        assert_eq!(hl.iter().filter(|&&f| f & FLAG_HIGHLIGHT != 0).count(), 2);
    }
}
