//! Visualization geometry: squarified treemap and icicle (flame-graph style) layouts.
//!
//! The UI draws flat rectangles, so the work here is bounded by the number of cells,
//! not by the number of files: children smaller than `min_area` are merged into one
//! "rest" cell, directories stop subdividing at `max_depth` or when `max_cells` is
//! reached. Coordinates are logical pixels (the toolkit applies the DPI scale).
//!
//! Squarified layout: Bruls, Huizing, van Wijk, "Squarified Treemaps" (2000).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::tree::{NodeId, SizeBasis, Tree};

/// Work list that hands out the largest pending item first, so the cell budget is spent
/// on the biggest rectangles: a large directory is always subdivided before a small one.
/// Ties keep insertion order, so layouts are deterministic.
struct Largest<T> {
    heap: BinaryHeap<(u32, Reverse<usize>)>,
    items: Vec<Option<T>>,
}

impl<T> Largest<T> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            items: Vec::new(),
        }
    }

    /// `weight` must be finite and non-negative (an area or a width).
    fn push(&mut self, weight: f32, item: T) {
        let i = self.items.len();
        self.items.push(Some(item));
        // Non-negative floats order like their bit patterns.
        self.heap.push((weight.max(0.0).to_bits(), Reverse(i)));
    }

    fn pop(&mut self) -> Option<T> {
        let (_, Reverse(i)) = self.heap.pop()?;
        self.items[i].take()
    }
}

/// Axis-aligned rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Area.
    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    /// Whether the point lies inside (left/top edges inclusive).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// One laid-out rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// Geometry.
    pub rect: Rect,
    /// The node it stands for (for a rest cell: the directory whose small children it
    /// merges).
    pub node: NodeId,
    /// Depth below the layout root (root = 0).
    pub depth: u16,
    /// Drawn as a filled tile (files, unsubdivided directories, rest cells). Non-leaf
    /// cells are directory frames used for selection highlight and hit-testing.
    pub leaf: bool,
    /// Merges the children too small to draw.
    pub rest: bool,
}

/// Layout limits.
#[derive(Debug, Clone, Copy)]
pub struct LayoutOptions {
    /// Size basis.
    pub basis: SizeBasis,
    /// Children below this area (px²) are merged into a rest cell.
    pub min_area: f32,
    /// Stop once this many cells exist (directories then stay unsubdivided).
    pub max_cells: usize,
    /// Do not subdivide below this depth.
    pub max_depth: u16,
    /// Inset applied to a directory's rectangle before laying out its children, so
    /// nesting stays visible.
    pub padding: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            basis: SizeBasis::Allocated,
            min_area: 24.0,
            max_cells: 6000,
            max_depth: 24,
            padding: 1.0,
        }
    }
}

fn sized_children(t: &Tree, dir: NodeId, basis: SizeBasis) -> Vec<(NodeId, u64)> {
    let mut v: Vec<(NodeId, u64)> = t
        .children(dir)
        .map(|c| (c, t.node(c).size(basis)))
        .filter(|&(_, s)| s > 0)
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn worst(row: &[f64], side: f64) -> f64 {
    let s: f64 = row.iter().sum();
    if s <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let max = row.iter().copied().fold(0.0, f64::max);
    let min = row.iter().copied().fold(f64::INFINITY, f64::min);
    let s2 = s * s;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

/// Splits `rect` into one rectangle per area (areas must sum to `rect.area()` and be
/// sorted descending). Returns the rectangles in input order.
pub fn squarify(areas: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = Vec::with_capacity(areas.len());
    let (mut x, mut y, mut w, mut h) = (
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.w),
        f64::from(rect.h),
    );
    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..=j], side) <= worst(&areas[i..j], side) {
            j += 1;
        }
        let row = &areas[i..j];
        let sum: f64 = row.iter().sum();
        if w >= h {
            // Column on the left.
            let cw = if h > 0.0 { sum / h } else { 0.0 };
            let mut cy = y;
            for (k, a) in row.iter().enumerate() {
                let ch = if cw > 0.0 { a / cw } else { 0.0 };
                let ch = if k + 1 == row.len() { y + h - cy } else { ch };
                out.push(Rect::new(x as f32, cy as f32, cw as f32, ch as f32));
                cy += ch;
            }
            x += cw;
            w -= cw;
        } else {
            // Row on top.
            let rh = if w > 0.0 { sum / w } else { 0.0 };
            let mut cx = x;
            for (k, a) in row.iter().enumerate() {
                let cw = if rh > 0.0 { a / rh } else { 0.0 };
                let cw = if k + 1 == row.len() { x + w - cx } else { cw };
                out.push(Rect::new(cx as f32, y as f32, cw as f32, rh as f32));
                cx += cw;
            }
            y += rh;
            h -= rh;
        }
        w = w.max(0.0);
        h = h.max(0.0);
        i = j;
    }
    out
}

/// Squarified treemap of `root` in `area`. Cells are emitted parents before children.
pub fn treemap(t: &Tree, root: NodeId, area: Rect, o: &LayoutOptions) -> Vec<Cell> {
    let mut cells = Vec::new();
    if area.area() <= 0.0 || t.node(root).size(o.basis) == 0 {
        return cells;
    }
    let mut stack = Largest::new();
    stack.push(area.area(), (root, area, 0u16));
    while let Some((dir, rect, depth)) = stack.pop() {
        let n = t.node(dir);
        let kids = if n.is_dir() {
            sized_children(t, dir, o.basis)
        } else {
            Vec::new()
        };
        let can_split = !kids.is_empty()
            && depth < o.max_depth
            && cells.len() < o.max_cells
            && rect.area() >= o.min_area * 4.0;
        if !can_split {
            cells.push(Cell {
                rect,
                node: dir,
                depth,
                leaf: true,
                rest: false,
            });
            continue;
        }
        cells.push(Cell {
            rect,
            node: dir,
            depth,
            leaf: false,
            rest: false,
        });
        let pad = if depth == 0 { 0.0 } else { o.padding };
        let inner = Rect::new(
            rect.x + pad,
            rect.y + pad,
            (rect.w - 2.0 * pad).max(0.0),
            (rect.h - 2.0 * pad).max(0.0),
        );
        let total: u64 = kids.iter().map(|k| k.1).sum();
        let scale = f64::from(inner.area()) / total as f64;
        // Keep children while they are big enough; merge the tail.
        let mut keep = kids.len();
        let mut rest_size = 0u64;
        for (i, k) in kids.iter().enumerate() {
            if (k.1 as f64 * scale) < f64::from(o.min_area) {
                keep = i;
                rest_size = kids[i..].iter().map(|k| k.1).sum();
                break;
            }
        }
        let mut areas: Vec<f64> = kids[..keep].iter().map(|k| k.1 as f64 * scale).collect();
        if rest_size > 0 {
            areas.push(rest_size as f64 * scale);
        }
        let rects = squarify(&areas, inner);
        for (i, r) in rects.into_iter().enumerate() {
            if i < keep {
                stack.push(r.area(), (kids[i].0, r, depth + 1));
            } else {
                cells.push(Cell {
                    rect: r,
                    node: dir,
                    depth: depth + 1,
                    leaf: true,
                    rest: true,
                });
            }
        }
    }
    cells
}

/// Icicle layout (a flame graph drawn top-down): one row per depth level, each child
/// spanning its share of its parent's width. Rows are `row_h` high.
pub fn icicle(t: &Tree, root: NodeId, area: Rect, row_h: f32, o: &LayoutOptions) -> Vec<Cell> {
    let mut cells = Vec::new();
    if area.area() <= 0.0 || row_h <= 0.0 || t.node(root).size(o.basis) == 0 {
        return cells;
    }
    let levels = ((area.h / row_h).floor() as u16).max(1);
    let min_w = (o.min_area / row_h).max(1.0);
    let mut stack = Largest::new();
    stack.push(area.w, (root, area.x, area.w, 0u16));
    while let Some((id, x, w, depth)) = stack.pop() {
        let n = t.node(id);
        let kids = if n.is_dir() && depth + 1 < levels.min(o.max_depth + 1) {
            sized_children(t, id, o.basis)
        } else {
            Vec::new()
        };
        let leaf = kids.is_empty() || cells.len() >= o.max_cells;
        cells.push(Cell {
            rect: Rect::new(x, area.y + f32::from(depth) * row_h, w, row_h),
            node: id,
            depth,
            leaf,
            rest: false,
        });
        if leaf {
            continue;
        }
        let total = n.size(o.basis).max(1) as f64;
        let mut cx = f64::from(x);
        let mut rest = 0.0f64;
        for (c, s) in kids {
            let cw = f64::from(w) * s as f64 / total;
            if cw < f64::from(min_w) {
                rest += cw;
                continue;
            }
            stack.push(cw as f32, (c, cx as f32, cw as f32, depth + 1));
            cx += cw;
        }
        if rest >= 1.0 {
            cells.push(Cell {
                rect: Rect::new(
                    cx as f32,
                    area.y + f32::from(depth + 1) * row_h,
                    rest as f32,
                    row_h,
                ),
                node: id,
                depth: depth + 1,
                leaf: true,
                rest: true,
            });
        }
    }
    cells
}

/// The cell under a point: the deepest cell containing it (leaves win over frames).
pub fn hit(cells: &[Cell], x: f32, y: f32) -> Option<&Cell> {
    cells
        .iter()
        .filter(|c| c.rect.contains(x, y))
        .max_by_key(|c| (c.leaf, c.depth))
}

/// The rectangle of a node in a layout, if it was laid out.
pub fn rect_of(cells: &[Cell], node: NodeId) -> Option<Rect> {
    cells
        .iter()
        .find(|c| c.node == node && !c.rest)
        .map(|c| c.rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryMeta;
    use crate::path::PathStyle;
    use crate::tree::tests::sample;
    use proptest::prelude::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn squarify_example_from_paper() {
        // 6×4 rectangle with areas 6,6,4,3,2,2,1.
        let areas = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let r = squarify(&areas, Rect::new(0.0, 0.0, 6.0, 4.0));
        assert_eq!(r.len(), 7);
        let total: f32 = r.iter().map(Rect::area).sum();
        assert!(close(total, 24.0));
        for (rect, a) in r.iter().zip(areas) {
            assert!(close(rect.area(), a as f32), "{rect:?} {a}");
        }
        // First row is a column of the two 6s, 3 wide.
        assert!(close(r[0].w, 3.0) && close(r[0].h, 2.0));
        assert!(close(r[1].y, 2.0));
    }

    proptest! {
        #[test]
        fn squarify_tiles_without_overlap(
            sizes in proptest::collection::vec(1u32..1000, 1..40),
            w in 10f32..800.0,
            h in 10f32..600.0,
        ) {
            let mut s: Vec<f64> = sizes.iter().map(|&x| f64::from(x)).collect();
            s.sort_by(|a, b| b.partial_cmp(a).unwrap());
            let total: f64 = s.iter().sum();
            let area = f64::from(w) * f64::from(h);
            let areas: Vec<f64> = s.iter().map(|x| x / total * area).collect();
            let rects = squarify(&areas, Rect::new(0.0, 0.0, w, h));
            prop_assert_eq!(rects.len(), areas.len());
            let sum: f64 = rects.iter().map(|r| f64::from(r.area())).sum();
            prop_assert!((sum - area).abs() / area < 1e-3);
            for r in &rects {
                prop_assert!(r.x >= -0.01 && r.y >= -0.01);
                prop_assert!(r.x + r.w <= w + 0.05 && r.y + r.h <= h + 0.05);
            }
            // Centres of different rects are never inside each other.
            for (i, a) in rects.iter().enumerate() {
                for b in rects.iter().skip(i + 1) {
                    let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
                    if b.w > 0.1 && b.h > 0.1 && a.w > 0.1 && a.h > 0.1 {
                        prop_assert!(!(cx > a.x + 0.01 && cx < a.x + a.w - 0.01 && cy > a.y + 0.01 && cy < a.y + a.h - 0.01));
                    }
                }
            }
        }
    }

    #[test]
    fn treemap_of_sample() {
        let (t, r) = sample();
        let o = LayoutOptions {
            basis: SizeBasis::Logical,
            min_area: 1.0,
            ..LayoutOptions::default()
        };
        let cells = treemap(&t, r, Rect::new(0.0, 0.0, 400.0, 300.0), &o);
        assert!(!cells[0].leaf && cells[0].node == r);
        let leaves: Vec<_> = cells.iter().filter(|c| c.leaf).collect();
        let leaf_area: f32 = leaves.iter().map(|c| c.rect.area()).sum();
        // Every leaf is inside the area; nesting padding takes a little of it.
        assert!(
            leaf_area <= 120_000.5 && leaf_area > 110_000.0,
            "{leaf_area}"
        );
        let b = cells.iter().find(|c| t.name(c.node) == "b.BIN").unwrap();
        assert!(b.rect.area() > 100_000.0);
        let hit_b = hit(&cells, b.rect.x + 1.0, b.rect.y + 1.0).unwrap();
        assert_eq!(hit_b.node, b.node);
        assert_eq!(rect_of(&cells, b.node), Some(b.rect));
        assert!(hit(&cells, -5.0, 0.0).is_none());
    }

    #[test]
    fn small_children_merge_and_limits() {
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/r")])[0];
        let mut entries = vec![EntryMeta::file("big", 1_000_000)];
        for i in 0..500 {
            entries.push(EntryMeta::file(&format!("s{i}"), 10));
        }
        t.fill_dir(r, &entries);
        let o = LayoutOptions {
            basis: SizeBasis::Logical,
            ..LayoutOptions::default()
        };
        let cells = treemap(&t, r, Rect::new(0.0, 0.0, 200.0, 100.0), &o);
        assert!(cells.len() < 10, "{}", cells.len());
        assert!(cells.iter().any(|c| c.rest));
        let empty = treemap(&t, r, Rect::new(0.0, 0.0, 0.0, 10.0), &o);
        assert!(empty.is_empty());
        let shallow = LayoutOptions { max_depth: 0, ..o };
        let one = treemap(&t, r, Rect::new(0.0, 0.0, 200.0, 100.0), &shallow);
        assert_eq!(one.len(), 1);
        assert!(one[0].leaf);
    }

    #[test]
    fn cell_budget_goes_to_the_largest_directories_first() {
        // One big directory with many files next to many small directories: with a tight
        // budget the big one must still be subdivided (it used to be laid out last).
        let mut t = Tree::new(PathStyle::Posix);
        let r = t.set_roots(&[EntryMeta::dir("/r")])[0];
        let mut top = vec![EntryMeta::dir("big")];
        for i in 0..40 {
            top.push(EntryMeta::dir(&format!("small{i}")));
        }
        let ids = t.fill_dir(r, &top);
        let big = ids[0];
        let files: Vec<EntryMeta> = (0..50)
            .map(|i| EntryMeta::file(&format!("f{i}"), 1_000_000))
            .collect();
        t.fill_dir(big, &files);
        for &d in &ids[1..] {
            let small: Vec<EntryMeta> = (0..20)
                .map(|i| EntryMeta::file(&format!("g{i}"), 10_000))
                .collect();
            t.fill_dir(d, &small);
        }
        let o = LayoutOptions {
            basis: SizeBasis::Logical,
            min_area: 1.0,
            max_cells: 80,
            ..LayoutOptions::default()
        };
        let cells = treemap(&t, r, Rect::new(0.0, 0.0, 800.0, 600.0), &o);
        let big_cell = cells.iter().find(|c| c.node == big && !c.rest).unwrap();
        assert!(!big_cell.leaf, "largest directory was not subdivided");
        let icicle_cells = icicle(&t, r, Rect::new(0.0, 0.0, 800.0, 100.0), 20.0, &o);
        let big_bar = icicle_cells.iter().find(|c| c.node == big).unwrap();
        assert!(
            !big_bar.leaf,
            "largest directory was not expanded in the icicle"
        );
    }

    #[test]
    fn icicle_rows() {
        let (t, r) = sample();
        let o = LayoutOptions {
            basis: SizeBasis::Logical,
            min_area: 1.0,
            ..LayoutOptions::default()
        };
        let cells = icicle(&t, r, Rect::new(0.0, 0.0, 500.0, 100.0), 20.0, &o);
        assert_eq!(cells[0].node, r);
        assert!(close(cells[0].rect.w, 500.0));
        let depth1: f32 = cells
            .iter()
            .filter(|c| c.depth == 1)
            .map(|c| c.rect.w)
            .sum();
        assert!(close(depth1, 500.0), "{depth1}");
        assert!(cells.iter().all(|c| c.rect.y + c.rect.h <= 100.0 + 0.01));
        let sub = cells.iter().find(|c| t.name(c.node) == "sub").unwrap();
        assert!(!sub.leaf);
        assert!(icicle(&t, r, Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, &o).is_empty());
    }
}
