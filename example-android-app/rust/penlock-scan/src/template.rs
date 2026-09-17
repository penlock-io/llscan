//! Rendering primitives shared by the worksheet: rectangles in
//! millimetres, the bundled fonts, handwriting jitter, SVG text, and the
//! resvg rasteriser. The worksheet itself is [`crate::sheet`].

use std::fmt;
use std::sync::Arc;

use image::RgbImage;
use penlock::Share;
use penlock::word::symbols_to_string;

/// Words on a strip: a 12-word share, one row each.
pub const WORDS: usize = 12;

/// A rectangle in millimetres, origin at the card's top-left corner.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// Centre point.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `other` lies entirely inside this rectangle.
    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.w <= self.x + self.w
            && other.y + other.h <= self.y + self.h
    }

    /// Whether the two rectangles overlap with positive area.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    /// This rectangle grown by `d` on every side.
    pub fn grow(&self, d: f64) -> Rect {
        Rect {
            x: self.x - d,
            y: self.y - d,
            w: self.w + 2.0 * d,
            h: self.h + 2.0 * d,
        }
    }
}

/// A bundled font to fill cells with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Font {
    /// Patrick Hand: upright handwriting.
    PatrickHand,
    /// Caveat: looser handwriting.
    Caveat,
    /// IBM Plex Mono: printed.
    PlexMono,
}

impl Font {
    /// Every bundled font.
    pub const ALL: [Font; 3] = [Font::PatrickHand, Font::Caveat, Font::PlexMono];

    /// The name accepted on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Font::PatrickHand => "patrick-hand",
            Font::Caveat => "caveat",
            Font::PlexMono => "plex-mono",
        }
    }

    /// Looks a font up by [`Font::name`].
    pub fn from_name(name: &str) -> Option<Font> {
        Font::ALL.into_iter().find(|f| f.name() == name)
    }

    pub(crate) fn family(self) -> &'static str {
        match self {
            Font::PatrickHand => "Patrick Hand",
            Font::Caveat => "Caveat",
            Font::PlexMono => "IBM Plex Mono",
        }
    }

    fn data(self) -> &'static [u8] {
        match self {
            Font::PatrickHand => include_bytes!("../fonts/PatrickHand-Regular.ttf"),
            Font::Caveat => include_bytes!("../fonts/Caveat-Variable.ttf"),
            Font::PlexMono => include_bytes!("../fonts/IBMPlexMono-Regular.ttf"),
        }
    }
}

/// How far each written symbol may stray from its ideal placement,
/// imitating a hand that does not centre every character perfectly.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Jitter {
    /// Maximum shift of the symbol in each axis, millimetres.
    pub offset_mm: f64,
    /// Maximum relative change of the symbol's size, e.g. 0.15 for ±15%.
    pub size: f64,
    /// Maximum rotation of the symbol, degrees.
    pub rotation_deg: f64,
}

impl Jitter {
    /// Placement variance of a reasonably careful hand.
    pub const HAND: Jitter = Jitter {
        offset_mm: 0.8,
        size: 0.15,
        rotation_deg: 8.0,
    };
}

pub(crate) const INK: &str = "#000000";
pub(crate) const GUIDE: &str = "#c8c8c8";
pub(crate) const CHECKSUM_FILL: &str = "#f2f2f2";
/// Upstream shades its checksum boxes mid-grey.
pub(crate) const UPSTREAM_CHECKSUM_FILL: &str = "#a8a8a8";
pub(crate) const TEXT: &str = "#808080";
pub(crate) const LABEL_FONT: &str = "IBM Plex Mono";
pub(crate) const CELL_FONT_SIZE: f64 = 6.5;

/// Baseline offset that centres capitals vertically for the bundled fonts.
const BASELINE_SHIFT: f64 = 0.35;

/// Where a symbol's text element sits for a cell centred at `(cx, cy)`,
/// as it appears in [`crate::sheet::strip_group`]'s output.
pub fn cell_text_anchor(cx: f64, cy: f64) -> (f64, f64) {
    (cx, cy + CELL_FONT_SIZE * BASELINE_SHIFT)
}

pub(crate) fn text(
    x: f64,
    cy: f64,
    size: f64,
    anchor: &str,
    family: &str,
    color: &str,
    body: &str,
) -> String {
    text_spaced(x, cy, size, anchor, family, color, body, 0.0)
}

/// [`text`] with `spacing` extra user units between letters; the
/// attribute is written only when it is non-zero, so the sheets'
/// authored SVG stays byte for byte what it was.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_spaced(
    x: f64,
    cy: f64,
    size: f64,
    anchor: &str,
    family: &str,
    color: &str,
    body: &str,
    spacing: f64,
) -> String {
    let body = body
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    // A negative value sets the letters closer than the face's own
    // advances, which is how a fast hand writes; only exactly zero
    // means "leave the face alone".
    let spacing = if spacing == 0.0 {
        String::new()
    } else {
        format!(" letter-spacing=\"{spacing:.2}\"")
    };
    format!(
        "<text x=\"{x:.2}\" y=\"{:.2}\" font-size=\"{size:.2}\" text-anchor=\"{anchor}\" font-family=\"{family}\" fill=\"{color}\"{spacing}>{body}</text>\n",
        cy + size * BASELINE_SHIFT
    )
}

/// Rough width of `text` set in the label font at `size`, for layout
/// checks; the mono face is about 0.6 em wide per character.
pub fn label_width(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * 0.6
}

pub(crate) fn svg_document(width: f64, height: f64, body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}mm\" height=\"{height}mm\" viewBox=\"0 0 {width} {height}\">\n{body}</svg>\n"
    )
}

/// Why rendering failed.
#[derive(Debug)]
pub enum RenderError {
    /// The SVG could not be parsed or rasterised.
    Svg(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Svg(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for RenderError {}

/// CSS pixels per inch, the scale `usvg` reads millimetre sizes at.
const CSS_DPI: f64 = 96.0;

fn usvg_options() -> resvg::usvg::Options<'static> {
    let mut options = resvg::usvg::Options::default();
    let db = Arc::make_mut(&mut options.fontdb);
    for font in Font::ALL {
        db.load_font_data(font.data().to_vec());
    }
    options
}

fn parse(svg: &str) -> Result<resvg::usvg::Tree, RenderError> {
    resvg::usvg::Tree::from_str(svg, &usvg_options()).map_err(|e| RenderError::Svg(e.to_string()))
}

/// Rewrites an authored SVG with every glyph as a path and no font
/// references, keeping its physical size in millimetres.
pub(crate) fn flatten(svg: &str, width_mm: f64, height_mm: f64) -> Result<String, RenderError> {
    let tree = parse(svg)?;
    let written = tree.to_string(&resvg::usvg::WriteOptions::default());
    let Some(end) = written.find('>') else {
        return Err(RenderError::Svg("writer produced no root element".into()));
    };
    let root = &written[..end];
    let root = set_attribute(root, "width", &format!("{width_mm}mm"));
    let root = set_attribute(&root, "height", &format!("{height_mm}mm"));
    Ok(format!("{root}{}", &written[end..]))
}

fn set_attribute(tag: &str, name: &str, value: &str) -> String {
    let needle = format!(" {name}=\"");
    match tag.find(&needle) {
        Some(start) => {
            let value_start = start + needle.len();
            let value_end = value_start + tag[value_start..].find('"').unwrap_or(0);
            format!("{}{value}{}", &tag[..value_start], &tag[value_end..])
        }
        None => format!("{tag}{needle}{value}\""),
    }
}

/// Renders an SVG produced by this module at `dpi`.
pub fn rasterize(svg: &str, dpi: f64) -> Result<RgbImage, RenderError> {
    render(&parse(svg)?, dpi)
}

fn render(tree: &resvg::usvg::Tree, dpi: f64) -> Result<RgbImage, RenderError> {
    let scale = dpi / CSS_DPI;
    let size = tree.size();
    let width = (f64::from(size.width()) * scale).ceil() as u32;
    let height = (f64::from(size.height()) * scale).ceil() as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| RenderError::Svg(format!("cannot allocate a {width}×{height} pixmap")))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );
    let mut out = RgbImage::new(width, height);
    for (px, out_px) in pixmap.pixels().iter().zip(out.pixels_mut()) {
        let c = px.demultiply();
        *out_px = image::Rgb([c.red(), c.green(), c.blue()]);
    }
    Ok(out)
}

/// Pixels per millimetre at `dpi`.
pub fn px_per_mm(dpi: f64) -> f64 {
    dpi / 25.4
}

/// Symbols of a share as the characters written on the card, for display.
pub fn share_rows(share: &Share) -> Vec<String> {
    share
        .words()
        .iter()
        .map(|w| symbols_to_string(*w))
        .collect()
}
