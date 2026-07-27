//! Smoke tests over the compiled binary.

use std::process::Command;

#[test]
fn prints_its_own_version() {
    assert!(
        String::from_utf8_lossy(
            &Command::new(env!("CARGO_BIN_EXE_eoti"))
                .arg("--version")
                .output()
                .expect("cannot run the binary")
                .stdout
        )
        .contains(env!("CARGO_PKG_VERSION")),
        "the binary dont print its own version"
    );
}
