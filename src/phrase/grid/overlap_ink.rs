//! Evidence that two overlapping fragments sever the same connected stroke.
use super::*;
use imageproc::region_labelling::{Connectivity, connected_components};

pub(super) struct Evidence {
    #[cfg(test)]
    pub(super) union: Quad,
    #[cfg(test)]
    pub(super) crop: GrayImage,
    #[cfg(test)]
    #[allow(dead_code)] // Retained for the private white-box diagnostic overlay.
    pub(super) mask: GrayImage,
    // total, shared, exclusively in first fragment, exclusively in second
    counts: Vec<[usize; 4]>,
    ruling: Vec<serde_json::Value>,
}

impl Evidence {
    pub(super) fn bridges(&self) -> bool {
        self.counts.iter().any(|p| p[1] > 0 && p[2] > 0 && p[3] > 0)
    }

    pub(super) fn to_json(&self) -> serde_json::Value {
        json!({"method":"shared_connected_stroke_with_two_exclusive_regions",
            "connectivity":8,"crosses_both":self.bridges(),
            "crosses_either":self.counts.iter().any(|p|p[1]>0 && (p[2]>0 || p[3]>0)),
            "components":self.counts.iter().filter(|p|p[1]>0).map(|p|json!({
                "pixels":p[0],"shared":p[1],"left_only":p[2],"right_only":p[3]})).collect::<Vec<_>>(),
            "mask":"valid_union_otsu_existing_ruling_removal_no_dilation",
            "ruling":self.ruling})
    }
}

// Call only after the existing crop-pixel budget and geometric/label guards.
pub(super) fn measure(photo: &RgbImage, a: &Quad, b: &Quad, writing: WritingFrame) -> Evidence {
    let union = crate::joins::union_in(a, b, writing);
    let frame = writing.crop_frame(&union, 0.);
    let crop = level_crop_in(photo, frame);
    let mut ink = source_ink(photo, &union, frame, &crop);
    strip_rules(&mut ink);
    let ruling = ruling::strip_context_rules(photo, frame, &mut ink);
    let mask = GrayImage::from_fn(crop.width(), crop.height(), |x, y| {
        image::Luma([if ink[y as usize][x as usize] { 255 } else { 0 }])
    });
    let components = connected_components(&mask, Connectivity::Eight, image::Luma([0]));
    let (qa, qb) = (writing.project(a), writing.project(b));
    let bounds = Bounds::of(&writing.project(&union));
    let ox = (bounds.l + bounds.r - crop.width() as f32) / 2.;
    let oy = (bounds.t + bounds.b - crop.height() as f32) / 2.;
    let contains = |q: &Quad, x: f32, y: f32| {
        let crosses = [0, 1, 2, 3].map(|i| {
            let (a, b) = (q.0[i], q.0[(i + 1) % 4]);
            (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
        });
        crosses.iter().all(|c| *c >= 0.) || crosses.iter().all(|c| *c <= 0.)
    };
    let mut counts = Vec::<[usize; 4]>::new();
    for (x, y, id) in components.enumerate_pixels() {
        let id = id.0[0] as usize;
        if id == 0 {
            continue;
        }
        if counts.len() < id {
            counts.resize(id, [0; 4]);
        }
        let (px, py) = (ox + x as f32 + 0.5, oy + y as f32 + 0.5);
        let (l, r) = (contains(&qa, px, py), contains(&qb, px, py));
        let p = &mut counts[id - 1];
        p[0] += 1;
        p[1] += usize::from(l && r);
        p[2] += usize::from(l && !r);
        p[3] += usize::from(r && !l);
    }
    Evidence {
        #[cfg(test)]
        union,
        #[cfg(test)]
        crop,
        #[cfg(test)]
        mask,
        counts,
        ruling,
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
    fn fixture() -> (RgbImage, Vec<(usize, Quad)>) {
        let mut photo = RgbImage::from_pixel(160, 480, image::Rgb([255; 3]));
        // A connected sloping stroke crosses both exclusive regions and their overlap.
        for x in 40..95 {
            for dy in 0..4 {
                photo.put_pixel(x, 30 + (x - 40) / 5 + dy, image::Rgb([20; 3]));
            }
        }
        let mut parts = vec![(0, rect(30., 20., 50., 30.)), (1, rect(70., 30., 30., 16.))];
        parts.extend((2..13).map(|i| (i, rect(25., 70. + (i - 2) as f32 * 32., 90., 24.))));
        (photo, parts)
    }

    #[test]
    fn shared_stroke_owns_one_exact_unpadded_union_without_grid_or_ordinal() {
        let (photo, parts) = fixture();
        let mut lean = Grid::default();
        let mut traced = Grid {
            decisions: Some(DecisionTrace::default()),
            ..Default::default()
        };
        let groups = lean.compact_groups(&photo, &parts, WritingFrame::Local);
        assert_eq!(
            groups,
            traced.compact_groups(&photo, &parts, WritingFrame::Local)
        );
        assert_eq!(groups, vec![(vec![0, 1], rect(30., 20., 70., 30.))]);
        assert_eq!(parts.len() - groups[0].0.len() + 1, 12); // 13 outputs become12, never a forced count.
        let owner = lean
            .word_cell(&photo, &groups[0].1, WritingFrame::Local)
            .unwrap();
        assert!(owner.column.is_none() && owner.row.is_none() && owner.ordinal.is_none());
        assert!(lean.compact_word(&groups[0].1));
        assert!(!lean.compact_word(&parts[0].1));
        let e = measure(&photo, &parts[0].1, &parts[1].1, WritingFrame::Local);
        assert!(e.bridges());
        assert_eq!(e.crop.dimensions(), (70, 30));
        assert_eq!(e.union, groups[0].1);
    }

    #[test]
    fn counts_overlap_and_ruling_cannot_replace_shared_stroke_ownership() {
        let (photo, parts) = fixture();
        for case in 0..7 {
            let mut photo = photo.clone();
            let mut parts = parts.clone();
            let mut grid = Grid::default();
            match case {
                0 => {
                    for y in 0..photo.height() {
                        for x in 70..80 {
                            photo.put_pixel(x, y, image::Rgb([255; 3]));
                        }
                    }
                } // whitespace overlap
                1 => parts[1].1 = rect(90., 30., 30., 16.), // no shared area; positive-gap policy is unchanged
                2 => parts[1].1 = rect(70., 65., 30., 16.), // another row
                3 => parts[1].1 = rect(140., 30., 30., 16.), // another column
                4 => grid.labels.push(Bounds::of(&parts[0].1)),
                5 => grid.columns.push(ColumnBounds {
                    intercept: 0.,
                    slope: 0.,
                    right: 140.,
                    right_slope: 0.,
                    top: 0.,
                    bottom: 480.,
                }),
                6 => {
                    photo = RgbImage::from_pixel(160, 480, image::Rgb([255; 3]));
                    for x in 0..160 {
                        photo.put_pixel(x, 35, image::Rgb([20; 3]));
                    }
                }
                _ => unreachable!(),
            }
            // The 13/12 mismatch is never sufficient on its own.
            if case == 1 {
                assert!(!measure(&photo, &parts[0].1, &parts[1].1, WritingFrame::Local).bridges());
            } else {
                assert!(
                    grid.compact_groups(&photo, &parts, WritingFrame::Local)
                        .is_empty(),
                    "case {case}"
                );
            }
        }
        let contained = rect(70., 30., 8., 16.);
        assert!(!measure(&photo, &parts[0].1, &contained, WritingFrame::Local).bridges());
    }

    #[test]
    fn connected_overlap_survives_page_rotation_and_scale() {
        let (photo, parts) = fixture();
        for scale in [1, 2] {
            let photo = image::imageops::resize(
                &photo,
                photo.width() * scale,
                photo.height() * scale,
                image::imageops::FilterType::Nearest,
            );
            for angle in [0, 90, 180, 270] {
                let (w, h) = (photo.width() as f32, photo.height() as f32);
                let rotated = match angle {
                    0 => photo.clone(),
                    90 => image::imageops::rotate90(&photo),
                    180 => image::imageops::rotate180(&photo),
                    _ => image::imageops::rotate270(&photo),
                };
                let map = |q: &Quad| {
                    Quad(q.0.map(|(x, y)| {
                        let (x, y) = (x * scale as f32, y * scale as f32);
                        match angle {
                            0 => (x, y),
                            90 => (h - y, x),
                            180 => (w - x, h - y),
                            _ => (y, w - x),
                        }
                    }))
                };
                let writing = WritingFrame::Page(crate::page_frame::test_direction(angle as f32));
                assert!(
                    measure(&rotated, &map(&parts[0].1), &map(&parts[1].1), writing).bridges(),
                    "scale {scale}, angle {angle}"
                );
            }
        }
    }
}
