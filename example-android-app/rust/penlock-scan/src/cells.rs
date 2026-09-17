//! Cutting the rectified card into one image per symbol.
//!
//! Every cell is normalised the same way so a recogniser sees the same
//! framing whatever the photo: the ink is found with a threshold relative
//! to the local paper white (which drops the grey cell borders and the
//! checksum shading), its bounding box is scaled to fit a fixed box and
//! centred, black on white with a margin.

use image::{GrayImage, Luma, imageops};

use crate::sheet::{Layout, Strip};
use crate::template::Rect;

/// Side of a normalised cell image, in pixels, matching the classifier input.
pub const CELL_SIZE: u32 = 128;
/// Side of the box the ink is scaled to fit inside the cell image.
pub const INK_BOX: u32 = 51;
/// Side of the image handed to per-cell models.
pub const MODEL_SIDE: u32 = 128;
/// Ink is anything darker than this fraction of the local paper white.
/// The crop's inset already excludes the printed borders (≈0.71 of white),
/// and the checksum shading (≈0.9) stays above it, so it can sit high
/// enough that a thin, blurred stroke still counts.
const INK_FRACTION_OF_WHITE: f64 = 0.7;
/// Below this share of ink pixels a cell counts as empty; it absorbs
/// noise and stray border pixels.
pub(crate) const EMPTY_BELOW: f64 = 0.003;
/// How far a grown window reaches past the box, millimetres: writers
/// overflow, and the old insets amputated what they wrote (a `Q`
/// losing its measured bottom, whole rows their tops).
pub(crate) const GROW_SIDE: f64 = 0.3;
// Writers overshoot upward — capitals and ascenders leave the box at
// the top far more than strokes trail out the bottom.
pub(crate) const GROW_ABOVE: f64 = 2.0;
pub(crate) const GROW_BELOW: f64 = 0.55;
/// A component that never reaches this far inside the box is furniture
/// or a neighbour's overhang, not this cell's ink.
const INTERIOR_INSET: f64 = 0.35;
/// Grown-window components smaller than this are specks, wherever they
/// sit: no symbol has a part this small, but one noise dot stretches
/// the normalisation's bounding box and displaces the whole glyph.
const SPECK_MM2: f64 = 0.2;

/// One symbol's cell, normalised: `CELL_SIZE` square, ink black on white.
#[derive(Clone, PartialEq, Debug)]
pub struct CellImage {
    /// The normalised pixels, 0 = ink, 255 = paper.
    pub pixels: GrayImage,
    /// Share of the cropped cell that was ink, before normalisation.
    pub ink_fraction: f64,
}

impl CellImage {
    /// Whether the cell had nothing written in it.
    pub fn is_empty(&self) -> bool {
        self.ink_fraction < EMPTY_BELOW
    }

    /// The cell as the ONNX contract expects it: `MODEL_SIDE × MODEL_SIDE`
    /// (a `1×1×32×32` tensor), row-major, ink white on black in `0..=1`,
    /// downsampled from the canonical image.
    pub fn model_input(&self) -> Vec<f32> {
        let small = imageops::resize(
            &self.pixels,
            MODEL_SIDE,
            MODEL_SIDE,
            imageops::FilterType::Triangle,
        );
        small
            .pixels()
            .map(|p| 1.0 - f32::from(p.0[0]) / 255.0)
            .collect()
    }
}

/// Cuts every row of a rectified strip into cells, from the boxes
/// `layout` prints.
pub fn crop_cells(rectified: &GrayImage, px_per_mm: f64, layout: Layout) -> Vec<[CellImage; 6]> {
    (0..Strip::ROWS)
        .map(|word| {
            std::array::from_fn(|position| {
                cell_image(rectified, px_per_mm, layout, word, position, PRODUCTION)
            })
        })
        .collect()
}

/// One cell under `recipe`: its window cropped, its ink decided, and —
/// grown windows only — furniture and neighbour overhang dropped by
/// the interior rule, then normalised like every cell.
pub(crate) fn cell_image(
    rectified: &GrayImage,
    px_per_mm: f64,
    layout: Layout,
    word: usize,
    position: usize,
    recipe: Recipe,
) -> CellImage {
    let (mask, w, h) = cell_mask(rectified, px_per_mm, layout, word, position, recipe);
    normalised(&mask, w, h)
}

/// The window `recipe` reads a cell through, in millimetres.
pub fn cell_window(layout: Layout, word: usize, position: usize, window: Window) -> Rect {
    let cell = layout.cell_rect(word, position);
    match window {
        Window::Inset => {
            let inset = layout.crop_inset();
            Rect {
                x: cell.x + inset[3],
                y: cell.y + inset[0],
                w: cell.w - inset[1] - inset[3],
                h: cell.h - inset[0] - inset[2],
            }
        }
        Window::Grown => Rect {
            x: cell.x - GROW_SIDE,
            y: cell.y - GROW_ABOVE,
            w: cell.w + 2.0 * GROW_SIDE,
            h: cell.h + GROW_ABOVE + GROW_BELOW,
        },
    }
}
/// The pixel bounds of a window on a rectified strip, with
/// production's rounding and clamping: an instrument that crops what
/// the pipeline crops must come through here.
pub fn window_bounds(rectified: &GrayImage, px_per_mm: f64, window: Rect) -> (u32, u32, u32, u32) {
    let x0 = (window.x * px_per_mm).round().max(0.0) as u32;
    let y0 = (window.y * px_per_mm).round().max(0.0) as u32;
    let x1 = (((window.x + window.w) * px_per_mm).round() as u32).min(rectified.width());
    let y1 = (((window.y + window.h) * px_per_mm).round() as u32).min(rectified.height());
    (x0, y0, x1, y1)
}

/// A cell's ink mask under `recipe`, row-major, with its size.
pub(crate) fn cell_mask(
    rectified: &GrayImage,
    px_per_mm: f64,
    layout: Layout,
    word: usize,
    position: usize,
    recipe: Recipe,
) -> (Vec<bool>, usize, usize) {
    let window = cell_window(layout, word, position, recipe.window);
    let (x0, y0, x1, y1) = window_bounds(rectified, px_per_mm, window);
    let crop = imageops::crop_imm(
        rectified,
        x0,
        y0,
        x1.saturating_sub(x0),
        y1.saturating_sub(y0),
    )
    .to_image();
    let cell = layout.cell_rect(word, position);
    let inset = layout.crop_inset();
    let rx0 = (((cell.x + inset[3]) * px_per_mm).round().max(0.0) as u32).max(x0);
    let ry0 = (((cell.y + inset[0]) * px_per_mm).round().max(0.0) as u32).max(y0);
    let rx1 = ((((cell.x + cell.w - inset[1]) * px_per_mm).round() as u32).min(x1)).max(rx0 + 1);
    let ry1 = ((((cell.y + cell.h - inset[2]) * px_per_mm).round() as u32).min(y1)).max(ry0 + 1);
    let reference = imageops::crop_imm(rectified, rx0, ry0, rx1 - rx0, ry1 - ry0).to_image();
    let mut mask = recipe.mask(&crop, &reference);
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    if recipe.window == Window::Grown && layout == Layout::Upstream {
        // The strip's own dotted line sits on the box's bottom edge;
        // under hard blur its dashes bridge to nearby glyph ink and
        // ride into the kept component, so the line's band is cleared
        // before components form.
        let band0 = (((cell.y + cell.h - 0.05) * px_per_mm) - f64::from(y0)).max(0.0) as usize;
        let band1 = ((((cell.y + cell.h + 0.20) * px_per_mm) - f64::from(y0)) as usize).min(h);
        for y in band0..band1 {
            for x in 0..w {
                mask[y * w + x] = false;
            }
        }
    }
    if recipe.window == Window::Grown {
        let interior = Rect {
            x: cell.x + INTERIOR_INSET,
            y: cell.y + INTERIOR_INSET,
            w: cell.w - 2.0 * INTERIOR_INSET,
            h: cell.h - 2.0 * INTERIOR_INSET,
        };
        let ix0 = ((interior.x * px_per_mm) - f64::from(x0)).max(0.0) as usize;
        let iy0 = ((interior.y * px_per_mm) - f64::from(y0)).max(0.0) as usize;
        let ix1 = ((((interior.x + interior.w) * px_per_mm) - f64::from(x0)) as usize).min(w);
        let iy1 = ((((interior.y + interior.h) * px_per_mm) - f64::from(y0)) as usize).min(h);
        let speck_floor = (SPECK_MM2 * px_per_mm * px_per_mm) as usize;
        mask = interior_components(mask, w, h, (ix0, iy0, ix1, iy1), speck_floor);
    }
    (mask, w, h)
}

/// Keeps only components with at least one pixel inside the interior:
/// printed borders, dotted-line dashes and a neighbour's overhang all
/// live at the edges and never reach it, while a glyph that overflows
/// the box is connected to its own interior ink.
fn interior_components(
    mask: Vec<bool>,
    w: usize,
    h: usize,
    (ix0, iy0, ix1, iy1): (usize, usize, usize, usize),
    speck_floor: usize,
) -> Vec<bool> {
    let mut keep = vec![false; w * h];
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut component = vec![start];
        let mut queue = vec![start];
        seen[start] = true;
        let mut interior = false;
        while let Some(i) = queue.pop() {
            let (x, y) = (i % w, i / w);
            if x >= ix0 && x < ix1 && y >= iy0 && y < iy1 {
                interior = true;
            }
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < w && ny < h && mask[ny * w + nx] && !seen[ny * w + nx] {
                    seen[ny * w + nx] = true;
                    queue.push(ny * w + nx);
                    component.push(ny * w + nx);
                }
            }
        }
        if interior && component.len() >= speck_floor {
            for i in component {
                keep[i] = true;
            }
        }
    }
    keep
}

/// A mask normalised into a [`CellImage`]: ink bounding box scaled to
/// [`INK_BOX`], centred, black on white.
fn normalised(mask: &[bool], w: usize, h: usize) -> CellImage {
    let mut image = GrayImage::from_pixel(w.max(1) as u32, h.max(1) as u32, Luma([255]));
    let mut ink = 0u32;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
    for (i, is_ink) in mask.iter().enumerate() {
        if *is_ink {
            let (x, y) = ((i % w.max(1)) as u32, (i / w.max(1)) as u32);
            image.put_pixel(x, y, Luma([0]));
            ink += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    let ink_fraction = f64::from(ink) / (w.max(1) * h.max(1)) as f64;
    let mut pixels = GrayImage::from_pixel(CELL_SIZE, CELL_SIZE, Luma([255]));
    if ink_fraction >= EMPTY_BELOW {
        let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
        let scale = f64::from(INK_BOX) / f64::from(bw.max(bh));
        let (tw, th) = (
            ((f64::from(bw) * scale).round() as u32).max(1),
            ((f64::from(bh) * scale).round() as u32).max(1),
        );
        let glyph = imageops::crop_imm(&image, min_x, min_y, bw, bh).to_image();
        let scaled = imageops::resize(&glyph, tw, th, imageops::FilterType::Triangle);
        let (ox, oy) = ((CELL_SIZE - tw) / 2, (CELL_SIZE - th) / 2);
        imageops::replace(&mut pixels, &scaled, i64::from(ox), i64::from(oy));
    }
    CellImage {
        pixels,
        ink_fraction,
    }
}

/// Cuts one box of a rectified strip into a normalised cell, starting
/// `inset` millimetres — top, right, bottom, left — inside its edges.
pub fn crop_cell(rectified: &GrayImage, px_per_mm: f64, cell: Rect, inset: [f64; 4]) -> CellImage {
    let inner = Rect {
        x: cell.x + inset[3],
        y: cell.y + inset[0],
        w: cell.w - inset[1] - inset[3],
        h: cell.h - inset[0] - inset[2],
    };
    let x0 = (inner.x * px_per_mm).round().max(0.0) as u32;
    let y0 = (inner.y * px_per_mm).round().max(0.0) as u32;
    let x1 = (((inner.x + inner.w) * px_per_mm).round() as u32).min(rectified.width());
    let y1 = (((inner.y + inner.h) * px_per_mm).round() as u32).min(rectified.height());
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    let crop = imageops::crop_imm(rectified, x0, y0, w, h).to_image();

    let ink_mask = PRODUCTION.mask(&crop, &crop);
    let mut mask = GrayImage::from_pixel(w.max(1), h.max(1), Luma([255]));
    let mut ink = 0u32;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, _) in crop.enumerate_pixels() {
        if ink_mask[(y * w.max(1) + x) as usize] {
            mask.put_pixel(x, y, Luma([0]));
            ink += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    let ink_fraction = f64::from(ink) / f64::from(w.max(1) * h.max(1));
    let mut pixels = GrayImage::from_pixel(CELL_SIZE, CELL_SIZE, Luma([255]));
    if ink_fraction >= EMPTY_BELOW {
        let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
        let scale = f64::from(INK_BOX) / f64::from(bw.max(bh));
        let (tw, th) = (
            ((f64::from(bw) * scale).round() as u32).max(1),
            ((f64::from(bh) * scale).round() as u32).max(1),
        );
        let glyph = imageops::crop_imm(&mask, min_x, min_y, bw, bh).to_image();
        let scaled = imageops::resize(&glyph, tw, th, imageops::FilterType::Triangle);
        let (ox, oy) = ((CELL_SIZE - tw) / 2, (CELL_SIZE - th) / 2);
        imageops::replace(&mut pixels, &scaled, i64::from(ox), i64::from(oy));
    }
    CellImage {
        pixels,
        ink_fraction,
    }
}

/// The rule that decides ink, one of the candidates the fidelity
/// harness measures; `PRODUCTION` is the one `crop_cell` uses, so the
/// harness scores exactly what production does.
pub(crate) const PRODUCTION: Recipe = Recipe {
    binariser: Binariser::Otsu,
    close: false,
    despeckle: true,
    window: Window::Grown,
};

/// How ink is separated from paper in one cropped cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Binariser {
    /// Darker than 0.7 of the cell's 95th-percentile white.
    Fraction,
    /// The threshold at the histogram's between-class optimum, with a
    /// contrast guard: no ink unless the two classes are truly apart,
    /// or blank noise would be split into phantom ink.
    Otsu,
    /// A per-pixel threshold from the neighbourhood mean and deviation
    /// (Sauvola), for gradients inside one cell.
    Sauvola,
    /// A strict seed grown through a permissive bound, so faint stroke
    /// interiors connect without letting shading in.
    Hysteresis,
}

/// Where a cell's pixels come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Window {
    /// Inside the printed box, inset from its edges: never sees
    /// furniture, and never sees what the writer overflowed.
    Inset,
    /// The box grown by the overflow allowance; furniture and
    /// neighbour overhang are dropped by the interior rule.
    Grown,
}

/// A binariser plus its post-operations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Recipe {
    pub binariser: Binariser,
    /// Close single-pixel holes (3×3 dilate then erode).
    pub close: bool,
    /// Drop small components touching the crop edge: a neighbour's
    /// overhanging stroke, not this cell's ink.
    pub despeckle: bool,
    /// The crop window.
    pub window: Window,
}

impl Recipe {
    /// Parses `fraction`, `otsu`, `sauvola` or `hysteresis`, each
    /// optionally suffixed `+close` and/or `+despeckle`.
    pub(crate) fn parse(spec: &str) -> Option<Recipe> {
        let mut parts = spec.split('+');
        let binariser = match parts.next()? {
            "fraction" => Binariser::Fraction,
            "otsu" => Binariser::Otsu,
            "sauvola" => Binariser::Sauvola,
            "hysteresis" => Binariser::Hysteresis,
            _ => return None,
        };
        let mut recipe = Recipe {
            binariser,
            close: false,
            despeckle: false,
            window: Window::Inset,
        };
        for part in parts {
            match part {
                "close" => recipe.close = true,
                "despeckle" => recipe.despeckle = true,
                "grown" => recipe.window = Window::Grown,
                _ => return None,
            }
        }
        Some(recipe)
    }

    /// The ink mask of a cropped cell, row-major.
    /// Thresholds are estimated from `reference` (the printed box's
    /// own pixels) and applied to the whole crop: a grown window's
    /// margins hold borders, dotted lines and neighbours, and letting
    /// them into a global histogram poisons the threshold.
    pub(crate) fn mask(self, crop: &GrayImage, reference: &GrayImage) -> Vec<bool> {
        let (w, h) = (crop.width() as usize, crop.height() as usize);
        let mut mask = match self.binariser {
            Binariser::Fraction => scaled_threshold_mask(
                crop,
                f64::from(percentile(reference, 0.95)) * INK_FRACTION_OF_WHITE,
                reference,
            ),
            Binariser::Otsu => otsu_mask(crop, reference),
            Binariser::Sauvola => sauvola_mask(crop),
            Binariser::Hysteresis => hysteresis_mask(crop, reference),
        };
        if self.close {
            mask = closed(&mask, w, h);
        }
        if self.despeckle {
            mask = despeckled(mask, w, h);
        }
        mask
    }
}

/// Otsu's threshold with a contrast guard: the split must separate the
/// class means by at least 0.12 of the white level, or a blank cell's
/// noise would be halved into phantom ink.
fn otsu_mask(crop: &GrayImage, reference: &GrayImage) -> Vec<bool> {
    let mut histogram = [0u32; 256];
    for p in reference.pixels() {
        histogram[usize::from(p.0[0])] += 1;
    }
    let total = reference.pixels().len() as f64;
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(v, c)| v as f64 * f64::from(*c))
        .sum();
    let (mut best, mut threshold) = (0.0f64, None);
    let (mut weight_b, mut sum_b) = (0.0f64, 0.0f64);
    let white = f64::from(percentile(reference, 0.95));
    for (value, count) in histogram.iter().enumerate() {
        weight_b += f64::from(*count);
        if weight_b == 0.0 {
            continue;
        }
        let weight_f = total - weight_b;
        if weight_f == 0.0 {
            break;
        }
        sum_b += value as f64 * f64::from(*count);
        let mean_b = sum_b / weight_b;
        let mean_f = (sum - sum_b) / weight_f;
        let variance = weight_b * weight_f * (mean_b - mean_f).powi(2);
        if variance > best {
            best = variance;
            threshold = (mean_f - mean_b >= 0.12 * white).then_some(value as u8);
        }
    }
    match threshold {
        Some(t) => scaled_threshold_mask(crop, f64::from(t), reference),
        None => vec![false; crop.pixels().len()],
    }
}

/// Applies `threshold` with each row rescaled by its own paper white
/// against the reference's: vignetting darkens a grown window's far
/// margins below a flat threshold, and the resulting ink ring merges
/// with any edge-touching stroke and swallows the cell.
fn scaled_threshold_mask(crop: &GrayImage, threshold: f64, reference: &GrayImage) -> Vec<bool> {
    let reference_white = f64::from(percentile(reference, 0.95)).max(1.0);
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    let mut mask = Vec::with_capacity(w * h);
    for y in 0..h {
        let mut row: Vec<u8> = (0..w)
            .map(|x| crop.get_pixel(x as u32, y as u32).0[0])
            .collect();
        row.sort_unstable();
        let row_white = f64::from(row[(w.saturating_sub(1)) * 95 / 100]);
        let scaled = threshold * (row_white / reference_white).clamp(0.5, 1.2);
        for x in 0..w {
            mask.push(f64::from(crop.get_pixel(x as u32, y as u32).0[0]) < scaled);
        }
    }
    mask
}

/// Sauvola's local threshold: window 31, k 0.2, R 128, via integral
/// images.
fn sauvola_mask(crop: &GrayImage) -> Vec<bool> {
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut sums = vec![0f64; (w + 1) * (h + 1)];
    let mut squares = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let v = f64::from(crop.get_pixel(x as u32, y as u32).0[0]);
            sums[(y + 1) * (w + 1) + x + 1] =
                v + sums[y * (w + 1) + x + 1] + sums[(y + 1) * (w + 1) + x] - sums[y * (w + 1) + x];
            squares[(y + 1) * (w + 1) + x + 1] =
                v * v + squares[y * (w + 1) + x + 1] + squares[(y + 1) * (w + 1) + x]
                    - squares[y * (w + 1) + x];
        }
    }
    let half = 15isize;
    let (k, r) = (0.2, 128.0);
    let mut mask = Vec::with_capacity(w * h);
    for y in 0..h as isize {
        for x in 0..w as isize {
            let (x0, y0) = ((x - half).max(0) as usize, (y - half).max(0) as usize);
            let (x1, y1) = (
                ((x + half + 1).min(w as isize)) as usize,
                ((y + half + 1).min(h as isize)) as usize,
            );
            let area = ((x1 - x0) * (y1 - y0)) as f64;
            let total = sums[y1 * (w + 1) + x1] - sums[y0 * (w + 1) + x1] - sums[y1 * (w + 1) + x0]
                + sums[y0 * (w + 1) + x0];
            let total_sq = squares[y1 * (w + 1) + x1]
                - squares[y0 * (w + 1) + x1]
                - squares[y1 * (w + 1) + x0]
                + squares[y0 * (w + 1) + x0];
            let mean = total / area;
            let deviation = (total_sq / area - mean * mean).max(0.0).sqrt();
            let threshold = mean * (1.0 + k * (deviation / r - 1.0));
            mask.push(f64::from(crop.get_pixel(x as u32, y as u32).0[0]) < threshold);
        }
    }
    mask
}

/// Strict seeds (0.65 of white) grown through a permissive bound (0.85
/// of white), 4-connected.
fn hysteresis_mask(crop: &GrayImage, reference: &GrayImage) -> Vec<bool> {
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    let white = f64::from(percentile(reference, 0.95));
    let strong = scaled_threshold_mask(crop, white * 0.65, reference);
    let weak = scaled_threshold_mask(crop, white * 0.85, reference);
    let mut mask = vec![false; w * h];
    let mut queue = Vec::new();
    for (x, y, _) in crop.enumerate_pixels() {
        if strong[y as usize * w + x as usize] {
            let i = y as usize * w + x as usize;
            mask[i] = true;
            queue.push((x as usize, y as usize));
        }
    }
    while let Some((x, y)) = queue.pop() {
        for (nx, ny) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if nx < w && ny < h && !mask[ny * w + nx] && weak[ny * w + nx] {
                mask[ny * w + nx] = true;
                queue.push((nx, ny));
            }
        }
    }
    mask
}

fn closed(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let neighbourhood = |m: &[bool], x: usize, y: usize, any: bool| -> bool {
        let mut all = true;
        let mut some = false;
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                let v = nx >= 0
                    && ny >= 0
                    && (nx as usize) < w
                    && (ny as usize) < h
                    && m[ny as usize * w + nx as usize];
                all &= v;
                some |= v;
            }
        }
        if any { some } else { all }
    };
    let mut dilated = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            dilated[y * w + x] = neighbourhood(mask, x, y, true);
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = neighbourhood(&dilated, x, y, false);
        }
    }
    out
}

/// Drops connected components that touch the crop edge and are smaller
/// than 1.5 mm² at the rectified scale: an overhanging neighbour
/// stroke, not this cell's ink.
fn despeckled(mut mask: Vec<bool>, w: usize, h: usize) -> Vec<bool> {
    let cap =
        (1.5 * crate::locate::RECTIFIED_PX_PER_MM * crate::locate::RECTIFIED_PX_PER_MM) as usize;
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut component = vec![start];
        let mut queue = vec![start];
        seen[start] = true;
        let mut touches_edge = false;
        while let Some(i) = queue.pop() {
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                touches_edge = true;
            }
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < w && ny < h && mask[ny * w + nx] && !seen[ny * w + nx] {
                    seen[ny * w + nx] = true;
                    queue.push(ny * w + nx);
                    component.push(ny * w + nx);
                }
            }
        }
        if touches_edge && component.len() < cap {
            for i in component {
                mask[i] = false;
            }
        }
    }
    mask
}

fn percentile(image: &GrayImage, fraction: f64) -> u8 {
    let mut histogram = [0u32; 256];
    for p in image.pixels() {
        histogram[usize::from(p.0[0])] += 1;
    }
    let total = image.pixels().len() as f64;
    let target = (total * fraction) as u32;
    let mut seen = 0;
    for (value, count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= target {
            return value as u8;
        }
    }
    255
}
