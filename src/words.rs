//! Production classifier preprocessing and model ABI.
use image::{GrayImage, Luma};
use std::path::Path;
/// The word ABI's canvas height.
pub const WORD_HEIGHT: u32 = 48;
/// The word ABI's canvas width.
pub const WORD_WIDTH: u32 = 256;
/// Logits the word ABI promises: one per list word.
pub const WORD_CLASSES: usize = 2048;

/// Names the canonical rule. Bump it when the canvas a crop produces
/// changes, so canvas stores built under the old rule are rebuilt
/// rather than trained on, and a model calibrated under it is
/// refused rather than served.
pub const CANON_RULE: &str = "stretch-fill-v2";

/// The model's input for a canvas: ink `1 − grey/255`, row-major.
/// The only place a canvas becomes a tensor, so a stored canvas and
/// a live one reach the model as the same floats.
pub fn canvas_input(canvas: &GrayImage) -> Vec<f32> {
    debug_assert_eq!(canvas.dimensions(), (WORD_WIDTH, WORD_HEIGHT));
    canvas
        .pixels()
        .map(|p| 1.0 - f32::from(p.0[0]) / 255.0)
        .collect()
}

/// Paper left around the stretched ink box on every side, so a
/// stroke on the box's edge is not also the canvas's edge.
const MARGIN: u32 = 2;
/// Rows fuller than this fraction of the crop are ruled lines, not
/// letters, and never define the ink box.
const LINE_ROW: f64 = 0.85;
/// A row or column joins the ink box only with this much ink in it,
/// so a speck of dust does not stretch the word.
const MIN_INK: u32 = 2;

/// The canvas the model sees for a crop, as an image: paper white,
/// [`WORD_WIDTH`]×[`WORD_HEIGHT`]. The ink box — the rows and
/// columns holding pixels clearly darker than the paper, ruled lines
/// left out — is stretched to fill the canvas inside a fixed margin
/// in both directions, so on-paper size, margins, crop slack and the
/// word's extent all vanish. Spacing inside the box stays: spread
/// letters come out thinner, and letter count is carried by letter
/// proportions. A crop with no ink box fills the canvas as it is.
/// This is the only implementation of the rule — the trainer reads
/// stores of these bytes, and [`canvas_input`] is the one way they
/// become a tensor.
pub fn canonical_canvas(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    // Paper is the median pixel, not the brightest: on dark or
    // gridded paper the brightest pixels are highlights and a cut
    // measured from them counts the paper itself as ink.
    let mut histogram = [0u32; 256];
    for p in img.pixels() {
        histogram[p.0[0] as usize] += 1;
    }
    let lo = histogram.iter().position(|&n| n > 0).unwrap_or(0) as u8;
    let half = w * h / 2;
    let mut seen = 0u32;
    let paper = histogram
        .iter()
        .position(|&n| {
            seen += n;
            seen > half
        })
        .unwrap_or(255) as u8;
    let cut = f64::from(paper) - (f64::from(paper) - f64::from(lo)) * 0.35;
    let ink = |p: u8| paper > lo && f64::from(p) < cut;
    let mut rowink = vec![0u32; h as usize];
    for (_, y, p) in img.enumerate_pixels() {
        if ink(p.0[0]) {
            rowink[y as usize] += 1;
        }
    }
    let letter_row = |y: u32| {
        let n = rowink[y as usize];
        n >= MIN_INK && f64::from(n) < f64::from(w) * LINE_ROW
    };
    let mut colink = vec![0u32; w as usize];
    for (x, y, p) in img.enumerate_pixels() {
        if letter_row(y) && ink(p.0[0]) {
            colink[x as usize] += 1;
        }
    }
    let rows: Vec<u32> = (0..h).filter(|&y| letter_row(y)).collect();
    let cols: Vec<u32> = (0..w).filter(|&x| colink[x as usize] >= MIN_INK).collect();
    let (x0, x1, y0, y1) = match (cols.first(), cols.last(), rows.first(), rows.last()) {
        (Some(&x0), Some(&x1), Some(&y0), Some(&y1)) => (x0, x1, y0, y1),
        _ => (0, w - 1, 0, h - 1),
    };
    let ink_box = image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image();
    let stretched = image::imageops::resize(
        &ink_box,
        WORD_WIDTH - 2 * MARGIN,
        WORD_HEIGHT - 2 * MARGIN,
        image::imageops::FilterType::Triangle,
    );
    let mut canvas = GrayImage::from_pixel(WORD_WIDTH, WORD_HEIGHT, Luma([255]));
    for (gx, gy, p) in stretched.enumerate_pixels() {
        canvas.put_pixel(MARGIN + gx, MARGIN + gy, *p);
    }
    canvas
}

/// The model's input for a crop: [`canonical_canvas`] through
/// [`canvas_input`].
pub fn canonicalize(img: &GrayImage) -> Vec<f32> {
    canvas_input(&canonical_canvas(img))
}

/// What the decision rule says about a reading. Uncertain never
/// hides the ranked candidates — it is a flag beside them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// The top word is accepted.
    Accept,
    /// Ask the user: the top probability is too low, or the margin
    /// to the runner-up is too thin.
    Uncertain,
}

/// The confidence rule over a ranked reading: confident only when
/// the top probability meets `min_prob` and its lead over the
/// runner-up meets `min_margin`. The top word is reported either
/// way — the classifier always guesses.
pub fn verdict(ranked: &[(usize, f32)], min_prob: f32, min_margin: f32) -> Verdict {
    match ranked {
        [(_, p), (_, q), ..] if *p >= min_prob && p - q >= min_margin => Verdict::Accept,
        _ => Verdict::Uncertain,
    }
}

/// The word classifier, run in process with `tract`: one input
/// `1×1×48×256` float as [`canonicalize`] gives it, one output of
/// [`WORD_CLASSES`] logits — the 2,048 list words in order —
/// softmaxed here at temperature one, the
/// trainer's calibration already folded in. Any other shape is
/// refused at load. Built without the `onnx` feature the type still
/// exists so callers stay feature-agnostic, but loading fails.
#[cfg(feature = "onnx")]
#[derive(Clone)]
pub struct WordModel {
    plan: std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>,
}

/// The socket's stand-in without the `onnx` feature: loading reports
/// the missing feature instead of failing to compile downstream.
#[cfg(not(feature = "onnx"))]
#[derive(Clone)]
pub struct WordModel {}

#[cfg(not(feature = "onnx"))]
impl WordModel {
    /// Always fails: the `onnx` feature is off.
    pub fn load(_path: &Path) -> Result<WordModel, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn from_bytes(_bytes: &[u8]) -> Result<WordModel, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn read(&self, _img: &GrayImage) -> Result<Vec<(usize, f32)>, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn read_canvas(&self, _canvas: &GrayImage) -> Result<Vec<(usize, f32)>, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }
}

#[cfg(feature = "onnx")]
impl WordModel {
    /// Loads the model at `path` and checks it against the contract.
    pub fn load(path: &Path) -> Result<WordModel, String> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        WordModel::checked(model, &path.display().to_string())
    }

    /// Loads a model already in memory: on a phone it ships inside
    /// the app, not at a filesystem path.
    pub fn from_bytes(bytes: &[u8]) -> Result<WordModel, String> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(bytes))
            .map_err(|e| format!("word:<memory>: {e}"))?;
        WordModel::checked(model, "word:<memory>")
    }

    fn checked(
        model: tract_onnx::prelude::InferenceModel,
        spec: &str,
    ) -> Result<WordModel, String> {
        use tract_onnx::prelude::*;
        let (h, w) = (WORD_HEIGHT as usize, WORD_WIDTH as usize);
        let plan = model
            .with_input_fact(0, f32::fact([1, 1, h, w]).into())
            .map_err(|e| format!("{spec}: input is not 1×1×{h}×{w}: {e}"))?
            .into_optimized()
            .map_err(|e| format!("{spec}: {e}"))?
            .into_runnable()
            .map_err(|e| format!("{spec}: {e}"))?;
        let probe =
            Tensor::from_shape(&[1, 1, h, w], &vec![0.0f32; h * w]).map_err(|e| e.to_string())?;
        crate::inference_limit::before(crate::inference_limit::Kind::Classifier)?;
        let outputs = crate::runtime::run_plan(&plan, probe)?.len();
        if outputs != WORD_CLASSES {
            return Err(format!(
                "{spec}: {outputs} outputs; the contract is {WORD_CLASSES} word logits"
            ));
        }
        Ok(WordModel { plan })
    }

    /// Reads a word crop: every word with its probability, most
    /// probable first.
    pub fn read(&self, img: &GrayImage) -> Result<Vec<(usize, f32)>, String> {
        let _read = crate::timing::span("word.read");
        let _input = crate::timing::span("word.input");
        let canvas = canonical_canvas(img);
        drop(_input);
        self.read_canvas(&canvas)
    }

    /// Reads a canvas [`canonical_canvas`] already produced — the
    /// scorer keeps the canvas to emit it beside the reading.
    pub fn read_canvas(&self, canvas: &GrayImage) -> Result<Vec<(usize, f32)>, String> {
        use tract_onnx::prelude::*;
        let (h, w) = (WORD_HEIGHT as usize, WORD_WIDTH as usize);
        let input =
            Tensor::from_shape(&[1, 1, h, w], &canvas_input(canvas)).map_err(|e| e.to_string())?;
        let _infer = crate::timing::span("word.infer");
        crate::inference_limit::before(crate::inference_limit::Kind::Classifier)?;
        let logits = crate::runtime::run_plan(&self.plan, input)?;
        drop(_infer);
        if logits.len() != WORD_CLASSES || logits.iter().any(|v| !v.is_finite()) {
            return Err(format!(
                "the model returned {} outputs, not all finite, where {WORD_CLASSES} were promised",
                logits.len()
            ));
        }
        let peak = logits.iter().cloned().fold(f32::MIN, f32::max);
        let exps: Vec<f32> = logits.iter().map(|v| (v - peak).exp()).collect();
        let total: f32 = exps.iter().sum();
        let mut ranked: Vec<(usize, f32)> = exps
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e / total))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(ranked)
    }
}
