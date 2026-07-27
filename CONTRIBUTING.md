# Contributing

## Build and check

```bash
cargo test                                    # unit + integration tests
cargo fmt --all --check                       # formatting
cargo clippy --all-targets -- -D warnings     # lints; warnings are errors
```

CI runs exactly these on Linux and macOS. A red build is not mergeable.

## The build discipline

The engine is built under the guard of `conformance/`. Two rules follow from
that, and neither is negotiable:

1. **The contract comes first.** A behavior is a row in
   `conformance/examples.json` before it is a line of code. The contract was
   frozen before the engine existed, which is what makes it a guard rather than
   a record of whatever got written.
2. **One module at a time,** in the dependency order of §7 of the README:
   `types`, `solver`, `infer`, `xmir`, `resolve`, `render`, `diag`. Each lands
   as its own change with the tests that pin it. The four MLsub subtleties in
   §12 were each a real, silent bug — read them before touching `solver`.

## Working style

- Branch as `bug/#123/short-name`, commit as `bug(#123): what changed`.
- Reproduce with a failing test first; do not fix what no test catches.
- Small changes. Leave a puzzle rather than growing the diff.
- Flag a smell as an issue instead of fixing it silently.
- Never silence a lint. Fix the code; the checkers are right.

## Dependencies

Few, and each one justified. Today: `clap` for the command line, `serde_json`
for the conformance contract in tests. Planned: `roxmltree` for reading XMIR
and `serde` for emitting diagnostics. Nothing else without a reason worth
writing down — a checker that ships as one static binary should not drag a
dependency tree behind it.
