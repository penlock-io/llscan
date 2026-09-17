//! Optional, per-attempt observations of work, independent of FFI and timing.
//!
//! Observers never choose words, alter calibration or cancel model execution.

use crate::detect::Quad;

/// An identity allocated in the stage's post-split column-order index space.
pub type RegionId = u32;

/// Real boundaries of one scan; an opaque stage has no invented percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Model construction or native photo preparation.
    Preparing,
    /// Sheet identification or generic word detection.
    Finding,
    /// Fit columns, partition detections and merge cell fragments before fixing the read count.
    Geometry,
    /// Reading a fixed set of candidate regions or sheet fields.
    Reading,
    /// Numbering, trimming, rereads, joins and extent replacements.
    Checking,
    /// Packing the authoritative result for review.
    Packing,
}

/// Processing state, never a statement that a word is correct or confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionState {
    /// Geometry is available, but no reading has started.
    Found,
    /// The region is currently being read.
    Reading,
    /// A provisional reading exists; later passes can still revise it.
    Read,
    /// Retained in the result for restoration, but not selected for review.
    Excluded,
}

/// A complete replaceable region snapshot in canonical full-image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// Stable for this attempt; never reused after retirement.
    pub id: RegionId,
    /// Same orientation/frame as the resolved photo and final result.
    pub quad: Quad,
    /// Processing state, independent of confidence and user checks.
    pub state: RegionState,
}

/// Identity map computed alongside the stage's existing final reindexing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalRegion {
    /// The original stable region identity, including excluded parents.
    pub id: RegionId,
    /// Index in the final result, not a phrase position or printed number.
    pub word_index: u32,
}

/// Ordered deltas from one synchronous native attempt, with no photo/text data.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Canonical dimensions established by native decode/orientation.
    Photo {
        /// Full canonical image width.
        width: u32,
        /// Full canonical image height.
        height: u32,
    },
    /// Stage-local progress; `None` means unknown, never a guessed total.
    Work {
        /// A real work boundary.
        phase: Phase,
        /// Completed units in this phase, not all scan work.
        completed: u32,
        /// Fixed total for the phase; zero is a valid empty phase.
        total: Option<u32>,
    },
    /// Insert or replace all geometry/state for this ID.
    Region(Region),
    /// Atomically retire parent overlays and insert their selected replacement.
    Replaced {
        /// A fresh identity and its complete state.
        region: Region,
        /// Earlier IDs; their original result records remain restorable.
        parents: Vec<RegionId>,
    },
    /// Remove an overlay whose region has no final result record.
    Removed {
        /// A retired identity, never recycled.
        id: RegionId,
    },
    /// Successful native completion; UI navigation still waits for the result.
    Finished {
        /// One mapping per final record, including hidden original parents.
        regions: Vec<FinalRegion>,
    },
    /// Native failure. The normal return value supplies the error, not this event.
    Failed,
}

impl Event {
    /// Whether this ends the stream for a still-attached observer.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished { .. } | Self::Failed)
    }
}

/// Per-attempt observer, called synchronously and one event at a time.
///
/// A delivery completes before the scanner continues, and no two overlap, but
/// the thread is not fixed: reading a page runs several words at once and each
/// announces its own. Implementations must be quick, non-panicking and must
/// not re-enter the scanner. An adapter may detach after a delivery failure
/// without failing recognition. No observer is retained globally or in a
/// reusable model.
pub trait Observer: Send + Sync {
    /// Observe one event without changing scan computation.
    fn on_event(&self, event: Event);
}

/// Lazily build an event only when an observer is present.
///
/// The ordinary no-observer path performs only the option check: no vectors,
/// geometry copies, clocks, queues or FFI calls are constructed for progress.
pub fn emit(observer: Option<&dyn Observer>, event: impl FnOnce() -> Event) {
    if let Some(observer) = observer {
        observer.on_event(event());
    }
}

/// Emit a truthful phase-local count, with no payload work when unobserved.
pub fn work(observer: Option<&dyn Observer>, phase: Phase, completed: u32, total: Option<u32>) {
    emit(observer, || Event::Work {
        phase,
        completed,
        total,
    });
}

/// Stage-owned identities in allocation order. Each owner of a word
/// permutation (joins, then extent) updates the current indices in place.
#[doc(hidden)]
pub struct Stage<'a> {
    observer: Option<&'a dyn Observer>,
    regions: Vec<FinalRegion>,
    total: u32,
    next_id: RegionId,
    reading_started: bool,
}

impl<'a> Stage<'a> {
    /// Begin progress for a caller-owned fixed-field layout.
    pub fn new(observer: Option<&'a dyn Observer>, count: usize) -> Self {
        let mut stage = Self::geometry(observer, count);
        stage.reading();
        stage
    }

    /// Start geometry discovery. Candidate counts may change here, never during Reading.
    pub fn geometry(observer: Option<&'a dyn Observer>, count: usize) -> Self {
        let regions = if observer.is_some() {
            (0..count)
                .map(|i| FinalRegion {
                    id: i as u32,
                    word_index: i as u32,
                })
                .collect()
        } else {
            Vec::new()
        };
        work(observer, Phase::Geometry, 0, None);
        Self {
            observer,
            regions,
            total: count as u32,
            next_id: count as u32,
            reading_started: false,
        }
    }

    /// Freeze the final candidate count after all geometric partitions/merges.
    pub fn reading(&mut self) {
        assert!(
            !self.reading_started,
            "reading begins once after geometry is final"
        );
        self.reading_started = true;
        work(self.observer, Phase::Reading, 0, Some(self.total));
    }

    /// Emit a field's geometry/state using its stable identity.
    pub fn region(&self, index: usize, quad: &Quad, state: RegionState) {
        emit(self.observer, || {
            Event::Region(Region {
                id: self.id_at(index),
                quad: quad.clone(),
                state,
            })
        });
    }

    fn id_at(&self, index: usize) -> RegionId {
        self.regions
            .iter()
            .find(|r| r.word_index as usize == index)
            .expect("every current word has a stable region identity")
            .id
    }

    /// Report the number of completed field reads.
    pub fn read_done(&self, count: usize) {
        assert!(self.reading_started && count <= self.total as usize);
        work(
            self.observer,
            Phase::Reading,
            count as u32,
            Some(self.total),
        );
    }

    /// Geometry partition before full reading. Reuse the parent's identity for
    /// its first child, allocate fresh identities for additional children, and
    /// update the fixed reading total before emitting any completion counts.
    pub(crate) fn partitioned(&mut self, parents: &[usize], quads: &[Quad]) {
        assert!(
            !self.reading_started,
            "partition before freezing the reading count"
        );
        assert_eq!(parents.len(), quads.len());
        if self.observer.is_some() {
            let mut used = std::collections::HashSet::new();
            let mut next = self.next_id;
            let regions = parents
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    let id = if used.insert(p) {
                        self.id_at(p)
                    } else {
                        let id = next;
                        next += 1;
                        id
                    };
                    FinalRegion {
                        id,
                        word_index: i as u32,
                    }
                })
                .collect();
            self.regions = regions;
            self.next_id = next;
        }
        self.total = quads.len() as u32;
        work(self.observer, Phase::Geometry, 0, None);
        for (i, q) in quads.iter().enumerate() {
            self.region(i, q, RegionState::Found);
        }
    }

    pub(crate) fn coalesced(&mut self, parents: &[Vec<usize>], quads: &[Quad]) {
        assert!(
            !self.reading_started,
            "merge before freezing the reading count"
        );
        if let Some(observer) = self.observer {
            let regions = parents
                .iter()
                .enumerate()
                .map(|(i, group)| {
                    for &p in &group[1..] {
                        observer.on_event(Event::Removed { id: self.id_at(p) });
                    }
                    FinalRegion {
                        id: self.id_at(group[0]),
                        word_index: i as u32,
                    }
                })
                .collect();
            self.regions = regions;
        }
        self.total = quads.len() as u32;
        work(self.observer, Phase::Geometry, 0, None);
        for (i, q) in quads.iter().enumerate() {
            self.region(i, q, RegionState::Found);
        }
    }

    /// Report the final checking phase after field reads.
    pub fn checking(&self) {
        work(self.observer, Phase::Checking, 0, None);
    }

    // Called exactly where a successful union is appended. Rejected trials
    // never allocate a region or replace parents.
    pub(crate) fn joined(&mut self, quad: &Quad, parents: [usize; 2]) {
        self.replaced(quad, &parents);
    }

    pub(crate) fn alternative(&mut self, quad: &Quad) {
        if let Some(observer) = self.observer {
            let id = self.next_id;
            self.next_id += 1;
            self.regions.push(FinalRegion {
                id,
                word_index: self.regions.len() as u32,
            });
            observer.on_event(Event::Region(Region {
                id,
                quad: quad.clone(),
                state: RegionState::Excluded,
            }));
        }
    }

    // A singleton expansion and a pair union both replace overlays, without
    // conflating their separate result provenance. Parents are CURRENT word
    // indices, which can differ from IDs after an earlier owner's reindex.
    pub(crate) fn replaced(&mut self, quad: &Quad, parents: &[usize]) {
        if let Some(observer) = self.observer {
            let id = self.next_id;
            self.next_id += 1;
            let parents = parents.iter().map(|&p| self.id_at(p)).collect();
            self.regions.push(FinalRegion {
                id,
                word_index: self.regions.len() as u32,
            });
            observer.on_event(Event::Replaced {
                region: Region {
                    id,
                    quad: quad.clone(),
                    state: RegionState::Read,
                },
                parents,
            });
        }
    }

    pub(crate) fn reindex(&mut self, inverse: &[usize]) {
        // The same inverse used for result provenance, not an inferred
        // reconciliation against output geometry or a second sort.
        for region in &mut self.regions {
            region.word_index = inverse[region.word_index as usize] as u32;
        }
    }

    /// Return the identity map after all field reads/reindexing have finished.
    pub fn into_regions(self) -> Vec<FinalRegion> {
        self.regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn grid_changes_finish_before_the_single_fixed_reading_total() {
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, e: Event) {
                self.0.lock().unwrap().push(e);
            }
        }
        let observer = Recorder(Mutex::new(Vec::new()));
        let q = Quad([(0., 0.); 4]);
        let mut stage = Stage::geometry(Some(&observer), 2);
        stage.partitioned(&[0, 1, 0], &[q.clone(), q.clone(), q.clone()]);
        stage.coalesced(&[vec![0, 2], vec![1]], &[q.clone(), q]);
        stage.reading();
        stage.read_done(1);
        stage.read_done(2);
        stage.checking();
        let events = observer.0.lock().unwrap();
        let reading: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Work {
                    phase: Phase::Reading,
                    completed,
                    total,
                } => Some((*completed, *total)),
                _ => None,
            })
            .collect();
        assert_eq!(reading, [(0, Some(2)), (1, Some(2)), (2, Some(2))]);
    }

    #[test]
    #[should_panic(expected = "merge before freezing")]
    fn reading_cannot_silently_restart_after_a_merge() {
        let mut stage = Stage::new(None, 1);
        stage.coalesced(&[vec![0]], &[Quad([(0., 0.); 4])]);
    }

    #[test]
    fn coalescence_retires_fragment_ids_without_reusing_them() {
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, e: Event) {
                self.0.lock().unwrap().push(e);
            }
        }
        let observer = Recorder(Mutex::new(Vec::new()));
        let mut stage = Stage::geometry(Some(&observer), 3);
        let q = Quad([(0., 0.); 4]);
        stage.coalesced(&[vec![0, 2], vec![1]], &[q.clone(), q.clone()]);
        assert_eq!(
            stage.regions.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(
            observer
                .0
                .lock()
                .unwrap()
                .contains(&Event::Removed { id: 2 })
        );
        stage.alternative(&q);
        assert_eq!(stage.id_at(2), 3);
        stage.replaced(&q, &[0, 1]);
        assert_eq!(stage.id_at(3), 4);
    }

    #[test]
    fn partition_children_have_unique_ids_and_updated_total() {
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, e: Event) {
                self.0.lock().unwrap().push(e);
            }
        }
        let observer = Recorder(Mutex::new(Vec::new()));
        let mut stage = Stage::geometry(Some(&observer), 2);
        let q = Quad([(0., 0.); 4]);
        stage.partitioned(&[0, 1, 0], &[q.clone(), q.clone(), q.clone()]);
        assert_eq!(
            stage.regions.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        stage.reading();
        stage.read_done(3);
        assert!(observer.0.lock().unwrap().contains(&Event::Work {
            phase: Phase::Reading,
            completed: 3,
            total: Some(3)
        }));
        stage.reindex(&[2, 0, 1]);
        assert_eq!(stage.id_at(1), 2);
    }

    #[test]
    fn empty_stage_and_unobserved_identity_storage() {
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, event: Event) {
                self.0.lock().unwrap().push(event);
            }
        }
        let observer = Recorder(Mutex::new(Vec::new()));
        let stage = Stage::new(Some(&observer), 0);
        stage.checking();
        assert!(stage.into_regions().is_empty());
        assert_eq!(
            *observer.0.lock().unwrap(),
            vec![
                Event::Work {
                    phase: Phase::Geometry,
                    completed: 0,
                    total: None
                },
                Event::Work {
                    phase: Phase::Reading,
                    completed: 0,
                    total: Some(0)
                },
                Event::Work {
                    phase: Phase::Checking,
                    completed: 0,
                    total: None
                },
            ]
        );
        let mut absent = Stage::new(None, 500);
        let quad = Quad([(0., 0.); 4]);
        absent.region(499, &quad, RegionState::Reading);
        absent.joined(&quad, [100, 499]);
        absent.reindex(&[]); // No index lookup/allocation when unobserved.
        assert_eq!(absent.regions.capacity(), 0);
    }

    #[test]
    fn absent_observer_does_not_construct_an_event() {
        emit(None, || {
            panic!("no allocation or payload work on this path")
        });
    }

    #[test]
    fn observer_receives_events_in_call_order() {
        #[derive(Default)]
        struct Recorder(Mutex<Vec<Event>>);
        impl Observer for Recorder {
            fn on_event(&self, event: Event) {
                self.0.lock().unwrap().push(event);
            }
        }
        let recorder = Recorder::default();
        let events = [
            Event::Work {
                phase: Phase::Finding,
                completed: 0,
                total: None,
            },
            Event::Work {
                phase: Phase::Reading,
                completed: 0,
                total: Some(0),
            },
            Event::Finished { regions: vec![] },
        ];
        for event in &events {
            emit(Some(&recorder), || event.clone());
        }
        assert_eq!(*recorder.0.lock().unwrap(), events);
        assert!(!events[0].is_terminal());
        assert!(events[2].is_terminal());
        assert!(Event::Failed.is_terminal());
    }
}
