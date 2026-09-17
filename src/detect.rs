//! Finding the words in a photo: the PP-OCRv4 detector, a differentiable
//! binarisation network, run in process through `tract`, and DB's own
//! post-processing over its probability map. Feature `onnx`.
//!
//! The photo is resized so its long side is at most [`LIMIT`] and both
//! sides are multiples of 32, normalised as PaddleOCR does, and the
//! network returns one probability per pixel of text. Pixels above
//! [`THRESH`] form regions; a region whose mean probability is under
//! [`BOX_THRESH`] is dropped; the rest are boxed by their minimum-area
//! rectangle grown by [`UNCLIP`] times area over perimeter, which undoes
//! the shrink the network was trained with, and scaled back into the
//! photo. This is the survey's post-processing (`detect_survey.py`),
//! so the two agree polygon for polygon within a pixel.

use std::path::Path;

use image::{GrayImage, Luma, RgbImage};
use imageproc::contours::{BorderType, find_contours};
use imageproc::drawing::draw_polygon_mut;
use imageproc::geometry::{arc_length, contour_area};
use imageproc::point::Point;

/// The photo's long side is resized to at most this before detection.
pub const LIMIT: u32 = 960;
/// A pixel is text above this probability.
pub const THRESH: f32 = 0.3;
/// A region is kept when its mean probability reaches this.
pub const BOX_THRESH: f32 = 0.6;
/// A region's box grows by this times its area over its perimeter.
pub const UNCLIP: f32 = 1.5;
/// A box narrower than this in either direction is noise.
const MIN_SIDE: f32 = 3.0;
#[cfg(feature = "onnx")]
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
#[cfg(feature = "onnx")]
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Four corners in photo pixels, clockwise from the top left.
#[derive(Clone, Debug, PartialEq)]
pub struct Quad(pub [(f32, f32); 4]);

/// Closed convex-quad intersection via the separating-axis test. Shared by
/// extent and fragment ownership so neighboring-ink vetoes use one geometry.
pub(crate) fn intersects(a: &Quad, b: &Quad) -> bool {
    for q in [a, b] {
        for i in 0..4 {
            let (p, r) = (q.0[i], q.0[(i + 1) % 4]);
            let axis = (p.1 - r.1, r.0 - p.0);
            let project = |s: &Quad| s.0.map(|v| v.0 * axis.0 + v.1 * axis.1);
            let (pa, pb) = (project(a), project(b));
            let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
            let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
            if max(pa) < min(pb) || max(pb) < min(pa) {
                return false;
            }
        }
    }
    true
}

/// The detector: the network, with one optimised plan per input shape
/// it has seen, since tract fixes the shape at optimisation.
#[cfg(feature = "onnx")]
pub struct Detector {
    model: tract_onnx::prelude::InferenceModel,
    plans: std::sync::Mutex<
        std::collections::HashMap<
            (usize, usize),
            std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>,
        >,
    >,
    spec: String,
}

/// The detector needs a build with the `onnx` feature; without it,
/// loading refuses.
#[cfg(not(feature = "onnx"))]
pub struct Detector {}

#[cfg(not(feature = "onnx"))]
impl Detector {
    /// Refuses loading bytes when the inference runtime is disabled.
    pub fn from_bytes(_bytes: &[u8]) -> Result<Detector, String> {
        Err("the detector requires bitcoin-vision's onnx feature".into())
    }

    /// Refuses: no runtime in this build.
    pub fn load(_path: &Path) -> Result<Detector, String> {
        Err("the detector needs a build with penlock-scan's onnx feature".into())
    }

    /// Refuses: no runtime in this build.
    pub fn probability_map(&self, _photo: &RgbImage) -> Result<ProbabilityMap, String> {
        Err("the detector needs a build with penlock-scan's onnx feature".into())
    }

    /// Refuses: no runtime in this build.
    pub fn probability_map_with_limit(
        &self,
        _photo: &RgbImage,
        _limit: u32,
    ) -> Result<ProbabilityMap, String> {
        Err("the detector needs a build with penlock-scan's onnx feature".into())
    }

    /// Refuses: no runtime in this build.
    pub fn detect(&self, _photo: &RgbImage) -> Result<Vec<Quad>, String> {
        Err("the detector needs a build with penlock-scan's onnx feature".into())
    }
}

#[cfg(feature = "onnx")]
impl Detector {
    /// Loads the detector at `path`; the input must be `1×3×H×W`.
    pub fn load(path: &Path) -> Result<Detector, String> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Detector {
            model,
            plans: std::sync::Mutex::new(std::collections::HashMap::new()),
            spec: path.display().to_string(),
        })
    }

    /// Loads a detector already in memory, as an app ships it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Detector, String> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))
            .map_err(|e| format!("detector:<memory>: {e}"))?;
        Ok(Detector {
            model,
            plans: std::sync::Mutex::new(std::collections::HashMap::new()),
            spec: "detector:<memory>".into(),
        })
    }

    fn plan(
        &self,
        h: usize,
        w: usize,
    ) -> Result<std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>, String> {
        use tract_onnx::prelude::*;
        let mut plans = self.plans.lock().map_err(|e| e.to_string())?;
        if let Some(plan) = plans.get(&(h, w)) {
            return Ok(plan.clone());
        }
        let _build = crate::timing::span("detector.plan_build");
        let plan = self
            .model
            .clone()
            .with_input_fact(0, f32::fact([1, 3, h, w]).into())
            .map_err(|e| format!("{}: input is not 1×3×H×W: {e}", self.spec))?
            .into_optimized()
            .map_err(|e| format!("{}: {e}", self.spec))?
            .into_runnable()
            .map_err(|e| format!("{}: {e}", self.spec))?;
        plans.insert((h, w), plan.clone());
        Ok(plan)
    }

    /// The network's text-probability map for the photo, at the
    /// resized shape it was computed at.
    pub fn probability_map(&self, photo: &RgbImage) -> Result<ProbabilityMap, String> {
        self.probability_map_with_limit(photo, LIMIT)
    }

    /// The same detector with an explicit long-side limit, for controlled
    /// offline studies. The app uses [`Self::probability_map`] and [`LIMIT`].
    /// Limits outside 32..=2048 are refused to bound diagnostic input shapes.
    pub fn probability_map_with_limit(
        &self,
        photo: &RgbImage,
        limit: u32,
    ) -> Result<ProbabilityMap, String> {
        if !(32..=2048).contains(&limit) {
            return Err("detector long-side limit must be in 32..=2048".into());
        }
        let _input = crate::timing::span("detector.input");
        let (w, h) = photo.dimensions();
        let scale = (limit as f32 / w.max(h) as f32).min(1.0);
        // ties go to even, as the survey's Python `round` does, so the
        // two runtimes see the same input shape
        let round32 = |v: f32| (((v / 32.0).round_ties_even() as u32) * 32).max(32);
        let (rw, rh) = (round32(w as f32 * scale), round32(h as f32 * scale));
        let resized = image::imageops::resize(photo, rw, rh, image::imageops::FilterType::Triangle);
        let (rw, rh) = (rw as usize, rh as usize);
        let mut input = vec![0f32; 3 * rh * rw];
        for (x, y, p) in resized.enumerate_pixels() {
            for c in 0..3 {
                input[c * rh * rw + y as usize * rw + x as usize] =
                    (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
        use tract_onnx::prelude::*;
        let tensor = Tensor::from_shape(&[1, 3, rh, rw], &input).map_err(|e| e.to_string())?;
        drop(_input);
        let plan = self.plan(rh, rw)?;
        let _infer = crate::timing::span("detector.infer");
        let data = crate::runtime::run_plan(&plan, tensor)?;
        drop(_infer);
        if data.len() != rh * rw {
            return Err(format!(
                "{}: the map has {} values for a {rw}×{rh} input",
                self.spec,
                data.len()
            ));
        }
        Ok(ProbabilityMap {
            width: rw,
            height: rh,
            data,
            photo_width: w,
            photo_height: h,
        })
    }

    /// The text regions of the photo as quads in photo pixels.
    pub fn detect(&self, photo: &RgbImage) -> Result<Vec<Quad>, String> {
        Ok(self.probability_map(photo)?.quads())
    }
}

/// One probability per pixel of the resized photo.
pub struct ProbabilityMap {
    /// Width of the map, the resized photo's.
    pub width: usize,
    /// Height of the map.
    pub height: usize,
    /// Row-major probabilities.
    pub data: Vec<f32>,
    photo_width: u32,
    photo_height: u32,
}

impl ProbabilityMap {
    /// Rehydrate a saved, full-precision map for offline postprocessing. This
    /// does not load or run a model; dimensions retain the original photo frame.
    pub fn from_data(
        width: usize,
        height: usize,
        data: Vec<f32>,
        photo_width: u32,
        photo_height: u32,
    ) -> Result<Self, String> {
        if width == 0
            || height == 0
            || photo_width == 0
            || photo_height == 0
            || width.checked_mul(height) != Some(data.len())
            || data
                .iter()
                .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
        {
            return Err("invalid probability map dimensions or values".into());
        }
        Ok(Self {
            width,
            height,
            data,
            photo_width,
            photo_height,
        })
    }

    /// The map as an 8-bit image, probability times 255, for inspection.
    pub fn to_image(&self) -> GrayImage {
        let mut img = GrayImage::new(self.width as u32, self.height as u32);
        for (i, p) in self.data.iter().enumerate() {
            img.put_pixel(
                (i % self.width) as u32,
                (i / self.width) as u32,
                Luma([(p.clamp(0.0, 1.0) * 255.0).round() as u8]),
            );
        }
        img
    }

    /// DB's post-processing: regions above [`THRESH`], scored, boxed by
    /// their minimum-area rectangle, unclipped, scaled to the photo.
    pub fn quads(&self) -> Vec<Quad> {
        self.quads_with_threshold(THRESH)
            .expect("the locked threshold is valid")
    }

    /// DB postprocessing at an explicit text-pixel threshold for offline
    /// studies. Region scoring and box growth stay unchanged. This is a
    /// plain lower threshold, not hysteresis with a separate high seed.
    /// Non-finite thresholds and values outside [0, 1] are refused.
    pub fn quads_with_threshold(&self, threshold: f32) -> Result<Vec<Quad>, String> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err("detector threshold must be finite and in [0, 1]".into());
        }
        let _post = crate::timing::span("detector.postprocess");
        let (w, h) = (self.width as u32, self.height as u32);
        let mut binary = GrayImage::new(w, h);
        for (i, p) in self.data.iter().enumerate() {
            if *p > threshold {
                binary.put_pixel(
                    (i % self.width) as u32,
                    (i / self.width) as u32,
                    Luma([255]),
                );
            }
        }
        let sx = self.photo_width as f32 / w as f32;
        let sy = self.photo_height as f32 / h as f32;
        let mut out = Vec::new();
        for contour in find_contours::<i32>(&binary) {
            if contour.border_type != BorderType::Outer || contour.points.len() < 3 {
                continue;
            }
            let mut points = contour.points.clone();
            if points.first() == points.last() {
                points.pop();
            }
            if points.len() < 3 {
                continue;
            }
            let Some(score) = self.region_score(&points) else {
                continue;
            };
            if score < BOX_THRESH {
                continue;
            }
            let fpoints: Vec<Point<f32>> = points
                .iter()
                .map(|p| Point::new(p.x as f32, p.y as f32))
                .collect();
            let area = contour_area(&fpoints) as f32;
            let perimeter = arc_length(&fpoints, true) as f32;
            if perimeter <= 0.0 {
                continue;
            }
            let grow = area * UNCLIP / perimeter;
            let rect = min_area_rect(&unclipped(&fpoints, grow));
            let side =
                |a: Point<f32>, b: Point<f32>| ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
            if side(rect[0], rect[1]).min(side(rect[1], rect[2])) < MIN_SIDE {
                continue;
            }
            out.push(Quad(clockwise(rect.map(|p| (p.x * sx, p.y * sy)))));
        }
        Ok(out)
    }

    /// The mean probability inside the region's filled outline.
    fn region_score(&self, points: &[Point<i32>]) -> Option<f32> {
        let (w, h) = (self.width as u32, self.height as u32);
        let mut mask = GrayImage::new(w, h);
        draw_polygon_mut(&mut mask, points, Luma([255]));
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for (x, y, p) in mask.enumerate_pixels() {
            if p.0[0] > 0 {
                sum += self.data[y as usize * self.width + x as usize];
                n += 1;
            }
        }
        (n > 0).then(|| sum / n as f32)
    }
}

/// The outline offset outward by `d` with round joins, as the survey's
/// polygon clipper does it: the points of the outline's Minkowski sum
/// with a disc, whose convex hull is what the minimum-area rectangle
/// sees, so it is enough to sample the disc around every point.
fn unclipped(points: &[Point<f32>], d: f32) -> Vec<Point<f32>> {
    const SAMPLES: usize = 16;
    let mut out = Vec::with_capacity(points.len() * SAMPLES);
    for p in points {
        for k in 0..SAMPLES {
            let a = k as f32 * std::f32::consts::TAU / SAMPLES as f32;
            out.push(Point::new(p.x + d * a.cos(), p.y + d * a.sin()));
        }
    }
    out
}

/// The convex hull by monotone chain, counter-clockwise, no repeats.
fn convex_hull(points: &[Point<f32>]) -> Vec<Point<f32>> {
    let mut pts: Vec<(f64, f64)> = points.iter().map(|p| (p.x as f64, p.y as f64)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts
            .iter()
            .map(|&(x, y)| Point::new(x as f32, y as f32))
            .collect();
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
        .iter()
        .map(|&(x, y)| Point::new(x as f32, y as f32))
        .collect()
}

/// The rectangle of least area holding the points, for the splitter.
pub fn min_area_rect_of(points: &[Point<f32>]) -> [Point<f32>; 4] {
    min_area_rect(points)
}

/// The rectangle of least area holding the points, by rotating
/// calipers over the convex hull: one side of the rectangle lies along
/// a hull edge. Corners in hull order.
fn min_area_rect(points: &[Point<f32>]) -> [Point<f32>; 4] {
    let hull = convex_hull(points);
    if hull.is_empty() {
        return [Point::new(0.0, 0.0); 4];
    }
    if hull.len() < 3 {
        let (a, b) = (hull[0], *hull.last().unwrap());
        return [a, b, b, a];
    }
    let mut best: Option<(f64, [Point<f32>; 4])> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let (ex, ey) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let len = (ex * ex + ey * ey).sqrt();
        if len == 0.0 {
            continue;
        }
        let (ux, uy) = (ex / len, ey / len);
        let (vx, vy) = (-uy, ux);
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in &hull {
            let (px, py) = (p.x as f64, p.y as f64);
            let (u, v) = (px * ux + py * uy, px * vx + py * vy);
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let area = (u1 - u0) * (v1 - v0);
        if best.as_ref().is_none_or(|(a, _)| area < *a) {
            let corner =
                |u: f64, v: f64| Point::new((u * ux + v * vx) as f32, (u * uy + v * vy) as f32);
            best = Some((
                area,
                [
                    corner(u0, v0),
                    corner(u1, v0),
                    corner(u1, v1),
                    corner(u0, v1),
                ],
            ));
        }
    }
    best.map(|(_, r)| r).unwrap_or([hull[0]; 4])
}

/// Four points ordered clockwise from the top left, as the survey
/// orders them: the two highest by x, then the two lowest.
pub fn clockwise(mut pts: [(f32, f32); 4]) -> [(f32, f32); 4] {
    pts.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    let (mut top, mut bottom) = ([pts[0], pts[1]], [pts[2], pts[3]]);
    top.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    bottom.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    [top[0], top[1], bottom[1], bottom[0]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_map_preserves_geometry_and_rejects_invalid_data() {
        let mut data = vec![0.; 32 * 24];
        for y in 5..15 {
            for x in 3..25 {
                data[y * 32 + x] = 0.9;
            }
        }
        let map = ProbabilityMap::from_data(32, 24, data.clone(), 64, 48).unwrap();
        let bytes: Vec<_> = data.iter().flat_map(|p| p.to_le_bytes()).collect();
        let saved = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let replay = ProbabilityMap::from_data(32, 24, saved, 64, 48).unwrap();
        assert_eq!(map.data, replay.data);
        assert_eq!(map.quads(), replay.quads());
        for (w, h, d, pw, ph) in [
            (0, 1, vec![], 1, 1),
            (1, 1, vec![], 1, 1),
            (1, 1, vec![0.5], 0, 1),
            (1, 1, vec![f32::NAN], 1, 1),
            (1, 1, vec![1.1], 1, 1),
            (usize::MAX, 2, vec![], 1, 1),
        ] {
            assert!(ProbabilityMap::from_data(w, h, d, pw, ph).is_err());
        }
    }

    #[test]
    fn lower_threshold_connects_a_faint_bridge_but_keeps_region_scoring() {
        let mut map = ProbabilityMap {
            width: 32,
            height: 24,
            data: vec![0.0; 32 * 24],
            photo_width: 32,
            photo_height: 24,
        };
        for y in 5..15 {
            for x in 3..25 {
                map.data[y * map.width + x] = if (12..15).contains(&x) { 0.2 } else { 0.9 };
            }
        }
        assert_eq!(map.quads(), map.quads_with_threshold(THRESH).unwrap());
        assert_eq!(map.quads().len(), 2);
        assert_eq!(map.quads_with_threshold(0.1).unwrap().len(), 1);
        for p in &mut map.data {
            *p = p.min(0.4);
        }
        assert!(map.quads_with_threshold(0.1).unwrap().is_empty());
        for threshold in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(map.quads_with_threshold(threshold).is_err());
        }
    }

    #[test]
    fn a_bright_region_becomes_one_grown_quad() {
        let (width, height) = (128usize, 64usize);
        let mut data = vec![0.0f32; width * height];
        for y in 20..30 {
            for x in 30..90 {
                data[y * width + x] = 0.9;
            }
        }
        let map = ProbabilityMap {
            width,
            height,
            data,
            photo_width: 256,
            photo_height: 128,
        };
        let quads = map.quads();
        assert_eq!(quads.len(), 1);
        let xs: Vec<f32> = quads[0].0.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = quads[0].0.iter().map(|p| p.1).collect();
        // grown by area over perimeter times 1.5 on every side, then doubled by the photo scale
        assert!(xs.iter().cloned().fold(f32::MAX, f32::min) < 60.0);
        assert!(xs.iter().cloned().fold(f32::MIN, f32::max) > 180.0);
        assert!(ys.iter().cloned().fold(f32::MAX, f32::min) < 40.0);
        assert!(ys.iter().cloned().fold(f32::MIN, f32::max) > 60.0);
    }

    #[test]
    fn a_faint_region_is_dropped() {
        let (width, height) = (64usize, 64usize);
        let mut data = vec![0.0f32; width * height];
        for y in 10..20 {
            for x in 10..50 {
                data[y * width + x] = 0.4;
            }
        }
        let map = ProbabilityMap {
            width,
            height,
            data,
            photo_width: 64,
            photo_height: 64,
        };
        assert!(map.quads().is_empty());
    }

    #[test]
    fn the_least_rectangle_of_a_tilted_box_is_the_box() {
        let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
        let corners = [(0.0, 0.0), (100.0, 0.0), (100.0, 20.0), (0.0, 20.0)];
        let pts: Vec<Point<f32>> = corners
            .iter()
            .flat_map(|&(x, y)| [(x, y), (x + 0.5, y + 0.5)])
            .map(|(x, y): (f32, f32)| Point::new(x * c - y * s + 200.0, x * s + y * c + 200.0))
            .collect();
        let rect = min_area_rect(&pts);
        let side =
            |a: Point<f32>, b: Point<f32>| ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let (long, short) = (
            side(rect[0], rect[1]).max(side(rect[1], rect[2])),
            side(rect[0], rect[1]).min(side(rect[1], rect[2])),
        );
        assert!((long - 100.7).abs() < 1.0, "{long}");
        assert!((short - 20.7).abs() < 1.0, "{short}");
    }

    #[test]
    fn corners_come_back_clockwise_from_the_top_left() {
        let q = clockwise([(10.0, 20.0), (0.0, 0.0), (10.0, 0.0), (0.0, 20.0)]);
        assert_eq!(q, [(0.0, 0.0), (10.0, 0.0), (10.0, 20.0), (0.0, 20.0)]);
    }
}
