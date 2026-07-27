# EO type inference — gap list from hand-typing real objects

**What this is.** The result of the stress test from the plan: I took ten real
stdlib objects and tried to type each **by hand** using the checker rules in
§6 of `eo-type-inference.tex` (the `typeof` / `usable-as` steps). This file
records every place the rules go **silent** (say nothing), **wrong** (give a
bad answer), or **loop forever**. It is the input to the next step — extending
the grammar and rules — so the extensions are driven by real code, not guesses.

**Objects typed.** `number`, `seq`, `tuple/reduced` (+`mapped`), `map`,
`switch`, `malloc.for`/`of`, the `malloc` memory block (`allocated`), `while`,
`dataized`, `bool`/`true`/`false`.

---

## What already works (so we know the rules aren't all broken)

- **`bool` / `true` / `false`** type cleanly. `bool` is `[if] > bool`;
  `.if` comes out polymorphic — `for any A,B: A -> B -> (A | B)` — and
  `true`/`false` decorate `bool`. No gap.
- **`number`'s decoration** works the way §2 predicts: a value that decorates
  a number is *usable as* a number by following `@`. The accept/reject traces
  in §6 hold up.
- **Statefulness is a non-issue.** `malloc`'s `put`/`write`/`read` mutate
  memory, but types ignore evaluation order and mutation, so nothing here needs
  a special rule. Good to have confirmed rather than assumed.
- **The design-smell payoff is real.** `while` returns `false` on a zero-trip
  loop and `switch` returns `true` on no-match, so both inferred result types
  carry a stray `bool` the author never meant — exactly the kind of latent bug
  the type system is supposed to surface. Confirmed on real code.

---

## Ranked gaps

### Tier 1 — blocks a v0 checker from running at all

**1. Recursion / self-reference makes the checker loop forever.** *(the big one)*
Seen in: `seq` (`seq steps.tail`), `map` (`rec-rebuild`, `rec-key-search`),
`switch` (`rec-case`), `while` (`loop (index.plus 1)`), `number` (`eq` uses
`number`, `as-i64` uses `.as-number`), `malloc` (`resized` returns another
`allocated`).
Problem: §6's `typeof` on an object whose body refers to itself calls `typeof`
on itself again, and never stops. This is the bug that only shows up against
real code — nearly every non-trivial object hits it.
Fix direction: when we start typing an object, give it a placeholder type
variable, type the body against that, then "tie the knot" (the self-referring /
`loop A. …` shape from the grammar). Standard, but must be built in from day one.

**2. The rules *check*, but never *solve* — there is no inference engine.**
Seen in: every object with a free attribute (`number`'s `as-bytes`, `seq`'s
`steps`, `reduced`'s `list`/`func`, …).
Problem: §6 says *what* must hold ("`usable-as` must hold", "set the unknown to
X"), but real inference has to walk a whole object, **collect constraints** on
each unknown ("`steps` must have `.head`/`.tail`/`.length`", "this value is used
as both a number and a string → its type is a union"), then **solve them all
together**. The collect-and-solve step is the actual engine, and it is
unspecified. Without it, unknowns from free attributes never get pinned down.
Fix direction: this is the algebraic-subtyping core (the Simple-sub reference in
the paper). Write it as: generate constraints while walking, then solve.

**3. No rules for tuples.** Seen in: `seq`, `switch` (`cases` is a tuple of
`(bool, value)` pairs; reads `cases.head.head`), `reduced`/`mapped` (`list`),
and `tuple.eo` itself (`head`/`tail`/`length`/`at`/`with`), plus the `*`
constructor used everywhere.
Problem: the "@-core" has four moves (make, apply, dispatch, decorate) but
tuples are a fifth thing in practice, and §6 has **zero** rules for `*`,
`.head`, `.tail`, `.length`, `.at`, `.with`. Half the stdlib is untypeable
without them.
Fix direction: add tuple types (both `tuple A` and fixed `<A,B,C>`) and rules
for the constructor and the accessors. Note EO tuples grow at the head, so
`.head` = last, `.tail` = the rest.

**4. No way to *apply* a parametric atom signature.** Seen in: `recovered`,
`seq`, `while`, `tuple.head`/`with`, `map.get`, `malloc.for` — every atom whose
**result type depends on its argument's type**.
Problem: §5 wrote nice signatures like `recovered: for any A,B: A? -> B ->
(A|B)` and `seq: (tuple ending in A) -> (A | bool)`, but §6 has no mechanism to
*use* them: instantiate the "for any", bind the variables to the actual
arguments, and compute the result. §6 only knows how to type *user formations*
by looking inside them; it can't apply the §5 table. The bridge between §5 and
§6 is missing. (Includes the flip side — **generalization**: deciding when a
user object's inferred type becomes a reusable "for any A", so `reduced` can be
used at many element types.)
Fix direction: instantiate schemes on use, generalize at definitions (the
standard let-polymorphism move).

**5. Callback parameters need their type pushed *in*, not guessed.** Seen in:
`malloc` (`[m]` — `m` is the memory block), `while` (`[i]` — `i` is a number),
`reduced`/`mapped` (`func`, and the adapter `[accum item idx]`).
Problem: these atoms hand a **known** type to a callback's parameter (malloc
gives `m : block`; while gives `i : number`). If the checker treats the
callback's parameter as a bare unknown and only reads the body, it can't check
`m.put` or `m.read` — it doesn't yet know `m` is a block. The expected type has
to flow *into* the callback.
Fix direction: check callbacks against an expected `param -> result` type
(the "checking direction" — bidirectional typing).

### Tier 2 — needed for real coverage (can follow a running core)

**6. Dispatch on a union.** Seen in: `map` — `found(key)` returns either a
"found" object (`exists`, `get`) or `not-found`, and callers do `.exists` /
`.get` on the union.
Problem: §6's DISPATCH assumes a single object shape. On an `A | B` it must
require the attribute in **both** arms with compatible types (or narrow first).
Fix direction: add a dispatch-on-union rule.

**7. The data (Δ) layer needs its own story.** Seen in: `dataized`
(`any -> bytes`), `as-bytes` / `as-number` / `as-bool` / `as-i64`, byte
literals (`01-`, `00-00-…`).
Problem: there are really two layers — object *shapes* and raw *data* (bytes).
`dataized x` throws away `x`'s shape and yields bytes; `as-number` reinterprets
bytes as a number. The type system needs a coherent account of `bytes` as a
base type and of the `as-…` reinterpretations, or dataization becomes a hole.
Fix direction: treat `bytes` as a base type; give `dataized` type
`for any A: A -> bytes`; give each `as-…` a `bytes -> …` type.

**8. Free attributes have no declared type.** Seen in: `number`'s `as-bytes`
(should be `bytes`), `seq`'s `steps` (should be a tuple).
Problem: the rule makes every free attribute a fresh unknown, and EO has **no
syntax** to say "this parameter must be a `bytes`." So everything is inferred
from use — which only works once the engine (gap 2) exists, and sometimes use
is too weak to pin it down.
Fix direction: rely on use-inference where possible; decide whether to add an
optional parameter-type annotation for the cases where use is ambiguous.

### Tier 3 — corners and cleanups

**9. Forma-annotation shapes we don't yet explain.** Seen in: `read /bytes`
with `? > cant-read /{string}` (**brace forma** `/{string}` — meaning unclear),
`resized /Q.malloc.of.allocated` (**fully-qualified path to a nested object**,
and self-referential), `/number` inside `number` (**self-reference**), atoms
carrying **void sub-attributes** (`cant-read`).
Problem: §5 says "a forma is just an alias for a shape," but real annotations
come in shapes (`{…}`), FQN paths to nested objects, and self-references that
the alias story hasn't been checked against.
Fix direction: pin down what each annotation form maps to; likely `/{string}`
means "an object usable-as string" and the FQN just resolves to that nested
object's shape.

**10. Explicit `.@` dispatch.** Seen in: `map` (`(rec-rebuild pairs).@`),
`switch` (`(rec-case cases).@`), `malloc` (`m.@`).
Problem: §6 treats `@` as the *fall-through target*, but code also calls `.@`
**explicitly** to get "the decoratee of this object." Small, but the rule must
allow `e.@` as a normal dispatch.
Fix direction: allow `@` as a dispatchable label returning the decoratee's type.

---

## What this changes about the plan

The paper's next step was "build the checker." This says: **before any code,
resolve Tier 1 on paper**, because a checker without recursion-handling (1), a
solve step (2), tuples (3), parametric-signature application (4), and
callback-checking (5) cannot even run on the standard library. Those five are
the real "extend the rules" work, now concretely scoped by evidence.

Suggested order:
1. Extend §3/§6 with **recursion** (tie-the-knot) and a **collect-then-solve**
   engine — gaps 1 and 2, the spine.
2. Add **tuple** rules (3) and **parametric-atom application + generalization**
   (4) and **callback-checking** (5).
3. *Now* build a minimal prototype over XMIR for the core + these atoms; run it
   on the stdlib; let it surface Tier 2/3 in practice.
4. Fold Tier 2 (union dispatch, data layer, free-attr story) as coverage grows;
   clean up Tier 3 corners last.

A soundness proof (Lean, deferred) only after Tier 1–2 stop moving.
