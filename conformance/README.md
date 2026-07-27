# Conformance suite

The contract that any implementation of the checker must satisfy, in any
language. It exists so the Rust rewrite is a *checked* migration rather than a
risky redo: the contract was frozen before a line of the engine was ported, and
the Python in `reference/` stays as the oracle a third implementation could be
checked against too.

Two parts:

- **`examples.json`** — the accept/reject cases, each with the regression it
  guards. Every implementation must produce the same verdict on every one.
  Rejection *reasons* are recorded for orientation; the verdict is the contract,
  because prose is not an API (the machine string is `code`, see §5b of the
  top-level README).
- **`runtime`** in the same file — the large test. The whole `eo-runtime`
  `1-parse` tree must type with nothing rejected, in well under a second.
  Requires an EO checkout; see §4 of the top-level README for generating it.

Verify the oracle still agrees with the contract:

```bash
python3 reference/eo_type_inference.py     # -> 13/13 examples behaved as expected
```

Amending the contract is a deliberate act. A new example is a new row here
first, then an implementation change — never the other way round.
