//! Ruling must continue outside both sides of the measured word box.
use super::*;

pub(super) fn strip_context_rules(
    photo: &RgbImage,
    frame: Frame,
    ink: &mut [Vec<bool>],
) -> Vec<serde_json::Value> {
    let mut evidence = Vec::new();
    let (w, h) = (frame.w.round() as usize, frame.h.round() as usize);
    if w < 2 * h || h < 8 || ink.len() != h {
        return evidence;
    }
    let pad = (w as f32 * 0.12).ceil() as usize;
    let vertical = (h as f32 * 0.25).ceil() as usize;
    let expanded = Frame {
        w: (w + 2 * pad) as f32,
        h: (h + 2 * vertical) as f32,
        ..frame
    };
    let q = Quad(
        [
            (-expanded.w / 2., -expanded.h / 2.),
            (expanded.w / 2., -expanded.h / 2.),
            (expanded.w / 2., expanded.h / 2.),
            (-expanded.w / 2., expanded.h / 2.),
        ]
        .map(|(x, y)| {
            let (x, y) = turn(x, y, frame.angle);
            (frame.cx + x, frame.cy + y)
        }),
    );
    // The two paper borders are evidence too: artificial out-of-photo fill
    // must never make a candidate appear thin or locally high-contrast.
    if q.0
        .iter()
        .any(|&(x, y)| x < 0. || y < 0. || x >= photo.width() as f32 || y >= photo.height() as f32)
    {
        return evidence;
    }
    let crop = level_crop_in(photo, expanded);
    let mask = source_ink(photo, &q, expanded, &crop);
    let thickness = (h as f32 * 0.12).ceil() as usize;
    let runs = |x: usize| {
        let mut out = Vec::new();
        let mut y = 0;
        while y < mask.len() {
            if !mask[y][x] {
                y += 1;
                continue;
            }
            let start = y;
            while y < mask.len() && mask[y][x] {
                y += 1;
            }
            if start > 0 && y < mask.len() && y - start <= thickness {
                out.push((start as f32, (y - 1) as f32));
            }
        }
        out
    };
    let span = crop.width() as usize - 1;
    for (lt, lb) in runs(0) {
        for (rt, rb) in runs(span) {
            let left = (lt + lb) / 2.;
            let right = (rt + rb) / 2.;
            let slope = (right - left) / span as f32;
            if slope.abs() > 0.15 {
                continue;
            }
            let radius = ((lb - lt).max(rb - rt) / 2. + 1.).max(1.);
            // Word strokes and notebook ruling need not have the same ink
            // darkness. Fit the candidate's contrast using exactly its band
            // and paper borders, not unrelated text elsewhere in the crop.
            let bands: Vec<_> = (0..=span)
                .map(|x| {
                    let cy = left + slope * x as f32;
                    let top = (cy - radius).floor() as isize;
                    let bottom = (cy + radius).ceil() as isize;
                    (top >= 1 && bottom + 1 < mask.len() as isize).then_some((top, bottom))
                })
                .collect();
            let mut samples = Vec::new();
            for (x, band) in bands.iter().enumerate() {
                if let Some((top, bottom)) = band {
                    for y in top - 1..=bottom + 1 {
                        samples.push(crop.get_pixel(x as u32, y as u32).0[0]);
                    }
                }
            }
            if samples
                .first()
                .is_none_or(|first| samples.iter().all(|p| p == first))
            {
                continue;
            }
            let histogram = GrayImage::from_raw(samples.len() as u32, 1, samples).unwrap();
            let threshold = imageproc::contrast::otsu_level(&histogram);
            // A thin band, bounded by paper above and below, must continue
            // across almost the full extended width. Crossed letter stems
            // may interrupt that evidence, but cannot supply most of it.
            let mut supported = 0;
            for x in 0..=span {
                let Some((top, bottom)) = bands[x] else {
                    continue;
                };
                let dark_at = |y| crop.get_pixel(x as u32, y as u32).0[0] <= threshold;
                let dark = (top..=bottom).any(dark_at);
                if dark && !dark_at(top - 1) && !dark_at(bottom + 1) {
                    supported += 1;
                }
            }
            if supported * 5 < (span + 1) * 4 {
                continue;
            }
            evidence.push(json!({"context_quad":q.0,"sample_size":[crop.width(),crop.height()],
                "left_y":left,"right_y":right,"radius":radius,"slope":slope,
                "contrast_region":"candidate_band_and_paper_borders","threshold":threshold,
                "supported_columns":supported,"total_columns":span+1,"minimum_support_fraction":0.8}));
            for (y, row) in ink.iter_mut().enumerate() {
                for (x, dark) in row.iter_mut().enumerate() {
                    let cy = left + slope * (x + pad) as f32;
                    // Use exactly the raster band that supplied the support
                    // above; centre-only removal leaves fractional edge tails.
                    let sample_y = (y + vertical) as f32;
                    if sample_y >= (cy - radius).floor() && sample_y <= (cy + radius).ceil() {
                        *dark = false;
                    }
                }
            }
        }
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_thin_continuation_beyond_both_word_edges() {
        for angle in [0., 18.] {
            for (full, faint, broken) in [
                (false, false, false),
                (true, false, false),
                (true, true, false),
                (true, true, true),
            ] {
                let mut photo = RgbImage::from_pixel(440, 300, image::Rgb([240; 3]));
                let frame = Frame {
                    cx: 220.,
                    cy: 150.,
                    w: 200.,
                    h: 60.,
                    angle,
                };
                for (x, y, p) in photo.enumerate_pixels_mut() {
                    let (x, y) = turn(x as f32 + 0.5 - frame.cx, y as f32 + 0.5 - frame.cy, -angle);
                    let stroke = (!broken || x.abs() > 35.)
                        && x.abs() < (if full { 150. } else { 80. })
                        && (y - 15. - 0.04 * x).abs() < 1.5;
                    let word = x.abs() < 65. && y > -20. && y < 2. && (x + 65.) % 30. < 8.;
                    if word {
                        *p = image::Rgb([20; 3]);
                    } else if stroke {
                        *p = image::Rgb(
                            [if faint {
                                if x.abs() < 90. { 180 } else { 120 }
                            } else {
                                20
                            }; 3],
                        );
                    }
                }
                let crop = level_crop_in(&photo, frame);
                let mut ink = crate::split::binarise(&crop);
                let before = ink.clone();
                let evidence = strip_context_rules(&photo, frame, &mut ink);
                if full && !broken {
                    assert!(!evidence.is_empty(), "angle={angle}, faint={faint}");
                    if faint {
                        assert!(
                            evidence
                                .iter()
                                .any(|e| e["threshold"].as_u64().unwrap() >= 180)
                        );
                    }
                    assert!(before.iter().any(|row| row[0]));
                    assert!(
                        ink.iter().all(|row| !row[0]),
                        "angle={angle}, evidence={evidence:?}, left={:?}",
                        ink.iter()
                            .enumerate()
                            .filter_map(|(y, r)| r[0].then_some(y))
                            .collect::<Vec<_>>()
                    );
                    assert!(ink.iter().flatten().any(|p| *p));
                    assert_eq!(&ink[5..30], &before[5..30]);
                } else {
                    assert!(evidence.is_empty());
                    assert_eq!(ink, before, "a word stroke is not external ruling");
                }
                let mut edge_ink = before.clone();
                assert!(
                    strip_context_rules(&photo, Frame { cx: 20., ..frame }, &mut edge_ink)
                        .is_empty()
                );
                assert_eq!(edge_ink, before, "out-of-photo fill is not paper evidence");
            }
        }
    }
}
