//! The large test: the whole EO runtime must type with nothing rejected.
//!
//! Needs an EO checkout, so it is driven by `EO_HOME` and skips without one.
//! Section 4 of the README explains how to generate the XMIR it reads.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use eoti::infer::{Engine, Env};
use eoti::xmir;

/// The `1-parse` tree of an EO checkout, if one was pointed at.
fn tree() -> Option<PathBuf> {
    let home = PathBuf::from(env::var_os("EO_HOME")?);
    let parsed = home.join("eo-runtime/target/eo/1-parse");
    parsed.is_dir().then_some(parsed)
}

fn xmirs(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            xmirs(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "xmir") {
            found.push(path);
        }
    }
}

/// Every object of the runtime, and whether it was rejected.
fn checked(parsed: &Path) -> Vec<(String, bool)> {
    eoti::deeply(|| walk(parsed))
}

fn walk(parsed: &Path) -> Vec<(String, bool)> {
    let mut paths = Vec::new();
    xmirs(parsed, &mut paths);
    paths.sort();
    let mut engine = Engine::lenient(false);
    engine.locs.learn(xmir::locators(&paths));
    let mut verdicts = Vec::new();
    for path in &paths {
        let Ok(objects) = xmir::load(path) else {
            continue;
        };
        for (name, node) in objects {
            verdicts.push((
                name.unwrap_or_else(|| "?".to_owned()),
                engine.infer(&node, &Env::new()).is_err(),
            ));
        }
    }
    verdicts
}

#[test]
fn rejects_nothing_of_the_whole_runtime() {
    let Some(parsed) = tree() else {
        eprintln!("skipped: point EO_HOME at an EO checkout to run the whole-runtime test");
        return;
    };
    let rejected: Vec<String> = checked(&parsed)
        .into_iter()
        .filter(|(_, rejected)| *rejected)
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        rejected,
        Vec::<String>::new(),
        "the runtime dont type with nothing rejected"
    );
}

#[test]
fn types_at_least_as_many_objects_as_the_contract_recorded() {
    let Some(parsed) = tree() else {
        eprintln!("skipped: point EO_HOME at an EO checkout to run the whole-runtime test");
        return;
    };
    assert!(
        checked(&parsed).len() >= 153,
        "the runtime dont hold at least the objects the contract recorded"
    );
}
