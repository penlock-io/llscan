//! One extra-work allowance shared by required recovery, fragment unions
//! and optional E3 expansion, independent of the expansion feature flag.

use crate::detect::Quad;
use crate::page_frame::WritingFrame;
#[cfg(any(test, feature = "word-extent"))]
use crate::phrase::CROP_MARGIN;
use crate::split::Frame;

#[cfg(feature = "word-extent")]
pub(crate) const MAX_ROIS: usize = 32;
#[cfg(feature = "word-extent")]
pub(crate) const MAX_ROI_PIXELS: u64 = 1_048_576;
#[cfg(feature = "word-extent")]
pub(crate) const TOTAL_ROI_PIXELS: u64 = 8_388_608;
pub(crate) const MAX_READS: usize = 4;
pub(crate) const MAX_READ_PIXELS: u64 = 1_048_576;

/// Preflight work budgets. Calls are charged at prepare's actual call sites.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Budget {
    pub(crate) writing: WritingFrame,
    pub(crate) rois: usize,
    pub(crate) roi_pixels: u64,
    pub(crate) reads: usize,
    pub(crate) read_pixels: u64,
    pub(crate) classifiers: usize,
    pub(crate) ocrs: usize,
}

impl Budget {
    #[cfg(feature = "word-extent")]
    pub(crate) fn inspect(&mut self) -> Result<(), &'static str> {
        if self.rois == MAX_ROIS {
            return Err("roi_count_budget");
        }
        self.rois += 1;
        Ok(())
    }

    #[cfg(feature = "word-extent")]
    pub(crate) fn roi(&mut self, pixels: u64) -> Result<(), &'static str> {
        if pixels > MAX_ROI_PIXELS || pixels > TOTAL_ROI_PIXELS - self.roi_pixels {
            return Err("roi_pixel_budget");
        }
        self.roi_pixels += pixels;
        Ok(())
    }

    #[cfg(any(test, feature = "word-extent"))]
    pub(crate) fn read(&mut self, quad: &Quad) -> Result<(), &'static str> {
        self.read_margin(quad, CROP_MARGIN, true)
    }

    pub(crate) fn read_margin(
        &mut self,
        quad: &Quad,
        margin: f32,
        ocr: bool,
    ) -> Result<(), &'static str> {
        let f = self.writing.crop_frame(quad, margin);
        let pixels = pixels_ceil(f)?;
        let classifiers =
            if !self.writing.shared() && self.writing.frame_of(quad).angle.abs() > 45.0 {
                2
            } else {
                1
            };
        if self.classifiers + classifiers > 2 * MAX_READS || (ocr && self.ocrs == MAX_READS) {
            return Err("read_budget");
        }
        self.crop(pixels)
    }

    pub(crate) fn crop(&mut self, pixels: u64) -> Result<(), &'static str> {
        if self.reads == MAX_READS || pixels > MAX_READ_PIXELS - self.read_pixels {
            return Err("read_budget");
        }
        self.reads += 1;
        self.read_pixels += pixels;
        Ok(())
    }

    pub(crate) fn classifier(&mut self) -> Result<(), String> {
        if self.classifiers == 2 * MAX_READS {
            return Err("extra-read classifier call budget exceeded before call".into());
        }
        self.classifiers += 1;
        Ok(())
    }

    pub(crate) fn ocr(&mut self) -> Result<(), String> {
        if self.ocrs == MAX_READS {
            return Err("extra-read OCR call budget exceeded before call".into());
        }
        self.ocrs += 1;
        Ok(())
    }

    #[cfg(feature = "word-extent")]
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({"inspected_rois": self.rois, "roi_pixels": self.roi_pixels,
            "read_attempts": self.reads, "read_pixels": self.read_pixels,
            "classifier_calls": self.classifiers, "ocr_calls": self.ocrs})
    }
}

pub(crate) fn pixels_ceil(f: Frame) -> Result<u64, &'static str> {
    if ![f.cx, f.cy, f.w, f.h, f.angle]
        .iter()
        .all(|n| n.is_finite())
        || f.w <= 0.
        || f.h <= 0.
    {
        return Err("invalid_frame");
    }
    (f.w.ceil() as u64)
        .checked_mul(f.h.ceil() as u64)
        .ok_or("invalid_frame")
}
