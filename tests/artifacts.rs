//! Only the release artifacts and the public example, never a private corpus.
use bitcoin_hashes::{Hash, sha256};
use std::collections::BTreeMap;

const PROSE: [&str; 3] = ["README.md", "NOTICE", "LICENSE-APACHE"];

/// `models/README.md` is the document of record for what ships. Deriving the
/// expectation from it, rather than repeating digests here, means adopting a
/// model is one edit and a half-done adoption fails loudly.
#[test]
fn shipped_model_inventory_matches_notices() {
    let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models");
    let readme = std::fs::read_to_string(models.join("README.md")).unwrap();

    let mut declared = BTreeMap::new();
    for line in readme.lines() {
        let Some((digest, name)) = line.split_once("  ") else {
            continue;
        };
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        assert!(
            declared.insert(name, digest).is_none(),
            "{name} is listed twice"
        );
    }

    let mut shipped: Vec<_> = std::fs::read_dir(&models)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with('.') && !PROSE.contains(&name.as_str()))
        .collect();
    shipped.sort();
    let listed: Vec<_> = declared.keys().map(|name| name.to_string()).collect();
    assert_eq!(
        shipped, listed,
        "the inventory and the shipped files disagree"
    );

    let mut bytes = 0;
    let mut digests = BTreeMap::new();
    for (&name, &declared) in &declared {
        let data = std::fs::read(models.join(name)).unwrap();
        bytes += data.len();
        let digest = sha256::Hash::hash(&data).to_string();
        assert_eq!(digest, declared, "{name}");
        digests.insert(name, digest);
    }

    let total: usize = readme
        .split_once("Required set: ")
        .and_then(|(_, rest)| rest.split_once(" bytes"))
        .map(|(count, _)| count.replace(',', "").parse().unwrap())
        .expect("the inventory states the required byte count");
    assert_eq!(bytes, total, "the inventory's stated byte count");

    let calibration = std::fs::read_to_string(models.join("word-reference.calibration")).unwrap();
    let bound = calibration
        .lines()
        .find_map(|line| line.strip_prefix("model_sha256="))
        .expect("the calibration names the classifier it was fitted to");
    assert_eq!(
        bound, digests["word-reference.onnx"],
        "the calibration is bound to a classifier that is not the one shipping"
    );
}

#[test]
fn public_example_images_have_no_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, dimensions) in [
        ("examples/photos/handwritten.png", (3456, 4608)),
        ("docs/images/handwritten-input.png", (750, 1000)),
        ("docs/images/handwritten-boxes.png", (750, 1000)),
    ] {
        let data = std::fs::read(root.join(name)).unwrap();
        assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n");
        let mut cursor = 8;
        while cursor < data.len() {
            let length = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
            let kind = &data[cursor + 4..cursor + 8];
            assert!(
                [b"IHDR", b"IDAT", b"IEND"].contains(&kind.try_into().unwrap()),
                "unexpected PNG metadata in {name}: {kind:?}"
            );
            cursor += 12 + length;
        }
        assert_eq!(cursor, data.len());
        let decoded = image::load_from_memory(&data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), dimensions);
    }
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
