//! Forma resolution through `@loc`.
//!
//! A return type such as `Φ.posix.return` is also a locator into the object
//! graph, so a non-primitive forma can be resolved to the shape of the object it
//! names instead of staying opaque. Resolution is memoized, cycle-guarded and
//! best-effort: what cannot be typed in isolation falls back to opaque rather
//! than failing.

use std::collections::HashMap;

use crate::infer::{Engine, Env};
use crate::solver::Clash;
use crate::types::{Level, Type, TypeId};
use crate::xmir::Node;

/// What is known about locators: the top-level objects to resolve through, what
/// has already been resolved, and what is being resolved right now.
#[derive(Debug, Default)]
pub struct Locs {
    toplevel: HashMap<String, Node>,
    resolved: HashMap<String, Option<TypeId>>,
    underway: HashMap<String, TypeId>,
}

impl Locs {
    /// Take in an index of top-level objects by locator.
    pub fn learn(&mut self, found: HashMap<String, Node>) {
        self.toplevel.extend(found);
    }

    /// How many locators are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.toplevel.len()
    }

    /// Whether nothing is known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.toplevel.is_empty()
    }

    /// The top-level ancestor of a locator and the path down to it. A nested
    /// object is reached *through* its ancestor, because inferred there its
    /// parent chain resolves, which it cannot in isolation. The longest prefix
    /// wins.
    fn ancestor(&self, loc: &str) -> Option<(Node, Vec<String>)> {
        let segments: Vec<&str> = loc.split('.').collect();
        for cut in (1..=segments.len()).rev() {
            if let Some(found) = self.toplevel.get(&segments[..cut].join(".")) {
                return Some((
                    found.clone(),
                    segments[cut..]
                        .iter()
                        .map(|seg| (*seg).to_owned())
                        .collect(),
                ));
            }
        }
        None
    }
}

impl Engine {
    /// The shape a forma names, or nothing at all.
    ///
    /// Inferred once and remembered, then instantiated per use so a shared
    /// resolution neither costs a re-inference nor gets over-constrained by the
    /// first place that used it. A self-referential object ties the knot through
    /// a shared unknown. Best-effort throughout: anything that cannot be typed
    /// in isolation resolves to nothing, so resolution never turns a passing
    /// check into a failing one.
    pub fn resolve(&mut self, loc: &str) -> Option<TypeId> {
        if let Some(&knot) = self.locs.underway.get(loc) {
            return Some(knot);
        }
        if !self.locs.resolved.contains_key(loc) {
            let (top, path) = self.locs.ancestor(loc)?;
            let save = self.solver.jump(Level::default());
            let unknown = self.types.var(Level::default().deeper());
            let knot = self.types.node(Type::Var(unknown));
            self.locs.underway.insert(loc.to_owned(), knot);
            let built = self.walk(&top, &path, knot);
            self.locs.underway.remove(loc);
            self.solver.jump(save);
            self.locs.resolved.insert(loc.to_owned(), built.ok());
        }
        let found = *self.locs.resolved.get(loc)?;
        found.map(|ty| self.solver.freshen(&mut self.types, Level::default(), ty))
    }

    /// Infer the ancestor, then walk down to the object the locator names.
    fn walk(&mut self, top: &Node, path: &[String], knot: TypeId) -> Result<TypeId, Clash> {
        let mut ty = self.infer(top, &Env::new())?;
        for seg in path {
            ty = self.project(ty, seg)?;
        }
        self.solver.constrain(&mut self.types, ty, knot)?;
        Ok(ty)
    }
}
