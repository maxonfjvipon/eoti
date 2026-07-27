# eoti — EO type inference

[![ci](https://github.com/maxonfjvipon/eoti/actions/workflows/ci.yml/badge.svg)](https://github.com/maxonfjvipon/eoti/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

A standalone, **structural type inferencer and pre-run type checker for EO**, operating entirely on **XMIR** (EO's φ-calculus XML intermediate representation).

It reads the XMIR a program compiles to, infers a type for every object, and reports the mistakes that would otherwise only surface at runtime — for example, dispatching `.plus` on a `string`. It is deliberately **decoupled from any compiler**: it consumes only XMIR, so the same checker works no matter what language the EO compiler or runtime is written in.

> **Status:** working research prototype. It type-checks the entire `eo-runtime` (153/153 top-level objects, 0 rejected) in ~0.7s, passes its 13 accept/reject examples, and resolves every non-primitive atom return type to a real object shape. It is **not yet** wired into a build as a gate, and it emits a human-readable report rather than the machine format described in §5. Those are the roadmap (§13).
>
> **Language:** the production implementation is **Rust**, in `src/`, being ported module by module under the guard of `conformance/` — the engine is scaffolded but not yet wired. The **working checker today is the Python reference** in `reference/` (zero-dependency, readable — it doubles as the executable spec), which also stays on as the conformance oracle. Where the two disagree, the Python is right until `conformance/` says otherwise. See §13.

---

## Table of contents

1. [Why this exists (the vision)](#1-why-this-exists-the-vision)
2. [Quick start](#2-quick-start)
3. [Input: XMIR](#3-input-xmir)
4. [How to generate XMIR from an EO checkout](#4-how-to-generate-xmir-from-an-eo-checkout)
5. [Output](#5-output)
6. [How it works (the type system)](#6-how-it-works-the-type-system)
7. [Code map](#7-code-map)
8. [Type annotations it reads (EO #5741)](#8-type-annotations-it-reads-eo-5741)
9. [`@loc` and forma resolution (EO #5742)](#9-loc-and-forma-resolution-eo-5742)
10. [Extending & maintaining](#10-extending--maintaining)
11. [Testing & conformance](#11-testing--conformance)
12. [Design decisions & rationale (settled — do not re-litigate)](#12-design-decisions--rationale-settled--do-not-re-litigate)
13. [Roadmap](#13-roadmap)
14. [Provenance & relationship to EO](#14-provenance--relationship-to-eo)
15. [Limitations & caveats](#15-limitations--caveats)
16. [Repository layout](#16-repository-layout)
17. [License](#17-license)

---

## 1. Why this exists (the vision)

EO's compiler and runtime are Java today, but may be reimplemented in other languages (e.g. a JS compiler, a Rust runtime). The **one persistent, language-neutral artifact is XMIR**. The design goal is therefore:

- **Type checking is done once, over XMIR, by a tool that is not part of any compiler.** Switching the compiler's language changes nothing about type checking, because no compiler re-implements it.
- **Catch compilation errors before running** (missing attributes, wrong argument types, un-recovered failures).
- **Feed an IDE** the same facts (hover, completion, error squiggles).

This tool is the reference implementation of that checker. The type *semantics* (see the paper, `docs/eo-type-inference.tex`) plus a conformance suite (§11) are the real language-neutral deliverable; the Python here is one implementation of them.

---

## 2. Quick start

**Requirements:** the reference needs **Python 3.8+ only**, with **no third-party dependencies** — standard library exclusively (`xml.etree`, `re`, `os`, `sys`, `itertools`). The Rust side needs a stable toolchain (edition 2024, MSRV 1.85).

```bash
# 1) Run the built-in examples (accept + reject cases). No XMIR needed.
python3 reference/eo_type_inference.py
#   -> "13/13 examples behaved as expected."

# 2) Type-check specific XMIR files (prints each top-level object's type or REJECT).
python3 reference/eo_type_inference.py path/to/foo.xmir path/to/bar.xmir

# 3) Diagnostic: which atoms are unmodelled (hit the lenient fallback)?
python3 reference/eo_type_inference.py --atoms path/to/*.xmir

# 4) Full report over a whole 1-parse tree (two passes -> Markdown report).
EO_HOME=/path/to/eo ./reference/run.sh
#   or pass them: ./reference/run.sh <1-parse-dir> <output-file>

# 5) Build and check the Rust side.
cargo test
cargo clippy --all-targets -- -D warnings
```

Environment flag:

- `EO_INCOMPLETE=1` — enable the **object-level fragility** pass (flags dispatching on an object that still has an unset attribute; see §6 and §12). Off by default so the shape baseline stays clean.

---

## 3. Input: XMIR

The checker reads EO's **`1-parse`** XMIR — the output of the parser stage, before assembly/transpilation. Each program is an XML document whose objects are `<o>` elements. The reader (`load_xmir` → `convert`) understands this vocabulary:

| XMIR construct | Meaning to the checker |
| --- | --- |
| `<o name="x">…children…</o>` | A **formation** (an object). Children are its attributes. |
| `base="∅"` | A **void** (an unset parameter/attribute). |
| `name="φ"` or `name="@"` | The **decoratee** (`@`) — width-subtyping / "usable-as". |
| `base=".m"` | A **dispatch** of attribute `m` on the receiver (first child). |
| `base="Φ.x"` | A reference to a global/top-level object `x`. |
| `base="ξ"`, `base="ξ.ρ…"` | Self / parent-relative reference (`ρ` = parent link, `φ` = decoratee). |
| `as="αN"` | An **application argument** (fills the next void). |
| `atom="…"` on a `name="λ"` child | A **native atom** with a declared **return type** (see §8). |
| `type="…"` on a void | The void's declared **own type** (see §8). |
| `args="…"` on a void | The void's declared **callback argument types** (see §8). |
| `loc="Φ.…"` | The object's stable **graph locator** / identity (see §9). |
| `line`, `pos` | Source position (used for diagnostics placement). |

Raw data leaves (a `<o>` with text and no children) are `bytes`.

---

## 4. How to generate XMIR from an EO checkout

This tool does not contain EO. Point it at an EO checkout's parser output.

The straightforward `mvn … compile` triggers a canonical-format check (`MjFormat`) that gates the lifecycle and suggests `-Deo.autoFix`, which **rewrites `.eo` sources** — do not use it. Instead, build/install the plugin and invoke the parse goals directly, bypassing the lifecycle gate:

```bash
cd /path/to/eo                      # an objectionary/eo checkout

# Build & install the maven plugin (also builds eo-parser). Version is 1.0-SNAPSHOT.
mvn -q -pl eo-maven-plugin -am install -DskipTests

# Regenerate the runtime's 1-parse XMIR (register + parse only).
rm -rf eo-runtime/target/eo/1-parse
mvn -q -pl eo-runtime \
  org.eolang:eo-maven-plugin:1.0-SNAPSHOT:register \
  org.eolang:eo-maven-plugin:1.0-SNAPSHOT:parse

# Now point the checker at it:
python3 /path/to/eoti/reference/eo_type_inference.py eo-runtime/target/eo/1-parse/**/*.xmir
```

Notes:
- The parser reads the **working tree**, so whatever `.eo` you have checked out is what gets typed.
- If Maven reports a "cached not found" resolution error, purge `~/.m2/repository/org/eolang/eo-maven-plugin/<ver>/*.lastUpdated` and re-run with `-U`.
- `@loc` and the `#5741` type annotations require a **recent** EO (post July 2026). On an older checkout, `@loc` is absent and forma resolution (§9) silently falls back to opaque.

---

## 5. Output

### 5a. Current output (implemented)

- **stdout, per file:** a `== <path> ==` header, then one line per top-level object: `name : <type>` or `name : REJECT -- <reason>`.
- **`reference/run.sh`:** a Markdown report — summary table (objects / typed OK / rejected / incompleteness smells), grouped rejection reasons, the object-level smell list, and the full per-object listing.

Types are rendered in **named-binder** form: variables named once, single-use ones inlined, the rest in a trailing `where` clause, open variables `∀`-quantified. Concrete positions are shown by their object name (e.g. `number`); positions resolved via `@loc` show the object's shape.

### 5b. Target machine output (designed, not yet emitted — see §13)

Two outputs with different lifetimes:

**Diagnostics** — transient results, for gating and the IDE. A JSON document shaped like an LSP `Diagnostic` (so an editor's language server can pass it through), extended with `@loc`:

```json
{
  "schema": "eo-type-diagnostics/1",
  "source": "app/target/eo/1-parse",
  "status": "errors",
  "summary": { "errors": 1, "warnings": 1, "objects": 154 },
  "diagnostics": [
    {
      "severity": "error",
      "code": "type/unknown-attribute",
      "loc": "Φ.app.greeting.φ",
      "range": { "line": 12, "pos": 4 },
      "message": "no attribute `plus` on `string`",
      "detail": { "attribute": "plus", "receiver": "Φ.string" }
    }
  ]
}
```

- `status` — the compiler gates on this one field (`errors` → stop before codegen; `ok` → proceed).
- `severity` — `error` gates; `warning`/`info`/`hint` do not.
- `code` — a stable machine string; the tool's real API. Starter taxonomy: `type/unknown-attribute`, `type/argument-mismatch`, `type/not-recovered`, `type/incomplete-dispatch`, `type/unsatisfiable-parameter`, `ref/dangling-forma`.
- `loc` — the `@loc`: stable across reformatting, links to type facts and enables IDE caching.
- `range` — `line`/`pos` for placement and the `[L:P]` + caret rendering (done by the consumer).
- `message` — one sentence with context, no trailing period.
- `detail` — structured payload so consumers need not parse the prose.

**Type facts** — persistent, written back **into XMIR** (not a new format): a `type="…"` attribute on `<o>`, valued in the `#5741` vocabulary (an `@loc` pointer, a generic `A–F`, a primitive, an optional `?`). Produced only when a consumer (IDE, cross-package check) needs types. See §12 for why this is not a separate XML dialect.

**Compiler contract (any language):**

```
run inferencer over 1-parse XMIR -> diagnostics.json
read status:
   "errors" -> render diagnostics (from range + message), fail the build
   "ok"     -> proceed to codegen on the same XMIR
```

---

## 6. How it works (the type system)

The full design is in `docs/eo-type-inference.tex`. In brief:

- **Structural subtyping** in the MLsub / Simple-sub / algebraic-subtyping tradition. A type is a *shape* (a set of attributes), not a name.
- **`bytes` is the one base type.** Everything else is an object shape (`Rec`). `@` is the decoratee and models width subtyping ("usable-as").
- **Four moves:** *make* (a formation), *apply* (fill the next void), *dispatch* (require an attribute), *decorate* (`@`, fall through to the decoratee).
- **Constraint solver** — `constrain(lhs, rhs)` means "`lhs` must be usable as `rhs`". Each unknown (`Var`) collects lower bounds (things that flow in) and upper bounds (slots it flows out to); a `seen` set makes cyclic/recursive objects terminate.
- **Levels (MLsub)** — each `Var` has a `level`. Formations **generalize** at their definition level and each use **instantiates** via `freshen`; `constrain` performs **extrusion** (lowering) when a deep variable meets a shallower one. This is what makes a reused object's *parameters* fresh per use while its *recursion and context* stay shared.
- **Options / `⊥`** — `T?` (`Opt`) is a maybe-bottom value. The **option discipline**: plain dispatch on a `T?` is rejected; you must `recovered value alternative` (the `⊥`-eliminator: `∀A. A? → A → A`) or use `?.` (optional chaining, `FragileDispatch`). Chains stay `T?` until recovered.
- **Incompleteness as fragility** (behind `EO_INCOMPLETE=1`) — an object that still has an unset void is treated as fragile; dispatching on it is flagged. This is the object-level rule (see §12).
- **Generics** — `A–F` type variables, read from `#5741` annotations (§8).
- **Forma resolution** — a non-primitive atom return type is resolved to the *actual object's shape* via `@loc` (§9).

### Key behaviors, by example

```
"hi".plus 1                       -> REJECT: no attribute `plus` on `string`
42.plus 1                         -> ok, : number
recovered (fragile number) 7      -> ok, : number
(fragile number).plus 1           -> REJECT: value can be ⊥; recover it first

# a parameter used two ways accumulates BOTH requirements (an intersection):
[x] with x.plus 1 and x.length
  definition : ∀A B. ({plus:(number->A)} & {length:B}) -> {…}    # accepted, no error
  applied to a number : REJECT (number lacks .length)
  applied to a string : REJECT (string lacks .plus)
```

The last case is the crux of structural inference: a requirement is **accumulated at the definition and checked at each call site** — never guessed. (No known object has both `.plus` and `.length`; that this parameter is therefore dead-on-arrival is exactly what a `type/unsatisfiable-parameter` *warning* would surface early — see §12.)

---

## 7. Code map

The reference is a single file, `reference/eo_type_inference.py` (~1100 lines); the Rust port splits the same pieces across `src/` (§16). Reading order:

- **Types** — `Var` (unknown, with `level`/bounds/`rec`), `Prim` (`bytes`), `Fun`, `Rec` (shape: `fields` + ordered `voids` + `alias`), `Opt` (`T?`). `Clash` is the type-error exception.
- **Solver** — `constrain` (the heart; extrusion lives here), `freshen` (instantiate: copy variables deeper than a limit), `_level_of`, `_host` (the record behind a receiver), `_peek` (see past single-lower-bound variables), `_need`/`_drop_void` (caches that make cyclic `@`/recursive application terminate), `_defs_first` (order defs before expressions so forward references resolve).
- **Walker** — `infer(node, env)`: one branch per AST node kind.
- **AST nodes** — `Name`, `NumLit`/`StrLit`/`BytesLit`/`BoolLit`/`TupleLit`, `Obj` (no-void formation), `Lam` (formation with voids), `Apply`, `Dispatch`, `FragileDispatch` (`?.`), `Fragile`, `Recovered`, `AtomSig` (a `#5741`-annotated atom, §8), `LocRef` (a forma resolved by `@loc`, §9).
- **Built-in types** — `bytes_type`/`number_type`/`string_type`/`bool_value`/`tuple_type`/`stdout_type`/`recovered_type`; `_prims` builds the three mutually-recursive base objects. `ATOMS` maps a handful of names to built-in signatures; `_PRIM`/`_TYPES` map primitive names.
- **Annotation reading** — `_generic`, `_spec` (one annotation → a type), `_atom_sig` (an atom's whole signature → a `Rec`).
- **Frontend** — `_forma`, `_formation`, `_scope`, `convert`, `load_xmir` (XMIR → AST); `build_loc_index`, `_toplevel_of`, `_project`, `_resolve_loc` (the `@loc` machinery, §9).
- **Renderer** — `show(ty)`: the named-binder pretty-printer.
- **Entry points** — `run_examples`, `check_file`, `main`.

Globals worth knowing: `_LEVEL` (current MLsub level), `_DEFINING` (records mid-inference, for monomorphic recursion), `LENIENT` (file mode: unknown names → fresh unknown), `INCOMPLETE_FRAGILE` (the `EO_INCOMPLETE` flag), `LOC_INDEX`/`LOC_TOPLEVEL`/`_LOC_CACHE`/`_LOC_INPROGRESS` (forma resolution).

---

## 8. Type annotations it reads (EO #5741)

EO atoms may declare types. In `.eo`:

```
[] > recovered /A          # generic return type variable A–F
  ? > value /A?            #   a typed void: own type A, optional (maybe-⊥)
  ? > alternative /A       #   a typed void: own type A

[x] > div /Q.i64           # concrete return forma
  ? > cant-divide /{Q.string}   # a callback void; its argument types
```

In XMIR these become attributes: the return signature on the `name="λ"` child as `atom="…"`; a void's own type as `type="…"` (trailing `?` = maybe-⊥); a callback void's argument types as `args="…"`. Values may be a generic letter `A–F`, `⊥`, a primitive (`Φ.bytes`), or any Φ-rooted path.

The checker builds an atom's type straight from these (`_atom_sig`): generics `A–F` are shared across the whole atom via one map (so `recovered`'s `value:A?`, `alternative:A`, and return `A` are the *same* `A`), `?` becomes an `Opt`, a callback becomes a function over its argument types, a primitive becomes its built-in type, and any other path is resolved via `@loc` (§9).

Result: `recovered` infers `∀A. (A? -> (A -> A))` **from the XMIR** — no hard-coded signature.

> These same annotations are also harvested, independently, into `atoms.csv` by EO's `MjAtomsTable` for a **runtime** return-type check (`AtomTyped`, opt-in via `-Deo.typing`). That is a *separate* mechanism (see §12/§14); this checker does not read `atoms.csv`.

---

## 9. `@loc` and forma resolution (EO #5742)

Every `<o>` carries `loc="Φ.…"`, a stable graph path identifying the object from the root `Φ`. It survives reformatting (unlike `line`/`pos`), and — crucially — it lives in the **same namespace as formas**: a return type like `Φ.posix.return` is *also* a locator into the graph.

The checker exploits this to resolve non-primitive return types to real shapes (`_resolve_loc`):

1. `build_loc_index` indexes every `<o>` by `@loc`, and records top-level objects separately.
2. For a forma `Φ.a.b.c`, `_toplevel_of` finds the longest top-level ancestor (`Φ.a`) plus the path down (`b.c`).
3. It infers the ancestor (so the nested object's `ξ.ρ…` parent chain resolves), then `_project`s down the path.
4. The result is inferred **once** (memoized), generalizable, and **instantiated per use with `freshen`** (no re-inference blow-up, no cross-use over-constraint). Self-reference is cycle-guarded. It is **best-effort**: anything that cannot be typed in isolation falls back to opaque, so resolution never turns a passing check into a spurious failure.

Effect (current runtime): `posix.return`/`win32.return` → `{code, output, called, @}`, `tuple` → `{tail, head, length, at, eq, with, empty}`, `chunk`, `i64`, `regex.pattern`, `regex.matched` all resolve — where before they were opaque "accepts anything".

`@loc` also enables a **dangling-forma lint** with no inference at all: check that every forma (`atom=`/`type=`/`args=` that is a Φ-path) names an existing `@loc`. (This would have caught EO bug #5785, where `posix.@`/`win32.@` declared `/Q.return` — a root object that did not exist.)

---

## 10. Extending & maintaining

**Most atoms need no maintenance** — their return types are read from XMIR (§8/§9). You touch the hand-modeled tables only for the small set of *structurally* modeled built-ins.

- **`_prims`** — the three mutually-recursive base objects (`bytes`, `number`, a `bool` value). Their full method shapes are hand-written because they are the ground truth `bytes` decorates.
- **`ATOMS`** — a few polymorphic/special built-ins that cannot be read from a single forma: `recovered`, `dataized` (`∀A. A → bytes`), `stdout`, plus the base names.
- **`_TYPES`/`_PRIM`** — name → built-in constructor maps.

When the runtime grows and `--atoms` shows a new unmodelled primitive, add its shape here (or, better, ensure it carries a `#5741` annotation so no hand-editing is needed). Historically this was the only recurring upkeep (e.g. `string.printf`, `bytes.as-u8…as-u64`, `tuple.contains`, `number.power`).

**Golden rule:** prefer teaching EO to *declare* the type (`#5741`) over hard-coding it here. The hand table should shrink over time, not grow.

---

## 11. Testing & conformance

- **Examples** — `python3 reference/eo_type_inference.py` runs 13 hand-built accept/reject cases (including the ones that must be rejected and a recursive object that once made the solver loop). These are the minimal behavioral spec.
- **Conformance suite** — the whole `eo-runtime` is the large test: it must type **153/153, 0 rejected** (whatever the current object count is), in well under a second. Any implementation in any language must produce identical verdicts on the examples and the runtime.
- **The frozen contract** — `conformance/examples.json` records both: every example with the regression it guards, plus the whole-runtime verdict. It was frozen before the port began (§13's migration discipline) and `tests/conformance.rs` runs it. A new behavior is a row there first, then an implementation change.

Recommended CI: regenerate `1-parse` (§4), run the examples, run the full report, and assert `rejected == 0` and `examples == all`.

---

## 12. Design decisions & rationale (settled — do not re-litigate)

- **Structural, not nominal.** Types are shapes. EO is **open-world** (user programs define new objects), so a requirement like `{plus:…}` means "*anything* with `.plus`", and must stay a variable — exactly as Hindley-Milner keeps `id : ∀a. a → a` and never rewrites it to `int → int`.
- **No "discovery" substitution.** Replacing a generic variable with the catalog objects that match its attributes is **unsound** in an open world (it would reject a future object that also matches) and lossy (it destroys the polymorphism that makes objects reusable). The *only* sound use of attribute-subset matching is a **warning** ("no known object satisfies this parameter; did you mean…") — never a type rewrite.
- **Types travel with XMIR as `@loc` pointers, not a new format.** Because a type is usually an object's shape and every object already has an `@loc`, a type is usually just a pointer — expressible in the existing `#5741` vocabulary as a string attribute. Do **not** invent a parallel XML dialect for types; if a standalone artifact is ever needed, use JSON, but prefer enriching XMIR in place.
- **The checker is a decoupled, language-neutral gate**, not logic inside a compiler. Its output contract (§5b) is the language-neutral part; the tool's own implementation language is incidental.
- **`atoms.csv` / `AtomTyped` is a separate concern.** That table feeds a *runtime* return-type assertion (opt-in `-Deo.typing`); it holds only return types, skips generics, and is off by default. This static checker neither needs nor uses it. Do not conflate them.

### The four MLsub subtleties (regressions to never reintroduce)

These were each a real bug; keep them in mind before touching the solver:

1. A formation's recursion variable must live at the **body level** (`outer+1`), not the definition level. Otherwise tying the knot extrudes/de-generalizes its parameters and the checker misses real errors (unsoundness).
2. Atoms and literals must be built at **level 0** (`_at0`). Otherwise extrusion storms (observed ~100k calls).
3. `_level_of` must **skip `ρ`** (the parent link) consistently with `freshen`/extrusion — else a record can never be lowered below a deep parent and `constrain` spins forever.
4. `freshen` shares `ρ` and aliased primitives (does not copy them); resolution to a return *value* returns the object's **instance**, not its fillable formation.

---

## 13. Roadmap

**Next step (smallest, highest value):** make it a build gate.
1. Emit the diagnostics JSON of §5b (the tool already computes every field but `code`/`detail`).
2. Run it as a post-parse stage that fails on `status: "errors"`. Safe to enable: silent on correct code (153/153), lenient on anything unmodeled, so it never blocks a valid build.
3. Ship the **dangling-forma lint** (§9) as the first zero-inference check.
4. Validate end-to-end on a deliberately broken `.eo` (e.g. `"hi".plus 1`).

**Later, each with a trigger:**
- **Union types `|`** — so `if`/`switch`/`tuple.at` can be *declared* instead of abstaining. (Currently the honest gap: they carry no annotation.)
- **Typed-XMIR facts** (`type=` pointers on `<o>`) — build when an IDE or cross-package check actually reads types.
- **IDE / LSP** — the diagnostics JSON is already LSP-shaped; add a server.
- **The Rust rewrite** — the production implementation in the standalone repo (see below). This Python stays as the reference/oracle.
- **A formal spec + conformance harness** — the guard for the rewrite (the 13 examples + the whole-runtime verdicts); write it before porting.
- **Discovery as an unsatisfiability *warning*** — the `{plus}&{length}` case (§6); never as a rewrite.

### The Rust rewrite (production implementation)

This repository's implementation is **Rust**; the Python remains the reference implementation (the executable spec) and the conformance oracle. The rationale is *not* about raw speed — Python's ~0.7s over the whole runtime is already fine for a gate, and a responsive IDE needs *incrementality* (re-check a changed `@loc` and its dependents), not a faster language. It's about deployment and correctness:

- **Deploys anywhere.** A single static binary any pipeline can invoke — a Java, JS, or Rust EO toolchain alike — with no runtime dependency and near-instant startup (matters for a gate run per build, and for a responsive language server). Python needs an interpreter present and cannot embed in a JS/Rust process.
- **Right fit for the algorithm.** The types and the AST are sum types; Rust's `enum` + exhaustive `match` model them directly and turn whole classes of solver mistakes into compile errors — including the four MLsub subtleties in §12, each of which was a *silent* bug in dynamically-typed Python.
- **IDE-ready.** Fast enough for an incremental LSP server, and the diagnostics JSON of §5b is already LSP-shaped.

**Migration discipline (do not skip):** write the conformance suite first, then port under its guard so the Rust implementation is proven to agree with the Python **object-for-object** on the 13 examples and the whole runtime. That turns the rewrite from a risky redo into a checked migration — and keeps the Python as an oracle a third implementation could later be checked against too.

---

## 14. Provenance & relationship to EO

Built against `objectionary/eo`. The pieces it depends on, all merged upstream:

- **#5741** — generic type variables and typed void annotations for atoms (`atom=`/`type=`/`args=`, generics `A–F`). Retyped `recovered` from a false `/bytes` to `∀A. A? → A → A`.
- **#5742** — emit the graph locator `@loc` on every `<o>` at parse time (moved `set-locators.xsl` into the parse pipeline).
- **#5785** — fixed a dangling return forma (`posix.@`/`win32.@` declared `/Q.return`, a root object that did not exist; now `/Q.posix.return` / `/Q.win32.return`). Found by this checker; would be caught automatically by the dangling-forma lint.

Related but **separate**: `MjAtomsTable` generates `atoms.csv` (a locator→return-type table) for the runtime `AtomTyped` check. See §12.

Design paper: `docs/eo-type-inference.tex` (build with `pdflatex`; note Unicode like `ρ` must be `$\rho$`, not inside `\verb`).

---

## 15. Limitations & caveats

- **Prototype, whole-program, Python.** It infers the whole input each run (fast enough — <1s for the runtime — but not incremental) and reads the working-tree XMIR.
- **No union types yet.** Genuinely polymorphic atoms whose type needs `A|B` (`if`, `switch`, `tuple.at`) carry no annotation and stay as the checker infers them.
- **Lenient on the unknown.** An unmodeled atom/name becomes a fresh unknown ("anything"): the checker never emits a *false* error, but it can *miss* one. `--atoms` shows what is unmodeled.
- **Best-effort forma resolution.** An object that cannot be typed in isolation falls back to opaque rather than failing (see §9).
- **Fragility features are partly flag-gated.** `?.`, the option discipline, and object-level incompleteness are implemented, but the incompleteness pass is behind `EO_INCOMPLETE` so the 153/153 shape baseline stays clean; `?.` is not yet EO surface syntax.

---

## 16. Repository layout

```
eoti/
├── README.md                 # this file — the spec and the map
├── CONTRIBUTING.md           # how to build, and the porting discipline
├── LICENSE                   # MIT (match EO)
├── Cargo.toml                # edition 2024, MSRV 1.85, warnings are errors
├── rust-toolchain.toml
├── src/                      # the Rust implementation
│   ├── main.rs               #   CLI: XMIR in -> diagnostics JSON (§5b)
│   ├── lib.rs                #   the crate root
│   ├── xmir.rs               #   XMIR reader (the §3 vocabulary)
│   ├── types.rs              #   Var / Prim / Fun / Rec / Opt as enums
│   ├── solver.rs             #   constrain / freshen / levels (mind §12's four subtleties)
│   ├── infer.rs              #   the walker
│   ├── resolve.rs            #   @loc forma resolution (§9)
│   ├── render.rs             #   the named-binder pretty-printer
│   └── diag.rs               #   the diagnostics document (§5b)
├── reference/                # the Python reference implementation (executable spec + oracle)
│   ├── eo_type_inference.py
│   └── run.sh                #   whole-tree report; needs $EO_HOME or a path
├── conformance/              # the frozen contract both implementations answer to
│   ├── README.md
│   ├── examples.json         #   13 examples + the whole-runtime verdict
│   └── fixtures/             #   small .xmir programs (testable without an EO checkout)
├── docs/
│   ├── eo-type-inference.tex #   the design paper
│   ├── eo-type-inference.pdf
│   └── gaps.md
└── tests/                    # integration tests that run the conformance suite
```

The fixtures sit under `conformance/` rather than a top-level `examples/` because in a Cargo project that directory means runnable Rust examples.

Order of work, and where it stands:

1. ~~**Drop the Python in under `reference/`**~~ — done; it runs as-is (no build, no dependencies), so there is a working checker on day one.
2. ~~**Freeze the conformance suite**~~ — done; `conformance/examples.json` is the contract the Rust port must satisfy, recorded before the port began.
3. **Add XMIR fixtures** under `conformance/fixtures/` (a handful of small programs + their expected verdicts) so the repo is testable without an EO checkout; keep the EO checkout only for full-runtime conformance.
4. **Build the Rust implementation** in `src/`, one module at a time, with `tests/` asserting it matches the reference **object-for-object** on the suite. Retire the Python from the gating path only once Rust passes; keep it as the oracle.

---

## 17. License

MIT, to match `objectionary/eo`.
