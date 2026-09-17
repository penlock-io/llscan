//! Per-call UniFFI adapter for the plain scan observer.

use bitcoin_vision::progress as core;
use std::sync::Mutex;
// Outside a diagnostic build only the tests still count with an atomic.
#[cfg(any(test, feature = "scan-profile"))]
use std::sync::atomic::{AtomicBool, Ordering};

/// Real stage boundaries, not predicted fractions of elapsed time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ScanWorkPhase {
    /// Model/photo preparation.
    Preparing,
    /// Sheet identification or generic detection.
    Finding,
    /// Fit columns and consolidate boxes before freezing the reading count.
    Geometry,
    /// Reading a fixed set of regions.
    Reading,
    /// Numbering, trimming and joins.
    Checking,
    /// Packing the final result.
    Packing,
}

/// Native processing status, not acceptance or human confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ScanRegionState {
    /// A region's geometry is known.
    Found,
    /// A native read is in progress.
    Reading,
    /// A provisional reading is available.
    Read,
    /// The result retains this region, but does not initially select it.
    Excluded,
}

/// Compact, complete geometry/state snapshot; no words, image or crop bytes.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ScanRegion {
    /// Stable attempt-local ID, assigned by the stage, not the UI.
    pub id: u32,
    /// Eight coordinates: clockwise x/y pairs in canonical full-image pixels.
    pub corners: Vec<f32>,
    /// Whether this region has been processed, independent of confidence.
    pub state: ScanRegionState,
}

/// A stable region's index in the authoritative final result.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ScanRegionMapping {
    /// Stable identity, including hidden originals retained for restoration.
    pub id: u32,
    /// Index in PhraseScan.words, not the printed word number.
    pub word_index: u32,
}

/// One progress update; structural updates must not be dropped individually.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum ScanProgressUpdate {
    /// The frame shared by all subsequent region coordinates.
    Photo {
        /// Canonical width.
        width: u32,
        /// Canonical height.
        height: u32,
    },
    /// Work units within a declared stage.
    Work {
        /// Current real stage.
        phase: ScanWorkPhase,
        /// Completed units.
        completed: u32,
        /// Fixed stage total, or unknown; zero is allowed.
        total: Option<u32>,
    },
    /// Insert or update all state for this region.
    Region {
        /// Complete state, not an index into a transient detector output.
        region: ScanRegion,
    },
    /// Insert a selected replacement while retiring its parent overlays.
    Replaced {
        /// Fresh replacement identity and state.
        region: ScanRegion,
        /// Original IDs, still available through the final result's mapping.
        parents: Vec<u32>,
    },
    /// Permanently retire an overlay with no final result record.
    Removed {
        /// The identity being retired.
        id: u32,
    },
    /// The native result is ready; Kotlin still waits for its normal return.
    Finished {
        /// ID-to-result mapping computed at the existing stage reindex.
        regions: Vec<ScanRegionMapping>,
    },
    /// The operation failed; the ordinary Result carries the error details.
    Failed,
}

/// A per-call sequence envelope. The Kotlin closure supplies its attempt token.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ScanProgressEvent {
    /// Starts at one and increases for every delivered event in this call.
    pub sequence: u64,
    /// Geometry, processing status, counts or a terminal outcome.
    pub update: ScanProgressUpdate,
}

/// Delivery failures detach observation, never fail or change recognition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Error)]
pub enum ProgressDeliveryError {
    /// The owning attempt is no longer observed.
    Detached,
    /// A foreign callback failed unexpectedly; no exception text is retained.
    Failed,
}

impl std::fmt::Display for ProgressDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("progress observer detached or failed")
    }
}
impl std::error::Error for ProgressDeliveryError {}

impl From<uniffi::UnexpectedUniFFICallbackError> for ProgressDeliveryError {
    fn from(_: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed
    }
}

/// Per-attempt foreign callback; only enqueue/reduce, never render or block.
#[uniffi::export(callback_interface)]
pub trait ScanProgressObserver: Send + Sync {
    /// Runs before the native call returns and never alongside another
    /// delivery, though not always on the thread that made the call: the
    /// words of a page are read several at a time.
    fn on_progress(&self, event: ScanProgressEvent) -> Result<(), ProgressDeliveryError>;
}

/// Judges how a stream was delivered rather than what it carried.
///
/// Delivery is serialized and finishes before the scan returns, but the thread
/// is not fixed: the words of a page are read several at a time and each
/// announces its own. A harness that wants the contract enforced wraps every
/// delivery in [`DeliveryWatch::deliver`], calls [`DeliveryWatch::returned`]
/// when the scan comes back, and reads [`DeliveryWatch::faulted`].
///
/// Entering, leaving and returning are one lifecycle rather than three flags,
/// because the interesting failure is a delivery that spans the return: a
/// worker still publishing from a scan that has already answered its caller.
/// The state is held only to cross those boundaries and never across the
/// delivery itself, which would serialize the overlap this exists to catch.
#[derive(Default)]
pub struct DeliveryWatch {
    state: Mutex<Delivery>,
}

#[derive(Default)]
struct Delivery {
    open: usize,
    returned: bool,
    faulted: bool,
}

impl DeliveryWatch {
    /// Runs one delivery, faulting if another is already open, if the scan has
    /// already returned, or if the scan returns before this one finishes.
    pub fn deliver<T>(&self, body: impl FnOnce() -> T) -> T {
        {
            let mut state = self.locked();
            state.faulted |= state.returned || state.open > 0;
            state.open += 1;
        }
        let delivered = body();
        let mut state = self.locked();
        state.open -= 1;
        state.faulted |= state.returned;
        delivered
    }

    /// Marks the scan as returned. Anything still being delivered at that
    /// moment outlived the call that produced it.
    pub fn returned(&self) {
        let mut state = self.locked();
        state.faulted |= state.open > 0;
        state.returned = true;
    }

    /// Whether any delivery overlapped another, outlived the scan, or followed
    /// its return.
    pub fn faulted(&self) -> bool {
        self.locked().faulted
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Delivery> {
        self.state
            .lock()
            .expect("a delivery that panicked is already a failed scan")
    }
}

// Production adapter; only the synthetic handshake below is scan-profile-only.
pub(crate) struct CallbackObserver {
    callback: Box<dyn ScanProgressObserver>,
    stream: Mutex<Stream>,
}

/// The envelope the app validates: the next sequence number, and whether it is
/// still listening.
///
/// The app reads a gap in the sequence as a lost event and refuses the whole
/// stream, so taking a number and delivering under it cannot be two steps.
/// Several regions are read at once, and any two of them would otherwise be
/// free to swap places between the two.
struct Stream {
    next: u64,
    attached: bool,
}

impl CallbackObserver {
    pub(crate) fn new(callback: Box<dyn ScanProgressObserver>) -> Self {
        Self {
            callback,
            stream: Mutex::new(Stream {
                next: 1,
                attached: true,
            }),
        }
    }
}

// One outer return path covers decode, sheet/read failures and packing errors.
// The result stays authoritative even if the foreign observer has detached.
pub(crate) fn complete<T, E>(
    observer: Option<&dyn core::Observer>,
    run: impl FnOnce() -> Result<(T, Vec<core::FinalRegion>), E>,
) -> Result<T, E> {
    match run() {
        Ok((value, regions)) => {
            core::emit(observer, || core::Event::Finished { regions });
            Ok(value)
        }
        Err(error) => {
            core::emit(observer, || core::Event::Failed);
            Err(error)
        }
    }
}

#[cfg(test)]
impl CallbackObserver {
    /// Whether a delivery is in progress on some other thread.
    fn publishing(&self) -> bool {
        self.stream.try_lock().is_err()
    }
}

impl core::Observer for CallbackObserver {
    fn on_event(&self, event: core::Event) {
        let terminal = event.is_terminal();
        let mut stream = self
            .stream
            .lock()
            .expect("the foreign callback returns a result rather than unwinding");
        if !stream.attached {
            return;
        }
        let sequence = stream.next;
        stream.next += 1;
        let delivered = self.callback.on_progress(ScanProgressEvent {
            sequence,
            update: event.into(),
        });
        if delivered.is_err() || terminal {
            stream.attached = false;
        }
    }
}

impl From<core::Region> for ScanRegion {
    fn from(region: core::Region) -> Self {
        Self {
            id: region.id,
            corners: region
                .quad
                .0
                .into_iter()
                .flat_map(|(x, y)| [x, y])
                .collect(),
            state: match region.state {
                core::RegionState::Found => ScanRegionState::Found,
                core::RegionState::Reading => ScanRegionState::Reading,
                core::RegionState::Read => ScanRegionState::Read,
                core::RegionState::Excluded => ScanRegionState::Excluded,
            },
        }
    }
}

impl From<core::Event> for ScanProgressUpdate {
    fn from(event: core::Event) -> Self {
        match event {
            core::Event::Photo { width, height } => Self::Photo { width, height },
            core::Event::Work {
                phase,
                completed,
                total,
            } => Self::Work {
                phase: match phase {
                    core::Phase::Preparing => ScanWorkPhase::Preparing,
                    core::Phase::Finding => ScanWorkPhase::Finding,
                    core::Phase::Geometry => ScanWorkPhase::Geometry,
                    core::Phase::Reading => ScanWorkPhase::Reading,
                    core::Phase::Checking => ScanWorkPhase::Checking,
                    core::Phase::Packing => ScanWorkPhase::Packing,
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
            core::Event::Finished { regions } => Self::Finished {
                regions: regions
                    .into_iter()
                    .map(|r| ScanRegionMapping {
                        id: r.id,
                        word_index: r.word_index,
                    })
                    .collect(),
            },
            core::Event::Failed => Self::Failed,
        }
    }
}

/// Synthetic transport handshake only: no models, photos or scans are loaded.
#[cfg(feature = "scan-profile")]
#[derive(uniffi::Object, Default)]
pub struct ProgressCallbackProbe {
    released: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
    used: AtomicBool,
}

#[cfg(feature = "scan-profile")]
#[uniffi::export]
impl ProgressCallbackProbe {
    /// Create a one-use handshake that times out rather than blocking forever.
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// The test thread releases native work after observing the live callback.
    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }

    /// Emit synthetic geometry, wait for release, then emit completion.
    /// Returns whether released before the timeout, not recognition success.
    pub fn run(&self, observer: Box<dyn ScanProgressObserver>) -> bool {
        use bitcoin_vision::detect::Quad;
        use bitcoin_vision::progress::{Event, FinalRegion, Observer, Phase, Region, RegionState};
        if self.used.swap(true, Ordering::Relaxed) {
            return false;
        }
        let observer = CallbackObserver::new(observer);
        observer.on_event(Event::Photo {
            width: 640,
            height: 480,
        });
        observer.on_event(Event::Work {
            phase: Phase::Reading,
            completed: 0,
            total: Some(1),
        });
        let mut region = Region {
            id: 0,
            quad: Quad([(10., 20.), (110., 20.), (110., 50.), (10., 50.)]),
            state: RegionState::Reading,
        };
        observer.on_event(Event::Region(region.clone()));
        let (released, _) = self
            .wake
            .wait_timeout_while(
                self.released.lock().unwrap(),
                std::time::Duration::from_secs(10),
                |done| !*done,
            )
            .unwrap();
        if !*released {
            observer.on_event(Event::Failed);
            return false;
        }
        drop(released);
        region.state = RegionState::Read;
        observer.on_event(Event::Region(region));
        observer.on_event(Event::Work {
            phase: Phase::Reading,
            completed: 1,
            total: Some(1),
        });
        observer.on_event(Event::Finished {
            regions: vec![FinalRegion {
                id: 0,
                word_index: 0,
            }],
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_vision::progress::Observer;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    #[test]
    fn outer_result_emits_one_terminal_for_errors_at_any_stage_and_success_after_packing() {
        for phase in [
            core::Phase::Preparing,
            core::Phase::Finding,
            core::Phase::Geometry,
            core::Phase::Reading,
            core::Phase::Checking,
            core::Phase::Packing,
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let observer = CallbackObserver::new(Box::new(Recorder {
                events: events.clone(),
                fail: false,
            }));
            let result: Result<(), &str> = complete(Some(&observer), || {
                core::work(Some(&observer), phase, 0, None);
                Err("injected operation failure")
            });
            assert_eq!(result, Err("injected operation failure"));
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(events[1].update, ScanProgressUpdate::Failed);
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = CallbackObserver::new(Box::new(Recorder {
            events: events.clone(),
            fail: false,
        }));
        let result: Result<u32, &str> = complete(Some(&observer), || {
            core::work(Some(&observer), core::Phase::Packing, 0, None);
            assert_eq!(events.lock().unwrap().len(), 1); // Not finished during packing.
            Ok((7, vec![]))
        });
        assert_eq!(result, Ok(7));
        assert_eq!(
            events.lock().unwrap()[1].update,
            ScanProgressUpdate::Finished { regions: vec![] }
        );
        assert_eq!(complete(None, || Ok::<_, &str>((7, vec![]))), Ok(7));
    }

    struct Recorder {
        events: Arc<Mutex<Vec<ScanProgressEvent>>>,
        fail: bool,
    }
    impl ScanProgressObserver for Recorder {
        fn on_progress(&self, event: ScanProgressEvent) -> Result<(), ProgressDeliveryError> {
            self.events.lock().unwrap().push(event);
            if self.fail {
                Err(ProgressDeliveryError::Detached)
            } else {
                Ok(())
            }
        }
    }

    /// Records the sequence it was handed, and can be held inside delivery.
    #[derive(Default)]
    struct Held {
        seen: Mutex<Vec<u64>>,
        inside: Mutex<bool>,
        arrived: std::sync::Condvar,
        release: Mutex<bool>,
        released: std::sync::Condvar,
        hold_first: bool,
        fail_after: Option<usize>,
    }

    impl Held {
        fn wait_until_inside(&self) {
            let mut inside = self.inside.lock().unwrap();
            while !*inside {
                let (next, timeout) = self
                    .arrived
                    .wait_timeout(inside, Duration::from_secs(10))
                    .unwrap();
                assert!(!timeout.timed_out(), "nobody ever entered delivery");
                inside = next;
            }
        }

        fn let_go(&self) {
            *self.release.lock().unwrap() = true;
            self.released.notify_all();
        }
    }

    impl ScanProgressObserver for Arc<Held> {
        fn on_progress(&self, event: ScanProgressEvent) -> Result<(), ProgressDeliveryError> {
            let first = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(event.sequence);
                seen.len() == 1
            };
            if self.hold_first && first {
                *self.inside.lock().unwrap() = true;
                self.arrived.notify_all();
                let mut release = self.release.lock().unwrap();
                while !*release {
                    let (next, timeout) = self
                        .released
                        .wait_timeout(release, Duration::from_secs(10))
                        .unwrap();
                    assert!(!timeout.timed_out(), "the held delivery was never let go");
                    release = next;
                }
            }
            match self.fail_after {
                Some(n) if self.seen.lock().unwrap().len() >= n => {
                    Err(ProgressDeliveryError::Detached)
                }
                _ => Ok(()),
            }
        }
    }

    fn held(held: &Arc<Held>) -> CallbackObserver {
        CallbackObserver::new(Box::new(held.clone()))
    }

    fn reading(completed: u32) -> core::Event {
        core::Event::Work {
            phase: core::Phase::Reading,
            completed,
            total: Some(32),
        }
    }

    #[test]
    fn publishers_starting_together_hand_the_app_a_gap_free_sequence() {
        // The envelope only: each publisher's own counts are not a valid
        // reading stream, and the parallel pass is where those are checked.
        let seen = Arc::new(Held::default());
        let observer = held(&seen);
        let publishers = 8;
        let each = 6;
        let start = std::sync::Barrier::new(publishers);
        std::thread::scope(|scope| {
            for _ in 0..publishers {
                scope.spawn(|| {
                    start.wait();
                    for completed in 0..each {
                        observer.on_event(reading(completed as u32));
                    }
                });
            }
        });
        assert_eq!(
            *seen.seen.lock().unwrap(),
            (1..=(publishers * each) as u64).collect::<Vec<_>>(),
            "the app refuses the whole stream on one gap or swap"
        );
    }

    #[test]
    fn a_second_publisher_waits_outside_a_delivery_already_under_way() {
        let first = Arc::new(Held {
            hold_first: true,
            ..Held::default()
        });
        let observer = held(&first);
        assert!(!observer.publishing(), "nothing is being delivered yet");
        std::thread::scope(|scope| {
            scope.spawn(|| observer.on_event(reading(0)));
            first.wait_until_inside();
            // The envelope is held for as long as the app is being handed the
            // event, which is what stops a second publisher taking a number and
            // arriving with it first.
            assert!(
                observer.publishing(),
                "the stream was free while a delivery was open"
            );
            let second = scope.spawn(|| observer.on_event(reading(1)));
            assert_eq!(
                *first.seen.lock().unwrap(),
                vec![1],
                "a second delivery entered while the first was still inside"
            );
            first.let_go();
            second.join().unwrap();
        });
        assert!(!observer.publishing());
        assert_eq!(*first.seen.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn a_refused_delivery_detaches_the_stream_for_every_publisher() {
        let refusing = Arc::new(Held {
            fail_after: Some(3),
            ..Held::default()
        });
        let observer = held(&refusing);
        let publishers = 4;
        let start = std::sync::Barrier::new(publishers);
        std::thread::scope(|scope| {
            for _ in 0..publishers {
                scope.spawn(|| {
                    start.wait();
                    for completed in 0..5 {
                        observer.on_event(reading(completed));
                    }
                });
            }
        });
        assert_eq!(
            *refusing.seen.lock().unwrap(),
            vec![1, 2, 3],
            "delivery continued after the app refused one"
        );
    }

    /// Holds a body open until the test lets it go.
    #[derive(Default)]
    struct Hold {
        inside: Mutex<bool>,
        arrived: std::sync::Condvar,
        go: Mutex<bool>,
        released: std::sync::Condvar,
    }

    impl Hold {
        /// Called from inside the body: announce, then wait to be let go.
        fn keep(&self) {
            *self.inside.lock().unwrap() = true;
            self.arrived.notify_all();
            let mut go = self.go.lock().unwrap();
            while !*go {
                let (next, timeout) = self
                    .released
                    .wait_timeout(go, Duration::from_secs(10))
                    .unwrap();
                assert!(!timeout.timed_out(), "the held body was never let go");
                go = next;
            }
        }

        fn wait_until_inside(&self) {
            let mut inside = self.inside.lock().unwrap();
            while !*inside {
                let (next, timeout) = self
                    .arrived
                    .wait_timeout(inside, Duration::from_secs(10))
                    .unwrap();
                assert!(!timeout.timed_out(), "nobody ever entered the body");
                inside = next;
            }
        }

        fn let_go(&self) {
            *self.go.lock().unwrap() = true;
            self.released.notify_all();
        }
    }

    #[test]
    fn a_word_announced_from_a_worker_is_an_ordinary_delivery() {
        let watch = DeliveryWatch::default();
        std::thread::scope(|scope| {
            scope.spawn(|| watch.deliver(|| {}));
        });
        watch.returned();
        assert!(!watch.faulted());
    }

    #[test]
    fn two_deliveries_open_at_once_are_a_fault() {
        let watch = DeliveryWatch::default();
        let held = Hold::default();
        std::thread::scope(|scope| {
            scope.spawn(|| watch.deliver(|| held.keep()));
            held.wait_until_inside();
            watch.deliver(|| {});
            held.let_go();
        });
        assert!(watch.faulted(), "the app was handed two events at once");
    }

    #[test]
    fn a_delivery_still_open_when_the_scan_returns_is_a_fault() {
        let watch = DeliveryWatch::default();
        let held = Hold::default();
        std::thread::scope(|scope| {
            scope.spawn(|| watch.deliver(|| held.keep()));
            held.wait_until_inside();
            // The scan answers its caller while a worker is still publishing.
            watch.returned();
            assert!(watch.faulted(), "a delivery outlived the scan");
            held.let_go();
        });
        assert!(watch.faulted());
    }

    #[test]
    fn a_delivery_after_the_scan_returned_is_a_fault() {
        let watch = DeliveryWatch::default();
        watch.returned();
        watch.deliver(|| {});
        assert!(watch.faulted());
    }

    #[test]
    fn delivery_is_ordered_and_terminal_suppresses_late_events() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = CallbackObserver::new(Box::new(Recorder {
            events: events.clone(),
            fail: false,
        }));
        observer.on_event(core::Event::Photo {
            width: 5,
            height: 4,
        });
        observer.on_event(core::Event::Finished {
            regions: vec![core::FinalRegion {
                id: 8,
                word_index: 2,
            }],
        });
        observer.on_event(core::Event::Failed);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
        assert_eq!(
            events[1].update,
            ScanProgressUpdate::Finished {
                regions: vec![ScanRegionMapping {
                    id: 8,
                    word_index: 2
                }]
            }
        );
    }

    #[test]
    fn delivery_error_detaches_without_propagating_to_the_caller() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = CallbackObserver::new(Box::new(Recorder {
            events: events.clone(),
            fail: true,
        }));
        observer.on_event(core::Event::Photo {
            width: 1,
            height: 1,
        });
        observer.on_event(core::Event::Failed);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert_eq!(
            ProgressDeliveryError::from(uniffi::UnexpectedUniFFICallbackError::new("not retained")),
            ProgressDeliveryError::Failed
        );
    }

    #[test]
    fn native_failure_is_terminal_and_callback_is_owned_only_by_the_call() {
        struct Owned {
            calls: Arc<AtomicU64>,
            dropped: Arc<AtomicBool>,
        }
        impl ScanProgressObserver for Owned {
            fn on_progress(&self, event: ScanProgressEvent) -> Result<(), ProgressDeliveryError> {
                assert_eq!(event.update, ScanProgressUpdate::Failed);
                self.calls.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        }
        impl Drop for Owned {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }
        let calls = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        {
            let observer = CallbackObserver::new(Box::new(Owned {
                calls: calls.clone(),
                dropped: dropped.clone(),
            }));
            observer.on_event(core::Event::Failed);
            observer.on_event(core::Event::Finished { regions: vec![] });
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            assert!(!dropped.load(Ordering::Relaxed));
        }
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn revisions_keep_geometry_and_parent_ids_in_one_update() {
        let region = core::Region {
            id: 12,
            quad: bitcoin_vision::detect::Quad([(1., 2.), (8., 2.), (8., 6.), (1., 6.)]),
            state: core::RegionState::Read,
        };
        let update = ScanProgressUpdate::from(core::Event::Replaced {
            region: region.clone(),
            parents: vec![3, 9],
        });
        assert_eq!(
            update,
            ScanProgressUpdate::Replaced {
                region: ScanRegion {
                    id: 12,
                    corners: vec![1., 2., 8., 2., 8., 6., 1., 6.],
                    state: ScanRegionState::Read,
                },
                parents: vec![3, 9],
            }
        );
        assert_eq!(
            ScanProgressUpdate::from(core::Event::Removed { id: 12 }),
            ScanProgressUpdate::Removed { id: 12 }
        );
        assert_eq!(
            ScanRegion::from(core::Region {
                state: core::RegionState::Excluded,
                ..region
            })
            .state,
            ScanRegionState::Excluded
        );
    }
}
