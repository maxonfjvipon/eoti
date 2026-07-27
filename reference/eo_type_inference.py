  #!/usr/bin/env python3
"""
Minimal type-inference prototype for EO.

It implements, in the smallest honest form, the design written up in
`eo-type-inference.tex`:

  * the four-move core          -- make / apply / dispatch / decorate (`@`)
  * the spine (Section 7)       -- constraint solving with flows-in / flows-out
                                   bounds on each unknown (Simple-sub style),
                                   plus a `seen` set so recursion terminates
  * bytes as the ONE base type  -- everything else is an object shape
  * the option (`T?`) discipline -- a possibly-bottom value cannot be dispatched
                                   on; `recovered` strips the bottom

Two ways to run it:
  * with no arguments it type-checks a handful of hand-built examples --
    including the ones that MUST be rejected and the recursive object that
    used to make the checker loop forever;
  * given .xmir files it reads EO's real current parser output (the kind in
    eo-runtime/target/eo/1-parse) and types each top-level object.

    $ python3 eo-type-inference-prototype.py                  # the examples
    $ python3 eo-type-inference-prototype.py foo.xmir bar.xmir
"""

import os
import re
import sys
import xml.etree.ElementTree as ET
from itertools import count

sys.setrecursionlimit(15000)   # some real objects (fs/path) nest very deeply

# ===========================================================================
# Types -- the language of "shapes"
# ===========================================================================

_ids = count()
_LEVEL = [0]           # current let-nesting level (MLsub); deeper == more local
_DEFINING = set()      # ids of formation records currently being inferred


class Var:
    """An unknown. Collects flows-in (lower bounds) and flows-out (upper). Its
    `level` (MLsub) records how local it is: a variable is generalizable at a use
    site only when its level is deeper than that site's, so a reused object's
    parameters become fresh per use while its recursion and context stay shared."""

    def __init__(self, hint="t", level=None):
        self.id = next(_ids)
        self.hint = hint
        self.alias = None      # a display name for built-in atoms (number, ...)
        self.lower = []        # things that flow INTO this unknown
        self.upper = []        # slots this unknown flows OUT to
        self.level = _LEVEL[0] if level is None else level
        self.rec = None        # if this var IS an object: its (in-progress) record


class Prim:
    """A base type. In EO the only inhabitant is `bytes`."""

    def __init__(self, name):
        self.name = name


class Fun:
    """Fill the parameter, get the result. (A formation with a void attribute.)"""

    def __init__(self, arg, res):
        self.arg = arg
        self.res = res


class Rec:
    """An object shape: attribute name -> type. `@` is the decoratee."""

    def __init__(self, fields, alias=None, voids=None):
        self.fields = fields   # dict[str, type]
        self.alias = alias     # display name for a built-in atom (number, bool, ...)
        self.voids = voids or []  # ordered void params still fillable by application


class Opt:
    """T?  ==  T or bottom. Made by fragile dispatch; removed by `recovered`."""

    def __init__(self, inner):
        self.inner = inner


LENIENT = False        # in file mode, unknown names become fresh unknowns (see check_file)
_UNMODELLED = {}       # in file mode: how many times each unmodelled atom was hit
INCOMPLETE_FRAGILE = os.environ.get("EO_INCOMPLETE") == "1"   # object-with-unset-attr is fragile
LOC_INDEX = {}         # @loc graph path -> its <o> element (every object)
LOC_TOPLEVEL = {}      # @loc -> element, top-level objects only (resolution entry points)
_LOC_CACHE = {}        # @loc -> its resolved (generalizable) type, memoized once
_LOC_INPROGRESS = {}   # @loc being resolved -> its recursion knot (cycle guard)


class Clash(Exception):
    """A real type error: two shapes that cannot fit."""


# ===========================================================================
# The solver -- `constrain(lhs, rhs)` == "lhs must be usable as rhs"
# ===========================================================================

_NEED = {}


def _need(label, want, line=None):
    """A cached single-attribute requirement `{label: want}`. Caching is what lets
    a cyclic decoratee (`@`) chain terminate: the same requirement re-appears as the
    SAME object, so the `seen` set recognizes it instead of spinning on fresh copies."""
    key = (label, want)
    req = _NEED.get(key)
    if req is None:
        req = _NEED[key] = Rec({label: want})
    if line is not None:
        req.line = line
    return req


def _drop_void(rec):
    """The formation with its first void filled. Cached on the record, so a
    recursive application reuses the same object and `seen` can tie it off
    instead of spinning on fresh copies."""
    key = tuple(rec.voids[1:])
    cache = rec.__dict__.setdefault("_reduced", {})
    got = cache.get(key)
    if got is None:
        got = cache[key] = Rec(rec.fields, rec.alias, rec.voids[1:])
    return got


def _level_of(ty, seen=None):
    """The deepest level appearing in a type. A variable's own level is
    authoritative (constrain keeps it >= its bounds' levels); composites take the
    max over their parts. `seen` guards the rare non-variable cycle."""
    if isinstance(ty, Var):
        return ty.level
    if seen is None:
        seen = set()
    if id(ty) in seen:
        return 0
    seen.add(id(ty))
    if isinstance(ty, Fun):
        return max(_level_of(ty.arg, seen), _level_of(ty.res, seen))
    if isinstance(ty, Rec):
        # ρ (the parent link) is navigation, shared not copied by extrude/freshen,
        # so it must not count toward this object's level either -- else extrude
        # can never lower the record below a deep parent and constrain spins.
        return max([_level_of(v, seen) for k, v in ty.fields.items() if k != "ρ"],
                   default=0)
    if isinstance(ty, Opt):
        return _level_of(ty.inner, seen)
    return 0


def freshen(lim, ty, memo):
    """Instantiate: a fresh copy of `ty` in which every variable DEEPER than `lim`
    becomes a new variable (at the current level), copying its bounds. Variables at
    or above `lim` -- the definition's context and its own recursion -- are shared.
    This is MLsub's freshenAbove; it is what gives each use of a formation its own
    parameters without touching what must stay shared."""
    if getattr(ty, "alias", None) or _level_of(ty) <= lim:
        return ty
    if isinstance(ty, Var):
        if id(ty) in memo:
            return memo[id(ty)]
        nv = memo[id(ty)] = Var(ty.hint)
        nv.lower = [freshen(lim, x, memo) for x in ty.lower]
        nv.upper = [freshen(lim, x, memo) for x in ty.upper]
        return nv
    if isinstance(ty, Fun):
        return Fun(freshen(lim, ty.arg, memo), freshen(lim, ty.res, memo))
    if isinstance(ty, Rec):
        if id(ty) in memo:
            return memo[id(ty)]
        fresh = memo[id(ty)] = Rec({}, ty.alias, list(ty.voids))
        if hasattr(ty, "line"):
            fresh.line = ty.line
        if hasattr(ty, "deflevel"):
            fresh.deflevel = ty.deflevel
        for label, sub in ty.fields.items():
            fresh.fields[label] = sub if label == "ρ" else freshen(lim, sub, memo)
        return fresh
    if isinstance(ty, Opt):
        return Opt(freshen(lim, ty.inner, memo))
    return ty


def extrude(ty, lvl, pol, memo):
    """Lower every variable deeper than `lvl` to a fresh variable at `lvl`, linked
    to the original so nothing generalizes that has escaped to the shallower side.
    `pol` is the polarity (which side's bounds to carry). MLsub's extrude, called by
    constrain when the two sides sit at different levels."""
    if _level_of(ty) <= lvl:
        return ty
    if isinstance(ty, Var):
        key = (id(ty), pol)
        if key in memo:
            return memo[key]
        nv = memo[key] = Var(ty.hint, lvl)
        if pol:
            ty.upper.append(nv)
            nv.lower = [extrude(x, lvl, pol, memo) for x in ty.lower]
        else:
            ty.lower.append(nv)
            nv.upper = [extrude(x, lvl, pol, memo) for x in ty.upper]
        return nv
    if isinstance(ty, Fun):
        return Fun(extrude(ty.arg, lvl, not pol, memo), extrude(ty.res, lvl, pol, memo))
    if isinstance(ty, Rec):
        key = (id(ty), pol)
        if key in memo:
            return memo[key]
        fresh = memo[key] = Rec({}, ty.alias, list(ty.voids))
        if hasattr(ty, "line"):
            fresh.line = ty.line
        if hasattr(ty, "deflevel"):
            fresh.deflevel = ty.deflevel
        for label, sub in ty.fields.items():
            fresh.fields[label] = sub if label == "ρ" else extrude(sub, lvl, pol, memo)
        return fresh
    if isinstance(ty, Opt):
        return Opt(extrude(ty.inner, lvl, pol, memo))
    return ty


def constrain(lhs, rhs, seen=None):
    if seen is None:
        seen = set()
    key = (id(lhs), id(rhs))
    if key in seen:                       # tie-the-knot: already handling this pair
        return
    seen.add(key)

    # --- the option discipline -------------------------------------------
    if isinstance(lhs, Opt) and isinstance(rhs, Opt):
        return constrain(lhs.inner, rhs.inner, seen)
    if isinstance(lhs, Opt) and isinstance(rhs, Fun) \
            and isinstance(lhs.inner, Rec) and lhs.inner.voids:
        return constrain(lhs.inner, rhs, seen)   # completing an incomplete object is allowed
    if isinstance(lhs, Opt) and not isinstance(rhs, Var):   # meets a definite requirement
        inner = lhs.inner
        if isinstance(inner, Rec) and inner.voids and isinstance(rhs, Rec) and rhs.fields:
            raise Clash(
                "incomplete object (attribute `%s` not set): cannot dispatch `.%s` on it "
                "(at line %s) -- use ?. or recover"
                % (inner.voids[0], next(iter(rhs.fields)), getattr(rhs, "line", "?"))
            )
        raise Clash("value can be ⊥ (bottom); recover it first")  # (into a Var it just flows)
    if isinstance(rhs, Opt):              # a definite value fits where a T? is wanted
        return constrain(lhs, rhs.inner, seen)

    # --- unknowns: record the bound and push it along, lowering levels so a
    #     variable that meets a deeper one gets extruded (never over-generalized) -
    if isinstance(lhs, Var) and _level_of(rhs) <= lhs.level:
        lhs.upper.append(rhs)
        for lb in list(lhs.lower):
            constrain(lb, rhs, seen)
        return
    if isinstance(rhs, Var) and _level_of(lhs) <= rhs.level:
        rhs.lower.append(lhs)
        for ub in list(rhs.upper):
            constrain(lhs, ub, seen)
        return
    if isinstance(lhs, Var):
        return constrain(lhs, extrude(rhs, lhs.level, False, {}), seen)
    if isinstance(rhs, Var):
        return constrain(extrude(lhs, rhs.level, True, {}), rhs, seen)

    # --- functions: results the same way, arguments the OPPOSITE way -----
    if isinstance(lhs, Fun) and isinstance(rhs, Fun):
        constrain(rhs.arg, lhs.arg, seen)     # contravariant argument
        constrain(lhs.res, rhs.res, seen)     # covariant result
        return

    # --- a formation is usable as a function: applying it fills its next void,
    #     giving back the same formation with that void now bound --------------
    if isinstance(lhs, Rec) and isinstance(rhs, Fun):
        if lhs.voids:
            head = lhs.voids[0]
            constrain(rhs.arg, lhs.fields[head], seen)                    # arg fills the void
            reduced = _drop_void(lhs)
            if reduced.voids and INCOMPLETE_FRAGILE:                       # still unfilled voids
                constrain(Opt(reduced), rhs.res, seen)                    #   -> an incomplete, fragile value
            else:
                constrain(reduced, rhs.res, seen)
            return
        if "@" in lhs.fields:                                             # over-applied: via @
            constrain(lhs.fields["@"], rhs, seen)
            return

    # --- object shapes: one note per attribute the rhs asks for ----------
    if isinstance(lhs, Rec) and isinstance(rhs, Rec):
        for label, want in rhs.fields.items():
            if label in lhs.fields:
                constrain(lhs.fields[label], want, seen)
            elif "@" in lhs.fields:                       # fall through the decoratee
                constrain(lhs.fields["@"], _need(label, want, getattr(rhs, "line", None)), seen)
            else:
                raise Clash("no attribute `%s` on %s (at line %s)"
                            % (label, show(lhs), getattr(rhs, "line", "?")))
        return

    # --- base type ------------------------------------------------------
    if isinstance(lhs, Prim) and isinstance(rhs, Prim) and lhs.name == rhs.name:
        return

    # --- everything else is a clash -------------------------------------
    if isinstance(rhs, Rec):
        want = ", ".join(rhs.fields)
        raise Clash("%s has no attributes (wanted %s)" % (show(lhs), want))
    raise Clash("cannot use %s as %s" % (show(lhs), show(rhs)))


# ===========================================================================
# The AST -- the four moves, plus literals and the two failure forms
# ===========================================================================

class Name:
    def __init__(self, x): self.x = x


class NumLit:
    def __init__(self, n): self.n = n


class StrLit:
    def __init__(self, s): self.s = s


class BytesLit:
    pass


class BoolLit:
    pass


class TupleLit:
    pass


class Obj:
    """No-parameter formation (an object): binds label -> expression, incl. `@`."""
    def __init__(self, name, binds): self.name = name; self.binds = binds


class Lam:
    """Parameter formation (a function): fill params, behave as `body` (its `@`).
    `binds` are its other named attributes -- inferred so their bodies get checked."""
    def __init__(self, name, params, body, binds=None):
        self.name = name; self.params = params; self.body = body; self.binds = binds or {}


class Apply:
    def __init__(self, fn, arg): self.fn = fn; self.arg = arg


class Dispatch:
    def __init__(self, obj, label, line=None): self.obj = obj; self.label = label; self.line = line


class FragileDispatch:
    """x?.m -- dispatch on a maybe-⊥ receiver: if the receiver can be ⊥, propagate
    that (the result is maybe-⊥ too); otherwise dispatch as usual. Chains compose,
    staying maybe-⊥, until `recovered` ends the chain with a definite value."""
    def __init__(self, obj, label, line=None): self.obj = obj; self.label = label; self.line = line


class Fragile:
    """Marks an expression whose value may be a bottom -- gives it type T?."""
    def __init__(self, inner): self.inner = inner


class Recovered:
    """recovered value alternative -- strips the bottom, unions the two."""
    def __init__(self, value, alt): self.value = value; self.alt = alt


class AtomSig:
    """A native atom typed purely by its declared signature (#5741): ordered
    voids -- each optionally annotated with an own type (`type=`) or callback
    argument types (`args=`) -- and a return type (`atom=`). Generic variables
    A-F are shared across them, so `value:A?`, `alternative:A` and return `A`
    name one and the same A. There is no body to infer; the shape IS the type."""
    def __init__(self, name, voids, ret):
        self.name = name; self.voids = voids; self.ret = ret


class LocRef:
    """A forma that names a non-primitive object by its Φ-rooted @loc path
    (`Φ.posix.return`, `Φ.tuple`). Resolved to that object's inferred shape via
    the locator index, rather than left an opaque unknown."""
    def __init__(self, loc): self.loc = loc


# ===========================================================================
# The walker -- `infer(node, env)` produces a type and piles up constraints
# ===========================================================================

def _at0(thunk):
    """Build a starting-fact type (an atom or a literal) at level 0 -- fully general,
    so it never needs extruding and stays shared across the site that uses it."""
    save = _LEVEL[0]
    _LEVEL[0] = 0
    try:
        return thunk()
    finally:
        _LEVEL[0] = save


def _host(ot):
    """The record backing a dispatch receiver, if we know it: the in-progress
    record of `self`/an ancestor, or a variable pinned to a single record."""
    if isinstance(ot, Rec):
        return ot
    if isinstance(ot, Var):
        if ot.rec is not None:
            return ot.rec
        if len(ot.lower) == 1 and isinstance(ot.lower[0], Rec):
            return ot.lower[0]
    return None


def _peek(ot):
    """See past single-valued variables to the concrete type they carry (an Opt, a
    record, ...) -- so `?.` can find a ⊥/incomplete value even behind the solver's
    generated variables."""
    seen = set()
    while isinstance(ot, Var) and id(ot) not in seen and len(ot.lower) == 1:
        seen.add(id(ot))
        ot = ot.lower[0]
    return ot


def _defs_first(binds):
    """Object-definition binds before expression binds, so a forward reference to
    a sibling object (used before it is written) still resolves to its record."""
    return sorted(binds.items(), key=lambda kv: 0 if isinstance(kv[1], (Obj, Lam)) else 1)


def infer(node, env):
    if isinstance(node, Name):
        if node.x in env:
            return env[node.x]
        if node.x in ATOMS:
            return _at0(ATOMS[node.x])            # fresh copy per use, fully general
        if LENIENT:
            _UNMODELLED[node.x] = _UNMODELLED.get(node.x, 0) + 1
            return _at0(lambda: Var(node.x))      # an unmodelled atom: treat as anything
        raise Clash("unbound name `%s`" % node.x)

    if isinstance(node, LocRef):
        tail = _tail(node.loc)
        if tail in env:
            return env[tail]                       # a self/sibling in scope (as a Name would)
        if tail in ATOMS:
            return _at0(ATOMS[tail])
        resolved = _resolve_loc(node.loc)          # else resolve the object by its @loc
        if resolved is not None:
            return resolved
        if LENIENT:
            _UNMODELLED[node.loc] = _UNMODELLED.get(node.loc, 0) + 1
            return _at0(lambda: Var(tail))
        raise Clash("unbound object `%s`" % node.loc)

    if isinstance(node, NumLit):
        return _at0(number_type)
    if isinstance(node, StrLit):
        return _at0(string_type)
    if isinstance(node, BytesLit):
        return _at0(bytes_type)
    if isinstance(node, BoolLit):
        return _at0(bool_value)
    if isinstance(node, TupleLit):
        return _at0(lambda: tuple_type(Var("elem")))
    if isinstance(node, AtomSig):
        return _at0(lambda: _atom_sig(node))       # a native atom's declared shape

    if isinstance(node, Obj):
        outer = _LEVEL[0]
        _LEVEL[0] = outer + 1                     # its own vars are one level deeper
        t = Var(node.name or "obj", outer + 1)    # the recursion var lives at body level
        env2 = dict(env)
        if node.name:
            env2[node.name] = t                   # register BEFORE descending
        parent = env.get("ξ")                     # the enclosing self, if any
        env2["ξ"] = t                             # ξ == this object
        rec = t.rec = Rec({})                     # filled as we descend
        rec.deflevel = outer                      # scheme level: generalize deeper vars
        if parent is not None:
            rec.fields["ρ"] = parent              # ρ == parent (for ξ.ρ... access)
        _DEFINING.add(id(rec))
        for label, sub in _defs_first(node.binds):
            rec.fields[label] = infer(sub, env2)  # a sibling is visible once written
        _DEFINING.discard(id(rec))
        _LEVEL[0] = outer
        constrain(rec, t)                         # tie the knot (same level, no extrusion)
        return t

    if isinstance(node, Lam):
        outer = _LEVEL[0]
        _LEVEL[0] = outer + 1
        t = Var(node.name or "fn", outer + 1)
        env2 = dict(env)
        if node.name:
            env2[node.name] = t                   # register BEFORE descending
        parent = env.get("ξ")                     # the enclosing self, if any
        env2["ξ"] = t                             # ξ == this object
        rec = t.rec = Rec({}, None, node.params)  # a record whose voids are fillable
        rec.deflevel = outer
        if parent is not None:
            rec.fields["ρ"] = parent              # ρ == parent (for ξ.ρ... access)
        _DEFINING.add(id(rec))
        for p in node.params:
            rec.fields[p] = env2[p] = Var(p)      # each void is a fillable field, one deeper
        for label, sub in _defs_first(node.binds):
            rec.fields[label] = infer(sub, env2)  # its other named attributes (checked)
        if node.body is not None:
            rec.fields["@"] = infer(node.body, env2)   # its decoratee, after siblings
        _DEFINING.discard(id(rec))
        _LEVEL[0] = outer
        constrain(rec, t)
        return t

    if isinstance(node, Apply):
        ft = infer(node.fn, env)
        r = Var("r")
        constrain(ft, Fun(infer(node.arg, env), r))
        return r

    if isinstance(node, Dispatch):
        ot = infer(node.obj, env)
        host = _host(ot)
        if host is not None and node.label in host.fields:
            field = host.fields[node.label]
            if node.label == "ρ":                             # navigate to the parent
                return field
            frec = _host(field)
            if frec is not None and frec.voids and id(frec) not in _DEFINING:
                lim = getattr(frec, "deflevel", 0)            # a formation: fresh per use
                inst = freshen(lim, frec, {})
                return Opt(inst) if INCOMPLETE_FRAGILE else inst   # incomplete -> fragile
        r = Var(node.label)
        req = Rec({node.label: r})
        req.line = node.line
        constrain(ot, req)
        return r

    if isinstance(node, FragileDispatch):
        ot = infer(node.obj, env)
        r = Var(node.label)
        req = Rec({node.label: r})
        req.line = node.line
        peeked = _peek(ot)                             # see past variables to the value
        if isinstance(peeked, Opt):                    # receiver can be ⊥ / is incomplete
            constrain(peeked.inner, req)               # dispatch on what's inside
            return Opt(r)                              # ... and the result can be ⊥ too
        constrain(ot, req)                             # definite receiver: no ⊥ to add
        return r

    if isinstance(node, Fragile):
        return Opt(infer(node.inner, env))

    if isinstance(node, Recovered):
        vt = infer(node.value, env)
        at = infer(node.alt, env)
        r = Var("recovered")
        constrain(vt.inner if isinstance(vt, Opt) else vt, r)   # value, bottom stripped
        constrain(at, r)                                        # or the alternative
        return r

    raise Clash("unknown node %r" % node)


# ===========================================================================
# The atoms -- built-in signatures, the "starting facts"
# ===========================================================================

def _prims():
    """Fresh copies of the three mutually-recursive built-ins -- `bytes` (the
    leaf object), `number`, and a `bool` value -- built together so their
    methods can name one another. `bytes` is the one object that decorates
    nothing; everything else bottoms out in it."""
    b = Var("bytes"); b.alias = "bytes"
    n = Var("number"); n.alias = "number"
    bl = Var("bool"); bl.alias = "bool"
    a1, a2, pick = Var("A"), Var("B"), Var("pick")
    constrain(a1, pick); constrain(a2, pick)                 # `if` result = A or B
    constrain(Rec({"eq": Fun(b, bl), "and": Fun(b, b), "or": Fun(b, b), "xor": Fun(b, b),
                   "not": b, "left": Fun(n, b), "right": Fun(n, b), "concat": Fun(b, b),
                   "slice": Fun(n, Fun(n, b)), "size": n,
                   "as-i64": n, "as-i32": n, "as-i16": n, "as-i8": n,
                   "as-u64": n, "as-u32": n, "as-u16": n, "as-u8": n,
                   "as-bytes": b, "as-number": n, "as-bool": bl}, "bytes"), b)
    # numeric methods only need their argument to be dataizable (they take its
    # bytes), so the argument type is `bytes` -- and a number is usable-as bytes
    # through its own `@`, so both number and bytes arguments are accepted.
    constrain(Rec({"plus": Fun(b, n), "minus": Fun(b, n), "times": Fun(b, n), "div": Fun(b, n),
                   "power": Fun(b, n),
                   "gt": Fun(b, bl), "lt": Fun(b, bl), "gte": Fun(b, bl), "lte": Fun(b, bl),
                   "eq": Fun(b, bl), "neg": n, "is-nan": bl, "is-integer": bl,
                   "is-finite": bl, "floor": n, "as-i64": n, "as-i32": n, "as-i16": n,
                   "as-number": n, "as-bytes": b, "@": b}, "number"), n)
    constrain(Rec({"if": Fun(a1, Fun(a2, pick)), "not": bl, "and": Fun(bl, bl), "or": Fun(bl, bl),
                   "eq": Fun(bl, bl), "as-bytes": b, "@": b}, "bool"), bl)
    return b, n, bl


def bytes_type():
    return _prims()[0]


def number_type():
    return _prims()[1]


def bool_value():
    return _prims()[2]


def string_type():
    t = Var("string"); t.alias = "string"
    b = bytes_type()
    constrain(Rec({"eq": Fun(b, bool_value()), "concat": Fun(b, t),
                   "length": number_type(), "as-bytes": b,
                   "printf": Fun(Var("args"), t), "@": b}, "string"), t)
    return t


def tuple_type(elem):
    t = Var("tuple"); t.alias = "tuple"
    constrain(Rec({"head": elem, "tail": t, "with": Fun(elem, t),
                   "at": Fun(number_type(), elem), "contains": Fun(elem, bool_value()),
                   "length": number_type()}, "tuple"), t)
    return t


def stdout_type():
    # [text] > stdout   -- takes a string, offers `.print` (which yields bytes)
    return Fun(string_type(), Rec({"print": bytes_type()}, "printed"))


def recovered_type():
    # recovered value alternative -- value (maybe ⊥) and alternative behave the
    # SAME (type A); it returns that A. (Its `/bytes` forma is a known placeholder.)
    a = Var("A")
    return Rec({"value": Opt(a), "alternative": a, "@": a},
               "recovered", ["value", "alternative"])


ATOMS = {
    "number": number_type,
    "bool": bool_value,
    "true": bool_value,
    "false": bool_value,
    "string": string_type,
    "bytes": bytes_type,
    "stdout": stdout_type,
    "dataized": lambda: Fun(Var("x"), bytes_type()),   # anything dataizable -> bytes
    "recovered": recovered_type,
}


# ===========================================================================
# Reading declared atom signatures (#5741 generic/void type annotations)
# ===========================================================================

_TYPES = {"bytes": bytes_type, "number": number_type,
          "string": string_type, "bool": bool_value}


def _generic(sig):
    """A single-letter A-F is a universally-quantified type variable."""
    return len(sig) == 1 and sig in "ABCDEF"


def _spec(text, gmap):
    """One type annotation to a type. A generic letter A-F shares `gmap` (so the
    same letter is the same variable across the atom); a trailing `?` is an Opt; a
    known forma is its primitive; any other forma is an opaque object (an unknown)."""
    if text is None:
        return Var("t")
    opt = text.endswith("?")
    core = text[:-1] if opt else text
    if _generic(core):
        ty = gmap.setdefault(core, Var(core))
    else:
        tail = _tail(core)
        ty = _TYPES[tail]() if tail in _TYPES else (_resolve_loc(core) or Var(tail))
    return Opt(ty) if opt else ty


def _atom_sig(node):
    """The atom's shape straight from its signature: each void becomes a field
    (a callback void `/{...}` becomes a function over its argument types), the
    return becomes `@`, and one `gmap` shares the generics across the whole atom."""
    gmap, fields, voids = {}, {}, []
    for label, kind, spec in node.voids:
        voids.append(label)
        if kind == "args":
            cb = Var("cb")
            for arg in reversed(spec.split()):
                cb = Fun(_spec(arg, gmap), cb)
            fields[label] = cb
        else:
            fields[label] = _spec(spec, gmap)
    fields["@"] = _spec(node.ret, gmap)
    return Rec(fields, None, voids)                    # no alias: show the derived shape


# ===========================================================================
# Pretty-printer -- a produced value is the union of what flowed into it
# ===========================================================================

def show(ty):
    """Render a type. Each variable is rendered ONCE (as a token), so a large shared
    or recursive type (fs/path) prints in LINEAR time -- the naive inline form
    re-rendered shared subterms exponentially. Then variables used once and not
    recursive are inlined (so simple types read as `number`, not `A where A=number`),
    while shared/recursive ones stay named in a trailing `where` clause; open
    variables are quantified with ∀."""
    order, known, defs, refc = [], set(), {}, {}
    sent = "\x01"

    def ref(v):
        vid = id(v)
        refc[vid] = refc.get(vid, 0) + 1
        if vid not in known:
            known.add(vid)
            order.append(v)
        return "%s%d%s" % (sent, vid, sent)

    def render(t):
        if isinstance(t, Prim):
            return t.name
        if isinstance(t, Opt):
            return render(t.inner) + "?"
        if isinstance(t, Fun):
            return "(%s -> %s)" % (render(t.arg), render(t.res))
        if isinstance(t, Rec):
            if t.alias:
                return t.alias
            body = "{" + ", ".join("%s: %s" % (k, render(v)) for k, v in t.fields.items()
                                   if k not in t.voids and k != "ρ") + "}"
            for v in reversed(t.voids):
                body = "(%s -> %s)" % (render(t.fields[v]), body)
            return body
        if isinstance(t, Var):
            return t.alias if t.alias else ref(t)
        return repr(t)

    top = render(ty)
    i = 0
    while i < len(order):                              # define each variable exactly once
        v = order[i]; i += 1; vid = id(v)
        if v.lower:
            defs[vid] = " | ".join(_dedup(render(lb) for lb in v.lower))
        elif v.upper:
            defs[vid] = " & ".join(_dedup(render(ub) for ub in v.upper))
        else:
            defs[vid] = None                           # an open (for-any) variable
    tok = re.compile("%s(\\d+)%s" % (sent, sent))
    edges = {vid: set(map(int, tok.findall(d))) for vid, d in defs.items() if d}

    def loops(vid):
        stack, seen = list(edges.get(vid, ())), set()
        while stack:
            x = stack.pop()
            if x == vid:
                return True
            if x not in seen:
                seen.add(x)
                stack.extend(edges.get(x, ()))
        return False

    inline = {vid for vid, d in defs.items()
              if d is not None and refc.get(vid, 0) <= 1 and not loops(vid)}
    letters = {}

    def letter(vid):
        if vid not in letters:
            n = len(letters)
            letters[vid] = chr(ord("A") + n % 26) + ("" if n < 26 else str(n // 26))
        return letters[vid]
    for v in order:                                    # stable letters for the kept names
        if id(v) not in inline:
            letter(id(v))

    def sub(s):
        def rep(m):
            vid = int(m.group(1))
            if vid not in inline:
                return letter(vid)
            d = sub(defs[vid])                         # functions already self-parenthesize
            return "(%s)" % d if (" | " in d or " & " in d) else d
        return tok.sub(rep, s)

    whole = tok.fullmatch(top)
    body = sub(defs[int(whole.group(1))]) if whole and int(whole.group(1)) in inline else sub(top)
    clauses = ", ".join("%s = %s" % (letter(id(v)), sub(defs[id(v)]))
                        for v in order if defs[id(v)] is not None and id(v) not in inline)
    openv = [id(v) for v in order if defs[id(v)] is None]
    result = "%s where %s" % (body, clauses) if clauses else body
    if openv:
        result = "∀%s. %s" % (" ".join(letter(vid) for vid in openv), result)
    return result


def _dedup(strings):
    out, seen = [], set()
    for s in strings:
        if s not in seen:
            seen.add(s)
            out.append(s)
    return out


# ===========================================================================
# XMIR frontend -- read EO's real XML (eo-runtime/target/eo/1-parse) into the AST
# ===========================================================================
#
# The current parser emits phi-calculus normal form -- no <program>, no
# <objects>, no `abstract`. In it:
#   * a FORMATION is an `<o>` with no `base`; its children are its attributes;
#   * a VOID attribute is `<o base="∅" name="x"/>`;
#   * the DECORATEE is the child named `φ` (we store it internally under "@");
#   * ARGUMENTS carry `as="α0"`, `as="α1"`, ...; a DISPATCH `<o base=".m">`
#     takes its one child without `as` as the receiver;
#   * BASES are scope-qualified: `Φ.x` (root), `ξ.x` (self), `ξ.ρ.x` (parent);
#   * LITERALS are `Φ.bytes` / `Φ.number` / ... applied to raw hex text.
# This is a SUBSET -- enough to load real 1-parse output and type it.

# primitive constructors -> the AST literal that infers to their result type
_PRIM = {"bytes": BytesLit, "number": lambda: NumLit(0), "string": lambda: StrLit(""),
         "bool": BoolLit, "tuple": TupleLit, "i64": lambda: NumLit(0),
         "i32": lambda: NumLit(0), "i16": lambda: NumLit(0), "float": lambda: NumLit(0)}


def _tail(base):
    """Last segment of a scope-qualified base: Φ.bool->bool, ξ.ρ.if->if, .eq->eq."""
    return base.lstrip(".").split(".")[-1]


def _split(el):
    """Split children into (receiver-or-None, [argument-children in α-order])."""
    recv, args = None, []
    for c in el:
        if c.get("as") is not None:
            args.append(c)
        else:
            recv = c
    return recv, args


def _forma(fqn):
    """The declared result type of a native atom: a primitive (`Φ.bool` -> a bool)
    or, for any other forma, a reference to the object it names by its @loc."""
    name = _tail(fqn)
    if name in _PRIM:
        return _PRIM[name]()
    return LocRef(fqn)


def _formation(el):
    ret = next((c.get("atom") for c in el if c.get("atom") is not None), None)
    if ret is not None and (_generic(ret) or any(c.get("type") is not None for c in el)):
        voids = []                                     # a fully-declared atom (#5741)
        for c in el:
            if c.get("base") == "∅":
                if c.get("type") is not None:
                    voids.append((c.get("name"), "type", c.get("type")))
                elif c.get("args") is not None:
                    voids.append((c.get("name"), "args", c.get("args")))
                else:
                    voids.append((c.get("name"), None, None))
        return AtomSig(el.get("name"), voids, ret)
    params, binds, body = [], {}, None
    for c in el:
        label = c.get("name")
        if c.get("atom") is not None:                  # native atom: forma is its result
            body = _forma(c.get("atom"))
        elif c.get("base") == "∅":
            params.append(label)
        elif label == "φ":
            body = convert(c)
        elif label and label[0] not in "+-":            # skip +tests-.../-...  assertions
            binds[label] = convert(c)
    name = el.get("name")
    if params:
        return Lam(name, params, body, binds)
    if body is not None:
        binds["@"] = body
    return Obj(name, binds)


def _scope(base, line):
    """A self/parent-relative path: dispatch each segment on `ξ` (self). `ρ` is
    the parent link (so `ξ.ρ.ρ.x` walks up two parents), `φ` the decoratee (`@`)."""
    segs = base.split(".")
    node = Name(segs[0])                               # "ξ" (the current object)
    for seg in segs[1:]:
        node = Dispatch(node, "@" if seg == "φ" else seg, line)
    return node


def convert(el):
    base = el.get("base")
    if base is None:
        if len(list(el)) == 0 and (el.text or "").strip():
            return BytesLit()                          # a raw data leaf
        return _formation(el)                          # no base + children == a formation
    name = _tail(base)
    recv, args = _split(el)
    if name in _PRIM and recv is None:                 # a literal / primitive constructor
        return _PRIM[name]()
    if base.startswith("."):                           # a method dispatch on the receiver
        node = Dispatch(convert(recv), "@" if name == "φ" else name, el.get("line"))
    elif base == "ξ" or base.startswith("ξ."):         # self / parent-relative reference
        node = _scope(base, el.get("line"))
    else:                                              # a global (Φ.x) or a bare param name
        node = Name(name)
    for a in args:
        node = Apply(node, convert(a))
    return node


def load_xmir(path):
    root = ET.parse(path).getroot()
    return [(o.get("name"), convert(o)) for o in root if o.tag == "o"]


def build_loc_index(paths):
    """Index every <o> in every file by its @loc, so a non-primitive forma (which
    is itself a Φ-rooted locator) can be resolved to the object it names."""
    for path in paths:
        try:
            root = ET.parse(path).getroot()
        except Exception:
            continue
        for o in root:                             # a program's direct children are its
            if o.tag == "o" and o.get("loc"):      # top-level objects (their parent chain
                LOC_TOPLEVEL[o.get("loc")] = o     # is self-contained -> typeable alone)
        for el in root.iter("o"):
            loc = el.get("loc")
            if loc is not None:
                LOC_INDEX.setdefault(loc, el)


def _toplevel_of(loc):
    """The top-level ancestor of `loc` and the attribute path down to it. A nested
    object is reached THROUGH its ancestor -- inferred there, its parent chain
    (ξ.ρ...) resolves, which it cannot in isolation. Longest prefix wins."""
    segs = loc.split(".")
    for i in range(len(segs), 0, -1):
        head = ".".join(segs[:i])
        if head in LOC_TOPLEVEL:
            return LOC_TOPLEVEL[head], segs[i:]
    return None, None


def _project(ty, seg):
    """Navigate to attribute `seg` of an already-inferred object's type."""
    host = _host(ty)
    if host is None or seg not in host.fields:
        raise Clash("no attribute `%s` to navigate" % seg)
    return host.fields[seg]


def _resolve_loc(loc):
    """The type of the object a forma names: found through its top-level ancestor,
    inferred once (generalizable), then instantiated per use with `freshen`. A
    self-referential object ties the knot through a shared recursion variable;
    best-effort -- an unknown or un-navigable locator returns None (stay opaque)."""
    if loc in _LOC_INPROGRESS:
        return _LOC_INPROGRESS[loc]
    if loc not in _LOC_CACHE:
        top, path = _toplevel_of(loc)
        if top is None:
            return None
        def build():
            knot = Var(_tail(loc), _LEVEL[0] + 1)
            _LOC_INPROGRESS[loc] = knot
            try:
                ty = infer(convert(top), {})
                for seg in path:                   # walk down to the nested object,
                    ty = _project(ty, seg)         # whose ρ resolved within the ancestor
                constrain(ty, knot)
                return ty
            finally:
                _LOC_INPROGRESS.pop(loc, None)
        try:
            _LOC_CACHE[loc] = _at0(build)
        except Exception:
            _LOC_CACHE[loc] = None
    cached = _LOC_CACHE[loc]
    return freshen(0, cached, {}) if cached is not None else None


# ===========================================================================
# Examples -- the paper's accept / reject cases, run as a test suite
# ===========================================================================

def nm(x): return Name(x)
def num(n): return NumLit(n)
def dot(o, l): return Dispatch(o, l)


def app(fn, *args):
    for a in args:
        fn = Apply(fn, a)
    return fn


def my_num(decoratee):
    return Obj("my-num", {"@": decoratee})


# last: walk a tuple recursively -- the object that used to loop forever.
#   [tup] > last
#     (tup.tail.length.eq 0).if tup.head (last tup.tail) > @
LAST = Lam("last", ["tup"],
           app(dot(app(dot(dot(dot(nm("tup"), "tail"), "length"), "eq"), num(0)), "if"),
               dot(nm("tup"), "head"),
               app(nm("last"), dot(nm("tup"), "tail"))))


EXAMPLES = [
    ("my-num (=5) . plus 1",              app(dot(my_num(num(5)), "plus"), num(1)),        "ok"),
    ("my-num (=\"hi\") . plus 1",         app(dot(my_num(StrLit("hi")), "plus"), num(1)),  "reject"),
    ("recovered (fragile number) 7",      Recovered(Fragile(num(1)), num(7)),              "ok"),
    ("(fragile number) . plus 1",         app(dot(Fragile(num(1)), "plus"), num(1)),       "reject"),
    ("bool.if 123 42",                    app(dot(nm("bool"), "if"), num(123), num(42)),   "ok"),
    ("bool.if 123 <bytes>  (a union)",    app(dot(nm("bool"), "if"), num(123), BytesLit()), "ok"),
    ("(bool.if 42 7).plus 1  (both branches number)",
     app(dot(app(dot(nm("bool"), "if"), num(42), num(7)), "plus"), num(1)),                "ok"),
    ("(bool.if 42 \"x\").plus 1  (misuse caught by A|B)",
     app(dot(app(dot(nm("bool"), "if"), num(42), StrLit("x")), "plus"), num(1)),           "reject"),
    ("main = [a] > a.x   (structural requirement)", Lam("main", ["a"], dot(nm("a"), "x")), "ok"),
    ("(main=[a]>a.plus 5) 42    (number has .plus)",
     app(Lam("main", ["a"], app(dot(nm("a"), "plus"), num(5))), num(42)),                  "ok"),
    ("(main=[a]>a.plus 5) \"hi\"  (string lacks .plus -> caught)",
     app(Lam("main", ["a"], app(dot(nm("a"), "plus"), num(5))), StrLit("hi")),             "reject"),
    ("rec=[n]>(n.eq 0).if 0 (rec (n.minus 1))  (recursion -> fixpoint)",
     Lam("rec", ["n"],
         app(dot(app(dot(nm("n"), "eq"), num(0)), "if"),
             num(0),
             app(nm("rec"), app(dot(nm("n"), "minus"), num(1))))),                         "ok"),
    ("last  (recursive; used to loop)",   LAST,                                            "ok"),
]


def run_examples():
    passed = 0
    for desc, ast, expect in EXAMPLES:
        try:
            ty = infer(ast, {})
            verdict, detail = "ok", show(ty)
        except Clash as err:
            verdict, detail = "reject", str(err)
        ok = verdict == expect
        passed += ok
        mark = "PASS" if ok else "FAIL"
        tag = "OK    " if verdict == "ok" else "REJECT"
        print("[%s] %-6s %-34s %s" % (mark, tag, desc, ":  " + detail))
    print("\n%d/%d examples behaved as expected." % (passed, len(EXAMPLES)))


def check_file(path):
    global LENIENT
    LENIENT = True
    print("== %s ==" % path)
    for name, ast in load_xmir(path):
        try:
            print("  %s : %s" % (name, show(infer(ast, {}))))
        except Clash as err:
            print("  %s : REJECT -- %s" % (name, err))
        except Exception as err:                      # a loader / atom gap, not a type error
            print("  %s : (unmodelled) %s" % (name, err))


def main():
    args = sys.argv[1:]
    if args and args[0] == "--atoms":             # diagnostic: which atoms are unmodelled?
        global LENIENT
        LENIENT = True
        build_loc_index(args[1:])
        for path in args[1:]:
            try:
                trees = load_xmir(path)
            except Exception:
                continue
            for _name, tree in trees:
                try:
                    infer(tree, {})
                except Exception:
                    pass
        for name, cnt in sorted(_UNMODELLED.items(), key=lambda kv: (-kv[1], kv[0])):
            print("%5d  %s" % (cnt, name))
        return
    if args:
        build_loc_index(args)
        for path in args:
            check_file(path)
    else:
        run_examples()


if __name__ == "__main__":
    main()
