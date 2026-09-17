//! Partition a detector extent across established word columns before reading.
use super::*;
use image::Luma;
use imageproc::region_labelling::{Connectivity, connected_components};

impl Grid {
    pub(in crate::phrase) fn partition(
        &mut self,
        photo: &RgbImage,
        q: &Quad,
        writing: WritingFrame,
    ) -> Vec<Quad> {
        let b = Bounds::of(&writing.project(q));
        let pieces: Vec<_> = self
            .columns
            .iter()
            .filter_map(|c| {
                if b.y() < c.top || b.y() > c.bottom {
                    return None;
                }
                let cut = Bounds {
                    l: b.l.max(c.left_limit(b)),
                    r: b.r.min(c.right_limit(b)),
                    ..b
                };
                (cut.w() > 1.).then_some(cut)
            })
            .collect();
        if pieces.len() < 2 {
            return vec![q.clone()];
        }
        // Classify connected ink BEFORE a column boundary can turn a label's
        // edge into an apparently independent word fragment.
        let frame = writing.crop_frame(q, 0.);
        let crop = level_crop_in(photo, frame);
        let mut ink = source_ink(photo, q, frame, &crop);
        strip_rules(&mut ink);
        let mask = GrayImage::from_fn(crop.width(), crop.height(), |x, y| {
            Luma([if ink[y as usize][x as usize] { 255 } else { 0 }])
        });
        let point = |x: u32, y: u32| {
            let (dx, dy) = turn(
                x as f32 + 0.5 - crop.width() as f32 / 2.,
                y as f32 + 0.5 - crop.height() as f32 / 2.,
                frame.angle,
            );
            writing
                .project(&Quad([(frame.cx + dx, frame.cy + dy); 4]))
                .0[0]
        };
        let mut label_ink = GrayImage::new(crop.width(), crop.height());
        for (gutter, (c, width)) in self.gutters.iter().enumerate() {
            if b.b < c.top
                || b.t > c.bottom
                || b.l >= c.left_limit(b)
                || b.r <= c.left(b.t).min(c.left(b.b)) - width
            {
                continue;
            }
            // First apply the already-established word-start line. A ruling
            // can connect label ink to its own word; that authorized cut must
            // separate them before measuring label-side component ownership.
            let side = GrayImage::from_fn(crop.width(), crop.height(), |x, y| {
                let p = point(x, y);
                if p.0 < c.left(p.1) {
                    *mask.get_pixel(x, y)
                } else {
                    Luma([0])
                }
            });
            let components = connected_components(&side, Connectivity::Eight, Luma([0]));
            let mut counts = std::collections::BTreeMap::<u32, (usize, usize)>::new();
            for (x, y, id) in components
                .enumerate_pixels()
                .filter(|(_, _, id)| id[0] != 0)
            {
                let p = point(x, y);
                let (total, inside) = counts.entry(id[0]).or_default();
                *total += 1;
                if p.1 >= c.top && p.1 <= c.bottom && p.0 >= c.left(p.1) - width {
                    *inside += 1;
                }
            }
            for (x, y, id) in components
                .enumerate_pixels()
                .filter(|(_, _, id)| id[0] != 0)
            {
                let (total, inside) = counts[&id[0]];
                if inside * 2 > total {
                    label_ink.put_pixel(x, y, Luma([255]));
                }
            }
            DecisionTrace::push(&mut self.decisions, || {
                json!({
                    "rule":"grid_partition_ink_ownership","status":"evaluated",
                    "original_quad":q.0,"gutter":gutter,"ownership":"label_side_component_majority_in_gutter",
                    "components":counts.iter().map(|(&id,(total,inside))|json!({"component":id,"pixels":total,"gutter_pixels":inside,"label_owned":inside*2>*total})).collect::<Vec<_>>(),
                    "extra_ocr_calls":0
                })
            });
        }
        // A box reaching into the next column's whitespace is not another
        // word. Measure only ink inside the original polygon; remove ruling
        // lines before tightening width. Keep the original vertical extent.
        let mut occupied = Vec::new();
        let has_labels = label_ink.pixels().any(|p| p[0] != 0);
        for mut piece in pieces {
            if has_labels
                && !mask.enumerate_pixels().any(|(x, y, pixel)| {
                    let (px, py) = point(x, y);
                    pixel[0] != 0
                        && label_ink.get_pixel(x, y)[0] == 0
                        && px >= piece.l
                        && px <= piece.r
                        && py >= piece.t
                        && py <= piece.b
                })
            {
                DecisionTrace::push(&mut self.decisions, || {
                    json!({
                        "rule":"grid_partition_piece","status":"evaluated",
                        "original_quad":q.0,"intersection_quad":writing.unproject(&piece.quad()).0,
                        "reason":"no_nonlabel_ink_before_partition","promoted_to_word":false,
                        "extra_ocr_calls":0
                    })
                });
                continue;
            }
            let quad = writing.unproject(&piece.quad());
            let frame = writing.crop_frame(&quad, 0.);
            let crop = level_crop_in(photo, frame);
            let mut ink = source_ink(photo, q, frame, &crop);
            strip_rules(&mut ink);
            let mut lo = usize::MAX;
            let mut hi = 0;
            for row in &ink {
                for (x, &dark) in row.iter().enumerate() {
                    if dark {
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
            }
            if lo == usize::MAX {
                continue;
            }
            let width = crop.width() as f32;
            let left = piece.l;
            let span = piece.w();
            piece.l = left + lo as f32 / width * span;
            piece.r = left + (hi + 1) as f32 / width * span;
            occupied.push(writing.unproject(&piece.quad()));
        }
        // No geometry change when there is only one ink-bearing intersection.
        // The ordinary common crop owner still applies its width constraints.
        if occupied.len() < 2 {
            vec![q.clone()]
        } else {
            occupied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_cannot_promote_a_connected_label_edge_but_keeps_real_left_word() {
        for angle in [0_f32, 18.] {
            let map = |x, y| {
                let (x, y) = turn(x, y, angle);
                (x + 100., y + 100.)
            };
            let q = Quad([(0., 0.), (260., 0.), (260., 50.), (0., 50.)].map(|(x, y)| map(x, y)));
            let writing = WritingFrame::Page(
                crate::page_frame::PageAxis::from_detections(&[q.clone()]).direction(false),
            );
            let b = Bounds::of(&writing.project(&q));
            let right = ColumnBounds {
                right_slope: 0.,
                intercept: b.l + 155.,
                slope: 0.,
                right: b.r,
                top: b.t - 20.,
                bottom: b.b + 20.,
            };
            let mut grid = Grid {
                columns: vec![
                    ColumnBounds {
                        intercept: b.l + 10.,
                        right: b.l + 115.,
                        ..right
                    },
                    right,
                ],
                gutters: vec![(right, 40.)],
                ..Grid::default()
            };
            let mut photo = RgbImage::from_pixel(450, 300, image::Rgb([240; 3]));
            let paint = |photo: &mut RgbImage, l, r, t, b| {
                for y in t..b {
                    for x in l..r {
                        let (x, y) = map(x as f32, y as f32);
                        photo.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                    }
                }
            };
            paint(&mut photo, 108, 143, 10, 40); // label extends past gutter's left edge
            for x in [180, 205, 230] {
                paint(&mut photo, x, x + 10, 10, 40);
            } // its word
            paint(&mut photo, 110, 245, 24, 27); // thin ruling connects the two
            assert_eq!(grid.partition(&photo, &q, writing), vec![q.clone()]);
            // A real neighbouring word still gets its own piece, even though
            // both it and the label occur before the next word-start line.
            for x in [25, 50, 75] {
                paint(&mut photo, x, x + 10, 10, 40);
            }
            assert_eq!(grid.partition(&photo, &q, writing).len(), 2);
        }
    }

    #[test]
    fn separates_two_ink_regions_without_clipping_height_or_inventing_empty_words() {
        for angle in [0_f32, 18.] {
            let map = |x, y| {
                let (x, y) = turn(x, y, angle);
                (x + 100., y + 100.)
            };
            let q = Quad([(0., 0.), (260., 0.), (260., 50.), (0., 50.)].map(|(x, y)| map(x, y)));
            let writing = WritingFrame::Page(
                crate::page_frame::PageAxis::from_detections(&[q.clone()]).direction(false),
            );
            let b = Bounds::of(&writing.project(&q));
            let mut grid = Grid {
                columns: vec![
                    ColumnBounds {
                        right_slope: 0.,
                        intercept: b.l + 10.,
                        slope: 0.,
                        right: b.l + 115.,
                        top: b.t - 20.,
                        bottom: b.b + 20.,
                    },
                    ColumnBounds {
                        right_slope: 0.,
                        intercept: b.l + 155.,
                        slope: 0.,
                        right: b.r,
                        top: b.t - 20.,
                        bottom: b.b + 20.,
                    },
                ],
                ..Grid::default()
            };
            let mut photo = RgbImage::from_pixel(450, 300, image::Rgb([240; 3]));
            for (l, r) in [(25, 95), (180, 245)] {
                for y in 10..40 {
                    for x in l..r {
                        if (x - l) % 15 > 6 {
                            continue;
                        }
                        let (x, y) = map(x as f32, y as f32);
                        photo.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                    }
                }
            }
            let parts = grid.partition(&photo, &q, writing);
            assert_eq!(parts.len(), 2);
            for (part, c) in parts.iter().zip(&grid.columns) {
                let p = Bounds::of(&writing.project(part));
                assert!(p.l >= c.left_limit(p) - 0.001 && p.r <= c.right + 0.001);
                assert!((p.t - b.t).abs() < 0.001 && (p.b - b.b).abs() < 0.001);
            }
            let blank = RgbImage::from_pixel(450, 300, image::Rgb([240; 3]));
            assert_eq!(grid.partition(&blank, &q, writing), vec![q.clone()]);
            // A wide detection may extend into a completely empty neighbour.
            let mut only_left = blank.clone();
            for y in 10..40 {
                for x in 25..95 {
                    if (x - 25) % 15 > 6 {
                        continue;
                    }
                    let (x, y) = map(x as f32, y as f32);
                    only_left.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                }
            }
            assert_eq!(grid.partition(&only_left, &q, writing), vec![q.clone()]);
            let first = parts[0].clone();
            assert_eq!(grid.partition(&photo, &first, writing), vec![first]);
        }
    }
}
