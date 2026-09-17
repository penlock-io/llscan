//! Finding strips in a photo and reading which strip each one is.
//!
//! A strip's four corner fiducials are solid dark squares; the small solid
//! square printed beside the top-left one says which corner that is, and
//! only a quadruple whose homography puts that mark where the strip says
//! it is — and gives each square its expected size — is accepted, so
//! clutter elsewhere cannot pass for a corner. Their centroids give the
//! homography. Which strip it is comes from the printed identity code,
//! read by blob presence at its six positions and decoded through the
//! distance-4 codebook: one wrong mark is corrected, two make it
//! [`Identity::Unknown`], never another strip.

use std::fmt;

use image::{GrayImage, Luma};
use imageproc::geometric_transformations::{Border, Interpolation, Projection, warp_into};
use imageproc::region_labelling::{Connectivity, connected_components};

use penlock::ShareIndex;

use crate::cells::{CellImage, crop_cells};
use crate::homography::Homography;
use crate::sheet::{Layout, Strip, StripKind};
use crate::template::Rect;

/// What the aided locator needs to know about a printed thing: where
/// its fiducials, orientation mark and mark-field slots are within its
/// frame. The strip and the word sheet each provide theirs; the search
/// and the field reading are the same code over either.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Geometry {
    /// The frame, origin at its top-left corner, millimetres.
    pub bounds: Rect,
    /// The four fiducials: top-left, top-right, bottom-left, bottom-right.
    pub fiducials: [Rect; 4],
    /// The orientation mark beside the top-left fiducial.
    pub mark: Rect,
    /// The sixteen mark-field slots, slot `i` carrying bit `i`.
    pub mark_slots: [Rect; 16],
}

impl Geometry {
    /// The worksheet strip's.
    pub fn strip() -> Geometry {
        Geometry {
            bounds: Strip.bounds(),
            fiducials: Strip.fiducials(),
            mark: Strip.mark(),
            mark_slots: Strip.mark_slots(),
        }
    }

    /// The word sheet's.
    pub fn word_sheet() -> Geometry {
        use crate::wordsheet::WordSheet;
        Geometry {
            bounds: WordSheet.bounds(),
            fiducials: WordSheet.fiducials(),
            mark: WordSheet.mark(),
            mark_slots: WordSheet.mark_slots(),
        }
    }
}

/// Resolution of rectified card images.
pub const RECTIFIED_PX_PER_MM: f64 = 8.0;

/// Prints a line of the locator's reasoning to stderr when
/// `PENLOCK_TRACE` is set, for finding out why a photo did not read.
pub(crate) fn trace(args: fmt::Arguments<'_>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PENLOCK_TRACE").is_some()) {
        eprintln!("{args}");
    }
}

/// What a strip was found by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchors {
    /// The four corner squares and the orientation mark.
    Fiducials,
    /// The shaded checksum boxes of a strip without scan aids, oriented
    /// by the letter boxes' borders.
    ChecksumBoxes,
}

impl fmt::Display for Anchors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Anchors::Fiducials => f.write_str("corner squares"),
            Anchors::ChecksumBoxes => f.write_str("checksum boxes (no corner squares)"),
        }
    }
}

/// Where a strip, or a word sheet, is in a photo.
#[derive(Clone, Debug)]
pub struct Located {
    /// Fiducial centres in photo pixels: top-left, top-right, bottom-left,
    /// bottom-right — where the frame puts them, printed or not.
    pub fiducials: [(f64, f64); 4],
    /// What it was found by.
    pub anchors: Anchors,
    /// Which boxes a strip prints; a word sheet has none.
    pub layout: Layout,
    /// The frame it was found as.
    pub geometry: Geometry,
    mm_to_photo: Homography,
    photo_to_mm: Homography,
}

impl Located {
    pub(crate) fn new(
        mm_to_photo: Homography,
        anchors: Anchors,
        layout: Layout,
        geometry: Geometry,
    ) -> Option<Located> {
        let photo_to_mm = mm_to_photo.inverse()?;
        let fiducials = geometry.fiducials.map(|r| {
            let (x, y) = r.center();
            mm_to_photo.map(x, y)
        });
        Some(Located {
            fiducials,
            anchors,
            layout,
            geometry,
            mm_to_photo,
            photo_to_mm,
        })
    }

    /// Where a point on the card, in millimetres, is in the photo.
    pub fn to_photo(&self, x_mm: f64, y_mm: f64) -> (f64, f64) {
        self.mm_to_photo.map(x_mm, y_mm)
    }

    /// Where a photo pixel is on the card, in millimetres.
    pub fn to_mm(&self, x_px: f64, y_px: f64) -> (f64, f64) {
        self.photo_to_mm.map(x_px, y_px)
    }

    /// The frame as seen face-on at `px_per_mm`, background white.
    pub fn rectify(&self, photo: &GrayImage, px_per_mm: f64) -> GrayImage {
        let mut out = GrayImage::new(
            (self.geometry.bounds.w * px_per_mm).round() as u32,
            (self.geometry.bounds.h * px_per_mm).round() as u32,
        );
        warp_into(
            photo,
            Projection::scale(px_per_mm as f32, px_per_mm as f32)
                * self.photo_to_mm.to_projection(),
            Interpolation::Bilinear,
            Border::Constant(Luma([255])),
            &mut out,
        );
        out
    }
}

/// Why no strip could be found.
#[derive(Clone, PartialEq, Debug)]
pub enum LocateError {
    /// Fewer than four solid square blobs.
    TooFewFiducials {
        /// Candidates found.
        found: usize,
    },
    /// No quadruple of solid squares has the sizes its homography implies
    /// and reprojects the orientation mark onto a mark-sized blob.
    FiducialsMismatched,
    /// Four squares of consistent size, but no mark-sized blob where the
    /// orientation mark should be in any orientation.
    NoOrientationMark,
    /// More than one strip where exactly one was expected.
    Several(usize),
    /// A mark field whose id byte is not Penlock's: another project's
    /// strip, its raw bytes preserved.
    UnsupportedProject {
        /// The top row's byte.
        id: u8,
        /// The bottom row's byte.
        bits: u8,
    },
    /// A mark field present but invalid beyond one mark: a damaged
    /// versioned strip, refused rather than read as anything older.
    MarkFieldDamaged,
    /// No corner squares, and too few rows of checksum boxes to stand in
    /// for them.
    NoChecksumBoxes {
        /// Rows of boxes found.
        rows: usize,
    },
    /// Checksum boxes found, but which way up the strip is could not be
    /// told from the letter boxes' borders.
    NoOrientation,
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewFiducials { found } => {
                write!(f, "found {found} of the 4 corner squares")
            }
            Self::FiducialsMismatched => {
                write!(f, "no four squares fit together as one card's corners")
            }
            Self::NoOrientationMark => {
                write!(
                    f,
                    "corners found but not the orientation mark beside the top-left one"
                )
            }
            Self::Several(n) => write!(
                f,
                "{n} strips in one photo; photograph one cut strip at a time, or pass the sheet with --sheet"
            ),
            Self::UnsupportedProject { id, bits } => write!(
                f,
                "the mark field carries project id {id:#04x} (bits {bits:#04x}): not a Penlock strip"
            ),
            Self::MarkFieldDamaged => write!(
                f,
                "the mark field is damaged beyond one mark; refusing to guess what the strip is"
            ),
            Self::NoChecksumBoxes { rows } => write!(
                f,
                "no corner squares, and only {rows} of the 12 rows of shaded boxes to go by"
            ),
            Self::NoOrientation => write!(
                f,
                "shaded boxes found but not which way up the strip is; photograph it flat with the whole strip in frame"
            ),
        }
    }
}

impl std::error::Error for LocateError {}

/// Adaptive-threshold window as a fraction of the photo's width; wider
/// than any fiducial so a square's interior stays darker than its
/// surroundings.
const WINDOW_FRACTION: f64 = 0.125;
/// How much darker than its surroundings a pixel must be to count as ink.
const DARK_DELTA: i32 = 20;
/// Minimum blob area, in pixels, to be considered at all.
const MIN_BLOB_AREA: u32 = 40;
/// Minimum area / bounding-box area for a blob to be a solid square. A
/// square rolled 15° fills 0.67 of its box.
const MIN_FILL: f64 = 0.55;
/// How many of the largest solid blobs are tried as fiducials; a whole
/// uncut sheet has sixteen, so sixteen pieces of clutter larger than a
/// fiducial can precede them and every strip is still found.
const FIDUCIAL_CANDIDATES: usize = 32;
/// A blob's measured area over the area its projected fiducial square
/// would have, for the blob to pass as that fiducial.
const AREA_MATCH: (f64, f64) = (0.6, 1.6);
/// How far the reprojected mark centre may sit from the mark blob's
/// centroid, as a fraction of the fiducial spacing.
const MARK_TOLERANCE: f64 = 0.03;
/// A quadruple's misfit: the mark's distance plus the fiducials' size
/// misfit. Three true corners and a neighbouring strip's fiducial project
/// the mark as well as the true quadruple does, but that fourth blob is
/// 0.6 of the size its homography implies; the size term (about 0.5
/// against under 0.1 for a true quadruple) is what ranks them.
fn misfit(mark_fit: f64, size_ratios: &[f64; 4]) -> f64 {
    mark_fit + size_ratios.iter().map(|r| r.ln().abs()).sum::<f64>()
}
/// The orientation mark's area relative to its projected size.
const MARK_AREA_MATCH: (f64, f64) = (0.5, 2.0);
/// How far the strip's two axes may depart from perpendicular at its
/// centre, in degrees. A camera pitched and yawed 20° leaves them within
/// 8°; two corners of one strip with two of a neighbour on the sheet make
/// a parallelogram sheared 20° or more — an affine shape the size and
/// mark checks cannot tell from a strip, since the mark lies on the line
/// through the top fiducials.
const MAX_AXIS_SKEW_DEGREES: f64 = 15.0;

/// Whether `mm_to_photo` keeps the frame's axes near perpendicular at
/// its centre.
fn axes_near_perpendicular(mm_to_photo: &Homography, bounds: Rect) -> bool {
    let (cx, cy) = bounds.center();
    let o = mm_to_photo.map(cx, cy);
    let ax = mm_to_photo.map(cx + 1.0, cy);
    let ay = mm_to_photo.map(cx, cy + 1.0);
    let ux = (ax.0 - o.0, ax.1 - o.1);
    let uy = (ay.0 - o.0, ay.1 - o.1);
    let cos = (ux.0 * uy.0 + ux.1 * uy.1) / (ux.0.hypot(ux.1) * uy.0.hypot(uy.1));
    cos.abs() <= MAX_AXIS_SKEW_DEGREES.to_radians().sin()
}

/// Finds every strip in `photo`: fiducials, orientation, homography.
///
/// Every quadruple among the largest solid blobs is tried in every
/// orientation; a quadruple is accepted only if the homography it implies
/// keeps the strip's axes near perpendicular, reprojects the strip's
/// orientation mark onto a solid blob of the right size and each fiducial
/// blob has the area its projected square would have. Accepted quadruples that share a blob with a better-fitting one
/// (by mark distance and size misfit together) are suppressed, so clutter
/// squares and a neighbouring strip's corners cannot displace a true
/// corner and one strip is never reported twice. Strips come back in reading order:
/// by the row of their top-left fiducial, then left to right.
pub fn locate_all(photo: &GrayImage) -> Result<Vec<Located>, LocateError> {
    match locate_aided(photo, Geometry::strip()) {
        Ok(found) => Ok(found),
        // Nothing with corner squares was accepted, whatever the miss:
        // the strip may be upstream's, which has none. If its boxes were
        // found, the bare path's miss is the one worth reporting.
        Err(aided) => match crate::bare::locate_bare(photo) {
            Ok(bare) => Ok(vec![bare]),
            Err(bare @ LocateError::NoOrientation) => Err(bare),
            Err(_) => Err(aided),
        },
    }
}

/// Finds every frame of `geometry` in `photo` by its fiducials and
/// orientation mark: [`locate_all`] without the bare fallback, for
/// the word sheet as much as for strips.
pub fn locate_all_with(photo: &GrayImage, geometry: Geometry) -> Result<Vec<Located>, LocateError> {
    locate_aided(photo, geometry)
}

fn locate_aided(photo: &GrayImage, geometry: Geometry) -> Result<Vec<Located>, LocateError> {
    let dark = dark_mask(photo);
    let mut blobs = solid_blobs(&dark);
    blobs.sort_by_key(|b| std::cmp::Reverse(b.area));
    if blobs.len() < 4 {
        return Err(LocateError::TooFewFiducials { found: blobs.len() });
    }
    let candidates = &blobs[..blobs.len().min(FIDUCIAL_CANDIDATES)];
    let template_centres = geometry.fiducials.map(|r| r.center());
    let (mark_x, mark_y) = geometry.mark.center();

    let mut fits: Vec<(f64, [usize; 4], Located)> = Vec::new();
    let mut sized_quadruple = false;
    for quad in quadruples(candidates.len()) {
        // Four corners of one strip are the same printed square; a mark or
        // code square cannot stand in for one.
        let areas = quad.map(|i| f64::from(candidates[i].area));
        let (smallest, largest) = areas.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), &a| {
            (lo.min(a), hi.max(a))
        });
        if largest > 2.5 * smallest {
            continue;
        }
        let centres = quad.map(|i| candidates[i].centroid());
        let cx = centres.iter().map(|c| c.0).sum::<f64>() / 4.0;
        let cy = centres.iter().map(|c| c.1).sum::<f64>() / 4.0;
        let angle = |p: (f64, f64)| (p.1 - cy).atan2(p.0 - cx);
        let mut clockwise = [0, 1, 2, 3];
        clockwise.sort_by(|&a, &b| angle(centres[a]).total_cmp(&angle(centres[b])));
        let spacing = (0..4)
            .flat_map(|i| (i + 1..4).map(move |j| (i, j)))
            .map(|(i, j)| distance(centres[i], centres[j]))
            .fold(f64::INFINITY, f64::min);

        for rotation in 0..4 {
            let at = |k: usize| clockwise[(rotation + k) % 4];
            let order = [at(0), at(1), at(3), at(2)];
            let photo_centres = order.map(|i| centres[i]);
            let Some(mm_to_photo) = Homography::from_points(template_centres, photo_centres) else {
                continue;
            };
            let Some(photo_to_mm) = mm_to_photo.inverse() else {
                continue;
            };
            if !axes_near_perpendicular(&mm_to_photo, geometry.bounds) {
                continue;
            }
            let project = |x: f64, y: f64| mm_to_photo.map(x, y);

            let fiducial_rects = geometry.fiducials;
            let mut size_ratios = [0.0; 4];
            for (k, (&i, r)) in order.iter().zip(fiducial_rects).enumerate() {
                size_ratios[k] = f64::from(candidates[quad[i]].area) / projected_area(r, &project);
            }
            if size_ratios
                .iter()
                .any(|&ratio| ratio < AREA_MATCH.0 || ratio > AREA_MATCH.1)
            {
                continue;
            }
            sized_quadruple = true;

            let mark_expected = project(mark_x, mark_y);
            let mark_area = projected_area(geometry.mark, &project);
            let mark_fit = blobs
                .iter()
                .filter(|b| {
                    let ratio = f64::from(b.area) / mark_area;
                    ratio >= MARK_AREA_MATCH.0 && ratio <= MARK_AREA_MATCH.1
                })
                .map(|b| distance(b.centroid(), mark_expected) / spacing)
                .fold(f64::INFINITY, f64::min);
            if mark_fit > MARK_TOLERANCE {
                continue;
            }
            fits.push((
                misfit(mark_fit, &size_ratios),
                quad,
                Located {
                    fiducials: order.map(|i| centres[i]),
                    anchors: Anchors::Fiducials,
                    layout: Layout::Aided,
                    geometry,
                    mm_to_photo,
                    photo_to_mm,
                },
            ));
        }
    }
    fits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut taken: Vec<usize> = Vec::new();
    let mut found: Vec<Located> = Vec::new();
    for (_, quad, located) in fits {
        if quad.iter().any(|i| taken.contains(i)) {
            continue;
        }
        // The mark and code squares of a strip already found can form a
        // small quadruple of their own; anything whose corners lie on an
        // accepted strip is part of it, not another strip.
        let inside_found = located.fiducials.iter().any(|&(x, y)| {
            found.iter().any(|f| {
                let (sx, sy) = f.to_mm(x, y);
                (-2.0..=geometry.bounds.w + 2.0).contains(&sx)
                    && (-2.0..=geometry.bounds.h + 2.0).contains(&sy)
            })
        });
        if inside_found {
            continue;
        }
        taken.extend(quad);
        found.push(located);
    }
    if found.is_empty() {
        return Err(if sized_quadruple {
            LocateError::NoOrientationMark
        } else {
            LocateError::FiducialsMismatched
        });
    }
    // Reading order by strip centres: rows are separated by more than
    // half a strip's smaller extent (strips lie sideways on the sheet).
    let centre = |l: &Located| {
        let (sx, sy) = l
            .fiducials
            .iter()
            .fold((0.0, 0.0), |(x, y), c| (x + c.0, y + c.1));
        (sx / 4.0, sy / 4.0)
    };
    let extent = |l: &Located| {
        distance(l.fiducials[0], l.fiducials[1]).min(distance(l.fiducials[0], l.fiducials[2]))
    };
    found.sort_by(|a, b| {
        let band = (extent(a) + extent(b)) / 4.0;
        let (ca, cb) = (centre(a), centre(b));
        if (ca.1 - cb.1).abs() > band {
            ca.1.total_cmp(&cb.1)
        } else {
            ca.0.total_cmp(&cb.0)
        }
    });
    Ok(found)
}

/// Finds the one strip in `photo`; several is [`LocateError::Several`].
pub fn locate(photo: &GrayImage) -> Result<Located, LocateError> {
    let mut all = locate_all(photo)?;
    match all.len() {
        1 => Ok(all.remove(0)),
        n => Err(LocateError::Several(n)),
    }
}

/// Every 4-subset of `0..n`.
fn quadruples(n: usize) -> impl Iterator<Item = [usize; 4]> {
    (0..n).flat_map(move |a| {
        (a + 1..n)
            .flat_map(move |b| (b + 1..n).flat_map(move |c| (c + 1..n).map(move |d| [a, b, c, d])))
    })
}

/// Area in photo pixels of a template rectangle under `project`.
fn projected_area(r: crate::template::Rect, project: &impl Fn(f64, f64) -> (f64, f64)) -> f64 {
    let corners = [
        project(r.x, r.y),
        project(r.x + r.w, r.y),
        project(r.x + r.w, r.y + r.h),
        project(r.x, r.y + r.h),
    ];
    let mut twice = 0.0;
    for i in 0..4 {
        let (x0, y0) = corners[i];
        let (x1, y1) = corners[(i + 1) % 4];
        twice += x0 * y1 - x1 * y0;
    }
    (twice / 2.0).abs().max(1.0)
}

/// Which strip a located strip is, from its printed identity code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Identity {
    /// The seed-phrase strip.
    Seed,
    /// A share strip.
    Share(ShareIndex),
    /// The code could not be read: two or more marks wrong, or a
    /// defaced strip.
    Unknown,
}

impl Identity {
    /// The share index, if this is a share strip.
    pub fn share(self) -> Option<ShareIndex> {
        match self {
            Identity::Share(i) => Some(i),
            _ => None,
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Identity::Seed => f.write_str("seed phrase"),
            Identity::Share(i) => write!(f, "share {i}"),
            Identity::Unknown => f.write_str("unknown strip"),
        }
    }
}

impl From<StripKind> for Identity {
    fn from(kind: StripKind) -> Identity {
        match kind {
            StripKind::Seed => Identity::Seed,
            StripKind::Share(i) => Identity::Share(i),
        }
    }
}

/// A located strip cut into the cells a model reads.
#[derive(Clone, Debug)]
pub struct Found {
    /// Where the strip is.
    pub located: Located,
    /// The strip face-on at [`RECTIFIED_PX_PER_MM`].
    pub rectified: GrayImage,
    /// Which strip it is, from the printed code.
    pub identity: Identity,
    /// What the mark field says the strip's format is: always legacy
    /// or a recognised version. Any other field never becomes a
    /// `Found` — it is a [`LocateError`] instead.
    pub format: crate::marks::Format,
    /// Every row's cells, in word order.
    pub cells: Vec<[CellImage; 6]>,
}

/// A code position counts as marked above this ink fraction.
const CODE_MARK_INK: f64 = 0.4;

/// Reads the sixteen-slot mark field and dispatches the format: a
/// Penlock field carries the sheet number, a confidently absent field
/// falls back to the legacy six-mark code, and anything else is
/// refused outright — a foreign project's field (raw bytes preserved)
/// or a damaged one must fail loudly, never misread as legacy.
pub fn read_format(rectified: &GrayImage) -> Result<(crate::marks::Format, Identity), LocateError> {
    use crate::marks::{Field, Format};
    let field = read_field(rectified, &Geometry::strip());
    // One dark slot is not a field: a legacy strip must survive a
    // single spurious mark, and every real field prints at least four
    // — the id's two and the number row's masked minimum.
    if field.count_ones() <= 1 {
        return Ok((Format::Legacy, read_identity(rectified)));
    }
    match crate::marks::decode(field) {
        Field::Penlock { number } => {
            let identity = if number == 0 {
                Identity::Seed
            } else {
                ShareIndex::new(number).map_or(Identity::Unknown, Identity::Share)
            };
            Ok((Format::Penlock { number }, identity))
        }
        Field::Foreign { id, bits } => Err(LocateError::UnsupportedProject { id, bits }),
        Field::Damaged => Err(LocateError::MarkFieldDamaged),
    }
}

/// The sixteen-slot mark field as printed on a rectified frame: bit
/// `i` set when slot `i` holds ink against the frame's own paper white.
pub fn read_field(rectified: &GrayImage, geometry: &Geometry) -> u16 {
    let white = paper_white(rectified);
    let mut field = 0u16;
    for (i, slot) in geometry.mark_slots.iter().enumerate() {
        if ink_fraction(rectified, slot.grow(-0.2), RECTIFIED_PX_PER_MM, white) > CODE_MARK_INK {
            field |= 1 << i;
        }
    }
    field
}

/// Reads the identity code off a rectified strip. Ink is judged against
/// the strip's own paper white, since a solid square has no white of its
/// own to compare with.
pub fn read_identity(rectified: &GrayImage) -> Identity {
    let white = paper_white(rectified);
    let read: [bool; 6] = Strip
        .code_positions()
        .map(|r| ink_fraction(rectified, r.grow(-0.4), RECTIFIED_PX_PER_MM, white) > CODE_MARK_INK);
    StripKind::decode(read).map_or(Identity::Unknown, Identity::from)
}

/// The 95th percentile of a rectified strip: its paper.
pub(crate) fn paper_white(rectified: &GrayImage) -> f64 {
    let mut histogram = [0u32; 256];
    for p in rectified.pixels() {
        histogram[usize::from(p.0[0])] += 1;
    }
    let target = rectified.pixels().len() as u32 * 95 / 100;
    let mut seen = 0;
    for (value, count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= target {
            return f64::from(value as u8).max(60.0);
        }
    }
    255.0
}

/// Cuts a located strip: rectifies it, reads its identity, crops its cells.
pub fn cut(photo: &GrayImage, located: Located) -> Result<Found, LocateError> {
    let rectified = located.rectify(photo, RECTIFIED_PX_PER_MM);
    let (format, identity) = match located.anchors {
        Anchors::Fiducials => read_format(&rectified)?,
        Anchors::ChecksumBoxes => (
            crate::marks::Format::Legacy,
            crate::title::read_title(&rectified, RECTIFIED_PX_PER_MM),
        ),
    };
    let cells = crop_cells(&rectified, RECTIFIED_PX_PER_MM, located.layout);
    Ok(Found {
        located,
        rectified,
        identity,
        format,
        cells,
    })
}

/// Finds the one strip in `photo` and cuts it.
pub fn find_strip(photo: &GrayImage) -> Result<Found, LocateError> {
    cut(photo, locate(photo)?)
}

/// Finds every strip in `photo` and cuts each, in reading order.
pub fn find_strips(photo: &GrayImage) -> Result<Vec<Found>, LocateError> {
    locate_all(photo)?
        .into_iter()
        .map(|l| cut(photo, l))
        .collect()
}

/// Share of `area`'s pixels darker than 0.6 of `white`.
fn ink_fraction(
    rectified: &GrayImage,
    area: crate::template::Rect,
    px_per_mm: f64,
    white: f64,
) -> f64 {
    let x0 = (area.x * px_per_mm).round().max(0.0) as u32;
    let y0 = (area.y * px_per_mm).round().max(0.0) as u32;
    let x1 = (((area.x + area.w) * px_per_mm).round() as u32).min(rectified.width());
    let y1 = (((area.y + area.h) * px_per_mm).round() as u32).min(rectified.height());
    let mut values: Vec<u8> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            values.push(rectified.get_pixel(x, y).0[0]);
        }
    }
    if values.is_empty() {
        return 0.0;
    }
    let threshold = white * 0.6;
    values.iter().filter(|&&v| f64::from(v) < threshold).count() as f64 / values.len() as f64
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Pixels darker than their surroundings, 255 where dark.
/// Summed-area table of a photo, for local means.
pub(crate) struct Integral {
    w: usize,
    h: usize,
    sums: Vec<u64>,
}

impl Integral {
    pub(crate) fn new(photo: &GrayImage) -> Integral {
        let (w, h) = (photo.width() as usize, photo.height() as usize);
        let mut sums = vec![0u64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0u64;
            for x in 0..w {
                row += u64::from(photo.get_pixel(x as u32, y as u32).0[0]);
                sums[(y + 1) * (w + 1) + x + 1] = sums[y * (w + 1) + x + 1] + row;
            }
        }
        Integral { w, h, sums }
    }

    /// Mean over the window of `radius` around `(x, y)`, clipped to the
    /// photo.
    pub(crate) fn local_mean(&self, x: usize, y: usize, radius: usize) -> f64 {
        let (w, h) = (self.w, self.h);
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius + 1).min(h);
        let x0 = x.saturating_sub(radius);
        let x1 = (x + radius + 1).min(w);
        let sum = self.sums[y1 * (w + 1) + x1] + self.sums[y0 * (w + 1) + x0]
            - self.sums[y0 * (w + 1) + x1]
            - self.sums[y1 * (w + 1) + x0];
        sum as f64 / ((y1 - y0) * (x1 - x0)) as f64
    }
}

fn dark_mask(photo: &GrayImage) -> GrayImage {
    let integral = Integral::new(photo);
    let radius = ((photo.width() as f64 * WINDOW_FRACTION) as usize).max(1);
    let mut out = GrayImage::new(photo.width(), photo.height());
    for (x, y, p) in photo.enumerate_pixels() {
        let mean = integral.local_mean(x as usize, y as usize, radius);
        if i32::from(p.0[0]) < mean as i32 - DARK_DELTA {
            out.put_pixel(x, y, Luma([255]));
        }
    }
    out
}

pub(crate) struct Blob {
    pub(crate) area: u32,
    sum_x: u64,
    sum_y: u64,
    sum_xx: u64,
    sum_yy: u64,
    sum_xy: u64,
    pub(crate) min_x: u32,
    pub(crate) max_x: u32,
    pub(crate) min_y: u32,
    pub(crate) max_y: u32,
}

impl Blob {
    /// Direction of the blob's long axis (radians, sign-free) and its
    /// extents along and across it, for a solid shape.
    pub(crate) fn principal_axis(&self) -> (f64, f64, f64) {
        let n = f64::from(self.area);
        let (mx, my) = self.centroid();
        let cxx = self.sum_xx as f64 / n - mx * mx;
        let cyy = self.sum_yy as f64 / n - my * my;
        let cxy = self.sum_xy as f64 / n - mx * my;
        let angle = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
        let mean = (cxx + cyy) / 2.0;
        let diff = ((cxx - cyy) / 2.0).hypot(cxy);
        // A uniform rectangle of side L has variance L² / 12 along it.
        let long = (12.0 * (mean + diff).max(0.0)).sqrt();
        let short = (12.0 * (mean - diff).max(0.0)).sqrt();
        (angle, long, short)
    }

    pub(crate) fn centroid(&self) -> (f64, f64) {
        (
            self.sum_x as f64 / f64::from(self.area),
            self.sum_y as f64 / f64::from(self.area),
        )
    }

    fn is_solid_square(&self) -> bool {
        let w = f64::from(self.max_x - self.min_x + 1);
        let h = f64::from(self.max_y - self.min_y + 1);
        let fill = f64::from(self.area) / (w * h);
        self.area >= MIN_BLOB_AREA && fill >= MIN_FILL && w / h > 0.5 && w / h < 2.0
    }
}

fn solid_blobs(dark: &GrayImage) -> Vec<Blob> {
    label_blobs(dark)
        .into_iter()
        .filter(Blob::is_solid_square)
        .collect()
}

/// Every connected component of a mask.
pub(crate) fn label_blobs(mask: &GrayImage) -> Vec<Blob> {
    let labels = connected_components(mask, Connectivity::Eight, Luma([0]));
    let mut blobs: Vec<Option<Blob>> = Vec::new();
    for (x, y, label) in labels.enumerate_pixels() {
        let label = label.0[0] as usize;
        if label == 0 {
            continue;
        }
        if blobs.len() <= label {
            blobs.resize_with(label + 1, || None);
        }
        let blob = blobs[label].get_or_insert(Blob {
            area: 0,
            sum_x: 0,
            sum_y: 0,
            sum_xx: 0,
            sum_yy: 0,
            sum_xy: 0,
            min_x: x,
            max_x: x,
            min_y: y,
            max_y: y,
        });
        blob.area += 1;
        blob.sum_x += u64::from(x);
        blob.sum_y += u64::from(y);
        blob.sum_xx += u64::from(x) * u64::from(x);
        blob.sum_yy += u64::from(y) * u64::from(y);
        blob.sum_xy += u64::from(x) * u64::from(y);
        blob.min_x = blob.min_x.min(x);
        blob.max_x = blob.max_x.max(x);
        blob.min_y = blob.min_y.min(y);
        blob.max_y = blob.max_y.max(y);
    }
    blobs.into_iter().flatten().collect()
}
