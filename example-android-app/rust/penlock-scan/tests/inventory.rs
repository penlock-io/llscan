//! The classifier this crate loads, pinned where this crate can see it.
//!
//! It is kept beside the app rather than in the repository's `models/`
//! directory, which is a reason and not an excuse: it is documented to the same
//! standard, so neither pipeline's artifacts can drift undetected. The root
//! crate cannot hold this guard — its package excludes the app tree, so the
//! test would pass in a checkout and fail in the published crate.
use bitcoin_hashes::{Hash, sha256};

#[test]
fn the_penlock_classifier_matches_its_own_inventory() {
    let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app-models");
    let data = std::fs::read(models.join("cell-reference.onnx")).unwrap();
    let readme = std::fs::read_to_string(models.join("README.md")).unwrap();
    let digest = sha256::Hash::hash(&data).to_string();
    assert!(
        readme.contains(&format!("{digest}  cell-reference.onnx")),
        "the inventory does not name the bytes that ship"
    );
    assert!(readme.contains(&thousands(data.len())), "byte count");
}

fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
