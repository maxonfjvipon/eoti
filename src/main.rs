//! The command-line entry point: XMIR in, verdicts out.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use eoti::infer::{Engine, Env};
use eoti::render::show;
use eoti::xmir;

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
    let mut rejected = 0;
    for path in &files {
        println!("== {} ==", path.display());
        let objects = match xmir::load(path) {
            Ok(objects) => objects,
            Err(trouble) => {
                println!("  (unreadable) {trouble:?}");
                rejected += 1;
                continue;
            }
        };
        for (name, node) in objects {
            let name = name.unwrap_or_else(|| "?".to_owned());
            match engine.infer(&node, &Env::new()) {
                Ok(ty) => println!("  {name} : {}", show(&engine.types, ty)),
                Err(clash) => {
                    rejected += 1;
                    println!("  {name} : REJECT -- {} {clash:?}", clash.code());
                }
            }
        }
    }
    if rejected > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
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
