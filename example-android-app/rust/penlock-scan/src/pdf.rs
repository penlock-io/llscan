//! Sides of the worksheet as pages of one PDF, for printing at actual size.

use std::collections::HashMap;

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref};
use svg2pdf::usvg;

use crate::template::RenderError;

const POINTS_PER_MM: f64 = 72.0 / 25.4;

struct Refs(i32);

impl Refs {
    fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

/// A PDF with one page per SVG in `pages`, each `page_mm` wide and high,
/// the SVG scaled to fill it.
pub fn document(pages: &[&str], page_mm: (f64, f64)) -> Result<Vec<u8>, RenderError> {
    let mut refs = Refs(0);
    let catalog = refs.next();
    let page_tree = refs.next();
    let (width, height) = (
        (page_mm.0 * POINTS_PER_MM) as f32,
        (page_mm.1 * POINTS_PER_MM) as f32,
    );
    let mut pdf = Pdf::new();
    let mut page_ids = Vec::with_capacity(pages.len());
    for svg in pages {
        let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
            .map_err(|e| RenderError::Svg(e.to_string()))?;
        let (chunk, xobject) = svg2pdf::to_chunk(&tree, svg2pdf::ConversionOptions::default())
            .map_err(|e| RenderError::Svg(e.to_string()))?;
        let mut renumbered = HashMap::new();
        let chunk = chunk.renumber(|old| *renumbered.entry(old).or_insert_with(|| refs.next()));
        let xobject = renumbered[&xobject];
        let page_id = refs.next();
        let content_id = refs.next();
        page_ids.push(page_id);

        let side = Name(b"Side");
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, width, height));
        page.parent(page_tree);
        page.contents(content_id);
        page.resources().x_objects().pair(side, xobject);
        page.finish();
        let mut content = Content::new();
        content
            .transform([width, 0.0, 0.0, height, 0.0, 0.0])
            .x_object(side);
        pdf.stream(content_id, &content.finish());
        pdf.extend(&chunk);
    }
    pdf.catalog(catalog).pages(page_tree);
    pdf.pages(page_tree)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    Ok(pdf.finish())
}
