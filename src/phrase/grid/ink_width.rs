//! Remove side whitespace before recognition; returned and read boxes agree.
use super::*;

// Original grid-stage tightening stays unchanged before repair/extent decisions.
impl Grid {
    pub(in crate::phrase) fn ink_width(
        &self,
        photo: &RgbImage,
        q: &Quad,
        writing: WritingFrame,
    ) -> Option<(Quad, Vec<serde_json::Value>)> {
        self.assigned_column(q, writing)?;
        let projected = writing.project(q);
        let b = Bounds::of(&projected);
        // Only shrink rectangles already expressed in the common writing
        // frame; do not replace a differently angled polygon with its AABB.
        if projected.0.iter().any(|&(x, y)| {
            (x - b.l).abs().min((x - b.r).abs()) > 0.001
                || (y - b.t).abs().min((y - b.b).abs()) > 0.001
        }) {
            return None;
        }
        let frame = writing.crop_frame(q, 0.);
        let crop = level_crop_in(photo, frame);
        let mut ink = source_ink(photo, q, frame, &crop);
        strip_rules(&mut ink);
        let ruling = super::ruling::strip_context_rules(photo, frame, &mut ink);
        let mut lo = usize::MAX;
        let mut hi = 0;
        for row in ink {
            for (x, dark) in row.into_iter().enumerate() {
                if dark {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        if lo == usize::MAX {
            return None;
        }
        // level_crop_in samples a rounded-size pixel grid at unit spacing.
        // A sampled centre can be displaced from its source pixel centre by
        // half a pixel on each photo axis (nearest-neighbour sampling). Keep
        // that half-diagonal uncertainty when mapping back to photo geometry.
        let origin = (b.l + b.r - crop.width() as f32) / 2.;
        let guard = std::f32::consts::FRAC_1_SQRT_2;
        let l = (origin + lo as f32 - guard).max(b.l);
        let r = (origin + hi as f32 + 1. + guard).min(b.r);
        if r <= l || (l - b.l < 0.001 && b.r - r < 0.001) {
            return None;
        }
        Some((writing.unproject(&Bounds { l, r, ..b }.quad()), ruling))
    }
}

/// Small components affect neither horizontal extreme. This does not erase
/// their source pixels or use model probabilities to decide which ink matters.
pub(super) const SMALL_PIXELS: usize = 20;

struct Components {
    ids: Vec<usize>,
    sizes: Vec<usize>,
    bounds: Vec<[usize; 4]>, // exclusive right/bottom, indexed by component ID
}

fn label_components(ink: &[Vec<bool>]) -> Components {
    let h = ink.len();
    let w = ink.first().map_or(0, Vec::len);
    let mut ids = vec![0usize; w * h];
    let mut sizes = vec![0usize];
    let mut bounds = vec![[0; 4]];
    for y in 0..h {
        for x in 0..w {
            if !ink[y][x] || ids[y * w + x] != 0 {
                continue;
            }
            let id = sizes.len();
            ids[y * w + x] = id;
            let mut stack = vec![(x, y)];
            let (mut count, mut lo, mut hi) = (0, x, x);
            let (mut top, mut bottom) = (y, y);
            while let Some((xx, yy)) = stack.pop() {
                count += 1;
                lo = lo.min(xx);
                hi = hi.max(xx);
                top = top.min(yy);
                bottom = bottom.max(yy);
                for ny in yy.saturating_sub(1)..=(yy + 1).min(h - 1) {
                    for nx in xx.saturating_sub(1)..=(xx + 1).min(w - 1) {
                        if ink[ny][nx] && ids[ny * w + nx] == 0 {
                            ids[ny * w + nx] = id;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            sizes.push(count);
            bounds.push([lo, top, hi + 1, bottom + 1]);
        }
    }
    Components { ids, sizes, bounds }
}

/// Only the final extent measurement may ignore detached, short leading marks.
/// Do not reuse this in prefix discovery: punctuation is useful label evidence.
/// Connected strokes and components whose x projections overlap cannot qualify.
fn leading_marks(c: &Components, boundary_connected: &[usize]) -> Vec<serde_json::Value> {
    let mut ordered: Vec<_> = (1..c.sizes.len())
        .filter(|&i| c.sizes[i] > SMALL_PIXELS)
        .collect();
    ordered.sort_by_key(|&i| c.bounds[i][0]);
    let glyph_height = ordered
        .iter()
        .map(|&i| c.bounds[i][3] - c.bounds[i][1])
        .max()
        .unwrap_or(0);
    let mut ignored = Vec::new();
    for (index, &id) in ordered.iter().enumerate() {
        let [left, top, right, bottom] = c.bounds[id];
        let Some(next_left) = ordered[index + 1..].iter().map(|&i| c.bounds[i][0]).min() else {
            break;
        };
        let width = right - left;
        let height = bottom - top;
        let gap = next_left.saturating_sub(right);
        if width * 4 > glyph_height || height * 4 > glyph_height || gap < width {
            break;
        }
        if boundary_connected.contains(&id) {
            break; // A clipped stroke is not an isolated mark.
        }
        ignored.push(
            json!({"component":id,"reason":"isolated_short_leading_mark",
            "bounds":[left,top,right,bottom],"pixels":c.sizes[id],"gap":gap,
            "glyph_height":glyph_height,"max_dimension_fraction":0.25,"min_gap_width_ratio":1.0}),
        );
    }
    ignored
}

/// Reuse final shrinking's component definition for a proposal-only mask.
/// Source pixels and recognition crops are never modified by this operation.
pub(super) fn filter_small(ink: &mut [Vec<bool>]) -> (usize, usize) {
    let Components { ids, sizes, .. } = label_components(ink);
    let mut removed_pixels = 0;
    for (pixel, id) in ink.iter_mut().flatten().zip(ids) {
        if id != 0 && sizes[id] <= SMALL_PIXELS {
            *pixel = false;
            removed_pixels += 1;
        }
    }
    (
        sizes.iter().skip(1).filter(|&&n| n <= SMALL_PIXELS).count(),
        removed_pixels,
    )
}

#[cfg(test)]
fn components(ink: &[Vec<bool>], trace: bool) -> (Option<(usize, usize)>, serde_json::Value) {
    component_extents(ink, trace, |_, _| Some(false))
}

fn component_extents(
    ink: &[Vec<bool>],
    trace: bool,
    outside_ink: impl Fn(isize, isize) -> Option<bool>,
) -> (Option<(usize, usize)>, serde_json::Value) {
    let h = ink.len();
    let w = ink.first().map_or(0, Vec::len);
    let labelled = label_components(ink);
    // A cropped letter can look like a dot. Follow each proposed mark into
    // source context. A tiny visible remnant may belong to a smaller printed
    // label, but must not belong to a full-height word stroke. Allow its source
    // component up to 3/4 glyph height in either dimension, leaving a 25% size
    // separation from the word. Joining other word ink always vetoes removal.
    let mut boundary_connected = Vec::new();
    let mut context_checks = Vec::new();
    for proposal in leading_marks(&labelled, &[]) {
        let id = proposal["component"].as_u64().unwrap() as usize;
        let glyph_height = proposal["glyph_height"].as_u64().unwrap() as isize;
        let [l, t, r, b] = labelled.bounds[id];
        let mut full = [l as isize, t as isize, r as isize, b as isize];
        let mut stack = Vec::new();
        for y in t..b {
            for x in l..r {
                if labelled.ids[y * w + x] == id && (x == 0 || x + 1 == w || y == 0 || y + 1 == h) {
                    for yy in y as isize - 1..=y as isize + 1 {
                        for xx in x as isize - 1..=x as isize + 1 {
                            if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                                stack.push((xx, yy));
                            }
                        }
                    }
                }
            }
        }
        let mut visited = std::collections::HashSet::new();
        let mut reason = "isolated_in_source_context";
        'flood: while let Some((x, y)) = stack.pop() {
            if !visited.insert((x, y)) {
                continue;
            }
            match outside_ink(x, y) {
                Some(false) => continue,
                None => {
                    reason = "unknown_source_context";
                    break;
                }
                Some(true) => {}
            }
            full = [
                full[0].min(x),
                full[1].min(y),
                full[2].max(x + 1),
                full[3].max(y + 1),
            ];
            if (full[2] - full[0]) * 4 > 3 * glyph_height
                || (full[3] - full[1]) * 4 > 3 * glyph_height
            {
                reason = "continues_as_larger_source_component";
                break;
            }
            for yy in y - 1..=y + 1 {
                for xx in x - 1..=x + 1 {
                    if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                        stack.push((xx, yy));
                    } else {
                        let other = labelled.ids[yy as usize * w + xx as usize];
                        if other != 0 && other != id {
                            reason = "joins_other_component_outside_box";
                            break 'flood;
                        }
                    }
                }
            }
        }
        let next_left = r as isize + proposal["gap"].as_u64().unwrap() as isize;
        let source_gap = next_left - full[2];
        if reason == "isolated_in_source_context" && source_gap < (r - l) as isize {
            reason = "source_component_too_close_to_word";
        }
        if reason != "isolated_in_source_context" {
            boundary_connected.push(id);
        }
        context_checks.push(json!({"component":id,"reason":reason,"bounds":full,
            "outside_samples":visited.len(),"glyph_height":glyph_height,"source_gap":source_gap,
            "max_source_dimension_fraction":0.75}));
    }
    let leading = leading_marks(&labelled, &boundary_connected);
    let ignored: Vec<_> = leading
        .iter()
        .map(|v| v["component"].as_u64().unwrap() as usize)
        .collect();
    let limits = (1..labelled.sizes.len())
        .filter(|i| labelled.sizes[*i] > SMALL_PIXELS && !ignored.contains(i))
        .map(|i| (labelled.bounds[i][0], labelled.bounds[i][2] - 1))
        .reduce(|(l, r), (lo, hi)| (l.min(lo), r.max(hi)));
    let Components { ids, sizes, .. } = labelled;
    let mut runs = Vec::new();
    if trace {
        for y in 0..h {
            let mut x = 0;
            while x < w {
                let id = ids[y * w + x];
                if id == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < w && ids[y * w + x] == id {
                    x += 1;
                }
                runs.push([y, start, x - start, id]);
            }
        }
    }
    (
        limits,
        if trace {
            json!({"size":[w,h],"small_cutoff":SMALL_PIXELS,
        "connectivity":8,"component_sizes":sizes,"runs":runs,
        "ignored_leading_components":ignored,"leading_component_decisions":leading,
        "boundary_protected_components":boundary_connected,"leading_context_checks":context_checks,
        "boundary_policy":"follow_source_component_until_small_dimensions_exceeded; unknown_context_retains",
        "run_encoding":"y,x,length,component_id; id 0 is background",
        "extents":limits,"mask":"source_valid_otsu_after_ruling_removal"})
        } else {
            serde_json::Value::Null
        },
    )
}

/// Final word tightening does not require a grid. Preserve a rectangle's own
/// skew; choose its edge closest to the shared writing axis without changing
/// the reader's page direction. Unsupported nonrectangles stay unchanged.
pub(in crate::phrase) fn shrink(
    photo: &RgbImage,
    q: &Quad,
    writing: WritingFrame,
    trace: bool,
) -> (Quad, Option<serde_json::Value>) {
    let refuse = |reason| {
        (
            q.clone(),
            trace.then(|| {
                json!({"rule":"box_ink_width",
            "status":"unchanged","reason":reason,"input_quad":q.0,"final_quad":q.0})
            }),
        )
    };
    let mut frame = crate::split::frame_of(q);
    if (frame.angle - writing.angle_or(frame.angle))
        .to_radians()
        .cos()
        .abs()
        < std::f32::consts::FRAC_1_SQRT_2
    {
        std::mem::swap(&mut frame.w, &mut frame.h);
        frame.angle += 90.;
    }
    if crate::read_budget::pixels_ceil(frame).is_err() {
        return refuse("invalid_or_oversized_frame");
    }
    let projected = Quad(q.0.map(|(x, y)| turn(x - frame.cx, y - frame.cy, -frame.angle)));
    let b = Bounds::of(&projected);
    if projected.0.iter().any(|&(x, y)| {
        (x - b.l).abs().min((x - b.r).abs()) > 0.01 || (y - b.t).abs().min((y - b.b).abs()) > 0.01
    }) {
        return refuse("nonrectangular_quad");
    }
    let crop = level_crop_in(photo, frame);
    let (mut ink, threshold) = source_ink_with_level(photo, q, frame, &crop);
    strip_rules(&mut ink);
    let ruling = super::ruling::strip_context_rules(photo, frame, &mut ink);
    let (extents, mask) = component_extents(&ink, trace, |x, y| {
        let (dx, dy) = turn(
            x as f32 + 0.5 - crop.width() as f32 / 2.,
            y as f32 + 0.5 - crop.height() as f32 / 2.,
            frame.angle,
        );
        let (px, py) = (frame.cx + dx, frame.cy + dy);
        if px < 0. || py < 0. || px >= photo.width() as f32 || py >= photo.height() as f32 {
            return None;
        }
        let p = photo.get_pixel(px as u32, py as u32).0;
        let gray = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).round() as u8;
        threshold.map(|level| gray <= level)
    });
    // level_crop_in samples a rounded-size pixel grid at unit spacing.
    // A sampled centre can be displaced from its source pixel centre by
    // half a pixel on each photo axis (nearest-neighbour sampling). Keep
    // that half-diagonal uncertainty when mapping back to photo geometry.
    let origin = (b.l + b.r - crop.width() as f32) / 2.;
    let guard = std::f32::consts::FRAC_1_SQRT_2;
    let usable = extents;
    let (l, r) = usable.map_or((b.l, b.r), |(lo, hi)| {
        (
            (origin + lo as f32 - guard).max(b.l),
            (origin + hi as f32 + 1. + guard).min(b.r),
        )
    });
    let changed = r > l && (l - b.l >= 0.001 || b.r - r >= 0.001);
    let result = if changed {
        Quad(Bounds { l, r, ..b }.quad().0.map(|(x, y)| {
            let (x, y) = turn(x, y, frame.angle);
            (x + frame.cx, y + frame.cy)
        }))
    } else {
        q.clone()
    };
    let event = trace.then(|| json!({"rule":"box_ink_width","status":if changed {"shrunk"} else {"unchanged"},
            "reason":if extents.is_none() {"no_large_component_retain_box"} else {"component_extents"},
            "input_quad":q.0,"final_quad":result.0,"before_recognition":true,"height_unchanged":true,
            "frame":{"cx":frame.cx,"cy":frame.cy,"angle_degrees":frame.angle},
            "mask":mask,"threshold":threshold,"ruling_bands":ruling,"source_pixel_guard":guard}));
    (result, event)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block(ink: &mut [Vec<bool>], l: usize, t: usize, r: usize, b: usize) {
        for row in &mut ink[t..b] {
            row[l..r].fill(true);
        }
    }

    #[test]
    fn detached_leading_marks_do_not_anchor_final_width_or_change_prefix_mask() {
        for scale in [1, 2, 4] {
            let mut ink = vec![vec![false; 90 * scale]; 50 * scale];
            block(&mut ink, 0, 20 * scale, 5 * scale, 27 * scale);
            block(&mut ink, 15 * scale, 20 * scale, 22 * scale, 26 * scale);
            block(&mut ink, 33 * scale, 3 * scale, 65 * scale, 45 * scale);
            let original = ink.clone();
            let (extents, trace) = components(&ink, true);
            assert_eq!(extents, Some((33 * scale, 65 * scale - 1)));
            assert_eq!(
                trace["ignored_leading_components"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(components(&ink, false).0, extents);
            assert_eq!(ink, original);
            assert_eq!(filter_small(&mut ink), (0, 0));
            assert_eq!(ink, original); // Leading punctuation is still useful for labels.
        }
    }

    #[test]
    fn connected_short_stroke_dot_over_stem_and_tall_narrow_letter_survive() {
        for case in 0..3 {
            let mut ink = vec![vec![false; 90]; 50];
            block(&mut ink, 33, 3, 65, 45);
            match case {
                0 => {
                    // A short stroke connected to the C/word, not a speck.
                    block(&mut ink, 5, 20, 34, 27);
                }
                1 => {
                    // Dot and stem overlap in x, even if disconnected in y.
                    block(&mut ink, 5, 0, 10, 7);
                    block(&mut ink, 6, 14, 9, 43);
                }
                _ => block(&mut ink, 5, 3, 7, 45), // Narrow tall letter.
            }
            assert_eq!(components(&ink, false).0, Some((5, 64)));
        }
    }

    #[test]
    fn short_letter_with_close_neighbour_and_all_short_crop_survive() {
        let mut ink = vec![vec![false; 90]; 50];
        block(&mut ink, 5, 20, 10, 27);
        block(&mut ink, 12, 3, 45, 45); // Gap2 < mark width5.
        assert_eq!(components(&ink, false).0, Some((5, 44)));
        let mut short = vec![vec![false; 50]; 50];
        block(&mut short, 5, 20, 10, 27);
        block(&mut short, 30, 20, 35, 27);
        assert_eq!(components(&short, false).0, Some((5, 34)));
    }

    #[test]
    fn cropped_stroke_is_not_isolated_when_it_continues_outside_the_box() {
        let mut ink = vec![vec![false; 90]; 50];
        block(&mut ink, 0, 20, 5, 27);
        block(&mut ink, 33, 3, 65, 45);
        assert_eq!(components(&ink, false).0, Some((33, 64)));
        for context in [Some(true), None] {
            let (extent, trace) = component_extents(&ink, true, |_, _| context);
            assert_eq!(extent, Some((0, 64)));
            assert!(
                !trace["boundary_protected_components"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        // A diagonal connection outside the top also protects a clipped glyph.
        let mut top = vec![vec![false; 90]; 50];
        block(&mut top, 5, 0, 10, 7);
        block(&mut top, 33, 3, 65, 45);
        assert_eq!(
            component_extents(&top, false, |x, y| Some(x == 4 && y < 0)).0,
            Some((5, 64))
        );
        // One extra pixel outside is still a genuinely small, isolated mark.
        assert_eq!(
            component_extents(&top, false, |x, y| Some(x == 4 && y == -1)).0,
            Some((33, 64))
        );
    }

    #[test]
    fn small_printed_label_remnant_can_be_removed_but_full_size_letter_cannot() {
        let mut ink = vec![vec![false; 90]; 50];
        block(&mut ink, 0, 20, 5, 27);
        block(&mut ink, 33, 3, 65, 45); // Glyph height42.
        // A clipped 25x24 label still has a tiny isolated remnant in this box.
        let (extent, trace) = component_extents(&ink, true, |x, y| {
            Some((-20..0).contains(&x) && (7..31).contains(&y))
        });
        assert_eq!(extent, Some((33, 64)));
        assert_eq!(
            trace["leading_context_checks"][0]["bounds"],
            json!([-20, 7, 5, 31])
        );
        // A word-sized clipped C/f stays, even if disconnected from other letters.
        assert_eq!(
            component_extents(&ink, false, |x, y| Some(
                (-20..0).contains(&x) && (3..45).contains(&y)
            ))
            .0,
            Some((0, 64))
        );
    }

    #[test]
    fn proposal_filter_shares_exact_cutoff_and_diagonal_connectivity() {
        let mut ink = vec![vec![false; 60]; 30];
        for (y, row) in ink.iter_mut().enumerate().take(21) {
            row[y] = true; // 21 diagonally connected pixels survive.
            if y < 20 {
                row[40] = true;
            }
        }
        let (extents, _) = components(&ink, false);
        assert_eq!(extents, Some((0, 20)));
        assert_eq!(filter_small(&mut ink), (1, 20));
        assert_eq!(ink.iter().flatten().filter(|&&v| v).count(), 21);
        assert_eq!(components(&ink, false).0, extents);
        assert_eq!(filter_small(&mut []), (0, 0));
        assert_eq!(filter_small(&mut vec![vec![false; 3]; 2]), (0, 0));
    }

    #[test]
    fn small_components_are_only_excluded_from_extent_measurement() {
        let mut ink = vec![vec![false; 70]; 30];
        for row in ink.iter_mut().take(22).skip(1) {
            row[30] = true;
        }
        for row in ink.iter_mut().take(21).skip(1) {
            row[5] = true;
        }
        ink[0][60] = true;
        let (extents, trace) = components(&ink, true);
        assert_eq!(extents, Some((30, 30))); // 21 survives, 20 does not.
        assert_eq!(trace["component_sizes"], json!([0, 1, 20, 21]));
        assert!(ink[0][60] && ink[1][5]); // The raster was not changed.
        let mut restored = vec![vec![false; 70]; 30];
        for run in trace["runs"].as_array().unwrap() {
            let y = run[0].as_u64().unwrap() as usize;
            let x = run[1].as_u64().unwrap() as usize;
            let n = run[2].as_u64().unwrap() as usize;
            restored[y][x..x + n].fill(true);
        }
        assert_eq!(restored, ink);
        assert!(components(&vec![vec![false; 10]; 10], true).0.is_none());
        assert!(components(&[], false).0.is_none());
    }

    #[test]
    fn diagonal_strokes_are_eight_connected_and_dot_above_stem_does_not_shrink_width() {
        let mut ink = vec![vec![false; 40]; 40];
        for (y, row) in ink.iter_mut().enumerate().take(30).skip(5) {
            row[y] = true;
        }
        ink[0][12] = true;
        assert_eq!(components(&ink, false).0, Some((5, 29)));
    }

    #[test]
    fn no_grid_tightens_speckled_boxes_but_retains_all_small_and_preserves_pixels() {
        let q = Quad([(10., 10.), (110., 10.), (110., 70.), (10., 70.)]);
        let mut photo = RgbImage::from_pixel(130, 90, image::Rgb([240; 3]));
        photo.put_pixel(11, 12, image::Rgb([20; 3]));
        assert_eq!(shrink(&photo, &q, WritingFrame::Local, true).0, q);
        for y in 20..60 {
            for x in 40..70 {
                photo.put_pixel(x, y, image::Rgb([100; 3]));
            }
        }
        let original = photo.clone();
        let (tight, trace) = shrink(&photo, &q, WritingFrame::Local, true);
        let b = Bounds::of(&tight);
        assert!(b.l > 38. && b.l < 40. && b.r > 70. && b.r < 72.);
        assert_eq!((b.t, b.b), (10., 70.));
        assert_eq!(photo, original);
        assert_eq!(trace.unwrap()["status"], "shrunk");
    }

    #[test]
    fn crossing_rule_does_not_remove_connected_letter_stems() {
        let q = Quad([(10., 10.), (210., 10.), (210., 100.), (10., 100.)]);
        let mut photo = RgbImage::from_pixel(230, 120, image::Rgb([240; 3]));
        for x in 0..230 {
            photo.put_pixel(x, 60, image::Rgb([20; 3]));
        }
        for y in 25..85 {
            for x in [45, 46, 47, 130, 131, 132] {
                photo.put_pixel(x, y, image::Rgb([20; 3]));
            }
        }
        let (tight, _) = shrink(&photo, &q, WritingFrame::Local, true);
        let b = Bounds::of(&tight);
        assert!(b.l <= 45. && b.r >= 133.);
    }

    #[test]
    fn side_component_is_never_cut_from_the_final_box() {
        let q = Quad([(10., 10.), (110., 10.), (110., 70.), (10., 70.)]);
        for edge in [10, 109] {
            let mut photo = RgbImage::from_pixel(130, 90, image::Rgb([240; 3]));
            for y in 20..60 {
                photo.put_pixel(edge, y, image::Rgb([20; 3]));
            }
            let (out, event) = shrink(&photo, &q, WritingFrame::Local, true);
            let b = Bounds::of(&out);
            assert!(b.l <= edge as f32 && b.r >= (edge + 1) as f32);
            assert_eq!(event.unwrap()["reason"], "component_extents");
        }
    }
    #[test]
    fn trims_sides_preserving_height_and_all_ink_in_rotated_frames() {
        for angle in [0., 20.] {
            let map = |x, y| {
                let (x, y) = turn(x, y, angle);
                (x + 100., y + 100.)
            };
            let q = Quad([(0., 0.), (150., 0.), (150., 80.), (0., 80.)].map(|(x, y)| map(x, y)));
            let writing = WritingFrame::Page(
                crate::page_frame::PageAxis::from_detections(&[q.clone()]).direction(false),
            );
            let b = Bounds::of(&writing.project(&q));
            let grid = Grid {
                columns: vec![ColumnBounds {
                    right_slope: 0.,
                    intercept: b.l,
                    slope: 0.,
                    right: b.r,
                    top: b.t,
                    bottom: b.b,
                }],
                ..Grid::default()
            };
            let mut photo = RgbImage::from_pixel(350, 300, image::Rgb([240; 3]));
            for y in 0..80 {
                for x in 30..120 {
                    if ((10..50).contains(&y) && x % 20 < 8)
                        || (y >= 50 && (108..116).contains(&x))
                        || (y < 10 && (32..40).contains(&x))
                    {
                        let (x, y) = map(x as f32, y as f32);
                        photo.put_pixel(x.round() as u32, y.round() as u32, image::Rgb([20; 3]));
                    }
                }
            }
            let (result, _) = shrink(&photo, &q, writing, false);
            let tight = Bounds::of(&writing.project(&result));
            assert!(tight.l > b.l + 20. && tight.r < b.r - 20.);
            assert!((tight.t - b.t).abs() < 0.001 && (tight.b - b.b).abs() < 0.001);
            let f = writing.crop_frame(&q, 0.);
            let crop = level_crop_in(&photo, f);
            let ink = source_ink(&photo, &q, f, &crop);
            for row in ink {
                for (x, dark) in row.into_iter().enumerate() {
                    if dark {
                        let x = (b.l + b.r - crop.width() as f32) / 2. + x as f32 + 0.5;
                        assert!(x >= tight.l - 0.001 && x <= tight.r + 0.001);
                    }
                }
            }
            // Independently check original photo pixel centres, not only the
            // rotated mask against which the bounds were fitted.
            for (x, y, pixel) in photo.enumerate_pixels() {
                if pixel.0[0] < 80 {
                    let p = (x as f32 + 0.5, y as f32 + 0.5);
                    let (x, y) = writing.project(&Quad([p; 4])).0[0];
                    if x >= b.l && x <= b.r && y >= b.t && y <= b.b {
                        assert!(x >= tight.l - 0.001 && x <= tight.r + 0.001);
                    }
                }
            }
            assert!(
                shrink(
                    &RgbImage::from_pixel(350, 300, image::Rgb([240; 3])),
                    &q,
                    writing,
                    false
                )
                .0 == q
            );
            let _ = grid; // The same geometry is tightened without a column.
        }
    }
}
