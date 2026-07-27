# XMIR fixtures

Small `1-parse` XMIR programs and their expected verdicts, so the repository is
testable without an EO checkout. The checkout stays necessary only for
whole-runtime conformance.

The filename states the verdict: an `accept-` file must type with nothing
rejected, a `reject-` file must reject. Keeping the expectation in the name means
a fixture cannot quietly drift away from what it was added to pin, and
`tests/fixtures.rs` needs no table to read it from.

A fixture earns its place by pinning something the prose alone could get away
with being wrong about. What is here:

| fixture | what it pins |
| --- | --- |
| `accept-number-plus` | the plain case types at all |
| `accept-open-requirement` | a requirement on a parameter stays open in an open world |
| `accept-lenient-global` | an unmodelled global becomes an unknown, not an error (§15) |
| `accept-callback-argument-type` | a callback that fits what `args=` promises is accepted |
| `reject-string-plus` | width subtyping does not invent an attribute the decoratee lacks |
| `reject-unknown-attribute` | nor does the `bytes` a number decorates |
| `reject-callback-argument-type` | a callback that cannot take what `args=` promises is refused |

The two `callback-argument-type` files come as a pair on purpose: a reading of
`args=` that only ever refuses would be no better than one that never looks.
