//! The constraint solver.
//!
//! Constraining says one type must be usable as another, accumulating lower and
//! upper bounds on every unknown it passes through. Levels keep a formation's
//! parameters fresh per use while its recursion and context stay shared, and
//! extrusion lowers a deep unknown when it meets a shallower one.
//!
//! Nothing here formats prose. A failure is a [`Clash`] carrying the types and
//! labels involved, so the same fact can become a sentence for a person or a
//! payload for a build gate.

use std::collections::{HashMap, HashSet};

use crate::types::{Level, RecId, Site, Type, TypeId, Types, VarId};

/// A real type error: two shapes that cannot fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clash {
    /// The receiver has no such attribute, and no decoratee to fall through to.
    UnknownAttribute {
        /// The attribute that was asked for.
        attribute: String,
        /// The receiver that lacks it.
        receiver: TypeId,
        /// Where the dispatch was written.
        site: Site,
    },
    /// A value that may be `⊥` met a requirement needing it to be definite.
    NotRecovered,
    /// Dispatching on an object that still has an attribute unset.
    IncompleteDispatch {
        /// The attribute that was never set.
        unset: String,
        /// The attribute the dispatch asked for.
        attribute: String,
        /// Where the dispatch was written.
        site: Site,
    },
    /// A shape was wanted where the value carries no attributes at all.
    NoAttributes {
        /// The value that has none.
        value: TypeId,
        /// The attributes that were wanted.
        wanted: Vec<String>,
    },
    /// Nothing else fits.
    Mismatch {
        /// What was given.
        value: TypeId,
        /// What was wanted.
        wanted: TypeId,
    },
    /// A name or a forma that stands for nothing the checker knows.
    Unbound {
        /// What could not be found.
        name: String,
    },
}

impl Clash {
    /// The stable machine string a consumer gates on. This, not the prose, is
    /// the tool's contract with whatever reads its verdicts.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownAttribute { .. } => "type/unknown-attribute",
            Self::NotRecovered => "type/not-recovered",
            Self::IncompleteDispatch { .. } => "type/incomplete-dispatch",
            Self::NoAttributes { .. } | Self::Mismatch { .. } => "type/argument-mismatch",
            Self::Unbound { .. } => "ref/unbound-name",
        }
    }

    /// The same failure, blamed on the shape the attribute was asked of rather
    /// than on the decoratee the search ran out in.
    ///
    /// Falling through `@` is how width subtyping works, so a missing attribute
    /// surfaces at the far end of a decoratee chain — at `bytes`, nearly always.
    /// That is true and useless: `"hi".plus` is a mistake about a string, and
    /// the string is what somebody wrote. Only a failure about this very
    /// attribute moves; whatever went wrong deeper down stays where it happened.
    #[must_use]
    fn blamed(self, label: &str, receiver: TypeId) -> Self {
        match self {
            Self::UnknownAttribute {
                attribute, site, ..
            } if attribute == label => Self::UnknownAttribute {
                attribute,
                receiver,
                site,
            },
            settled => settled,
        }
    }

    /// Whether a failure is worth stopping a build over.
    ///
    /// All but one are: a shape that cannot fit is a mistake the program would
    /// pay for at run time. An incomplete object is the exception, because it
    /// passes the shape check — dispatching on a half-built object is a design
    /// smell, worth saying and not worth failing on.
    #[must_use]
    pub fn gates(&self) -> bool {
        !matches!(self, Self::IncompleteDispatch { .. })
    }
}

/// What identity means while walking the graph. A record reached twice through
/// two different handles is the same record, so the knot-tying set has to key on
/// the thing itself rather than on the handle that found it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ident {
    Var(VarId),
    Rec(RecId),
    Node(TypeId),
}

/// Pairs already being handled, so a cyclic decoratee or a recursive
/// application ties off instead of descending forever.
type Seen = HashSet<(Ident, Ident)>;

/// A copy in progress. A handle seen twice must map to one replacement, or the
/// copy loses the sharing that made the original finite.
type Memo = HashMap<Ident, TypeId>;

/// A lowering in progress. Keyed by polarity as well, since the same unknown
/// lowered on the way in and on the way out yields two different unknowns.
type Polar = HashMap<(Ident, bool), TypeId>;

/// The solver.
///
/// It holds the current level, the caches that make cyclic work terminate, and
/// whether an object with an unset attribute counts as fragile. The type graph
/// itself stays in the arena and is passed in, so the solver owns policy and the
/// arena owns data.
#[derive(Debug)]
pub struct Solver {
    level: Level,
    needs: HashMap<(String, TypeId), TypeId>,
    applied: HashMap<(RecId, usize), RecId>,
    fragile: bool,
}

impl Solver {
    /// A solver at the outermost level. `fragile` turns on the object-level
    /// incompleteness rule, which flags dispatching on a half-built object.
    #[must_use]
    pub fn new(fragile: bool) -> Self {
        Self {
            level: Level::default(),
            needs: HashMap::new(),
            applied: HashMap::new(),
            fragile,
        }
    }

    /// How local the solver is right now. Instantiation puts the unknowns it
    /// makes at this level.
    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    /// Go one level more local, as into a formation's body.
    pub fn descend(&mut self) {
        self.level = self.level.deeper();
    }

    /// Come back out of a formation's body.
    ///
    /// # Panics
    ///
    /// If the solver is already at the outermost level.
    pub fn ascend(&mut self) {
        self.level = self
            .level
            .shallower()
            .expect("already at the outermost level");
    }

    /// Go to a level, handing back the one just left. Used to build a starting
    /// fact — an atom or a literal — at the outermost level, where it is fully
    /// general and never needs lowering.
    pub fn jump(&mut self, level: Level) -> Level {
        std::mem::replace(&mut self.level, level)
    }

    /// Whether an object with an unset attribute counts as fragile.
    #[must_use]
    pub fn fragile(&self) -> bool {
        self.fragile
    }

    /// `lhs` must be usable as `rhs`.
    ///
    /// Every fact this discovers is written onto the unknowns it passes through:
    /// a lower bound where something flowed in, an upper bound where something
    /// is demanded.
    ///
    /// # Errors
    ///
    /// A [`Clash`] the moment a lower bound and an upper bound cannot both hold.
    pub fn constrain(&mut self, types: &mut Types, lhs: TypeId, rhs: TypeId) -> Result<(), Clash> {
        self.fit(types, lhs, rhs, &mut Seen::new())
    }

    /// Instantiate: a copy of `ty` in which every unknown deeper than `limit`
    /// becomes a new unknown at the current level, with its bounds copied too.
    ///
    /// Unknowns at or above `limit` are shared, not copied — they are the
    /// definition's context and its own recursion, and making those fresh would
    /// break the knot. A named built-in is shared for the same reason.
    pub fn freshen(&mut self, types: &mut Types, limit: Level, ty: TypeId) -> TypeId {
        self.copy(types, limit, ty, &mut Memo::new())
    }

    /// Lower every unknown deeper than `level` to a fresh one at `level`, linked
    /// to the original so nothing that escaped to the shallower side can still
    /// be generalized.
    ///
    /// `positive` is the polarity: which side's bounds travel with the copy.
    ///
    /// Unlike instantiation this needs nothing from the solver — it is told
    /// where to land rather than asking where it is.
    pub fn extrude(types: &mut Types, ty: TypeId, level: Level, positive: bool) -> TypeId {
        Self::sink(types, ty, level, positive, &mut Polar::new())
    }

    fn fit(
        &mut self,
        types: &mut Types,
        lhs: TypeId,
        rhs: TypeId,
        seen: &mut Seen,
    ) -> Result<(), Clash> {
        if !seen.insert((Self::ident(types, lhs), Self::ident(types, rhs))) {
            return Ok(());
        }
        if let Some(settled) = self.options(types, lhs, rhs, seen) {
            return settled;
        }
        if let Some(settled) = self.unknowns(types, lhs, rhs, seen) {
            return settled;
        }
        self.shapes(types, lhs, rhs, seen)
    }

    /// The option discipline. A `T?` fits another `T?`, and a definite value
    /// fits where a `T?` is wanted — but a `T?` meeting a definite requirement
    /// is the error the discipline exists for, unless the requirement is an
    /// application, since finishing a half-built object is allowed. Into an
    /// unknown it simply flows.
    fn options(
        &mut self,
        types: &mut Types,
        lhs: TypeId,
        rhs: TypeId,
        seen: &mut Seen,
    ) -> Option<Result<(), Clash>> {
        let (left, right) = (types.at(lhs), types.at(rhs));
        if let (Type::Opt(value), Type::Opt(want)) = (left, right) {
            return Some(self.fit(types, value, want, seen));
        }
        if let Type::Opt(value) = left {
            if matches!(right, Type::Fun { .. }) && Self::unfinished(types, value) {
                return Some(self.fit(types, value, rhs, seen));
            }
            if !matches!(right, Type::Var(_)) {
                return Some(Err(Self::bottom(types, value, rhs)));
            }
        }
        if let Type::Opt(want) = right {
            return Some(self.fit(types, lhs, want, seen));
        }
        None
    }

    /// Unknowns. A bound is recorded and pushed along to what is already known
    /// about the unknown, so a fact learned later still meets a fact learned
    /// earlier. When the two sides sit at different levels the deeper one is
    /// lowered first, which is what stops something that escaped outward from
    /// being generalized as though it had not.
    fn unknowns(
        &mut self,
        types: &mut Types,
        lhs: TypeId,
        rhs: TypeId,
        seen: &mut Seen,
    ) -> Option<Result<(), Clash>> {
        let (left, right) = (types.at(lhs), types.at(rhs));
        if let Type::Var(unknown) = left {
            if Self::level_of(types, rhs) <= types.level(unknown) {
                types.flow_out(unknown, rhs);
                for reached in types.lower(unknown).to_vec() {
                    if let Err(clash) = self.fit(types, reached, rhs, seen) {
                        return Some(Err(clash));
                    }
                }
                return Some(Ok(()));
            }
        }
        if let Type::Var(unknown) = right {
            if Self::level_of(types, lhs) <= types.level(unknown) {
                types.flow_in(unknown, lhs);
                for demanded in types.upper(unknown).to_vec() {
                    if let Err(clash) = self.fit(types, lhs, demanded, seen) {
                        return Some(Err(clash));
                    }
                }
                return Some(Ok(()));
            }
        }
        if let Type::Var(unknown) = left {
            let lowered = Self::extrude(types, rhs, types.level(unknown), false);
            return Some(self.fit(types, lhs, lowered, seen));
        }
        if let Type::Var(unknown) = right {
            let lowered = Self::extrude(types, lhs, types.level(unknown), true);
            return Some(self.fit(types, lowered, rhs, seen));
        }
        None
    }

    /// Everything with a shape. Functions pair up, a formation is usable as a
    /// function by filling its next void, and one shape fits another when it
    /// answers every attribute the other asks for — directly, or through its
    /// decoratee. Anything left over is a clash.
    fn shapes(
        &mut self,
        types: &mut Types,
        lhs: TypeId,
        rhs: TypeId,
        seen: &mut Seen,
    ) -> Result<(), Clash> {
        let (left, right) = (types.at(lhs), types.at(rhs));
        if let (
            Type::Fun {
                arg: given,
                ret: got,
            },
            Type::Fun {
                arg: wanted,
                ret: expected,
            },
        ) = (left, right)
        {
            self.fit(types, wanted, given, seen)?;
            return self.fit(types, got, expected, seen);
        }
        if let (Type::Rec(shape), Type::Fun { arg, ret }) = (left, right) {
            if let Some(head) = types.voids(shape).first().cloned() {
                let slot = types
                    .field(shape, &head)
                    .expect("a void without the attribute that holds it");
                self.fit(types, arg, slot, seen)?;
                let filled = self.apply(types, shape);
                let result = types.node(Type::Rec(filled));
                let result = if self.fragile && !types.voids(filled).is_empty() {
                    types.opt(result)
                } else {
                    result
                };
                return self.fit(types, result, ret, seen);
            }
            if let Some(decoratee) = types.field(shape, "@") {
                return self.fit(types, decoratee, rhs, seen);
            }
        }
        if let (Type::Rec(have), Type::Rec(want)) = (left, right) {
            for (label, wanted) in Self::attributes(types, want) {
                if let Some(mine) = types.field(have, &label) {
                    self.fit(types, mine, wanted, seen)?;
                } else if let Some(decoratee) = types.field(have, "@") {
                    let site = types.site(want).clone();
                    let need = self.need(types, &label, wanted, &site);
                    self.fit(types, decoratee, need, seen)
                        .map_err(|clash| clash.blamed(&label, lhs))?;
                } else {
                    return Err(Clash::UnknownAttribute {
                        attribute: label,
                        receiver: lhs,
                        site: types.site(want).clone(),
                    });
                }
            }
            return Ok(());
        }
        if let Type::Rec(want) = right {
            return Err(Clash::NoAttributes {
                value: lhs,
                wanted: Self::attributes(types, want)
                    .into_iter()
                    .map(|(label, _)| label)
                    .collect(),
            });
        }
        Err(Clash::Mismatch {
            value: lhs,
            wanted: rhs,
        })
    }

    fn copy(&mut self, types: &mut Types, limit: Level, ty: TypeId, memo: &mut Memo) -> TypeId {
        if types.alias(ty).is_some() || Self::level_of(types, ty) <= limit {
            return ty;
        }
        match types.at(ty) {
            Type::Var(unknown) => {
                if let Some(&done) = memo.get(&Ident::Var(unknown)) {
                    return done;
                }
                let fresh = types.var(self.level);
                let node = types.node(Type::Var(fresh));
                memo.insert(Ident::Var(unknown), node);
                for reached in types.lower(unknown).to_vec() {
                    let copied = self.copy(types, limit, reached, memo);
                    types.flow_in(fresh, copied);
                }
                for demanded in types.upper(unknown).to_vec() {
                    let copied = self.copy(types, limit, demanded, memo);
                    types.flow_out(fresh, copied);
                }
                node
            }
            Type::Rec(shape) => {
                if let Some(&done) = memo.get(&Ident::Rec(shape)) {
                    return done;
                }
                let fresh = types.shell(shape);
                let node = types.node(Type::Rec(fresh));
                memo.insert(Ident::Rec(shape), node);
                for (label, sub) in Self::attributes(types, shape) {
                    let copied = if label == "ρ" {
                        sub
                    } else {
                        self.copy(types, limit, sub, memo)
                    };
                    types.bind(fresh, &label, copied);
                }
                node
            }
            Type::Fun { arg, ret } => {
                let arg = self.copy(types, limit, arg, memo);
                let ret = self.copy(types, limit, ret, memo);
                types.fun(arg, ret)
            }
            Type::Opt(value) => {
                let value = self.copy(types, limit, value, memo);
                types.opt(value)
            }
        }
    }

    fn sink(
        types: &mut Types,
        ty: TypeId,
        level: Level,
        positive: bool,
        memo: &mut Polar,
    ) -> TypeId {
        if Self::level_of(types, ty) <= level {
            return ty;
        }
        match types.at(ty) {
            Type::Var(unknown) => {
                let key = (Ident::Var(unknown), positive);
                if let Some(&done) = memo.get(&key) {
                    return done;
                }
                let fresh = types.var(level);
                let node = types.node(Type::Var(fresh));
                memo.insert(key, node);
                if positive {
                    types.flow_out(unknown, node);
                    for reached in types.lower(unknown).to_vec() {
                        let lowered = Self::sink(types, reached, level, positive, memo);
                        types.flow_in(fresh, lowered);
                    }
                } else {
                    types.flow_in(unknown, node);
                    for demanded in types.upper(unknown).to_vec() {
                        let lowered = Self::sink(types, demanded, level, positive, memo);
                        types.flow_out(fresh, lowered);
                    }
                }
                node
            }
            Type::Rec(shape) => {
                let key = (Ident::Rec(shape), positive);
                if let Some(&done) = memo.get(&key) {
                    return done;
                }
                let fresh = types.shell(shape);
                let node = types.node(Type::Rec(fresh));
                memo.insert(key, node);
                for (label, sub) in Self::attributes(types, shape) {
                    let lowered = if label == "ρ" {
                        sub
                    } else {
                        Self::sink(types, sub, level, positive, memo)
                    };
                    types.bind(fresh, &label, lowered);
                }
                node
            }
            Type::Fun { arg, ret } => {
                let arg = Self::sink(types, arg, level, !positive, memo);
                let ret = Self::sink(types, ret, level, positive, memo);
                types.fun(arg, ret)
            }
            Type::Opt(value) => {
                let value = Self::sink(types, value, level, positive, memo);
                types.opt(value)
            }
        }
    }

    /// A single-attribute requirement, `{label: want}`, reused rather than
    /// rebuilt. Reuse is what lets a cyclic decoratee chain terminate: the same
    /// requirement comes back as the same record, so the knot-tying set
    /// recognises it instead of chasing fresh copies.
    fn need(&mut self, types: &mut Types, label: &str, want: TypeId, site: &Site) -> TypeId {
        let node = if let Some(&found) = self.needs.get(&(label.to_owned(), want)) {
            found
        } else {
            let shape = types.rec(Vec::new());
            types.bind(shape, label, want);
            let node = types.node(Type::Rec(shape));
            self.needs.insert((label.to_owned(), want), node);
            node
        };
        if let (true, Type::Rec(shape)) = (site.known(), types.at(node)) {
            types.mark(shape, site.clone());
        }
        node
    }

    /// The shape with its next void filled, reused rather than rebuilt, so a
    /// recursive application ties off instead of spinning on fresh copies.
    fn apply(&mut self, types: &mut Types, shape: RecId) -> RecId {
        let key = (shape, types.voids(shape).len());
        if let Some(&found) = self.applied.get(&key) {
            return found;
        }
        let filled = types.applied(shape);
        self.applied.insert(key, filled);
        filled
    }

    /// The deepest level appearing in a type.
    ///
    /// An unknown's own level is authoritative, since constraining keeps it at
    /// or above its bounds. Composites take the deepest of their parts, except
    /// that `ρ` — the parent link — does not count: it is navigation, shared
    /// rather than copied, so counting it would stop a record ever being lowered
    /// below a deep parent and constraining would never settle.
    #[must_use]
    pub fn level_of(types: &Types, ty: TypeId) -> Level {
        Self::depth(types, ty, &mut HashSet::new())
    }

    fn depth(types: &Types, ty: TypeId, seen: &mut HashSet<Ident>) -> Level {
        if let Type::Var(unknown) = types.at(ty) {
            return types.level(unknown);
        }
        if !seen.insert(Self::ident(types, ty)) {
            return Level::default();
        }
        match types.at(ty) {
            Type::Fun { arg, ret } => {
                Self::depth(types, arg, seen).max(Self::depth(types, ret, seen))
            }
            Type::Rec(shape) => types
                .labels(shape)
                .filter(|(label, _)| *label != "ρ")
                .map(|(_, sub)| sub)
                .collect::<Vec<_>>()
                .into_iter()
                .map(|sub| Self::depth(types, sub, seen))
                .max()
                .unwrap_or_default(),
            Type::Opt(value) => Self::depth(types, value, seen),
            Type::Var(_) => Level::default(),
        }
    }

    fn ident(types: &Types, ty: TypeId) -> Ident {
        match types.at(ty) {
            Type::Var(unknown) => Ident::Var(unknown),
            Type::Rec(shape) => Ident::Rec(shape),
            Type::Fun { .. } | Type::Opt(_) => Ident::Node(ty),
        }
    }

    fn attributes(types: &Types, shape: RecId) -> Vec<(String, TypeId)> {
        types
            .labels(shape)
            .map(|(label, ty)| (label.to_owned(), ty))
            .collect()
    }

    fn unfinished(types: &Types, ty: TypeId) -> bool {
        matches!(types.at(ty), Type::Rec(shape) if !types.voids(shape).is_empty())
    }

    fn bottom(types: &Types, value: TypeId, rhs: TypeId) -> Clash {
        if let (Type::Rec(half), Type::Rec(want)) = (types.at(value), types.at(rhs)) {
            if let (Some(unset), Some((attribute, _))) =
                (types.voids(half).first(), types.labels(want).next())
            {
                return Clash::IncompleteDispatch {
                    unset: unset.clone(),
                    attribute: attribute.to_owned(),
                    site: types.site(want).clone(),
                };
            }
        }
        Clash::NotRecovered
    }
}

impl Default for Solver {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{Clash, Solver};
    use crate::types::{Level, Site, Type, TypeId, Types};

    /// A ground shape standing in for a built-in, named the way the engine names
    /// one so that instantiation shares it.
    fn ground(types: &mut Types, alias: &str) -> TypeId {
        let shape = types.rec(Vec::new());
        let ty = types.node(Type::Rec(shape));
        types.name(ty, alias);
        ty
    }

    #[test]
    fn lets_a_base_type_be_used_as_itself() {
        let mut types = Types::default();
        let (given, wanted) = (ground(&mut types, "bytes"), ground(&mut types, "bytes"));
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Ok(()),
            "bytes dont fit where bytes is wanted"
        );
    }

    #[test]
    fn refuses_an_attribute_the_shape_does_not_have() {
        let mut types = Types::default();
        let empty = types.rec(Vec::new());
        let given = types.node(Type::Rec(empty));
        let asked = types.rec(Vec::new());
        let anything = ground(&mut types, "bytes");
        types.bind(asked, "plus", anything);
        let wanted = types.node(Type::Rec(asked));
        assert_eq!(
            Solver::default()
                .constrain(&mut types, given, wanted)
                .map_err(|clash| clash.code()),
            Err("type/unknown-attribute"),
            "dispatching an attribute nothing has dont fail"
        );
    }

    #[test]
    fn blames_the_shape_a_missing_attribute_was_asked_of() {
        let mut types = Types::default();
        let bare = types.rec(Vec::new());
        let bare = types.node(Type::Rec(bare));
        let decorated = types.rec(Vec::new());
        types.bind(decorated, "@", bare);
        let given = types.node(Type::Rec(decorated));
        let asked = types.rec(Vec::new());
        let anything = ground(&mut types, "bytes");
        types.bind(asked, "plus", anything);
        let wanted = types.node(Type::Rec(asked));
        assert_eq!(
            Solver::default()
                .constrain(&mut types, given, wanted)
                .err()
                .and_then(|clash| match clash {
                    Clash::UnknownAttribute { receiver, .. } => Some(receiver),
                    _ => None,
                }),
            Some(given),
            "a missing attribute dont get blamed on the shape it was asked of"
        );
    }

    #[test]
    fn falls_through_a_decoratee_to_find_an_attribute() {
        let mut types = Types::default();
        let inner = types.rec(Vec::new());
        let bytes = ground(&mut types, "bytes");
        types.bind(inner, "plus", bytes);
        let decorated = types.rec(Vec::new());
        let inner = types.node(Type::Rec(inner));
        types.bind(decorated, "@", inner);
        let given = types.node(Type::Rec(decorated));
        let asked = types.rec(Vec::new());
        let slot = types.var(Level::default());
        let slot = types.node(Type::Var(slot));
        types.bind(asked, "plus", slot);
        let wanted = types.node(Type::Rec(asked));
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Ok(()),
            "an attribute reached through the decoratee dont resolve"
        );
    }

    #[test]
    fn refuses_a_plain_dispatch_on_a_value_that_can_be_bottom() {
        let mut types = Types::default();
        let bytes = ground(&mut types, "bytes");
        let given = types.opt(bytes);
        let asked = types.rec(Vec::new());
        types.bind(asked, "plus", bytes);
        let wanted = types.node(Type::Rec(asked));
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Err(Clash::NotRecovered),
            "dispatching on a value that can be bottom dont fail"
        );
    }

    #[test]
    fn lets_a_definite_value_fit_where_bottom_is_allowed() {
        let mut types = Types::default();
        let given = ground(&mut types, "bytes");
        let inner = ground(&mut types, "bytes");
        let wanted = types.opt(inner);
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Ok(()),
            "a definite value dont fit where a maybe-bottom value is wanted"
        );
    }

    #[test]
    fn lets_a_value_that_can_be_bottom_flow_into_an_unknown() {
        let mut types = Types::default();
        let bytes = ground(&mut types, "bytes");
        let given = types.opt(bytes);
        let unknown = types.var(Level::default());
        let wanted = types.node(Type::Var(unknown));
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Ok(()),
            "a maybe-bottom value dont flow into an unknown unchallenged"
        );
    }

    #[test]
    fn remembers_what_reached_an_unknown() {
        let mut types = Types::default();
        let given = ground(&mut types, "bytes");
        let unknown = types.var(Level::default());
        let wanted = types.node(Type::Var(unknown));
        Solver::default()
            .constrain(&mut types, given, wanted)
            .expect("bytes cannot reach an unknown");
        assert_eq!(
            types.lower(unknown),
            [given],
            "the unknown dont remember the type that reached it"
        );
    }

    #[test]
    fn pushes_a_new_demand_through_to_what_already_reached_the_unknown() {
        let mut types = Types::default();
        let empty = types.rec(Vec::new());
        let given = types.node(Type::Rec(empty));
        let unknown = types.var(Level::default());
        let middle = types.node(Type::Var(unknown));
        let asked = types.rec(Vec::new());
        let bytes = ground(&mut types, "bytes");
        types.bind(asked, "plus", bytes);
        let wanted = types.node(Type::Rec(asked));
        let mut solver = Solver::default();
        solver
            .constrain(&mut types, given, middle)
            .expect("a shape cannot reach an unknown");
        assert_eq!(
            solver
                .constrain(&mut types, middle, wanted)
                .map_err(|clash| clash.code()),
            Err("type/unknown-attribute"),
            "a demand on an unknown dont reach what already flowed into it"
        );
    }

    #[test]
    fn takes_a_function_argument_the_opposite_way_round() {
        let mut types = Types::default();
        let narrow = types.rec(Vec::new());
        let bytes = ground(&mut types, "bytes");
        types.bind(narrow, "plus", bytes);
        let narrow = types.node(Type::Rec(narrow));
        let wide = types.rec(Vec::new());
        let wide = types.node(Type::Rec(wide));
        let given = types.fun(wide, bytes);
        let wanted = types.fun(narrow, bytes);
        assert_eq!(
            Solver::default().constrain(&mut types, given, wanted),
            Ok(()),
            "a function taking less dont fit where one taking more is wanted"
        );
    }

    #[test]
    fn applying_a_formation_fills_its_next_void() {
        let mut types = Types::default();
        let shape = types.rec(vec!["x".to_owned()]);
        let slot = types.var(Level::default());
        let slot = types.node(Type::Var(slot));
        types.bind(shape, "x", slot);
        let given = types.node(Type::Rec(shape));
        let argument = ground(&mut types, "bytes");
        let result = types.var(Level::default());
        let result = types.node(Type::Var(result));
        let wanted = types.fun(argument, result);
        Solver::default()
            .constrain(&mut types, given, wanted)
            .expect("a formation cannot be applied");
        let Type::Var(slot) = types.at(slot) else {
            unreachable!("the slot stopped being an unknown")
        };
        assert_eq!(
            types.lower(slot),
            [argument],
            "the argument dont reach the void it fills"
        );
    }

    #[test]
    fn reports_the_deepest_level_in_a_type() {
        let mut types = Types::default();
        let shallow = types.var(Level::default());
        let shallow = types.node(Type::Var(shallow));
        let deep = types.var(Level::default().deeper().deeper());
        let deep = types.node(Type::Var(deep));
        let both = types.fun(shallow, deep);
        assert_eq!(
            Solver::level_of(&types, both),
            Level::default().deeper().deeper(),
            "the deepest level in a type dont win"
        );
    }

    #[test]
    fn does_not_count_the_parent_link_towards_a_shapes_level() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let parent = types.var(Level::default().deeper().deeper());
        let parent = types.node(Type::Var(parent));
        types.bind(shape, "ρ", parent);
        let ty = types.node(Type::Rec(shape));
        assert_eq!(
            Solver::level_of(&types, ty),
            Level::default(),
            "the parent link dont stay out of the shape's level"
        );
    }

    #[test]
    fn makes_an_unknown_deeper_than_the_limit_fresh() {
        let mut types = Types::default();
        let inner = types.var(Level::default().deeper());
        let inner = types.node(Type::Var(inner));
        assert_ne!(
            Solver::default().freshen(&mut types, Level::default(), inner),
            inner,
            "an unknown deeper than the limit dont go fresh"
        );
    }

    #[test]
    fn shares_an_unknown_at_the_limit() {
        let mut types = Types::default();
        let context = types.var(Level::default());
        let context = types.node(Type::Var(context));
        assert_eq!(
            Solver::default().freshen(&mut types, Level::default(), context),
            context,
            "an unknown at the limit dont stay shared"
        );
    }

    #[test]
    fn shares_a_named_builtin_rather_than_copying_it() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let deep = types.var(Level::default().deeper().deeper());
        let deep = types.node(Type::Var(deep));
        types.bind(shape, "plus", deep);
        let number = types.node(Type::Rec(shape));
        types.name(number, "number");
        assert_eq!(
            Solver::default().freshen(&mut types, Level::default(), number),
            number,
            "a named builtin dont stay shared through instantiation"
        );
    }

    #[test]
    fn copies_a_cyclic_shape_once() {
        let mut types = Types::default();
        let host = types.var(Level::default().deeper());
        let shape = types.rec(Vec::new());
        types.tie(host, shape);
        let itself = types.node(Type::Var(host));
        types.bind(shape, "φ", itself);
        let ty = types.node(Type::Rec(shape));
        let fresh = Solver::default().freshen(&mut types, Level::default(), ty);
        let (Type::Rec(fresh), Type::Rec(_)) = (types.at(fresh), types.at(ty)) else {
            unreachable!("instantiating a shape gave something else")
        };
        assert_ne!(
            types.field(fresh, "φ"),
            Some(itself),
            "a cyclic shape dont get its own copy of the unknown inside it"
        );
    }

    #[test]
    fn links_a_lowered_unknown_back_to_the_one_it_came_from() {
        let mut types = Types::default();
        let deep = types.var(Level::default().deeper().deeper());
        let ty = types.node(Type::Var(deep));
        let lowered = Solver::extrude(&mut types, ty, Level::default(), true);
        assert_eq!(
            types.upper(deep),
            [lowered],
            "a lowered unknown dont stay linked to the one it came from"
        );
    }

    #[test]
    fn lowers_an_unknown_to_the_level_it_was_asked_for() {
        let mut types = Types::default();
        let deep = types.var(Level::default().deeper().deeper());
        let ty = types.node(Type::Var(deep));
        let lowered = Solver::extrude(&mut types, ty, Level::default(), false);
        assert_eq!(
            Solver::level_of(&types, lowered),
            Level::default(),
            "a lowered unknown dont land at the level it was asked for"
        );
    }

    #[test]
    fn flips_polarity_through_a_function_argument() {
        let mut types = Types::default();
        let deep = types.var(Level::default().deeper());
        let argument = types.node(Type::Var(deep));
        let bytes = ground(&mut types, "bytes");
        let ty = types.fun(argument, bytes);
        Solver::extrude(&mut types, ty, Level::default(), true);
        assert_eq!(
            types.upper(deep).len(),
            0,
            "an argument lowered on the way in dont take the opposite polarity"
        );
    }

    #[test]
    fn names_every_failure_with_a_stable_code() {
        assert_eq!(
            Clash::NotRecovered.code(),
            "type/not-recovered",
            "a failure dont carry the machine string a gate reads"
        );
    }

    #[test]
    fn tells_an_unbound_name_apart_from_a_broken_forma() {
        assert_eq!(
            Clash::Unbound {
                name: "whatever".to_owned(),
            }
            .code(),
            "ref/unbound-name",
            "a name that stands for nothing dont keep a code of its own"
        );
    }

    #[test]
    fn stops_a_build_over_a_shape_that_cannot_fit() {
        assert!(
            Clash::NotRecovered.gates(),
            "a value that can be bottom dont stop a build"
        );
    }

    #[test]
    fn spares_a_build_the_smell_of_a_half_built_object() {
        assert!(
            !Clash::IncompleteDispatch {
                unset: "uri".to_owned(),
                attribute: "separator".to_owned(),
                site: Site::default(),
            }
            .gates(),
            "dispatching on a half-built object dont pass the gate it only smells to"
        );
    }
}
