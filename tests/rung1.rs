//! Rungs 1 and 5, self-contained: the reference kit's example and interrupt
//! manifests produce records equal to the committed reference traces.
//!
//! The authoritative check is `python -m m68000_python.conformance diff`
//! (scripts/rung1.sh, scripts/rung5.sh); this test lets `cargo test` catch a
//! regression without Python. `tests/data` holds copies of
//! m68000-python's `examples/conformance/` at the pinned commit; the
//! reference's `mnemonic` and `operands` are dropped before comparing, since
//! this core writes neither.

use std::path::{Path, PathBuf};

use m68000_rust::conformance::{load_manifest, trace_manifest};
use m68000_rust::trace::record_to_json;
use serde_json::Value;

fn check(manifest: &Path) {
    let loaded = load_manifest(manifest).expect("manifest loads");
    let mut produced: Vec<Value> = Vec::new();
    trace_manifest(&loaded, true, |record| {
        produced.push(serde_json::from_str(&record_to_json(&record)).unwrap());
    });
    let reference = std::fs::read_to_string(manifest.with_extension("jsonl")).unwrap();
    let mut expected: Vec<Value> = reference
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for record in &mut expected {
        if let Some(instruction) = record["instruction"].as_object_mut() {
            instruction.remove("mnemonic");
            instruction.remove("operands");
        }
    }
    let name = manifest.display();
    assert_eq!(produced.len(), expected.len(), "{name}: record count");
    for (position, (mine, theirs)) in produced.iter().zip(&expected).enumerate() {
        assert_eq!(mine, theirs, "{name}: record {position} differs");
    }
}

fn manifests(directory: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(directory);
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    found.sort();
    found
}

#[test]
fn example_manifests_match_the_reference() {
    let found = manifests("tests/data");
    assert_eq!(found.len(), 3);
    found.iter().for_each(|path| check(path));
}

#[test]
fn interrupt_manifests_match_the_reference() {
    let found = manifests("tests/data/interrupts");
    assert_eq!(found.len(), 20);
    found.iter().for_each(|path| check(path));
}
