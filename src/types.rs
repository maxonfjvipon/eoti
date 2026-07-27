//! The type vocabulary.
//!
//! A type is a shape, not a name. An unknown carries an MLsub level and the
//! bounds that flow through it, `bytes` is the single base type, a record holds
//! its fields and its ordered voids, and an option is a value that may be `⊥`.
//!
//! Every type lives in one arena, [`Types`], and is named by a small `Copy`
//! handle into it. The graph the arena holds is cyclic — an object's attribute
//! can be the object itself — and it is mutated while being walked, since a
//! formation publishes its record before inferring the body that fills it.
//! Handles are what make that expressible: a cycle is an integer, so nothing
//! owns anything twice, and each window of mutation provably closes before the
//! next descent.

/// How local an unknown is. Deeper means more local, which is what lets a
/// reused object's parameters go fresh per use while its recursion and context
/// stay shared.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Level(usize);

impl Level {
    /// One step more local, as a formation's body is to its definition.
    #[must_use]
    pub fn deeper(self) -> Self {
        Self(self.0 + 1)
    }

    /// One step less local, or nothing at all at the outermost level.
    #[must_use]
    pub fn shallower(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }
}

/// A type in the arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(usize);

/// An unknown in the arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct VarId(usize);

/// A record in the arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RecId(usize);

/// A table of attributes in the arena. Records name one rather than owning one,
/// because filling a void yields a shape with the same attributes and one fewer
/// slot, and the two must stay the same table: the original may still be gaining
/// attributes while the reduced one is already in use.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FieldsId(usize);

/// A type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Type {
    /// An unknown, collecting the bounds that flow through it.
    Var(VarId),
    /// Fill the parameter, get the result: a formation with a void attribute.
    Fun {
        /// What fills the void.
        arg: TypeId,
        /// What comes back once it is filled.
        ret: TypeId,
    },
    /// An object shape.
    Rec(RecId),
    /// `T?` — the value, or `⊥`.
    Opt(TypeId),
}

/// What flows through an unknown: the types that reach it, and the slots it is
/// asked to fit.
#[derive(Debug, Default)]
struct Bounds {
    lower: Vec<TypeId>,
    upper: Vec<TypeId>,
}

/// An unknown.
#[derive(Debug)]
struct VarData {
    level: Level,
    bounds: Bounds,
    knot: Option<RecId>,
    alias: Option<String>,
}

/// A record's attributes, in the order they were bound. Order is kept because
/// it is the order a shape reads back in, and a record is small enough that a
/// scan beats a hash.
#[derive(Debug, Default)]
struct Fields(Vec<(String, TypeId)>);

impl Fields {
    fn at(&self, label: &str) -> Option<TypeId> {
        self.0
            .iter()
            .find(|(name, _)| name == label)
            .map(|&(_, ty)| ty)
    }

    fn bind(&mut self, label: &str, ty: TypeId) {
        if let Some(slot) = self.0.iter_mut().find(|(name, _)| name == label) {
            slot.1 = ty;
        } else {
            self.0.push((label.to_owned(), ty));
        }
    }
}

/// Where a shape came from, and the level it was defined at — which a use site
/// needs in order to know what to make fresh.
#[derive(Clone, Debug, Default)]
struct Origin {
    site: Site,
    level: Option<Level>,
}

/// A place in a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    /// The line it was written on.
    pub line: u32,
    /// How far into the line.
    pub pos: u32,
}

/// Where something was written: a place in a file, and the stable locator of
/// the object in the graph. Both are optional, because not every input carries
/// them — and the locator is the one that survives reformatting, which is what
/// lets a consumer cache a fact about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    /// Where in the file, if the reader was told.
    pub at: Option<Spot>,
    /// Which object in the graph, if it has a locator.
    pub loc: Option<String>,
}

impl Site {
    /// Whether anything at all is known about where something was written.
    ///
    /// Worth asking before recording it over a place already known: an input
    /// that carries no position must not blank one that does.
    #[must_use]
    pub fn known(&self) -> bool {
        self.at.is_some() || self.loc.is_some()
    }
}

/// An object shape.
#[derive(Debug)]
struct RecData {
    fields: FieldsId,
    voids: Vec<String>,
    alias: Option<String>,
    origin: Origin,
}

/// The arena every type lives in.
///
/// Handles are only meaningful to the arena that made them. Mixing arenas is a
/// programming error and fails fast rather than reading someone else's type.
#[derive(Debug, Default)]
pub struct Types {
    nodes: Vec<Type>,
    vars: Vec<VarData>,
    recs: Vec<RecData>,
    tables: Vec<Fields>,
}

impl Types {
    /// Put a type in the arena.
    pub fn node(&mut self, ty: Type) -> TypeId {
        self.nodes.push(ty);
        TypeId(self.nodes.len() - 1)
    }

    /// The type behind a handle.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn at(&self, id: TypeId) -> Type {
        *self.nodes.get(id.0).expect("no such type in this arena")
    }

    /// Fill the parameter, get the result.
    pub fn fun(&mut self, arg: TypeId, ret: TypeId) -> TypeId {
        self.node(Type::Fun { arg, ret })
    }

    /// The value, or `⊥`.
    pub fn opt(&mut self, inner: TypeId) -> TypeId {
        self.node(Type::Opt(inner))
    }

    /// A fresh unknown at a level.
    pub fn var(&mut self, level: Level) -> VarId {
        self.vars.push(VarData {
            level,
            bounds: Bounds::default(),
            knot: None,
            alias: None,
        });
        VarId(self.vars.len() - 1)
    }

    /// A fresh record expecting these voids, in application order.
    pub fn rec(&mut self, voids: Vec<String>) -> RecId {
        let fields = self.table();
        self.hold(RecData {
            fields,
            voids,
            alias: None,
            origin: Origin::default(),
        })
    }

    /// The same shape with no attributes yet: its name, its slots and where it
    /// came from, ready for a copy to fill.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn shell(&mut self, like: RecId) -> RecId {
        let fields = self.table();
        let (voids, alias, origin) = {
            let data = self.rec_at(like);
            (data.voids.clone(), data.alias.clone(), data.origin.clone())
        };
        self.hold(RecData {
            fields,
            voids,
            alias,
            origin,
        })
    }

    /// The same shape with its first void filled: one fewer slot, and the very
    /// same attributes, so whatever the original gains later is seen here too.
    ///
    /// It keeps no origin. The result is a shape produced by applying, not one
    /// anybody wrote, so it belongs to no definition level — and claiming one
    /// would raise the limit that instantiation compares against, making a use
    /// site share the formation it should have copied.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena, or the shape has no void left.
    pub fn applied(&mut self, rec: RecId) -> RecId {
        let (fields, voids, alias) = {
            let data = self.rec_at(rec);
            assert!(!data.voids.is_empty(), "the shape has no void left to fill");
            (data.fields, data.voids[1..].to_vec(), data.alias.clone())
        };
        self.hold(RecData {
            fields,
            voids,
            alias,
            origin: Origin::default(),
        })
    }

    /// Where in the source a shape was written, when it stands for a
    /// requirement that can be pointed at.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn site(&self, rec: RecId) -> &Site {
        &self.rec_at(rec).origin.site
    }

    /// Say where a shape was written.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn mark(&mut self, rec: RecId, site: Site) {
        self.rec_mut(rec).origin.site = site;
    }

    /// The level a formation was defined at. Instantiation makes everything
    /// deeper than this fresh, and shares everything at or above it.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn defined_at(&self, rec: RecId) -> Option<Level> {
        self.rec_at(rec).origin.level
    }

    /// Say what level a formation was defined at.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn define(&mut self, rec: RecId, level: Level) {
        self.rec_mut(rec).origin.level = Some(level);
    }

    /// How local an unknown is.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn level(&self, var: VarId) -> Level {
        self.var_at(var).level
    }

    /// What has flowed into an unknown.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn lower(&self, var: VarId) -> &[TypeId] {
        &self.var_at(var).bounds.lower
    }

    /// The slots an unknown is asked to fit.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn upper(&self, var: VarId) -> &[TypeId] {
        &self.var_at(var).bounds.upper
    }

    /// Record that a type flows into an unknown.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn flow_in(&mut self, var: VarId, ty: TypeId) {
        self.var_mut(var).bounds.lower.push(ty);
    }

    /// Record that an unknown must fit a slot.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn flow_out(&mut self, var: VarId, ty: TypeId) {
        self.var_mut(var).bounds.upper.push(ty);
    }

    /// The record an unknown stands for, once it is known to be an object.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn knot(&self, var: VarId) -> Option<RecId> {
        self.var_at(var).knot
    }

    /// Say that an unknown stands for a record, so the body being inferred can
    /// refer to the object it is defining.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn tie(&mut self, var: VarId, rec: RecId) {
        self.var_mut(var).knot = Some(rec);
    }

    /// The type of one attribute of a record.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn field(&self, rec: RecId, label: &str) -> Option<TypeId> {
        self.table_at(rec).at(label)
    }

    /// Give a record an attribute, replacing it if the label is taken.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn bind(&mut self, rec: RecId, label: &str, ty: TypeId) {
        let table = self.rec_at(rec).fields;
        self.tables
            .get_mut(table.0)
            .expect("no such attribute table in this arena")
            .bind(label, ty);
    }

    /// Every attribute of a record, in the order they were bound.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    pub fn labels(&self, rec: RecId) -> impl Iterator<Item = (&str, TypeId)> {
        self.table_at(rec)
            .0
            .iter()
            .map(|(label, ty)| (label.as_str(), *ty))
    }

    /// The voids a record still expects, in application order.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn voids(&self, rec: RecId) -> &[String] {
        &self.rec_at(rec).voids
    }

    /// Give a built-in its display name. An aliased type is shown by that name
    /// rather than by its shape, and is shared rather than copied when a use
    /// site instantiates it.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena, or names something that cannot
    /// carry a name.
    pub fn name(&mut self, id: TypeId, alias: &str) {
        match self.at(id) {
            Type::Var(var) => self.var_mut(var).alias = Some(alias.to_owned()),
            Type::Rec(rec) => self.rec_mut(rec).alias = Some(alias.to_owned()),
            other => panic!("cannot name {other:?}, only an unknown or a shape carries a name"),
        }
    }

    /// The display name of a built-in, if it has one.
    ///
    /// # Panics
    ///
    /// If the handle was not made by this arena.
    #[must_use]
    pub fn alias(&self, id: TypeId) -> Option<&str> {
        match self.at(id) {
            Type::Var(var) => self.var_at(var).alias.as_deref(),
            Type::Rec(rec) => self.rec_at(rec).alias.as_deref(),
            _ => None,
        }
    }

    fn var_at(&self, var: VarId) -> &VarData {
        self.vars.get(var.0).expect("no such unknown in this arena")
    }

    fn var_mut(&mut self, var: VarId) -> &mut VarData {
        self.vars
            .get_mut(var.0)
            .expect("no such unknown in this arena")
    }

    fn rec_at(&self, rec: RecId) -> &RecData {
        self.recs.get(rec.0).expect("no such shape in this arena")
    }

    fn rec_mut(&mut self, rec: RecId) -> &mut RecData {
        self.recs
            .get_mut(rec.0)
            .expect("no such shape in this arena")
    }

    fn table_at(&self, rec: RecId) -> &Fields {
        self.tables
            .get(self.rec_at(rec).fields.0)
            .expect("no such attribute table in this arena")
    }

    fn table(&mut self) -> FieldsId {
        self.tables.push(Fields::default());
        FieldsId(self.tables.len() - 1)
    }

    fn hold(&mut self, data: RecData) -> RecId {
        self.recs.push(data);
        RecId(self.recs.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, Site, Spot, Type, TypeId, Types};

    /// A ground shape standing in for a built-in, named the way the engine names
    /// one so that instantiation shares it.
    fn ground(types: &mut Types, alias: &str) -> TypeId {
        let shape = types.rec(Vec::new());
        let ty = types.node(Type::Rec(shape));
        types.name(ty, alias);
        ty
    }

    #[test]
    fn gives_every_unknown_its_own_identity() {
        let mut types = Types::default();
        assert_ne!(
            types.var(Level::default()),
            types.var(Level::default()),
            "two fresh unknowns dont differ"
        );
    }

    #[test]
    fn a_body_is_more_local_than_its_definition() {
        assert!(
            Level::default().deeper() > Level::default(),
            "a formation body dont sit deeper than the definition around it"
        );
    }

    #[test]
    fn remembers_what_flowed_into_an_unknown() {
        let mut types = Types::default();
        let unknown = types.var(Level::default().deeper());
        let bytes = ground(&mut types, "bytes");
        types.flow_in(unknown, bytes);
        assert_eq!(
            types.lower(unknown),
            [bytes],
            "the unknown dont remember what flowed into it"
        );
    }

    #[test]
    fn keeps_the_slots_an_unknown_must_fit_apart_from_what_reaches_it() {
        let mut types = Types::default();
        let unknown = types.var(Level::default());
        let bytes = ground(&mut types, "bytes");
        types.flow_out(unknown, bytes);
        assert!(
            types.lower(unknown).is_empty(),
            "a slot the unknown must fit dont stay out of what reached it"
        );
    }

    #[test]
    fn keeps_attributes_in_the_order_they_were_bound() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let bytes = ground(&mut types, "bytes");
        for label in ["zebra", "aardvark", "μ"] {
            types.bind(shape, label, bytes);
        }
        assert_eq!(
            types
                .labels(shape)
                .map(|(label, _)| label)
                .collect::<Vec<_>>(),
            ["zebra", "aardvark", "μ"],
            "the shape dont keep its attributes in the order they were bound"
        );
    }

    #[test]
    fn replaces_an_attribute_bound_twice() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let bytes = ground(&mut types, "bytes");
        let later = types.opt(bytes);
        types.bind(shape, "φ", bytes);
        types.bind(shape, "φ", later);
        assert_eq!(
            types.field(shape, "φ"),
            Some(later),
            "binding an attribute twice dont replace the first one"
        );
    }

    #[test]
    fn holds_an_object_whose_attribute_is_the_object_itself() {
        let mut types = Types::default();
        let host = types.var(Level::default());
        let shape = types.rec(Vec::new());
        types.tie(host, shape);
        let itself = types.node(Type::Var(host));
        types.bind(shape, "φ", itself);
        assert_eq!(
            types.field(shape, "φ"),
            Some(itself),
            "the arena dont hold an object that contains itself"
        );
    }

    #[test]
    fn reaches_a_record_back_through_the_unknown_that_hosts_it() {
        let mut types = Types::default();
        let host = types.var(Level::default());
        let shape = types.rec(vec!["x".to_owned()]);
        types.tie(host, shape);
        assert_eq!(
            types.knot(host).map(|rec| types.voids(rec)),
            Some(&["x".to_owned()][..]),
            "the unknown dont reach the shape tied to it"
        );
    }

    #[test]
    fn shows_a_named_builtin_by_its_name() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let number = types.node(Type::Rec(shape));
        types.name(number, "number");
        assert_eq!(
            types.alias(number),
            Some("number"),
            "a named builtin dont carry the name it was given"
        );
    }

    #[test]
    fn applying_a_shape_drops_its_first_slot() {
        let mut types = Types::default();
        let shape = types.rec(vec!["head".to_owned(), "tail".to_owned()]);
        let filled = types.applied(shape);
        assert_eq!(
            types.voids(filled),
            ["tail".to_owned()],
            "filling a slot dont leave the rest of them"
        );
    }

    #[test]
    fn an_applied_shape_sees_attributes_the_original_gains_later() {
        let mut types = Types::default();
        let shape = types.rec(vec!["x".to_owned()]);
        let filled = types.applied(shape);
        let bytes = ground(&mut types, "bytes");
        types.bind(shape, "later", bytes);
        assert_eq!(
            types.field(filled, "later"),
            Some(bytes),
            "an applied shape dont share the attributes of the shape it came from"
        );
    }

    #[test]
    fn an_applied_shape_claims_no_definition_level() {
        let mut types = Types::default();
        let shape = types.rec(vec!["x".to_owned()]);
        types.define(shape, Level::default().deeper());
        let filled = types.applied(shape);
        assert_eq!(
            types.defined_at(filled),
            None,
            "an applied shape dont drop the definition level of the shape it came from"
        );
    }

    #[test]
    fn a_shell_keeps_the_slots_but_none_of_the_attributes() {
        let mut types = Types::default();
        let shape = types.rec(vec!["x".to_owned()]);
        let bytes = ground(&mut types, "bytes");
        types.bind(shape, "φ", bytes);
        let empty = types.shell(shape);
        assert_eq!(
            (types.voids(empty).len(), types.field(empty, "φ")),
            (1, None),
            "a shell dont keep the slots while dropping the attributes"
        );
    }

    #[test]
    fn remembers_where_a_requirement_was_written() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        types.mark(
            shape,
            Site {
                at: Some(Spot { line: 41, pos: 7 }),
                loc: Some("Φ.somewhere".to_owned()),
            },
        );
        let copy = types.shell(shape);
        assert_eq!(
            types.site(copy).loc.as_deref(),
            Some("Φ.somewhere"),
            "a copied shape dont remember which object the original stood for"
        );
    }

    #[test]
    fn knows_nothing_about_a_place_nobody_recorded() {
        assert!(
            !Site::default().known(),
            "a place nobody recorded dont admit it knows nothing"
        );
    }

    #[test]
    fn knows_a_place_named_by_its_locator_alone() {
        assert!(
            Site {
                at: None,
                loc: Some("Φ.somewhere".to_owned()),
            }
            .known(),
            "a place known only by its locator dont count as known"
        );
    }

    #[test]
    fn leaves_an_unnamed_shape_without_a_name() {
        let mut types = Types::default();
        let anonymous = types.rec(Vec::new());
        let shape = types.node(Type::Rec(anonymous));
        assert_eq!(
            types.alias(shape),
            None,
            "an unnamed shape dont stay without a name"
        );
    }
}
