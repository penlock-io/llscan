//! The stage's geometric evidence: column/row groups and reach components.
//! These are existing geometric rules, not an inferred semantic slot grid.

use crate::detect::Quad;
use crate::page_frame::{Direction, WritingFrame};

/// Vertical reach in box heights, unchanged from the region rule.
pub const ROW_REACH: f32 = 1.0;
/// Horizontal reach in box heights, unchanged from the region rule.
pub const COLUMN_REACH: f32 = 1.5;

/// One exposure of the stage's clustering, indexed by original observations.
#[derive(Clone, Debug)]
pub struct PageLayout {
    /// Columns left to right, members top to bottom.
    pub columns: Vec<Vec<usize>>,
    /// Rows top to bottom, members left to right.
    pub rows: Vec<Vec<usize>>,
    bounds: Vec<[f32; 4]>,
    pub(crate) writing: WritingFrame,
}

impl PageLayout {
    // Shared row-block owner for numbered-list completion and dot-track fits.
    // The two consumers retain their explicit word-shape thresholds.
    pub(crate) fn dominant_word_block(
        &self,
        members: &[usize],
        aspect: f32,
    ) -> Option<(Vec<usize>, f32)> {
        let b = &self.bounds;
        let mut words: Vec<_> = members
            .iter()
            .copied()
            .filter(|&i| b[i][2] - b[i][0] >= aspect * (b[i][3] - b[i][1]))
            .collect();
        if words.len() < 2 {
            return None;
        }
        let mut heights: Vec<_> = words.iter().map(|&i| b[i][3] - b[i][1]).collect();
        heights.sort_by(f32::total_cmp);
        let h = heights[heights.len() / 2];
        let y = |i: usize| (b[i][1] + b[i][3]) / 2.;
        words.sort_by(|&a, &b| y(a).total_cmp(&y(b)));
        let mut blocks: Vec<Vec<usize>> = Vec::new();
        for i in words {
            if blocks
                .last()
                .is_none_or(|block| y(i) - y(*block.last().unwrap()) > 2. * h)
            {
                blocks.push(Vec::new());
            }
            blocks.last_mut().unwrap().push(i);
        }
        let words = blocks.into_iter().max_by_key(Vec::len)?;
        (words.len() >= 2).then_some((words, h))
    }
    /// Runs the existing centre-in-span grouping on both axes once.
    pub fn new(quads: &[Quad]) -> Self {
        Self::with_writing(quads, WritingFrame::Local)
    }

    /// Group in page writing coordinates while retaining the original indices.
    pub fn in_direction(quads: &[Quad], direction: Direction) -> Self {
        Self::with_writing(quads, WritingFrame::Page(direction))
    }

    pub(crate) fn with_writing(quads: &[Quad], writing: WritingFrame) -> Self {
        let projected: Vec<_> = quads.iter().map(|q| writing.project(q)).collect();
        Self {
            columns: groups(&projected, 0),
            rows: groups(&projected, 1),
            bounds: projected.iter().map(bounds).collect(),
            writing,
        }
    }

    /// The column traversal, without deciding which traversal is intended.
    pub fn column_order(&self) -> Vec<usize> {
        self.columns.iter().flatten().copied().collect()
    }

    /// The row traversal, without deciding which traversal is intended.
    pub fn row_order(&self) -> Vec<usize> {
        self.rows.iter().flatten().copied().collect()
    }

    /// Reindexes existing evidence to a permutation without clustering again.
    pub fn reindexed(&self, order: &[usize]) -> Self {
        assert_eq!(order.len(), self.bounds.len());
        self.selected(order)
    }

    /// Retains selected observations in the supplied order, preserving their
    /// existing group membership without clustering again. Removed fragments
    /// cannot contribute bounds or scale to subsequent reach components.
    pub fn selected(&self, order: &[usize]) -> Self {
        let mut inverse = vec![None; self.bounds.len()];
        for (new, &old) in order.iter().enumerate() {
            assert!(inverse[old].is_none(), "layout needs unique observations");
            inverse[old] = Some(new);
        }
        let remap = |groups: &[Vec<usize>]| {
            groups
                .iter()
                .map(|g| g.iter().filter_map(|&i| inverse[i]).collect())
                .collect()
        };
        Self {
            columns: remap(&self.columns),
            rows: remap(&self.rows),
            bounds: order.iter().map(|&i| self.bounds[i]).collect(),
            writing: self.writing,
        }
    }

    /// Eligible observations linked by the existing height-scaled reach rule.
    /// Ineligible observations do not bridge components or set their scale.
    pub fn reach_groups(&self, eligible: &[bool]) -> Vec<Vec<usize>> {
        self.reach_groups_with_scale(eligible).0
    }

    pub(crate) fn observation_bounds(&self, i: usize) -> [f32; 4] {
        self.bounds[i]
    }

    pub(crate) fn reach_groups_with_scale(
        &self,
        eligible: &[bool],
    ) -> (Vec<Vec<usize>>, Option<f32>) {
        assert_eq!(eligible.len(), self.bounds.len());
        let boxes: Vec<_> = self
            .bounds
            .iter()
            .copied()
            .enumerate()
            .filter(|&(i, _)| eligible[i])
            .collect();
        if boxes.is_empty() {
            return (Vec::new(), None);
        }
        let mut heights: Vec<_> = boxes.iter().map(|(_, b)| b[3] - b[1]).collect();
        heights.sort_by(f32::total_cmp);
        let h = heights[heights.len() / 2].max(1.0);
        let (dx, dy) = (COLUMN_REACH * h, ROW_REACH * h);
        let mut parent: Vec<usize> = (0..boxes.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for a in 0..boxes.len() {
            for b in a + 1..boxes.len() {
                let (p, q) = (boxes[a].1, boxes[b].1);
                if p[0] - dx < q[2] + dx
                    && q[0] - dx < p[2] + dx
                    && p[1] - dy < q[3] + dy
                    && q[1] - dy < p[3] + dy
                {
                    let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                    parent[ra] = rb;
                }
            }
        }
        let mut groups = vec![Vec::new(); boxes.len()];
        for (k, &(original, _)) in boxes.iter().enumerate() {
            let r = root(&mut parent, k);
            groups[r].push(original);
        }
        (
            groups.into_iter().filter(|g| !g.is_empty()).collect(),
            Some(h),
        )
    }
}

fn bounds(q: &Quad) -> [f32; 4] {
    let xs = q.0.iter().map(|p| p.0);
    let ys = q.0.iter().map(|p| p.1);
    [
        xs.clone().fold(f32::MAX, f32::min),
        ys.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
        ys.fold(f32::MIN, f32::max),
    ]
}

// This is the original split.rs ordering algorithm, parameterized by axis.
fn groups(quads: &[Quad], axis: usize) -> Vec<Vec<usize>> {
    let coord = |p: &(f32, f32), axis| if axis == 0 { p.0 } else { p.1 };
    let centre = |q: &Quad, axis| q.0.iter().map(|p| coord(p, axis)).sum::<f32>() / 4.0;
    let span = |q: &Quad| {
        (
            q.0.iter().map(|p| coord(p, axis)).fold(f32::MAX, f32::min),
            q.0.iter().map(|p| coord(p, axis)).fold(f32::MIN, f32::max),
        )
    };
    let mut sorted: Vec<usize> = (0..quads.len()).collect();
    sorted.sort_by(|&a, &b| span(&quads[a]).0.total_cmp(&span(&quads[b]).0));
    let mut groups: Vec<((f32, f32), Vec<usize>)> = Vec::new();
    for i in sorted {
        let c = centre(&quads[i], axis);
        let (lo, hi) = span(&quads[i]);
        match groups.iter_mut().find(|(s, _)| c >= s.0 && c <= s.1) {
            Some((s, members)) => {
                s.0 = s.0.min(lo);
                s.1 = s.1.max(hi);
                members.push(i);
            }
            None => groups.push(((lo, hi), vec![i])),
        }
    }
    groups.sort_by(|a, b| a.0.0.total_cmp(&b.0.0));
    groups
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_by(|&a, &b| {
                centre(&quads[a], 1 - axis).total_cmp(&centre(&quads[b], 1 - axis))
            });
            members
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32, y: f32) -> Quad {
        Quad([(x, y), (x + 50., y), (x + 50., y + 10.), (x, y + 10.)])
    }

    #[test]
    fn selecting_parts_preserves_groups_but_removes_reach_bridges() {
        let quads = [quad(0., 0.), quad(0., 25.), quad(0., 50.)];
        let layout = PageLayout::new(&quads);
        assert_eq!(layout.reach_groups(&[true; 3]), [vec![0, 1, 2]]);
        let selected = layout.selected(&[2, 0]);
        assert_eq!(selected.columns, [vec![1, 0]]);
        assert_eq!(selected.rows, [vec![1], vec![], vec![0]]);
        assert_eq!(selected.reach_groups(&[true; 2]), [vec![0], vec![1]]);
        assert!(layout.selected(&[]).reach_groups(&[]).is_empty());
    }

    #[test]
    fn groups_and_reindexing_preserve_both_original_traversals() {
        let quads = [quad(100., 50.), quad(0., 0.), quad(100., 0.), quad(0., 50.)];
        let layout = PageLayout::new(&quads);
        assert_eq!(layout.columns, [vec![1, 3], vec![2, 0]]);
        assert_eq!(layout.rows, [vec![1, 2], vec![3, 0]]);
        let columns = layout.column_order();
        let reindexed = layout.reindexed(&columns);
        assert_eq!(reindexed.column_order(), [0, 1, 2, 3]);
        assert_eq!(reindexed.row_order(), [0, 2, 1, 3]);
        let normalize = |mut groups: Vec<Vec<usize>>| {
            for group in &mut groups {
                group.sort_unstable();
            }
            groups.sort();
            groups
        };
        let mapped = reindexed
            .reach_groups(&[true; 4])
            .iter()
            .map(|g| g.iter().map(|&i| columns[i]).collect())
            .collect();
        assert_eq!(
            normalize(mapped),
            normalize(layout.reach_groups(&[true; 4]))
        );
        assert!(PageLayout::new(&[]).column_order().is_empty());
        assert!(PageLayout::new(&[]).reach_groups(&[]).is_empty());
    }
}
