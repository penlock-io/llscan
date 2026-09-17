//! The recovery phrase sheet: a printed page with twelve fields for a
//! phrase, one word per field, laid out so a phone can find the page
//! by its marks and read each field by its place instead of finding
//! words in a photo. [`WordSheet`] is the one contract: it says where
//! the aids, the identity marks and the twelve writable interiors are,
//! the renderer draws from it, and the locator will read from it. The
//! frame and number printed beside a field sit outside its interior by
//! a quiet margin, so an interior holds handwriting and nothing else.
//!
//! Coordinates are millimetres in the sheet's own frame, origin at the
//! frame's top-left corner; the frame sits centred on A4 or Letter.
//! Nothing secret is ever printed.

use image::{GrayImage, RgbImage};
use penlock::Word;
use rand::{Rng, SeedableRng};

use crate::detect::{Quad, clockwise};
use crate::locate::{
    Geometry, Located, RECTIFIED_PX_PER_MM, locate_all_with, paper_white, read_field,
};
use crate::marks::{Field, WORD_SHEET_ID, decode_with, encode_with};
use crate::numbering::Numbering;
use crate::phrase::{PageScan, Reader, WordBox, read_crop};
use crate::recogniser::Recogniser;
use crate::template::{
    Font, GUIDE, INK, Jitter, LABEL_FONT, Rect, RenderError, TEXT, WORDS, rasterize, svg_document,
    text,
};

/// The sheet's version, printed in its identity marks.
pub const VERSION: u8 = 1;

/// The page frame every mark and field is placed in, millimetres.
pub const FRAME: (f64, f64) = (190.0, 260.0);

/// A paper size the frame is printed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paper {
    /// 210 × 297 mm.
    A4,
    /// 8.5 × 11 inches.
    Letter,
}

impl Paper {
    /// Every paper size.
    pub const ALL: [Paper; 2] = [Paper::A4, Paper::Letter];

    /// The command-line name.
    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "a4",
            Paper::Letter => "letter",
        }
    }

    /// Looks a paper up by name.
    pub fn from_name(name: &str) -> Option<Paper> {
        Paper::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Width and height in millimetres.
    pub fn size(self) -> (f64, f64) {
        match self {
            Paper::A4 => (210.0, 297.0),
            Paper::Letter => (215.9, 279.4),
        }
    }

    /// Where the frame's origin lies on the paper: centred.
    pub fn frame_origin(self) -> (f64, f64) {
        let (w, h) = self.size();
        ((w - FRAME.0) / 2.0, (h - FRAME.1) / 2.0)
    }
}

/// The word sheet's geometry.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WordSheet;

impl WordSheet {
    /// Side of the corner fiducials: the worksheet's, so the same blob
    /// search finds them.
    pub const FIDUCIAL: f64 = 7.0;
    /// Fiducial inset from the frame's edge.
    pub const INSET: f64 = 4.0;
    /// Clearance kept free of other ink around fiducials and marks.
    pub const QUIET: f64 = 2.0;
    /// Side of a mark-field slot: larger than a strip's, since the
    /// sheet is photographed from further away.
    pub const SLOT: f64 = 2.4;
    /// Fields down the page.
    pub const FIELDS: usize = WORDS;
    /// Top of the first field's interior.
    pub const FIELDS_TOP: f64 = 40.0;
    /// Pitch between fields.
    pub const FIELD_PITCH: f64 = 17.0;
    /// A field's writable interior.
    pub const INTERIOR: (f64, f64) = (128.0, 12.0);
    /// Left edge of the interiors.
    pub const INTERIOR_LEFT: f64 = 38.0;
    /// The printed frame sits this far outside the interior, and the
    /// number this far left of the frame.
    pub const MARGIN: f64 = 1.0;

    /// The whole frame.
    pub fn bounds(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: FRAME.0,
            h: FRAME.1,
        }
    }

    /// The four fiducials: top-left, top-right, bottom-left, bottom-right.
    pub fn fiducials(&self) -> [Rect; 4] {
        let far_x = FRAME.0 - WordSheet::INSET - WordSheet::FIDUCIAL;
        let far_y = FRAME.1 - WordSheet::INSET - WordSheet::FIDUCIAL;
        let at = |x, y| Rect {
            x,
            y,
            w: WordSheet::FIDUCIAL,
            h: WordSheet::FIDUCIAL,
        };
        [
            at(WordSheet::INSET, WordSheet::INSET),
            at(far_x, WordSheet::INSET),
            at(WordSheet::INSET, far_y),
            at(far_x, far_y),
        ]
    }

    /// The orientation mark beside the top-left fiducial.
    pub fn mark(&self) -> Rect {
        Rect {
            x: 15.5,
            y: 5.5,
            w: 4.0,
            h: 4.0,
        }
    }

    /// The sixteen mark-field slots, laid out as the strip's are
    /// (docs/marks.md): eight between the orientation mark and the
    /// top-right fiducial, eight between the bottom fiducials, left to
    /// right, [`WordSheet::QUIET`] clear of the dark squares flanking
    /// them; slot `i` carries bit `i` of the printed field.
    pub fn mark_slots(&self) -> [Rect; 16] {
        let side = WordSheet::SLOT;
        let row = |from: f64, to: f64, centre: f64, i: usize| Rect {
            x: from + (to - from - side) / 7.0 * i as f64,
            y: centre - side / 2.0,
            w: side,
            h: side,
        };
        let mark = self.mark();
        let [tl, tr, bl, br] = self.fiducials();
        std::array::from_fn(|i| {
            if i < 8 {
                row(
                    mark.x + mark.w + WordSheet::QUIET,
                    tr.x - WordSheet::QUIET - side,
                    tl.y + tl.h / 2.0,
                    i,
                )
            } else {
                row(
                    bl.x + bl.w + WordSheet::QUIET,
                    br.x - WordSheet::QUIET - side,
                    bl.y + bl.h / 2.0,
                    i - 8,
                )
            }
        })
    }

    /// Where the version is printed: under the orientation mark, out
    /// of every mark's way.
    pub fn version_anchor(&self) -> (f64, f64) {
        let mark = self.mark();
        (mark.x, mark.y + mark.h + 3.0)
    }

    /// The printed identity: the word sheet's id byte and its version.
    pub fn field(&self) -> u16 {
        encode_with(WORD_SHEET_ID, VERSION)
    }

    /// The writable interiors, in phrase order.
    pub fn fields(&self) -> [Rect; WordSheet::FIELDS] {
        std::array::from_fn(|i| Rect {
            x: WordSheet::INTERIOR_LEFT,
            y: WordSheet::FIELDS_TOP + WordSheet::FIELD_PITCH * i as f64,
            w: WordSheet::INTERIOR.0,
            h: WordSheet::INTERIOR.1,
        })
    }

    /// The printed frame around field `i`, just outside its interior.
    pub fn frame(&self, i: usize) -> Rect {
        self.fields()[i].grow(WordSheet::MARGIN)
    }

    /// Where field `i`'s number is printed: right-aligned, left of the
    /// frame.
    pub fn number_anchor(&self, i: usize) -> (f64, f64) {
        let f = self.frame(i);
        (f.x - WordSheet::MARGIN - 1.0, f.y + f.h / 2.0)
    }

    /// The title's centre.
    pub fn title_center(&self) -> (f64, f64) {
        (FRAME.0 / 2.0, 22.0)
    }

    /// The instruction line's centre.
    pub fn hint_center(&self) -> (f64, f64) {
        (FRAME.0 / 2.0, 30.0)
    }
}

/// Words to write into the fields, for fixtures: `None` leaves a
/// field blank.
pub struct WordFill {
    /// One entry per field.
    pub words: Vec<Option<Word>>,
    /// The handwriting font.
    pub font: Font,
    /// Placement variance and its seed.
    pub jitter: Option<(Jitter, u64)>,
}

fn rect(s: &mut String, r: Rect, style: &str) {
    s.push_str(&format!(
        "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" {style}/>\n",
        r.x, r.y, r.w, r.h
    ));
}

/// The sheet's SVG elements in the frame's coordinates, blank or with
/// `fill`'s words, without the `<svg>` wrapper.
pub fn frame_group(fill: Option<&WordFill>) -> String {
    let t = WordSheet;
    let mut s = String::new();
    rect(&mut s, t.bounds(), "fill=\"#ffffff\"");
    for f in t.fiducials() {
        rect(&mut s, f, &format!("fill=\"{INK}\""));
    }
    rect(&mut s, t.mark(), &format!("fill=\"{INK}\""));
    let field = t.field();
    for (i, slot) in t.mark_slots().iter().enumerate() {
        if field >> i & 1 == 1 {
            rect(&mut s, *slot, &format!("fill=\"{INK}\""));
        } else {
            rect(
                &mut s,
                *slot,
                &format!("fill=\"none\" stroke=\"{GUIDE}\" stroke-width=\"0.2\""),
            );
        }
    }
    let (tx, ty) = t.title_center();
    s.push_str(&text(
        tx,
        ty,
        6.0,
        "middle",
        LABEL_FONT,
        INK,
        "Recovery phrase sheet",
    ));
    let (hx, hy) = t.hint_center();
    s.push_str(&text(
        hx,
        hy,
        3.2,
        "middle",
        LABEL_FONT,
        TEXT,
        "One word per field, lowercase, inside the frame, in pen. Keep this sheet as secret as the words.",
    ));
    let (vx, vy) = t.version_anchor();
    s.push_str(&text(
        vx,
        vy,
        2.6,
        "start",
        LABEL_FONT,
        TEXT,
        &format!("v{VERSION}"),
    ));
    for i in 0..WordSheet::FIELDS {
        rect(
            &mut s,
            t.frame(i),
            &format!("fill=\"none\" stroke=\"{GUIDE}\" stroke-width=\"0.3\""),
        );
        let (nx, ny) = t.number_anchor(i);
        s.push_str(&text(
            nx,
            ny,
            3.6,
            "end",
            LABEL_FONT,
            TEXT,
            &format!("{}.", i + 1),
        ));
    }
    if let Some(fill) = fill {
        let mut rng = fill
            .jitter
            .map(|(jitter, seed)| (jitter, rand::rngs::StdRng::seed_from_u64(seed)));
        for (i, word) in fill.words.iter().enumerate().take(WordSheet::FIELDS) {
            let Some(word) = word else { continue };
            let interior = t.fields()[i];
            // a large hand: the word starts a little in from the left
            // and its letters stand most of the interior's height
            let base = interior.h * 0.62;
            let (cx, cy) = (interior.x + 4.0, interior.y + interior.h / 2.0);
            let (dx, dy, size, angle) = match &mut rng {
                Some((j, rng)) => (
                    rng.random_range(-j.offset_mm..=j.offset_mm),
                    rng.random_range(-j.offset_mm..=j.offset_mm),
                    base * (1.0 + rng.random_range(-j.size..=j.size)),
                    rng.random_range(-j.rotation_deg..=j.rotation_deg),
                ),
                None => (0.0, 0.0, base, 0.0),
            };
            let glyphs = text(
                cx + dx,
                cy + dy,
                size,
                "start",
                fill.font.family(),
                INK,
                word.as_str(),
            );
            if angle == 0.0 {
                s.push_str(&glyphs);
            } else {
                s.push_str(&format!(
                    "<g transform=\"rotate({angle:.2} {:.2} {:.2})\">{glyphs}</g>\n",
                    cx + dx,
                    cy + dy
                ));
            }
        }
    }
    s
}

/// The sheet as a self-contained SVG on `paper`, blank or filled.
pub fn sheet_svg(paper: Paper, fill: Option<&WordFill>) -> String {
    let (w, h) = paper.size();
    let (ox, oy) = paper.frame_origin();
    let body = format!(
        "<rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>\n<g transform=\"translate({ox:.2} {oy:.2})\">\n{}</g>\n",
        frame_group(fill)
    );
    svg_document(w, h, &body)
}

/// The blank sheet as a one-page PDF.
pub fn sheet_pdf(paper: Paper) -> Result<Vec<u8>, RenderError> {
    crate::pdf::document(&[&sheet_svg(paper, None)], paper.size())
}

/// The sheet rendered at `dpi`, blank or filled.
pub fn sheet_png(
    paper: Paper,
    fill: Option<&WordFill>,
    dpi: f64,
) -> Result<image::RgbImage, RenderError> {
    rasterize(&sheet_svg(paper, fill), dpi)
}

/// Ink in a field's interior is this much darker than the paper.
const INK_SHARE_OF_WHITE: f64 = 0.6;
/// Fewer ink pixels than this at [`RECTIFIED_PX_PER_MM`] is a blank
/// field: under a square millimetre, a speck or a fibre.
const MIN_INK_PIXELS: usize = 50;
/// The tight box round the ink grows by this much on every side.
const INK_PAD_MM: f64 = 1.0;

/// The tight box round the handwriting in `interior`, in frame
/// millimetres, or `None` when the interior is blank.
fn ink_bounds(rectified: &GrayImage, interior: &Rect, px_per_mm: f64) -> Option<Rect> {
    let dark = (paper_white(rectified) * INK_SHARE_OF_WHITE) as u8;
    let x0 = (interior.x * px_per_mm).round().max(0.0) as u32;
    let y0 = (interior.y * px_per_mm).round().max(0.0) as u32;
    let x1 = (((interior.x + interior.w) * px_per_mm).round() as u32).min(rectified.width());
    let y1 = (((interior.y + interior.h) * px_per_mm).round() as u32).min(rectified.height());
    let (mut count, mut lo_x, mut lo_y, mut hi_x, mut hi_y) =
        (0usize, u32::MAX, u32::MAX, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            if rectified.get_pixel(x, y)[0] < dark {
                count += 1;
                lo_x = lo_x.min(x);
                lo_y = lo_y.min(y);
                hi_x = hi_x.max(x);
                hi_y = hi_y.max(y);
            }
        }
    }
    if count < MIN_INK_PIXELS {
        return None;
    }
    let tight = Rect {
        x: f64::from(lo_x) / px_per_mm - INK_PAD_MM,
        y: f64::from(lo_y) / px_per_mm - INK_PAD_MM,
        w: f64::from(hi_x - lo_x + 1) / px_per_mm + 2.0 * INK_PAD_MM,
        h: f64::from(hi_y - lo_y + 1) / px_per_mm + 2.0 * INK_PAD_MM,
    };
    // never past the interior: the frame's ink is outside it
    let x = tight.x.max(interior.x);
    let y = tight.y.max(interior.y);
    Some(Rect {
        x,
        y,
        w: (tight.x + tight.w).min(interior.x + interior.w) - x,
        h: (tight.y + tight.h).min(interior.y + interior.h) - y,
    })
}

/// Whether `gray` shows one recovery phrase sheet of this version: its
/// fiducials and orientation mark found as one frame, and its mark
/// field decoding to the sheet's id and version. A strip, an ordinary
/// page or any other marks give `None`. The sheet comes back with its
/// face-on view for the fields to be cut from.
pub fn identify(gray: &GrayImage) -> Option<(Located, GrayImage)> {
    let geometry = Geometry::word_sheet();
    let located = match locate_all_with(gray, geometry) {
        Ok(mut all) if all.len() == 1 => all.remove(0),
        _ => return None,
    };
    let rectified = located.rectify(gray, RECTIFIED_PX_PER_MM);
    let field = read_field(&rectified, &geometry);
    if field.count_ones() <= 1 {
        return None;
    }
    match decode_with(WORD_SHEET_ID, field) {
        Field::Penlock { number } if number == VERSION => Some((located, rectified)),
        _ => None,
    }
}

/// Reads a photographed word sheet field by field, the crops read by
/// `crop_reader`: `Ok(None)` when the photo's marks do not establish a
/// word sheet of this version, so the detector path is the one to
/// take; `Ok(Some)` with exactly twelve records in field order once
/// every field is read, a blank one at its place; `Err` when an
/// identified sheet cannot be read, which is a failure to report, never
/// a page to hand to the detector.
pub fn read_with(
    photo: &RgbImage,
    crop_reader: &dyn Fn(&RgbImage, &Quad) -> Result<WordBox, String>,
) -> Result<Option<PageScan>, String> {
    read_with_observed(photo, crop_reader, None).map(|value| value.map(|(scan, _)| scan))
}

/// [`read_with`] with field progress in canonical photo coordinates. The caller
/// owns terminal delivery, including errors here and subsequent result packing.
pub fn read_with_observed(
    photo: &RgbImage,
    crop_reader: &dyn Fn(&RgbImage, &Quad) -> Result<WordBox, String>,
    observer: Option<&dyn crate::progress::Observer>,
) -> Result<Option<(PageScan, Vec<crate::progress::FinalRegion>)>, String> {
    use crate::progress::{RegionState, Stage};
    let gray = image::DynamicImage::ImageRgb8(photo.clone()).into_luma8();
    let Some((located, rectified)) = identify(&gray) else {
        return Ok(None);
    };
    let mut progress = Stage::geometry(observer, WordSheet::FIELDS);
    let quads: Vec<_> = WordSheet
        .fields()
        .iter()
        .enumerate()
        .map(|(i, interior)| {
            let tight = ink_bounds(&rectified, interior, RECTIFIED_PX_PER_MM).unwrap_or(*interior);
            let corners = [
                (tight.x, tight.y),
                (tight.x + tight.w, tight.y),
                (tight.x + tight.w, tight.y + tight.h),
                (tight.x, tight.y + tight.h),
            ]
            .map(|(x, y)| {
                let (px, py) = located.to_photo(x, y);
                (px as f32, py as f32)
            });
            let quad = Quad(clockwise(corners));
            progress.region(i, &quad, RegionState::Found);
            quad
        })
        .collect();
    progress.reading();
    let mut words = Vec::with_capacity(WordSheet::FIELDS);
    for (i, quad) in quads.into_iter().enumerate() {
        progress.region(i, &quad, RegionState::Reading);
        let mut word = crop_reader(photo, &quad).map_err(|e| format!("field {}: {e}", i + 1))?;
        word.column_rank = i;
        word.row_rank = i;
        crate::phrase::DecisionTrace::push(&mut word.evidence.decisions, || {
            serde_json::json!({"rule":"worksheet_layout","status":"evaluated",
                "field":i,"reason":"registered field; no freeform label, region or replacement passes"})
        });
        progress.region(
            i,
            &word.quad,
            if word.stray {
                RegionState::Excluded
            } else {
                RegionState::Read
            },
        );
        progress.read_done(i + 1);
        words.push(word);
    }
    progress.checking();
    let mut scan = PageScan {
        page_direction: None,
        width: photo.width(),
        height: photo.height(),
        words,
        numbering: Numbering::None,
        join_trials: Vec::new(),
        sources: Vec::new(), // Registered fields, not raw detector observations.
    };
    let regions = progress.into_regions();
    crate::phrase::history::terminal(&mut scan, &regions, "scan_return");
    Ok(Some((scan, regions)))
}

/// [`read_with`] through the stage's own crop reading: the word model,
/// the recogniser, the keep rule and the pick, and no number looked for,
/// since a field holds nothing but its word.
pub fn read(
    photo: &RgbImage,
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
) -> Result<Option<PageScan>, String> {
    read_with(photo, &|photo, quad| {
        read_crop(photo, quad, reader, recogniser, margin)
    })
}

/// [`read`] with optional native field observations and a final identity map.
pub fn read_observed(
    photo: &RgbImage,
    reader: &Reader,
    recogniser: Option<&Recogniser>,
    margin: f32,
    observer: Option<&dyn crate::progress::Observer>,
) -> Result<Option<(PageScan, Vec<crate::progress::FinalRegion>)>, String> {
    read_with_observed(
        photo,
        &|photo, quad| read_crop(photo, quad, reader, recogniser, margin),
        observer,
    )
}
