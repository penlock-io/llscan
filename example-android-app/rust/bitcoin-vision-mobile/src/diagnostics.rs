//! Explicit saved-stage transport. No image encoding, recognition or I/O here.
use crate::{InitialOrder, Layout, PhotoUpSource, PhraseScan, VisionError};
use bitcoin_vision::phrase::PageScan;
use bitcoin_vision::progress::FinalRegion;
use serde_json::{Value, json};

/// One named UTF-8 stage artifact, created only by the diagnostic scan method.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DiagnosticFile {
    /// Fixed relative filename within the saved stage directory.
    pub name: String,
    /// JSON or JSONL containing the actual native observations.
    pub text: String,
}

/// Normal review result plus opt-in saved processing evidence for that same call.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DiagnosedScan {
    /// Unchanged review words, crops, orders and geometry.
    pub scan: PhraseScan,
    /// No photo pixels or second encoded crop copies; use scan.words' PNGs.
    pub files: Vec<DiagnosticFile>,
}

const PAGE: &str = "photo.png";

pub(crate) fn pack(
    scan: &PageScan,
    result: &PhraseScan,
    regions: &[FinalRegion],
    orientation: u8,
    photo_up: PhotoUpSource,
) -> Result<Vec<DiagnosticFile>, VisionError> {
    let boxes = bitcoin_vision::boxes::BoxScan::from_page(scan, regions)
        .map_err(|detail| VisionError::Scan { detail })?;
    let file = |name: &str, rows: Vec<Value>| DiagnosticFile {
        name: name.into(),
        text: rows.into_iter().map(|r| r.to_string() + "\n").collect(),
    };
    let order = match result.initial_order {
        InitialOrder::Numbers => &result.numbered,
        InitialOrder::Rows => &result.rows,
        InitialOrder::Columns => &result.columns,
    };
    let sources = scan.sources.iter().map(|s| {
        use bitcoin_vision::sources::Outcome;
        let outcome = match &s.outcome {
            Outcome::Parts(parts)=>json!({"kind":"parts","parts":parts.iter().map(|p| json!({"part":p.part,"quad":p.quad.0,"word_index":p.word_index})).collect::<Vec<_>>()}),
            Outcome::Empty(reason)=>json!({"kind":"empty","reason":reason.as_str()}),
            Outcome::Rescued{reason,word_index}=>json!({"kind":"rescued","reason":reason.as_str(),"word_index":word_index}),
        };
        json!({"detector":s.detector,"quad":s.quad.0,"outcome":outcome,
            "decision":s.decision.as_ref().map(|d| json!({"reason":d.reason(),
                "scale":d.scale().map(|s| json!({"height":s.height,"supports":s.supports})),
                "owner":d.owner().map(|o| json!({"source":o.source,"part":o.part,"word_index":o.word_index(&scan.sources)})),
                "union_geometry":match d {
                    bitcoin_vision::fragments::Decision::Tiny { ownership:bitcoin_vision::fragments::Ownership::Ending {union,footprint,..},.. } => Some(json!({"quad":union.0,"footprint":footprint.0})),
                    _=>None,
                }})),
            "union":s.union.as_ref().map(|u| json!({"word_index":u.word_index,"selected":u.selected,"reason":u.reason}))})
    }).collect::<Vec<_>>();
    let mut numbering = match &scan.numbering {
        bitcoin_vision::numbering::Numbering::None => json!({"state":"none"}),
        bitcoin_vision::numbering::Numbering::Inconsistent { detail } => {
            json!({"state":"inconsistent","detail":detail})
        }
        bitcoin_vision::numbering::Numbering::Held {
            count,
            missing,
            out_of_place,
            unresolved,
            ..
        } => {
            json!({"state":"held","count":count,"missing":missing,"out_of_place":out_of_place,"unresolved":unresolved})
        }
    };
    numbering["page"] = json!(PAGE);
    numbering["list_numbered"] = json!(result.list_numbered);
    Ok(vec![
        file("manifest.json",vec![json!({"schema":"penlock-phone-decisions-v1","page":PAGE,
            "coordinate_frame":"canonical_photo","orientation_applied":orientation,"layout":if result.layout==Layout::Page {"page"} else {"word_sheet"},
            "photo_up_source":bitcoin_vision::page_frame::PhotoUp::from(photo_up).source(),
            "page_writing_direction":scan.page_direction.as_ref().and_then(|d| d.decisions.as_ref()).map(|d|d.to_json()),
            "package_version":env!("CARGO_PKG_VERSION"),"code_revision":option_env!("VISION_BUILD_REVISION"),
            "word_count":scan.words.len(),"scope":"native scan before user edits"})]),
        file("final-boxes.jsonl",vec![boxes.to_json(PAGE)]),
        file("word-links.jsonl",regions.iter().filter(|r| !scan.words[r.word_index as usize].stray).map(|r| json!({"schema":"penlock-box-word-links-v1","page":PAGE,"box_id":r.id,"word_index":r.word_index})).collect()),
        file("readings.jsonl",scan.words.iter().enumerate().map(|(i,w)| json!({"page":PAGE,"file":format!("{PAGE}~w{i}.png"),
            "kept":!w.stray,"corners":w.quad.0,"ranked":result.words[i].ranked.iter().map(|r| json!([r.word,r.probability])).collect::<Vec<_>>(),
            "raw":w.raw.as_ref().map(|r| vec![&r.text]),"raw_confidence":w.raw.as_ref().map(|r| r.confidence),
            "selection":w.selection.as_ref().map(|s| s.diagnostic()),"verdict":format!("{:?}",w.verdict()).to_lowercase(),
            "label":w.label,"number":w.number,"apart":w.apart,"narrowed":w.narrowed,"evidence":w.evidence.to_json(),
            "joined_from":w.joined_from,"expanded_from":w.expanded_from.iter().map(|p| p.word_index).collect::<Vec<_>>(),
            "original_crop":w.evidence.original.as_ref().map(|_| format!("original-crops/{PAGE}~w{i}.png"))})).collect()),
        file("sources.jsonl",vec![json!({"page":PAGE,"sources":sources})]),
        file("numbering.jsonl",vec![numbering]),
        file("ordering.jsonl",vec![json!({"page":PAGE,"initial_order":format!("{:?}",result.initial_order).to_lowercase(),
            "requires_review":result.order_requires_review,"basis":result.order_basis,
            "layout_support":scan.initial_traversal().layout_support.to_json(),
            "columns_pass":result.columns_pass,"rows_pass":result.rows_pass,"kept_indices":order.iter().copied().filter(|&i| !scan.words[i as usize].stray).collect::<Vec<_>>()})]),
        file("joins.jsonl",vec![json!({"page":PAGE,"trials":scan.join_trials.iter().map(|t| json!({"column":t.column,"row":t.row,"parents":t.parents,"reason":t.reason,
            "read":t.read.as_ref().map(|r| json!({"top":r.top,"probability":r.probability,"margin":r.margin,"pick":r.pick,"kept":r.kept}))})).collect::<Vec<_>>()})]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_vision::phrase::{DecisionTrace, WordBox};

    fn fixture() -> PageScan {
        let word = WordBox {
            quad: bitcoin_vision::detect::Quad([(10., 20.), (50., 20.), (50., 40.), (10., 40.)]),
            crop: bitcoin_vision::image::GrayImage::from_raw(2, 3, vec![0, 20, 40, 60, 80, 255])
                .unwrap(),
            ranked: vec![(0, 0.9), (1, 0.1)],
            selection: Some(
                bitcoin_vision::hybrid::select(
                    Some("abandon"),
                    None,
                    &[(0, 0.9), (1, 0.1)],
                    0.85,
                    0.75,
                )
                .unwrap(),
            ),
            column_rank: 0,
            row_rank: 0,
            turned: false,
            raw: Some(bitcoin_vision::recogniser::LineRead {
                text: "abandon".into(),
                confidence: 0.6,
            }),
            evidence: Default::default(),
            narrowed: false,
            stray: false,
            number: None,
            label: None,
            apart: false,
            joined_from: None,
            expanded_from: vec![],
        };
        let mut excluded = word.clone();
        excluded.stray = true;
        excluded.column_rank = 1;
        excluded.row_rank = 1;
        PageScan {
            page_direction: None,
            width: 100,
            height: 100,
            words: vec![word, excluded],
            numbering: bitcoin_vision::numbering::Numbering::None,
            join_trials: vec![],
            sources: vec![],
        }
    }

    #[test]
    fn ambiguous_order_review_requirement_survives_mobile_packing() {
        let mut scan = fixture();
        scan.words[1].stray = false;
        scan.words[0].row_rank = 1;
        scan.words[1].row_rank = 0;
        let result = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        assert!(result.order_requires_review);
        assert_eq!(result.order_basis, "equal_dimensions_unverified_layout");
        assert_eq!(result.initial_order, crate::InitialOrder::Columns);
        scan.words[0].row_rank = 0;
        scan.words[1].row_rank = 1;
        let result = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        assert!(!result.order_requires_review);
        assert_eq!(result.order_basis, "same_retained_traversal");
    }

    #[test]
    fn saved_transport_preserves_review_and_explicit_identity_without_reading() {
        let mut scan = fixture();
        let regions = [
            FinalRegion {
                id: 42,
                word_index: 1,
            },
            FinalRegion {
                id: 91,
                word_index: 0,
            },
        ];
        let normal = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        let plain = pack(&scan, &normal, &regions, 6, PhotoUpSource::Camera).unwrap();
        scan.words[0].evidence.decisions = Some(DecisionTrace::default());
        let traced = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        assert_eq!(
            normal, traced,
            "diagnostics must not alter review bytes/order/selection"
        );
        let files = pack(&scan, &traced, &regions, 6, PhotoUpSource::Camera).unwrap();
        assert_eq!(files.len(), 8);
        let rows = |files: &[DiagnosticFile], name: &str| -> Vec<Value> {
            files
                .iter()
                .find(|f| f.name == name)
                .unwrap()
                .text
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        };
        let readings = rows(&files, "readings.jsonl");
        assert_eq!(
            readings.len(),
            2,
            "inactive observations keep their original indices"
        );
        assert_eq!(readings[0]["raw"], json!(["abandon"]));
        assert_eq!(
            readings[0]["selection"],
            scan.words[0].selection.as_ref().unwrap().diagnostic()
        );
        assert_eq!(
            readings[0]["evidence"]["decisions"],
            scan.words[0].evidence.decisions.as_ref().unwrap().to_json()
        );
        assert!(
            rows(&plain, "readings.jsonl")[0]["evidence"]
                .get("decisions")
                .is_none()
        );
        assert_eq!(readings[1]["kept"], false);
        assert_eq!(
            rows(&files, "word-links.jsonl"),
            vec![
                json!({"schema":"penlock-box-word-links-v1","page":"photo.png","box_id":91,"word_index":0})
            ]
        );
        let boxes = rows(&files, "final-boxes.jsonl");
        assert_eq!(boxes[0]["boxes"].as_array().unwrap().len(), 1);
        assert_eq!(boxes[0]["boxes"][0]["id"], 91);
        assert_eq!(
            rows(&files, "ordering.jsonl")[0]["kept_indices"],
            json!([0])
        );
        assert_eq!(
            rows(&files, "ordering.jsonl")[0]["requires_review"],
            json!(traced.order_requires_review)
        );
        assert_eq!(
            rows(&files, "ordering.jsonl")[0]["basis"],
            json!(traced.order_basis)
        );
        assert_eq!(rows(&files, "manifest.json")[0]["orientation_applied"], 6);
        assert_eq!(
            rows(&files, "manifest.json")[0]["photo_up_source"],
            "camera"
        );
        assert_eq!(
            bitcoin_vision::image::load_from_memory(&traced.words[0].crop_png)
                .unwrap()
                .to_luma8(),
            scan.words[0].crop
        );
    }

    #[test]
    fn saved_transport_refuses_incomplete_or_conflicting_region_links() {
        let scan = fixture();
        let packed = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        for regions in [
            vec![],
            vec![FinalRegion {
                id: 1,
                word_index: 0,
            }],
            vec![
                FinalRegion {
                    id: 1,
                    word_index: 0,
                },
                FinalRegion {
                    id: 1,
                    word_index: 1,
                },
            ],
            vec![
                FinalRegion {
                    id: 1,
                    word_index: 0,
                },
                FinalRegion {
                    id: 2,
                    word_index: 5,
                },
            ],
        ] {
            assert!(pack(&scan, &packed, &regions, 1, PhotoUpSource::Unknown).is_err());
        }
    }

    #[test]
    fn empty_page_still_exports_the_page_direction_and_separate_photo_transform() {
        let mut scan = fixture();
        scan.words.clear();
        scan.sources.clear();
        scan.page_direction = Some(
            bitcoin_vision::page_frame::PageAxis::from_detections(&[])
                .resolve(
                    &bitcoin_vision::image::RgbImage::new(0, 0),
                    bitcoin_vision::page_frame::PhotoUp::Unknown,
                    None,
                    0.15,
                    true,
                )
                .unwrap(),
        );
        let result = crate::pack_scan(Layout::Page, &scan, None).unwrap();
        let files = pack(&scan, &result, &[], 1, PhotoUpSource::Unknown).unwrap();
        let manifest: Value = serde_json::from_str(
            &files
                .iter()
                .find(|f| f.name == "manifest.json")
                .unwrap()
                .text,
        )
        .unwrap();
        assert_eq!(manifest["orientation_applied"], 1);
        assert_eq!(manifest["photo_up_source"], "unknown");
        let decision = &manifest["page_writing_direction"]["events"][0];
        assert_eq!(
            decision["axis_reason"],
            "no_valid_detections_canonical_horizontal"
        );
        assert_eq!(decision["orientation_reads"], 0);
    }
}
