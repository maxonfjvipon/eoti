# Contributing

## Build and check

```bash
cargo test                                    # unit + integration tests
cargo fmt --all --check                       # formatting
cargo clippy --all-targets -- -D warnings     # lints; warnings are errors
python3 reference/eo_type_inference.py        # the oracle: 13/13
```

CI runs exactly these on Linux and macOS. A red build is not mergeable.

## The porting discipline

The engine is a rewrite of `reference/eo_type_inference.py` under the guard of
`conformance/`. Two rules follow from that, and neither is negotiable:

1. **The contract comes first.** A behavior is a row in
   `conformance/examples.json` before it is a line of Rust. The Python is the
   oracle: where the two disagree, the Python is right until the contract says
   otherwise.
2. **One module at a time.** Port `xmir`, `types`, `solver`, `infer`, `resolve`,
   `render`, `diag` as separate changes, each with the tests that pin it. The
   four MLsub subtleties in §12 of the README were each a real, silent bug in the
   Python — read them before touching `solver`.

## Working style

- Branch as `bug/#123/short-name`, commit as `bug(#123): what changed`.
- Reproduce with a failing test first; do not fix what no test catches.
- Small changes. Leave a puzzle rather than growing the diff.
- Flag a smell as an issue instead of fixing it silently.
- Never silence a lint. Fix the code; the checkers are right.

## Dependencies

Few, and each one justified. Today: `clap` for the command line, `serde_json`
for the conformance contract in tests. Planned: `roxmltree` for reading XMIR
and `serde` for emitting diagnostics. The Python reference has *zero*
dependencies by design; the Rust side should stay close to that.
