use super::raw_witness_geometry as raw_geometry;
use super::*;

fn geometry(
    g: &mut Geometry,
    ink: &[Vec<bool>],
    parent: &Quad,
    others: &[&Quad],
    selected: &[Quad],
) -> &'static str {
    continue_ink_for(
        Arm::E2,
        Audit::Components,
        g,
        ink,
        &[parent],
        others,
        selected,
    )
}

fn evidence(g: &Geometry) -> Value {
    g.witness.as_ref().unwrap().json()
}

fn unseeded(g: &Geometry) -> Vec<Value> {
    evidence(g)["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["seeded"] == false)
        .cloned()
        .collect()
}

#[test]
fn empty_polygon_overlap_is_not_ink_but_an_unseeded_fragment_is() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    let neighbour = q(152., 174., 3., 8.);
    assert_eq!(geometry(&mut g, &ink, &p, &[&neighbour], &[]), "read");
    assert!(unseeded(&g).is_empty());
    // Ink is a veto even without a detector observation. At h=20, A=1.
    ink[8][30..34].fill(true);
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "unowned_ink");
    assert_eq!(unseeded(&g)[0]["total_pixels"], 4);
    assert_eq!(unseeded(&g)[0]["policy_role"], "unseeded_veto_capable");
}

#[test]
fn size_floor_counts_pixels_at_multiple_scales_including_equality() {
    for (scale, count, expected) in [
        (20., 1, "unowned_ink"),
        (30., 2, "read"),
        (30., 3, "unowned_ink"),
        (40., 3, "read"),
        (40., 4, "unowned_ink"),
        (40., 5, "unowned_ink"),
        (60., 8, "read"),
        (60., 9, "unowned_ink"),
        (60., 10, "unowned_ink"),
    ] {
        let (mut g, p, mut ink) = raw_geometry(0.);
        // Isolate the rule using a supplied support scale; the existing
        // geometry builder's support median is tested separately below.
        g.scale = scale;
        ink[8][40..40 + count].fill(true);
        assert_eq!(
            geometry(&mut g, &ink, &p, &[], &[]),
            expected,
            "h={scale}, n={count}"
        );
        let c = &unseeded(&g)[0];
        assert_eq!(c["total_pixels"], count);
        assert_eq!(c["union_pixels"], count);
        assert_eq!(c["at_or_above_floor"], expected == "unowned_ink");
    }
    assert_eq!(ink_witness::area_floor(30.), 2.25);
    assert_eq!(ink_witness::area_floor(40.), 4.);
}

#[test]
fn bounding_area_does_not_stand_in_for_foreground_pixel_count() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    g.scale = 40.;
    // An 8-connected diagonal has three pixels, not its 3x3 bounding area.
    for (x, y) in [(60, 6), (61, 7), (62, 8)] {
        ink[y][x] = true;
    }
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "read");
    let c = &unseeded(&g)[0];
    assert_eq!(c["roi_bounds"], json!([60, 6, 63, 9]));
    assert_eq!(c["total_pixels"], 3);
    assert_eq!(c["union_pixels"], 2);
    assert_eq!(c["policy_role"], "unseeded_ignored");
}

#[test]
fn one_intersecting_pixel_of_a_large_component_still_vetoes() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    g.scale = 40.;
    for row in &mut ink[0..8] {
        row[40] = true;
    }
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "unowned_ink");
    let c = &unseeded(&g)[0];
    assert_eq!(c["total_pixels"], 8);
    assert_eq!(c["union_pixels"], 1);
    assert_eq!(c["touches_roi_border"], true);
    assert_eq!(c["roi_bounds"], json!([40, 0, 41, 8]));
    assert_eq!(c["intersection_roi_bounds"], json!([40, 7, 41, 8]));
    assert_eq!(c["photo_bounds"], json!([190.5, 160.5, 190.5, 167.5]));
    assert_eq!(c["representative_photo_pixel"], json!([190.5, 167.5]));
}

#[test]
fn separate_small_marks_are_not_merged_and_all_witnesses_are_exported() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    g.scale = 40.;
    for x in [30, 34, 38, 42, 46, 50] {
        ink[8][x] = true;
    }
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "read");
    assert_eq!(unseeded(&g).len(), 6);
    assert!(
        unseeded(&g)
            .iter()
            .all(|c| c["policy_role"] == "unseeded_ignored")
    );
    // A later significant component must not suppress earlier or later specks
    // from the log, and refusal must not stop at the first component.
    ink[8][55..59].fill(true);
    ink[8][63..67].fill(true);
    ink[8][71] = true;
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "unowned_ink");
    assert_eq!(unseeded(&g).len(), 9);
    assert_eq!(
        unseeded(&g)
            .iter()
            .filter(|c| c["at_or_above_floor"] == true)
            .count(),
        2
    );
}

#[test]
fn touching_foreign_ink_is_visible_as_seeded_and_large_not_claimed_safe() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    // A hypothetical neighbour touches the existing parent stroke. It now
    // shares a seed: E2 cannot identify the semantic error; the audit must.
    for row in &mut ink[22..26] {
        row[5..9].fill(true);
    }
    let neighbour = q(154., 182., 6., 4.);
    assert_eq!(geometry(&mut g, &ink, &p, &[&neighbour], &[]), "read");
    let e = evidence(&g);
    let c = &e["components"][0];
    assert_eq!(e["components"].as_array().unwrap().len(), 1);
    assert_eq!(c["policy_role"], "seeded_and_large");
    assert_eq!(c["total_pixels"], 104);
    assert_eq!(c["roi_bounds"], json!([4, 18, 26, 26]));
    assert_eq!(c["intersection_roi_bounds"], c["roi_bounds"]);
}

#[test]
fn footprint_union_counts_each_pixel_once_including_rounding_slivers() {
    let (g, _, _) = raw_geometry(0.);
    let mut labels = vec![0; (g.raster.w * g.raster.h) as usize];
    for y in 7..9 {
        for x in 1..6 {
            labels[y * g.raster.w as usize + x] = 1;
        }
    }
    let components = [Component {
        bounds: [1, 7, 6, 9],
        pixels: 10,
        seeded: false,
        border: false,
        novel: false,
    }];
    // Same centre, dimensions 3.8x1.49 rounded to 4x1. One shrinks, one grows.
    let nominal = q(151.6, 167.495, 3.8, 1.49);
    let rounded = q(151.5, 167.74, 4., 1.);
    let w = ink_witness::Witness::collect(
        &g.raster,
        40.,
        &labels,
        &components,
        &nominal,
        &rounded,
        true,
    );
    let c = &w.json()["components"][0];
    assert_eq!(c["nominal_pixels"], 6);
    assert_eq!(c["rounded_pixels"], 5);
    assert_eq!(c["union_pixels"], 8); // three pixels are in both
    assert_eq!(c["representative_photo_pixel"], json!([152.5, 167.5]));
    assert!(w.has_unowned_ink());
}

#[test]
fn rotated_witness_bounds_and_representative_use_the_original_pixel_transform() {
    for angle in [25., -25., 70.] {
        let (mut g, p, mut ink) = raw_geometry(angle);
        g.scale = 40.;
        ink[8][30..34].fill(true);
        let expected: Vec<_> = (30..34)
            .map(|x| g.raster.point(x as f32 + 0.5, 8.5))
            .collect();
        assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "unowned_ink");
        let c = &unseeded(&g)[0];
        assert_eq!(c["total_pixels"], 4);
        assert_eq!(c["union_pixels"], 4);
        assert_eq!(c["representative_photo_pixel"], json!(expected[0]));
        let mut bounds = Rect([expected[0].0, expected[0].1, expected[0].0, expected[0].1]);
        for point in expected {
            bounds.include(point);
        }
        assert_eq!(c["photo_bounds"], json!(bounds.0));
        assert_eq!(c["intersection_photo_bounds"], c["photo_bounds"]);
        assert_eq!(evidence(&g)["coverage"], "complete");
    }
}

#[test]
fn earlier_margin_and_conflict_guards_preserve_precedence_and_complete_witness_lists() {
    let (mut g, p, mut ink) = raw_geometry(0.);
    g.base = q(149.9, 167., 93.1, 26.);
    for row in &mut ink[18..22] {
        row[1..26].fill(true);
    }
    ink[8][30..34].fill(true);
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "margin_outside_roi");
    assert_eq!(evidence(&g)["coverage"], "partial");
    assert_eq!(unseeded(&g).len(), 1);
    assert!(g.witness.as_ref().unwrap().has_unowned_ink());
    let (mut g, p, mut ink) = raw_geometry(0.);
    ink[8][30..34].fill(true);
    assert_eq!(
        geometry(&mut g, &ink, &p, &[], &[q(152., 174., 3., 8.)]),
        "candidate_conflict"
    );
    assert_eq!(evidence(&g)["coverage"], "complete");
    assert_eq!(unseeded(&g).len(), 1);
    // An axis-changing constructed box also gets a witness, without erasing
    // the earlier writing-axis refusal in favour of an ownership verdict.
    let (mut g, p, mut ink) = raw_geometry(0.);
    g.base = q(195., 165., 10., 30.);
    ink[8][30..34].fill(true);
    assert_eq!(geometry(&mut g, &ink, &p, &[], &[]), "writing_axis");
    assert!(g.witness.is_some());
}

#[test]
fn no_candidate_is_not_exported_as_an_empty_successful_ink_check() {
    for border in [false, true] {
        let (mut photo, layout, mut scan, mut report) = simple();
        report.arm = Arm::E2;
        if border {
            paint(&mut photo, 90, 108, 116, 112);
        }
        run(&photo, &layout, &mut scan, &mut report);
        let t = target(&report);
        assert_eq!(
            t.reason,
            if border {
                "continuation_at_border"
            } else {
                "no_new_ink"
            }
        );
        let log = report.to_json();
        let w = &log["trials"][1]["ink_witness"];
        assert_eq!(w["status"], "unavailable");
        assert_eq!(w["reason"], t.reason);
        assert!(w["coverage"].is_null() && w["components"].is_null());
        assert_eq!(report.budget.reads, 0);
    }
    let (photo, layout, mut scan, mut report) = simple();
    report.arm = Arm::E2;
    report.sources[1].labelled = true;
    run(&photo, &layout, &mut scan, &mut report);
    assert!(
        report.to_json()["trials"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |t| t["reason"] == "column_label" && t["ink_witness"]["status"] == "not_applicable"
            )
    );
    assert_eq!(report.budget.rois, 0);
}

#[test]
fn ignored_real_small_mark_stays_in_reader_pixels_and_original_alternatives_restore() {
    let (layout, mut scan, mut report) = scene(vec![
        q(100., 50., 80., 40.),
        q(100., 110., 80., 20.),
        q(100., 180., 80., 40.),
    ]);
    report.arm = Arm::E2;
    let mut photo = RgbImage::from_pixel(400, 300, Rgb([255; 3]));
    paint(&mut photo, 94, 118, 116, 122);
    paint(&mut photo, 94, 108, 96, 122); // connected ink explicitly grows the box upward
    paint(&mut photo, 130, 109, 131, 110); // a real small mark inside that box
    let before = photo.clone();
    let original = scan.words[1].clone();
    apply_with(&photo, &layout, &mut scan, &mut report, |quad, budget| {
        budget.classifier()?;
        budget.ocr()?;
        let mut read = word(quad.clone(), 0);
        read.crop = level_crop(&photo, &with_margin(quad, CROP_MARGIN));
        assert_eq!(read.crop.get_pixel(36, 1).0, [0]);
        Ok(read)
    })
    .unwrap();
    assert_eq!(photo, before);
    let t = target(&report);
    assert_eq!(t.reason, "expanded");
    assert_eq!(t.geometry.as_ref().unwrap().scale, 40.);
    let log = report.to_json();
    assert_eq!(log["arm"], "E2");
    assert!(
        log["trials"][1]["ink_witness"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["policy_role"] == "unseeded_ignored" && c["total_pixels"] == 1)
    );
    assert_eq!(
        (
            report.budget.reads,
            report.budget.classifiers,
            report.budget.ocrs
        ),
        (1, 1, 1)
    );
    let (selected, parent) = (t.selected.unwrap(), t.parents[0]);
    for active in [false, true, false] {
        report.choose_expansion(&mut scan, 1, active).unwrap();
        assert_eq!(scan.words[parent].stray, active);
        assert_eq!(scan.words[selected].stray, !active);
        assert_eq!(scan.words[parent].crop, original.crop);
        assert_eq!(scan.words[parent].quad.0, original.quad.0);
    }
}

#[test]
fn historical_e1_diagnostics_are_unchanged_and_e2_retains_witness_on_read_refusal() {
    for arm in [Arm::E1, Arm::E2] {
        let (mut photo, layout, mut scan, mut report) = simple();
        report.arm = arm;
        paint(&mut photo, 94, 108, 116, 112);
        apply_with(&photo, &layout, &mut scan, &mut report, |quad, budget| {
            budget.classifier()?;
            budget.ocr()?;
            let mut w = word(quad.clone(), 0);
            w.ranked = vec![(1, 0.97), (0, 0.03)];
            w.select_fixture(1, true);
            Ok(w)
        })
        .unwrap();
        assert_eq!(target(&report).reason, "accepted_identity");
        let log = report.to_json();
        assert_eq!(log["arm"], arm.label());
        assert_eq!(
            log["trials"][1].get("ink_witness").is_some(),
            arm == Arm::E2
        );
        if arm == Arm::E2 {
            assert_eq!(log["trials"][1]["ink_witness"]["status"], "collected");
        }
        assert_eq!(scan.words.len(), 3);
        assert!(!scan.words[1].stray);
    }
}

#[test]
fn e2_preserves_roi_read_call_limits_and_retains_witnesses_at_read_budget() {
    let (layout, mut scan, mut report) = scene(
        (0..35)
            .map(|i| q(100., 50. + i as f32 * 50., 80., 20.))
            .collect(),
    );
    report.arm = Arm::E2;
    let mut photo = RgbImage::from_pixel(400, 1800, Rgb([255; 3]));
    for i in 0..35 {
        paint(&mut photo, 94, 58 + i * 50, 116, 62 + i * 50);
    }
    apply_with(&photo, &layout, &mut scan, &mut report, |quad, budget| {
        assert!(budget.reads <= 4);
        budget.classifier()?;
        budget.classifier()?; // optional half-turn is charged too
        budget.ocr()?;
        Ok(word(quad.clone(), 0))
    })
    .unwrap();
    assert_eq!(
        (
            report.budget.rois,
            report.budget.reads,
            report.budget.classifiers,
            report.budget.ocrs
        ),
        (32, 4, 8, 4)
    );
    assert_eq!(
        report
            .trials
            .iter()
            .filter(|t| t.reason == "read_budget")
            .count(),
        28
    );
    for trial in report.to_json()["trials"].as_array().unwrap() {
        if trial["reason"] == "roi_count_budget" {
            assert_eq!(trial["ink_witness"]["status"], "not_applicable");
        } else {
            assert_eq!(trial["ink_witness"]["coverage"], "complete");
        }
    }
}

#[test]
fn e2_protects_native_join_parents_and_refuses_uninspected_photo_edges() {
    let (mut photo, layout, mut scan, mut report) = simple();
    report.arm = Arm::E2;
    paint(&mut photo, 94, 108, 116, 112);
    let mut joined = scan.words[0].clone();
    joined.joined_from = Some([0, 1]);
    scan.words.push(joined);
    scan.words[0].stray = true;
    scan.words[1].stray = true;
    apply_with(&photo, &layout, &mut scan, &mut report, |_, _| {
        panic!("protected parents cannot be read")
    })
    .unwrap();
    assert_eq!(report.trials[0].reason, "protected");
    assert_eq!(report.trials[1].reason, "protected");
    assert!(scan.words[0].stray && scan.words[1].stray);
    assert_eq!(scan.words[3].joined_from, Some([0, 1]));
    assert_eq!(report.budget.reads, 0);

    let (layout, mut scan, mut report) = scene(vec![
        q(3., 50., 80., 20.),
        q(3., 100., 80., 20.),
        q(3., 150., 80., 20.),
    ]);
    report.arm = Arm::E2;
    run(&photo, &layout, &mut scan, &mut report);
    assert_eq!(report.budget.roi_pixels, 0);
    for t in report.to_json()["trials"].as_array().unwrap() {
        assert_eq!(t["reason"], "photo_edge");
        assert_eq!(t["ink_witness"]["status"], "unavailable");
        assert!(t["ink_witness"]["components"].is_null());
    }
}

#[test]
#[cfg(feature = "scan-profile")]
fn e2_export_is_timed_separately_and_does_not_change_e1_profiling() {
    crate::timing::take();
    let e1 = Report::new(Arm::E1, Audit::Components).to_json();
    assert_eq!(e1["arm"], "E1");
    assert!(
        crate::timing::take()["spans"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let e2 = Report::new(Arm::E2, Audit::Components).to_json();
    assert_eq!(e2["arm"], "E2");
    let timing = crate::timing::take();
    let spans = timing["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0]["name"], "phrase.extent.export");
    assert_eq!(spans[0]["calls"], 1);
}
