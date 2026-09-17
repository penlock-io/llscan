use super::*;
use crate::page_frame::test_direction;
use image::{Luma, Rgb};

#[test]
fn component_filter_exposes_dotted_prefix_without_erasing_the_crop() {
    let mut source = vec![vec![false; 120]; 90];
    for row in &mut source[30..70] {
        row[10..20].fill(true);
    }
    for row in &mut source[65..70] {
        row[30..35].fill(true);
    }
    for row in &mut source[10..80] {
        row[60..100].fill(true);
    }
    // Isolated paper speckles occupy every x, defeating an all-white gap test.
    for x in 0..120 {
        source[1 + (x % 3) * 3][x] = true;
    }
    assert!(dotted_prefix_ink(source.clone()).is_none());
    assert!(prefix_ink_ranked(source.clone(), false).is_none());
    let mut proposal_mask = source.clone();
    assert_eq!(ink_width::filter_small(&mut proposal_mask), (120, 120));
    let (label, gap) = dotted_prefix_ink(proposal_mask).unwrap();
    assert_eq!((label.l, label.r, gap), (10., 35., 25.));
    assert!(source[1][0]); // Original recognition pixels are untouched.
    assert!(source[67][32]); // The actual dot survives.
}

fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        l: x,
        t: y,
        r: x + w,
        b: y + h,
    }
}
fn anchor(n: u32, x: f32) -> Anchor {
    Anchor {
        source: n as usize,
        bounds: b(x, 30. * n as f32, 10., 10.),
        ordinal: Some(n),
        literal: format!("{n}."),
        confidence: 0.99,
        split: None,
    }
}

#[test]
fn column_right_follows_next_gutter_or_widest_row_allowance_without_expanding_words() {
    let photo = RgbImage::new(320, 180);
    let first = ColumnBounds {
        right_slope: 0.,
        intercept: 20.,
        slope: 0.,
        right: 100.,
        top: 0.,
        bottom: 160.,
    };
    let next = ColumnBounds {
        intercept: 200.,
        slope: 0.1,
        right: 250.,
        ..first
    };
    let mut guides = vec![
        (first, json!({"label_gutter_width":15.,"rows":[]})),
        (
            next,
            json!({"label_gutter_width":30.,"rows":[],"word_layout_members":[0,1]}),
        ),
    ];
    let observations = vec![
        b(220., 40., 80., 30.).quad(),
        b(230., 100., 30., 30.).quad(),
    ];
    layout::extend_right_domains(&mut guides, &photo, WritingFrame::Local, &observations);
    assert_eq!(guides[0].0.right, 170.);
    assert_eq!(guides[0].0.right_at(160.), 186.);
    assert_eq!(guides[0].0.right_slope, next.slope);
    assert_eq!(guides[1].0.right, 316.); // 300 + 20% of the 80px widest word
    assert_eq!(guides[1].0.right_slope, 0.); // not forced parallel to its left
    let grid = Grid {
        columns: guides.iter().map(|v| v.0).collect(),
        ..Default::default()
    };
    // The long detected word survives the formerly estimated x=100 endpoint.
    let q = b(30., 40., 125., 30.).quad();
    let (effective, frame, _) = grid.constrain(&q, WritingFrame::Local, 0.15);
    assert_eq!(effective, q);
    assert!(frame.cx + frame.w / 2. > 155. && frame.cx + frame.w / 2. < 174.);
    let crossing = grid
        .constrain(&b(30., 40., 170., 30.).quad(), WritingFrame::Local, 0.)
        .0;
    assert_eq!(Bounds::of(&crossing).r, 174.);
    let final_word = b(220., 40., 80., 30.).quad();
    assert_eq!(
        grid.constrain(&final_word, WritingFrame::Local, 0.).0,
        final_word
    );
}

#[test]
fn extended_domain_stops_at_rotated_photo_edge_without_clipping_height() {
    for angle in [-17., 17., 90., 180.] {
        let writing = WritingFrame::Page(test_direction(angle));
        let grid = Grid {
            photo_size: Some((320, 180)),
            ..Default::default()
        };
        let (cx, cy) = writing.project(&Quad([(160., 90.); 4])).0[0];
        let q = writing.unproject(&b(cx - 180., cy - 10., 360., 20.).quad());
        let before = Bounds::of(&writing.project(&q));
        let (effective, _, _) = grid.constrain(&q, writing, 0.);
        let after = Bounds::of(&writing.project(&effective));
        assert!((before.t - after.t).abs() < 0.001 && (before.b - after.b).abs() < 0.001);
        assert!(
            effective
                .0
                .iter()
                .all(|&(x, y)| x >= 0. && y >= 0. && x <= 320. && y <= 180.),
            "{angle}: {effective:?}"
        );
    }
}

#[test]
fn cell_support_uses_labels_and_ink_not_word_recognition() {
    let column = ColumnBounds {
        right_slope: 0.,
        intercept: 25.,
        slope: 0.,
        right: 190.,
        top: 0.,
        bottom: 160.,
    };
    let rows = vec![10., 40., 60., 80., 100., 120., 140.];
    let marks: Vec<_> = rows[1..]
        .iter()
        .map(|&y| json!({"photo_bounds":[10.,y-4.,18.,y+4.]}))
        .collect();
    let guides = vec![(
        column,
        json!({"label_gutter_width":20.,"dot_reconciliation":{"measured_dot_edges":marks}}),
    )];
    let a = Anchor {
        source: 0,
        bounds: b(10., 76., 8., 8.),
        ordinal: Some(3),
        literal: "3".into(),
        confidence: 0.99,
        split: None,
    };
    let mut grid = Grid {
        columns: vec![column],
        row_centres: vec![rows],
        gutters: vec![(column, 20.)],
        ..Default::default()
    };
    grid.install_cells(&guides, &[a], WritingFrame::Local);
    let q = b(35., 132., 100., 16.).quad();
    let mut photo = RgbImage::from_pixel(220, 180, image::Rgb([240; 3]));
    assert!(grid.word_cell(&photo, &q, WritingFrame::Local).is_none());
    for y in 133..146 {
        for x in 40..130 {
            if x % 15 < 5 {
                photo.put_pixel(x, y, image::Rgb([30; 3]));
            }
        }
    }
    let cell = grid.word_cell(&photo, &q, WritingFrame::Local).unwrap();
    assert_eq!(cell.ordinal, Some(6));
    assert!(cell.ink_pixels > 0);
    assert_eq!(cell.row, Some(6));
    // Identical ink in a label gutter or unsupported header must stay out.
    for y in 2..150 {
        for x in [12, 13, 14, 40, 41, 42] {
            photo.put_pixel(x, y, image::Rgb([30; 3]));
        }
    }
    assert!(
        grid.word_cell(&photo, &b(10., 132., 8., 16.).quad(), WritingFrame::Local)
            .is_none()
    );
    assert!(
        grid.word_cell(&photo, &b(35., 2., 100., 16.).quad(), WritingFrame::Local)
            .is_none()
    );
    assert!(
        grid.word_cell(&photo, &b(195., 132., 20., 16.).quad(), WritingFrame::Local)
            .is_none()
    );
}

#[test]
fn local_prefix_survives_missing_track_without_inventing_a_grid() {
    for angle in [0., 17., 90., 180.] {
        let writing = WritingFrame::Page(test_direction(angle));
        let a = Anchor {
            source: 15,
            bounds: b(15., 20., 25., 15.),
            ordinal: Some(12),
            literal: "12".into(),
            confidence: 0.975,
            split: Some((55., b(55., 10., 95., 30.))),
        };
        let mut grid = Grid::default();
        grid.install_local_prefixes(&[a], &[], writing);
        assert!(grid.columns.is_empty() && grid.gutters.is_empty() && grid.envelopes.is_empty());
        let q = writing.unproject(&b(10., 10., 140., 30.).quad());
        let (cut, frame, bounded) = grid.constrain(&q, writing, 0.15);
        let actual = Bounds::of(&writing.project(&cut));
        assert!(bounded);
        assert!((actual.l - 55.).abs() < 0.001);
        assert!((actual.r - 150.).abs() < 0.001);
        assert!((actual.t - 10.).abs() < 0.001 && (actual.b - 40.).abs() < 0.001);
        // Right/vertical margins remain allowed, but the reader's leading
        // edge must never grow back into 12.
        let centre = writing.project(&Quad([(frame.cx, frame.cy); 4])).0[0].0;
        assert!((centre - frame.w / 2. - 55.).abs() < 0.001);
        assert_eq!(grid.label(&cut, writing).unwrap().ordinal, Some(12));
        // A neighbouring row does not inherit this local cut or label.
        let other = writing.unproject(&b(10., 70., 140., 30.).quad());
        assert_eq!(grid.constrain(&other, writing, 0.).0, other);
        assert!(grid.label(&other, writing).is_none());
        // The established column's tighter right boundary is still enforced.
        grid.columns.push(ColumnBounds {
            right_slope: 0.,
            intercept: 20.,
            slope: 0.,
            right: 140.,
            top: 0.,
            bottom: 100.,
        });
        let cut = grid.constrain(&q, writing, 0.15).0;
        let actual = Bounds::of(&writing.project(&cut));
        assert!((actual.l - 55.).abs() < 0.001 && (actual.r - 140.).abs() < 0.001);
    }
}

#[test]
fn local_prefix_does_not_promote_unsupported_dots_or_standalone_labels() {
    let mut standalone = anchor(12, 10.);
    let mut ambiguous = standalone.clone();
    ambiguous.ordinal = None;
    ambiguous.literal = "G.".into();
    ambiguous.split = Some((55., b(55., 10., 95., 30.)));
    let mut grid = Grid::default();
    grid.install_local_prefixes(
        &[standalone.clone(), ambiguous.clone()],
        &[],
        WritingFrame::Local,
    );
    assert!(grid.limits.is_empty() && grid.labels.is_empty());
    // Existing fitted-track behavior for the same evidence is preserved.
    standalone.source = 1;
    let column = Column {
        anchors: vec![0, 1],
        intercept: 40.,
        slope: 0.,
        height: 15.,
        rows: None,
    };
    grid.install_local_prefixes(&[standalone, ambiguous], &[column], WritingFrame::Local);
    assert_eq!(grid.limits.len(), 1);
    assert_eq!(grid.labels.len(), 1);
}

#[test]
fn sparse_column_shares_full_list_height_even_without_readable_ordinals() {
    let mut a: Vec<_> = [4, 5].into_iter().map(|n| anchor(n, 0.)).collect();
    for a in &mut a {
        a.ordinal = None;
    }
    a.extend((1..=6).map(|n| {
        let mut a = anchor(n, 200.);
        a.source += 10;
        a
    }));
    let words: Vec<_> = (1..=6)
        .flat_map(|n| {
            [
                b(20., 30. * n as f32, 80., 15.).quad(),
                b(220., 30. * n as f32, 80., 15.).quad(),
            ]
        })
        .collect();
    let guides = column_guides(&a, &words, WritingFrame::Local, 2);
    assert_eq!(guides.len(), 2);
    assert_eq!(guides[0].0.top, guides[1].0.top);
    assert_eq!(guides[0].0.bottom, guides[1].0.bottom);
    assert!(guides.iter().all(|(c, _)| c.top <= 30. && c.bottom >= 195.));
    assert!(guides.iter().any(|(c, _)| c.owns(b(20., 30., 80., 15.))));
}

#[test]
fn label_gutter_does_not_require_a_correct_individual_label_read() {
    for angle in [0., 17., 90., 180., 270.] {
        let writing = WritingFrame::Page(test_direction(angle));
        let c = ColumnBounds {
            right_slope: 0.,
            intercept: 50.,
            slope: 0.02,
            right: 200.,
            top: 20.,
            bottom: 250.,
        };
        let g = Grid {
            columns: vec![c],
            gutters: vec![(c, 30.)],
            ..Default::default()
        };
        let label = writing.unproject(&b(30., 80., 20., 20.).quad());
        assert_eq!(
            g.exclusion_read(&label, writing, Some("(o.")),
            Some("inside_label_gutter")
        );
        assert_eq!(
            g.exclusion_read(&label, writing, Some("fox")),
            Some("inside_label_gutter")
        );
        let word = writing.unproject(&b(60., 80., 100., 20.).quad());
        assert_eq!(g.exclusion_read(&word, writing, Some("churn")), None);
        let merged = writing.unproject(&b(30., 80., 130., 20.).quad());
        assert_eq!(g.exclusion_read(&merged, writing, Some("10.churn")), None);
        assert!(g.constrain(&merged, writing, 0.15).2);
        let wide_merged = writing.unproject(&b(0., 80., 90., 20.).quad());
        assert!(g.assigned_column(&wide_merged, writing).is_some());
        assert_eq!(
            g.exclusion_read(&wide_merged, writing, Some("5.riot")),
            None
        );
        assert!(g.constrain(&wide_merged, writing, 0.15).2);
    }
}

#[test]
fn oversized_label_rectangles_do_not_put_the_separator_through_separate_words() {
    let a: Vec<_> = (1..=3)
        .map(|n| {
            let mut a = anchor(n, 0.);
            a.bounds.r = 40.;
            a.bounds.b += 10.;
            a
        })
        .collect();
    let mut boxes: Vec<_> = a.iter().map(|a| a.bounds.quad()).collect();
    boxes.extend((1..=3).map(|n| b(35., 30. * n as f32, 60., 15.).quad()));
    let guides = column_guides(&a, &boxes, WritingFrame::Local, 2);
    assert_eq!(guides.len(), 1);
    assert!(guides[0].0.left(60.) <= 35.);
    assert!(guides[0].1["separator_corridor_shift"].as_f64().unwrap() > 0.);
}

#[test]
fn two_points_do_not_authorise_cuts_and_repeated_locations_do_not_vote() {
    assert!(fit(&[anchor(1, 0.), anchor(2, 0.)]).is_empty());
    let mut a = vec![anchor(1, 0.), anchor(2, 0.), anchor(3, 0.)];
    assert_eq!(fit(&a).len(), 1);
    a[2].source = a[1].source;
    assert!(fit(&a).is_empty());
    a[2] = anchor(2, 0.);
    a[2].source = 8;
    assert!(fit(&a).is_empty());
}

#[test]
fn independent_columns_missing_ordinals_and_outliers() {
    let mut a: Vec<_> = [1, 2, 5, 6].into_iter().map(|n| anchor(n, 0.)).collect();
    a.extend([8, 9, 10].into_iter().map(|n| anchor(n, 200.)));
    a.push(anchor(13, 800.));
    let columns = fit(&a);
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].anchors.len(), 4);
    assert!((columns[0].rows.unwrap().1 - 30.).abs() < 0.001);
}

#[test]
fn scarce_reads_prefer_a_new_track_over_more_votes_for_an_existing_one() {
    let anchors: Vec<_> = (1..=4).map(|n| anchor(n, 0.)).collect();
    let mut proposals: Vec<_> = (1..=3)
        .flat_map(|n| {
            [0., 200.].map(move |x| {
                let p = b(x, 30. * n as f32, 10., 10.);
                let f = Frame {
                    cx: 0.,
                    cy: 0.,
                    w: 0.,
                    h: 0.,
                    angle: 0.,
                };
                (
                    (n * 2 + if x == 0. { 0 } else { 1 }) as usize,
                    p.quad(),
                    f,
                    p,
                    5.,
                )
            })
        })
        .collect();
    schedule(&mut proposals, &anchors, WritingFrame::Local);
    assert!(proposals[..3].iter().all(|p| p.3.l == 200.));
}

#[test]
fn next_column_requires_a_gap_and_clamps_repairs_and_margin() {
    let writing = WritingFrame::Local;
    let mut photo = RgbImage::from_pixel(180, 150, Rgb([255; 3]));
    let mut anchors: Vec<_> = (1..=3).map(|n| anchor(n, 0.)).collect();
    anchors.extend((1..=3).map(|n| {
        let mut a = anchor(n, 110.);
        a.source += 10;
        a
    }));
    let columns = fit(&anchors);
    let q = b(25., 60., 95., 15.).quad();
    for y in 60..75 {
        for x in 25..100 {
            photo.put_pixel(x, y, Rgb([0; 3]));
        }
    }
    for y in 60..70 {
        for x in 110..120 {
            photo.put_pixel(x, y, Rgb([0; 3]));
        }
    }
    let limits = trailing_limits(&photo, &[q.clone()], &anchors, &columns, writing, 0.15);
    assert_eq!(limits.len(), 1);
    assert!(
        trailing_limits(
            &photo,
            &[b(5., 60., 115., 15.).quad()],
            &anchors,
            &columns,
            writing,
            0.15
        )
        .is_empty(),
        "a box still containing its own label must retain local prefix processing"
    );
    let right = limits[0].right;
    assert!(right >= 100. && right < 110.);
    let g = Grid {
        right_limits: limits,
        ..Default::default()
    };
    for candidate in [q, b(23., 58., 103., 19.).quad()] {
        let (effective, frame, changed) = g.constrain(&candidate, writing, 0.15);
        assert!(changed);
        assert!((Bounds::of(&effective).r - right).abs() < 0.001);
        assert!((frame.cx + frame.w / 2. - right).abs() < 0.001);
        assert_eq!(Bounds::of(&effective).h(), Bounds::of(&candidate).h());
    }
    // Connected letter/label ink supplies no safe separating gap.
    for y in 60..70 {
        for x in 100..110 {
            photo.put_pixel(x, y, Rgb([0; 3]));
        }
    }
    assert!(
        trailing_limits(
            &photo,
            &[b(25., 60., 95., 15.).quad()],
            &anchors,
            &columns,
            writing,
            0.15
        )
        .is_empty()
    );
}

#[test]
fn label_reads_are_not_vocabulary_guesses_or_invented_ordinals() {
    for text in ["un", "S", "Swarm", "1.huge", "hello.", "", "0"] {
        assert_eq!(
            numeric(&LineRead {
                text: text.into(),
                confidence: 0.99
            }),
            None,
            "{text}"
        );
    }
    for text in [".", "G.", "Hl."] {
        assert_eq!(
            numeric(&LineRead {
                text: text.into(),
                confidence: 0.99
            }),
            Some(None)
        );
    }
    assert_eq!(
        numeric(&LineRead {
            text: "16.".into(),
            confidence: 0.99
        }),
        Some(Some(16))
    );
    assert_eq!(
        numeric(&LineRead {
            text: "16.".into(),
            confidence: 0.89
        }),
        None
    );
}

#[test]
fn prefix_needs_a_real_gap_and_taller_following_ink() {
    let mut image = GrayImage::from_pixel(100, 40, Luma([255]));
    for y in 12..24 {
        for x in 5..15 {
            image.put_pixel(x, y, Luma([0]));
        }
    }
    for y in 5..35 {
        for x in 30..45 {
            image.put_pixel(x, y, Luma([0]));
        }
    }
    let (p, gap) = prefix(&image).unwrap();
    assert_eq!((p.l, p.r, gap), (5., 15., 15.));
    for y in 12..24 {
        for x in 15..30 {
            image.put_pixel(x, y, Luma([0]));
        }
    }
    assert!(prefix(&image).is_none());
}

#[test]
fn crop_limit_survives_margin_and_rotated_repair_without_changing_height() {
    for angle in [0., 17., 90., 180., 270.] {
        let writing = WritingFrame::Page(test_direction(angle));
        let mut g = Grid::default();
        g.limits.push(Limit {
            left: 20.,
            word: b(20., 30., 80., 20.),
            label: Some(super::super::LabelRead {
                corners: b(0., 30., 10., 10.).quad().0,
                literal: "1.".into(),
                origin: "grid_prefix".into(),
                ordinal: Some(1),
                dotted: true,
                ambiguous: false,
            }),
        });
        for original in [b(0., 30., 100., 20.), b(15., 28., 95., 24.)] {
            let q = writing.unproject(&original.quad());
            let (effective, frame, clipped) = g.constrain(&q, writing, 0.15);
            assert!(clipped);
            let bounded = Bounds::of(&writing.project(&effective));
            assert!((bounded.l - 20.).abs() < 0.01);
            assert!((bounded.t - original.t).abs() < 0.01);
            assert!((bounded.b - original.b).abs() < 0.01);
            assert!((frame.h - 1.3 * original.h()).abs() < 0.01);
            let p = from_crop(b(0., 0., frame.w, frame.h), frame, writing);
            assert!(p.l >= 19.4); // less than one pixel rounding of the raster canvas
        }
        let q = writing.unproject(&b(0., 55., 100., 20.).quad());
        assert!(!g.constrain(&q, writing, 0.15).2);
    }
}

#[test]
fn early_whole_read_is_reused_and_four_extra_reads_remain_shared() {
    let photo = RgbImage::from_pixel(300, 300, Rgb([255; 3]));
    let writing = WritingFrame::Page(test_direction(0.));
    let q = writing.unproject(&b(0., 0., 10., 20.).quad());
    let mut calls = 0;
    let mut budget = Budget {
        writing,
        ..Default::default()
    };
    let g = build(
        &photo,
        &[(0, q.clone())],
        &[q.clone()],
        writing,
        0.15,
        &mut budget,
        false,
        &Stage::new(None, 1),
        |_| {
            calls += 1;
            Ok(LineRead {
                text: "1.".into(),
                confidence: 0.99,
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(budget.ocrs, 0);
    let crop = level_crop_in(&photo, writing.crop_frame(&q, 0.15));
    assert_eq!(g.cached(&crop).unwrap().text, "1.");
    let mut changed = crop.clone();
    changed.put_pixel(0, 0, Luma([0]));
    assert!(g.cached(&changed).is_none());
}

#[test]
fn no_envelope_means_no_exclusion_and_boundary_straddlers_stay() {
    let writing = WritingFrame::Local;
    let mut g = Grid::default();
    assert!(
        g.exclusion(&b(10., 400., 100., 20.).quad(), writing)
            .is_none()
    );
    g.envelopes.push(Envelope {
        bounds: b(20., 30., 100., 200.),
        step: 30.,
        top_supported: true,
        bottom_supported: true,
        terminal_label: true,
    });
    assert_eq!(
        g.exclusion_read(
            &b(10., 400., 100., 20.).quad(),
            writing,
            Some("footer text")
        ),
        Some("outside_supported_list_rows")
    );
    assert!(
        g.exclusion(&b(10., 220., 100., 40.).quad(), writing)
            .is_none()
    );
    g.envelopes[0].bottom_supported = false;
    g.envelopes[0].terminal_label = false;
    assert!(
        g.exclusion(&b(10., 400., 100., 20.).quad(), writing)
            .is_none()
    );
}

fn merged_fixture() -> (RgbImage, Vec<(usize, Quad)>) {
    merged_fixture_rows(5)
}

fn merged_fixture_rows(rows: u32) -> (RgbImage, Vec<(usize, Quad)>) {
    let mut photo = RgbImage::from_pixel(150, 45 * rows + 45, Rgb([255; 3]));
    let mut qs = Vec::new();
    for n in 0..rows {
        let y = 20 + 45 * n;
        for yy in y + 10..y + 20 {
            for x in 13..23 {
                photo.put_pixel(x, yy, Rgb([0; 3]));
            }
        }
        for yy in y + 2..y + 28 {
            for x in 43..73 {
                photo.put_pixel(x, yy, Rgb([0; 3]));
            }
        }
        qs.push((n as usize, b(10., y as f32, 100., 30.).quad()));
    }
    (photo, qs)
}

#[test]
fn repeated_whitespace_establishes_twelve_cells_without_readable_numbers() {
    let (photo, qs) = merged_fixture_rows(12);
    let all: Vec<_> = qs.iter().map(|p| p.1.clone()).collect();
    let mut budget = Budget::default();
    // Use up all optional rescue reads: geometry is still mandatory.
    for _ in 0..4 {
        budget.crop(100).unwrap();
    }
    let g = build(
        &photo,
        &qs,
        &all,
        WritingFrame::Local,
        0.,
        &mut budget,
        true,
        &Stage::new(None, 12),
        |_| panic!("geometry cannot require an OCR vote"),
    )
    .unwrap();
    assert_eq!(g.columns.len(), 1);
    assert_eq!(g.row_centres[0].len(), 12);
    assert!(g.limits.is_empty()); // no invented literal readings
    for (_, q) in qs {
        assert!(g.constrain(&q, WritingFrame::Local, 0.).2);
    }
}

#[test]
fn sparse_prefix_calls_obey_shared_rescue_reservations_and_do_not_classify_labels() {
    let (photo, qs) = merged_fixture();
    for reserved in [0, 2, 4] {
        let mut budget = Budget::default();
        for _ in 0..reserved {
            budget.crop(100).unwrap();
        }
        let mut calls = 0;
        let g = build(
            &photo,
            &qs,
            &qs.iter().map(|p| p.1.clone()).collect::<Vec<_>>(),
            WritingFrame::Local,
            0.,
            &mut budget,
            true,
            &Stage::new(None, 5),
            |_| {
                calls += 1;
                Ok(LineRead {
                    text: format!("{calls}."),
                    confidence: 0.99,
                })
            },
        )
        .unwrap();
        assert_eq!(calls, 4 - reserved);
        assert_eq!(budget.reads, 4);
        assert_eq!(budget.classifiers, 0);
        // Two individually confirmed numeric prefixes keep their own limits
        // even when the remaining reservation prevents a three-anchor fit.
        assert_eq!(g.limits.len(), 4 - reserved);
        assert_eq!(
            g.limits.iter().filter(|l| l.label.is_none()).count(),
            0 // incomplete five-row layout cannot authorize inferred cuts
        );
    }
}

#[test]
fn incomplete_layout_keeps_local_prefixes_without_inferred_cuts() {
    let (photo, qs) = merged_fixture();
    let mut calls = 0;
    let g = build(
        &photo,
        &qs,
        &qs.iter().map(|p| p.1.clone()).collect::<Vec<_>>(),
        WritingFrame::Local,
        0.,
        &mut Budget::default(),
        true,
        &Stage::new(None, 5),
        |_| {
            calls += 1;
            Ok(LineRead {
                text: if calls == 4 {
                    "un".into()
                } else {
                    format!("{calls}.")
                },
                confidence: 0.99,
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert_eq!(g.limits.len(), 3); // independently read numeric prefixes only
    assert!(g.columns.is_empty());
    assert!(g.gutters.is_empty());
    assert!(g.envelopes.is_empty());
    assert!(!g.constrain(&qs[3].1, WritingFrame::Local, 0.).2);
    assert!(g.label(&qs[3].1, WritingFrame::Local).is_none());
    assert!(!g.constrain(&qs[4].1, WritingFrame::Local, 0.).2);
    assert!(g.label(&qs[4].1, WritingFrame::Local).is_none()); // no invented literal/ordinal
}

#[test]
fn every_assigned_box_and_repair_crop_is_inside_established_column_widths() {
    for angle in [0., 17., 90., 180., 270.] {
        let writing = WritingFrame::Page(test_direction(angle));
        let column = ColumnBounds {
            right_slope: 0.,
            intercept: 30.,
            slope: 0.1,
            right: 150.,
            top: 0.,
            bottom: 300.,
        };
        let g = Grid {
            columns: vec![column],
            ..Default::default()
        };
        // No per-box label, gap, or OCR evidence exists. Tall and expanded
        // repair rectangles are subject to exactly the same width constraint.
        for original in [
            b(10., 30., 170., 20.),
            b(20., 25., 160., 35.),
            b(25., 30., 130., 140.),
        ] {
            let (q, f, changed) = g.constrain(&writing.unproject(&original.quad()), writing, 0.15);
            assert!(changed);
            let out = Bounds::of(&writing.project(&q));
            assert!(out.l >= column.left_limit(out) - 0.001);
            assert!(out.r <= column.right + 0.001);
            assert!((out.h() - original.h()).abs() < 0.001);
            let corners = Quad(
                [
                    (-f.w / 2., -f.h / 2.),
                    (f.w / 2., -f.h / 2.),
                    (f.w / 2., f.h / 2.),
                    (-f.w / 2., f.h / 2.),
                ]
                .map(|(x, y)| {
                    let (x, y) = turn(x, y, f.angle);
                    (f.cx + x, f.cy + y)
                }),
            );
            let crop = Bounds::of(&writing.project(&corners));
            assert!(crop.l >= column.left_limit(crop) - 0.001);
            assert!(crop.r <= column.right + 0.001);
        }
    }
}

#[test]
fn rejected_prefixes_leave_all_word_boxes_and_pixels_unchanged() {
    let (photo, qs) = merged_fixture();
    let g = build(
        &photo,
        &qs,
        &qs.iter().map(|p| p.1.clone()).collect::<Vec<_>>(),
        WritingFrame::Local,
        0.,
        &mut Budget::default(),
        false,
        &Stage::new(None, 5),
        |_| {
            Ok(LineRead {
                text: "un".into(),
                confidence: 0.99,
            })
        },
    )
    .unwrap();
    assert!(g.limits.is_empty());
    assert!(g.envelopes.is_empty());
    for (_, q) in qs {
        let (actual, frame, changed) = g.constrain(&q, WritingFrame::Local, 0.15);
        assert_eq!(actual, q);
        assert!(!changed);
        assert_eq!(
            level_crop_in(&photo, frame),
            level_crop_in(&photo, WritingFrame::Local.crop_frame(&q, 0.15))
        );
    }
}

#[test]
fn merged_last_row_is_included_not_mistaken_for_outside_text() {
    let photo = RgbImage::from_pixel(200, 450, Rgb([255; 3]));
    let qs: Vec<_> = (1..=5)
        .map(|n| (n - 1, b(10., 30. * n as f32, 10., 10.).quad()))
        .collect();
    let mut all: Vec<_> = qs.iter().map(|p| p.1.clone()).collect();
    all.extend(
        (1..=12).map(|n| b(if n == 12 { 10. } else { 30. }, 30. * n as f32, 90., 15.).quad()),
    );
    let mut ordinal = 0;
    let g = build(
        &photo,
        &qs,
        &all,
        WritingFrame::Local,
        0.,
        &mut Budget::default(),
        true,
        &Stage::new(None, all.len()),
        |_| {
            ordinal += 1;
            Ok(LineRead {
                text: format!("{ordinal}."),
                confidence: 0.99,
            })
        },
    )
    .unwrap();
    assert_eq!(g.envelopes.len(), 1);
    let envelope = &g.envelopes[0];
    assert!(envelope.bounds.b >= 255.);
    assert!(!envelope.bottom_supported); // extrapolation is not a terminal-label observation
    assert!(
        g.exclusion(all.last().unwrap(), WritingFrame::Local)
            .is_none()
    );
}
