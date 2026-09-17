//! Which strip a bare strip is, from its printed title.
//!
//! Upstream prints `SEED PHRASE` or `SHARE 1`, `SHARE 2`, `SHARE 3` above
//! the rows in its label face. The whole title is matched against all
//! four, rendered in the bundled mono face, which tells the seed strip
//! from a share and turns away anything stretched, cut short or foreign;
//! which share is then the last glyph against the three digits,
//! normalised exactly like a scanned cell. No clear winner at either
//! gate means `Unknown`, never a guess: the recovery search treats an
//! unknown share number as reconstruction classes.

use std::sync::OnceLock;

use image::{GrayImage, Luma};
use penlock::ShareIndex;

use crate::cells::crop_cell;
use crate::locate::{Identity, RECTIFIED_PX_PER_MM, label_blobs, paper_white, trace};
use crate::model::{correlation, ink_vector};
use crate::template::{INK, LABEL_FONT, Rect, rasterize, svg_document, text};

/// Where the title is on upstream's strip: between the top and the first
/// row.
const TITLE_WINDOW: Rect = Rect {
    x: 3.0,
    y: 8.0,
    w: 45.0,
    h: 14.0,
};
/// Title ink is darker than this fraction of the paper.
const TITLE_INK: f64 = 0.6;
/// A blob smaller than this, in square millimetres, is a speck.
const SPECK_MM2: f64 = 0.25;
/// Narrower than this is no title.
const MIN_TITLE_WIDTH: f64 = 12.0;
/// The digit must correlate at least this well with its template, and
/// this much better than the runner-up.
const DIGIT_MATCH: f32 = 0.5;
const DIGIT_MARGIN: f32 = 0.1;
/// Size the digit templates are rendered at, in millimetres.
const TEMPLATE_SIZE: f64 = 5.0;

/// The title crop's size for matching, in pixels: wide, since the
/// shortest title is seven glyphs.
const TITLE_MATCH: (u32, u32) = (168, 32);
/// The whole title must correlate at least this well with its template,
/// and this much better than the best template of the other kind. Real
/// titles score 0.50–0.86 and synthetic ones from 0.64; a title with a
/// stray mark, a missing digit or a smear in its place scores under 0.2.
const TITLE_MATCH_MIN: f32 = 0.4;
const TITLE_MARGIN: f32 = 0.12;

/// Reads the printed title of a rectified strip without scan aids. Two
/// gates, both closed unless a template wins clearly: the whole title
/// against all four titles, which tells `SEED PHRASE` from `SHARE N` and
/// rejects a stretched, partial or foreign title; then, for a share, the
/// last glyph against the three digits.
pub fn read_title(rectified: &GrayImage, px_per_mm: f64) -> Identity {
    let paper = paper_white(rectified);
    let Some((blobs, span)) = title_ink(rectified, px_per_mm, paper) else {
        return Identity::Unknown;
    };
    let crop = matchable(rectified, span);
    let mut scores: Vec<(f32, &Title)> = titles().iter().map(|t| (t.score(&crop), t)).collect();
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, title) = scores[0];
    let other_kind = scores
        .iter()
        .filter(|(_, t)| t.seed != title.seed)
        .map(|(s, _)| *s)
        .fold(f32::NEG_INFINITY, f32::max);
    trace(format_args!(
        "title: best {:?} at {best:.3}, other kind {other_kind:.3}",
        title.text
    ));
    if best < TITLE_MATCH_MIN || best - other_kind < TITLE_MARGIN {
        return Identity::Unknown;
    }
    if title.seed {
        return Identity::Seed;
    }
    let digit = blobs
        .iter()
        .max_by_key(|b| b.max_x)
        .expect("a blob set the span");
    let rect = Rect {
        x: f64::from(digit.min_x) / px_per_mm,
        y: f64::from(digit.min_y) / px_per_mm,
        w: f64::from(digit.max_x - digit.min_x + 1) / px_per_mm,
        h: f64::from(digit.max_y - digit.min_y + 1) / px_per_mm,
    }
    .grow(0.4);
    let cell = crop_cell(rectified, px_per_mm, rect, [0.0; 4]);
    if cell.is_empty() {
        return Identity::Unknown;
    }
    let ink = ink_vector(&cell);
    let mut digits: Vec<(f32, u8)> = (1..=3u8)
        .map(|number| {
            let best = templates()
                .iter()
                .filter(|(n, _)| *n == number)
                .map(|(_, template)| correlation(&ink, template))
                .fold(f32::NEG_INFINITY, f32::max);
            (best, number)
        })
        .collect();
    digits.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, number) = digits[0];
    let runner_up = digits.get(1).map_or(f32::NEG_INFINITY, |s| s.0);
    trace(format_args!(
        "title: digit {number} at {best:.3}, runner-up {runner_up:.3}"
    ));
    if best < DIGIT_MATCH || best - runner_up < DIGIT_MARGIN {
        return Identity::Unknown;
    }
    ShareIndex::new(number).map_or(Identity::Unknown, Identity::Share)
}

/// The title's ink blobs in the window above the rows and the pixel
/// rectangle they span, if there is a title's worth of them.
fn title_ink(
    rectified: &GrayImage,
    px_per_mm: f64,
    paper: f64,
) -> Option<(Vec<crate::locate::Blob>, [u32; 4])> {
    let (x0, y0) = (
        (TITLE_WINDOW.x * px_per_mm) as u32,
        (TITLE_WINDOW.y * px_per_mm) as u32,
    );
    let (x1, y1) = (
        (((TITLE_WINDOW.x + TITLE_WINDOW.w) * px_per_mm) as u32).min(rectified.width()),
        (((TITLE_WINDOW.y + TITLE_WINDOW.h) * px_per_mm) as u32).min(rectified.height()),
    );
    let mut mask = GrayImage::new(rectified.width(), rectified.height());
    for y in y0..y1 {
        for x in x0..x1 {
            if f64::from(rectified.get_pixel(x, y).0[0]) < paper * TITLE_INK {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
    }
    let blobs: Vec<_> = label_blobs(&mask)
        .into_iter()
        .filter(|b| f64::from(b.area) >= SPECK_MM2 * px_per_mm * px_per_mm)
        .collect();
    let left = blobs.iter().map(|b| b.min_x).min()?;
    let right = blobs.iter().map(|b| b.max_x).max()?;
    let top = blobs.iter().map(|b| b.min_y).min()?;
    let bottom = blobs.iter().map(|b| b.max_y).max()?;
    if f64::from(right - left + 1) / px_per_mm < MIN_TITLE_WIDTH {
        return None;
    }
    Some((blobs, [left, top, right, bottom]))
}

/// The ink within `span` resampled to `TITLE_MATCH` as darkness, for
/// correlation: every title is stretched to the same box, so a title
/// stretched by a stray mark or cut short matches none of them.
fn matchable(image: &GrayImage, [left, top, right, bottom]: [u32; 4]) -> Vec<f32> {
    let crop =
        image::imageops::crop_imm(image, left, top, right - left + 1, bottom - top + 1).to_image();
    let small = image::imageops::resize(
        &crop,
        TITLE_MATCH.0,
        TITLE_MATCH.1,
        image::imageops::FilterType::Triangle,
    );
    small
        .pixels()
        .map(|p| 1.0 - f32::from(p.0[0]) / 255.0)
        .collect()
}

/// The image with every ink pixel spread to its eight neighbours: the
/// regular face set a stroke heavier, as upstream prints its titles.
fn emboldened(gray: &GrayImage) -> GrayImage {
    let mut out = gray.clone();
    for (x, y, p) in out.enumerate_pixels_mut() {
        let mut darkest = 255u8;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
                if nx >= 0
                    && ny >= 0
                    && nx < i64::from(gray.width())
                    && ny < i64::from(gray.height())
                {
                    darkest = darkest.min(gray.get_pixel(nx as u32, ny as u32).0[0]);
                }
            }
        }
        *p = Luma([darkest]);
    }
    out
}

/// One of the four titles rendered in the label face and made
/// matchable, at the face's own weight and set heavier.
struct Title {
    text: &'static str,
    seed: bool,
    weights: [Vec<f32>; 2],
}

impl Title {
    /// How well `crop` matches this title at whichever weight suits it.
    fn score(&self, crop: &[f32]) -> f32 {
        self.weights
            .iter()
            .map(|w| correlation(crop, w))
            .fold(f32::NEG_INFINITY, f32::max)
    }
}

fn titles() -> &'static [Title] {
    static TITLES: OnceLock<Vec<Title>> = OnceLock::new();
    TITLES.get_or_init(|| {
        [
            ("SEED PHRASE", true),
            ("SHARE 1", false),
            ("SHARE 2", false),
            ("SHARE 3", false),
        ]
        .into_iter()
        .map(|(text, seed)| {
            let (w, h) = (60.0, 12.0);
            let body = text_svg(w / 2.0, h / 2.0, TEMPLATE_SIZE, text);
            let png = rasterize(&svg_document(w, h, &body), RECTIFIED_PX_PER_MM * 25.4)
                .expect("a title renders");
            let gray = image::DynamicImage::ImageRgb8(png).into_luma8();
            let (_, span) = title_ink_anywhere(&gray, RECTIFIED_PX_PER_MM);
            let bold = emboldened(&gray);
            let (_, bold_span) = title_ink_anywhere(&bold, RECTIFIED_PX_PER_MM);
            Title {
                text,
                seed,
                weights: [matchable(&gray, span), matchable(&bold, bold_span)],
            }
        })
        .collect()
    })
}

/// The ink span of a rendered title, the whole image being its window.
fn title_ink_anywhere(gray: &GrayImage, px_per_mm: f64) -> (Vec<crate::locate::Blob>, [u32; 4]) {
    let mut mask = GrayImage::new(gray.width(), gray.height());
    for (x, y, p) in gray.enumerate_pixels() {
        if f64::from(p.0[0]) < 255.0 * TITLE_INK {
            mask.put_pixel(x, y, Luma([255]));
        }
    }
    let blobs: Vec<_> = label_blobs(&mask)
        .into_iter()
        .filter(|b| f64::from(b.area) >= SPECK_MM2 * px_per_mm * px_per_mm)
        .collect();
    let span = [
        blobs.iter().map(|b| b.min_x).min().unwrap_or(0),
        blobs.iter().map(|b| b.min_y).min().unwrap_or(0),
        blobs.iter().map(|b| b.max_x).max().unwrap_or(0),
        blobs.iter().map(|b| b.max_y).max().unwrap_or(0),
    ];
    (blobs, span)
}

fn text_svg(x: f64, cy: f64, size: f64, body: &str) -> String {
    text(x, cy, size, "middle", LABEL_FONT, INK, body)
}

/// The digits `1`, `2`, `3` in the label face, at its weight and set
/// heavier, normalised like a cell.
fn templates() -> &'static [(u8, Vec<f32>)] {
    static TEMPLATES: OnceLock<Vec<(u8, Vec<f32>)>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let side = TEMPLATE_SIZE * 2.0;
        (1..=3u8)
            .flat_map(|number| {
                let body = text_svg(side / 2.0, side / 2.0, TEMPLATE_SIZE, &number.to_string());
                let png = rasterize(&svg_document(side, side, &body), RECTIFIED_PX_PER_MM * 25.4)
                    .expect("a digit renders");
                let gray = image::DynamicImage::ImageRgb8(png).into_luma8();
                let whole = Rect {
                    x: 0.0,
                    y: 0.0,
                    w: side,
                    h: side,
                };
                [gray.clone(), emboldened(&gray)].into_iter().map(move |g| {
                    let cell = crop_cell(&g, RECTIFIED_PX_PER_MM, whole, [0.0; 4]);
                    (number, ink_vector(&cell))
                })
            })
            .collect()
    })
}
