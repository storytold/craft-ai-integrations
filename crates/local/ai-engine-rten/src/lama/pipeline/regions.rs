//! Tells the separate areas a mask marks, so each is filled from a window of its own.
//!
//! One box around every marked pixel would make two small marks in opposite corners of a large
//! photograph share one picture-wide window, scaled far down. Instead the marked pixels are
//! gathered on a coarse grid, marked cells that touch form one area, and areas are joined only
//! when they would overlap or when one unscaled window holds both. The number of areas, and so of
//! model runs, is bounded: past [`MAX_AREAS`] the grid is made coarser until near marks join.

use super::{MAX_RIM, Rect, SIDE};

/// The side of a grid cell, in picture pixels. Marks in cells that touch are one area.
const CELL: usize = 64;

/// The most areas one request is split into: each is one model run.
pub(super) const MAX_AREAS: usize = 16;

/// Above this many areas the pairwise joining is skipped and the grid is made coarser first, so
/// a hostile mask of scattered dots cannot make joining slow.
const JOIN_LIMIT: usize = 256;

/// The marked pixels gathered into cells: each cell holds the box around its marked pixels.
struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Option<Rect>>,
}

/// `slot` widened to hold `rect` as well.
fn include(slot: &mut Option<Rect>, rect: Rect) {
    *slot = Some(slot.map_or(rect, |held| held.union(rect)));
}

impl Grid {
    /// The grid of a `w x h` mask, both sides above zero.
    fn of(mask: &[u8], w: usize, h: usize) -> Self {
        let (cols, rows) = (w.div_ceil(CELL), h.div_ceil(CELL));
        let mut cells = vec![None; cols * rows];
        for (y, row) in mask.chunks_exact(w).take(h).enumerate() {
            let base = (y / CELL) * cols;
            for (x, _) in row.iter().enumerate().filter(|(_, v)| **v > 0) {
                if let Some(cell) = cells.get_mut(base + x / CELL) {
                    let pixel = Rect {
                        x0: x,
                        y0: y,
                        x1: x + 1,
                        y1: y + 1,
                    };
                    include(cell, pixel);
                }
            }
        }
        Self { cols, rows, cells }
    }

    /// The same marks on a grid with cells twice the side.
    fn coarser(&self) -> Self {
        let (cols, rows) = (self.cols.div_ceil(2), self.rows.div_ceil(2));
        let mut cells = vec![None; cols * rows];
        for (i, cell) in self.cells.iter().enumerate() {
            let Some(rect) = *cell else { continue };
            let (cx, cy) = (i % self.cols.max(1), i / self.cols.max(1));
            if let Some(slot) = cells.get_mut((cy / 2) * cols + cx / 2) {
                include(slot, rect);
            }
        }
        Self { cols, rows, cells }
    }

    /// The marked cell at `i`, the first time it is asked for.
    fn take(&self, i: usize, seen: &mut [bool]) -> Option<Rect> {
        let rect = (*self.cells.get(i)?)?;
        let flag = seen.get_mut(i)?;
        if *flag {
            return None;
        }
        *flag = true;
        Some(rect)
    }

    /// The cells around cell `i`, corners included (and `i` itself).
    fn around(&self, i: usize) -> impl Iterator<Item = usize> {
        let cols = self.cols.max(1);
        let (cx, cy) = (i % cols, i / cols);
        let xs = cx.saturating_sub(1)..=(cx + 1).min(cols - 1);
        let ys = cy.saturating_sub(1)..=(cy + 1).min(self.rows.saturating_sub(1));
        ys.flat_map(move |y| xs.clone().map(move |x| y * cols + x))
    }

    /// One box for each group of marked cells that touch.
    fn groups(&self) -> Vec<Rect> {
        let mut seen = vec![false; self.cells.len()];
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for start in 0..self.cells.len() {
            let Some(mut bounds) = self.take(start, &mut seen) else {
                continue;
            };
            stack.push(start);
            while let Some(i) = stack.pop() {
                for n in self.around(i) {
                    if let Some(rect) = self.take(n, &mut seen) {
                        bounds = bounds.union(rect);
                        stack.push(n);
                    }
                }
            }
            out.push(bounds);
        }
        out
    }
}

/// Whether one unscaled window holds `area` with its rim and the context around it.
fn fits_one_window(area: Rect) -> bool {
    (area.longest() + 2 * MAX_RIM).saturating_mul(3).div_ceil(2) <= SIDE
}

/// Whether two areas are filled as one: their rims would overlap, so each run would write into
/// the other's pixels, or one unscaled window holds both, so a second run would gain nothing.
fn belong_together(a: Rect, b: Rect, w: usize, h: usize) -> bool {
    a.grown(MAX_RIM, w, h).touches(b.grown(MAX_RIM, w, h)) || fits_one_window(a.union(b))
}

/// The first two areas that belong together, by position in `boxes`.
fn first_pair(boxes: &[Rect], w: usize, h: usize) -> Option<(usize, usize)> {
    for (a, first) in boxes.iter().enumerate() {
        for (b, second) in boxes.iter().enumerate().skip(a + 1) {
            if belong_together(*first, *second, w, h) {
                return Some((a, b));
            }
        }
    }
    None
}

/// Joins areas that belong together until none do. Every area left keeps its rim to itself.
fn join(boxes: &mut Vec<Rect>, w: usize, h: usize) {
    while let Some((a, b)) = first_pair(boxes, w, h) {
        let second = boxes.swap_remove(b);
        if let Some(first) = boxes.get_mut(a) {
            *first = first.union(second);
        }
    }
}

/// The separate marked areas of a `w x h` mask, each the box around its marked pixels, top to
/// bottom. At most [`MAX_AREAS`], and none when nothing is marked.
pub(super) fn areas(mask: &[u8], w: usize, h: usize) -> Vec<Rect> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut grid = Grid::of(mask, w, h);
    loop {
        let mut boxes = grid.groups();
        if boxes.len() <= JOIN_LIMIT {
            join(&mut boxes, w, h);
        }
        if boxes.len() <= MAX_AREAS || grid.cells.len() <= 1 {
            boxes.sort_by_key(|r| (r.y0, r.x0));
            return boxes;
        }
        grid = grid.coarser();
    }
}
