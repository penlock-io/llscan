//! Repair a compact two-fragment row before either fragment is called a label.
//! This is not a grid fallback: it supplies the same retention owner as a cell,
//! but only for the exact union, with no inferred ordinal or search domain.
use super::*;
use crate::numbering::LABEL_ASPECT_LIMIT;

impl Grid {
    pub(in crate::phrase) fn compact_word(&self, q: &Quad) -> bool {
        self.compact_words.iter().any(|word| word == q)
    }

    pub(in crate::phrase) fn compact_groups(
        &mut self,
        photo: &RgbImage,
        ordinary: &[(usize, Quad)],
        writing: WritingFrame,
    ) -> Vec<(Vec<usize>, Quad)> {
        // Established cells already own all their fragments. Do not introduce
        // a competing grouping rule on partially or fully gridded pages.
        if !self.columns.is_empty() {
            return Vec::new();
        }
        let quads: Vec<_> = ordinary.iter().map(|(_, q)| q.clone()).collect();
        let layout = crate::layout::PageLayout::with_writing(&quads, writing);
        let trials = crate::joins::candidates_observed(&layout, |_, _| {});
        let mut groups = Vec::new();
        for trial in trials {
            let [a, b] = trial.parents;
            let left = Bounds::of(&writing.project(&quads[a]));
            let right = Bounds::of(&writing.project(&quads[b]));
            let height = left.h().max(right.h());
            let gap = right.l - left.r;
            let overlap = left.b.min(right.b) - left.t.max(right.t);
            let union = crate::joins::union_in(&quads[a], &quads[b], writing);
            let bounds = Bounds::of(&writing.project(&union));
            let compact = |b: Bounds| b.w() >= b.h() && b.w() < LABEL_ASPECT_LIMIT * b.h();
            let protected = |q: &Quad| {
                self.exclusion_read(q, writing, None).is_some()
                    || self.label(q, writing).is_some()
                    || self
                        .cached(&word_crop(photo, writing.crop_frame(q, 0.), q, 0.))
                        .is_some_and(|r| numeric(&r).is_some())
            };
            let mut overlap_ink = None;
            let reason = if trial.reason != "candidate" {
                "row_structure"
            } else if !compact(left) || !compact(right) {
                "not_two_compact_fragments"
            } else if bounds.w() < LABEL_ASPECT_LIMIT * bounds.h() {
                "union_still_label_sized"
            } else if gap > 0.5 * height {
                "gap_too_large"
            } else if overlap < 0.6 * left.h().min(right.h()) {
                "insufficient_row_overlap"
            } else if bounds.w() as f64 * bounds.h() as f64 > crate::joins::MAX_CROP_PIXELS as f64 {
                "crop_budget"
            } else if protected(&quads[a]) || protected(&quads[b]) || protected(&union) {
                "label_evidence"
            } else if retention::word_ink_pixels(photo, &quads[a], writing) == 0
                || retention::word_ink_pixels(photo, &quads[b], writing) == 0
            {
                "empty_fragment"
            } else if gap < 0. && {
                let ink = super::overlap_ink::measure(photo, &quads[a], &quads[b], writing);
                let bridges = ink.bridges();
                overlap_ink = Some(ink);
                !bridges
            } {
                "no_shared_connected_stroke"
            } else {
                // Reuse the ordinary ownership ink check. No ink is sought
                // outside this union, and no probability affects the decision.
                self.compact_words.push(union.clone());
                if self.word_cell(photo, &union, writing).is_some() {
                    groups.push((vec![ordinary[a].0, ordinary[b].0], union.clone()));
                    "assembled"
                } else {
                    self.compact_words.pop();
                    "no_word_ink"
                }
            };
            DecisionTrace::push(&mut self.decisions, || {
                json!({
                    "rule":"compact_row_assembly", "status":"evaluated", "reason":reason,
                    "parents":[ordinary[a].0,ordinary[b].0], "parent_quads":[quads[a].0,quads[b].0],
                    "union_quad":union.0, "gap":gap, "height":height, "vertical_overlap":overlap,
                    "overlap_ink":overlap_ink.as_ref().map(super::overlap_ink::Evidence::to_json),
                    "observation_count":ordinary.len(),"one_above_standard_count":matches!(ordinary.len(),13|25),
                    "column_group":trial.column, "row_group":trial.row,
                    "thresholds":{"max_gap_height":0.5,"min_overlap_height":0.6,
                        "min_fragment_aspect":1.0,"label_aspect_limit":LABEL_ASPECT_LIMIT},
                    "support":"one_pair_row_and_at_least_two_singleton_rows_in_same_column",
                    "ownership":"same_retention_rule_as_cells_no_inferred_ordinal",
                    "recognition":"exact_union_zero_padding_no_prefix_recut", "extra_ocr_calls":0
                })
            });
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(l: f32, t: f32, w: f32, h: f32) -> Quad {
        Bounds {
            l,
            t,
            r: l + w,
            b: t + h,
        }
        .quad()
    }
    fn parts() -> Vec<(usize, Quad)> {
        vec![
            (0, rect(30., 20., 28., 20.)),
            (1, rect(63., 20., 35., 20.)),
            (2, rect(25., 60., 90., 24.)),
            (3, rect(20., 100., 95., 25.)),
        ]
    }
    fn ink() -> RgbImage {
        let mut photo = RgbImage::from_pixel(150, 150, image::Rgb([255; 3]));
        for y in 23..38 {
            for x in 32..96 {
                if (x + y) % 7 < 3 {
                    photo.put_pixel(x, y, image::Rgb([20; 3]));
                }
            }
        }
        photo
    }
    #[test]
    fn compact_union_gets_existing_owner_but_no_grid_or_ordinal() {
        let photo = ink();
        let mut grid = Grid::default();
        let input = parts();
        let groups = grid.compact_groups(&photo, &input, WritingFrame::Local);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, vec![0, 1]);
        let support = grid
            .word_cell(&photo, &groups[0].1, WritingFrame::Local)
            .unwrap();
        assert_eq!(support.basis, "compact_row_fragments_and_word_ink");
        assert!(support.column.is_none() && support.row.is_none() && support.ordinal.is_none());
        assert!(support.ink_pixels > 0);
        assert!(!grid.compact_word(&input[0].1));
        assert!(
            !grid.compact_word(&rect(29., 20., 70., 20.)),
            "ownership is not transferable to other boxes"
        );
    }
    #[test]
    fn compact_rule_refuses_other_words_labels_gaps_overlaps_and_ambiguous_rows() {
        let photo = ink();
        for case in 0..9 {
            let mut photo = photo.clone();
            let mut input = parts();
            let mut grid = Grid::default();
            match case {
                0 => {
                    input.pop();
                } // too few supporting rows
                1 => input[0].1 = rect(10., 20., 48., 20.), // already word-sized
                2 => input[1].1 = rect(90., 20., 35., 20.), // inter-word gap
                3 => {
                    input[1].1 = rect(55., 20., 35., 20.);
                    for y in 0..photo.height() {
                        for x in 55..59 {
                            photo.put_pixel(x, y, image::Rgb([255; 3]));
                        }
                    }
                } // disconnected ink does not establish ownership
                4 => input[1].1 = rect(63., 35., 35., 20.), // distinct row
                5 => grid.labels.push(Bounds::of(&input[0].1)),
                6 => {
                    grid.cache.get_mut().unwrap().insert(
                        key(&word_crop(
                            &photo,
                            WritingFrame::Local.crop_frame(&input[0].1, 0.),
                            &input[0].1,
                            0.,
                        )),
                        LineRead {
                            text: "3.".into(),
                            confidence: 0.99,
                        },
                    );
                }
                7 => input.push((4, rect(65., 60., 30., 24.))), // two fragmented rows
                8 => grid.columns.push(ColumnBounds {
                    intercept: 0.,
                    slope: 0.,
                    right: 120.,
                    right_slope: 0.,
                    top: 0.,
                    bottom: 150.,
                }),
                _ => unreachable!(),
            }
            assert!(
                grid.compact_groups(&photo, &input, WritingFrame::Local)
                    .is_empty(),
                "case {case}"
            );
        }
        assert!(
            Grid::default()
                .compact_groups(
                    &RgbImage::from_pixel(150, 150, image::Rgb([255; 3])),
                    &parts(),
                    WritingFrame::Local
                )
                .is_empty()
        );
    }

    #[test]
    fn compact_geometry_is_shared_page_direction_not_photo_x() {
        for angle in [0., 12., 90., 180., 270.] {
            let writing = WritingFrame::Page(crate::page_frame::test_direction(angle));
            let input: Vec<_> = parts()
                .into_iter()
                .map(|(i, q)| (i, writing.unproject(&q)))
                .collect();
            let quads: Vec<_> = input.iter().map(|(_, q)| q.clone()).collect();
            let layout = crate::layout::PageLayout::with_writing(&quads, writing);
            let trials = crate::joins::candidates_observed(&layout, |_, _| {});
            assert_eq!(trials.len(), 1);
            assert_eq!(trials[0].reason, "candidate");
            let q = crate::joins::union_in(&quads[0], &quads[1], writing);
            let bounds = Bounds::of(&writing.project(&q));
            assert!((bounds.w() - 68.).abs() < 0.001);
            assert!((bounds.h() - 20.).abs() < 0.001);
        }
    }

    // Native segmentation and native grouping on all saved detector polygons.
    // Models are neither loaded nor called. Existing grid pages must be no-ops;
    // the rest are the complete exposure, not just the motivating photograph.
}
