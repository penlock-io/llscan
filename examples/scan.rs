//! Usage: cargo run --release --features bundled-models --example scan -- photo.jpg new-output-dir
use base64::Engine as _;
use bitcoin_vision::{ScanOptions, ScanResult, Scanner, decode_image};
use std::{error::Error, fmt::Write as _, path::PathBuf};

const ACCEPTED: [u8; 3] = [0, 170, 114];
const UNCERTAIN: [u8; 3] = [216, 139, 0];

// Display-only previews. Recognition above always uses the full canonical photo.
fn preview(photo: &image::RgbImage) -> image::RgbImage {
    let scale = (1000. / photo.width().max(photo.height()) as f32).min(1.);
    image::imageops::thumbnail(
        photo,
        (photo.width() as f32 * scale).round().max(1.) as u32,
        (photo.height() as f32 * scale).round().max(1.) as u32,
    )
}

fn outline(image: &mut image::RgbImage, quad: &[(f32, f32); 4], scale: (f32, f32), color: [u8; 3]) {
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        for offset in -1..=1 {
            imageproc::drawing::draw_line_segment_mut(
                image,
                (a.0 * scale.0, a.1 * scale.1 + offset as f32),
                (b.0 * scale.0, b.1 * scale.1 + offset as f32),
                image::Rgb(color),
            );
        }
    }
}

fn accepted(scan: &ScanResult, observation: usize) -> bool {
    scan.page.words[observation]
        .selection
        .as_ref()
        .is_some_and(|s| s.confirmation() == bitcoin_vision::hybrid::Confirmation::Accepted)
}

fn ink(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

/// The picture the README shows: the display preview with every ordered word
/// outlined in photo coordinates. One renderer, so the committed image and
/// what a reader gets from this example cannot drift apart.
fn overlay(photo: &image::RgbImage, scan: &ScanResult) -> image::RgbImage {
    let mut annotated = preview(photo);
    let scale = (
        annotated.width() as f32 / photo.width() as f32,
        annotated.height() as f32 / photo.height() as f32,
    );
    let document = scan.document();
    for &index in &document.ordered_observations {
        let color = if accepted(scan, index) {
            ACCEPTED
        } else {
            UNCERTAIN
        };
        outline(
            &mut annotated,
            &document.observations[index].quad,
            scale,
            color,
        );
    }
    annotated
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: scan PHOTO NEW_OUTPUT_DIRECTORY".into());
    }
    let photo = decode_image(&std::fs::read(&args[0])?, None)?;
    let scanner = Scanner::bundled()?;
    let scan = scanner.scan_rgb(
        &photo,
        ScanOptions {
            diagnostics: true,
            ..Default::default()
        },
    )?;
    let document = scan.document();
    let output = PathBuf::from(&args[1]);
    // Refuse an existing directory: examples must not overwrite prior evidence.
    std::fs::create_dir(&output)?;
    std::fs::create_dir(output.join("crops"))?;
    for (i, word) in scan.page.words.iter().enumerate() {
        word.crop
            .save(output.join("crops").join(format!("{i}.png")))?;
    }
    photo.save(output.join("photo.png"))?;
    preview(&photo).save(output.join("preview.png"))?;
    let embedded =
        base64::engine::general_purpose::STANDARD.encode(std::fs::read(output.join("photo.png"))?);
    std::fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&document)?,
    )?;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\"><image href=\"data:image/png;base64,{embedded}\" width=\"{}\" height=\"{}\"/>",
        photo.width(),
        photo.height(),
        photo.width(),
        photo.height()
    );
    let stroke = (photo.width() as f32 / 600.).max(1.);
    for (position, &index) in document.ordered_observations.iter().enumerate() {
        let observation = &document.observations[index];
        let color = ink(if accepted(&scan, index) {
            ACCEPTED
        } else {
            UNCERTAIN
        });
        let points = observation
            .quad
            .iter()
            .map(|(x, y)| format!("{x},{y}"))
            .collect::<Vec<_>>()
            .join(" ");
        let (x, y) = observation.quad[0];
        write!(
            svg,
            "<polygon points=\"{points}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{stroke}\"/><text x=\"{x}\" y=\"{}\" fill=\"{color}\" font-family=\"sans-serif\" font-size=\"{}\">{}: {}</text>",
            (y - stroke * 2.).max(stroke * 6.),
            stroke * 7.,
            position + 1,
            observation.selected.unwrap_or("unknown")
        )?;
    }
    svg.push_str("</svg>\n");
    std::fs::write(output.join("overlay.svg"), svg)?;
    overlay(&photo, &scan).save(output.join("overlay.png"))?;
    // No seed phrase is logged. JSON output is explicitly requested on disk.
    println!(
        "Wrote {} observations and {} ordered words to {}",
        document.observations.len(),
        document.ordered_observations.len(),
        output.display()
    );
    Ok(())
}

#[test]
fn preview_is_display_only_and_outlines_use_photo_coordinates() {
    let source = image::RgbImage::from_pixel(2000, 1000, image::Rgb([255, 255, 255]));
    let mut displayed = preview(&source);
    assert_eq!(displayed.dimensions(), (1000, 500));
    outline(
        &mut displayed,
        &[(200., 200.), (600., 200.), (600., 400.), (200., 400.)],
        (0.5, 0.5),
        [0, 170, 114],
    );
    assert_eq!(displayed.get_pixel(100, 100).0, [0, 170, 114]);
    assert_eq!(displayed.get_pixel(200, 150).0, [255, 255, 255]);
    assert_eq!(source.dimensions(), (2000, 1000));
    assert!(source.pixels().all(|p| p.0 == [255, 255, 255]));
}

/// The README calls its overlay the actual output of this example, so a
/// pipeline change that moves a box has to move the committed picture too.
#[test]
fn the_readme_overlay_is_what_this_example_renders_today() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let photo = decode_image(
        &std::fs::read(root.join("examples/photos/handwritten.png")).unwrap(),
        None,
    )
    .unwrap();
    let scan = Scanner::bundled()
        .unwrap()
        .scan_rgb(
            &photo,
            ScanOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .unwrap();

    let document = scan.document();
    let read: Vec<_> = document
        .ordered_observations
        .iter()
        .map(|&index| document.observations[index].selected.unwrap_or("unknown"))
        .collect();
    assert_eq!(
        read.join(" "),
        "shop exhaust erase demand switch powder now fault anchor enact rather cook"
    );
    assert!(document.checksum_valid);

    let committed = image::open(root.join("docs/images/handwritten-boxes.png"))
        .unwrap()
        .to_rgb8();
    let rendered = overlay(&photo, &scan);
    assert_eq!(rendered.dimensions(), committed.dimensions());
    let moved = rendered
        .pixels()
        .zip(committed.pixels())
        .filter(|(drawn, shown)| drawn != shown)
        .count();
    assert_eq!(
        moved, 0,
        "docs/images/handwritten-boxes.png is stale: regenerate it with \
         `cargo run --release --features bundled-models --example scan`"
    );
}
