//! Constructed pixels/fake readers through the shared geometry and crop owners.
use super::*;
use crate::page_frame::{Direction, test_direction};
use image::Rgb;

fn q(d: Direction, x: f32, y: f32, w: f32, h: f32) -> Quad {
    d.quad_to_photo(&Quad([(x, y), (x + w, y), (x + w, y + h), (x, y + h)]))
}

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.002, "{a} != {b}");
}

#[test]
fn shared_direction_reaches_tall_crops_cut_geometry_and_read_preflight() {
    let photo = RgbImage::from_fn(512, 512, |x, y| Rgb([((x + 2 * y) % 251) as u8; 3]));
    let original = photo.clone();
    for angle in [0., 17., 90., 180., 270.] {
        let d = test_direction(angle);
        let writing = WritingFrame::Page(d);
        let quad = q(d, -20., -30., 10., 30.);
        let mut results = Vec::new();
        for trace in [false, true] {
            let mut budget = crate::read_budget::Budget {
                writing,
                ..Default::default()
            };
            budget.read_margin(&quad, CROP_MARGIN, true).unwrap();
            let (mut classifiers, mut ocrs) = (0, 0);
            let mut p = prepare_in(
                &photo,
                &quad,
                |crop| {
                    classifiers += 1;
                    assert_eq!(crop.dimensions(), (10, 30));
                    Ok(vec![(0, 0.9), (1, 0.1)])
                },
                Some(|crop: &GrayImage| {
                    ocrs += 1;
                    assert_eq!(crop.dimensions(), (10, 30));
                    Ok(LineRead {
                        text: "7.".into(),
                        confidence: 0.8,
                    })
                }),
                CROP_MARGIN,
                false,
                trace,
                Some(&mut budget),
                writing,
                None,
            )
            .unwrap();
            assert_eq!(
                (classifiers, ocrs),
                (1, 1),
                "no per-label half-turn at {angle}"
            );
            assert_eq!((budget.classifiers, budget.ocrs, budget.reads), (1, 1, 1));
            assert!(budget.read_pixels >= u64::from(p.crop.width() * p.crop.height()));
            assert!(!p.turned);
            assert_eq!(p.quad, quad, "preparing must not change canonical geometry");
            assert_eq!(
                p.crop,
                crate::split::level_crop_within(
                    &photo,
                    writing.crop_frame(&quad, CROP_MARGIN),
                    &quad
                )
            );
            if trace {
                let events = p.decisions.as_ref().unwrap().to_json();
                let half = events["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["rule"] == "half_turn")
                    .unwrap();
                assert_eq!(half["reason"], "shared_page_direction");
            }
            // Adoption uses the exact same writing direction, including polarity.
            let cut = 7;
            let after = Past {
                crop: image::imageops::crop_imm(
                    &p.crop,
                    cut,
                    0,
                    p.crop.width() - cut,
                    p.crop.height(),
                )
                .to_image(),
                ranked: p.ranked.clone(),
                read: p.read.clone().unwrap(),
            };
            let old = p.adopt(cut, after, CROP_MARGIN);
            assert_eq!(old.quad, quad);
            let projected = d.quad_to_page(&p.quad);
            close(
                projected
                    .0
                    .iter()
                    .map(|p| p.0)
                    .fold(f32::INFINITY, f32::min),
                -13.,
            );
            close(writing.frame_of(&p.quad).w, 3.);
            close(writing.frame_of(&p.quad).h, 30.);
            results.push((p.quad, p.crop, p.ranked, p.read));
        }
        assert_eq!(
            results[0], results[1],
            "trace-on/off must not change a reading"
        );
    }
    assert_eq!(photo, original);
}

#[test]
fn shared_direction_preserves_layout_label_owner_and_union_across_turns() {
    for angle in [0., 17., 90., 180., 270.] {
        let d = test_direction(angle);
        let writing = WritingFrame::Page(d);
        let quads = [
            q(d, 0., 0., 80., 30.),
            q(d, 120., 0., 80., 30.),
            q(d, 0., 45., 80., 30.),
            q(d, 120., 45., 80., 30.),
        ];
        let layout = crate::layout::PageLayout::in_direction(&quads, d);
        assert_eq!(layout.column_order(), [0, 2, 1, 3]);
        assert_eq!(layout.row_order(), [0, 1, 2, 3]);
        assert_eq!(layout.reindexed(&[0, 2, 1, 3]).row_order(), [0, 2, 1, 3]);
        assert_eq!(layout.selected(&[3, 0]).writing, writing);
        assert_eq!(crate::region::apart_in(&layout, &[true; 4]), [false; 4]);

        let label = q(d, -20., 0., 10., 30.);
        let candidates = [
            Candidate {
                quad: &label,
                read: Some("7."),
                token: None,
                word_like: false,
            },
            Candidate {
                quad: &quads[0],
                read: Some("basket"),
                token: None,
                word_like: true,
            },
        ];
        for trace in [false, true] {
            let out = labels::label_evidence_in(&candidates, trace, writing);
            assert_eq!(out.label_box, [true, false]);
            assert!(matches!(
                out.evidence[1],
                Some(Evidence::Beside(Label::Exact(7)))
            ));
            assert_eq!(out.observations[1].labels[0].corners, label.0);
        }
        let union = crate::joins::union_in(&quads[0], &quads[1], writing);
        close(writing.frame_of(&union).w, 200.);
        close(writing.frame_of(&union).h, 30.);
    }
}

#[test]
fn shared_splitter_keeps_raw_ids_and_tall_label_direction() {
    // Simple narrow mark, not a font/model test. Rotate the same photo and quads.
    let mut photo = RgbImage::from_pixel(256, 256, Rgb([240; 3]));
    for y in 80..105 {
        for x in 92..95 {
            photo.put_pixel(x, y, Rgb([10; 3]));
        }
    }
    for y in 81..86 {
        for x in 99..101 {
            photo.put_pixel(x, y, Rgb([10; 3]));
        }
    }
    let raw = [
        Quad([(90., 78.), (104., 78.), (104., 108.), (90., 108.)]),
        Quad([(120., 80.), (210., 80.), (210., 110.), (120., 110.)]),
    ];
    let original = raw.clone();
    for (image, degrees) in [
        (photo.clone(), 0.),
        (image::imageops::rotate90(&photo), 270.),
        (image::imageops::rotate180(&photo), 180.),
    ] {
        let rotated: Vec<_> = raw
            .iter()
            .map(|q| {
                Quad(q.0.map(|(x, y)| {
                    let (x, y) = turn(x - 128., y - 128., degrees);
                    (x + 128., y + 128.)
                }))
            })
            .collect();
        let axis = PageAxis::from_detections(&rotated);
        let d = axis.direction((degrees - axis.angle_degrees()).to_radians().cos() < 0.);
        let split = Segmentation::in_direction(&image, &rotated, d);
        assert_eq!(split.sources.len(), 2);
        for (i, s) in split.sources.iter().enumerate() {
            assert_eq!(s.detector, i);
            assert_eq!(s.quad, rotated[i]);
        }
        assert_eq!(split.sources[0].parts().len(), 1);
        let part = &split.sources[0].parts()[0];
        assert_eq!((part.part, part.word_index), (0, Some(0)));
        let f = d.frame(&part.quad, 0.).unwrap();
        assert!(
            f.h > 2. * f.w,
            "tall label must not split/crop on its long side: {f:?}"
        );
        assert!(split.sources[1].parts().is_empty()); // Preserve the blank anchor source too.
    }
    assert_eq!(raw, original);
}
