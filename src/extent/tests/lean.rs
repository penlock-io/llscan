use super::raw_witness_geometry as raw_geometry;
use super::*;

#[test]
fn lean_and_audit_match_floor_equality_and_rotated_pixel_geometry() {
    for angle in [0., 25., -25., 70.] {
        for (scale, count, expected) in [
            (20., 1, "unowned_ink"),
            (30., 2, "read"),
            (30., 3, "unowned_ink"),
            (40., 3, "read"),
            (40., 4, "unowned_ink"),
        ] {
            let results = [Audit::Components, Audit::None].map(|audit| {
                let (mut g, p, mut ink) = raw_geometry(angle);
                g.scale = scale;
                ink[8][40..40 + count].fill(true);
                let reason = continue_ink_for(Arm::E3, audit, &mut g, &ink, &[&p], &[], &[]);
                assert_eq!(reason, expected);
                assert_eq!(g.witness.is_some(), audit == Audit::Components);
                g.witness = None;
                g
            });
            assert_eq!(results[0], results[1]);
        }
    }
}

#[test]
fn lean_preserves_whole_component_count_coverage_and_refusal_precedence() {
    for expected in [
        "unowned_ink",
        "margin_outside_roi",
        "candidate_conflict",
        "writing_axis",
        "continuation_at_border",
        "no_new_ink",
    ] {
        let results = [Audit::Components, Audit::None].map(|audit| {
            let (mut g, p, mut ink) = raw_geometry(0.);
            g.scale = 40.;
            // Eight pixels total, only one in the footprint. Counting only
            // intersecting pixels would incorrectly let the lean path read.
            for row in &mut ink[..8] {
                row[40] = true;
            }
            let mut selected = Vec::new();
            match expected {
                "margin_outside_roi" | "continuation_at_border" => {
                    if expected == "margin_outside_roi" {
                        g.base = q(149.9, 167., 93.1, 26.);
                    }
                    let left = usize::from(expected != "continuation_at_border");
                    for row in &mut ink[18..22] {
                        row[left..26].fill(true);
                    }
                }
                "candidate_conflict" => selected.push(q(152., 174., 3., 8.)),
                "writing_axis" => g.base = q(195., 165., 10., 30.),
                "no_new_ink" => ink.iter_mut().for_each(|row| row.fill(false)),
                _ => (),
            }
            let reason = continue_ink_for(Arm::E3, audit, &mut g, &ink, &[&p], &[], &selected);
            assert_eq!(reason, expected);
            if audit == Audit::None {
                assert!(g.witness.is_none());
            }
            g.witness = None;
            g
        });
        assert_eq!(results[0], results[1], "{expected}");
    }
}

#[test]
fn page22_shaped_many_specks_and_seeded_neighbour_do_not_change_the_decision() {
    for foreign_large in [false, true] {
        let results = [Audit::Components, Audit::None].map(|audit| {
            let (mut g, p, mut ink) = raw_geometry(0.);
            g.scale = 40.; // Four-pixel whole-component floor.
            for y in [7, 9, 31] {
                for x in (30..90).step_by(3) {
                    ink[y][x] = true;
                }
            }
            // Connected to a parent stroke: large seeded neighbour ink is
            // retained, not newly classified as unowned by this storage change.
            for row in &mut ink[22..26] {
                row[5..9].fill(true);
            }
            if foreign_large {
                ink[8][3..7].fill(true);
            }
            let reason = continue_ink_for(Arm::E3, audit, &mut g, &ink, &[&p], &[], &[]);
            assert_eq!(reason, if foreign_large { "unowned_ink" } else { "read" });
            if audit == Audit::Components {
                let evidence = g.witness.as_ref().unwrap().json();
                let components = evidence["components"].as_array().unwrap();
                assert_eq!(
                    components
                        .iter()
                        .filter(|c| c["policy_role"] == "unseeded_ignored")
                        .count(),
                    60
                );
                assert!(
                    components
                        .iter()
                        .any(|c| c["policy_role"] == "seeded_and_large")
                );
            } else {
                assert!(g.witness.is_none());
            }
            g.witness = None;
            g
        });
        assert_eq!(results[0], results[1]);
    }
}

#[test]
fn lean_checks_the_union_of_nominal_and_rounded_pixels_not_their_bounding_boxes() {
    let (g, _, _) = raw_geometry(0.);
    let nominal = q(151.6, 167.495, 3.8, 1.49);
    let rounded = q(151.5, 167.74, 4., 1.);
    let mut seen = [false; 4];
    for y in 6..10 {
        for x in 0..7 {
            let mut labels = vec![0; (g.raster.w * g.raster.h) as usize];
            labels[(y * g.raster.w + x) as usize] = 1;
            let components = [Component {
                bounds: [x, y, x + 1, y + 1],
                pixels: 1,
                seeded: false,
                border: false,
                novel: false,
            }];
            let p = g.raster.point(x as f32 + 0.5, y as f32 + 0.5);
            let n = contains(&nominal, p);
            let r = contains(&rounded, p);
            seen[usize::from(n) + 2 * usize::from(r)] = true;
            let audit = ink_witness::Witness::collect(
                &g.raster,
                20.,
                &labels,
                &components,
                &nominal,
                &rounded,
                true,
            );
            let lean = ink_witness::has_unowned_ink(
                &g.raster,
                20.,
                &labels,
                &components,
                &nominal,
                &rounded,
            );
            assert_eq!(lean, n || r);
            assert_eq!(lean, audit.has_unowned_ink());
        }
    }
    assert_eq!(seen, [true; 4]); // Neither, nominal only, rounded only, both.
}

#[test]
fn lean_keeps_read_metadata_without_building_json_and_audit_export_stays_available() {
    let reports = [Audit::Components, Audit::None].map(|audit| {
        #[cfg(feature = "scan-profile")]
        crate::timing::take();
        let (mut photo, layout, mut scan, mut report) = simple();
        report.arm = Arm::E3;
        report.audit = audit;
        paint(&mut photo, 94, 108, 116, 112);
        run(&photo, &layout, &mut scan, &mut report);
        assert_eq!(target(&report).reason, "expanded");
        #[cfg(feature = "scan-profile")]
        assert!(
            !crate::timing::take()["spans"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "phrase.extent.export")
        );
        report
    });
    let [audit, lean] = reports;
    assert_eq!(lean.retained_component_witnesses(), 0);
    assert!(lean.retained_bytes() > std::mem::size_of::<Report>());
    assert!(audit.retained_bytes() > lean.retained_bytes());
    let mut audit_json = audit.to_json();
    assert!(
        audit_json["trials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["ink_witness"]["status"] == "collected")
    );
    for trial in audit_json["trials"].as_array_mut().unwrap() {
        trial.as_object_mut().unwrap().remove("ink_witness");
    }
    assert_eq!(audit_json, lean.to_json());
    let read = target(&lean).read.as_ref().unwrap();
    assert_eq!(
        read.json(),
        json!({"top":0,"probability":0.97f32,"margin":0.97f32 - 0.03f32,
        "pick":0,"kept":true,"verdict":"accept","turned":false,"raw":"abandon","literal_ocr_agrees":true})
    );
}
