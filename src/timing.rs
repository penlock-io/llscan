//! Opt-in numeric timings; no images, text, or word predictions are recorded.
//! Spans are inclusive (nested spans must not be added together). Collection
//! is per calling thread, and a thread that did work for another hands its
//! samples back with [`drain`] and [`absorb`]; the scan reports one thread's
//! worth of samples however many read the page.
//!
//! A span measures elapsed time, so where the reads of a page ran several at a
//! time their spans overlap and their sum exceeds the wall clock that contained
//! them. `read_workers` in the report says how many threads that sum came from,
//! and a span taken on the thread that owns the pass is the one to read as
//! latency.
//!
//! Without `scan-profile`, span and width recording are inlined no-ops.

/// A timed scope, recorded on drop when profiling is enabled.
pub struct Span {
    #[cfg(feature = "scan-profile")]
    name: &'static str,
    #[cfg(feature = "scan-profile")]
    start: std::time::Instant,
}

/// Starts an inclusive scope.
#[inline]
pub fn span(_name: &'static str) -> Span {
    Span {
        #[cfg(feature = "scan-profile")]
        name: _name,
        #[cfg(feature = "scan-profile")]
        start: std::time::Instant::now(),
    }
}

#[cfg(feature = "scan-profile")]
#[derive(Default)]
struct Samples {
    spans: std::collections::BTreeMap<&'static str, (u64, f64)>,
    widths: Vec<(u32, bool)>,
    resident: Vec<serde_json::Value>,
    workers: Option<usize>,
}

#[cfg(feature = "scan-profile")]
impl Samples {
    fn absorb(&mut self, other: Self) {
        for (name, (calls, ms)) in other.spans {
            let entry = self.spans.entry(name).or_default();
            entry.0 += calls;
            entry.1 += ms;
        }
        self.widths.extend(other.widths);
        self.resident.extend(other.resident);
        self.workers = self.workers.or(other.workers);
    }
}

/// One thread's samples, on their way back to the thread that owns the scan.
#[cfg(feature = "scan-profile")]
#[derive(Default)]
pub struct Drained(Samples);

/// One thread's samples. Nothing is recorded without `scan-profile`.
#[cfg(not(feature = "scan-profile"))]
#[derive(Default)]
pub struct Drained;

/// Takes everything this thread has recorded, leaving it empty.
pub fn drain() -> Drained {
    #[cfg(feature = "scan-profile")]
    return Drained(SAMPLES.with_borrow_mut(std::mem::take));
    #[cfg(not(feature = "scan-profile"))]
    Drained
}

/// Adds another thread's samples to this one's.
///
/// Spans are summed per name, so a name that ran on several threads reports
/// their total time rather than the span of wall clock they shared. Widths and
/// cache inventories are appended in whatever order the threads finished.
pub fn absorb(_taken: Drained) {
    #[cfg(feature = "scan-profile")]
    SAMPLES.with_borrow_mut(|s| s.absorb(_taken.0));
}

/// Records how many threads read the page's words.
pub fn read_workers(_workers: usize) {
    #[cfg(feature = "scan-profile")]
    SAMPLES.with_borrow_mut(|s| s.workers = Some(_workers));
}

#[cfg(feature = "scan-profile")]
thread_local! {
    static SAMPLES: std::cell::RefCell<Samples> = Default::default();
}

#[cfg(feature = "scan-profile")]
impl Drop for Span {
    fn drop(&mut self) {
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        SAMPLES.with_borrow_mut(|s| {
            let entry = s.spans.entry(self.name).or_default();
            entry.0 += 1;
            entry.1 += ms;
        });
    }
}

// Keep the explicit scope endings valid in normal builds as well. This
// empty destructor is optimized away together with the zero-sized span.
#[cfg(not(feature = "scan-profile"))]
impl Drop for Span {
    #[inline]
    fn drop(&mut self) {}
}

/// Records the recogniser's input width and whether its plan was cached.
#[inline]
pub fn ocr_width(_width: u32, _hit: bool) {
    #[cfg(feature = "scan-profile")]
    SAMPLES.with_borrow_mut(|s| s.widths.push((_width, _hit)));
}

/// Numeric cache inventory after each OCR plan lookup. Diagnostic builds only;
/// the caller defines the byte-accounting exclusions, not an allocator estimate.
#[cfg(feature = "scan-profile")]
pub(crate) fn ocr_resident(inventory: serde_json::Value) {
    SAMPLES.with_borrow_mut(|s| s.resident.push(inventory));
}

/// Drains this thread's samples. Call outside all timed scopes.
#[cfg(feature = "scan-profile")]
pub fn take() -> serde_json::Value {
    SAMPLES.with_borrow_mut(|s| {
        let s = std::mem::take(s);
        serde_json::json!({
            "spans": s.spans.into_iter().map(|(name, (calls, ms))|
                serde_json::json!({"name": name, "calls": calls, "ms": ms})
            ).collect::<Vec<_>>(),
            "ocr_widths": s.widths.into_iter().map(|(width, hit)|
                serde_json::json!({"width": width, "cache_hit": hit})
            ).collect::<Vec<_>>(),
            "ocr_resident": s.resident,
            "read_workers": s.workers,
        })
    })
}

#[cfg(all(test, feature = "scan-profile"))]
mod tests {
    use super::*;

    #[test]
    fn nested_spans_count_and_drain_without_recording_content() {
        take();
        {
            let _outer = span("outer");
            for _ in 0..2 {
                let _inner = span("inner");
                ocr_width(48, true);
            }
        }
        let report = take();
        assert_eq!(report["spans"][0]["calls"], 2);
        assert_eq!(report["spans"][1]["calls"], 1);
        assert!(
            report["spans"][1]["ms"].as_f64().unwrap()
                >= report["spans"][0]["ms"].as_f64().unwrap()
        );
        assert_eq!(report["ocr_widths"].as_array().unwrap().len(), 2);
        assert!(take()["spans"].as_array().unwrap().is_empty());
    }

    #[test]
    fn resident_inventory_is_drained_with_the_phase() {
        take();
        ocr_resident(serde_json::json!({"plan_count": 2, "widths_lru_to_mru": [48, 96]}));
        let report = take();
        assert_eq!(report["ocr_resident"][0]["plan_count"], 2);
        assert_eq!(
            report["ocr_resident"][0]["widths_lru_to_mru"],
            serde_json::json!([48, 96])
        );
        assert!(take()["ocr_resident"].as_array().unwrap().is_empty());
    }
}
