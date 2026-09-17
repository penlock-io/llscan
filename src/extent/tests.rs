use super::*;
use crate::phrase::InitialOrder;
use crate::recogniser::LineRead;
use image::{GrayImage, Rgb};

mod e2;
mod e3;
mod lean;
mod progress;

// Existing policy/geometry tests exercise the unchanged no-observer path.
fn apply_with(
    photo: &RgbImage,
    layout: &PageLayout,
    scan: &mut PageScan,
    report: &mut Report,
    read: impl FnMut(&Quad, &mut Budget) -> Result<WordBox, String>,
) -> Result<(), String> {
    let mut progress = Stage::new(None, scan.words.len());
    super::apply_with(photo, layout, scan, report, &mut progress, read)
}

fn reindex(scan: &mut PageScan, report: &mut Report) {
    let mut progress = Stage::new(None, scan.words.len());
    super::reindex(scan, report, &mut progress);
}

fn read_reason(parents: &[&WordBox], candidate: &WordBox) -> &'static str {
    read_reason_for(Arm::E1, parents, candidate)
}

// Keep the original E1 test cases and their historical expectations intact.
fn continue_ink(
    g: &mut Geometry,
    ink: &[Vec<bool>],
    parents: &[&Quad],
    others: &[&Quad],
    selected: &[Quad],
) -> &'static str {
    continue_ink_for(
        Arm::E1,
        Audit::Components,
        g,
        ink,
        parents,
        others,
        selected,
    )
}

fn q(x: f32, y: f32, w: f32, h: f32) -> Quad {
    Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
}

fn word(quad: Quad, rank: usize) -> WordBox {
    WordBox {
        quad,
        crop: GrayImage::new(1, 1),
        ranked: vec![(0, 0.97), (1, 0.03)],
        selection: Some(
            crate::hybrid::select(Some("abandon"), None, &[(0, 0.97), (1, 0.03)], 0.85, 0.75)
                .unwrap(),
        ),
        column_rank: rank,
        row_rank: rank,
        turned: false,
        raw: Some(LineRead {
            text: "abandon".into(),
            confidence: 0.9,
        }),
        evidence: Default::default(),
        narrowed: false,
        stray: false,
        number: None,
        label: None,
        apart: false,
        joined_from: None,
        expanded_from: Vec::new(),
    }
}

fn scene(quads: Vec<Quad>) -> (PageLayout, PageScan, Report) {
    scene_in(quads, WritingFrame::Local)
}

fn scene_in(quads: Vec<Quad>, writing: WritingFrame) -> (PageLayout, PageScan, Report) {
    let layout = PageLayout::with_writing(&quads, writing);
    let order = layout.column_order();
    let rows = layout.row_order();
    let mut row_rank = vec![0; quads.len()];
    for (r, &i) in rows.iter().enumerate() {
        row_rank[i] = r;
    }
    let sources: Vec<_> = order
        .iter()
        .enumerate()
        .map(|(i, &old)| Source {
            detector: old,
            part: Some(0),
            quad: quads[old].clone(),
            column_rank: i,
            row_rank: row_rank[old],
            word_index: i,
            labelled: false,
        })
        .collect();
    let scan = PageScan {
        page_direction: None,
        width: 400,
        height: 300,
        sources: Vec::new(),
        words: sources
            .iter()
            .map(|s| {
                let mut w = word(s.quad.clone(), s.column_rank);
                w.row_rank = s.row_rank;
                w
            })
            .collect(),
        numbering: Numbering::None,
        join_trials: Vec::new(),
    };
    (
        layout.reindexed(&order),
        scan,
        Report {
            sources,
            audit: Audit::Components,
            ..Default::default()
        },
    )
}

#[test]
fn shared_page_direction_reaches_extent_roi_continuation_and_crop_budget() {
    for angle in [0., 17., 90., 180., 270.] {
        let d = crate::page_frame::test_direction(angle);
        let writing = WritingFrame::Page(d);
        let quads: Vec<_> = [-60., 0., 60.]
            .into_iter()
            .map(|y| d.quad_to_photo(&q(-40., y, 80., 20.)))
            .collect();
        let (layout, scan, report) = scene_in(quads, writing);
        let trials = seeds(&layout, &report.sources, &scan);
        let target = trials.iter().find(|t| t.row == 1).unwrap();
        let mut g = geometry_seed(target, &layout, &report.sources, &scan).unwrap();
        assert_eq!(g.raster.writing, writing);
        assert!((g.raster.frame.angle - d.angle_degrees()).abs() < 0.002);
        assert_eq!((g.raster.w, g.raster.h), (100, 40));
        let mut ink = vec![vec![false; 100]; 40];
        for row in &mut ink[18..22] {
            row[4..26].fill(true);
        }
        let parents = [&report.sources[1].quad];
        assert_eq!(
            continue_ink_for(Arm::E3, Audit::None, &mut g, &ink, &parents, &[], &[]),
            "read"
        );
        let candidate = g.candidate.as_ref().unwrap();
        let f = writing.crop_frame(candidate, CROP_MARGIN);
        assert!((f.angle - d.angle_degrees()).abs() < 0.002);
        assert!((writing.frame_of(g.footprint.as_ref().unwrap()).w - f.w.round()).abs() < 0.002);
        let mut budget = Budget {
            writing,
            ..Default::default()
        };
        budget.read(candidate).unwrap();
        assert_eq!(budget.read_pixels, pixels_ceil(f).unwrap());
        assert_eq!((budget.classifiers, budget.ocrs), (0, 0)); // geometry/preflight never runs models
    }
}

fn simple() -> (RgbImage, PageLayout, PageScan, Report) {
    let (layout, scan, report) = scene(vec![
        q(100., 50., 80., 20.),
        q(100., 100., 80., 20.),
        q(100., 150., 80., 20.),
    ]);
    (
        RgbImage::from_pixel(400, 300, Rgb([255; 3])),
        layout,
        scan,
        report,
    )
}

fn paint(photo: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32) {
    for y in y0..y1 {
        for x in x0..x1 {
            photo.put_pixel(x, y, Rgb([0; 3]));
        }
    }
}

fn run(photo: &RgbImage, layout: &PageLayout, scan: &mut PageScan, report: &mut Report) {
    apply_with(photo, layout, scan, report, |quad, b| {
        b.classifier()?;
        b.ocr()?;
        Ok(word(quad.clone(), 0))
    })
    .unwrap();
}

fn target(report: &Report) -> &Trial {
    report.trials.iter().find(|t| t.sources == [1]).unwrap()
}

#[test]
fn crossing_component_grows_from_photo_pixels_and_preserves_parent() {
    let (mut photo, layout, mut scan, mut report) = simple();
    paint(&mut photo, 94, 108, 116, 112);
    let original = scan.words[1].clone();
    run(&photo, &layout, &mut scan, &mut report);
    let t = target(&report);
    assert_eq!(t.reason, "expanded");
    let grown = &scan.words[t.selected.unwrap()];
    assert_eq!(grown.quad.0[0], (94., 100.));
    for p in original.quad.0 {
        assert!(contains(&grown.quad, p));
    }
    let parent = &scan.words[t.parents[0]];
    assert_eq!(parent.quad.0, original.quad.0);
    assert_eq!(parent.crop, original.crop);
    assert!(parent.stray);
    assert_eq!(
        (
            report.budget.reads,
            report.budget.classifiers,
            report.budget.ocrs
        ),
        (1, 1, 1)
    );
    assert_eq!(
        report.to_json()["trials"][1]["read"]["literal_ocr_agrees"],
        true
    );
}

#[test]
fn no_crossing_or_only_existing_margin_ink_never_reads() {
    for left in [100, 98] {
        let (mut photo, layout, mut scan, mut report) = simple();
        paint(&mut photo, left, 108, 116, 112);
        apply_with(&photo, &layout, &mut scan, &mut report, |_, _| {
            panic!("no new ink must not read")
        })
        .unwrap();
        assert_eq!(target(&report).reason, "no_new_ink");
        assert_eq!(report.budget.reads, 0);
        assert_eq!(report.budget.rois, 3);
    }
}

#[test]
fn disconnected_missing_letter_is_unknown_not_a_gap_closing_retry() {
    let (mut photo, layout, mut scan, mut report) = simple();
    // The detached piece is outside the parent and does not seed itself.
    paint(&mut photo, 94, 108, 97, 112);
    paint(&mut photo, 101, 108, 116, 112);
    run(&photo, &layout, &mut scan, &mut report);
    assert_eq!(target(&report).reason, "no_new_ink");
    assert_eq!(report.budget.reads, 0);
    // If another stroke permits growth, the detached piece inside the new
    // footprint is explicitly unowned, not silently added as a letter.
    let (mut photo, layout, mut scan, mut report) = simple();
    paint(&mut photo, 94, 108, 116, 110);
    paint(&mut photo, 95, 114, 97, 116);
    run(&photo, &layout, &mut scan, &mut report);
    assert_eq!(target(&report).reason, "unowned_ink");
    assert_eq!(report.budget.reads, 0);
}

#[test]
fn touching_leading_ink_cannot_be_cut_or_declared_a_number() {
    let (mut photo, layout, mut scan, mut report) = simple();
    paint(&mut photo, 94, 108, 116, 112); // connected leading mark: role unknown
    run(&photo, &layout, &mut scan, &mut report);
    let grown = &scan.words[target(&report).selected.unwrap()];
    assert!(contains(&grown.quad, (94.5, 108.5)));
    assert!(!grown.narrowed);
    assert!(grown.label.is_none() && grown.number.is_none());
    assert!(matches!(scan.numbering, Numbering::None));
    // Existing evidence of a label makes the entire column ineligible,
    // even though numbering is not held and model confidence is high.
    let (_, layout, mut scan, mut report) = simple();
    report.sources[1].labelled = true;
    run(&photo, &layout, &mut scan, &mut report);
    assert!(report.trials.iter().all(|t| t.reason == "column_label"));
    assert_eq!(report.budget.reads, 0);
}

#[test]
fn separated_label_outside_exact_box_does_not_block_the_read() {
    let (mut photo, _, _, _) = simple();
    paint(&mut photo, 94, 108, 116, 112);
    let (layout, mut scan, mut report) = scene(vec![
        q(100., 50., 80., 20.),
        q(100., 100., 80., 20.),
        q(100., 150., 80., 20.),
        q(91., 102., 2., 6.),
    ]);
    let label = report.sources.iter().position(|s| s.detector == 3).unwrap();
    report.sources[label].labelled = true;
    // This label is outside the exact grown box. The removed source halo
    // must no longer make it a collision.
    run(&photo, &layout, &mut scan, &mut report);
    assert_eq!(report.budget.reads, 1);
}

#[test]
fn spaced_leading_letters_of_stable_and_wave_are_preserved() {
    for spelling in ["stable", "wave"] {
        let (mut photo, layout, mut scan, mut report) = simple();
        paint(&mut photo, 94, 108, 108, 112);
        for x in [112, 124, 138, 155, 170] {
            paint(&mut photo, x, 104, x + 3, 116);
        }
        let pick = crate::vocabulary::Word::from_bip39(spelling)
            .unwrap()
            .index() as usize;
        for w in &mut scan.words {
            w.ranked = vec![(pick, 0.97), (1, 0.03)];
            w.select_fixture(pick, true);
        }
        apply_with(&photo, &layout, &mut scan, &mut report, |quad, b| {
            b.classifier()?;
            b.ocr()?;
            let mut w = word(quad.clone(), 0);
            w.ranked = vec![(pick, 0.95), (1, 0.05)];
            w.select_fixture(pick, true);
            Ok(w)
        })
        .unwrap();
        let grown = &scan.words[target(&report).selected.unwrap()];
        assert_eq!(grown.pick(), Some(pick));
        for x in [94.5, 112.5, 124.5, 138.5, 155.5, 170.5] {
            assert!(contains(&grown.quad, (x, 108.5)));
        }
    }
}

#[test]
fn faint_or_broken_support_is_not_repaired_by_threshold_or_morphology_search() {
    let (mut photo, layout, scan, report) = simple();
    let trial = seeds(&layout, &report.sources, &scan).remove(1);
    let mut g = geometry_seed(&trial, &layout, &report.sources, &scan).unwrap();
    paint(&mut photo, 100, 108, 116, 112);
    let mut ink = binarise(&level_crop(&photo, &g.raster.quad));
    // Binary evidence with an explicit break: the geometry must not turn it
    // into a connected stroke, regardless of the grayscale explanation.
    for row in &mut ink[18..22] {
        row[4..7].fill(true);
    }
    assert_eq!(
        continue_ink(&mut g, &ink, &[&report.sources[1].quad], &[], &[]),
        "no_new_ink"
    );
    let mut blank = geometry_seed(&trial, &layout, &report.sources, &scan).unwrap();
    let empty = vec![vec![false; blank.raster.w as usize]; blank.raster.h as usize];
    assert_eq!(
        continue_ink(&mut blank, &empty, &[&report.sources[1].quad], &[], &[]),
        "no_new_ink"
    );
}

fn raw_geometry(angle: f32) -> (Geometry, Quad, Vec<Vec<bool>>) {
    let frame = Frame {
        cx: 200.,
        cy: 180.,
        w: 80.,
        h: 20.,
        angle,
    };
    let parent = Rect([-40., -10., 40., 10.]).quad(frame);
    let raster = Raster::new(Rect([-50., -20., 50., 20.]).quad(frame)).unwrap();
    let mut ink = vec![vec![false; raster.w as usize]; raster.h as usize];
    for row in &mut ink[18..22] {
        row[4..26].fill(true);
    }
    (
        Geometry {
            raster,
            supports: vec![],
            scale: 20.,
            base: parent.clone(),
            prior_footprints: vec![with_margin(&parent, CROP_MARGIN)],
            components: Vec::new(),
            candidate: None,
            footprint: None,
            witness: None,
        },
        parent,
        ink,
    )
}

// Ownership-witness tests need foreign ink inside a proposed extent but
// outside its seed parent. Make that extent explicit: it is not reader padding.
fn raw_witness_geometry(angle: f32) -> (Geometry, Quad, Vec<Vec<bool>>) {
    let (mut g, parent, ink) = raw_geometry(angle);
    g.base = Rect([-49., -13., 43., 13.]).quad(frame_of(&parent));
    (g, parent, ink)
}

#[test]
fn two_genuine_words_and_prior_candidates_conflict_before_reading() {
    let (mut g, p, ink) = raw_geometry(0.);
    let neighbour = q(152., 174., 3., 8.);
    assert_eq!(
        continue_ink(&mut g, &ink, &[&p], &[&neighbour], &[]),
        "other_observation"
    );
    let (mut g, p, ink) = raw_geometry(0.);
    assert_eq!(
        continue_ink(&mut g, &ink, &[&p], &[], &[neighbour]),
        "candidate_conflict"
    );
}

#[test]
fn tilted_neighbour_guard_uses_photo_polygons_and_exact_raster_inverse() {
    for angle in [25., -25., 70.] {
        let (mut g, p, ink) = raw_geometry(angle);
        let f = frame_of(&p);
        let neighbour = Rect([-48., -6., -45., 6.]).quad(f);
        assert!(!intersects(&neighbour, &p));
        assert_eq!(
            continue_ink(&mut g, &ink, &[&p], &[&neighbour], &[]),
            "other_observation"
        );
        for (x, y) in [(0.5, 0.5), (13.5, 20.5)] {
            let photo = g.raster.point(x, y);
            let (lx, ly) = local(g.raster.frame, photo);
            assert!((lx + g.raster.w as f32 / 2. - x).abs() < 1e-4);
            assert!((ly + g.raster.h as f32 / 2. - y).abs() < 1e-4);
        }
    }
}

#[test]
fn missing_observations_never_create_slots_or_seed_ink() {
    let (layout, mut scan, mut report) = scene(vec![]);
    let photo = RgbImage::from_pixel(400, 300, Rgb([0; 3]));
    apply_with(&photo, &layout, &mut scan, &mut report, |_, _| {
        panic!("no seed")
    })
    .unwrap();
    assert!(scan.words.is_empty() && report.trials.is_empty());
    assert_eq!(report.budget.rois, 0);
}

#[test]
fn photograph_edges_and_unresolved_roi_border_refuse() {
    let (photo, _, _, _) = simple();
    let (layout, mut scan, mut report) = scene(vec![
        q(3., 50., 80., 20.),
        q(3., 100., 80., 20.),
        q(3., 150., 80., 20.),
    ]);
    run(&photo, &layout, &mut scan, &mut report);
    assert!(report.trials.iter().all(|t| t.reason == "photo_edge"));
    assert_eq!(report.budget.roi_pixels, 0);
    let (mut g, p, mut ink) = raw_geometry(0.);
    ink[19][0..26].fill(true);
    assert_eq!(
        continue_ink(&mut g, &ink, &[&p], &[], &[]),
        "continuation_at_border"
    );
}

#[test]
fn row_first_initial_policy_and_original_crops_survive_switching() {
    let phrase = crate::vocabulary::Word::parse_phrase(
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let order: Vec<_> = (0..12).step_by(2).chain((1..12).step_by(2)).collect();
    let words: Vec<_> = order
        .iter()
        .enumerate()
        .map(|(column, &row)| {
            let mut w = word(q(column as f32 * 20., 100., 16., 8.), column);
            w.row_rank = row;
            w.ranked = vec![
                (phrase[row].index() as usize, 0.97),
                ((phrase[row].index() as usize + 1) % 2048, 0.03),
            ];
            w.select_fixture(phrase[row].index() as usize, true);
            w
        })
        .collect();
    let mut scan = PageScan {
        page_direction: None,
        width: 400,
        height: 300,
        sources: Vec::new(),
        words,
        numbering: Numbering::None,
        join_trials: vec![],
    };
    assert_eq!(scan.initial_traversal().order, InitialOrder::Rows);
    let before = scan.clone();
    let mut replacement = scan.words[1].clone();
    replacement.quad = q(19., 99., 18., 10.);
    scan.words[1].stray = true;
    scan.words.push(replacement);
    let mut report = Report {
        trials: vec![Trial {
            column: 0,
            row: 2,
            sources: vec![1],
            parents: vec![1],
            reason: "expanded",
            geometry: None,
            read: None,
            selected: Some(12),
            original_strays: vec![false],
            active: true,
        }],
        ..Default::default()
    };
    reindex(&mut scan, &mut report);
    let kept = |s: &PageScan, rows: bool| {
        let order = if rows {
            s.by_rows()
        } else {
            (0..s.words.len()).collect()
        };
        order
            .into_iter()
            .filter(|&i| !s.words[i].stray)
            .map(|i| s.words[i].pick().unwrap())
            .collect::<Vec<_>>()
    };
    for active in [true, false, true, false] {
        report.choose_expansion(&mut scan, 0, active).unwrap();
        assert_eq!(scan.initial_traversal().order, InitialOrder::Rows);
        assert_eq!(kept(&scan, false), kept(&before, false));
        assert_eq!(kept(&scan, true), kept(&before, true));
        let original = &scan.words[report.trials[0].parents[0]];
        assert_eq!(original.crop, before.words[1].crop);
        assert_eq!(original.quad.0, before.words[1].quad.0);
    }
}

#[test]
fn mixed_region_proposals_and_selected_native_joins_stay_protected() {
    let (mut g, p, ink) = raw_geometry(0.);
    // The geometry doesn't filter this original polygon because it was apart.
    let apart = q(151., 176., 4., 4.);
    assert_eq!(
        continue_ink(&mut g, &ink, &[&p], &[&apart], &[]),
        "other_observation"
    );
    let (_, layout, mut scan, report) = simple();
    let mut joined = word(q(100., 50., 80., 70.), 0);
    joined.joined_from = Some([0, 1]);
    scan.words.push(joined);
    scan.words[0].stray = true;
    scan.words[1].stray = true;
    let trials = seeds(&layout, &report.sources, &scan);
    assert_eq!(trials[0].reason, "protected");
    assert_eq!(trials[1].reason, "protected");
}

#[test]
fn earlier_fragment_ownership_or_union_prevents_a_second_expansion() {
    use crate::fragments::{Decision, Observation, Ownership, Scale};
    use crate::sources::{Outcome, Part, RawSource};
    for lower_ink in [true, false] {
        let (mut photo, layout, mut scan, mut report) = simple();
        paint(&mut photo, 94, 108, 116, 112);
        scan.sources = report
            .sources
            .iter()
            .map(|s| RawSource {
                detector: s.detector,
                quad: s.quad.clone(),
                outcome: Outcome::Parts(vec![Part {
                    part: 0,
                    quad: s.quad.clone(),
                    word_index: Some(s.word_index),
                }]),
                decision: None,
                union: None,
            })
            .collect();
        if lower_ink {
            scan.sources.push(RawSource {
                detector: 3,
                quad: q(150., 118., 5., 5.),
                outcome: Outcome::Empty(crate::split::EmptySplit::NoInkAfterRules),
                decision: Some(Decision::Tiny {
                    scale: Scale {
                        height: 20.,
                        supports: vec![0, 1, 2],
                    },
                    ownership: Ownership::Descender(Observation {
                        source: 1,
                        part: Some(0),
                    }),
                }),
                union: None,
            });
        } else {
            let mut inactive = word(q(100., 100., 90., 20.), 1);
            inactive.stray = true;
            inactive.expanded_from = vec![crate::phrase::ExtentParent {
                word_index: 1,
                stray: false,
            }];
            scan.words.push(inactive);
        }
        let original = scan.words[1].clone();
        apply_with(&photo, &layout, &mut scan, &mut report, |_, _| {
            panic!("protected parent reread")
        })
        .unwrap();
        assert_eq!(target(&report).reason, "protected");
        assert_eq!(report.budget.reads, 0);
        assert_eq!(scan.words[1].quad, original.quad);
        assert_eq!(scan.words[1].crop, original.crop);
    }
}

#[test]
fn every_budget_refuses_before_the_next_allocation_or_call() {
    let mut b = Budget::default();
    for _ in 0..MAX_ROIS {
        b.inspect().unwrap();
    }
    assert_eq!(b.inspect(), Err("roi_count_budget"));
    assert_eq!(b.rois, MAX_ROIS);
    assert_eq!(b.roi(MAX_ROI_PIXELS + 1), Err("roi_pixel_budget"));
    assert_eq!(b.roi_pixels, 0);
    for _ in 0..8 {
        b.roi(MAX_ROI_PIXELS).unwrap();
    }
    assert_eq!(b.roi(1), Err("roi_pixel_budget"));
    let oversized = q(0., 0., 2000., 1000.);
    assert_eq!(b.read(&oversized), Err("read_budget"));
    assert_eq!(b.reads, 0);
    for _ in 0..4 {
        b.read(&q(0., 0., 80., 20.)).unwrap();
        b.classifier().unwrap();
        b.classifier().unwrap();
        b.ocr().unwrap();
    }
    assert_eq!(b.read(&q(0., 0., 80., 20.)), Err("read_budget"));
    assert!(b.classifier().is_err() && b.ocr().is_err());
    assert_eq!((b.reads, b.classifiers, b.ocrs), (4, 8, 4));
    let mut pixel_limited = Budget {
        read_pixels: MAX_READ_PIXELS - 1,
        ..Default::default()
    };
    assert_eq!(pixel_limited.read(&q(0., 0., 80., 20.)), Err("read_budget"));
    assert_eq!(pixel_limited.reads, 0);
    // Optional preflight must include the half-turn classifier and OCR caps,
    // not start a crop read only to fail halfway through the model calls.
    let mut call_limited = Budget {
        classifiers: 7,
        ..Default::default()
    };
    assert_eq!(call_limited.read(&q(0., 0., 20., 80.)), Err("read_budget"));
    assert_eq!(call_limited.reads, 0);
    call_limited.read(&q(0., 0., 80., 20.)).unwrap();
    let mut ocr_limited = Budget {
        ocrs: 4,
        ..Default::default()
    };
    assert_eq!(ocr_limited.read(&q(0., 0., 80., 20.)), Err("read_budget"));
    assert_eq!(ocr_limited.reads, 0);
    assert!(
        pixels_ceil(Frame {
            cx: 0.,
            cy: 0.,
            w: f32::INFINITY,
            h: 2.,
            angle: 0.
        })
        .is_err()
    );
}

#[test]
fn literal_ocr_agreement_is_diagnostic_not_a_selection_gate() {
    let parent = word(q(0., 0., 80., 20.), 0);
    let mut candidate = parent.clone();
    candidate.ranked[0].1 = 0.95; // same accepted identity need not get surer
    candidate.select_fixture(0, true);
    assert_eq!(read_reason(&[&parent], &candidate), "expanded");
    assert_eq!(read_json(&candidate)["literal_ocr_agrees"], true);
    candidate.raw.as_mut().unwrap().text = "abamdon".into();
    assert_eq!(read_reason(&[&parent], &candidate), "expanded");
    assert_eq!(read_json(&candidate)["literal_ocr_agrees"], false);
    candidate.ranked = vec![(1, 0.95), (0, 0.03)];
    candidate.select_fixture(1, true);
    assert_eq!(read_reason(&[&parent], &candidate), "accepted_identity");
    candidate.ranked = vec![(0, 0.95), (1, 0.03)];
    candidate.select_fixture(0, true);
    let mut uncertain = parent.clone();
    uncertain.select_fixture(0, false);
    assert_eq!(read_reason(&[&uncertain], &candidate), "parent_surer");
    candidate.ranked[0].1 = 0.99;
    candidate.select_fixture(0, true);
    assert_eq!(read_reason(&[&uncertain], &candidate), "expanded");
    candidate.stray = true;
    assert_eq!(read_reason(&[&uncertain], &candidate), "stray");
    candidate.stray = false;
    candidate.select_fixture(0, false);
    assert_eq!(read_reason(&[&uncertain], &candidate), "uncertain");
}

#[test]
fn pair_seed_reuses_native_trial_without_individual_parent_retries() {
    let (layout, mut scan, report) = scene(vec![
        q(100., 50., 100., 20.),
        q(100., 100., 35., 20.),
        q(140., 100., 60., 20.),
        q(100., 150., 100., 20.),
    ]);
    for p in [1, 2] {
        scan.words[p].select_fixture(0, false);
    }
    let before = seeds(&layout, &report.sources, &scan);
    let pair = before.iter().find(|t| t.sources.len() == 2).unwrap();
    assert_eq!(pair.reason, "pair_unread");
    scan.join_trials.push(crate::joins::JoinTrial {
        column: pair.column,
        row: pair.row,
        parents: [1, 2],
        reason: "union_uncertain",
        read: Some(crate::joins::JoinRead {
            top: 0,
            probability: 0.5,
            margin: 0.1,
            pick: Some(0),
            kept: true,
        }),
    });
    let after = seeds(&layout, &report.sources, &scan);
    let pair = after.iter().find(|t| t.sources.len() == 2).unwrap();
    assert_eq!(pair.reason, "candidate");
    assert_eq!(after.len(), 3);
    let g = geometry_seed(pair, &layout, &report.sources, &scan).unwrap();
    assert_eq!(g.prior_footprints.len(), 3);
    // Even a hypothetical grown box cannot earn a read by reusing pixels
    // already present in the failed native union's full footprint.
    let original = crate::joins::union(&scan.words[1].quad, &scan.words[2].quad);
    assert!(
        g.prior_footprints[2]
            .0
            .iter()
            .all(|&p| outside_distance(&with_margin(&original, CROP_MARGIN), p) < 1e-4)
    );
}

#[test]
fn multiple_double_rows_cannot_be_hidden_by_rejected_reads() {
    let (layout, mut scan, report) = scene(vec![
        q(100., 50., 100., 20.),
        q(100., 100., 35., 20.),
        q(140., 100., 60., 20.),
        q(100., 150., 35., 20.),
        q(140., 150., 60., 20.),
    ]);
    for w in &mut scan.words {
        w.stray = true;
    }
    assert!(
        seeds(&layout, &report.sources, &scan)
            .iter()
            .all(|t| t.reason == "row_structure")
    );
}

#[test]
fn support_axis_limit_and_even_median_are_frozen() {
    assert!(same_axis(89., -89.));
    assert!(same_axis(0., 10.));
    assert!(!same_axis(0., 10.01));
    let (layout, scan, report) = scene(vec![
        q(100., 50., 80., 16.),
        q(100., 100., 80., 20.),
        q(100., 150., 80., 24.),
    ]);
    let trials = seeds(&layout, &report.sources, &scan);
    let g = geometry_seed(&trials[1], &layout, &report.sources, &scan).unwrap();
    assert_eq!(g.scale, 20.);
    assert_eq!(g.supports, vec![0, 2]);
}

#[test]
fn rounded_box_outside_inspected_roi_refuses_instead_of_sampling_new_ink() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    // The nominal extent fits; rounding 89.6 to 90 moves its left edge from
    // 150.1 to 149.9, outside the inspected ROI. No source halo is involved.
    g.base = q(150.1, 170., 89.6, 20.);
    for row in &mut ink[18..22] {
        row[1..26].fill(true);
    }
    assert_eq!(
        continue_ink(&mut g, &ink, &[&p], &[], &[]),
        "margin_outside_roi"
    );
}

#[test]
fn full_loop_enforces_read_and_roi_caps_without_invoking_the_reader_again() {
    let quads: Vec<_> = (0..35)
        .map(|i| q(100., 50. + i as f32 * 50., 80., 20.))
        .collect();
    let (layout, mut scan, mut report) = scene(quads);
    let mut photo = RgbImage::from_pixel(400, 1800, Rgb([255; 3]));
    for i in 0..35 {
        paint(&mut photo, 94, 58 + i * 50, 116, 62 + i * 50);
    }
    let mut calls = 0;
    apply_with(&photo, &layout, &mut scan, &mut report, |quad, b| {
        calls += 1;
        assert!(calls <= MAX_READS, "reader invoked after the preflight cap");
        b.classifier()?;
        b.ocr()?;
        Ok(word(quad.clone(), 0))
    })
    .unwrap();
    assert_eq!(calls, 4);
    assert_eq!((report.budget.reads, report.budget.rois), (4, 32));
    assert_eq!(
        report
            .trials
            .iter()
            .filter(|t| t.reason == "read_budget")
            .count(),
        28
    );
    assert_eq!(
        report
            .trials
            .iter()
            .filter(|t| t.reason == "roi_count_budget")
            .count(),
        3
    );
}

#[test]
fn inserting_an_expansion_reindexes_existing_native_join_links_and_both_choices() {
    use crate::sources::{Outcome, Part, RawSource};
    let mut words: Vec<_> = (0..5)
        .map(|i| word(q(100., 50. + i as f32 * 40., 80., 20.), i))
        .collect();
    words[1].stray = true;
    words[3].stray = true;
    words[2].joined_from = Some([1, 3]);
    let mut scan = PageScan {
        page_direction: None,
        width: 400,
        height: 300,
        sources: Vec::new(),
        words,
        numbering: Numbering::None,
        join_trials: vec![crate::joins::JoinTrial {
            column: 0,
            row: 1,
            parents: [1, 3],
            reason: "joined",
            read: None,
        }],
    };
    let replacement = scan.words[0].clone();
    scan.words[0].stray = true;
    scan.words.push(replacement);
    let mut report = Report {
        sources: [0, 1, 3, 4]
            .into_iter()
            .enumerate()
            .map(|(rank, index)| Source {
                detector: rank,
                part: Some(0),
                quad: scan.words[index].quad.clone(),
                column_rank: rank,
                row_rank: rank,
                word_index: index,
                labelled: false,
            })
            .collect(),
        trials: vec![Trial {
            column: 0,
            row: 0,
            sources: vec![0],
            parents: vec![0],
            reason: "expanded",
            geometry: None,
            read: None,
            selected: Some(5),
            original_strays: vec![false],
            active: true,
        }],
        ..Default::default()
    };
    scan.sources = report
        .sources
        .iter()
        .map(|s| RawSource {
            decision: None,
            union: None,
            detector: s.detector,
            quad: s.quad.clone(),
            outcome: Outcome::Parts(vec![Part {
                part: s.part.unwrap(),
                quad: s.quad.clone(),
                word_index: Some(s.word_index),
            }]),
        })
        .collect();
    let empty = RawSource {
        decision: None,
        union: None,
        detector: 4,
        quad: q(200., 100., 20., 10.),
        outcome: Outcome::Empty(crate::split::EmptySplit::FlatPhotoSamples),
    };
    scan.sources.push(empty.clone());
    reindex(&mut scan, &mut report);
    assert_eq!(scan.sources.last(), Some(&empty));
    for (raw, source) in scan.sources.iter().zip(&report.sources) {
        let part = &raw.parts()[0];
        assert_eq!(
            (raw.detector, Some(part.part)),
            (source.detector, source.part)
        );
        assert_eq!(part.word_index, Some(source.word_index));
        assert_eq!(part.quad, source.quad);
        assert_eq!(scan.words[part.word_index.unwrap()].quad, part.quad);
    }
    assert_eq!(
        report
            .sources
            .iter()
            .map(|s| s.word_index)
            .collect::<Vec<_>>(),
        [0, 2, 4, 5]
    );
    assert_eq!(scan.words[3].joined_from, Some([2, 4]));
    assert_eq!(scan.join_trials[0].parents, [2, 4]);
    assert_eq!(report.expanded_from(1), Some([0].as_slice()));
    assert!(
        scan.words[1].joined_from.is_none(),
        "growth is not a two-piece join"
    );
    for active in [false, true] {
        report.choose_expansion(&mut scan, 0, active).unwrap();
        assert_eq!(scan.words[0].stray, active);
        assert_eq!(scan.words[1].stray, !active);
        assert!(scan.words[2].stray && scan.words[4].stray);
        assert!(!scan.words[3].stray);
    }
}

#[test]
fn a_tall_parent_enclosure_cannot_silently_change_the_writing_axis() {
    let (layout, scan, report) = scene(vec![
        q(100., 40., 80., 20.),
        q(100., 100., 25., 20.),
        q(100., 140., 25., 20.),
        q(100., 210., 80., 20.),
    ]);
    let trial = Trial {
        column: 0,
        row: 1,
        sources: vec![1, 2],
        parents: vec![1, 2],
        reason: "candidate",
        geometry: None,
        read: None,
        selected: None,
        original_strays: vec![],
        active: false,
    };
    assert_eq!(
        geometry_seed(&trial, &layout, &report.sources, &scan).unwrap_err(),
        "writing_axis"
    );
}
