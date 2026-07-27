//! The large test: the whole EO runtime must type with nothing rejected.
//!
//! Needs an EO checkout, so it is driven by `EO_HOME` and skips without one.
//! Section 4 of the README explains how to generate the XMIR it reads.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use eoti::infer::{Engine, Env};
use eoti::solver::Clash;
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

/// Every object of the runtime, and what the checker made of it. `fragile` turns
/// on the object-level incompleteness pass.
fn verdicts(parsed: &Path, fragile: bool) -> Vec<(String, Option<Clash>)> {
    eoti::deeply(move || walk(parsed, fragile))
}

fn walk(parsed: &Path, fragile: bool) -> Vec<(String, Option<Clash>)> {
    let mut paths = Vec::new();
    xmirs(parsed, &mut paths);
    paths.sort();
    let mut engine = Engine::lenient(fragile);
    engine.locs.learn(xmir::locators(&paths));
    let mut found = Vec::new();
    for path in &paths {
        let Ok(objects) = xmir::load(path) else {
            continue;
        };
        for (name, node) in objects {
            found.push((
                name.unwrap_or_else(|| "?".to_owned()),
                engine.infer(&node, &Env::new()).err(),
            ));
        }
    }
    found
}

/// The objects whose failure would stop a build.
fn refused(parsed: &Path, fragile: bool) -> Vec<String> {
    verdicts(parsed, fragile)
        .into_iter()
        .filter(|(_, clash)| clash.as_ref().is_some_and(Clash::gates))
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn rejects_nothing_of_the_whole_runtime() {
    let Some(parsed) = tree() else {
        eprintln!("skipped: point EO_HOME at an EO checkout to run the whole-runtime test");
        return;
    };
    assert_eq!(
        refused(&parsed, false),
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
        verdicts(&parsed, false).len() >= 153,
        "the runtime dont hold at least the objects the contract recorded"
    );
}

/// Looking for design smells may only ever add warnings. If the second pass
/// could stop a build the first one let through, nobody could afford to run it —
/// which is the whole reason it is a separate pass.
#[test]
fn stops_no_build_when_it_goes_looking_for_smells() {
    let Some(parsed) = tree() else {
        eprintln!("skipped: point EO_HOME at an EO checkout to run the whole-runtime test");
        return;
    };
    assert_eq!(
        refused(&parsed, true),
        Vec::<String>::new(),
        "looking for design smells dont leave a build it could have passed"
    );
}
