# The oracle

Scaffolding, not part of the product. A working implementation of the type
system described in `docs/eo-type-inference.tex`, kept only so a module of the
engine can be diffed against known-correct output while it is being built.

Nothing in the crate, the tests, the CI or the documentation depends on it, and
nothing should start to. **Delete this directory** once `src/` satisfies
`conformance/` — that is the point at which it stops earning its place.

```bash
python3 .oracle/eo_type_inference.py                    # the examples
python3 .oracle/eo_type_inference.py path/to/*.xmir     # type-check files
EO_HOME=/path/to/eo ./.oracle/run.sh                    # whole-tree report
```
