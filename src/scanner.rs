//! Consumer entry point; decoding and result projection do not change inference.

use crate::{
    detect::Detector,
    phrase::{PageScan, Reader},
    recogniser::Recogniser,
};
use image::{ImageDecoder, RgbImage};

/// Borrowed artifacts used to create a reusable scanner. Loading copies no
/// photos and performs no downloads; model plans own their parsed weights.
pub struct ModelBytes<'a> {
    /// PP-OCR detector ONNX bytes.
    pub detector: &'a [u8],
    /// BIP39 classifier ONNX bytes.
    pub classifier: &'a [u8],
    /// Classifier-bound calibration text.
    pub calibration: &'a str,
    /// PP-OCR English recognition ONNX bytes.
    pub recogniser: &'a [u8],
    /// Recognition character dictionary, one entry per line.
    pub dictionary: &'a str,
}

/// Per-call settings. Options never alter crop padding or vocabulary selection.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanOptions {
    /// Retain detailed processing decisions in the result (may contain text).
    pub diagnostics: bool,
    /// Independent evidence that the supplied decoded image is upright.
    pub photo_up: crate::page_frame::PhotoUp,
    /// Explicit EXIF transform (1..=8), overriding embedded metadata. Use1
    /// when a caller already normalized the pixels. None reads embedded EXIF.
    /// This option applies only to encoded image input, never `scan_rgb`.
    pub orientation: Option<u8>,
}

/// A model-loading, image-decoding or scan-processing error.
#[derive(Debug)]
pub enum ScanError {
    /// A required artifact failed to load or did not match its contract.
    Model(String),
    /// Image input could not be decoded within the configured limits.
    Image(String),
    /// Geometry, inference or final-region validation failed.
    Processing(String),
}
impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Model(e) => write!(f, "model: {e}"),
            Self::Image(e) => write!(f, "image: {e}"),
            Self::Processing(e) => write!(f, "scan: {e}"),
        }
    }
}
impl std::error::Error for ScanError {}

/// Native observations plus stage-owned final geometry and suggested order.
pub struct ScanResult {
    /// Observations, exact recognition crops, candidates and label evidence.
    pub page: PageScan,
    /// Final output regions, mapped to the page observations by the scanner.
    pub regions: Vec<crate::progress::FinalRegion>,
}

/// Reusable offline detector, classifier and text recogniser.
pub struct Scanner {
    detector: Detector,
    reader: Reader,
    recogniser: Recogniser,
}

impl Scanner {
    /// Loads the model files compiled into the optional Git-distributed bundle.
    #[cfg(feature = "bundled-models")]
    pub fn bundled() -> Result<Self, ScanError> {
        Self::new(ModelBytes {
            detector: include_bytes!("../models/word-detector.onnx"),
            classifier: include_bytes!("../models/word-reference.onnx"),
            calibration: include_str!("../models/word-reference.calibration"),
            recogniser: include_bytes!("../models/text-recogniser.onnx"),
            dictionary: include_str!("../models/text-recogniser.dict.txt"),
        })
    }
    /// Loads the complete model set, validating the classifier calibration.
    pub fn new(models: ModelBytes<'_>) -> Result<Self, ScanError> {
        Ok(Self {
            detector: Detector::from_bytes(models.detector).map_err(ScanError::Model)?,
            reader: Reader::from_bytes(models.classifier, models.calibration)
                .map_err(ScanError::Model)?,
            recogniser: Recogniser::from_bytes(models.recogniser, models.dictionary)
                .map_err(ScanError::Model)?,
        })
    }

    /// Scans decoded RGB pixels in their canonical orientation. Returned boxes
    /// are in this image's pixel coordinates; no implicit scaling or padding.
    pub fn scan_rgb(
        &self,
        image: &RgbImage,
        options: ScanOptions,
    ) -> Result<ScanResult, ScanError> {
        self.scan_rgb_observed(image, options, None)
    }

    /// Decodes JPEG, PNG or WebP and applies EXIF orientation exactly once.
    /// Limits:64MiB encoded input,40 million decoded pixels, decoder allocation
    /// limit512MiB (best-effort codec support). Does not resize accepted images.
    pub fn scan(&self, encoded: &[u8], options: ScanOptions) -> Result<ScanResult, ScanError> {
        let image = decode_image(encoded, options.orientation)?;
        self.scan_rgb(&image, options)
    }

    /// The same pipeline with optional synchronous progress observations.
    pub fn scan_rgb_observed(
        &self,
        image: &RgbImage,
        options: ScanOptions,
        observer: Option<&dyn crate::progress::Observer>,
    ) -> Result<ScanResult, ScanError> {
        use crate::progress::{Event, Phase, emit, work};
        // Region identity belongs to the result even when no UI subscribes.
        // The app/harness likewise keep the stage observer present for mapping.
        struct Ignore;
        impl crate::progress::Observer for Ignore {
            fn on_event(&self, _: Event) {}
        }
        let ignore = Ignore;
        let observer: Option<&dyn crate::progress::Observer> = Some(observer.unwrap_or(&ignore));
        work(observer, Phase::Preparing, 0, None);
        let result = (|| {
            check_dimensions(image.width(), image.height())?;
            emit(observer, || Event::Photo {
                width: image.width(),
                height: image.height(),
            });
            work(observer, Phase::Finding, 0, None);
            let quads = self.detector.detect(image).map_err(ScanError::Processing)?;
            let reader = self
                .reader
                .clone()
                .with_photo_up(options.photo_up)
                .with_decision_trace(options.diagnostics);
            let (page, regions) = crate::phrase::read_words_observed(
                image,
                &quads,
                &reader,
                Some(&self.recogniser),
                crate::phrase::CROP_MARGIN,
                observer,
            )
            .map_err(ScanError::Processing)?;
            work(observer, Phase::Packing, 0, None);
            Ok(ScanResult { page, regions })
        })();
        emit(observer, || match &result {
            Ok(r) => Event::Finished {
                regions: r.regions.clone(),
            },
            Err(_) => Event::Failed,
        });
        result
    }
}

/// The pixel-count limit every entry point applies, encoded or canonical.
/// Callers handing in their own decoded pixels — including the sibling Penlock
/// pipeline — admit them through here, so a caller cannot reach past the limit
/// by decoding an image themselves.
pub fn check_dimensions(width: u32, height: u32) -> Result<(), ScanError> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 40_000_000 {
        return Err(ScanError::Image(
            "image must contain 1..=40,000,000 pixels".into(),
        ));
    }
    Ok(())
}

/// Decodes canonical RGB pixels with the same limits/orientation as [`Scanner::scan`].
/// Use this to display the exact coordinate frame associated with returned boxes.
pub fn decode_image(encoded: &[u8], orientation: Option<u8>) -> Result<RgbImage, ScanError> {
    if encoded.len() > 64 * 1024 * 1024 {
        return Err(ScanError::Image("encoded image exceeds64MiB".into()));
    }
    if orientation.is_some_and(|v| !(1..=8).contains(&v)) {
        return Err(ScanError::Image(
            "orientation must be an EXIF transform in1..=8".into(),
        ));
    }
    let error = |e: image::ImageError| ScanError::Image(e.to_string());
    let mut reader = image::ImageReader::new(std::io::Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|e| ScanError::Image(e.to_string()))?;
    reader.limits(image::Limits::default());
    let mut decoder = reader.into_decoder().map_err(error)?;
    let (width, height) = decoder.dimensions();
    check_dimensions(width, height)?;
    let orientation = match orientation {
        Some(n) => image::metadata::Orientation::from_exif(n).expect("validated EXIF"),
        None => decoder.orientation().map_err(error)?,
    };
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    Ok(image.into_rgb8())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimensions_are_bounded_before_inference() {
        assert!(check_dimensions(0, 2).is_err());
        assert!(check_dimensions(40_000_001, 1).is_err());
        assert!(check_dimensions(u32::MAX, u32::MAX).is_err());
        assert!(check_dimensions(5000, 8000).is_ok());
    }
    #[test]
    fn all_exif_transforms_preserve_marker_coordinates() {
        let original = RgbImage::from_fn(3, 2, |x, y| image::Rgb([(1 + y * 3 + x) as u8, 0, 0]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        original
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let expected: [&[u8]; 8] = [
            &[1, 2, 3, 4, 5, 6],
            &[3, 2, 1, 6, 5, 4],
            &[6, 5, 4, 3, 2, 1],
            &[4, 5, 6, 1, 2, 3],
            &[1, 4, 2, 5, 3, 6],
            &[4, 1, 5, 2, 6, 3],
            &[6, 3, 5, 2, 4, 1],
            &[3, 6, 2, 5, 1, 4],
        ];
        for n in 1..=8 {
            let decoded = decode_image(encoded.get_ref(), Some(n)).unwrap();
            assert_eq!(
                decoded.pixels().map(|p| p[0]).collect::<Vec<_>>(),
                expected[n as usize - 1]
            );
        }
        assert_eq!(decode_image(encoded.get_ref(), None).unwrap(), original);
        assert!(decode_image(encoded.get_ref(), Some(0)).is_err());
        assert!(decode_image(encoded.get_ref(), Some(9)).is_err());
        assert!(decode_image(b"invalid", None).is_err());
    }
}
