//! Only the release artifacts and the public example, never a private corpus.
use bitcoin_hashes::{Hash, sha256};

/// The three artifacts taken from upstream are checkable against the exports
/// they came from, which is the one thing committing them cannot say. Our own
/// bytes are identified by the commit and are not pinned twice.
#[test]
fn the_upstream_artifacts_are_the_published_exports() {
    let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
    let readme = std::fs::read_to_string(models.join("README.md")).unwrap();
    for name in [
        "word-detector.onnx",
        "text-recogniser.onnx",
        "text-recogniser.dict.txt",
    ] {
        let data = std::fs::read(models.join(name)).unwrap();
        let digest = sha256::Hash::hash(&data).to_string();
        assert!(
            readme.contains(&format!("{digest}  {name}")),
            "{name} is not the export the inventory names"
        );
    }
}

/// A calibration is fitted to one classifier's outputs, and the two arrive
/// through the API as separate byte strings: a caller can pair them wrongly in
/// a way no checkout would show.
#[test]
fn the_calibration_names_the_classifier_it_was_fitted_to() {
    let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
    let calibration = std::fs::read_to_string(models.join("word-reference.calibration")).unwrap();
    let bound = calibration
        .lines()
        .find_map(|line| line.strip_prefix("model_sha256="))
        .expect("the calibration names the classifier it was fitted to");
    let weights = std::fs::read(models.join("word-reference.onnx")).unwrap();
    assert_eq!(bound, sha256::Hash::hash(&weights).to_string());
}

/// A README animation is easy to replace with a heavier one; fetching this
/// page should stay cheap, and a recording is only worth its bytes while the
/// page still points at it.
#[test]
fn the_demo_animations_are_referenced_and_stay_small() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let mut total = 0;
    for name in ["docs/images/scan.gif", "docs/images/penlock-scan.gif"] {
        assert!(readme.contains(name), "{name} is not on the page");
        let data = std::fs::read(root.join(name)).unwrap();
        assert_eq!(&data[..6], b"GIF89a", "{name}");
        total += data.len();
    }
    assert!(total < 6 << 20, "{total} bytes of animation");
}
