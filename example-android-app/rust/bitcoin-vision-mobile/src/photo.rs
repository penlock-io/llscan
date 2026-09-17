//! One explicit orientation after decoding; never interpret EXIF a second time.

use crate::VisionError;
use bitcoin_vision::image::{ImageFormat, RgbImage, imageops};

/// Caller provenance for the top of the canonical photo, not another transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Enum)]
pub enum PhotoUpSource {
    /// Legacy caller or missing/invalid metadata; identity alone is not evidence.
    #[default]
    Unknown,
    /// Capture-time metadata, including rotation zero.
    Camera,
    /// Valid EXIF, including orientation 1.
    Exif,
    /// Caller explicitly declares the canonical photo upright.
    Explicit,
}

impl From<PhotoUpSource> for bitcoin_vision::page_frame::PhotoUp {
    fn from(value: PhotoUpSource) -> Self {
        match value {
            PhotoUpSource::Unknown => Self::Unknown,
            PhotoUpSource::Camera => Self::Camera,
            PhotoUpSource::Exif => Self::Exif,
            PhotoUpSource::Explicit => Self::Explicit,
        }
    }
}

pub(crate) fn decode(photo: &[u8], orientation: u8) -> Result<RgbImage, VisionError> {
    let _decode = bitcoin_vision::timing::span("mobile.decode");
    // One decoder for both pipelines, so a phone gets the same size and
    // pixel-count limits a caller of the library does. The app resolves EXIF
    // itself and hands the transform here, so the embedded value is never read
    // a second time.
    bitcoin_vision::decode_image(photo, Some(orientation)).map_err(|e| VisionError::Photo {
        detail: match e {
            bitcoin_vision::ScanError::Image(detail) => detail,
            other => other.to_string(),
        },
    })
}

#[allow(dead_code)]
fn orient(mut image: RgbImage, orientation: u8) -> RgbImage {
    match orientation {
        1 => image,
        2 => {
            imageops::flip_horizontal_in_place(&mut image);
            image
        }
        3 => {
            imageops::rotate180_in_place(&mut image);
            image
        }
        4 => {
            imageops::flip_vertical_in_place(&mut image);
            image
        }
        // Transpose and transverse: one destination buffer, then an in-place flip.
        5 => {
            let mut result = imageops::rotate90(&image);
            imageops::flip_horizontal_in_place(&mut result);
            result
        }
        6 => imageops::rotate90(&image),
        7 => {
            let mut result = imageops::rotate90(&image);
            imageops::flip_vertical_in_place(&mut result);
            result
        }
        8 => imageops::rotate270(&image),
        _ => unreachable!("validated by decode"),
    }
}

/// Explicit canonical export only: lossless pixels without a pending EXIF transform.
/// Scanning uses the decoded frame directly and never calls this encoder.
#[uniffi::export]
pub fn canonical_photo(photo: Vec<u8>, orientation: u8) -> Result<Vec<u8>, VisionError> {
    let image = decode(&photo, orientation)?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, ImageFormat::Png)
        .map_err(|e| VisionError::Photo {
            detail: e.to_string(),
        })?;
    Ok(encoded.into_inner())
}

#[cfg(test)]
mod tests {
    use bitcoin_vision::image;
    use super::*;

    fn marker() -> RgbImage {
        RgbImage::from_fn(3, 2, |x, y| image::Rgb([(1 + y * 3 + x) as u8, 0, 0]))
    }

    #[test]
    fn photo_up_bridge_preserves_identity_provenance_without_changing_pixels() {
        use bitcoin_vision::detect::Quad;
        use bitcoin_vision::page_frame::{PageAxis, PhotoUp, PolarityReason};
        let mut bytes = std::io::Cursor::new(Vec::new());
        marker().write_to(&mut bytes, ImageFormat::Png).unwrap();
        for (input, expected) in [
            (PhotoUpSource::Unknown, PhotoUp::Unknown),
            (PhotoUpSource::Camera, PhotoUp::Camera),
            (PhotoUpSource::Exif, PhotoUp::Exif),
            (PhotoUpSource::Explicit, PhotoUp::Explicit),
        ] {
            let hint = PhotoUp::from(input);
            assert_eq!(hint, expected);
            assert_eq!(decode(bytes.get_ref(), 1).unwrap(), marker());
            let d = PageAxis::from_detections(&[Quad([(0., 0.), (3., 0.), (3., 2.), (0., 2.)])])
                .resolve(&marker(), hint, None, 0.15, false)
                .unwrap();
            assert_eq!(
                d.reason,
                if input == PhotoUpSource::Unknown {
                    PolarityReason::NoRecogniser
                } else {
                    PolarityReason::PhotoUp
                }
            );
            assert_eq!(d.orientation_reads, 0);
        }
        assert_eq!(PhotoUpSource::default(), PhotoUpSource::Unknown);
    }

    #[test]
    fn all_eight_transforms_use_the_same_marker_coordinates() {
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
        for orientation in 1..=8 {
            let result = orient(marker(), orientation);
            assert_eq!(
                result.dimensions(),
                if orientation < 5 { (3, 2) } else { (2, 3) }
            );
            assert_eq!(
                result.pixels().map(|p| p[0]).collect::<Vec<_>>(),
                expected[orientation as usize - 1]
            );
        }
        let original = marker();
        let ptr = original.as_ptr();
        assert_eq!(
            orient(original, 1).as_ptr(),
            ptr,
            "identity must not allocate another raster"
        );
    }

    #[test]
    fn exported_pixels_reopen_without_a_second_transform() {
        let mut input = std::io::Cursor::new(Vec::new());
        marker().write_to(&mut input, ImageFormat::Png).unwrap();
        for orientation in 1..=8 {
            let output = canonical_photo(input.get_ref().clone(), orientation).unwrap();
            assert_eq!(decode(&output, 1).unwrap(), orient(marker(), orientation));
        }
        assert!(decode(input.get_ref(), 0).is_err());
        assert!(decode(input.get_ref(), 9).is_err());
        assert!(decode(b"not a photo", 1).is_err());
    }

    #[test]
    fn full_resolution_orientation_cost() {
        let image = RgbImage::from_fn(3456, 4608, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 251) as u8, 127])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image.write_to(&mut encoded, ImageFormat::Jpeg).unwrap();
        drop(image);
        // Microbenchmark only: no detector, model, app boundary or phone claim.
        for orientation in [1, 6, 1, 6] {
            let start = std::time::Instant::now();
            let raw = image::load_from_memory(encoded.get_ref())
                .unwrap()
                .into_rgb8();
            let decoded = start.elapsed();
            let start = std::time::Instant::now();
            let result = orient(raw, orientation);
            println!(
                "orientation={orientation} decode_ms={:.3} transform_ms={:.3} rgb_bytes={} dimensions={:?}",
                decoded.as_secs_f64() * 1000.0,
                start.elapsed().as_secs_f64() * 1000.0,
                result.as_raw().len(),
                result.dimensions()
            );
            std::hint::black_box(result);
        }
    }
}
