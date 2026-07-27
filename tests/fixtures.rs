//! Every XMIR fixture, checked against the verdict its name claims.
//!
//! A file named `accept-*` must type with nothing rejected; a `reject-*` must
//! reject at least one of its objects. Keeping the expectation in the filename
//! means a fixture cannot drift away from what it was added to pin.

use std::fs;
use std::path::PathBuf;

use eoti::infer::{Engine, Env};
use eoti::xmir;

fn fixtures() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> =
        fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/conformance/fixtures"))
            .expect("cannot read the fixture directory")
            .map(|entry| entry.expect("cannot read a fixture").path())
            .filter(|path| path.extension().is_some_and(|kind| kind == "xmir"))
            .collect();
    found.sort();
    found
}

/// Whether checking one fixture rejects anything, and what it was called.
fn verdicts() -> Vec<(String, bool)> {
    let paths = fixtures();
    let mut engine = Engine::lenient(false);
    engine.locs.learn(xmir::locators(&paths));
    paths
        .iter()
        .map(|path| {
            let objects = xmir::load(path).expect("a fixture is not readable XMIR");
            let rejected = objects
                .iter()
                .any(|(_, node)| engine.infer(node, &Env::new()).is_err());
            (
                path.file_name()
                    .expect("a fixture without a name")
                    .to_string_lossy()
                    .into_owned(),
                rejected,
            )
        })
        .collect()
}

#[test]
fn every_fixture_reaches_the_verdict_its_name_claims() {
    let wrong: Vec<String> = verdicts()
        .into_iter()
        .filter(|(name, rejected)| *rejected != name.starts_with("reject-"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        wrong,
        Vec::<String>::new(),
        "a fixture dont reach the verdict its name claims"
    );
}

#[test]
fn there_are_fixtures_of_both_verdicts() {
    let names: Vec<String> = verdicts().into_iter().map(|(name, _)| name).collect();
    assert!(
        names.iter().any(|name| name.starts_with("accept-"))
            && names.iter().any(|name| name.starts_with("reject-")),
        "the fixtures dont cover both verdicts"
    );
}
