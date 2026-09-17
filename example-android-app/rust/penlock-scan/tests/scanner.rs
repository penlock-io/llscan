//! The pipeline's entry points, and the shape of what they return.
//!
//! The sibling word-list pipeline answers the same kind of question, so these
//! pin the things that make the two interchangeable to a caller: bytes and
//! pixels read the same, the projection is serializable, and the decode limits
//! are the library's rather than each pipeline's own.
use penlock::word::WORD_LEN;
use penlock::{Distribution, Symbol};
use penlock_scan::model::{CellReadings, Model, ModelError};
use penlock_scan::{CellImage, ScanOptions, Scanner};

/// What the pipeline promises a caller holds whatever is behind the cells, so
/// these run on every build rather than only where a backend is compiled in.
struct Certain(char);

impl Model for Certain {
    fn read_cells(
        &self,
        cells: &[[CellImage; WORD_LEN]],
        _rectified: &image::GrayImage,
    ) -> Result<CellReadings, ModelError> {
        let mut probabilities = [0.01f32; 29];
        probabilities[usize::from(Symbol::from_char(self.0).unwrap().value())] = 1. - 0.01 * 28.;
        let row = std::array::from_fn(|_| Distribution::new(probabilities).unwrap());
        Ok(CellReadings {
            cells: cells.iter().map(|_| row.clone()).collect(),
            unread: Vec::new(),
        })
    }

    fn spec(&self) -> String {
        "certain".into()
    }
}

fn scanner() -> Scanner {
    Scanner::from_model(Box::new(Certain('K')))
}

fn strip() -> image::RgbImage {
    let dpi = 200.;
    let page = penlock_scan::sheet::front_png(None, dpi).unwrap();
    penlock_scan::sheet::cut_strip(&page, dpi, 1)
}

fn encoded(image: &image::RgbImage) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn encoded_bytes_and_canonical_pixels_read_the_same_strip() {
    let scanner = scanner();
    let strip = strip();
    let from_pixels = scanner.scan_rgb(&strip, ScanOptions::default()).unwrap();
    let from_bytes = scanner
        .scan(&encoded(&strip), ScanOptions::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(from_bytes.document()).unwrap(),
        serde_json::to_value(from_pixels.document()).unwrap()
    );
}

#[test]
fn the_projection_carries_a_cell_for_every_position() {
    let scanner = scanner();
    let document = scanner
        .scan_rgb(&strip(), ScanOptions::default())
        .unwrap()
        .document();
    let observation = &document.observations[0];
    assert_eq!(observation.words.len(), 12);
    assert_eq!(observation.text.len(), 12);
    for word in &observation.words {
        assert_eq!(word.len(), 6);
        for (position, cell) in word.iter().enumerate() {
            assert_eq!(cell.position, position);
            assert_eq!(cell.selected.is_none(), cell.unread);
            // Candidates are diagnostics, not the reading.
            assert!(cell.candidates.is_empty());
        }
    }

    let traced = scanner
        .scan_rgb(
            &strip(),
            ScanOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .unwrap()
        .document();
    assert_eq!(traced.observations[0].words[0][0].candidates.len(), 29);
}

#[test]
fn an_oversize_photograph_is_refused_before_anything_is_decoded() {
    let refusal = scanner().scan(&vec![0u8; 65 * 1024 * 1024], ScanOptions::default());
    assert!(matches!(refusal, Err(penlock_scan::ScanError::Image(_))));
}

/// Handing in pixels is not a way past the limits the decoder applies: a
/// caller who decodes an image themselves meets the same boundary.
#[test]
fn pixels_the_caller_decoded_meet_the_same_limits() {
    let scanner = scanner();
    for (width, height) in [(0, 8), (8, 0), (8_000, 5_001)] {
        let image = image::RgbImage::new(width, height);
        assert!(
            matches!(
                scanner.scan_rgb(&image, ScanOptions::default()),
                Err(penlock_scan::ScanError::Image(_))
            ),
            "{width}x{height} through scan_rgb"
        );
        assert!(
            matches!(
                scanner.scan_all_rgb(&image, ScanOptions::default()),
                Err(penlock_scan::ScanError::Image(_))
            ),
            "{width}x{height} through scan_all_rgb"
        );
    }
}

/// The backend-independent contracts above run on every build, which would
/// leave the real constructor uncovered if nothing asked for it.
#[cfg(feature = "onnx")]
#[test]
fn the_shipped_classifier_loads_from_bytes() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../app-models/cell-reference.onnx");
    let scanner = Scanner::new(penlock_scan::ModelBytes {
        cells: &std::fs::read(path).unwrap(),
    })
    .expect("the shipped classifier meets the cell ABI");
    let read = scanner.scan_rgb(&strip(), ScanOptions::default()).unwrap();
    assert_eq!(read.document().observations.len(), 1);
}
