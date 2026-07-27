//! The walker.
//!
//! One branch per node of the abstract syntax tree, turning the four moves of
//! the calculus — make a formation, apply an argument to the next void, dispatch
//! an attribute, fall through a decoratee — into constraints.
//!
//! The built-in shapes live here too: `bytes`, `number` and a `bool` value are
//! mutually recursive and hand-modelled, because they are the ground everything
//! else decorates. Every other atom is read from what it declares about itself.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::resolve::Locs;
use crate::solver::{Clash, Solver};
use crate::types::{Level, RecId, Site, Type, TypeId, Types};
use crate::xmir::{Kind, Node, generic, tail};

/// What names are in scope, and what they stand for.
pub type Env = HashMap<String, TypeId>;

/// How forgiving the walker is about what it does not know, and what it noticed
/// while being forgiving.
#[derive(Debug, Default)]
pub struct Leniency {
    /// Whether an unknown name becomes a fresh unknown rather than a failure.
    /// On when checking real files, where an unmodelled atom must not be
    /// mistaken for an error.
    pub lenient: bool,
    /// How often each unmodelled name was reached.
    pub unmodelled: BTreeMap<String, usize>,
}

/// The whole checker: the arena, the solver, the locator index, and how
/// forgiving to be.
#[derive(Debug, Default)]
pub struct Engine {
    /// The arena every type lives in.
    pub types: Types,
    /// The constraint solver.
    pub solver: Solver,
    /// Locator resolution.
    pub locs: Locs,
    /// What to do about the unknown.
    pub leniency: Leniency,
    defining: HashSet<RecId>,
}

impl Engine {
    /// A checker that fails on an unknown name, as the examples need.
    #[must_use]
    pub fn strict() -> Self {
        Self::default()
    }

    /// A checker that treats an unknown name as anything, as real files need:
    /// an unmodelled atom must never turn into a false error.
    #[must_use]
    pub fn lenient(fragile: bool) -> Self {
        Self {
            solver: Solver::new(fragile),
            leniency: Leniency {
                lenient: true,
                unmodelled: BTreeMap::new(),
            },
            ..Self::default()
        }
    }

    /// The type of one node.
    ///
    /// # Errors
    ///
    /// A [`Clash`] when the program cannot be typed.
    pub fn infer(&mut self, node: &Node, env: &Env) -> Result<TypeId, Clash> {
        match node {
            Node::Name(name) => self.name(name, env),
            Node::Loc(loc) => self.located(loc, env),
            Node::Num => Ok(self.ground(Ground::Number)),
            Node::Str => Ok(self.ground(Ground::String)),
            Node::Bytes => Ok(self.ground(Ground::Bytes)),
            Node::Bool => Ok(self.ground(Ground::Bool)),
            Node::Tuple => Ok(self.ground(Ground::Tuple)),
            Node::Atom { voids, ret } => {
                let save = self.solver.jump(Level::default());
                let ty = self.declared(voids, ret.as_deref());
                self.solver.jump(save);
                Ok(ty)
            }
            Node::Obj { name, binds } => self.formation(name.as_deref(), &[], binds, None, env),
            Node::Lam {
                name,
                params,
                body,
                binds,
            } => self.formation(name.as_deref(), params, binds, body.as_deref(), env),
            Node::Apply { fun, arg } => {
                let applied = self.infer(fun, env)?;
                let result = self.fresh();
                let given = self.infer(arg, env)?;
                let want = self.types.fun(given, result);
                self.solver.constrain(&mut self.types, applied, want)?;
                Ok(result)
            }
            Node::Dispatch { obj, label, site } => {
                let receiver = self.infer(obj, env)?;
                if let Some(shortcut) = self.through(receiver, label) {
                    return Ok(shortcut);
                }
                let result = self.fresh();
                let want = self.requirement(label, result, site);
                self.solver.constrain(&mut self.types, receiver, want)?;
                Ok(result)
            }
            Node::Fragile { obj, label, site } => {
                let receiver = self.infer(obj, env)?;
                let result = self.fresh();
                let want = self.requirement(label, result, site);
                if let Type::Opt(inner) = self.types.at(self.peek(receiver)) {
                    self.solver.constrain(&mut self.types, inner, want)?;
                    return Ok(self.types.opt(result));
                }
                self.solver.constrain(&mut self.types, receiver, want)?;
                Ok(result)
            }
            Node::Maybe(inner) => {
                let inner = self.infer(inner, env)?;
                Ok(self.types.opt(inner))
            }
            Node::Recovered { value, alt } => {
                let held = self.infer(value, env)?;
                let otherwise = self.infer(alt, env)?;
                let result = self.fresh();
                let stripped = match self.types.at(held) {
                    Type::Opt(inner) => inner,
                    _ => held,
                };
                self.solver.constrain(&mut self.types, stripped, result)?;
                self.solver.constrain(&mut self.types, otherwise, result)?;
                Ok(result)
            }
        }
    }

    /// A fresh unknown at the level the solver is at.
    pub fn fresh(&mut self) -> TypeId {
        let unknown = self.types.var(self.solver.level());
        self.types.node(Type::Var(unknown))
    }

    /// The record behind a dispatch receiver, when it is known: the in-progress
    /// record of a formation being defined, or an unknown pinned to one record.
    #[must_use]
    pub fn host(&self, ty: TypeId) -> Option<RecId> {
        match self.types.at(ty) {
            Type::Rec(shape) => Some(shape),
            Type::Var(unknown) => {
                if let Some(shape) = self.types.knot(unknown) {
                    return Some(shape);
                }
                match self.types.lower(unknown) {
                    [only] => match self.types.at(*only) {
                        Type::Rec(shape) => Some(shape),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Type::Fun { .. } | Type::Opt(_) => None,
        }
    }

    /// The attribute of an already inferred object, for walking down a locator.
    ///
    /// # Errors
    ///
    /// A [`Clash`] when there is no such attribute to walk through.
    pub fn project(&mut self, ty: TypeId, seg: &str) -> Result<TypeId, Clash> {
        self.host(ty)
            .and_then(|shape| self.types.field(shape, seg))
            .ok_or_else(|| Clash::UnknownAttribute {
                attribute: seg.to_owned(),
                receiver: ty,
                site: Site::default(),
            })
    }

    /// A name: what it stands for in scope, then among the built-ins, then —
    /// when forgiving — anything at all.
    fn name(&mut self, name: &str, env: &Env) -> Result<TypeId, Clash> {
        if let Some(&found) = env.get(name) {
            return Ok(found);
        }
        if let Some(atom) = self.atom(name) {
            return Ok(atom);
        }
        if self.leniency.lenient {
            *self.leniency.unmodelled.entry(name.to_owned()).or_default() += 1;
            return Ok(self.opaque());
        }
        Err(Clash::Unbound {
            name: name.to_owned(),
        })
    }

    /// A forma. In scope or a built-in it behaves as a name would; otherwise it
    /// is resolved through the locator index to the shape it points at.
    fn located(&mut self, loc: &str, env: &Env) -> Result<TypeId, Clash> {
        let last = tail(loc).to_owned();
        if let Some(&found) = env.get(&last) {
            return Ok(found);
        }
        if let Some(atom) = self.atom(&last) {
            return Ok(atom);
        }
        if let Some(resolved) = self.resolve(loc) {
            return Ok(resolved);
        }
        if self.leniency.lenient {
            *self.leniency.unmodelled.entry(loc.to_owned()).or_default() += 1;
            return Ok(self.opaque());
        }
        Err(Clash::Unbound {
            name: loc.to_owned(),
        })
    }

    /// An unknown at the outermost level: fully general, so it never needs
    /// lowering and stays shared with whatever uses it.
    fn opaque(&mut self) -> TypeId {
        let unknown = self.types.var(Level::default());
        self.types.node(Type::Var(unknown))
    }

    /// A formation. The recursion unknown lives at *body* level rather than
    /// definition level: tying the knot at definition level would lower the
    /// parameters along with it, and the checker would stop noticing real
    /// errors without failing anything.
    fn formation(
        &mut self,
        name: Option<&str>,
        params: &[String],
        binds: &[(String, Node)],
        body: Option<&Node>,
        env: &Env,
    ) -> Result<TypeId, Clash> {
        let outer = self.solver.level();
        self.solver.descend();
        let host = self.types.var(self.solver.level());
        let ty = self.types.node(Type::Var(host));
        let mut inner = env.clone();
        if let Some(name) = name {
            inner.insert(name.to_owned(), ty);
        }
        let parent = env.get("ξ").copied();
        inner.insert("ξ".to_owned(), ty);
        let shape = self.types.rec(params.to_vec());
        self.types.tie(host, shape);
        self.types.define(shape, outer);
        if let Some(parent) = parent {
            self.types.bind(shape, "ρ", parent);
        }
        self.defining.insert(shape);
        for param in params {
            let slot = self.fresh();
            self.types.bind(shape, param, slot);
            inner.insert(param.clone(), slot);
        }
        let outcome = self.attributes(shape, binds, body, &inner);
        self.defining.remove(&shape);
        self.solver.jump(outer);
        outcome?;
        let whole = self.types.node(Type::Rec(shape));
        self.solver.constrain(&mut self.types, whole, ty)?;
        Ok(ty)
    }

    /// Every attribute of a formation, definitions first so that a forward
    /// reference to a sibling written later still finds its record, and the
    /// decoratee last so it sees them all.
    fn attributes(
        &mut self,
        shape: RecId,
        binds: &[(String, Node)],
        body: Option<&Node>,
        env: &Env,
    ) -> Result<(), Clash> {
        let mut ordered: Vec<&(String, Node)> = binds.iter().collect();
        ordered
            .sort_by_key(|(_, sub)| u8::from(!matches!(sub, Node::Obj { .. } | Node::Lam { .. })));
        for (label, sub) in ordered {
            let ty = self.infer(sub, env)?;
            self.types.bind(shape, label, ty);
        }
        if let Some(body) = body {
            let ty = self.infer(body, env)?;
            self.types.bind(shape, "@", ty);
        }
        Ok(())
    }

    /// A one-attribute requirement, remembering where it was written so a
    /// failure can be pointed at.
    fn requirement(&mut self, label: &str, result: TypeId, site: &Site) -> TypeId {
        let want = self.types.rec(Vec::new());
        self.types.bind(want, label, result);
        self.types.mark(want, site.clone());
        self.types.node(Type::Rec(want))
    }

    /// Reaching an attribute directly, when the receiver's record is already
    /// known. The parent link is navigation and hands back what it holds; a
    /// formation still expecting voids is instantiated, so each use of it gets
    /// its own parameters.
    fn through(&mut self, receiver: TypeId, label: &str) -> Option<TypeId> {
        let shape = self.host(receiver)?;
        let field = self.types.field(shape, label)?;
        if label == "ρ" {
            return Some(field);
        }
        let inner = self.host(field)?;
        if self.types.voids(inner).is_empty() || self.defining.contains(&inner) {
            return None;
        }
        let limit = self.types.defined_at(inner).unwrap_or_default();
        let whole = self.types.node(Type::Rec(inner));
        let instance = self.solver.freshen(&mut self.types, limit, whole);
        Some(if self.solver.fragile() {
            self.types.opt(instance)
        } else {
            instance
        })
    }

    /// See past unknowns that carry exactly one thing, to what they carry — so
    /// optional chaining can find a maybe-`⊥` value even behind the unknowns the
    /// solver generated.
    fn peek(&self, mut ty: TypeId) -> TypeId {
        let mut seen = HashSet::new();
        while let Type::Var(unknown) = self.types.at(ty) {
            if !seen.insert(unknown) {
                break;
            }
            match self.types.lower(unknown) {
                [only] => ty = *only,
                _ => break,
            }
        }
        ty
    }

    /// The shape a native atom declares for itself: each void becomes an
    /// attribute, the return becomes the decoratee, and one map of generics is
    /// shared across the whole atom, so the same letter names the same variable
    /// wherever it appears.
    fn declared(&mut self, voids: &[crate::xmir::Void], ret: Option<&str>) -> TypeId {
        let mut generics = HashMap::new();
        let shape = self
            .types
            .rec(voids.iter().map(|void| void.label.clone()).collect());
        for void in voids {
            let ty = match &void.kind {
                Kind::Args(spec) => {
                    let mut callback = self.fresh();
                    for arg in spec
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        let taken = self.spec(Some(arg), &mut generics);
                        callback = self.types.fun(taken, callback);
                    }
                    callback
                }
                Kind::Own(spec) => self.spec(Some(spec), &mut generics),
                Kind::Plain => self.spec(None, &mut generics),
            };
            self.types.bind(shape, &void.label, ty);
        }
        let returned = self.spec(ret, &mut generics);
        self.types.bind(shape, "@", returned);
        self.types.node(Type::Rec(shape))
    }

    /// One annotation as a type. A letter shares the atom's map of generics, a
    /// trailing `?` makes it maybe-`⊥`, a known forma is its built-in, and any
    /// other forma is resolved through the locator index or left opaque.
    fn spec(&mut self, text: Option<&str>, generics: &mut HashMap<String, TypeId>) -> TypeId {
        let Some(text) = text else {
            return self.fresh();
        };
        let optional = text.ends_with('?');
        let core = if optional {
            &text[..text.len() - 1]
        } else {
            text
        };
        let ty = if generic(core) {
            if let Some(&found) = generics.get(core) {
                found
            } else {
                let made = self.fresh();
                generics.insert(core.to_owned(), made);
                made
            }
        } else {
            let last = tail(core);
            match Ground::named(last) {
                Some(ground) => self.build(ground),
                None => self.resolve(core).unwrap_or_else(|| self.fresh()),
            }
        };
        if optional { self.types.opt(ty) } else { ty }
    }

    /// A built-in, built at the outermost level. A starting fact is fully
    /// general, so it never needs lowering — and building it any deeper sets off
    /// extrusion storms.
    fn ground(&mut self, ground: Ground) -> TypeId {
        let save = self.solver.jump(Level::default());
        let built = self.build(ground);
        self.solver.jump(save);
        built
    }

    /// The built-in a name stands for, if it is one.
    fn atom(&mut self, name: &str) -> Option<TypeId> {
        let ground = match name {
            "number" => Ground::Number,
            "bool" | "true" | "false" => Ground::Bool,
            "string" => Ground::String,
            "bytes" => Ground::Bytes,
            "stdout" => Ground::Stdout,
            "dataized" => Ground::Dataized,
            "recovered" => Ground::Recovered,
            _ => return None,
        };
        Some(self.ground(ground))
    }
}

/// A built-in shape.
#[derive(Clone, Copy, Debug)]
enum Ground {
    /// The one leaf object everything else bottoms out in.
    Bytes,
    /// A number.
    Number,
    /// A boolean value.
    Bool,
    /// A string.
    String,
    /// A tuple over a fresh element type.
    Tuple,
    /// Takes a string, offers `print`.
    Stdout,
    /// Anything dataizable, giving back its bytes.
    Dataized,
    /// The `⊥` eliminator.
    Recovered,
}

impl Ground {
    /// The built-in a forma names, when it names one of the four primitives.
    fn named(name: &str) -> Option<Self> {
        match name {
            "bytes" => Some(Self::Bytes),
            "number" => Some(Self::Number),
            "string" => Some(Self::String),
            "bool" => Some(Self::Bool),
            _ => None,
        }
    }
}

impl Engine {
    fn build(&mut self, ground: Ground) -> TypeId {
        match ground {
            Ground::Bytes => self.prims().0,
            Ground::Number => self.prims().1,
            Ground::Bool => self.prims().2,
            Ground::String => self.string(),
            Ground::Tuple => {
                let element = self.fresh();
                self.tuple(element)
            }
            Ground::Stdout => self.stdout(),
            Ground::Dataized => {
                let anything = self.fresh();
                let bytes = self.prims().0;
                self.types.fun(anything, bytes)
            }
            Ground::Recovered => self.recovered(),
        }
    }

    /// A named unknown, standing for a built-in. The name makes it print by
    /// name rather than by shape, and makes instantiation share it rather than
    /// copy it.
    fn named(&mut self, alias: &str) -> TypeId {
        let unknown = self.types.var(self.solver.level());
        let ty = self.types.node(Type::Var(unknown));
        self.types.name(ty, alias);
        ty
    }

    /// Bind a shape's attributes and tie it to the unknown that names it.
    fn shape(&mut self, alias: &str, named: TypeId, fields: Vec<(&str, TypeId)>) {
        let shape = self.types.rec(Vec::new());
        for (label, ty) in fields {
            self.types.bind(shape, label, ty);
        }
        let whole = self.types.node(Type::Rec(shape));
        self.types.name(whole, alias);
        let _ = self.solver.constrain(&mut self.types, whole, named);
    }

    /// The three mutually recursive built-ins, built together so their methods
    /// can name one another. `bytes` is the one object that decorates nothing;
    /// everything else bottoms out in it.
    fn prims(&mut self) -> (TypeId, TypeId, TypeId) {
        let bytes = self.named("bytes");
        let number = self.named("number");
        let truth = self.named("bool");
        self.leaf(bytes, number, truth);
        self.arithmetic(bytes, number, truth);
        self.logic(bytes, truth);
        (bytes, number, truth)
    }

    fn leaf(&mut self, bytes: TypeId, number: TypeId, truth: TypeId) {
        let eq = self.types.fun(bytes, truth);
        let over = self.types.fun(bytes, bytes);
        let of_number = self.types.fun(number, bytes);
        let slice = {
            let inner = self.types.fun(number, bytes);
            self.types.fun(number, inner)
        };
        self.shape(
            "bytes",
            bytes,
            vec![
                ("eq", eq),
                ("and", over),
                ("or", over),
                ("xor", over),
                ("not", bytes),
                ("left", of_number),
                ("right", of_number),
                ("concat", over),
                ("slice", slice),
                ("size", number),
                ("as-i64", number),
                ("as-i32", number),
                ("as-i16", number),
                ("as-i8", number),
                ("as-u64", number),
                ("as-u32", number),
                ("as-u16", number),
                ("as-u8", number),
                ("as-bytes", bytes),
                ("as-number", number),
                ("as-bool", truth),
            ],
        );
    }

    /// The numeric methods only need their argument to be dataizable, since they
    /// take its bytes — so the argument is `bytes`, and a number is usable as
    /// bytes through its own decoratee. Both are therefore accepted.
    fn arithmetic(&mut self, bytes: TypeId, number: TypeId, truth: TypeId) {
        let arith = self.types.fun(bytes, number);
        let compare = self.types.fun(bytes, truth);
        self.shape(
            "number",
            number,
            vec![
                ("plus", arith),
                ("minus", arith),
                ("times", arith),
                ("div", arith),
                ("power", arith),
                ("gt", compare),
                ("lt", compare),
                ("gte", compare),
                ("lte", compare),
                ("eq", compare),
                ("neg", number),
                ("is-nan", truth),
                ("is-integer", truth),
                ("is-finite", truth),
                ("floor", number),
                ("as-i64", number),
                ("as-i32", number),
                ("as-i16", number),
                ("as-number", number),
                ("as-bytes", bytes),
                ("@", bytes),
            ],
        );
    }

    /// The branches of a conditional both flow into one result, which is what
    /// lets a demand on that result reach each of them.
    fn logic(&mut self, bytes: TypeId, truth: TypeId) {
        let (left, right, pick) = (self.fresh(), self.fresh(), self.fresh());
        let _ = self.solver.constrain(&mut self.types, left, pick);
        let _ = self.solver.constrain(&mut self.types, right, pick);
        let branch = {
            let inner = self.types.fun(right, pick);
            self.types.fun(left, inner)
        };
        let logic = self.types.fun(truth, truth);
        self.shape(
            "bool",
            truth,
            vec![
                ("if", branch),
                ("not", truth),
                ("and", logic),
                ("or", logic),
                ("eq", logic),
                ("as-bytes", bytes),
                ("@", bytes),
            ],
        );
    }

    fn string(&mut self) -> TypeId {
        let text = self.named("string");
        let bytes = self.prims().0;
        let truth = self.prims().2;
        let eq = self.types.fun(bytes, truth);
        let concat = self.types.fun(bytes, text);
        let length = self.prims().1;
        let printf = {
            let args = self.fresh();
            self.types.fun(args, text)
        };
        self.shape(
            "string",
            text,
            vec![
                ("eq", eq),
                ("concat", concat),
                ("length", length),
                ("as-bytes", bytes),
                ("printf", printf),
                ("@", bytes),
            ],
        );
        text
    }

    fn tuple(&mut self, element: TypeId) -> TypeId {
        let whole = self.named("tuple");
        let with = self.types.fun(element, whole);
        let at = {
            let index = self.prims().1;
            self.types.fun(index, element)
        };
        let contains = {
            let truth = self.prims().2;
            self.types.fun(element, truth)
        };
        let length = self.prims().1;
        self.shape(
            "tuple",
            whole,
            vec![
                ("head", element),
                ("tail", whole),
                ("with", with),
                ("at", at),
                ("contains", contains),
                ("length", length),
            ],
        );
        whole
    }

    fn stdout(&mut self) -> TypeId {
        let text = self.string();
        let printed = self.types.rec(Vec::new());
        let bytes = self.prims().0;
        self.types.bind(printed, "print", bytes);
        let whole = self.types.node(Type::Rec(printed));
        self.types.name(whole, "printed");
        self.types.fun(text, whole)
    }

    fn recovered(&mut self) -> TypeId {
        let held = self.fresh();
        let shape = self
            .types
            .rec(vec!["value".to_owned(), "alternative".to_owned()]);
        let maybe = self.types.opt(held);
        self.types.bind(shape, "value", maybe);
        self.types.bind(shape, "alternative", held);
        self.types.bind(shape, "@", held);
        let whole = self.types.node(Type::Rec(shape));
        self.types.name(whole, "recovered");
        whole
    }
}
