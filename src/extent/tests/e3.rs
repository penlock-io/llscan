use super::*;

fn uncertain(probability: f32) -> WordBox {
    let mut w = word(q(100., 100., 80., 20.), 0);
    w.ranked = vec![(0, probability), (1, (1. - probability).min(probability))];
    w.select_fixture(0, false);
    w
}

#[test]
fn selection_does_not_confirm_an_uncertain_candidate_or_require_literal_ocr() {
    let a = uncertain(0.548022);
    let b = uncertain(0.177223);
    let mut candidate = uncertain(0.702330);
    candidate.raw.as_mut().unwrap().text = "different raw reading".into();
    let before = read_json(&candidate);
    for arm in [Arm::E1, Arm::E2] {
        assert_eq!(read_reason_for(arm, &[&a, &b], &candidate), "uncertain");
    }
    assert_eq!(read_reason_for(Arm::E3, &[&a, &b], &candidate), "expanded");
    assert_eq!(read_json(&candidate), before);
    assert_eq!(candidate.verdict(), Verdict::Uncertain);
    candidate.select_fixture(0, true);
    assert_eq!(read_reason_for(Arm::E3, &[&a, &b], &candidate), "expanded");
}

#[test]
fn keep_top_and_every_parent_confidence_remain_required_with_equality_allowed() {
    let a = uncertain(0.5);
    let b = uncertain(0.7);
    let base = uncertain(0.7);
    assert_eq!(read_reason_for(Arm::E3, &[&a, &b], &base), "expanded");
    let mut candidate = base.clone();
    candidate.stray = true;
    assert_eq!(read_reason_for(Arm::E3, &[&a, &b], &candidate), "stray");
    candidate = base.clone();
    candidate.ranked.clear();
    candidate.selection = None;
    assert_eq!(read_reason_for(Arm::E3, &[&a, &b], &candidate), "uncertain");
    candidate = base.clone();
    candidate.select_fixture(1, false);
    assert_eq!(
        read_reason_for(Arm::E3, &[&a, &b], &candidate),
        "pick_disagrees"
    );
    candidate = base.clone();
    candidate.ranked[0].1 = 0.699;
    candidate.select_fixture(0, false);
    for parents in [[&a, &b], [&b, &a]] {
        assert_eq!(
            read_reason_for(Arm::E3, &parents, &candidate),
            "parent_surer"
        );
    }
    let mut missing = b.clone();
    missing.ranked.clear();
    missing.selection = None;
    assert_eq!(
        read_reason_for(Arm::E3, &[&a, &missing], &base),
        "parent_surer"
    );
}

#[test]
fn accepted_singletons_keep_identity_but_no_longer_skip_confidence_comparison() {
    let parent = word(q(100., 100., 80., 20.), 0);
    let mut candidate = parent.clone();
    candidate.ranked[0].1 = 0.95;
    candidate.select_fixture(0, true);
    for arm in [Arm::E1, Arm::E2] {
        assert_eq!(read_reason_for(arm, &[&parent], &candidate), "expanded");
    }
    assert_eq!(
        read_reason_for(Arm::E3, &[&parent], &candidate),
        "parent_surer"
    );
    candidate.ranked[0].1 = parent.ranked[0].1;
    candidate.select_fixture(0, false);
    assert_eq!(read_reason_for(Arm::E3, &[&parent], &candidate), "expanded");
    assert_eq!(candidate.verdict(), Verdict::Uncertain); // Never inherit the parent's tick.
    candidate.ranked = vec![(1, parent.ranked[0].1), (0, 0.03)];
    candidate.select_fixture(1, false);
    assert_eq!(
        read_reason_for(Arm::E3, &[&parent], &candidate),
        "accepted_identity"
    );
    let mut missing = parent.clone();
    missing.ranked.clear();
    missing.selection = None;
    candidate.ranked = vec![(0, parent.ranked[0].1), (1, 0.03)];
    candidate.select_fixture(0, false);
    assert_eq!(
        read_reason_for(Arm::E3, &[&missing], &candidate),
        "parent_surer"
    );
}

#[test]
fn e3_geometry_and_full_ink_witnesses_are_identical_to_e2() {
    for (scale, foreign_pixels) in [(20., 0), (40., 1), (20., 4)] {
        let observed = [Arm::E2, Arm::E3].map(|arm| {
            let (mut g, p, mut ink) = raw_geometry(0.);
            g.scale = scale;
            ink[8][30..30 + foreign_pixels].fill(true);
            let reason = continue_ink_for(arm, Audit::Components, &mut g, &ink, &[&p], &[], &[]);
            (reason, g.json(), g.witness.as_ref().unwrap().json())
        });
        assert_eq!(observed[0], observed[1]);
    }
}

#[test]
fn inactive_arms_read_the_same_crop_but_only_e3_selects_its_uncertain_result() {
    let outcomes = [Arm::E2, Arm::E3].map(|arm| {
        let (mut photo, layout, mut scan, mut report) = simple();
        report.arm = arm;
        scan.words[1].ranked[0].1 = 0.6;
        scan.words[1].select_fixture(0, false);
        paint(&mut photo, 94, 108, 116, 112);
        apply_with(&photo, &layout, &mut scan, &mut report, |quad, budget| {
            budget.classifier()?;
            budget.ocr()?;
            let mut candidate = uncertain(0.7);
            candidate.quad = quad.clone();
            Ok(candidate)
        })
        .unwrap();
        let t = target(&report);
        assert_eq!(
            t.reason,
            if arm == Arm::E3 {
                "expanded"
            } else {
                "uncertain"
            }
        );
        if let Some(selected) = t.selected {
            assert_eq!(scan.words[selected].verdict(), Verdict::Uncertain);
            assert!(scan.words[t.parents[0]].stray);
        }
        assert_eq!(
            (
                report.budget.reads,
                report.budget.classifiers,
                report.budget.ocrs
            ),
            (1, 1, 1)
        );
        // Selecting the middle expansion protects that source: the final row
        // then lacks two usable supports and is refused before rasterization.
        // This predates the selection owner; equal reads do not imply equal
        // page-wide geometry work between active E3 and inactive E2.
        let later = report.trials.iter().find(|t| t.sources == [2]).unwrap();
        if arm == Arm::E3 {
            assert_eq!(later.reason, "supports");
            assert!(later.geometry.is_none());
        } else {
            assert!(later.geometry.is_some());
        }
        assert_eq!(
            report.budget.json(),
            json!({
                "inspected_rois": 3, "roi_pixels": if arm == Arm::E3 { 8000 } else { 12000 },
                "read_attempts": 1, "read_pixels": 1720, "classifier_calls": 1, "ocr_calls": 1,
            })
        );
        (t.geometry.as_ref().unwrap().json(), t.read.clone())
    });
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn e3_pair_restoration_keeps_both_traversals_and_original_strays_losslessly() {
    let [(audit_scan, mut audit), (lean_scan, lean)] =
        [Audit::Components, Audit::None].map(pair_restoration);
    assert_eq!(format!("{audit_scan:?}"), format!("{lean_scan:?}"));
    assert!(audit.retained_component_witnesses() > 0);
    assert_eq!(lean.retained_component_witnesses(), 0);
    assert!(audit.retained_bytes() > lean.retained_bytes());
    for trial in &mut audit.trials {
        if let Some(g) = &mut trial.geometry {
            g.witness = None;
        }
    }
    audit.audit = Audit::None;
    assert_eq!(audit, lean); // Includes read, geometry, budgets, both mappings and restoration flags.
}

fn pair_restoration(audit: Audit) -> (PageScan, Report) {
    let (layout, mut scan, mut report) = scene(vec![
        q(100., 50., 100., 20.),
        q(100., 100., 35., 20.),
        q(140., 100., 60., 20.),
        q(100., 150., 100., 20.),
    ]);
    report.arm = Arm::E3;
    report.audit = audit;
    for p in [1, 2] {
        scan.words[p].ranked[0].1 = 0.5;
        scan.words[p].select_fixture(0, false);
    }
    scan.words[2].stray = true;
    scan.join_trials.push(crate::joins::JoinTrial {
        column: 0,
        row: 1,
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
    for accepted in [1, 2] {
        scan.words[accepted].select_fixture(0, true);
        let trials = seeds(&layout, &report.sources, &scan);
        assert_eq!(
            trials.iter().find(|t| t.sources.len() == 2).unwrap().reason,
            "parent_accepted"
        );
        scan.words[accepted].select_fixture(0, false);
    }
    let before: Vec<_> = scan
        .words
        .iter()
        .map(|w| (w.quad.0, w.stray, read_json(w)))
        .collect();
    let columns: Vec<_> = scan
        .kept_columns()
        .iter()
        .map(|&i| scan.words[i].quad.0)
        .collect();
    let rows: Vec<_> = scan
        .kept_rows()
        .iter()
        .map(|&i| scan.words[i].quad.0)
        .collect();
    let mut photo = RgbImage::from_pixel(400, 300, Rgb([255; 3]));
    paint(&mut photo, 94, 108, 116, 112);
    apply_with(&photo, &layout, &mut scan, &mut report, |quad, budget| {
        budget.classifier()?;
        budget.ocr()?;
        let mut candidate = uncertain(0.7);
        candidate.quad = quad.clone();
        Ok(candidate)
    })
    .unwrap();
    let trial = report
        .trials
        .iter()
        .position(|t| t.sources.len() == 2)
        .unwrap();
    let t = &report.trials[trial];
    assert_eq!(t.reason, "expanded");
    let selected = t.selected.unwrap();
    let parents = t.parents.clone();
    assert_eq!(report.expanded_from(selected), Some(parents.as_slice()));
    assert_eq!(scan.join_trials[0].parents.as_slice(), parents.as_slice());
    assert!(scan.words[selected].joined_from.is_none()); // Expansion is not a native join.
    for active in [false, true, false] {
        report.choose_expansion(&mut scan, trial, active).unwrap();
        assert_eq!(scan.words[selected].stray, !active);
        assert_eq!(scan.words[selected].verdict(), Verdict::Uncertain);
        for s in &report.sources {
            let current = &scan.words[s.word_index];
            let old = &before[s.detector];
            assert_eq!(current.quad.0, old.0);
            if !active {
                assert_eq!(read_json(current), old.2);
            }
        }
        if !active {
            assert_eq!(
                scan.kept_columns()
                    .iter()
                    .map(|&i| scan.words[i].quad.0)
                    .collect::<Vec<_>>(),
                columns
            );
            assert_eq!(
                scan.kept_rows()
                    .iter()
                    .map(|&i| scan.words[i].quad.0)
                    .collect::<Vec<_>>(),
                rows
            );
        } else {
            assert!(parents.iter().all(|&i| scan.words[i].stray));
        }
    }
    (scan, report)
}

#[test]
#[cfg(feature = "scan-profile")]
fn e3_diagnostics_identify_the_arm_and_charge_export_separately() {
    crate::timing::take();
    let output = Report::new(Arm::E3, Audit::Components).to_json();
    assert_eq!(output["arm"], "E3");
    let timing = crate::timing::take();
    let spans = timing["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0]["name"], "phrase.extent.export");
}
