//! Boxes-only UniFFI output over the existing scan owner and live observations.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bitcoin_vision::{boxes as core, progress};

use crate::{ProgressDeliveryError, ScanRegion, ScanWorkPhase, VisionError};

/// A selected box; no reading, word index or confirmation data.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct FinalBox {
    /// Attempt-local stable identity, not a printed number.
    pub id: u32,
    /// Eight ordered x/y coordinates in canonical full-photo pixels.
    pub corners: Vec<f32>,
}

/// Authoritative selected geometry after all native repairs and exclusions.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct BoxScan {
    /// Canonical photo width.
    pub width: u32,
    /// Canonical photo height.
    pub height: u32,
    /// Final set in stable-ID order, not phrase order.
    pub boxes: Vec<FinalBox>,
}

impl From<core::BoxScan> for BoxScan {
    fn from(scan: core::BoxScan) -> Self {
        Self {
            width: scan.width,
            height: scan.height,
            boxes: scan
                .boxes
                .into_iter()
                .map(|b| FinalBox {
                    id: b.id,
                    corners: b.quad.0.into_iter().flat_map(|(x, y)| [x, y]).collect(),
                })
                .collect(),
        }
    }
}

/// Box/work updates. Only Completed carries the final set, never word indices.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum BoxProgressUpdate {
    /// Establish canonical photo coordinates.
    Photo {
        /// Canonical width.
        width: u32,
        /// Canonical height.
        height: u32,
    },
    /// Real stage-local work units.
    Work {
        /// Current stage.
        phase: ScanWorkPhase,
        /// Completed units.
        completed: u32,
        /// Total, when known.
        total: Option<u32>,
    },
    /// Insert/update a complete provisional region snapshot.
    Region {
        /// Geometry and processing state, not confirmation.
        region: ScanRegion,
    },
    /// Atomically retire parent overlays and install their replacement.
    Replaced {
        /// Fresh replacement identity.
        region: ScanRegion,
        /// Retired parent identities.
        parents: Vec<u32>,
    },
    /// Retire an overlay with no final selected box.
    Removed {
        /// Retired stable identity.
        id: u32,
    },
    /// Replace all provisional state with the authoritative returned set.
    Completed {
        /// Same final value as the ordinary successful return.
        scan: BoxScan,
    },
    /// Failure, not a successfully completed empty set.
    Failed,
}

/// Ordered per-call envelope; the observer closure supplies its attempt token.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct BoxProgressEvent {
    /// Starts at one and increases for every delivered update.
    pub sequence: u64,
    /// Word-free work, geometry or terminal outcome.
    pub update: BoxProgressUpdate,
}

/// Box-only observer, called synchronously on the native scan worker.
#[uniffi::export(callback_interface)]
pub trait BoxObserver: Send + Sync {
    /// Enqueue or reduce quickly; never re-enter the scanner. Delivery failure
    /// detaches observation without changing recognition or its return value.
    fn on_boxes(&self, event: BoxProgressEvent) -> Result<(), ProgressDeliveryError>;
}

pub(crate) struct Callback {
    callback: Option<Box<dyn BoxObserver>>,
    attached: AtomicBool,
    next: AtomicU64,
}

impl Callback {
    pub(crate) fn new(callback: Option<Box<dyn BoxObserver>>) -> Self {
        Self {
            attached: AtomicBool::new(callback.is_some()),
            callback,
            next: AtomicU64::new(1),
        }
    }

    fn emit(&self, update: BoxProgressUpdate) {
        if !self.attached.load(Ordering::Relaxed) {
            return;
        }
        let terminal = matches!(
            update,
            BoxProgressUpdate::Completed { .. } | BoxProgressUpdate::Failed
        );
        let delivered =
            self.callback
                .as_ref()
                .expect("attached callback")
                .on_boxes(BoxProgressEvent {
                    sequence: self.next.fetch_add(1, Ordering::Relaxed),
                    update,
                });
        if terminal || delivered.is_err() {
            self.attached.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn complete(
        &self,
        run: impl FnOnce() -> Result<core::BoxScan, VisionError>,
    ) -> Result<BoxScan, VisionError> {
        match run() {
            Ok(scan) => {
                let scan = BoxScan::from(scan);
                if self.attached.load(Ordering::Relaxed) {
                    self.emit(BoxProgressUpdate::Completed { scan: scan.clone() });
                }
                Ok(scan)
            }
            Err(error) => {
                self.emit(BoxProgressUpdate::Failed);
                Err(error)
            }
        }
    }
}

impl progress::Observer for Callback {
    fn on_event(&self, event: progress::Event) {
        if !self.attached.load(Ordering::Relaxed) {
            return;
        }
        if let Some(event) = core::Event::observation(event) {
            self.emit(event.into());
        }
    }
}

impl From<core::Event> for BoxProgressUpdate {
    fn from(event: core::Event) -> Self {
        match event {
            core::Event::Photo { width, height } => Self::Photo { width, height },
            core::Event::Work {
                phase,
                completed,
                total,
            } => Self::Work {
                phase: match phase {
                    progress::Phase::Preparing => ScanWorkPhase::Preparing,
                    progress::Phase::Finding => ScanWorkPhase::Finding,
                    progress::Phase::Geometry => ScanWorkPhase::Geometry,
                    progress::Phase::Reading => ScanWorkPhase::Reading,
                    progress::Phase::Checking => ScanWorkPhase::Checking,
                    progress::Phase::Packing => ScanWorkPhase::Packing,
                },
                completed,
                total,
            },
            core::Event::Region(region) => Self::Region {
                region: region.into(),
            },
            core::Event::Replaced { region, parents } => Self::Replaced {
                region: region.into(),
                parents,
            },
            core::Event::Removed { id } => Self::Removed { id },
            core::Event::Completed(scan) => Self::Completed { scan: scan.into() },
            core::Event::Failed => Self::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use progress::Observer;
    use std::sync::{Arc, Mutex};

    struct Recorder {
        events: Arc<Mutex<Vec<BoxProgressEvent>>>,
        detach: bool,
    }
    impl BoxObserver for Recorder {
        fn on_boxes(&self, event: BoxProgressEvent) -> Result<(), ProgressDeliveryError> {
            self.events.lock().unwrap().push(event);
            if self.detach {
                Err(ProgressDeliveryError::Detached)
            } else {
                Ok(())
            }
        }
    }

    fn scan() -> core::BoxScan {
        core::BoxScan {
            width: 100,
            height: 80,
            boxes: vec![core::FinalBox {
                id: 7,
                quad: bitcoin_vision::detect::Quad([(1.25, 2.5), (30., 1.), (32., 12.), (2., 13.)]),
            }],
        }
    }

    #[test]
    fn box_completion_delivers_the_returned_value_without_word_indices() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = Callback::new(Some(Box::new(Recorder {
            events: events.clone(),
            detach: false,
        })));
        let result = callback
            .complete(|| {
                callback.on_event(progress::Event::Photo {
                    width: 100,
                    height: 80,
                });
                callback.on_event(progress::Event::Removed { id: 3 });
                Ok(scan())
            })
            .unwrap();
        callback.on_event(progress::Event::Removed { id: 7 });
        let events = events.lock().unwrap();
        assert_eq!(
            events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            events[2].update,
            BoxProgressUpdate::Completed {
                scan: result.clone()
            }
        );
        assert_eq!(result.boxes[0].corners[0], 1.25);
        // Public signatures return only boxes; no scanner/model construction.
        let _: fn(&crate::PhraseScanner, Vec<u8>, u8) -> Result<BoxScan, VisionError> =
            crate::PhraseScanner::scan_boxes;
    }

    #[test]
    fn box_callback_detachment_does_not_cancel_or_change_result() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = Callback::new(Some(Box::new(Recorder {
            events: events.clone(),
            detach: true,
        })));
        let result = callback
            .complete(|| {
                callback.on_event(progress::Event::Photo {
                    width: 100,
                    height: 80,
                });
                callback.on_event(progress::Event::Removed { id: 3 });
                Ok(scan())
            })
            .unwrap();
        assert_eq!(result, BoxScan::from(scan()));
        assert_eq!(events.lock().unwrap().len(), 1);
        assert_eq!(Callback::new(None).complete(|| Ok(scan())).unwrap(), result);
    }

    #[test]
    fn box_failure_is_terminal_and_not_an_empty_success() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = Callback::new(Some(Box::new(Recorder {
            events: events.clone(),
            detach: false,
        })));
        assert!(
            callback
                .complete(|| Err(VisionError::Scan {
                    detail: "fixture failure".into()
                }))
                .is_err()
        );
        callback.on_event(progress::Event::Removed { id: 3 });
        assert_eq!(events.lock().unwrap()[0].update, BoxProgressUpdate::Failed);
        assert_eq!(events.lock().unwrap().len(), 1);
    }
}
