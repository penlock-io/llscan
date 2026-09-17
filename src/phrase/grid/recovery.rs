//! Follow seeded ink components within an established cell, before recognition.
//! Unseeded or boundary-connected ink cannot authorize growth. Never cut the seed.
use super::*;

pub(super) fn recover_traced(
    photo: &RgbImage,
    base: &Quad,
    cell: &Quad,
    writing: WritingFrame,
    trace: bool,
) -> (Quad, serde_json::Value) {
    let b = Bounds::of(&writing.project(base));
    let c = Bounds::of(&writing.project(cell));
    let refuse = |why| {
        (
            base.clone(),
            if trace {
                json!({"reason":why,"changed":false})
            } else {
                serde_json::Value::Null
            },
        )
    };
    if b.l < c.l || b.r > c.r || b.t < c.t || b.b > c.b {
        return refuse("seed_crosses_cell");
    }
    if cell
        .0
        .iter()
        .any(|&(x, y)| x < 0. || y < 0. || x >= photo.width() as f32 || y >= photo.height() as f32)
    {
        return refuse("cell_crosses_photo");
    }
    let frame = writing.crop_frame(cell, 0.);
    if frame.w.ceil() as u64 * frame.h.ceil() as u64 > crate::read_budget::MAX_READ_PIXELS {
        return refuse("roi_limit");
    }
    let crop = level_crop_in(photo, frame);
    let mut ink = source_ink(photo, cell, frame, &crop);
    let ruling = super::ruling::strip_context_rules(photo, frame, &mut ink);
    strip_rules(&mut ink);
    let h = ink.len();
    let w = ink.first().map_or(0, Vec::len);
    if w == 0 || h == 0 {
        return refuse("empty");
    }
    let origin = ((c.l + c.r - w as f32) / 2., (c.t + c.b - h as f32) / 2.);
    let mut seen = vec![false; w * h];
    let mut out = b;
    let mut components = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if !ink[y][x] || seen[y * w + x] {
                continue;
            }
            seen[y * w + x] = true;
            let mut stack = vec![(x, y)];
            let (mut l, mut t, mut r, mut bottom) = (x, y, x + 1, y + 1);
            let mut seed = 0;
            let mut count = 0;
            let mut edge = false;
            while let Some((x, y)) = stack.pop() {
                count += 1;
                l = l.min(x);
                t = t.min(y);
                r = r.max(x + 1);
                bottom = bottom.max(y + 1);
                edge |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
                let p = (origin.0 + x as f32 + 0.5, origin.1 + y as f32 + 0.5);
                if p.0 >= b.l && p.0 <= b.r && p.1 >= b.t && p.1 <= b.b {
                    seed += 1;
                }
                for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        if ink[yy][xx] && !seen[yy * w + xx] {
                            seen[yy * w + xx] = true;
                            stack.push((xx, yy));
                        }
                    }
                }
            }
            let accepted = seed > 0 && !edge;
            let guard = std::f32::consts::FRAC_1_SQRT_2;
            let bounds = Bounds {
                l: (origin.0 + l as f32 - guard).max(c.l),
                t: (origin.1 + t as f32 - guard).max(c.t),
                r: (origin.0 + r as f32 + guard).min(c.r),
                b: (origin.1 + bottom as f32 + guard).min(c.b),
            };
            if accepted && seed < count {
                out.l = out.l.min(bounds.l);
                out.r = out.r.max(bounds.r);
                out.t = out.t.min(bounds.t);
                out.b = out.b.max(bounds.b);
            }
            if trace {
                components.push(
                    json!({"pixels":count,"seed_pixels":seed,"touches_cell_edge":edge,
            "accepted":accepted,"quad":writing.unproject(&bounds.quad()).0}),
                );
            }
        }
    }
    let changed = out.l < b.l || out.t < b.t || out.r > b.r || out.b > b.b;
    (
        writing.unproject(&out.quad()),
        if trace {
            json!({"reason":"seeded_components","changed":changed,
        "cell":cell.0,"base":base.0,"ruling":ruling,"components":components})
        } else {
            serde_json::Value::Null
        },
    )
}

#[cfg(test)]
fn recover(
    photo: &RgbImage,
    base: &Quad,
    cell: &Quad,
    writing: WritingFrame,
) -> (Quad, serde_json::Value) {
    recover_traced(photo, base, cell, writing, true)
}

#[test]
fn connected_strokes_expand_but_boundaries_and_unseeded_marks_do_not() {
    let q = |l, t, r, b| Bounds { l, t, r, b }.quad();
    let mut photo = RgbImage::from_pixel(160, 100, image::Rgb([240; 3]));
    // A connected stem extends left/up beyond the original box.
    for y in 25..65 {
        for x in 40..46 {
            photo.put_pixel(x, y, image::Rgb([20; 3]));
        }
    }
    for y in 50..55 {
        for x in 40..80 {
            photo.put_pixel(x, y, image::Rgb([20; 3]));
        }
    }
    // Unrelated isolated mark, and background connected to the cell's right edge.
    for y in 40..50 {
        for x in 22..28 {
            photo.put_pixel(x, y, image::Rgb([20; 3]));
        }
    }
    for y in 60..65 {
        for x in 75..145 {
            photo.put_pixel(x, y, image::Rgb([20; 3]));
        }
    }
    let base = q(60., 40., 90., 70.);
    let cell = q(20., 20., 140., 80.);
    let (out, _) = recover(&photo, &base, &cell, WritingFrame::Local);
    let out = Bounds::of(&out);
    assert!(out.l < 41. && out.l > 38. && out.t < 26. && out.t > 23.);
    assert_eq!(out.r, 90.);
    assert_eq!(out.b, 70.);
    // Ineligible geometry is not cropped or expanded to force an answer.
    assert_eq!(
        recover(&photo, &q(10., 40., 90., 70.), &cell, WritingFrame::Local).1["reason"],
        "seed_crosses_cell"
    );
}

#[test]
fn disconnected_ink_already_in_seed_is_preserved_not_dropped() {
    let mut photo = RgbImage::from_pixel(160, 100, image::Rgb([240; 3]));
    photo.put_pixel(65, 42, image::Rgb([20; 3]));
    let base = Bounds {
        l: 60.,
        t: 40.,
        r: 90.,
        b: 70.,
    }
    .quad();
    let cell = Bounds {
        l: 20.,
        t: 20.,
        r: 140.,
        b: 80.,
    }
    .quad();
    assert_eq!(recover(&photo, &base, &cell, WritingFrame::Local).0, base);
}

#[test]
fn rotated_ruled_cell_recovers_connected_cap_without_following_neighbor_or_label() {
    for (angle, ruled) in [(0., false), (18., false), (0., true), (18., true)] {
        let map = |x: f32, y: f32| {
            let (x, y) = turn(x, y, angle);
            (x + 200., y + 160.)
        };
        let make = |l, t, r, b| Quad([(l, t), (r, t), (r, b), (l, b)].map(|(x, y)| map(x, y)));
        let base = make(-20., -20., 40., 25.);
        let cell = make(-70., -50., 100., 50.);
        let writing = WritingFrame::Page(
            crate::page_frame::PageAxis::from_detections(&[cell.clone()]).direction(false),
        );
        let mut photo = RgbImage::from_pixel(440, 340, image::Rgb([240; 3]));
        for (x, y, p) in photo.enumerate_pixels_mut() {
            let (x, y) = turn(x as f32 + 0.5 - 200., y as f32 + 0.5 - 160., -angle);
            let stem = (-40. ..-34.).contains(&x) && (-35. ..35.).contains(&y);
            let bar = (-40. ..25.).contains(&x) && (-5. ..1.).contains(&y);
            let ruling = ruled && y > 15. && y < 17.;
            let neighbor = (30. ..35.).contains(&x) && y < -15.;
            let label = x < -75. && (-10. ..20.).contains(&y);
            if stem || bar || ruling || neighbor || label {
                *p = image::Rgb([80; 3]);
            }
        }
        let (q, detail) = recover(&photo, &base, &cell, writing);
        let b = Bounds::of(&writing.project(&q));
        let initial = Bounds::of(&writing.project(&base));
        // In this short rotated cell the existing ruling detector cannot
        // establish the line. Its boundary-connected component must not grow.
        if ruled && angle == 18. {
            assert_eq!(detail["changed"], false);
            assert!((b.l - initial.l).abs() < 0.001 && (b.r - initial.r).abs() < 0.001);
            continue;
        }
        assert!(b.l < initial.l - 15., "{detail}");
        assert!(
            b.t < initial.t - 10. && b.t > initial.t - 20.,
            "must follow cap, not neighbor: {detail}"
        );
        assert!(b.r <= initial.r + 0.001, "must not follow ruling: {detail}");
        assert!(
            b.l >= Bounds::of(&writing.project(&cell)).l,
            "label gutter excluded"
        );
        let (lean, detail) = recover_traced(&photo, &base, &cell, writing, false);
        assert_eq!(lean, q);
        assert!(detail.is_null());
    }
}
