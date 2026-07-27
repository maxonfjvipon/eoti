# Conformance suite

The contract the checker must satisfy, in any implementation and any language.
It exists so the engine is *checked* rather than merely written: the contract
was frozen before a line of the engine was built, so it guards the work instead
of recording it.

Two parts:

- **`examples.json`** — the accept/reject cases, each with the regression it
  guards. Every implementation must produce the same verdict on every one.
  Rejection *reasons* are recorded for orientation; the verdict is the contract,
  because prose is not an API (the machine string is `code`, see §5b of the
  top-level README).
- **`runtime`** in the same file — the large test. The whole `eo-runtime`
  `1-parse` tree must type with nothing rejected, in well under a second. Only
  the rejection count is asserted; the object and smell counts are the observed
  baseline. Requires an EO checkout; see §4 of the top-level README for
  generating one.
- **`fixtures/`** — small XMIR programs so the repository is testable without an
  EO checkout. The filename states the verdict: an `accept-` file must type with
  nothing rejected, a `reject-` file must reject. Keeping the expectation in the
  name means a fixture cannot quietly drift away from what it was added to pin.

Amending the contract is a deliberate act. A new behavior is a row here first,
then an implementation change — never the other way round.
