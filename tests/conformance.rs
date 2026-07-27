//! Tests over the frozen conformance contract.
//!
//! Until the engine is ported these assert the contract itself is intact; each
//! ported piece then adds the test that runs the contract against it.

use std::fs;

/// The contract, parsed.
fn contract() -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/examples.json"
        ))
        .expect("cannot read the conformance contract"),
    )
    .expect("cannot parse the conformance contract")
}

#[test]
fn freezes_every_reference_example() {
    assert_eq!(
        contract()["examples"]
            .as_array()
            .expect("the contract dont list its examples")
            .len(),
        13,
        "the contract dont freeze all 13 reference examples"
    );
}

#[test]
fn expects_a_verdict_of_every_example() {
    assert!(
        contract()["examples"]
            .as_array()
            .expect("the contract dont list its examples")
            .iter()
            .all(|example| matches!(example["expect"].as_str(), Some("ok" | "reject"))),
        "an example dont expect either verdict"
    );
}

#[test]
fn rejects_nothing_of_the_runtime() {
    assert_eq!(
        contract()["runtime"]["rejected"].as_u64(),
        Some(0),
        "the contract dont demand a clean runtime"
    );
}
