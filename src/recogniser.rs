//! The PP-OCRv4 text recogniser reading a crop as one line: the survey's
//! entrant, on the phone. It says what characters are written, list
//! word or not, which the word model cannot: the word model answers
//! every crop with a list word. Its raw read is the second opinion the
//! non-word filter and the hybrid pick are built on.
//!
//! The reading is the entrant's as `ocr_bench.py` runs it: the crop
//! resized to 48 pixels tall at its own aspect (at least 16 wide, and
//! squashed to 320 beyond that, RapidOCR's own ceiling, which no word
//! crop reaches), normalised to [-1, 1], one pass through the network,
//! and the CTC frames decoded greedily, a frame's class emitted when it
//! is not the blank and not the previous frame's. The confidence is the
//! mean, over the emitting frames, of the emitted class's probability.

use image::RgbImage;

/// Height of the recogniser's line input.
pub const LINE_HEIGHT: u32 = 48;
/// The narrowest line the entrant feeds the network.
pub const MIN_WIDTH: u32 = 16;
/// The widest: a crop of greater aspect is squashed to it, so no crop
/// can demand an arbitrarily wide plan.
pub const MAX_WIDTH: u32 = 320;
/// Maximum exact-width plans in the benchmark-only concrete reference.
#[cfg(feature = "scan-profile")]
pub const PLANS_KEPT: usize = 8;

/// Fixed execution preparation for one recogniser and its instance-owned state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preparation {
    /// Benchmark reference: concrete typing, optimization and an eight-plan LRU.
    #[cfg(feature = "scan-profile")]
    Optimized,
    /// Experimental concrete typing without whole-graph optimization. Only
    /// diagnostic builds can select it until the phone/parity gates pass.
    #[cfg(feature = "scan-profile")]
    TypedOnly,
    /// One guarded, optimized symbolic runnable, with no concrete cache.
    #[default]
    Symbolic,
}

impl Preparation {
    /// Stable diagnostic label, also recorded at the actual cache lookups.
    pub fn label(self) -> &'static str {
        match self {
            #[cfg(feature = "scan-profile")]
            Self::Optimized => "optimized",
            #[cfg(feature = "scan-profile")]
            Self::TypedOnly => "typed",
            Self::Symbolic => "symbolic",
        }
    }
}

/// One line read.
#[derive(Clone, Debug, PartialEq)]
pub struct LineRead {
    /// The characters read, as the dictionary spells them.
    pub text: String,
    /// Mean probability of the emitted characters; 0 when nothing was
    /// emitted.
    pub confidence: f32,
}

/// The dictionary as PP-OCR's decoder indexes it: the blank at 0, the
/// dictionary's lines from 1, and a space after them.
#[cfg_attr(not(feature = "onnx"), allow(dead_code))]
fn classes(dictionary: &str) -> Vec<String> {
    let mut chars = vec![String::new()];
    chars.extend(dictionary.lines().map(str::to_owned));
    chars.push(" ".into());
    chars
}

/// Greedy CTC decoding of `frames`, each a probability vector over the
/// classes.
#[cfg_attr(not(feature = "onnx"), allow(dead_code))]
fn decode(frames: &[Vec<f32>], chars: &[String]) -> LineRead {
    decode_observed(frames, chars, |_, _, _, _| {})
}

// The observer follows the existing argmax/collapse decision; it cannot change
// decoding or trigger inference. Normal reads retain no positional diagnostics.
#[cfg_attr(not(feature = "onnx"), allow(dead_code))]
fn decode_observed(
    frames: &[Vec<f32>],
    chars: &[String],
    mut observe: impl FnMut(usize, usize, f32, Option<[usize; 2]>),
) -> LineRead {
    let mut text = String::new();
    let mut confidences = Vec::new();
    let mut last = 0usize;
    for (t, frame) in frames.iter().enumerate() {
        let (best, p) =
            frame.iter().enumerate().fold(
                (0usize, f32::MIN),
                |acc, (i, &v)| if v > acc.1 { (i, v) } else { acc },
            );
        let mut emitted_bytes = None;
        if best != 0 && best != last {
            let start = text.len();
            if let Some(c) = chars.get(best) {
                text.push_str(c);
            }
            confidences.push(p);
            emitted_bytes = Some([start, text.len()]);
        }
        observe(t, best, p, emitted_bytes);
        last = best;
    }
    let confidence = if confidences.is_empty() {
        0.0
    } else {
        confidences.iter().sum::<f32>() / confidences.len() as f32
    };
    LineRead { text, confidence }
}

/// Run the actual CTC decoder on diagnostic model outputs. This opt-in helper
/// lets the isolated preparation probe share decoding, without changing the
/// scanner's model preparation or publishing a second implementation.
#[cfg(feature = "scan-profile")]
pub fn decode_for_profile(frames: &[Vec<f32>], dictionary: &str) -> LineRead {
    decode(frames, &classes(dictionary))
}

/// Describe already-computed CTC evidence, never inferred glyph boundaries.
#[cfg(all(feature = "scan-profile", any(feature = "onnx", test)))]
fn position_diagnostics(
    frames: &[Vec<f32>],
    chars: &[String],
    crop_size: (u32, u32),
    input_width: u32,
) -> Result<serde_json::Value, String> {
    use serde_json::{Value, json};
    if chars.is_empty()
        || frames
            .iter()
            .any(|f| f.len() != chars.len() || f.iter().any(|p| !p.is_finite()))
    {
        return Err("invalid CTC diagnostic frame shape or non-finite value".into());
    }
    let mut runs: Vec<Value> = Vec::new();
    let read = decode_observed(frames, chars, |t, class, p, emitted_bytes| {
        if let Some(last) = runs.last_mut().filter(|r| r["class_index"] == class) {
            last["frame_end"] = json!(t + 1);
        } else {
            runs.push(json!({
                "frame_start":t, "frame_end":t+1, "class_index":class,
                "class_text":chars[class], "first_probability":p,
                "emitted_utf8_bytes":emitted_bytes,
            }));
        }
    });
    Ok(json!({
        "schema":1,
        "read":{"text":read.text, "confidence":read.confidence},
        "crop_size":[crop_size.0,crop_size.1],
        "input_shape":[1,3,LINE_HEIGHT,input_width],
        "output_shape":[1,frames.len(),chars.len()],
        "classes":chars, "frames":frames, "greedy_runs":runs,
        "coordinate_convention":"zero-based half-open frame intervals and UTF-8 byte ranges; CTC time is not a calibrated pixel or glyph boundary",
    }))
}

/// Pillow's bilinear resampling, which the entrant's reference was made
/// with: a triangle filter whose support grows with the shrink, weights
/// rounded to 22-bit fixed point, a horizontal pass to 8 bits and then a
/// vertical one. The image crate's own triangle filter rounds otherwise
/// and moves a letter on one crop in seventy.
pub fn pil_bilinear(src: &RgbImage, out_w: u32, out_h: u32) -> RgbImage {
    const PRECISION: i32 = 22;
    fn coefficients(in_size: u32, out_size: u32) -> Vec<(usize, Vec<i32>)> {
        let scale = in_size as f64 / out_size as f64;
        let filter_scale = scale.max(1.0);
        let support = filter_scale;
        let ss = 1.0 / filter_scale;
        (0..out_size)
            .map(|xx| {
                let center = (xx as f64 + 0.5) * scale;
                let xmin = ((center - support + 0.5) as i64).max(0) as usize;
                let xmax =
                    (((center + support + 0.5) as i64).min(in_size as i64) as usize).max(xmin);
                let mut k: Vec<f64> = (xmin..xmax)
                    .map(|x| {
                        let t = ((x as f64 - center + 0.5) * ss).abs();
                        if t < 1.0 { 1.0 - t } else { 0.0 }
                    })
                    .collect();
                let total: f64 = k.iter().sum();
                if total != 0.0 {
                    for v in &mut k {
                        *v /= total;
                    }
                }
                let fixed = k
                    .iter()
                    .map(|&v| {
                        let scaled = v * (1u32 << PRECISION) as f64;
                        (if v < 0.0 { scaled - 0.5 } else { scaled + 0.5 }) as i32
                    })
                    .collect();
                (xmin, fixed)
            })
            .collect()
    }
    fn clip8(v: i64) -> u8 {
        (v >> PRECISION).clamp(0, 255) as u8
    }
    let (w, h) = src.dimensions();
    let horizontal = coefficients(w, out_w);
    let mut mid = RgbImage::new(out_w, h);
    for y in 0..h {
        for (xx, (xmin, k)) in horizontal.iter().enumerate() {
            let mut acc = [1i64 << (PRECISION - 1); 3];
            for (i, &weight) in k.iter().enumerate() {
                let px = src.get_pixel(*xmin as u32 + i as u32, y);
                for c in 0..3 {
                    acc[c] += px[c] as i64 * weight as i64;
                }
            }
            mid.put_pixel(
                xx as u32,
                y,
                image::Rgb([clip8(acc[0]), clip8(acc[1]), clip8(acc[2])]),
            );
        }
    }
    let vertical = coefficients(h, out_h);
    let mut out = RgbImage::new(out_w, out_h);
    for (yy, (ymin, k)) in vertical.iter().enumerate() {
        for x in 0..out_w {
            let mut acc = [1i64 << (PRECISION - 1); 3];
            for (i, &weight) in k.iter().enumerate() {
                let px = mid.get_pixel(x, *ymin as u32 + i as u32);
                for c in 0..3 {
                    acc[c] += px[c] as i64 * weight as i64;
                }
            }
            out.put_pixel(
                x,
                yy as u32,
                image::Rgb([clip8(acc[0]), clip8(acc[1]), clip8(acc[2])]),
            );
        }
    }
    out
}

/// The line input the entrant makes of a crop: 48 tall, the aspect kept,
/// each channel `v / 127.5 - 1`, laid out `1×3×48×W`.
#[cfg_attr(not(feature = "onnx"), allow(dead_code))]
fn line_input(crop: &RgbImage) -> (Vec<f32>, u32) {
    let (w, h) = crop.dimensions();
    let width =
        ((w as f32 * LINE_HEIGHT as f32 / h as f32).round() as u32).clamp(MIN_WIDTH, MAX_WIDTH);
    let resized = pil_bilinear(crop, width, LINE_HEIGHT);
    let plane = (LINE_HEIGHT * width) as usize;
    let mut input = vec![0f32; 3 * plane];
    for (x, y, px) in resized.enumerate_pixels() {
        let at = (y * width + x) as usize;
        for c in 0..3 {
            input[c * plane + at] = px[c] as f32 / 127.5 - 1.0;
        }
    }
    (input, width)
}

#[cfg(feature = "onnx")]
pub use with_onnx::Recogniser;

#[cfg(feature = "onnx")]
#[path = "recogniser_shape.rs"]
mod shape;

/// The same production builder, exposed only for the isolated comparison probe.
#[doc(hidden)]
#[cfg(all(feature = "onnx", feature = "scan-profile"))]
pub mod shape_for_profile {
    pub use super::shape::*;
}

#[cfg(feature = "onnx")]
mod with_onnx {
    use std::path::Path;
    use std::sync::Arc;
    #[cfg(feature = "scan-profile")]
    use std::sync::Mutex;

    use image::{GrayImage, RgbImage};
    use tract_onnx::prelude::*;

    #[cfg(feature = "scan-profile")]
    use super::PLANS_KEPT;
    use super::{LINE_HEIGHT, LineRead, Preparation, classes, decode, line_input};

    #[cfg(feature = "scan-profile")]
    struct ConcretePlans {
        model: InferenceModel,
        plans: Mutex<Vec<(u32, Arc<TypedRunnableModel>)>>,
    }

    // Mutually exclusive lifetimes: the symbolic variant cannot retain an
    // import graph, concrete plans, fallback strategy or width cache.
    enum PlanState {
        #[cfg(feature = "scan-profile")]
        Concrete(Box<ConcretePlans>),
        Symbolic(Arc<TypedRunnableModel>),
    }

    /// The production recogniser owns one exact-input symbolic runnable.
    /// Concrete preparation and its LRU exist only in profiling builds.
    pub struct Recogniser {
        state: PlanState,
        chars: Vec<String>,
        spec: String,
        // Immutable with the model/runtime for this instance: widths suffice
        // as local cache keys. There is no cross-instance or cross-arm cache.
        preparation: Preparation,
    }

    /// Diagnostic lower bound, NOT total plan allocation: top-level constant
    /// tensor buffers, deduplicated by Arc identity across the resident cache.
    /// Packed/exotic storage, nested graphs, operator state, allocation overhead
    /// and heap payloads inside plain Blob/String/TDim tensors are excluded.
    #[cfg(feature = "scan-profile")]
    fn profile_resident_plans<'a>(
        plans: impl Iterator<Item = &'a Arc<TypedRunnableModel>>,
        widths: Option<Vec<u32>>,
        preparation: Preparation,
    ) {
        let _inventory = crate::timing::span("ocr.resident_inventory");
        let mut seen = std::collections::HashSet::new();
        let mut plain_bytes = 0;
        let mut exotic_tensors = 0;
        let mut per_plan_plain_bytes = Vec::new();
        for plan in plans {
            let mut local_seen = std::collections::HashSet::new();
            let mut local_bytes = 0;
            for tensor in plan
                .model()
                .nodes()
                .iter()
                .flat_map(|node| &node.outputs)
                .filter_map(|out| out.fact.konst.as_ref())
            {
                if local_seen.insert(Arc::as_ptr(tensor))
                    && let Some(plain) = tensor.as_plain()
                {
                    local_bytes += plain.as_bytes().len();
                }
                if seen.insert(Arc::as_ptr(tensor)) {
                    if let Some(plain) = tensor.as_plain() {
                        plain_bytes += plain.as_bytes().len();
                    } else {
                        exotic_tensors += 1;
                    }
                }
            }
            per_plan_plain_bytes.push(local_bytes);
        }
        crate::timing::ocr_resident(serde_json::json!({
            "preparation": preparation.label(),
            "widths_lru_to_mru": widths,
            "state_kind": if preparation == Preparation::Symbolic { "symbolic" } else { "exact_width_lru" },
            "import_graph_retained": preparation != Preparation::Symbolic,
            "reusable_width_range": if preparation == Preparation::Symbolic { Some([super::MIN_WIDTH, super::MAX_WIDTH]) } else { None },
            "plan_count": per_plan_plain_bytes.len(),
            "top_level_plain_const_buffer_bytes": plain_bytes,
            "per_plan_plain_const_buffer_bytes": per_plan_plain_bytes,
            "top_level_exotic_const_tensors_excluded": exotic_tensors,
        }));
    }

    impl Recogniser {
        /// Loads the model bytes with the dictionary text (one character
        /// per line, as the ONNX metadata carries it).
        pub fn from_bytes(model: &[u8], dictionary: &str) -> Result<Recogniser, String> {
            Self::from_bytes_with_preparation(model, dictionary, Preparation::default())
        }

        /// Loads one model with a fixed preparation strategy. Experimental
        /// strategies are only available under `scan-profile`.
        pub fn from_bytes_with_preparation(
            model: &[u8],
            dictionary: &str,
            preparation: Preparation,
        ) -> Result<Recogniser, String> {
            match preparation {
                Preparation::Symbolic => {
                    Self::build_symbolic(model, dictionary, "recogniser:<memory>".into())
                }
                #[cfg(feature = "scan-profile")]
                other => Recogniser::build(
                    tract_onnx::onnx()
                        .model_for_read(&mut std::io::Cursor::new(model))
                        .map_err(|e| format!("recogniser:<memory>: {e}"))?,
                    dictionary,
                    "recogniser:<memory>".into(),
                    other,
                ),
            }
        }

        /// Loads the model file and the dictionary file.
        pub fn load(model: &Path, dictionary: &Path) -> Result<Recogniser, String> {
            Self::load_with_preparation(model, dictionary, Preparation::default())
        }

        /// Loads instance-owned preparation with a fixed strategy.
        pub fn load_with_preparation(
            model: &Path,
            dictionary: &Path,
            preparation: Preparation,
        ) -> Result<Recogniser, String> {
            let text = std::fs::read_to_string(dictionary)
                .map_err(|e| format!("{}: {e}", dictionary.display()))?;
            let spec = model.display().to_string();
            match preparation {
                Preparation::Symbolic => {
                    let bytes = std::fs::read(model).map_err(|e| format!("{spec}: {e}"))?;
                    Self::build_symbolic(&bytes, &text, spec)
                }
                #[cfg(feature = "scan-profile")]
                other => Recogniser::build(
                    tract_onnx::onnx()
                        .model_for_path(model)
                        .map_err(|e| format!("{spec}: {e}"))?,
                    &text,
                    spec,
                    other,
                ),
            }
        }

        #[cfg(feature = "scan-profile")]
        fn build(
            model: InferenceModel,
            dictionary: &str,
            spec: String,
            preparation: Preparation,
        ) -> Result<Recogniser, String> {
            let reader = Recogniser {
                state: PlanState::Concrete(Box::new(ConcretePlans {
                    model,
                    plans: Mutex::new(Vec::with_capacity(PLANS_KEPT)),
                })),
                chars: classes(dictionary),
                spec,
                preparation,
            };
            reader.check_contract()?;
            Ok(reader)
        }

        fn build_symbolic(model: &[u8], dictionary: &str, spec: String) -> Result<Self, String> {
            use super::shape;
            use bitcoin_hashes::{Hash, sha256};
            if sha256::Hash::hash(dictionary.as_bytes()).to_string() != shape::DICTIONARY_SHA256 {
                return Err(format!("{spec}: unsupported candidate dictionary hash"));
            }
            let plan = shape::build(model, |_| {})
                .map_err(|e| format!("{spec}: unsupported symbolic candidate: {e}"))?;
            let reader = Self {
                state: PlanState::Symbolic(plan),
                chars: classes(dictionary),
                spec,
                preparation: Preparation::Symbolic,
            };
            // Keep exactly the original constructor's real preprocessing/run
            // contract check. Its width is 192, not a separately prepared plan.
            reader.check_contract()?;
            Ok(reader)
        }

        fn check_contract(&self) -> Result<(), String> {
            // Includes the concrete arm's first plan build. The symbolic
            // arm has prepared its sole plan before entering this read.
            let _check = crate::timing::span("ocr.contract_check");
            self.frames(&RgbImage::from_pixel(64, 16, image::Rgb([255, 255, 255])))?;
            Ok(())
        }

        /// Plans held at the moment, for the bound's test.
        pub fn plans_held(&self) -> usize {
            match &self.state {
                #[cfg(feature = "scan-profile")]
                PlanState::Concrete(state) => state.plans.lock().map(|p| p.len()).unwrap_or(0),
                PlanState::Symbolic(_) => 1,
            }
        }

        /// This instance's immutable execution strategy.
        pub fn preparation(&self) -> Preparation {
            self.preparation
        }

        /// Bounded diagnostic input through the real preprocessing/cache/run
        /// path. Never exported to the app and never a page/accuracy benchmark.
        #[cfg(feature = "scan-profile")]
        pub fn profile_blank_width(&self, width: u32) -> Result<(), String> {
            if !(super::MIN_WIDTH..=super::MAX_WIDTH).contains(&width) {
                return Err("diagnostic width outside the exact-input domain".into());
            }
            self.read_rgb(&RgbImage::from_pixel(
                width,
                LINE_HEIGHT,
                image::Rgb([255; 3]),
            ))?;
            Ok(())
        }

        fn plan(&self, width: u32) -> Result<Arc<TypedRunnableModel>, String> {
            match &self.state {
                #[cfg(feature = "scan-profile")]
                PlanState::Concrete(state) => self.concrete_plan(state, width),
                PlanState::Symbolic(plan) => {
                    if !(super::MIN_WIDTH..=super::MAX_WIDTH).contains(&width) {
                        return Err("unsupported symbolic candidate width".into());
                    }
                    // Every lookup reuses the one constructor-prepared plan,
                    // including the blank constructor check at width 192.
                    crate::timing::ocr_width(width, true);
                    #[cfg(feature = "scan-profile")]
                    profile_resident_plans(std::iter::once(plan), None, self.preparation);
                    Ok(plan.clone())
                }
            }
        }

        #[cfg(feature = "scan-profile")]
        fn concrete_plan(
            &self,
            state: &ConcretePlans,
            width: u32,
        ) -> Result<Arc<TypedRunnableModel>, String> {
            let mut plans = state.plans.lock().map_err(|e| e.to_string())?;
            if let Some(i) = plans.iter().position(|(w, _)| *w == width) {
                crate::timing::ocr_width(width, true);
                let hit = plans.remove(i);
                let plan = hit.1.clone();
                plans.push(hit);
                #[cfg(feature = "scan-profile")]
                profile_resident_plans(
                    plans.iter().map(|(_, p)| p),
                    Some(plans.iter().map(|(w, _)| *w).collect()),
                    self.preparation,
                );
                return Ok(plan);
            }
            crate::timing::ocr_width(width, false);
            let _build = crate::timing::span("ocr.plan_build");
            let clone_span = crate::timing::span("ocr.plan_clone");
            let model = state.model.clone();
            drop(clone_span);
            let fact_span = crate::timing::span("ocr.plan_fact");
            let model = model
                .with_input_fact(
                    0,
                    f32::fact([1, 3, LINE_HEIGHT as usize, width as usize]).into(),
                )
                .map_err(|e| format!("{}: input is not 1×3×{LINE_HEIGHT}×W: {e}", self.spec))?;
            drop(fact_span);
            // InferenceModel::into_optimized is exactly into_typed followed by
            // TypedModel::into_optimized in the pinned tract 0.23.5 runtime.
            let type_span = crate::timing::span("ocr.plan_type");
            let model = model
                .into_typed()
                .map_err(|e| format!("{}: {e}", self.spec))?;
            drop(type_span);
            let model = if self.preparation == Preparation::Optimized {
                let optimize_span = crate::timing::span("ocr.plan_optimize");
                let model = model
                    .into_optimized()
                    .map_err(|e| format!("{}: {e}", self.spec))?;
                drop(optimize_span);
                model
            } else {
                // Keep all ad-hoc operator work inside the existing run path.
                model
            };
            let runnable_span = crate::timing::span("ocr.plan_runnable");
            let plan = model
                .into_runnable()
                .map_err(|e| format!("{}: {e}", self.spec))?;
            drop(runnable_span);
            if plans.len() == PLANS_KEPT {
                plans.remove(0);
            }
            plans.push((width, plan.clone()));
            drop(_build);
            #[cfg(feature = "scan-profile")]
            profile_resident_plans(
                plans.iter().map(|(_, p)| p),
                Some(plans.iter().map(|(w, _)| *w).collect()),
                self.preparation,
            );
            Ok(plan)
        }

        fn frames(&self, crop: &RgbImage) -> Result<Vec<Vec<f32>>, String> {
            self.frames_with_width(crop).map(|(frames, _)| frames)
        }

        fn frames_with_width(&self, crop: &RgbImage) -> Result<(Vec<Vec<f32>>, u32), String> {
            let _input = crate::timing::span("ocr.input");
            let (input, width) = line_input(crop);
            let tensor = Tensor::from_shape(&[1, 3, LINE_HEIGHT as usize, width as usize], &input)
                .map_err(|e| e.to_string())?;
            drop(_input);
            let plan = self.plan(width)?;
            let _infer = crate::timing::span("ocr.infer");
            crate::inference_limit::before(crate::inference_limit::Kind::Ocr)?;
            let result = plan
                .run(tvec!(tensor.into()))
                .map_err(|e| format!("{}: {e}", self.spec))?;
            drop(_infer);
            let [only] = result.as_slice() else {
                return Err(format!(
                    "{}: the model produced {} output tensors; the contract is one",
                    self.spec,
                    result.len()
                ));
            };
            let shape = only.shape().to_vec();
            let [1, frames, classes] = shape[..] else {
                return Err(format!(
                    "{}: output shape {shape:?}; the contract is 1×T×classes",
                    self.spec
                ));
            };
            if classes != self.chars.len() {
                return Err(format!(
                    "{}: output has {classes} classes per frame; the dictionary implies {}",
                    self.spec,
                    self.chars.len()
                ));
            }
            let view = only
                .to_plain_array_view::<f32>()
                .map_err(|e| format!("{}: {e}", self.spec))?;
            let frames = (0..frames)
                .map(|t| (0..classes).map(|c| view[[0, t, c]]).collect())
                .collect();
            Ok((frames, width))
        }

        /// Reads a colour crop.
        pub fn read_rgb(&self, crop: &RgbImage) -> Result<LineRead, String> {
            let _read = crate::timing::span("ocr.read");
            Ok(decode(&self.frames(crop)?, &self.chars))
        }

        /// Reads a grey crop, the level crops the stage cuts.
        pub fn read(&self, crop: &GrayImage) -> Result<LineRead, String> {
            self.read_rgb(&image::DynamicImage::ImageLuma8(crop.clone()).into_rgb8())
        }

        /// Reads one unchanged crop once, retaining the existing CTC output and
        /// greedy emission intervals for diagnostics. It does not locate glyph
        /// edges, run a second decoder/inference, or change normal read results.
        /// Put the inference-limit scope around construction as well as calls.
        #[cfg(feature = "scan-profile")]
        pub fn read_positions_for_profile(
            &self,
            crop: &GrayImage,
        ) -> Result<serde_json::Value, String> {
            if crop.width() == 0 || crop.height() == 0 {
                return Err("empty CTC diagnostic crop".into());
            }
            let _read = crate::timing::span("ocr.read");
            let rgb = image::DynamicImage::ImageLuma8(crop.clone()).into_rgb8();
            let (frames, width) = self.frames_with_width(&rgb)?;
            super::position_diagnostics(&frames, &self.chars, crop.dimensions(), width)
        }
    }
}

/// The socket's stand-in without the `onnx` feature: loading reports
/// the missing feature instead of failing to compile downstream.
#[cfg(not(feature = "onnx"))]
pub struct Recogniser {}

#[cfg(not(feature = "onnx"))]
impl Recogniser {
    /// Always fails: the `onnx` feature is off.
    pub fn from_bytes(_model: &[u8], _dictionary: &str) -> Result<Recogniser, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn load(
        _model: &std::path::Path,
        _dictionary: &std::path::Path,
    ) -> Result<Recogniser, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn read_rgb(&self, _crop: &RgbImage) -> Result<LineRead, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// Always fails: the `onnx` feature is off.
    pub fn read(&self, _crop: &image::GrayImage) -> Result<LineRead, String> {
        Err("penlock-scan was built without the onnx feature".into())
    }

    /// The arm a stand-in would have run, had it been able to run one. Callers
    /// that only ask in order to choose a schedule need an answer either way.
    pub fn preparation(&self) -> Preparation {
        Preparation::Symbolic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_is_indexed_as_the_decoder_indexes_it() {
        let chars = classes("a\nb\n \n");
        assert_eq!(chars, ["", "a", "b", " ", " "]);
    }

    #[test]
    fn greedy_decoding_drops_blanks_and_repeats_and_averages_confidence() {
        let chars = classes("a\nb\n");
        // frames: a a blank b b, with the emitting frames at 0.8 and 0.6
        let frames = vec![
            vec![0.1, 0.8, 0.1, 0.0],
            vec![0.2, 0.7, 0.1, 0.0],
            vec![0.9, 0.05, 0.05, 0.0],
            vec![0.1, 0.1, 0.6, 0.2],
            vec![0.1, 0.1, 0.7, 0.1],
        ];
        let read = decode(&frames, &chars);
        assert_eq!(read.text, "ab");
        assert!((read.confidence - 0.7).abs() < 1e-6);
        assert_eq!(
            decode(&[vec![1.0, 0.0, 0.0, 0.0]], &chars),
            LineRead {
                text: String::new(),
                confidence: 0.0
            }
        );
    }

    #[test]
    fn the_line_input_keeps_the_aspect_and_normalises() {
        let crop = RgbImage::from_pixel(96, 24, image::Rgb([255, 0, 127]));
        let (input, width) = line_input(&crop);
        assert_eq!(width, 192);
        let plane = (LINE_HEIGHT * width) as usize;
        assert_eq!(input.len(), 3 * plane);
        assert!((input[0] - 1.0).abs() < 1e-6);
        assert!((input[plane] + 1.0).abs() < 1e-6);
        assert!(input[2 * plane].abs() < 0.01);
        assert_eq!(line_input(&RgbImage::new(4, 40)).1, MIN_WIDTH);
        assert_eq!(line_input(&RgbImage::new(4000, 40)).1, MAX_WIDTH);
    }
}

#[cfg(all(test, feature = "scan-profile"))]
mod position_tests {
    use super::*;
    use serde_json::json;

    fn frame(class: usize, p: f32, count: usize) -> Vec<f32> {
        let mut f = vec![0.; count];
        f[class] = p;
        f
    }

    #[test]
    fn repeated_digits_keep_runs_and_blank_separation() {
        let chars = classes("1\n.\n");
        let frames: Vec<_> = [
            (0, 1.),
            (1, 0.7),
            (1, 0.99),
            (0, 1.),
            (1, 0.6),
            (2, 0.8),
            (2, 0.9),
            (0, 1.),
        ]
        .into_iter()
        .map(|(c, p)| frame(c, p, chars.len()))
        .collect();
        let scope = crate::inference_limit::Scope::new(0, 0).unwrap();
        let report = position_diagnostics(&frames, &chars, (200, 50), 192).unwrap();
        assert_eq!(report["read"]["text"], "11.");
        assert_eq!(
            report["read"]["confidence"],
            json!((0.7f32 + 0.6 + 0.8) / 3.)
        );
        assert_eq!(report["frames"], json!(frames));
        assert_eq!(report["classes"], json!(chars));
        let intervals: Vec<_> = report["greedy_runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                json!([
                    r["class_text"],
                    r["frame_start"],
                    r["frame_end"],
                    r["emitted_utf8_bytes"]
                ])
            })
            .collect();
        assert_eq!(
            intervals,
            vec![
                json!(["", 0, 1, null]),
                json!(["1", 1, 3, [0, 1]]),
                json!(["", 3, 4, null]),
                json!(["1", 4, 5, [1, 2]]),
                json!([".", 5, 7, [2, 3]]),
                json!(["", 7, 8, null]),
            ]
        );
        assert_eq!(report["greedy_runs"][1]["first_probability"], json!(0.7f32));
        assert_eq!(scope.report()["ocr_calls"], 0);
        assert_eq!(scope.report()["classifier_calls"], 0);
        scope.check().unwrap();
    }

    #[test]
    fn emitted_ranges_are_utf8_bytes_not_glyph_counts() {
        let chars = classes("é\nab\n");
        let frames: Vec<_> = [1, 0, 1, 2, 3]
            .into_iter()
            .map(|c| frame(c, 1., chars.len()))
            .collect();
        let report = position_diagnostics(&frames, &chars, (100, 50), 96).unwrap();
        let text = report["read"]["text"].as_str().unwrap();
        assert_eq!(text, "ééab ");
        let mut ranges = Vec::new();
        for r in report["greedy_runs"].as_array().unwrap() {
            if let Some(range) = r["emitted_utf8_bytes"].as_array() {
                let (a, b) = (
                    range[0].as_u64().unwrap() as usize,
                    range[1].as_u64().unwrap() as usize,
                );
                assert_eq!(&text[a..b], r["class_text"].as_str().unwrap());
                ranges.push([a, b]);
            }
        }
        assert_eq!(ranges, [[0, 2], [2, 4], [4, 6], [6, 7]]);
    }

    #[test]
    fn blank_empty_and_tied_frames_keep_original_collapse() {
        let chars = classes("a\nb\n");
        let empty = position_diagnostics(&[], &chars, (100, 50), 96).unwrap();
        assert_eq!(empty["read"], json!({"text":"","confidence":0.}));
        assert_eq!(empty["greedy_runs"], json!([]));
        let tied = vec![
            vec![0.5, 0.5, 0., 0.],
            vec![0., 0.5, 0.5, 0.],
            vec![0., 0.5, 0.5, 0.],
        ];
        let report = position_diagnostics(&tied, &chars, (100, 50), 96).unwrap();
        assert_eq!(report["read"], json!({"text":"a","confidence":0.5}));
        assert_eq!(report["greedy_runs"][0]["class_index"], 0);
        assert_eq!(report["greedy_runs"][1]["frame_end"], 3);
    }

    #[test]
    fn invalid_frames_are_errors_not_null_probabilities() {
        let chars = classes("a\n");
        for frames in [
            vec![vec![1.]],
            vec![vec![0., f32::NAN, 0.]],
            vec![vec![0., f32::INFINITY, 0.]],
        ] {
            assert!(position_diagnostics(&frames, &chars, (100, 50), 96).is_err());
        }
        assert!(position_diagnostics(&[], &[], (100, 50), 96).is_err());
    }

    #[test]
    fn diagnostics_keep_actual_preprocessing_width_and_source_dimensions() {
        let chars = classes("a\n");
        for ((w, h), expected) in [
            ((4, 40), MIN_WIDTH),
            ((96, 24), 192),
            ((4000, 40), MAX_WIDTH),
        ] {
            let crop = RgbImage::new(w, h);
            let (_, width) = line_input(&crop);
            let report = position_diagnostics(&[], &chars, crop.dimensions(), width).unwrap();
            assert_eq!(report["crop_size"], json!([w, h]));
            assert_eq!(report["input_shape"], json!([1, 3, LINE_HEIGHT, expected]));
            assert_eq!(report["output_shape"], json!([1, 0, chars.len()]));
        }
    }

    #[test]
    fn observed_decode_matches_the_prechange_decoder_exactly() {
        // Frozen pre-change algorithm: compare both the normal and observed
        // paths to this, not merely to each other after sharing a helper.
        fn previous(frames: &[Vec<f32>], chars: &[String]) -> LineRead {
            let mut text = String::new();
            let mut probabilities = Vec::new();
            let mut last = 0;
            for f in frames {
                let (best, p) = f
                    .iter()
                    .enumerate()
                    .fold(
                        (0usize, f32::MIN),
                        |acc, (i, &p)| if p > acc.1 { (i, p) } else { acc },
                    );
                if best != 0 && best != last {
                    if let Some(c) = chars.get(best) {
                        text.push_str(c);
                    }
                    probabilities.push(p);
                }
                last = best;
            }
            LineRead {
                text,
                confidence: if probabilities.is_empty() {
                    0.
                } else {
                    probabilities.iter().sum::<f32>() / probabilities.len() as f32
                },
            }
        }
        let chars = classes("1\né\n");
        for length in 0..=5 {
            for mut pattern in 0..4usize.pow(length) {
                let frames: Vec<_> = (0..length)
                    .map(|t| {
                        let class = pattern % 4;
                        pattern /= 4;
                        frame(class, 0.6 + t as f32 * 0.05, chars.len())
                    })
                    .collect();
                let expected = previous(&frames, &chars);
                assert_eq!(decode(&frames, &chars), expected);
                let report = position_diagnostics(&frames, &chars, (100, 50), 96).unwrap();
                assert_eq!(
                    report["read"],
                    json!({"text":expected.text,"confidence":expected.confidence})
                );
            }
        }
    }
}
