//! Final box geometry and a word-free roll-up of the existing scan observations.

use std::collections::{BTreeMap, BTreeSet};

use crate::detect::Quad;
use crate::phrase::PageScan;
use crate::progress::{self, FinalRegion, Phase, Region, RegionState};

/// A final selected box in canonical full-photo pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct FinalBox {
    /// Stable within this attempt, not a word index or printed number.
    pub id: u32,
    /// Original ordered corners, without padding, rounding or clamping.
    pub quad: Quad,
}

/// The authoritative selected set after all exclusions, merges and repairs.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxScan {
    /// Canonical photo width.
    pub width: u32,
    /// Canonical photo height.
    pub height: u32,
    /// Selected geometry in stable-ID order, not phrase order.
    pub boxes: Vec<FinalBox>,
}

impl BoxScan {
    /// Project existing selection and the stage's final ID map. Never reads text
    /// or chooses boxes. Incomplete, duplicate or out-of-range maps fail loudly.
    pub fn from_page(scan: &PageScan, regions: &[FinalRegion]) -> Result<Self, String> {
        if regions.len() != scan.words.len() {
            return Err("box identity map must cover every final record".into());
        }
        let mut ids = BTreeSet::new();
        let mut indices = BTreeSet::new();
        let mut boxes = Vec::new();
        for region in regions {
            let index = region.word_index as usize;
            if !ids.insert(region.id) || !indices.insert(index) {
                return Err("duplicate box identity or final record mapping".into());
            }
            let word = scan
                .words
                .get(index)
                .ok_or("box identity index out of range")?;
            if !word.stray {
                boxes.push(FinalBox {
                    id: region.id,
                    quad: word.quad.clone(),
                });
            }
        }
        boxes.sort_by_key(|b| b.id);
        Ok(Self {
            width: scan.width,
            height: scan.height,
            boxes,
        })
    }

    /// Versioned final-output record shared by native exports and the scorer.
    pub fn to_json(&self, page: &str) -> serde_json::Value {
        serde_json::json!({
            "schema": "penlock-final-boxes-v1", "page": page,
            "width": self.width, "height": self.height,
            "boxes": self.boxes.iter().map(|b| serde_json::json!({
                "id": b.id,
                "corners": b.quad.0.iter().flat_map(|&(x, y)| [x, y]).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Word-free updates; only Completed establishes an authoritative final set.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Establish the canonical coordinate frame.
    Photo {
        /// Canonical width.
        width: u32,
        /// Canonical height.
        height: u32,
    },
    /// Real stage-local work, not an inferred percentage.
    Work {
        /// Current stage.
        phase: Phase,
        /// Completed units in this stage.
        completed: u32,
        /// Known total, or unknown.
        total: Option<u32>,
    },
    /// Insert or replace the complete provisional region snapshot.
    Region(Region),
    /// Atomically retire parents and install their replacement.
    Replaced {
        /// New stable identity and geometry.
        region: Region,
        /// Retired parent identities.
        parents: Vec<u32>,
    },
    /// Retire a provisional overlay.
    Removed {
        /// Retired identity.
        id: u32,
    },
    /// Replace all provisional state with this authoritative selected set.
    Completed(BoxScan),
    /// No final set is available from this failed attempt.
    Failed,
}

impl Event {
    /// Adapt ordinary observations, never treating a legacy word-index terminal
    /// map as box completion. The scan owner must supply Completed separately.
    pub fn observation(event: progress::Event) -> Option<Self> {
        Some(match event {
            progress::Event::Photo { width, height } => Self::Photo { width, height },
            progress::Event::Work {
                phase,
                completed,
                total,
            } => Self::Work {
                phase,
                completed,
                total,
            },
            progress::Event::Region(region) => Self::Region(region),
            progress::Event::Replaced { region, parents } => Self::Replaced { region, parents },
            progress::Event::Removed { id } => Self::Removed { id },
            progress::Event::Failed => Self::Failed,
            progress::Event::Finished { .. } => return None,
        })
    }
}

/// Enable the stage's existing stable-ID accounting without retaining events.
pub struct TrackIds;

impl progress::Observer for TrackIds {
    fn on_event(&self, _: progress::Event) {}
}

/// A reusable word-free overlay reducer. Partial state is never final output.
#[derive(Default, Debug)]
pub struct Rollup {
    regions: BTreeMap<u32, Region>,
    retired: BTreeSet<u32>,
    terminal: bool,
    result: Option<BoxScan>,
}

impl Rollup {
    /// Apply one ordered update. Completion reconciles the full selected set,
    /// including excluding provisional boxes not present in that final value.
    pub fn apply(&mut self, event: &Event) -> Result<(), String> {
        if self.terminal {
            return Err("box event after terminal outcome".into());
        }
        match event {
            Event::Photo { .. } | Event::Work { .. } => {}
            Event::Region(region) => {
                if self.retired.contains(&region.id) {
                    return Err("retired box identity reused".into());
                }
                self.regions.insert(region.id, region.clone());
            }
            Event::Replaced { region, parents } => {
                if self.regions.contains_key(&region.id)
                    || self.retired.contains(&region.id)
                    || parents.contains(&region.id)
                {
                    return Err("replacement box identity is not fresh".into());
                }
                for id in parents {
                    self.regions.remove(id);
                    self.retired.insert(*id);
                }
                self.regions.insert(region.id, region.clone());
            }
            Event::Removed { id } => {
                self.regions.remove(id);
                self.retired.insert(*id);
            }
            Event::Completed(scan) => {
                let ids: BTreeSet<_> = scan.boxes.iter().map(|b| b.id).collect();
                if ids.len() != scan.boxes.len() || ids.iter().any(|id| self.retired.contains(id)) {
                    return Err("invalid identities in box completion".into());
                }
                self.regions = scan
                    .boxes
                    .iter()
                    .map(|b| {
                        (
                            b.id,
                            Region {
                                id: b.id,
                                quad: b.quad.clone(),
                                state: RegionState::Read,
                            },
                        )
                    })
                    .collect();
                self.result = Some(scan.clone());
                self.terminal = true;
            }
            Event::Failed => {
                self.regions.clear();
                self.terminal = true;
            }
        }
        Ok(())
    }

    /// Current non-excluded overlays, still provisional until completion.
    pub fn visible(&self) -> impl Iterator<Item = &Region> {
        self.regions
            .values()
            .filter(|r| r.state != RegionState::Excluded)
    }

    /// Successful final result, absent while running and after failure.
    pub fn result(&self) -> Option<&BoxScan> {
        self.result.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::{WordBox, WordEvidence};

    fn quad(x: f32) -> Quad {
        Quad([
            (x, 10.25),
            (x + 20.5, 8.125),
            (x + 22., 20.),
            (x + 1.5, 22.125),
        ])
    }

    fn word(x: f32, stray: bool) -> WordBox {
        WordBox {
            quad: quad(x),
            crop: image::GrayImage::new(1, 1),
            ranked: vec![],
            selection: None,
            column_rank: 0,
            row_rank: 0,
            turned: false,
            raw: None,
            evidence: WordEvidence::default(),
            narrowed: false,
            stray,
            number: None,
            label: None,
            apart: false,
            joined_from: None,
            expanded_from: vec![],
        }
    }

    fn page() -> PageScan {
        PageScan {
            page_direction: None,
            width: 100,
            height: 80,
            words: vec![word(50., false), word(0., true), word(20., false)],
            numbering: crate::numbering::Numbering::None,
            join_trials: vec![],
            sources: vec![],
        }
    }

    fn region(id: u32, state: RegionState) -> Region {
        Region {
            id,
            quad: quad(id as f32),
            state,
        }
    }

    #[test]
    fn box_projection_preserves_reindexed_ids_and_unrounded_geometry() {
        let mut page = page();
        let map = [
            FinalRegion {
                id: 8,
                word_index: 1,
            },
            FinalRegion {
                id: 9,
                word_index: 0,
            },
            FinalRegion {
                id: 3,
                word_index: 2,
            },
        ];
        let result = BoxScan::from_page(&page, &map).unwrap();
        assert_eq!(
            result.boxes,
            vec![
                FinalBox {
                    id: 3,
                    quad: quad(20.)
                },
                FinalBox {
                    id: 9,
                    quad: quad(50.)
                }
            ]
        );
        assert_eq!((result.width, result.height), (100, 80));
        page.words[0].turned = true;
        page.words[0].label = Some("different text".into());
        page.words[0].ranked = vec![(42, 0.9)];
        assert_eq!(BoxScan::from_page(&page, &map).unwrap(), result);
        let record = result.to_json("test.png");
        assert_eq!(record["schema"], "penlock-final-boxes-v1");
        assert_eq!(record["boxes"][0]["corners"][1], 10.25);
        assert!(record["boxes"][0].get("word_index").is_none());
    }

    #[test]
    fn box_projection_refuses_incomplete_or_ambiguous_identity_maps() {
        let page = page();
        let map: Vec<_> = (0..3)
            .map(|i| FinalRegion {
                id: i,
                word_index: i,
            })
            .collect();
        assert!(BoxScan::from_page(&page, &map[..2]).is_err());
        for (id, index) in [(1, 0), (0, 1), (0, 99)] {
            let mut bad = map.clone();
            bad[0] = FinalRegion {
                id,
                word_index: index,
            };
            assert!(BoxScan::from_page(&page, &bad).is_err());
        }
    }

    #[test]
    fn box_rollup_reconciles_updates_exclusion_merge_removal_and_final_repair() {
        let mut state = Rollup::default();
        for id in 0..4 {
            state
                .apply(&Event::Region(region(id, RegionState::Found)))
                .unwrap();
        }
        state
            .apply(&Event::Region(region(0, RegionState::Reading)))
            .unwrap();
        state
            .apply(&Event::Region(region(2, RegionState::Excluded)))
            .unwrap();
        state
            .apply(&Event::Replaced {
                region: region(4, RegionState::Read),
                parents: vec![0, 1],
            })
            .unwrap();
        state.apply(&Event::Removed { id: 3 }).unwrap();
        assert_eq!(state.visible().map(|r| r.id).collect::<Vec<_>>(), vec![4]);
        assert!(state.result().is_none());
        let result = BoxScan {
            width: 100,
            height: 80,
            boxes: vec![FinalBox {
                id: 4,
                quad: quad(50.),
            }],
        };
        state.apply(&Event::Completed(result.clone())).unwrap();
        assert_eq!(state.result(), Some(&result));
        assert_eq!(state.visible().next().unwrap().quad, quad(50.));
        assert!(state.apply(&Event::Failed).is_err());
    }

    #[test]
    fn empty_box_success_is_distinct_from_failure_or_partial_stream() {
        let mut state = Rollup::default();
        assert!(state.result().is_none());
        state.apply(&Event::Failed).unwrap();
        assert!(state.result().is_none());
        let result = BoxScan {
            width: 4,
            height: 3,
            boxes: vec![],
        };
        let mut success = Rollup::default();
        success.apply(&Event::Completed(result.clone())).unwrap();
        assert_eq!(success.result(), Some(&result));
        assert!(Event::observation(progress::Event::Finished { regions: vec![] }).is_none());
        assert_eq!(
            Event::observation(progress::Event::Failed),
            Some(Event::Failed)
        );
    }
}
