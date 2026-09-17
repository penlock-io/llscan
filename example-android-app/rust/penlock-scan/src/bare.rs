//! Locating a strip that has no scan aids by its shaded checksum boxes.
//!
//! Upstream's worksheet prints two shaded boxes per row: twenty-four
//! mid-grey squares in two columns at the row pitch, at known positions.
//! Their centres fit the same homography the fiducials give. The lattice
//! is its own image under a half-turn, so which way up the strip is has
//! to come from something the lattice does not have: upstream draws a
//! dotted line along the bottom of each row's boxes, from the shaded
//! pair to the far edge of the last letter box, and nothing along the
//! top. Which side and edge that line runs along is the orientation;
//! where it ends pins the across axis a strip-width from the pair.

use image::{GrayImage, Luma};

use crate::homography::{Correspondence, Homography};
use crate::locate::{Anchors, Blob, Integral, LocateError, Located, label_blobs, trace};
use crate::sheet::{Layout, Strip};

/// The photo is averaged down by these before boxes are looked for, so a
/// box is one solid blob whatever is written on it; a strip small in the
/// frame needs the finer one.
const DOWNSAMPLES: [u32; 2] = [4, 2];
/// Windows, as fractions of the photo's width, over which "brighter than
/// its surroundings" is judged when looking for the boxes. Before the
/// strip is found no window is right for every photo — a wide one takes
/// in the table around a small strip, a narrow one sits inside a big
/// strip's own box — so each is tried and the lattice with most rows
/// kept.
const WINDOW_FRACTIONS: [f64; 3] = [0.125, 0.0625, 0.03125];
/// A pixel between these fractions of its surroundings' brightness is
/// printed grey: darker than paper, lighter than ink.
const GREY_BAND: (f64, f64) = (0.45, 0.88);
/// Share of a downsampled cell's pixels that must be grey for the cell to
/// be part of a box.
const GREY_FILL: f64 = 0.4;
/// Rows of boxes that must be found; the first and last rows must be
/// among them, so the rows' numbers are known.
const MIN_ROWS: usize = 10;

/// Finds the one strip in `photo` by its checksum boxes.
pub(crate) fn locate_bare(photo: &GrayImage) -> Result<Located, LocateError> {
    let integral = Integral::new(photo);
    let mut best: Result<(Vec<Row>, (f64, f64)), LocateError> =
        Err(LocateError::NoChecksumBoxes { rows: 0 });
    for fraction in WINDOW_FRACTIONS {
        let radius = ((photo.width() as f64 * fraction) as usize).max(1);
        for factor in DOWNSAMPLES {
            let grey = grey_boxes_mask(photo, &integral, radius, factor);
            let pairs: Vec<PairBlob> = label_blobs(&grey)
                .iter()
                .filter_map(|b| PairBlob::of(b, factor))
                .collect();
            let found = lattice(&pairs);
            trace(format_args!(
                "bare: window {fraction} at 1/{factor}: {} pairs -> {}",
                pairs.len(),
                match &found {
                    Ok((rows, _)) => format!("{} rows", rows.len()),
                    Err(e) => e.to_string(),
                }
            ));
            let rows_of = |r: &Result<(Vec<Row>, (f64, f64)), LocateError>| match r {
                Ok((rows, _)) => rows.len(),
                Err(LocateError::NoChecksumBoxes { rows }) => *rows,
                Err(_) => 0,
            };
            if found.is_ok() && (best.is_err() || rows_of(&found) > rows_of(&best))
                || (found.is_err() && best.is_err() && rows_of(&found) > rows_of(&best))
            {
                best = found;
            }
            if matches!(&best, Ok((rows, _)) if rows.len() == Strip::ROWS) {
                break;
            }
        }
    }
    let (mut rows, across) = best?;
    let along = (-across.1, across.0);
    let pitch = {
        let first = midpoint(rows[0].boxes[0], rows[0].boxes[1]);
        let last = midpoint(rows[rows.len() - 1].boxes[0], rows[rows.len() - 1].boxes[1]);
        let span = (rows[rows.len() - 1].index - rows[0].index).max(1) as f64;
        distance(first, last) / span
    };
    let mut extents = Vec::with_capacity(rows.len());
    for row in &mut rows {
        let centre = midpoint(row.boxes[0], row.boxes[1]);
        let coarse = distance(row.boxes[0], row.boxes[1]) * 2.0;
        let height = coarse * Strip::UPSTREAM_BOX_ALONG / (2.0 * Strip::BOX_ACROSS);
        let paper = row_paper(photo, centre, along, height, pitch);
        let (centre, length) =
            run_extent(photo, paper, centre, across, coarse).unwrap_or((centre, coarse));
        let height = length * Strip::UPSTREAM_BOX_ALONG / (2.0 * Strip::BOX_ACROSS);
        let (centre, height) =
            run_extent(photo, paper, centre, along, height).unwrap_or((centre, height));
        row.boxes = box_centres(centre, length, across);
        trace(format_args!(
            "bare: row {} at ({:.1}, {:.1}) pair {length:.1} px, box {height:.1} px, paper {paper:.0}",
            row.index, centre.0, centre.1
        ));
        extents.push((centre, length, height, paper));
    }
    settle_end_row(photo, &mut rows, &extents, across, along, pitch);

    // The dotted line along the bottom of a row's boxes runs from the
    // shaded pair to the far edge of the last letter box, on the letter
    // side only. Which side and which edge it runs along is the
    // orientation; its direction pins the across axis a strip-width away
    // from the pair, which the pair's 13 mm alone cannot.
    let mut ends: [Vec<Tracked>; 4] = Default::default();
    for (row, &(centre, length, height, paper)) in rows.iter().zip(&extents) {
        for (which, (side, edge)) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
            .into_iter()
            .enumerate()
        {
            let start = (
                centre.0 + side * length / 2.0 * across.0 + edge * height / 2.0 * along.0,
                centre.1 + side * length / 2.0 * across.1 + edge * height / 2.0 * along.1,
            );
            let dir = (side * across.0, side * across.1);
            let expected = length * LINE_BEYOND_PAIR / (2.0 * Strip::BOX_ACROSS);
            if let Some(track) = track_line(photo, paper, start, dir, along, expected) {
                trace(format_args!(
                    "bare: row {} line on side {side:+} edge {edge:+}: {:.0} of {expected:.0} px",
                    row.index,
                    distance(start, track.end)
                ));
                ends[which].push(Tracked {
                    index: row.index,
                    track,
                    start,
                    paper,
                    length,
                });
            }
        }
    }
    let counts = ends.each_ref().map(Vec::len);
    let best = (0..4).max_by_key(|&i| counts[i]).expect("four ways");
    let rest = (0..4)
        .filter(|&i| i != best)
        .map(|i| counts[i])
        .max()
        .unwrap_or(0);
    let decided = if rest == 0 {
        counts[best] >= MIN_TRACKED_ROWS
    } else {
        counts[best] > MIN_TRACKED_ROWS && 3 * rest <= counts[best]
    };
    if !decided {
        return Err(LocateError::NoOrientation);
    }
    // Letters on the +across side with the line on the +along edge is
    // the strip as its frame has it; both flipped is the half-turn; one
    // flipped would be a mirror image, which no photo makes.
    let turned = match best {
        0 => false,
        3 => true,
        _ => return Err(LocateError::NoOrientation),
    };
    let mut pairs = correspondences(&rows, turned);
    let (side, edge) = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)][best];
    let dir = (side * across.0, side * across.1);
    let mut ticks = 0;
    for tracked in &ends[best] {
        let (row, _) = template_row(tracked.index, turned);
        let first = Layout::Upstream.cell_rect(row, 2);
        for (border, point) in tracked.ticks(photo, dir, along, edge) {
            let x = first.x + Strip::BOX_ACROSS * border as f64;
            pairs.push(((x, first.y + first.h), point));
            ticks += 1;
        }
    }
    trace(format_args!(
        "bare: {ticks} border ticks on {} lines",
        ends[best].len()
    ));
    let boxes = correspondences(&rows, turned).len();
    let mut fit =
        Homography::fit(&pairs).ok_or(LocateError::NoChecksumBoxes { rows: rows.len() })?;
    let residuals = |fit: &Homography, pairs: &[Correspondence]| -> Vec<f64> {
        pairs
            .iter()
            .map(|&(from, to)| distance(fit.map(from.0, from.1), to))
            .collect()
    };
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let mut residual = mean(&residuals(&fit, &pairs));
    // A line end that ran onto something else pulls the whole fit; drop
    // the line ends that stand out and fit again.
    if residual > OUTLIER_RESIDUAL_PX {
        let r = residuals(&fit, &pairs);
        let kept: Vec<Correspondence> = pairs
            .iter()
            .enumerate()
            .filter(|&(i, _)| i < boxes || r[i] <= OUTLIER_FACTOR * residual)
            .map(|(_, p)| *p)
            .collect();
        if kept.len() < pairs.len()
            && let Some(refit) = Homography::fit(&kept)
        {
            trace(format_args!(
                "bare: dropped {} line ends with residuals over {:.1} px",
                pairs.len() - kept.len(),
                OUTLIER_FACTOR * residual
            ));
            pairs = kept;
            fit = refit;
            residual = mean(&residuals(&fit, &pairs));
        }
    }
    trace(format_args!(
        "bare: lines {counts:?}, turned {turned}, {} correspondences, mean residual {residual:.2} px",
        pairs.len()
    ));
    let located = Located::new(
        fit,
        Anchors::ChecksumBoxes,
        Layout::Upstream,
        crate::locate::Geometry::strip(),
    )
    .ok_or(LocateError::NoChecksumBoxes { rows: rows.len() })?;
    Ok(located)
}

/// A row's two shaded boxes, which touch and so are one blob: a solid
/// grey rectangle about twice as long as it is high, in photo pixels.
struct PairBlob {
    centre: (f64, f64),
    /// Direction of the long axis, sign-free.
    angle: f64,
    long: f64,
}

impl PairBlob {
    fn of(blob: &Blob, factor: u32) -> Option<PairBlob> {
        let (angle, long, short) = blob.principal_axis();
        let s = f64::from(factor);
        let aspect = long / short.max(f64::EPSILON);
        let fill = f64::from(blob.area) / (long * short).max(f64::EPSILON);
        let expected = 2.0 * Strip::BOX_ACROSS / Strip::UPSTREAM_BOX_ALONG;
        let min_area = 8 * 16 / (factor * factor);
        if blob.area < min_area
            || !(expected * 0.7..=expected * 1.35).contains(&aspect)
            || fill < 0.7
        {
            return None;
        }
        let (x, y) = blob.centroid();
        Some(PairBlob {
            centre: (x * s + (s - 1.0) / 2.0, y * s + (s - 1.0) / 2.0),
            angle,
            long: long * s,
        })
    }
}

/// The photo downsampled to cells of `factor` pixels, each on when enough
/// of its pixels are printed grey.
fn grey_boxes_mask(
    photo: &GrayImage,
    integral: &Integral,
    radius: usize,
    factor: u32,
) -> GrayImage {
    let (w, h) = (photo.width() / factor, photo.height() / factor);
    let mut out = GrayImage::new(w, h);
    let cell = factor * factor;
    for by in 0..h {
        for bx in 0..w {
            let (cx, cy) = (bx * factor + factor / 2, by * factor + factor / 2);
            let paper = integral.local_mean(cx as usize, cy as usize, radius);
            let (lo, hi) = (paper * GREY_BAND.0, paper * GREY_BAND.1);
            let mut grey = 0;
            for y in by * factor..(by + 1) * factor {
                for x in bx * factor..(bx + 1) * factor {
                    let v = f64::from(photo.get_pixel(x, y).0[0]);
                    if v >= lo && v <= hi {
                        grey += 1;
                    }
                }
            }
            if f64::from(grey) >= GREY_FILL * f64::from(cell) {
                out.put_pixel(bx, by, Luma([255]));
            }
        }
    }
    out
}

/// One row of the lattice: its number from the top, and the two box
/// centres in photo pixels, first the one on the reading-left.
struct Row {
    index: usize,
    boxes: [(f64, f64); 2],
}

/// The two box centres of a pair `length` long centred at `centre`, the
/// across axis `d`, the first on the reading-left.
fn box_centres(centre: (f64, f64), length: f64, d: (f64, f64)) -> [(f64, f64); 2] {
    let quarter = length / 4.0;
    [
        (centre.0 - quarter * d.0, centre.1 - quarter * d.1),
        (centre.0 + quarter * d.0, centre.1 + quarter * d.1),
    ]
}

/// When the rows found span eleven of the twelve, one end row is missing
/// and the numbering does not say which. The shaded run is looked for one
/// pitch above the first row and one pitch below the last; the end that
/// has it is the missing row, and the rows are renumbered to match.
fn settle_end_row(
    photo: &GrayImage,
    rows: &mut [Row],
    extents: &[((f64, f64), f64, f64, f64)],
    across: (f64, f64),
    along: (f64, f64),
    pitch: f64,
) {
    let span = rows.last().map_or(0, |r| r.index);
    if span != Strip::ROWS - 2 || rows.len() < 2 {
        return;
    }
    let (first, last) = (extents[0], extents[extents.len() - 1]);
    let shaded_at = |(centre, length, height, paper): ((f64, f64), f64, f64, f64), sign: f64| {
        let c = (
            centre.0 + sign * pitch * along.0,
            centre.1 + sign * pitch * along.1,
        );
        run_extent(photo, paper, c, across, length)
            .and_then(|(c, _)| run_extent(photo, paper, c, along, height))
            .is_some()
    };
    let above = shaded_at(first, -1.0);
    let below = shaded_at(last, 1.0);
    if above && !below {
        for row in rows.iter_mut() {
            row.index += 1;
        }
    }
}

/// The paper's brightness at a row: the median of the pixels in the
/// blank gaps above and below its shaded pair, along the strip through
/// the pair's centre — always paper, whatever the table.
fn row_paper(
    photo: &GrayImage,
    centre: (f64, f64),
    along: (f64, f64),
    height: f64,
    pitch: f64,
) -> f64 {
    let mut samples = Vec::new();
    for sign in [-1.0, 1.0] {
        let mut t = height / 2.0 + 3.0;
        while t < pitch - height / 2.0 - 3.0 {
            let (x, y) = (centre.0 + sign * t * along.0, centre.1 + sign * t * along.1);
            if x >= 0.0 && y >= 0.0 && x < f64::from(photo.width()) && y < f64::from(photo.height())
            {
                samples.push(f64::from(photo.get_pixel(x as u32, y as u32).0[0]));
            }
            t += 1.0;
        }
    }
    if samples.is_empty() {
        return 255.0;
    }
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// Paper this many pixels wide ends the shaded run; an ink stroke across
/// the box is narrower.
const RUN_BREAK_PX: usize = 8;

/// Re-measures a shaded run at full resolution: its extent along `d`
/// through `centre`, ink strokes and all, expected to be about `coarse`
/// long. Returns the run's centre and length. A coarse walk finds the
/// run and the shade inside it; the edges are then where the brightness
/// crosses halfway between that shade and the paper, which blur moves
/// neither in nor out. The quarter-resolution blob gives these to a
/// pixel or so, and the far letter boxes are three box-widths of
/// extrapolation away.
fn run_extent(
    photo: &GrayImage,
    paper: f64,
    centre: (f64, f64),
    d: (f64, f64),
    coarse: f64,
) -> Option<((f64, f64), f64)> {
    let value = |t: f64| -> Option<f64> {
        let (x, y) = (centre.0 + t * d.0, centre.1 + t * d.1);
        if x < 0.0 || y < 0.0 || x >= f64::from(photo.width()) || y >= f64::from(photo.height()) {
            return None;
        }
        Some(f64::from(photo.get_pixel(x as u32, y as u32).0[0]))
    };
    let shaded = |t: f64| value(t).is_some_and(|v| v <= paper * GREY_BAND.1);
    let rough_edge = |sign: f64| {
        let mut last_shaded = 0.0;
        let mut blank = 0;
        let mut t = 0.0;
        while t < coarse {
            if shaded(sign * t) {
                last_shaded = t;
                blank = 0;
            } else {
                blank += 1;
                if blank >= RUN_BREAK_PX {
                    break;
                }
            }
            t += 1.0;
        }
        last_shaded
    };
    let (back, forward) = (rough_edge(-1.0), rough_edge(1.0));
    if back + forward < coarse * 0.5 {
        return None;
    }
    let mut inside: Vec<f64> = (0..)
        .map(f64::from)
        .take_while(|&t| t < forward * 0.8)
        .filter_map(&value)
        .chain(
            (1..)
                .map(f64::from)
                .take_while(|&t| t < back * 0.8)
                .filter_map(|t| value(-t)),
        )
        .collect();
    if inside.is_empty() {
        return None;
    }
    inside.sort_by(f64::total_cmp);
    let shade = inside[inside.len() / 2];
    let mid = (paper + shade) / 2.0;
    let smooth =
        |t: f64| -> Option<f64> { Some((value(t - 1.0)? + value(t)? + value(t + 1.0)?) / 3.0) };
    let fine_edge = |sign: f64, rough: f64| {
        let mut t = 0.0;
        while t < rough + RUN_BREAK_PX as f64 {
            match smooth(sign * t) {
                Some(v) if v > mid => return t - 0.5,
                Some(_) => t += 1.0,
                None => break,
            }
        }
        rough + 0.5
    };
    let (back, forward) = (fine_edge(-1.0, back), fine_edge(1.0, forward));
    let length = back + forward;
    if !(0.8..1.25).contains(&(length / coarse)) {
        return None;
    }
    let shift = (forward - back) / 2.0;
    Some(((centre.0 + shift * d.0, centre.1 + shift * d.1), length))
}

fn midpoint(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Sorts the pair blobs into rows and numbers the rows; returns them
/// with the across axis. The rows' pairs agree with each other in length
/// and direction; letters written in a grey pen can pass for pairs one
/// at a time but not as a set, so the best-supported blob names the set.
/// Rows are sorted along the perpendicular and numbered by their
/// spacing, which must reach from the first row to the twelfth.
fn lattice(pairs: &[PairBlob]) -> Result<(Vec<Row>, (f64, f64)), LocateError> {
    if pairs.len() < MIN_ROWS {
        return Err(LocateError::NoChecksumBoxes { rows: pairs.len() });
    }
    let agrees = |a: &PairBlob, b: &PairBlob| {
        (0.8..1.25).contains(&(a.long / b.long))
            && ((a.angle - b.angle).cos()).abs() > 15f64.to_radians().cos()
    };
    let support: Vec<usize> = pairs
        .iter()
        .map(|a| pairs.iter().filter(|b| agrees(a, b)).count())
        .collect();
    let (seed, _) = support
        .iter()
        .enumerate()
        .max_by_key(|&(i, &n)| (n, pairs[i].long as u64))
        .expect("at least MIN_ROWS pairs");
    let alike: Vec<&PairBlob> = pairs.iter().filter(|p| agrees(&pairs[seed], p)).collect();
    let mut lengths: Vec<f64> = alike.iter().map(|p| p.long).collect();
    lengths.sort_by(f64::total_cmp);
    let pair_length = lengths[lengths.len() / 2];
    let (mut sx, mut sy) = (0.0, 0.0);
    for p in &alike {
        sx += (2.0 * p.angle).cos();
        sy += (2.0 * p.angle).sin();
    }
    let axis = sy.atan2(sx) / 2.0;
    let d = (axis.cos(), axis.sin());
    let normal = (-d.1, d.0);
    // Rows lie in one column: pairs off to the side are something else.
    let mut offsets: Vec<f64> = alike
        .iter()
        .map(|p| p.centre.0 * d.0 + p.centre.1 * d.1)
        .collect();
    offsets.sort_by(f64::total_cmp);
    let column = offsets.get(offsets.len() / 2).copied().unwrap_or(0.0);
    let mut rows: Vec<(f64, [(f64, f64); 2])> = alike
        .iter()
        .filter(|p| (p.centre.0 * d.0 + p.centre.1 * d.1 - column).abs() <= 0.5 * pair_length)
        .map(|p| {
            (
                p.centre.0 * normal.0 + p.centre.1 * normal.1,
                box_centres(p.centre, p.long, d),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    if rows.len() < MIN_ROWS {
        return Err(LocateError::NoChecksumBoxes { rows: rows.len() });
    }
    let mut pitch = pair_length * Strip::ROW_PITCH / (2.0 * Strip::BOX_ACROSS);
    let mut numbered = vec![Row {
        index: 0,
        boxes: rows[0].1,
    }];
    for window in rows.windows(2) {
        let gap = window[1].0 - window[0].0;
        let steps = (gap / pitch).round() as usize;
        if steps == 0 {
            continue;
        }
        pitch = gap / steps as f64;
        let index = numbered.last().map_or(0, |r| r.index) + steps;
        numbered.push(Row {
            index,
            boxes: window[1].1,
        });
    }
    let span = numbered.last().map_or(0, |r| r.index);
    if numbered.len() < MIN_ROWS || span > Strip::ROWS - 1 || span + 1 < Strip::ROWS - 1 {
        return Err(LocateError::NoChecksumBoxes {
            rows: numbered.len().min(Strip::ROWS),
        });
    }
    Ok((numbered, d))
}

/// Template row and first column for a lattice row, the strip the given
/// way up.
fn template_row(index: usize, turned: bool) -> (usize, usize) {
    if turned {
        (Strip::ROWS - 1 - index, 1)
    } else {
        (index, 0)
    }
}

/// Template box centre for a lattice row and column, the strip the given
/// way up.
fn template_centre(index: usize, column: usize, turned: bool) -> (f64, f64) {
    let (row, first) = template_row(index, turned);
    let position = if first == 0 { column } else { 1 - column };
    Layout::Upstream.cell_rect(row, position).center()
}

/// `(template mm, photo px)` for every box, the strip the given way up.
fn correspondences(rows: &[Row], turned: bool) -> Vec<Correspondence> {
    rows.iter()
        .flat_map(|row| {
            (0..2).map(move |column| {
                (
                    template_centre(row.index, column, turned),
                    row.boxes[column],
                )
            })
        })
        .collect()
}

/// A dotted line is followed while its gaps stay this short, in pixels.
const LINE_GAP_PX: usize = 10;
/// How far, along the line's normal, the tracker looks for the next dot.
const LINE_DRIFT_PX: i32 = 3;
/// Three pixels along the line average at most this bright relative to
/// the paper where a dot is.
const LINE_DARK: f64 = 0.88;
/// A tracked line must run between these shares of its expected length:
/// shorter is a letter or a smudge, longer ran on past the line's end.
const LINE_LENGTH: (f64, f64) = (0.75, 1.15);
/// Above this mean reprojection error the line ends are suspected.
const OUTLIER_RESIDUAL_PX: f64 = 2.5;
/// A line end whose residual is this many times the mean is dropped.
const OUTLIER_FACTOR: f64 = 2.0;
/// How far beside the line the paper is checked, in pixels.
const LINE_BESIDE_PX: i32 = LINE_DRIFT_PX + 3;
/// The paper beside a line must be at least this much brighter than the
/// line: a dark table is dark on both counts and is not a line.
const LINE_CONTRAST: f64 = 1.06;
/// How far the bottom line runs beyond the shaded pair: the four letter
/// boxes.
const LINE_BEYOND_PAIR: f64 = 4.0 * Strip::BOX_ACROSS;
/// How far from the pair's corner the line may start, in pixels.
const LINE_START_PX: i32 = 6;
/// Rows whose bottom line was tracked on the same side and edge, for
/// the orientation to be decided when no other side and edge tracked
/// any; with opposition, one more, and none may reach a third as many.
const MIN_TRACKED_ROWS: usize = 3;

/// Follows a dotted line from `start` along `dir`, letting it drift along
/// `normal`, and returns where it ends if it ran at least `LINE_ENOUGH`
/// of `expected` pixels.
fn track_line(
    photo: &GrayImage,
    paper: f64,
    start: (f64, f64),
    dir: (f64, f64),
    normal: (f64, f64),
    expected: f64,
) -> Option<Track> {
    let pixel = |t: f64, k: i32| -> Option<f64> {
        let x = start.0 + t * dir.0 + f64::from(k) * normal.0;
        let y = start.1 + t * dir.1 + f64::from(k) * normal.1;
        if x < 0.0 || y < 0.0 || x >= f64::from(photo.width()) || y >= f64::from(photo.height()) {
            return None;
        }
        let (xi, yi) = (x as u32, y as u32);
        Some(f64::from(photo.get_pixel(xi, yi).0[0]) / paper.max(1.0))
    };
    // Three pixels along the line at a time: a dot spans them, noise on
    // blank paper averages out.
    let sample = |t: f64, k: i32| -> Option<f64> {
        Some((pixel(t - 1.0, k)? + pixel(t, k)? + pixel(t + 1.0, k)?) / 3.0)
    };
    let limit = (expected * 1.3) as usize;
    let follow = |mut offset: i32| -> Vec<(f64, i32)> {
        let mut path = Vec::new();
        let mut gap = 0;
        for step in 1..=limit {
            let t = step as f64;
            let darkest = (offset - LINE_DRIFT_PX..=offset + LINE_DRIFT_PX)
                .filter_map(|k| sample(t, k).map(|v| (v, k)))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match darkest {
                Some((v, k)) if v <= LINE_DARK => {
                    offset = k;
                    path.push((t, k));
                    gap = 0;
                }
                Some(_) => {
                    gap += 1;
                    if gap > LINE_GAP_PX {
                        break;
                    }
                }
                None => break,
            }
        }
        path
    };
    // The corner the pair's extents predict may be a few pixels off the
    // line; the start that follows it furthest is the line's.
    let path = (-LINE_START_PX..=LINE_START_PX)
        .map(follow)
        .max_by_key(|p| p.len())?;
    let &(t, k) = path.last()?;
    if !(LINE_LENGTH.0 * expected..=LINE_LENGTH.1 * expected).contains(&t) {
        return None;
    }
    // A line runs near where it started — a separator line a millimetre
    // off is another line — and has paper on both sides of it.
    let mean_offset = path.iter().map(|p| f64::from(p.1.abs())).sum::<f64>() / path.len() as f64;
    if mean_offset > f64::from(LINE_START_PX) {
        return None;
    }
    let (mut on, mut beside, mut n) = (0.0, 0.0, 0.0);
    for &(t, k) in &path {
        if let (Some(a), Some(b), Some(c)) = (
            pixel(t, k),
            pixel(t, k - LINE_BESIDE_PX),
            pixel(t, k + LINE_BESIDE_PX),
        ) {
            on += a;
            beside += (b + c) / 2.0;
            n += 1.0;
        }
    }
    if n == 0.0 || beside / n < LINE_CONTRAST * (on / n) {
        return None;
    }
    Some(Track {
        end: (
            start.0 + t * dir.0 + f64::from(k) * normal.0,
            start.1 + t * dir.1 + f64::from(k) * normal.1,
        ),
        path,
    })
}

/// A dotted line followed from a pair's corner: where it ended, and
/// every step it was seen at, as `(distance along, offset across)`.
struct Track {
    end: (f64, f64),
    path: Vec<(f64, i32)>,
}

/// A row's tracked bottom line with what the ticks along it need.
struct Tracked {
    index: usize,
    track: Track,
    start: (f64, f64),
    paper: f64,
    length: f64,
}

/// How far inside the box, from the bottom line, a border tick is looked
/// for, in pixels: the ticks rise a third of the box.
const TICK_INSIDE_PX: [i32; 3] = [3, 4, 5];
/// How far either way from where the frame predicts a tick, as a share
/// of a box width, the tick is looked for.
const TICK_SLACK: f64 = 0.2;

impl Tracked {
    /// The border ticks along the tracked bottom line: upstream draws a
    /// short dotted stroke rising from the line at every letter-box
    /// border, so each is an anchor at a known place on the strip,
    /// measured where it is — no perspective assumed, no dash phase to
    /// guess. Returns `(border number 1..=4, photo point on the line)`.
    fn ticks(
        &self,
        photo: &GrayImage,
        dir: (f64, f64),
        normal: (f64, f64),
        edge: f64,
    ) -> Vec<(usize, (f64, f64))> {
        let (paper, start, box_px, track) =
            (self.paper, self.start, self.length / 2.0, &self.track);
        let pixel = |t: f64, k: i32| -> Option<f64> {
            let x = start.0 + t * dir.0 + f64::from(k) * normal.0;
            let y = start.1 + t * dir.1 + f64::from(k) * normal.1;
            if x < 0.0 || y < 0.0 || x >= f64::from(photo.width()) || y >= f64::from(photo.height())
            {
                return None;
            }
            Some(f64::from(photo.get_pixel(x as u32, y as u32).0[0]) / paper.max(1.0))
        };
        let inside = |t: f64, k: i32| -> Option<f64> {
            let mut sum = 0.0;
            for j in TICK_INSIDE_PX {
                sum += pixel(t, k - (edge as i32) * j)?;
            }
            Some(sum / TICK_INSIDE_PX.len() as f64)
        };
        let mut found = Vec::new();
        for border in 1..=4usize {
            let expected = box_px * border as f64;
            let best = track
                .path
                .iter()
                .filter(|(t, _)| (t - expected).abs() <= TICK_SLACK * box_px)
                .filter_map(|&(t, k)| inside(t, k).map(|v| (v, t, k)))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((v, t, k)) = best
                && v <= LINE_DARK
            {
                found.push((
                    border,
                    (
                        start.0 + t * dir.0 + f64::from(k) * normal.0,
                        start.1 + t * dir.1 + f64::from(k) * normal.1,
                    ),
                ));
            }
        }
        found
    }
}
