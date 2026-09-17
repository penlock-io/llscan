//! Cutting a detector's polygon into its words: the survey's common
//! splitter (`detect_split.py`), with its locked gap/rule constants.
//! A polygon is worked in its own frame: the photo around it is turned
//! level by the angle of its minimum rotated
//! rectangle, everything outside the polygon is paper, and only valid
//! photo samples contribute to Otsu's threshold. Long horizontal runs (ruled
//! lines) are removed, and the column profile is cut where a gap exceeds
//! both [`Split::factor`] times the median gap between ink components
//! and [`Split::gap`] of the line height, so a lone word is not cut at
//! its widest letter gap; a small detached mark within [`Split::rejoin`]
//! heights of its neighbour joins it, each word keeps the line's height
//! plus [`Split::pad`] of it on either side, and the words are turned
//! back into the photo. Generic pages register gap measurement to the region's
//! edge nearest the page direction, then enclose those spans in page-aligned
//! output boxes. Measurement never chooses a separate OCR orientation.

use image::{GrayImage, Luma, RgbImage};
use imageproc::contrast::otsu_level;
use imageproc::geometric_transformations::{
    Border, Interpolation, Projection, rotate_about_center,
};
use imageproc::region_labelling::{Connectivity, connected_components};

use crate::detect::{Quad, clockwise};

/// The splitter's constants, locked by the word-detection plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    /// A gap splits words when it exceeds this times the median gap.
    pub factor: f32,
    /// And this fraction of the line height.
    pub gap: f32,
    /// A small mark within this many heights of a neighbour rejoins it.
    pub rejoin: f32,
    /// Each word keeps this fraction of the height on either side.
    pub pad: f32,
}

impl Split {
    /// The constants the plan locked on the sealed pages.
    pub const LOCKED: Split = Split {
        factor: 3.0,
        gap: 0.35,
        rejoin: 0.3,
        pad: 0.0,
    };
}

/// Ink runs across at least this share of a row are ruled lines.
const RULE_RUN: f32 = 0.5;

/// Detached marks are small in both dimensions relative to the line height.
pub(crate) const SMALL_INK_RATIO: f32 = 0.4;

/// A polygon's level frame: centre, size and angle in degrees, the
/// angle counting anticlockwise on the page as PIL turns an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Centre in photo pixels.
    pub cx: f32,
    /// Centre in photo pixels.
    pub cy: f32,
    /// Length along the writing direction.
    pub w: f32,
    /// Height across it.
    pub h: f32,
    /// Degrees, anticlockwise on the page.
    pub angle: f32,
}

/// The frame of a quad: its minimum rotated rectangle, the longer side
/// taken as the writing direction, the angle folded into (-90, 90].
pub fn frame_of(quad: &Quad) -> Frame {
    let pts: Vec<imageproc::point::Point<f32>> = quad
        .0
        .iter()
        .map(|&(x, y)| imageproc::point::Point::new(x, y))
        .collect();
    let rect = crate::detect::min_area_rect_of(&pts);
    let e1 = (rect[1].x - rect[0].x, rect[1].y - rect[0].y);
    let e2 = (rect[2].x - rect[1].x, rect[2].y - rect[1].y);
    let len = |e: (f32, f32)| (e.0 * e.0 + e.1 * e.1).sqrt();
    let (long, short) = if len(e1) >= len(e2) {
        (e1, e2)
    } else {
        (e2, e1)
    };
    let mut angle = (-long.1).atan2(long.0).to_degrees();
    if angle > 90.0 {
        angle -= 180.0;
    } else if angle <= -90.0 {
        angle += 180.0;
    }
    let cx = rect.iter().map(|p| p.x).sum::<f32>() / 4.0;
    let cy = rect.iter().map(|p| p.y).sum::<f32>() / 4.0;
    Frame {
        cx,
        cy,
        w: len(long),
        h: len(short),
        angle,
    }
}

/// (dx, dy) turned by `angle` degrees as PIL turns an image:
/// anticlockwise on the page, y pointing down.
pub fn turn(dx: f32, dy: f32, angle: f32) -> (f32, f32) {
    let (s, c) = angle.to_radians().sin_cos();
    (dx * c + dy * s, -dx * s + dy * c)
}

/// Otsu's threshold on the crop's own histogram, the darker side as
/// ink; a flat crop has none.
pub fn binarise(grey: &GrayImage) -> Vec<Vec<bool>> {
    let (w, h) = grey.dimensions();
    let (lo, hi) = grey
        .pixels()
        .fold((255u8, 0u8), |(lo, hi), p| (lo.min(p.0[0]), hi.max(p.0[0])));
    if w == 0 || h == 0 || lo == hi {
        return vec![vec![false; w as usize]; h as usize];
    }
    let level = otsu_level(grey);
    (0..h)
        .map(|y| (0..w).map(|x| grey.get_pixel(x, y).0[0] <= level).collect())
        .collect()
}

/// Padding is neither ink nor a histogram observation. Keep real white
/// pixels: membership comes from geometry, never from a pixel's intensity.
#[cfg(test)]
pub(crate) fn binarise_valid(grey: &GrayImage, valid: Vec<Vec<bool>>) -> Vec<Vec<bool>> {
    binarise_valid_accounted(grey, valid).pixels
}

pub(crate) fn binarise_valid_with_level(
    grey: &GrayImage,
    valid: Vec<Vec<bool>>,
) -> (Vec<Vec<bool>>, Option<u8>) {
    let mask = binarise_valid_accounted(grey, valid);
    (mask.pixels, mask.level)
}

struct InkMask {
    pixels: Vec<Vec<bool>>,
    empty: Option<EmptySplit>,
    level: Option<u8>,
}

fn binarise_valid_accounted(grey: &GrayImage, mut valid: Vec<Vec<bool>>) -> InkMask {
    assert_eq!(valid.len(), grey.height() as usize);
    assert!(valid.iter().all(|row| row.len() == grey.width() as usize));
    let samples: Vec<u8> = grey
        .pixels()
        .zip(valid.iter().flatten())
        .filter_map(|(p, &keep)| keep.then_some(p.0[0]))
        .collect();
    let flat = samples
        .first()
        .is_none_or(|first| samples.iter().all(|p| p == first));
    if flat {
        for row in &mut valid {
            row.fill(false);
        }
        return InkMask {
            pixels: valid,
            level: None,
            empty: Some(if samples.is_empty() {
                EmptySplit::NoPhotoSamples
            } else {
                EmptySplit::FlatPhotoSamples
            }),
        };
    }
    // Otsu depends only on the histogram, not spatial arrangement. Reuse
    // the existing implementation (including its threshold tie behavior).
    let histogram = GrayImage::from_raw(samples.len() as u32, 1, samples).unwrap();
    let level = otsu_level(&histogram);
    for (p, keep) in grey.pixels().zip(valid.iter_mut().flatten()) {
        *keep &= p.0[0] <= level;
    }
    InkMask {
        pixels: valid,
        empty: None,
        level: Some(level),
    }
}

/// Rows whose ink runs across most of the width are ruled lines, not
/// writing: those pixels are cleared.
pub fn strip_rules(ink: &mut [Vec<bool>]) {
    let Some(width) = ink.first().map(Vec::len) else {
        return;
    };
    let long = (RULE_RUN * width as f32).ceil() as usize;
    for row in ink.iter_mut() {
        if row.iter().filter(|&&v| v).count() < long {
            continue;
        }
        let mut x = 0;
        while x < width {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && row[x] {
                x += 1;
            }
            if x - start >= long {
                for v in &mut row[start..x] {
                    *v = false;
                }
            }
        }
    }
}

/// The binarised, rule-stripped ink inside the quad in its own frame,
/// with the frame; None when the frame is empty. Beyond the photo's
/// edge is paper, and so is everything outside the polygon. Artificial
/// paper is excluded from the threshold histogram as well as the ink.
pub fn ink_of(photo: &RgbImage, quad: &Quad) -> Option<(Frame, Vec<Vec<bool>>)> {
    ink_of_accounted(photo, quad)
        .ok()
        .map(|(frame, ink)| (frame, ink.pixels))
}

fn ink_of_accounted(photo: &RgbImage, quad: &Quad) -> Result<(Frame, InkMask), EmptySplit> {
    ink_in_frame(photo, quad, frame_of(quad))
}

fn ink_in_frame(
    photo: &RgbImage,
    quad: &Quad,
    frame: Frame,
) -> Result<(Frame, InkMask), EmptySplit> {
    if ![frame.cx, frame.cy, frame.w, frame.h, frame.angle]
        .iter()
        .all(|n| n.is_finite())
        || frame.w < 2.0
        || frame.h < 2.0
    {
        return Err(EmptySplit::NoFrame);
    }
    let r = (frame.w * frame.w + frame.h * frame.h).sqrt() / 2.0 + 2.0;
    let (x0, y0) = ((frame.cx - r).floor() as i64, (frame.cy - r).floor() as i64);
    let (x1, y1) = (
        (frame.cx + r).ceil() as i64 + 1,
        (frame.cy + r).ceil() as i64 + 1,
    );
    let (rw, rh) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut region = GrayImage::from_pixel(rw, rh, Luma([255]));
    for y in 0..rh {
        for x in 0..rw {
            let (px, py) = (x0 + x as i64, y0 + y as i64);
            if px >= 0 && py >= 0 && (px as u32) < photo.width() && (py as u32) < photo.height() {
                let p = photo.get_pixel(px as u32, py as u32).0;
                let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                region.put_pixel(x, y, Luma([l.round() as u8]));
            }
        }
    }
    // imageproc turns clockwise by a positive angle; PIL's positive
    // angle is anticlockwise, and the frame counts as PIL does
    let (lcx, lcy) = (frame.cx - x0 as f32, frame.cy - y0 as f32);
    let level = rotate_about_center(
        &region,
        frame.angle.to_radians(),
        Interpolation::Bicubic,
        Border::Constant(Luma([255])),
    );
    let level = recentre(&level, lcx, lcy, rw as f32 / 2.0, rh as f32 / 2.0);
    let (bx0, by0) = (
        (lcx - frame.w / 2.0).round() as i64,
        (lcy - frame.h / 2.0).round() as i64,
    );
    let (bx1, by1) = (
        (lcx + frame.w / 2.0).round() as i64,
        (lcy + frame.h / 2.0).round() as i64,
    );
    let (cw, ch) = ((bx1 - bx0).max(0) as u32, (by1 - by0).max(0) as u32);
    if cw < 2 || ch < 2 {
        return Err(EmptySplit::NoFrame);
    }
    let mut crop = GrayImage::from_pixel(cw, ch, Luma([255]));
    let mut valid = vec![vec![false; cw as usize]; ch as usize];
    let is_photo = photo_sample_map(
        photo.dimensions(),
        (x0, y0),
        (rw, rh),
        (lcx, lcy),
        frame.angle,
    );
    // the polygon in the crop's frame, and paper outside it
    let local: Vec<(f32, f32)> = quad
        .0
        .iter()
        .map(|&(px, py)| {
            let (dx, dy) = turn(px - frame.cx, py - frame.cy, -frame.angle);
            (dx + frame.w / 2.0, dy + frame.h / 2.0)
        })
        .collect();
    for y in 0..ch {
        for x in 0..cw {
            let (lx, ly) = (bx0 + x as i64, by0 + y as i64);
            if lx < 0 || ly < 0 || lx as u32 >= level.width() || ly as u32 >= level.height() {
                continue;
            }
            if inside(&local, x as f32 + 0.5, y as f32 + 0.5) {
                crop.put_pixel(x, y, *level.get_pixel(lx as u32, ly as u32));
                valid[y as usize][x as usize] = is_photo(lx as u32, ly as u32);
            }
        }
    }
    let mut ink = binarise_valid_accounted(&crop, valid);
    strip_rules(&mut ink.pixels);
    if ink.empty.is_none() && !ink.pixels.iter().flatten().any(|&p| p) {
        ink.empty = Some(EmptySplit::NoInkAfterRules);
    }
    Ok((frame, ink))
}

// Mirror imageproc's inverse rotation and our recentre lookup without
// changing grayscale interpolation. The dependency-coupling test below
// checks this against actual imageproc interpolation, including rejection.
fn photo_sample_map(
    (pw, ph): (u32, u32),
    (x0, y0): (i64, i64),
    (rw, rh): (u32, u32),
    (lcx, lcy): (f32, f32),
    angle: f32,
) -> impl Fn(u32, u32) -> bool {
    let (mx, my) = (rw as f32 / 2.0, rh as f32 / 2.0);
    let from_level = (Projection::translate(mx, my)
        * Projection::rotate(angle.to_radians())
        * Projection::translate(-mx, -my))
    .invert();
    move |lx, ly| {
        let Some((sx, sy)) = recentred_pixel(lx, ly, lcx - mx, lcy - my, rw, rh) else {
            return false;
        };
        let (rx, ry) = from_level * (sx as f32, sy as f32);
        let (left, right) = cubic_support(rx);
        let (top, bottom) = cubic_support(ry);
        // Include only samples whose nonzero interpolation support is all photo.
        left >= 0
            && top >= 0
            && right < rw as i64
            && bottom < rh as i64
            && x0 + left >= 0
            && y0 + top >= 0
            && x0 + right < pw as i64
            && y0 + bottom < ph as i64
    }
}

/// Inclusive support of imageproc's cubic interpolation. At an integer
/// coordinate only the centre pixel contributes, including at a photo edge.
fn cubic_support(x: f32) -> (i64, i64) {
    let base = x.floor();
    if x == base {
        (base as i64, base as i64)
    } else {
        (base as i64 - 1, base as i64 + 2)
    }
}

fn recentred_pixel(x: u32, y: u32, dx: f32, dy: f32, w: u32, h: u32) -> Option<(u32, u32)> {
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return Some((x, y));
    }
    let (sx, sy) = (x as f32 - dx, y as f32 - dy);
    (sx >= 0.0 && sy >= 0.0 && (sx as u32) < w && (sy as u32) < h).then(|| {
        (
            sx.round().min(w as f32 - 1.0) as u32,
            sy.round().min(h as f32 - 1.0) as u32,
        )
    })
}

/// `rotate_about_center` turns about the image centre; the frame's
/// centre is not there, so the turned image is shifted to put the
/// frame's centre where it was, as PIL's `center=` does.
fn recentre(level: &GrayImage, cx: f32, cy: f32, mx: f32, my: f32) -> GrayImage {
    let (w, h) = level.dimensions();
    let (dx, dy) = (cx - mx, cy - my);
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return level.clone();
    }
    let mut out = GrayImage::from_pixel(w, h, Luma([255]));
    for y in 0..h {
        for x in 0..w {
            if let Some((sx, sy)) = recentred_pixel(x, y, dx, dy, w, h) {
                out.put_pixel(x, y, *level.get_pixel(sx, sy));
            }
        }
    }
    out
}

/// Point-in-polygon by the even-odd rule.
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut hit = false;
    let n = poly.len();
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[(i + n - 1) % n];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            hit = !hit;
        }
    }
    hit
}

/// Column spans `[x0, x1)` of the words in a binarised line.
pub fn split_columns(ink: &[Vec<bool>], split: Split) -> Vec<(usize, usize)> {
    let h = ink.len();
    let w = ink.first().map_or(0, Vec::len);
    if h == 0 || w == 0 {
        return Vec::new();
    }
    let mut mask = GrayImage::new(w as u32, h as u32);
    for (y, row) in ink.iter().enumerate() {
        for (x, &v) in row.iter().enumerate() {
            if v {
                mask.put_pixel(x as u32, y as u32, Luma([255]));
            }
        }
    }
    let labels = connected_components(&mask, Connectivity::Eight, Luma([0]));
    let mut boxes: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (x, y, l) in labels.enumerate_pixels() {
        let l = l.0[0] as usize;
        if l == 0 {
            continue;
        }
        if boxes.len() < l {
            boxes.resize(l, (usize::MAX, 0, usize::MAX, 0));
        }
        let b = &mut boxes[l - 1];
        b.0 = b.0.min(x as usize);
        b.1 = b.1.max(x as usize + 1);
        b.2 = b.2.min(y as usize);
        b.3 = b.3.max(y as usize + 1);
    }
    let mut boxes: Vec<_> = boxes.into_iter().filter(|b| b.0 != usize::MAX).collect();
    if boxes.is_empty() {
        return Vec::new();
    }
    boxes.sort();
    let hf = h as f32;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (x0, x1, y0, y1) in boxes {
        let small =
            ((y1 - y0) as f32) < SMALL_INK_RATIO * hf && ((x1 - x0) as f32) < SMALL_INK_RATIO * hf;
        if small && !spans.is_empty() {
            let (px0, px1) = *spans.last().unwrap();
            if x0 as f32 - px1 as f32 <= split.rejoin * hf {
                *spans.last_mut().unwrap() = (px0.min(x0), px1.max(x1));
                continue;
            }
        }
        spans.push((x0, x1));
    }
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (x0, x1) in spans {
        match merged.last_mut() {
            Some(last) if x0 <= last.1 => last.1 = last.1.max(x1),
            _ => merged.push((x0, x1)),
        }
    }
    if merged.len() < 2 {
        return merged;
    }
    let gaps: Vec<f32> = merged
        .windows(2)
        .map(|p| p[1].0 as f32 - p[0].1 as f32)
        .collect();
    let median = {
        let mut g = gaps.clone();
        g.sort_by(|a, b| a.total_cmp(b));
        let n = g.len();
        if n % 2 == 1 {
            g[n / 2]
        } else {
            (g[n / 2 - 1] + g[n / 2]) / 2.0
        }
    };
    let cut = (split.factor * median).max(split.gap * hf);
    let mut words = vec![merged[0]];
    for (gap, span) in gaps.iter().zip(merged.iter().skip(1)) {
        if *gap > cut {
            words.push(*span);
        } else {
            words.last_mut().unwrap().1 = span.1;
        }
    }
    words
}

/// The word quads inside one prepared polygon, in photo pixels: the
/// spans in the frame, each the frame's full height plus the pad,
/// turned back about the frame's centre.
pub fn words_of(frame: Frame, ink: &[Vec<bool>], split: Split) -> Vec<Quad> {
    let hh = ink.len() as f32;
    let ww = ink.first().map_or(0, Vec::len) as f32;
    let mut out = Vec::new();
    for (sx0, sx1) in split_columns(ink, split) {
        if !ink.iter().any(|row| row[sx0..sx1].iter().any(|&v| v)) {
            continue;
        }
        let (px0, px1) = (
            (sx0 as f32 - split.pad * hh).max(0.0),
            (sx1 as f32 + split.pad * hh).min(ww),
        );
        let corners = [(px0, 0.0), (px1, 0.0), (px1, hh), (px0, hh)].map(|(lx, ly)| {
            let (dx, dy) = turn(lx - ww / 2.0, ly - hh / 2.0, frame.angle);
            (frame.cx + dx, frame.cy + dy)
        });
        out.push(Quad(clockwise(corners)));
    }
    out
}

/// The word cut level out of the photo: its frame's box, turned
/// level, as a grey crop with beyond-the-edge as paper. What the word
/// model reads.
pub fn level_crop(photo: &RgbImage, quad: &Quad) -> GrayImage {
    level_crop_in(photo, frame_of(quad))
}

/// Sample an explicitly supplied frame, without choosing another writing axis.
/// Callers own frame validation, margins and orientation; no photo is rotated.
pub fn level_crop_in(photo: &RgbImage, frame: Frame) -> GrayImage {
    sample_frame(photo, frame, None)
}

/// Level a word in the shared writing frame, without sampling outside its box.
/// A tilted quad need not fill its axis-aligned envelope; those corners are paper.
pub fn level_crop_within(photo: &RgbImage, frame: Frame, quad: &Quad) -> GrayImage {
    sample_frame(photo, frame, Some(quad))
}

fn sample_frame(photo: &RgbImage, frame: Frame, quad: Option<&Quad>) -> GrayImage {
    let (w, h) = (
        frame.w.round().max(1.0) as u32,
        frame.h.round().max(1.0) as u32,
    );
    let mut crop = GrayImage::from_pixel(w, h, Luma([255]));
    for y in 0..h {
        for x in 0..w {
            let (lx, ly) = (
                x as f32 + 0.5 - w as f32 / 2.0,
                y as f32 + 0.5 - h as f32 / 2.0,
            );
            let (dx, dy) = turn(lx, ly, frame.angle);
            let (px, py) = (frame.cx + dx, frame.cy + dy);
            if quad.is_some_and(|q| !inside(&q.0, px, py)) {
                continue;
            }
            if px >= 0.0 && py >= 0.0 && px < photo.width() as f32 && py < photo.height() as f32 {
                let p = photo.get_pixel(px as u32, py as u32).0;
                let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                crop.put_pixel(x, y, Luma([l.round() as u8]));
            }
        }
    }
    crop
}

#[test]
fn box_only_sampler_excludes_neighbour_ink_in_the_frame_envelope() {
    let quad = Quad([(15., 0.), (40., 15.), (25., 40.), (0., 25.)]);
    let mut photo = RgbImage::from_pixel(40, 40, image::Rgb([0; 3]));
    for (x, y, p) in photo.enumerate_pixels_mut() {
        if inside(&quad.0, x as f32 + 0.5, y as f32 + 0.5) {
            *p = image::Rgb([170; 3]);
        }
    }
    let frame = Frame {
        cx: 20.,
        cy: 20.,
        w: 40.,
        h: 40.,
        angle: 0.,
    };
    assert!(level_crop_in(&photo, frame).pixels().any(|p| p.0[0] == 0));
    let exact = level_crop_within(&photo, frame, &quad);
    assert!(exact.pixels().all(|p| matches!(p.0[0], 170 | 255)));
    assert!(exact.pixels().any(|p| p.0[0] == 170));
    assert!(exact.pixels().any(|p| p.0[0] == 255));
}

/// The order a person reads the words in: columns left to right, top
/// to bottom within a column. A word joins a column when its centre
/// falls within an existing column's horizontal span; the column's
/// span grows to hold it.
pub fn reading_order(words: &[Quad]) -> Vec<usize> {
    crate::layout::PageLayout::new(words).column_order()
}

/// The other order a person reads in: rows top to bottom, left to
/// right within a row. A word joins a row when its centre falls within
/// the row's vertical span. Pages are written both ways, and geometry
/// cannot say which, so both orders are offered.
pub fn reading_order_rows(words: &[Quad]) -> Vec<usize> {
    crate::layout::PageLayout::new(words).row_order()
}

/// A quad grown by `margin` of its height on every side, in its own
/// frame: the margin the cut table's crops carry, which the word model
/// was trained to see.
pub fn with_margin(quad: &Quad, margin: f32) -> Quad {
    let frame = frame_of(quad);
    let m = margin * frame.h;
    let (hw, hh) = (frame.w / 2.0 + m, frame.h / 2.0 + m);
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(lx, ly)| {
        let (dx, dy) = turn(lx, ly, frame.angle);
        (frame.cx + dx, frame.cy + dy)
    });
    Quad(clockwise(corners))
}

/// The crop turned a half turn: a line standing on end in the photo
/// is levelled by a quarter turn either way, and only a reader can
/// say which way was up.
pub fn upside_down(crop: &GrayImage) -> GrayImage {
    image::imageops::rotate180(crop)
}

// Compatibility helper for the original mask tests, never a production API.
#[cfg(test)]
fn split_quad(photo: &RgbImage, quad: &Quad, split: Split) -> Vec<Quad> {
    split_quad_accounted(photo, quad, split).unwrap_or_default()
}

/// Why an accepted detector polygon produced no word parts. This describes
/// segmentation, not a judgement that the source is safe to discard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptySplit {
    /// The frame or rounded extraction was too small to form a mask.
    NoFrame,
    /// No sample inside the polygon came entirely from the photograph.
    NoPhotoSamples,
    /// All valid samples had the same intensity, so no ink threshold exists.
    FlatPhotoSamples,
    /// Thresholded ink was entirely removed by the locked rule stripper.
    NoInkAfterRules,
    /// A nonempty mask yielded no usable word spans.
    NoSpans,
}

impl EmptySplit {
    /// Stable diagnostic name; not OCR text or a classifier decision.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoFrame => "no_frame",
            Self::NoPhotoSamples => "no_photo_samples",
            Self::FlatPhotoSamples => "flat_photo_samples",
            Self::NoInkAfterRules => "no_ink_after_rules",
            Self::NoSpans => "no_spans",
        }
    }
}

/// Splits once, returning either nonempty parts or an explicit empty reason.
/// This interface cannot silently flatten away a source.
pub fn split_quad_accounted(
    photo: &RgbImage,
    quad: &Quad,
    split: Split,
) -> Result<Vec<Quad>, EmptySplit> {
    let (frame, ink) = ink_of_accounted(photo, quad)?;
    parts_of(frame, ink, split)
}

pub(crate) fn split_in_direction(
    photo: &RgbImage,
    quad: &Quad,
    split: Split,
    direction: crate::page_frame::Direction,
) -> Result<Vec<Quad>, EmptySplit> {
    let frame = measurement_frame(quad, direction).ok_or(EmptySplit::NoFrame)?;
    let (frame, ink) = ink_in_frame(photo, quad, frame)?;
    let writing = crate::page_frame::WritingFrame::Page(direction);
    Ok(parts_of(frame, ink, split)?
        .iter()
        .map(|part| writing.with_margin(part, 0.))
        .collect())
}

/// Register the gap mask to the closest rectangle edge, with page-agreeing
/// polarity. Tall regions use their short edge, not a private 90-degree turn.
/// Numerically square rectangles and equally close edges have no unique local
/// measurement axis, so use the page frame. No split threshold changes here.
fn measurement_frame(quad: &Quad, direction: crate::page_frame::Direction) -> Option<Frame> {
    let page = direction.frame(quad, 0.)?; // Also validates the input polygon.
    let mut local = frame_of(quad);
    if (local.w - local.h).abs() <= 8. * f32::EPSILON * local.w {
        return Some(page);
    }
    let delta = (local.angle - page.angle + 90.).rem_euclid(180.) - 90.;
    if (delta.abs() - 45.).abs() <= 8. * f32::EPSILON * 180. {
        return Some(page);
    }
    if delta.abs() > 45. {
        std::mem::swap(&mut local.w, &mut local.h);
        local.angle += 90.;
    }
    // Preserve the original min-rectangle angle exactly when already agreeing:
    // re-deriving it from page.angle + delta can perturb the sampling grid.
    local.angle += 180. * ((page.angle - local.angle) / 180.).round();
    Some(local)
}

fn parts_of(frame: Frame, ink: InkMask, split: Split) -> Result<Vec<Quad>, EmptySplit> {
    if let Some(reason) = ink.empty {
        return Err(reason);
    }
    let parts = words_of(frame, &ink.pixels, split);
    if parts.is_empty() {
        Err(EmptySplit::NoSpans)
    } else {
        Ok(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn rectangle(w: f32, h: f32, angle: f32) -> Quad {
        Quad(clockwise(
            [
                (-w / 2., -h / 2.),
                (w / 2., -h / 2.),
                (w / 2., h / 2.),
                (-w / 2., h / 2.),
            ]
            .map(|(x, y)| {
                let (x, y) = turn(x, y, angle);
                (200. + x, 200. + y)
            }),
        ))
    }

    fn assert_page_enclosure(span: &Quad, output: &Quad, direction: crate::page_frame::Direction) {
        let f = direction.frame(output, 0.).unwrap();
        assert_eq!(f.angle, direction.angle_degrees());
        for &(x, y) in &span.0 {
            let (x, y) = turn(x - f.cx, y - f.cy, -f.angle);
            assert!(
                x.abs() <= f.w / 2. + 0.002 && y.abs() <= f.h / 2. + 0.002,
                "span must fit output: {span:?} in {output:?}"
            );
        }
        for &(x, y) in &output.0 {
            let (x, y) = turn(x - f.cx, y - f.cy, -f.angle);
            assert!((x.abs() - f.w / 2.).abs() < 0.002);
            assert!((y.abs() - f.h / 2.).abs() < 0.002);
        }
        // The actual reader frame also retains the page angle, including tall
        // labels whose min-area-rectangle long edge would choose another axis.
        assert_eq!(
            crate::page_frame::WritingFrame::Page(direction)
                .crop_frame(output, 0.15)
                .angle,
            f.angle
        );
    }

    #[test]
    fn gap_registration_keeps_tall_square_and_turned_outputs_in_page_direction() {
        for page_angle in [0., 90.] {
            let axis =
                crate::page_frame::PageAxis::from_detections(&[rectangle(300., 20., page_angle)]);
            for reversed in [false, true] {
                let direction = axis.direction(reversed);
                for (w, h) in [(160., 40.), (30., 80.), (60., 60.)] {
                    let angle = direction.angle_degrees() + 12.;
                    let quad = rectangle(w, h, angle);
                    let measured = measurement_frame(&quad, direction).unwrap();
                    if w == h {
                        assert_eq!(measured, direction.frame(&quad, 0.).unwrap());
                    } else {
                        assert!((measured.angle - angle).abs() < 0.001, "{measured:?}");
                        assert!((measured.w - w).abs() < 0.001);
                        assert!((measured.h - h).abs() < 0.001);
                    }
                    let photo = RgbImage::from_fn(400, 400, |x, y| {
                        let (x, y) = turn(x as f32 - 200., y as f32 - 200., -angle);
                        // Two rows of strokes are still one tall word/label;
                        // the wide case has two groups of strokes across x.
                        let ink = if w > h {
                            y.abs() < 12.
                                && [-55., -40., -25., 25., 40., 55.]
                                    .iter()
                                    .any(|p| (x - p).abs() < 4.)
                        } else {
                            x.abs() < 4. && (y.abs() - h * 0.2).abs() < 7.
                        };
                        Rgb([if ink { 20 } else { 235 }; 3])
                    });
                    let (f, ink) = ink_in_frame(&photo, &quad, measured).unwrap();
                    let spans = parts_of(f, ink, Split::LOCKED).unwrap();
                    let outputs =
                        split_in_direction(&photo, &quad, Split::LOCKED, direction).unwrap();
                    assert_eq!(outputs.len(), if w > h { 2 } else { 1 });
                    assert_eq!(spans.len(), outputs.len());
                    for (span, output) in spans.iter().zip(&outputs) {
                        assert_page_enclosure(span, output, direction);
                    }
                }
                // Distinct rectangle edges can also be equally close to the
                // page direction. Neither edge gets an arbitrary tie-break.
                let diagonal = rectangle(120., 40., direction.angle_degrees() + 45.);
                assert_eq!(
                    measurement_frame(&diagonal, direction),
                    direction.frame(&diagonal, 0.)
                );
                assert!(measurement_frame(&Quad([(f32::NAN, 0.); 4]), direction).is_none());
            }
        }
    }

    fn line(words: &[usize], gap: u32, rule: bool, dot: bool) -> (RgbImage, Vec<(u32, u32)>) {
        let mut img = RgbImage::from_pixel(400, 60, Rgb([235, 235, 235]));
        let mut x = 20u32;
        let mut boxes = Vec::new();
        for &w in words {
            for k in 0..w as u32 {
                for yy in 20..45 {
                    for xx in (x + k * 14)..(x + k * 14 + 10) {
                        img.put_pixel(xx, yy, Rgb([20, 20, 20]));
                    }
                }
            }
            boxes.push((x, x + w as u32 * 14 - 5));
            x += w as u32 * 14 - 5 + gap;
        }
        if rule {
            for xx in 0..400 {
                img.put_pixel(xx, 46, Rgb([20, 20, 20]));
                img.put_pixel(xx, 47, Rgb([20, 20, 20]));
            }
        }
        if dot {
            for yy in 8..13 {
                for xx in (boxes[0].0 + 2)..(boxes[0].0 + 7) {
                    img.put_pixel(xx, yy, Rgb([20, 20, 20]));
                }
            }
        }
        (img, boxes)
    }

    fn grey(img: &RgbImage) -> GrayImage {
        image::imageops::grayscale(img)
    }

    #[test]
    fn photo_membership_matches_imageproc_rotation_and_cubic_support() {
        // Real photo is uniform zero; synthetic padding has contrasting,
        // nonuniform values to avoid cubic positive/negative tap cancellation.
        // Float pixels expose even weak padding contributions that byte
        // rounding/clamping could hide. The coordinate/kernel code is shared
        // with imageproc's byte interpolation, not reimplemented by this test.
        let region = image::ImageBuffer::from_fn(32, 28, |x, y| {
            Luma([if (7..25).contains(&x) && (6..20).contains(&y) {
                0.0f32
            } else {
                1e8 * (1 + (x * 31 + y * 17) % 23) as f32
            }])
        });
        for angle in [0.0f32, 17.0, -29.0] {
            let level = rotate_about_center(
                &region,
                angle.to_radians(),
                Interpolation::Bicubic,
                Border::Constant(Luma([1e8])),
            );
            for (dx, dy) in [(0.0, 0.0), (0.27, -0.31), (-0.8, 0.6)] {
                let is_photo =
                    photo_sample_map((18, 14), (-7, -6), (32, 28), (16.0 + dx, 14.0 + dy), angle);
                let mut included = 0;
                for y in 0..28 {
                    for x in 0..32 {
                        let value = recentred_pixel(x, y, dx, dy, 32, 28)
                            .map_or(1e8, |(sx, sy)| level.get_pixel(sx, sy).0[0]);
                        let valid = is_photo(x, y);
                        assert_eq!(
                            valid,
                            value == 0.0,
                            "angle={angle} shift={dx},{dy} at={x},{y} value={value}"
                        );
                        included += usize::from(valid);
                    }
                }
                assert!(included > 100 && included < 300);
            }
        }
    }

    #[test]
    fn valid_histogram_ignores_padding_but_keeps_real_white_samples() {
        let crop = GrayImage::from_raw(5, 1, vec![0, 60, 80, 255, 255]).unwrap();
        let valid = vec![vec![false, true, true, false, false]];
        assert_eq!(
            binarise_valid(&crop, valid),
            vec![vec![false, true, false, false, false]]
        );
        // This time one white pixel is actual photographed paper, not
        // padding. It must participate, exactly as it does in binarise.
        let actual = GrayImage::from_raw(3, 1, vec![60, 80, 255]).unwrap();
        assert_eq!(binarise(&actual), vec![vec![true, true, false]]);
        assert_eq!(
            binarise_valid(&crop, vec![vec![false, true, true, true, false]]),
            vec![vec![false, true, true, false, false]]
        );
    }

    #[test]
    fn valid_histogram_matches_unmasked_otsu_without_padding() {
        let crop = GrayImage::from_fn(32, 16, |x, y| Luma([((x * 17 + y * 31) % 256) as u8]));
        assert_eq!(
            binarise_valid(&crop, vec![vec![true; 32]; 16]),
            binarise(&crop)
        );
    }

    #[test]
    fn flat_or_absent_valid_samples_are_not_ink() {
        let crop = GrayImage::from_raw(4, 1, vec![0, 80, 80, 255]).unwrap();
        for valid in [vec![vec![false, true, true, false]], vec![vec![false; 4]]] {
            assert_eq!(binarise_valid(&crop, valid), vec![vec![false; 4]]);
        }
        assert!(binarise_valid(&GrayImage::new(0, 0), vec![]).is_empty());
    }

    #[test]
    fn a_white_padding_border_must_not_erase_a_dark_word_as_a_rule() {
        // A small rounding border is much brighter than either the ink or
        // dimly photographed paper: the same failure mode as saved absorb.
        let mut crop = GrayImage::from_pixel(101, 31, Luma([255]));
        let mut valid = vec![vec![false; 101]; 31];
        for y in 0..30 {
            for x in 0..100 {
                crop.put_pixel(x, y, Luma([if (40..44).contains(&x) { 60 } else { 80 }]));
                valid[y as usize][x as usize] = true;
            }
        }
        let mut old = binarise(&crop);
        assert!(old[0][0]); // Wrongly calls the photographed paper ink.
        strip_rules(&mut old);
        assert!(!old.iter().flatten().any(|&p| p));
        let mut corrected = binarise_valid(&crop, valid);
        strip_rules(&mut corrected);
        assert_eq!(corrected.iter().flatten().filter(|&&p| p).count(), 4 * 30);
        assert_eq!(split_columns(&corrected, Split::LOCKED), vec![(40, 44)]);
    }

    #[test]
    fn a_rounded_dark_photo_quad_keeps_its_word() {
        let mut photo = RgbImage::from_pixel(200, 100, Rgb([80; 3]));
        for y in 20..51 {
            for x in 60..64 {
                photo.put_pixel(x, y, Rgb([60; 3]));
            }
        }
        let quad = Quad([(20.4, 20.4), (120.6, 20.4), (120.6, 50.6), (20.4, 50.6)]);
        let (_, ink) = ink_of(&photo, &quad).unwrap();
        assert_eq!((ink[0].len(), ink.len()), (101, 31));
        assert!(ink.iter().flatten().any(|&p| p));
        assert!(ink.iter().all(|row| !row[100]));
        assert!(ink[30].iter().all(|&p| !p));
        assert_eq!(split_quad(&photo, &quad, Split::LOCKED).len(), 1);
    }

    #[test]
    fn photo_edges_do_not_manufacture_ink_from_flat_paper() {
        let photo = RgbImage::from_pixel(80, 60, Rgb([80; 3]));
        for angle in [0.0, 20.0, -20.0] {
            for (cx, cy) in [(0.0, 30.0), (80.0, 30.0), (40.0, 0.0), (40.0, 60.0)] {
                let quad = Quad(
                    [(-30.0, -10.0), (30.0, -10.0), (30.0, 10.0), (-30.0, 10.0)].map(|(x, y)| {
                        let (dx, dy) = turn(x, y, angle);
                        (cx + dx, cy + dy)
                    }),
                );
                let (_, ink) = ink_of(&photo, &quad).unwrap();
                assert!(!ink.iter().flatten().any(|&p| p), "{angle}: {quad:?}");
            }
        }
    }

    #[test]
    fn photo_edge_ink_and_integer_cubic_samples_are_retained() {
        assert_eq!(cubic_support(0.0), (0, 0));
        assert_eq!(cubic_support(0.5), (-1, 2));
        assert_eq!(cubic_support(-0.5), (-2, 1));
        let mut photo = RgbImage::from_pixel(80, 60, Rgb([80; 3]));
        for y in 10..30 {
            for x in 0..4 {
                photo.put_pixel(x, y, Rgb([20; 3]));
            }
        }
        let quad = Quad([(-20.0, 5.0), (40.0, 5.0), (40.0, 35.0), (-20.0, 35.0)]);
        let (_, ink) = ink_of(&photo, &quad).unwrap();
        assert_eq!(ink.iter().flatten().filter(|&&p| p).count(), 80);
        assert_eq!(split_columns(&ink, Split::LOCKED).len(), 1);
    }

    #[test]
    fn three_words_split_at_the_wide_gaps() {
        let (img, boxes) = line(&[3, 4, 2], 30, false, false);
        let ink = binarise(&grey(&img));
        let spans = split_columns(&ink, Split::LOCKED);
        assert_eq!(spans.len(), 3);
        for ((x0, x1), (bx0, bx1)) in spans.iter().zip(boxes) {
            assert!((*x0 as i64 - bx0 as i64).abs() <= 1);
            assert!((*x1 as i64 - bx1 as i64).abs() <= 1);
        }
    }

    #[test]
    fn a_ruled_line_does_not_glue_the_words_and_a_dot_rejoins_its_word() {
        let (img, _) = line(&[3, 4, 2], 30, true, false);
        let mut ink = binarise(&grey(&img));
        strip_rules(&mut ink);
        assert_eq!(split_columns(&ink, Split::LOCKED).len(), 3);
        assert!(!ink[46].iter().all(|&v| v));
        let (img, boxes) = line(&[3, 4], 30, false, true);
        let spans = split_columns(&binarise(&grey(&img)), Split::LOCKED);
        assert_eq!(spans.len(), 2);
        assert!((spans[0].0 as i64 - boxes[0].0 as i64).abs() <= 1);
    }

    #[test]
    fn a_lone_word_is_not_cut_at_its_widest_letter_gap() {
        let mut img = RgbImage::from_pixel(200, 60, Rgb([235, 235, 235]));
        for x in [20u32, 34, 48, 66, 80] {
            for yy in 20..45 {
                for xx in x..x + 10 {
                    img.put_pixel(xx, yy, Rgb([20, 20, 20]));
                }
            }
        }
        let ink = binarise(&grey(&img));
        assert_eq!(split_columns(&ink, Split::LOCKED).len(), 1);
        // the widest letter gap is 8 px: a fifth of the height, so a gap floor of a tenth lets a low factor cut it
        assert_eq!(
            split_columns(
                &ink,
                Split {
                    factor: 1.5,
                    gap: 0.1,
                    ..Split::LOCKED
                }
            )
            .len(),
            2
        );
    }

    #[test]
    fn reading_order_runs_down_each_column_then_to_the_next() {
        let q = |x: f32, y: f32| Quad([(x, y), (x + 40.0, y), (x + 40.0, y + 10.0), (x, y + 10.0)]);
        // a left column of three and a right column of two, listed out of order
        let words = vec![
            q(300.0, 50.0),
            q(10.0, 90.0),
            q(15.0, 10.0),
            q(305.0, 5.0),
            q(12.0, 50.0),
        ];
        assert_eq!(reading_order(&words), vec![2, 4, 1, 3, 0]);
    }

    #[test]
    fn row_order_runs_along_each_row_then_down() {
        let q = |x: f32, y: f32| Quad([(x, y), (x + 40.0, y), (x + 40.0, y + 10.0), (x, y + 10.0)]);
        let words = vec![
            q(300.0, 50.0),
            q(10.0, 90.0),
            q(15.0, 10.0),
            q(305.0, 5.0),
            q(12.0, 50.0),
        ];
        assert_eq!(reading_order_rows(&words), vec![2, 3, 4, 0, 1]);
    }

    #[test]
    fn a_margin_grows_the_quad_in_its_own_frame() {
        let q = Quad([
            (100.0, 100.0),
            (200.0, 100.0),
            (200.0, 120.0),
            (100.0, 120.0),
        ]);
        let g = with_margin(&q, 0.5);
        assert_eq!(g.0[0], (90.0, 90.0));
        assert_eq!(g.0[2], (210.0, 130.0));
    }

    #[test]
    fn a_level_line_on_a_page_splits_into_words_in_photo_pixels() {
        let (img, boxes) = line(&[3, 4, 2], 30, false, false);
        let mut page = RgbImage::from_pixel(600, 200, Rgb([235, 235, 235]));
        image::imageops::overlay(&mut page, &img, 100, 70);
        let quad = Quad([(100.0, 85.0), (500.0, 85.0), (500.0, 120.0), (100.0, 120.0)]);
        let words = split_quad(&page, &quad, Split::LOCKED);
        assert_eq!(words.len(), 3);
        assert!(
            (words[0].0[0].0 - (100.0 + boxes[0].0 as f32)).abs() <= 1.5,
            "{:?}",
            words[0]
        );
        assert!((words[0].0[0].1 - 85.0).abs() <= 1.0);
        assert!((words[0].0[2].1 - 120.0).abs() <= 1.0);
    }

    #[test]
    fn a_turned_line_splits_in_its_own_frame_and_ignores_ink_outside_the_polygon() {
        let (img, _) = line(&[3, 4, 2], 30, false, false);
        let mut page = RgbImage::from_pixel(700, 500, Rgb([235, 235, 235]));
        image::imageops::overlay(&mut page, &img, 150, 220);
        let angle = 20f32;
        let (cx, cy) = (350.0f32, 250.0f32);
        let turned = rotate_about_center(
            &page,
            -angle.to_radians(),
            Interpolation::Bicubic,
            Border::Constant(Rgb([235, 235, 235])),
        );
        let corner = |x: f32, y: f32| {
            let (dx, dy) = turn(x - cx, y - cy, angle);
            (cx + dx, cy + dy)
        };
        let quad = Quad(clockwise([
            corner(150.0, 235.0),
            corner(550.0, 235.0),
            corner(550.0, 270.0),
            corner(150.0, 270.0),
        ]));
        let mut with_distractor = turned.clone();
        let (dx, dy) = turn(0.0, -60.0, angle);
        let (bx, by) = (cx + dx + 50.0, cy + dy);
        for yy in (by as u32 - 6)..(by as u32 + 6) {
            for xx in (bx as u32 - 6)..(bx as u32 + 6) {
                with_distractor.put_pixel(xx, yy, Rgb([20, 20, 20]));
            }
        }
        let words = split_quad(&with_distractor, &quad, Split::LOCKED);
        assert_eq!(words.len(), 3, "{words:?}");
        // every word's centre, turned back, lies on the drawn line's band, and the words hold the letters, not the blob
        for w in &words {
            let (mx, my) = (
                w.0.iter().map(|p| p.0).sum::<f32>() / 4.0,
                w.0.iter().map(|p| p.1).sum::<f32>() / 4.0,
            );
            let (dx, dy) = turn(mx - cx, my - cy, -angle);
            let (ux, uy) = (cx + dx, cy + dy);
            assert!((240.0..=265.0).contains(&uy), "{w:?} -> ({ux}, {uy})");
            assert!((150.0..=550.0).contains(&ux), "{w:?} -> ({ux}, {uy})");
        }
    }
}
