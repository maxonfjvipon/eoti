//! The command-line entry point: XMIR in, verdicts out.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use eoti::diag::Findings;
use eoti::infer::{Engine, Env};
use eoti::render::{explain, show};
use eoti::xmir::{self, Broken};

/// What the checker was asked to look at.
#[derive(Parser)]
#[command(
    name = "eoti",
    version,
    about = "Structural type inference and pre-run type checker for EO, over XMIR"
)]
struct Args {
    /// XMIR files to check, as EO's parser writes them into `1-parse`
    #[arg(value_name = "XMIR")]
    files: Vec<PathBuf>,
    /// Emit the machine-readable diagnostics a build gates on
    #[arg(long)]
    json: bool,
    /// Report how often each unmodelled atom was reached, instead of checking
    #[arg(long)]
    atoms: bool,
    /// Treat an object with an unset attribute as fragile
    #[arg(long)]
    incomplete: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    eoti::deeply(move || check(args))
}

fn check(args: Args) -> ExitCode {
    let Args {
        files,
        json,
        atoms: only_atoms,
        incomplete,
    } = args;
    if files.is_empty() {
        eprintln!("nothing to check, pass one or more XMIR files");
        return ExitCode::from(2);
    }
    let mut engine = Engine::lenient(incomplete);
    engine.locs.learn(xmir::locators(&files));
    if only_atoms {
        atoms(&mut engine, &files);
        return ExitCode::SUCCESS;
    }
    let mut findings = Findings::default();
    let dangling = xmir::survey(&files).dangling();
    for broken in &dangling {
        findings.dangling(broken);
    }
    for path in &files {
        if !json {
            println!("== {} ==", path.display());
        }
        let objects = match xmir::load(path) {
            Ok(objects) => objects,
            Err(trouble) => {
                if !json {
                    println!("  {trouble}");
                }
                findings.unreadable(&trouble);
                continue;
            }
        };
        for (name, node) in objects {
            let name = name.unwrap_or_else(|| "?".to_owned());
            match engine.infer(&node, &Env::new()) {
                Ok(ty) => {
                    findings.typed();
                    if !json {
                        println!("  {name} : {}", show(&engine.types, ty));
                    }
                }
                Err(clash) => {
                    if !json {
                        println!(
                            "  {name} : {} -- {}",
                            if clash.gates() { "REJECT" } else { "WARN" },
                            explain(&engine.types, &clash)
                        );
                    }
                    findings.rejected(&engine.types, &clash);
                }
            }
        }
    }
    let (broken, refused, flagged) = (findings.errors(), findings.refused(), findings.flagged());
    let report = findings.report(&common(&files));
    if json {
        println!("{}", report.json());
    } else {
        summarize(report.summary.objects, &dangling, refused, flagged);
    }
    if broken > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// The last word of a human-readable run: the broken references, which sit under
/// no object of their own, and then the counts.
fn summarize(objects: usize, dangling: &[Broken], refused: usize, flagged: usize) {
    println!();
    for broken in dangling {
        println!("  {broken}");
    }
    println!(
        "{objects} object(s), {} typed, {refused} rejected{}",
        objects - refused - flagged,
        if flagged > 0 {
            format!(", {flagged} flagged")
        } else {
            String::new()
        }
    );
}

/// The directory the inputs share, which is what a consumer means by the source
/// of a report.
fn common(files: &[PathBuf]) -> String {
    let mut shared: Option<PathBuf> = None;
    for path in files {
        let Some(parent) = path.parent() else {
            continue;
        };
        shared = Some(match shared {
            None => parent.to_path_buf(),
            Some(known) => shorten(&known, parent),
        });
    }
    shared.unwrap_or_default().display().to_string()
}

/// The longest prefix two directories share.
fn shorten(known: &Path, other: &Path) -> PathBuf {
    known
        .components()
        .zip(other.components())
        .take_while(|(left, right)| left == right)
        .map(|(left, _)| left)
        .collect()
}

/// Which atoms hit the forgiving fallback, most frequent first.
fn atoms(engine: &mut Engine, files: &[PathBuf]) {
    for path in files {
        let Ok(objects) = xmir::load(path) else {
            continue;
        };
        for (_, node) in objects {
            let _ = engine.infer(&node, &Env::new());
        }
    }
    let mut counted: Vec<(&String, &usize)> = engine.leniency.unmodelled.iter().collect();
    counted.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    for (name, hits) in counted {
        println!("{hits:5}  {name}");
    }
}
