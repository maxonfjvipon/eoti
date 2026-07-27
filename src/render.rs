//! The pretty-printer.
//!
//! Types in named-binder form: every variable named once, the single-use ones
//! inlined, the rest bound in a trailing `where` clause, and whatever stays open
//! quantified up front.
//!
//! Naming every variable once is not cosmetic. Rendering a large shared or
//! recursive shape inline re-renders its shared parts over and over, which grows
//! exponentially; naming them keeps it linear.

use std::collections::{HashMap, HashSet};

use crate::solver::Clash;
use crate::types::{Type, TypeId, Types, VarId};

/// The character that brackets a placeholder while a type is being laid out. It
/// cannot occur in a type, so it never collides with real output.
const MARK: char = '\u{1}';

/// A type, rendered for a person to read.
#[must_use]
pub fn show(types: &Types, ty: TypeId) -> String {
    Printer {
        types,
        order: Vec::new(),
        slots: HashMap::new(),
        counts: Vec::new(),
    }
    .whole(ty)
}

/// A failure, said in one sentence.
///
/// The solver deliberately builds no prose, so this is where a clash becomes
/// something a person reads. It stays one sentence with the context in it and no
/// trailing stop, because a consumer may put it anywhere.
#[must_use]
pub fn explain(types: &Types, clash: &Clash) -> String {
    match clash {
        Clash::UnknownAttribute {
            attribute,
            receiver,
            ..
        } => format!("no attribute `{attribute}` on {}", show(types, *receiver)),
        Clash::NotRecovered => {
            "value can be ⊥, so it has to be recovered before anything is dispatched on it"
                .to_owned()
        }
        Clash::IncompleteDispatch {
            unset, attribute, ..
        } => format!(
            "attribute `{unset}` is not set, so `{attribute}` cannot be dispatched on this object"
        ),
        Clash::NoAttributes { value, wanted } => format!(
            "{} has no attributes, but {} was wanted",
            show(types, *value),
            wanted
                .iter()
                .map(|label| format!("`{label}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Clash::Mismatch { value, wanted } => format!(
            "cannot use {} as {}",
            show(types, *value),
            show(types, *wanted)
        ),
        Clash::Unbound { name } => format!("nothing is known about `{name}`"),
    }
}

/// A rendering in progress.
struct Printer<'a> {
    types: &'a Types,
    order: Vec<VarId>,
    slots: HashMap<VarId, usize>,
    counts: Vec<usize>,
}

impl Printer<'_> {
    fn whole(mut self, ty: TypeId) -> String {
        let top = self.layout(ty);
        let mut defs: Vec<Option<String>> = Vec::new();
        while defs.len() < self.order.len() {
            let unknown = self.order[defs.len()];
            let reached = self.types.lower(unknown).to_vec();
            let demanded = self.types.upper(unknown).to_vec();
            let (bounds, joiner) = if reached.is_empty() {
                (demanded, " & ")
            } else {
                (reached, " | ")
            };
            if bounds.is_empty() {
                defs.push(None);
                continue;
            }
            let mut parts = Vec::new();
            for bound in bounds {
                let text = self.layout(bound);
                if !parts.contains(&text) {
                    parts.push(text);
                }
            }
            defs.push(Some(parts.join(joiner)));
        }
        Binders::new(defs, &self.counts).finish(&top)
    }

    /// One type as text, with a placeholder wherever a variable appears.
    fn layout(&mut self, ty: TypeId) -> String {
        let types = self.types;
        match types.at(ty) {
            Type::Opt(value) => format!("{}?", self.layout(value)),
            Type::Fun { arg, ret } => {
                format!("({} -> {})", self.layout(arg), self.layout(ret))
            }
            Type::Var(unknown) => types
                .alias(ty)
                .map_or_else(|| self.refer(unknown), str::to_owned),
            Type::Rec(shape) => {
                if let Some(alias) = types.alias(ty) {
                    return alias.to_owned();
                }
                let voids = types.voids(shape).to_vec();
                let fields: Vec<(String, TypeId)> = types
                    .labels(shape)
                    .filter(|(label, _)| *label != "ρ" && !voids.iter().any(|void| void == label))
                    .map(|(label, sub)| (label.to_owned(), sub))
                    .collect();
                let mut parts = Vec::new();
                for (label, sub) in fields {
                    parts.push(format!("{label}: {}", self.layout(sub)));
                }
                let mut body = format!("{{{}}}", parts.join(", "));
                for void in voids.iter().rev() {
                    let slot = types
                        .field(shape, void)
                        .expect("a void without the attribute that holds it");
                    body = format!("({} -> {body})", self.layout(slot));
                }
                body
            }
        }
    }

    /// The placeholder for a variable, counting how often it is mentioned.
    fn refer(&mut self, unknown: VarId) -> String {
        let slot = if let Some(&found) = self.slots.get(&unknown) {
            found
        } else {
            let made = self.order.len();
            self.slots.insert(unknown, made);
            self.order.push(unknown);
            self.counts.push(0);
            made
        };
        self.counts[slot] += 1;
        format!("{MARK}{slot}{MARK}")
    }
}

/// Deciding which variables to inline and what to call the rest.
struct Binders {
    defs: Vec<Option<String>>,
    inline: HashSet<usize>,
    letters: HashMap<usize, String>,
}

impl Binders {
    fn new(defs: Vec<Option<String>>, counts: &[usize]) -> Self {
        let edges: Vec<HashSet<usize>> = defs
            .iter()
            .map(|def| def.as_deref().map(placeholders).unwrap_or_default())
            .collect();
        let inline = (0..defs.len())
            .filter(|&slot| {
                defs[slot].is_some()
                    && counts.get(slot).copied().unwrap_or(0) <= 1
                    && !recursive(&edges, slot)
            })
            .collect::<HashSet<_>>();
        let mut letters = HashMap::new();
        for slot in 0..defs.len() {
            if !inline.contains(&slot) {
                let next = letters.len();
                letters.insert(slot, name_of(next));
            }
        }
        Self {
            defs,
            inline,
            letters,
        }
    }

    fn finish(&self, top: &str) -> String {
        let alone = only(top).filter(|slot| self.inline.contains(slot));
        let body = match alone {
            Some(slot) => self.expand(self.defs[slot].as_deref().unwrap_or_default()),
            None => self.expand(top),
        };
        let clauses = (0..self.defs.len())
            .filter(|slot| self.defs[*slot].is_some() && !self.inline.contains(slot))
            .map(|slot| {
                format!(
                    "{} = {}",
                    self.letters[&slot],
                    self.expand(self.defs[slot].as_deref().unwrap_or_default())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let open = (0..self.defs.len())
            .filter(|slot| self.defs[*slot].is_none())
            .map(|slot| self.letters[&slot].clone())
            .collect::<Vec<_>>();
        let result = if clauses.is_empty() {
            body
        } else {
            format!("{body} where {clauses}")
        };
        if open.is_empty() {
            result
        } else {
            format!("∀{}. {result}", open.join(" "))
        }
    }

    /// Replace each placeholder by its letter, or by its definition when it is
    /// inlined. A joined definition is parenthesized so `A | B` inside a
    /// function does not read as part of the arrow; functions already bracket
    /// themselves.
    fn expand(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some((before, slot, after)) = next_placeholder(rest) {
            out.push_str(before);
            if self.inline.contains(&slot) {
                let inner = self.expand(self.defs[slot].as_deref().unwrap_or_default());
                if inner.contains(" | ") || inner.contains(" & ") {
                    out.push('(');
                    out.push_str(&inner);
                    out.push(')');
                } else {
                    out.push_str(&inner);
                }
            } else {
                out.push_str(&self.letters[&slot]);
            }
            rest = after;
        }
        out.push_str(rest);
        out
    }
}

/// The letters variables are called, running past `Z` by adding a number.
fn name_of(nth: usize) -> String {
    let letter = char::from(b'A' + u8::try_from(nth % 26).unwrap_or(0));
    if nth < 26 {
        letter.to_string()
    } else {
        format!("{letter}{}", nth / 26)
    }
}

/// The next placeholder in a string, as what came before it, which slot it is,
/// and what comes after.
fn next_placeholder(text: &str) -> Option<(&str, usize, &str)> {
    let open = text.find(MARK)?;
    let rest = &text[open + MARK.len_utf8()..];
    let close = rest.find(MARK)?;
    let slot = rest[..close].parse().ok()?;
    Some((&text[..open], slot, &rest[close + MARK.len_utf8()..]))
}

/// Every slot mentioned in a string.
fn placeholders(text: &str) -> HashSet<usize> {
    let mut found = HashSet::new();
    let mut rest = text;
    while let Some((_, slot, after)) = next_placeholder(rest) {
        found.insert(slot);
        rest = after;
    }
    found
}

/// The one slot a string consists of, when it is nothing but a placeholder.
fn only(text: &str) -> Option<usize> {
    let (before, slot, after) = next_placeholder(text)?;
    (before.is_empty() && after.is_empty()).then_some(slot)
}

/// Whether a slot's definition eventually mentions the slot itself.
fn recursive(edges: &[HashSet<usize>], slot: usize) -> bool {
    let mut stack: Vec<usize> = edges.get(slot).into_iter().flatten().copied().collect();
    let mut seen = HashSet::new();
    while let Some(next) = stack.pop() {
        if next == slot {
            return true;
        }
        if seen.insert(next) {
            stack.extend(edges.get(next).into_iter().flatten().copied());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::show;
    use crate::types::{Level, Type, Types};

    #[test]
    fn shows_a_named_builtin_by_its_name() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let number = types.node(Type::Rec(shape));
        types.name(number, "number");
        assert_eq!(
            show(&types, number),
            "number",
            "a named builtin dont print as its name"
        );
    }

    #[test]
    fn quantifies_a_variable_nothing_is_known_about() {
        let mut types = Types::default();
        let unknown = types.var(Level::default());
        let ty = types.node(Type::Var(unknown));
        assert_eq!(
            show(&types, ty),
            "∀A. A",
            "a variable nothing is known about dont get quantified"
        );
    }

    #[test]
    fn inlines_a_variable_used_once() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let number = types.node(Type::Rec(shape));
        types.name(number, "number");
        let unknown = types.var(Level::default());
        types.flow_in(unknown, number);
        let ty = types.node(Type::Var(unknown));
        assert_eq!(
            show(&types, ty),
            "number",
            "a variable used once dont get inlined into its own definition"
        );
    }

    #[test]
    fn joins_everything_that_reached_a_variable() {
        let mut types = Types::default();
        let one = types.rec(Vec::new());
        let one = types.node(Type::Rec(one));
        types.name(one, "number");
        let other = types.rec(Vec::new());
        let other = types.node(Type::Rec(other));
        types.name(other, "string");
        let unknown = types.var(Level::default());
        types.flow_in(unknown, one);
        types.flow_in(unknown, other);
        let ty = types.node(Type::Var(unknown));
        assert_eq!(
            show(&types, ty),
            "number | string",
            "the things that reached a variable dont join"
        );
    }

    #[test]
    fn keeps_a_recursive_variable_in_a_where_clause() {
        let mut types = Types::default();
        let shape = types.rec(Vec::new());
        let unknown = types.var(Level::default());
        let ty = types.node(Type::Var(unknown));
        types.bind(shape, "tail", ty);
        let whole = types.node(Type::Rec(shape));
        types.flow_in(unknown, whole);
        assert_eq!(
            show(&types, ty),
            "A where A = {tail: A}",
            "a recursive variable dont stay named in a where clause"
        );
    }

    #[test]
    fn shows_a_void_as_something_to_fill() {
        let mut types = Types::default();
        let shape = types.rec(vec!["x".to_owned()]);
        let slot = types.rec(Vec::new());
        let slot = types.node(Type::Rec(slot));
        types.name(slot, "number");
        types.bind(shape, "x", slot);
        types.bind(shape, "@", slot);
        let ty = types.node(Type::Rec(shape));
        assert_eq!(
            show(&types, ty),
            "(number -> {@: number})",
            "a void dont print as something the shape still wants"
        );
    }
}
