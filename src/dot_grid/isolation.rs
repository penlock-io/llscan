//! Distinguish a compact isolated mark from a peak connected to letter ink.
use crate::split::turn;
use image::GrayImage;
use serde_json::{Value, json};

pub fn assess(mask: &GrayImage, angle: f32, scale: f32, x: f32, y: f32, height: f32) -> Value {
    let h = height * scale;
    let rx = h.ceil() as i32;
    let ry = (0.6 * h).ceil() as i32;
    let w = (2 * rx + 1) as usize;
    let n = (2 * ry + 1) as usize;
    let mut ink = vec![vec![false; w]; n];
    for (j, row) in ink.iter_mut().enumerate() {
        for (i, v) in row.iter_mut().enumerate() {
            let (px, py) = turn(
                x + (i as i32 - rx) as f32 / scale,
                y + (j as i32 - ry) as f32 / scale,
                angle,
            );
            let (px, py) = ((px * scale).round() as i32, (py * scale).round() as i32);
            if px < 0 || py < 0 || px >= mask.width() as i32 || py >= mask.height() as i32 {
                return json!({"isolated":false,"reason":"insufficient_image_context"});
            }
            *v = mask.get_pixel(px as u32, py as u32)[0] == 0;
        }
    }
    classify(ink, h, rx as usize, ry as usize)
}

fn classify(mut ink: Vec<Vec<bool>>, h: f32, cx: usize, cy: usize) -> Value {
    let n = ink.len();
    let w = ink[0].len();
    // A long, thin band may be a ruling. A thick letter stroke must not be
    // erased just to manufacture isolation. This changes the probe mask only.
    let rows: Vec<_> = ink
        .iter()
        .map(|r| r.iter().filter(|&&p| p).count() as f32 >= 0.8 * w as f32)
        .collect();
    let mut j = 0;
    let mut removed = 0;
    while j < n {
        if !rows[j] {
            j += 1;
            continue;
        }
        let start = j;
        while j < n && rows[j] {
            j += 1;
        }
        if (j - start) as f32 <= (0.055 * h).max(1.) {
            for row in &mut ink[start..j] {
                row.fill(false);
                removed += 1;
            }
        }
    }
    let r = (0.1 * h).ceil() as usize;
    let mut seed = None;
    for (j, row) in ink
        .iter()
        .enumerate()
        .take((cy + r + 1).min(n))
        .skip(cy.saturating_sub(r))
    {
        for (i, &v) in row
            .iter()
            .enumerate()
            .take((cx + r + 1).min(w))
            .skip(cx.saturating_sub(r))
        {
            if v {
                let distance = i.abs_diff(cx).pow(2) + j.abs_diff(cy).pow(2);
                if seed.is_none_or(|(_, _, d)| distance < d) {
                    seed = Some((i, j, distance));
                }
            }
        }
    }
    let Some((sx, sy, _)) = seed else {
        return json!({"isolated":false,"reason":"no_ink_after_thin_rule_removal","removed_rule_rows":removed});
    };
    let mut stack = vec![(sx, sy)];
    ink[sy][sx] = false;
    let (mut left, mut right, mut top, mut bottom) = (sx, sx, sy, sy);
    let mut count = 0;
    while let Some((x, y)) = stack.pop() {
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
        count += 1;
        for yy in y.saturating_sub(1)..=(y + 1).min(n - 1) {
            for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                if ink[yy][xx] {
                    ink[yy][xx] = false;
                    stack.push((xx, yy));
                }
            }
        }
    }
    let width = (right - left + 1) as f32;
    let height = (bottom - top + 1) as f32;
    let reason = if left == 0 || right == w - 1 || top == 0 || bottom == n - 1 {
        "connected_to_window_edge"
    } else if width > 0.4 * h || height > 0.4 * h {
        "connected_to_large_ink"
    } else if width < 0.035 * h || height < 0.035 * h || count < 3 {
        "insufficient_compact_ink"
    } else if !(0.35..=3.).contains(&(width / height)) || count as f32 / (width * height) < 0.3 {
        "not_compact"
    } else {
        "isolated_compact_mark"
    };
    json!({"isolated":reason=="isolated_compact_mark","reason":reason,"removed_rule_rows":removed,"component_pixels":count,"component_size_in_word_heights":[width/h,height/h]})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn canvas() -> Vec<Vec<bool>> {
        vec![vec![false; 201]; 121]
    }
    fn dot(m: &mut [Vec<bool>]) {
        for row in &mut m[56..65] {
            row[96..105].fill(true);
        }
    }
    #[test]
    fn isolated_and_ruled_dots_survive_but_letter_connections_do_not() {
        let mut m = canvas();
        dot(&mut m);
        assert_eq!(classify(m.clone(), 100., 100, 60)["isolated"], true);
        for row in &mut m[62..65] {
            row.fill(true);
        }
        assert_eq!(classify(m.clone(), 100., 100, 60)["isolated"], true);
        // The same compact base connected to a letter stem is not a dot.
        for row in &mut m[5..60] {
            row[99..105].fill(true);
        }
        assert_eq!(classify(m, 100., 100, 60)["isolated"], false);
    }
    #[test]
    fn thick_bands_and_cropped_context_are_not_background() {
        let mut m = canvas();
        for row in &mut m[55..67] {
            row.fill(true);
        }
        assert_eq!(classify(m, 100., 100, 60)["isolated"], false);
        let image = GrayImage::from_pixel(200, 200, image::Luma([255]));
        assert_eq!(
            assess(&image, 0., 1., 0., 0., 100.)["reason"],
            "insufficient_image_context"
        );
    }
}
