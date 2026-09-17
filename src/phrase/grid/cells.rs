//! One recognition extent per occupied, established grid cell.
use super::*;

// Cell membership is a search boundary, not permission to bridge an arbitrarily
// large blank interval. Keep the leading horizontal group; a distant tail is
// consumed by this cell decision but contributes no pixels to its one output.
fn leading_group(members: &[(usize, Quad)], writing: WritingFrame) -> (Vec<usize>, Bounds) {
    let mut ordered: Vec<_> = members
        .iter()
        .map(|(i, q)| (*i, Bounds::of(&writing.project(q))))
        .collect();
    ordered.sort_by(|a, b| a.1.l.total_cmp(&b.1.l).then(a.0.cmp(&b.0)));
    let (first, mut union) = ordered[0];
    let mut included = vec![first];
    for &(i, b) in &ordered[1..] {
        let gap = b.l - union.r;
        // Use both sides: a tiny first fragment must not set the scale and
        // incorrectly exclude the rest of a normal-sized word (e.g. its stem).
        if gap > union.h().max(b.h()).max(0.5 * union.w()).max(0.5 * b.w()) {
            break;
        }
        included.push(i);
        union.l = union.l.min(b.l);
        union.r = union.r.max(b.r);
        union.t = union.t.min(b.t);
        union.b = union.b.max(b.b);
    }
    (included, union)
}

pub(super) fn complete_rows(rows: &mut Vec<f32>, column: ColumnBounds) {
    if rows.len() < 2 {
        return;
    }
    let step = median(rows.windows(2).map(|r| r[1] - r[0]).collect());
    if !step.is_finite() || step <= 1. {
        return;
    }
    let mut y = rows[0] - step;
    let mut before = Vec::new();
    while y >= column.top {
        before.push(y);
        y -= step;
    }
    before.reverse();
    before.append(rows);
    y = before[before.len() - 1] + step;
    while y <= column.bottom {
        before.push(y);
        y += step;
    }
    *rows = before;
}

impl Grid {
    pub(in crate::phrase) fn cell_groups(
        &mut self,
        photo: &RgbImage,
        ordinary: &[(usize, Quad)],
        writing: WritingFrame,
        _margin: f32,
    ) -> Vec<(Vec<usize>, Quad)> {
        let mut groups: std::collections::BTreeMap<(usize, usize), Vec<(usize, Quad)>> =
            Default::default();
        for (i, q) in ordinary {
            // Geometry owns membership. A blank/numeric fragment read cannot
            // veto ink in the word cell: recognize the assembled word instead.
            // Labels are excluded by their measured geometry, not this read.
            if self.exclusion_read(q, writing, None).is_some() {
                continue;
            }
            let effective = self.constrain(q, writing, 0.).0;
            let b = Bounds::of(&writing.project(&effective));
            let Some((column, _)) = self.columns.iter().enumerate().find(|(_, c)| {
                b.y() >= c.top
                    && b.y() <= c.bottom
                    && b.l >= c.left_limit(b) - 0.01
                    && b.r <= c.right_limit(b) + 0.01
            }) else {
                continue;
            };
            let Some(rows) = self.row_centres.get(column).filter(|r| r.len() >= 2) else {
                continue;
            };
            let row = (0..rows.len())
                .min_by(|&a, &brow| {
                    (rows[a] - b.y())
                        .abs()
                        .total_cmp(&(rows[brow] - b.y()).abs())
                })
                .unwrap();
            // The outer cells terminate at the established list extent, not
            // at a guessed extra row. Their vertical bounds own centres only.
            groups
                .entry((column, row))
                .or_default()
                .push((*i, effective));
        }
        groups
            .into_iter()
            .filter_map(|((column, row), members)| {
                if members.len() < 2 {
                    return None;
                }
                let (included, mut union) = leading_group(&members, writing);
                DecisionTrace::push(&mut self.decisions, || {
                    json!({"rule":"grid_cell_merge_gap","status":"evaluated",
                        "column":column,"row":row,"source_index_space":"after_column_partition_before_cell_coalescence",
                        "included_observations":included,
                        "excluded_observations":members.iter().filter(|(i,_)|!included.contains(i)).map(|(i,_)|i).collect::<Vec<_>>(),
                        "members":members.iter().map(|(i,q)|json!({"observation":i,"quad":q.0})).collect::<Vec<_>>(),
                        "extent_quad":writing.unproject(&union.quad()).0,
                        "max_gap":"max(left_height, right_height, 0.5 * left_width, 0.5 * right_width)",
                        "reason":"leading_group_only; distant_tail_consumed_without_independent_read",
                        "recognition_independent":true})
                });
                // The cell owns the nearby detected fragments, not every dark pixel
                // across its legal column. Bound the read by their union:
                // expanding to the photo edge admits table/background ink that
                // whitespace trimming cannot distinguish from the word.
                let q = writing.unproject(&union.quad());
                let f = writing.crop_frame(&q, 0.);
                let crop = level_crop_in(photo, f);
                let mut ink = source_ink(photo, &q, f, &crop);
                strip_rules(&mut ink);
                let mut lo = usize::MAX;
                let mut hi = 0;
                for row in ink {
                    for (x, dark) in row.into_iter().enumerate() {
                        if dark {
                            lo = lo.min(x);
                            hi = hi.max(x);
                        }
                    }
                }
                if lo == usize::MAX {
                    return None;
                }
                let origin = (union.l + union.r - crop.width() as f32) / 2.;
                let guard = std::f32::consts::FRAC_1_SQRT_2;
                union.l = (origin + lo as f32 - guard).max(union.l);
                union.r = (origin + (hi + 1) as f32 + guard).min(union.r);
                let q = self
                    .constrain(&writing.unproject(&union.quad()), writing, 0.)
                    .0;
                // The supported cell bounds an ink search, never a padded read.
                let q = if let Some(cell) = self
                    .row_support
                    .get(column)
                    .and_then(|rows| rows.get(row))
                    .and_then(Option::as_ref)
                {
                    let (recovered, detail) = super::recovery::recover_traced(
                        photo,
                        &q,
                        &writing.unproject(&cell.bounds.quad()),
                        writing,
                        self.decisions.is_some(),
                    );
                    let recovered = self.constrain(&recovered, writing, 0.).0;
                    DecisionTrace::push(&mut self.decisions, || {
                        json!({
                        "rule":"grid_stroke_recovery","status":"evaluated",
                        "column":column,"row":row,"detail":detail,"quad":recovered.0,
                        "recognition":"same_output_quad_zero_padding"})
                    });
                    recovered
                } else {
                    q
                };
                Some((members.iter().map(|m| m.0).collect(), q))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distant_tail_is_not_word_extent_but_close_terminal_fragments_are() {
        let rect = |l, r| {
            Bounds {
                l,
                r,
                t: 10.,
                b: 50.,
            }
            .quad()
        };
        // Input order must not change ownership. The small terminal fragment
        // belongs; the second column's sliver across a word-sized gap does not.
        let members = vec![
            (9, rect(300., 310.)),
            (3, rect(10., 110.)),
            (5, rect(115., 130.)),
            (10, rect(312., 320.)),
        ];
        let (ids, b) = leading_group(&members, WritingFrame::Local);
        assert_eq!(ids, vec![3, 5]);
        assert_eq!((b.l, b.r), (10., 130.));
        // A short first fragment is protected by the height floor.
        let (ids, _) = leading_group(
            &[(0, rect(0., 10.)), (1, rect(35., 110.))],
            WritingFrame::Local,
        );
        assert_eq!(ids, vec![0, 1]);
        // Overlapping detector fragments never fail the gap rule.
        let (ids, _) = leading_group(
            &[(0, rect(0., 90.)), (1, rect(70., 130.))],
            WritingFrame::Local,
        );
        assert_eq!(ids, vec![0, 1]);
        // A tiny leading mark cannot set the scale and discard the actual word.
        let tiny = Bounds {
            l: 0.,
            r: 5.,
            t: 25.,
            b: 35.,
        }
        .quad();
        let word = Bounds {
            l: 20.,
            r: 165.,
            t: 10.,
            b: 70.,
        }
        .quad();
        let (ids, _) = leading_group(&[(0, tiny), (1, word)], WritingFrame::Local);
        assert_eq!(ids, vec![0, 1]);
    }

    #[test]
    fn distant_cell_fragment_is_consumed_without_expanding_the_one_output() {
        let rect = |l, r| {
            Bounds {
                l,
                r,
                t: 20.,
                b: 60.,
            }
            .quad()
        };
        let mut grid = Grid {
            columns: vec![ColumnBounds {
                intercept: 0.,
                slope: 0.,
                right: 500.,
                right_slope: 0.,
                top: 0.,
                bottom: 200.,
            }],
            row_centres: vec![vec![40., 140.]],
            ..Grid::default()
        };
        let mut photo = RgbImage::from_pixel(500, 200, image::Rgb([240; 3]));
        for y in 25..55 {
            for x in (35..110).chain(300..310) {
                if x >= 300 || x % 15 < 5 {
                    photo.put_pixel(x, y, image::Rgb([20; 3]));
                }
            }
        }
        let groups = grid.cell_groups(
            &photo,
            &[(0, rect(30., 120.)), (1, rect(295., 315.))],
            WritingFrame::Local,
            0.,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].0,
            vec![0, 1],
            "no second word for the discarded distant tail"
        );
        assert!(Bounds::of(&groups[0].1).r < 121.);
        grid.decisions = Some(DecisionTrace::default());
        let traced = grid.cell_groups(
            &photo,
            &[(0, rect(30., 120.)), (1, rect(295., 315.))],
            WritingFrame::Local,
            0.,
        );
        assert_eq!(groups, traced, "diagnostics must not affect geometry");
        let trace = grid.decisions.as_ref().unwrap().to_json();
        let event = &trace["events"][0];
        assert_eq!(event["rule"], "grid_cell_merge_gap");
        assert_eq!(event["included_observations"], json!([0]));
        assert_eq!(event["excluded_observations"], json!([1]));
        assert_eq!(
            event["source_index_space"],
            "after_column_partition_before_cell_coalescence"
        );
    }

    #[test]
    fn recovery_requires_supported_cell_and_keeps_one_group() {
        let rect = |l, t, r, b| Bounds { l, t, r, b }.quad();
        let c = ColumnBounds {
            intercept: 20.,
            slope: 0.,
            right: 140.,
            right_slope: 0.,
            top: 20.,
            bottom: 180.,
        };
        let mut grid = Grid {
            columns: vec![c],
            row_centres: vec![vec![60., 140.]],
            ..Grid::default()
        };
        let mut photo = RgbImage::from_pixel(180, 200, image::Rgb([240; 3]));
        for y in 30..75 {
            for x in 40..46 {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        for y in 50..55 {
            for x in 40..80 {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        for y in 42..65 {
            for x in 70..75 {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        let words = vec![(0, rect(60., 40., 75., 70.)), (1, rect(70., 40., 90., 70.))];
        let ordinary = grid.cell_groups(&photo, &words, WritingFrame::Local, 0.);
        assert_eq!(ordinary.len(), 1);
        assert!(Bounds::of(&ordinary[0].1).l >= 60.);
        grid.install_cells(
            &[(
                c,
                json!({"row_fit":{"accepted":true},"label_gutter_width":15.,
            "rows":[{"ordinal":1},{"ordinal":2}]}),
            )],
            &[],
            WritingFrame::Local,
        );
        assert!(
            grid.cell_groups(&photo, &words[..1], WritingFrame::Local, 0.)
                .is_empty(),
            "single-fragment controls do not enter the new recovery hook"
        );
        let recovered = grid.cell_groups(&photo, &words, WritingFrame::Local, 0.);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].0, vec![0, 1]);
        assert!(Bounds::of(&recovered[0].1).l < 41.);
    }
    #[test]
    fn merged_word_does_not_capture_dark_table_in_legal_column() {
        let mut grid = Grid {
            columns: vec![ColumnBounds {
                intercept: 10.,
                slope: 0.,
                right: 500.,
                right_slope: 0.,
                top: 0.,
                bottom: 200.,
            }],
            row_centres: vec![vec![40., 140.]],
            ..Default::default()
        };
        let ordinary = vec![
            (
                0,
                Bounds {
                    l: 30.,
                    r: 80.,
                    t: 20.,
                    b: 60.,
                }
                .quad(),
            ),
            (
                1,
                Bounds {
                    l: 85.,
                    r: 120.,
                    t: 20.,
                    b: 60.,
                }
                .quad(),
            ),
        ];
        let mut photo = RgbImage::from_pixel(500, 200, image::Rgb([240; 3]));
        for y in 0..200 {
            for x in 0..500 {
                if x >= 300
                    || ((25..55).contains(&y) && ((40..70).contains(&x) || (90..110).contains(&x)))
                {
                    photo.put_pixel(x, y, image::Rgb([20; 3]));
                }
            }
        }
        let groups = grid.cell_groups(&photo, &ordinary, WritingFrame::Local, 0.);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, vec![0, 1]);
        let bounds = Bounds::of(&groups[0].1);
        assert!(bounds.l >= 30. && bounds.r <= 120.);
        assert_eq!((bounds.t, bounds.b), (20., 60.));
    }

    #[test]
    fn a_cell_has_one_extent_even_when_its_fragments_do_not_overlap_vertically() {
        let mut grid = Grid {
            columns: vec![ColumnBounds {
                right_slope: 0.,
                intercept: 0.,
                slope: 0.,
                right: 400.,
                top: 0.,
                bottom: 500.,
            }],
            row_centres: vec![vec![150., 400.]],
            ..Grid::default()
        };
        let words = vec![
            (
                0,
                Bounds {
                    l: 30.,
                    t: 60.,
                    r: 180.,
                    b: 130.,
                }
                .quad(),
            ),
            (
                1,
                Bounds {
                    l: 30.,
                    t: 150.,
                    r: 160.,
                    b: 220.,
                }
                .quad(),
            ),
        ];
        let mut photo = RgbImage::from_pixel(400, 500, image::Rgb([240; 3]));
        for y in [80, 170] {
            for x in 50..70 {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        let groups = grid.cell_groups(&photo, &words, WritingFrame::Local, 0.);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, vec![0, 1]);
    }
    #[test]
    fn same_cell_fragments_merge_but_neighbours_and_gutters_do_not() {
        for angle in [0., 15.] {
            let map = |x, y| {
                let (x, y) = turn(x, y, angle);
                (x + 150., y + 150.)
            };
            let make = |l, t, r, b| Quad([(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| map(x, y)));
            let anchor = make(0., 0., 300., 60.);
            let writing = WritingFrame::Page(
                crate::page_frame::PageAxis::from_detections(&[anchor]).direction(false),
            );
            let origin = writing.project(&Quad([map(0., 0.); 4])).0[0];
            let c = ColumnBounds {
                right_slope: 0.,
                intercept: origin.0 + 20.,
                slope: 0.,
                right: origin.0 + 190.,
                top: origin.1 - 10.,
                bottom: origin.1 + 220.,
            };
            let mut grid = Grid {
                columns: vec![
                    c,
                    ColumnBounds {
                        intercept: origin.0 + 220.,
                        right: origin.0 + 350.,
                        ..c
                    },
                ],
                row_centres: vec![vec![origin.1 + 30., origin.1 + 130.]; 2],
                ..Grid::default()
            };
            let ordinary = vec![
                (0, make(30., 0., 130., 60.)),
                (1, make(150., 0., 180., 60.)),
                (2, make(30., 100., 170., 160.)),
                (3, make(230., 0., 330., 60.)),
                (4, make(0., 0., 15., 60.)),
            ];
            let mut photo = RgbImage::from_pixel(650, 500, image::Rgb([240; 3]));
            for (l, r) in [(40, 120), (155, 175)] {
                for y in 10..50 {
                    for x in l..r {
                        if (x - l) % 15 > 6 {
                            continue;
                        }
                        let (x, y) = map(x as f32, y as f32);
                        photo.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                    }
                }
            }
            let groups = grid.cell_groups(&photo, &ordinary, writing, 0.15);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].0, vec![0, 1]);
            let b = Bounds::of(&writing.project(&groups[0].1));
            assert!(b.l > origin.0 + 30. && b.r < origin.0 + 181.);
            assert!((b.t - origin.1).abs() < 0.01 && (b.b - origin.1 - 60.).abs() < 0.01);
            let crop = level_crop_in(&photo, writing.crop_frame(&ordinary[1].1, 0.15));
            for literal in ["", "3)", "\""] {
                grid.cache.lock().unwrap().insert(
                    key(&crop),
                    LineRead {
                        text: literal.into(),
                        confidence: 0.5,
                    },
                );
                let groups = grid.cell_groups(&photo, &ordinary, writing, 0.15);
                assert_eq!(groups.len(), 1, "OCR {literal:?} cannot veto membership");
                assert_eq!(groups[0].0, vec![0, 1]);
            }
            grid.cache.lock().unwrap().insert(
                key(&crop),
                LineRead {
                    text: "e".into(),
                    confidence: 0.9,
                },
            );
            assert_eq!(grid.cell_groups(&photo, &ordinary, writing, 0.15).len(), 1);
            // Ink beyond every detected fragment is not owned merely because
            // it lies inside the column. A separate detected terminal fragment
            // DOES extend this cell's one box, regardless of its OCR result.
            for (l, r) in [(183, 188), (5, 15), (230, 245)] {
                for y in 15..45 {
                    for x in l..r {
                        let (x, y) = map(x as f32, y as f32);
                        photo.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                    }
                }
            }
            let groups = grid.cell_groups(&photo, &ordinary, writing, 0.15);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].0, vec![0, 1]);
            let b = Bounds::of(&writing.project(&groups[0].1));
            assert!(b.r <= origin.0 + 180.01);
            assert!(b.l > origin.0 + 30.);
            assert!((b.t - origin.1).abs() < 0.01 && (b.b - origin.1 - 60.).abs() < 0.01);
            let mut with_terminal = ordinary.clone();
            with_terminal.push((5, make(181., 0., 190., 60.)));
            let groups = grid.cell_groups(&photo, &with_terminal, writing, 0.);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].0, vec![0, 1, 5]);
            let b = Bounds::of(&writing.project(&groups[0].1));
            assert!(b.r >= origin.0 + 188. && b.r <= origin.0 + 190.01);
        }
    }

    #[test]
    fn rows_extend_over_column_before_cell_assignment() {
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: 0.,
            slope: 0.,
            right: 300.,
            top: 400.,
            bottom: 1250.,
        };
        let mut rows = vec![890., 980., 1062., 1148.];
        complete_rows(&mut rows, c);
        assert!(rows.iter().any(|y| (*y - 718.).abs() < 1.));
        assert!(rows.iter().any(|y| (*y - 804.).abs() < 1.));
        let nearest = |y: f32| {
            (0..rows.len())
                .min_by(|&a, &b| (rows[a] - y).abs().total_cmp(&(rows[b] - y).abs()))
                .unwrap()
        };
        assert_ne!(nearest(718.), nearest(804.));
        assert_ne!(nearest(804.), nearest(890.));
        assert_ne!(nearest(460.), nearest(718.));
    }
}
