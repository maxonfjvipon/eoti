//! The compiled binary, as a build would meet it.
//!
//! These go through the command line rather than the library, because the gate's
//! contract with a compiler is its exit status and the document it writes — and
//! neither is exercised by calling the engine directly.

use std::env;
use std::fs;
use std::process::{Command, Output};

/// One program of this test's own, in a directory of its own, so a run sees
/// nothing but what the test put there. That matters here: the dangling-forma
/// lint answers only for the input it is given.
fn given(what: &str, xml: &str) -> String {
    let dir = env::temp_dir().join(format!("eoti-{what}"));
    fs::create_dir_all(&dir).expect("cannot make a directory to check in");
    let path = dir.join("program.xmir");
    fs::write(&path, xml).expect("cannot write the program to check");
    path.display().to_string()
}

/// The checker, run over whatever it was pointed at.
fn eoti(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eoti"))
        .args(args)
        .output()
        .expect("cannot run the binary")
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `"hi".plus 1` — a string has no `plus`, and nor does the bytes it decorates.
fn refused() -> String {
    given(
        "refused",
        r#"<program name="refused">
             <o name="main" loc="Φ.main" line="1">
               <o name="φ" base=".plus" loc="Φ.main.φ" line="2" pos="4">
                 <o base="Φ.string" loc="Φ.main.φ.ρ" line="2"/>
                 <o base="Φ.number" as="α0" loc="Φ.main.φ.α0" line="2"/>
               </o>
             </o>
           </program>"#,
    )
}

#[test]
fn prints_its_own_version() {
    assert!(
        said(&eoti(&["--version"])).contains(env!("CARGO_PKG_VERSION")),
        "the binary dont print its own version"
    );
}

#[test]
fn stops_a_build_over_a_program_it_refuses() {
    assert_eq!(
        eoti(&[&refused()]).status.code(),
        Some(1),
        "a program the checker refuses dont stop the build"
    );
}

#[test]
fn lets_a_build_go_on_over_a_program_it_types() {
    let path = given(
        "typed",
        r#"<program name="typed">
             <o name="main" loc="Φ.main" line="1">
               <o name="φ" base=".plus" loc="Φ.main.φ" line="2">
                 <o base="Φ.number" loc="Φ.main.φ.ρ" line="2"/>
                 <o base="Φ.number" as="α0" loc="Φ.main.φ.α0" line="2"/>
               </o>
             </o>
           </program>"#,
    );
    assert_eq!(
        eoti(&[&path]).status.code(),
        Some(0),
        "a program that types dont let the build go on"
    );
}

#[test]
fn stops_a_build_it_could_not_read_the_input_of() {
    assert_eq!(
        eoti(&[&given("unread", "not xml at all <<<")])
            .status
            .code(),
        Some(1),
        "an input nobody could read dont stop the build that waved it through"
    );
}

#[test]
fn writes_the_one_field_a_compiler_gates_on() {
    assert!(
        said(&eoti(&["--json", &refused()])).contains("\"status\": \"errors\""),
        "a refused program dont write the one field a compiler gates on"
    );
}

#[test]
fn faults_a_forma_reaching_into_an_object_that_lacks_it() {
    let path = given(
        "dangling",
        r#"<program name="dangling">
             <o name="main" loc="Φ.main" line="1">
               <o name="λ" atom="Φ.main.nowhere" loc="Φ.main.λ" line="1"/>
             </o>
           </program>"#,
    );
    assert!(
        said(&eoti(&["--json", &path])).contains("ref/dangling-forma"),
        "a forma reaching into an object right here dont get faulted"
    );
}

#[test]
fn stays_quiet_about_a_forma_from_a_file_nobody_passed() {
    let path = given(
        "elsewhere",
        r#"<program name="elsewhere">
             <o name="main" loc="Φ.main" line="1">
               <o name="λ" atom="Φ.bool" loc="Φ.main.λ" line="1"/>
             </o>
           </program>"#,
    );
    assert_eq!(
        eoti(&[&path]).status.code(),
        Some(0),
        "a forma naming an object from a file nobody passed dont go unaccused"
    );
}

#[test]
fn says_there_was_nothing_to_check() {
    assert_eq!(
        eoti(&[]).status.code(),
        Some(2),
        "a run with nothing to check dont say so"
    );
}
