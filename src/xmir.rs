//! The reader for EO's `1-parse` XMIR.
//!
//! It understands the `<o>` vocabulary the parser emits: formations and their
//! attributes, voids, decoratees, dispatches, global references, application
//! arguments, and the type annotations an atom declares about itself.
//!
//! The parser emits φ-calculus normal form, in which a formation is an `<o>`
//! with no `base` and its children are its attributes; a void is
//! `base="∅"`; the decoratee is the child named `φ`; arguments carry
//! `as="αN"` and a dispatch `base=".m"` takes its one child without `as` as
//! the receiver; bases are scope-qualified as `Φ.x`, `ξ.x` or `ξ.ρ.x`; and a
//! literal is a primitive constructor applied to raw hex text.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use roxmltree::{Document, Node as Elem};

use crate::types::{Site, Spot};

/// One node of the abstract syntax tree: the four moves, the literals, and the
/// two failure forms.
#[derive(Clone, Debug)]
pub enum Node {
    /// A name, looked up in scope and then among the built-ins.
    Name(String),
    /// A number literal.
    Num,
    /// A string literal.
    Str,
    /// Raw data.
    Bytes,
    /// A boolean literal.
    Bool,
    /// A tuple literal.
    Tuple,
    /// A formation with no voids: an object.
    Obj {
        /// What it is called, if anything.
        name: Option<String>,
        /// Its attributes, in the order they were written.
        binds: Vec<(String, Node)>,
    },
    /// A formation with voids: fill them, then behave as the body.
    Lam {
        /// What it is called, if anything.
        name: Option<String>,
        /// Its voids, in application order.
        params: Vec<String>,
        /// Its decoratee.
        body: Option<Box<Node>>,
        /// Its other named attributes, inferred so their bodies get checked.
        binds: Vec<(String, Node)>,
    },
    /// Fill the next void of something.
    Apply {
        /// What is being applied.
        fun: Box<Node>,
        /// What fills the void.
        arg: Box<Node>,
    },
    /// Require an attribute of something.
    Dispatch {
        /// The receiver.
        obj: Box<Node>,
        /// The attribute wanted.
        label: String,
        /// Where it was written.
        site: Site,
    },
    /// Optional chaining: dispatch on a receiver that may be `⊥`, propagating
    /// that possibility rather than failing.
    Fragile {
        /// The receiver.
        obj: Box<Node>,
        /// The attribute wanted.
        label: String,
        /// Where it was written.
        site: Site,
    },
    /// Mark a value as possibly `⊥`.
    Maybe(Box<Node>),
    /// Strip the `⊥` off a value, falling back to an alternative.
    Recovered {
        /// The value that may be `⊥`.
        value: Box<Node>,
        /// What to use instead.
        alt: Box<Node>,
    },
    /// A native atom typed purely by what it declares about itself.
    Atom {
        /// Its voids, in application order.
        voids: Vec<Void>,
        /// Its declared return type.
        ret: Option<String>,
    },
    /// A forma naming a non-primitive object by its `Φ`-rooted locator.
    Loc(String),
}

/// A void of a declared atom, with whatever it says about itself.
#[derive(Clone, Debug)]
pub struct Void {
    /// The name of the void.
    pub label: String,
    /// What it declares.
    pub kind: Kind,
}

/// What a declared void says about itself.
#[derive(Clone, Debug)]
pub enum Kind {
    /// Nothing.
    Plain,
    /// Its own type.
    Own(String),
    /// It is a callback, over these argument types.
    Args(String),
}

/// Something went wrong before type checking could start.
#[derive(Debug)]
pub enum Trouble {
    /// The file could not be read.
    Unreadable(String),
    /// The file is not well-formed XML.
    Malformed(String),
}

/// Every top-level object of one XMIR file, with the name it was given.
///
/// # Errors
///
/// [`Trouble`] if the file cannot be read or is not well-formed XML.
pub fn load(path: &Path) -> Result<Vec<(Option<String>, Node)>, Trouble> {
    let text = fs::read_to_string(path)
        .map_err(|failure| Trouble::Unreadable(format!("{}: {failure}", path.display())))?;
    let document = Document::parse(&text)
        .map_err(|failure| Trouble::Malformed(format!("{}: {failure}", path.display())))?;
    Ok(document
        .root_element()
        .children()
        .filter(|child| child.has_tag_name("o"))
        .map(|object| (object.attribute("name").map(str::to_owned), convert(object)))
        .collect())
}

/// Index every top-level object of every file by its locator, so a forma —
/// which is itself a `Φ`-rooted locator — can be resolved to what it names.
///
/// A top-level object is indexed rather than every object, because its parent
/// chain is self-contained and so it can be typed alone. Files that cannot be
/// read are skipped: a broken input should not stop the rest from being checked.
pub fn locators(paths: &[impl AsRef<Path>]) -> HashMap<String, Node> {
    let mut found = HashMap::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path.as_ref()) else {
            continue;
        };
        let Ok(document) = Document::parse(&text) else {
            continue;
        };
        for object in document.root_element().children() {
            if !object.has_tag_name("o") {
                continue;
            }
            if let Some(loc) = object.attribute("loc") {
                found.insert(loc.to_owned(), convert(object));
            }
        }
    }
    found
}

/// A forma written somewhere, and where it was written. A forma is a locator
/// into the object graph, so it can be checked against the graph without
/// inferring anything.
#[derive(Clone, Debug)]
pub struct Forma {
    /// The object it claims exists.
    pub path: String,
    /// Where the claim was made.
    pub site: Site,
}

/// Every locator that exists, and every forma that claims one.
///
/// This is all the dangling-forma lint needs, and it needs no inference at all:
/// a forma naming an object that is not there is a broken reference whatever
/// its type would have been.
#[derive(Debug, Default)]
pub struct Survey {
    /// Every `@loc` in the input.
    pub known: HashSet<String>,
    /// Every forma claimed by an `atom`, `type` or `args` annotation.
    pub claimed: Vec<Forma>,
}

impl Survey {
    /// The formas that name nothing.
    #[must_use]
    pub fn dangling(&self) -> Vec<&Forma> {
        self.claimed
            .iter()
            .filter(|forma| !self.known.contains(&forma.path))
            .collect()
    }
}

/// Walk every object of every file, noting what exists and what is claimed.
///
/// Files that cannot be read are skipped, so one broken input does not stop the
/// rest from being surveyed.
pub fn survey(paths: &[impl AsRef<Path>]) -> Survey {
    let mut found = Survey::default();
    for path in paths {
        let Ok(text) = fs::read_to_string(path.as_ref()) else {
            continue;
        };
        let Ok(document) = Document::parse(&text) else {
            continue;
        };
        for object in document.descendants().filter(|el| el.has_tag_name("o")) {
            if let Some(loc) = object.attribute("loc") {
                found.known.insert(loc.to_owned());
            }
            let here = site(object);
            for annotation in ["atom", "type", "args"] {
                let Some(text) = object.attribute(annotation) else {
                    continue;
                };
                for claim in text.split_whitespace() {
                    if let Some(path) = rooted(claim) {
                        found.claimed.push(Forma {
                            path,
                            site: here.clone(),
                        });
                    }
                }
            }
        }
    }
    found
}

/// The object a single annotation claims, when it claims one at all. A generic
/// letter names no object, nor does `⊥`, nor does anything that is not rooted
/// at `Φ`.
fn rooted(claim: &str) -> Option<String> {
    let core = claim.strip_suffix('?').unwrap_or(claim);
    if generic(core) || core == "⊥" || !core.starts_with("Φ.") {
        return None;
    }
    Some(core.to_owned())
}

/// The last segment of a scope-qualified base: `Φ.bool` to `bool`, `ξ.ρ.if` to
/// `if`, `.eq` to `eq`.
#[must_use]
pub fn tail(base: &str) -> &str {
    base.trim_start_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or(base)
}

/// Whether an annotation is a universally quantified variable rather than a
/// forma: a single letter from `A` to `F`.
#[must_use]
pub fn generic(sig: &str) -> bool {
    sig.len() == 1 && matches!(sig, "A" | "B" | "C" | "D" | "E" | "F")
}

/// The literal a primitive constructor stands for.
fn primitive(name: &str) -> Option<Node> {
    match name {
        "bytes" => Some(Node::Bytes),
        "number" | "i64" | "i32" | "i16" | "float" => Some(Node::Num),
        "string" => Some(Node::Str),
        "bool" => Some(Node::Bool),
        "tuple" => Some(Node::Tuple),
        _ => None,
    }
}

fn elements<'a>(el: Elem<'a, 'a>) -> impl Iterator<Item = Elem<'a, 'a>> {
    el.children().filter(roxmltree::Node::is_element)
}

/// The receiver and the arguments of an application, the latter in `α` order.
fn split<'a>(el: Elem<'a, 'a>) -> (Option<Elem<'a, 'a>>, Vec<Elem<'a, 'a>>) {
    let mut receiver = None;
    let mut args = Vec::new();
    for child in elements(el) {
        if child.attribute("as").is_some() {
            args.push(child);
        } else {
            receiver = Some(child);
        }
    }
    (receiver, args)
}

/// The declared result of a native atom: a primitive, or a reference to the
/// object the forma names.
fn forma(fqn: &str) -> Node {
    primitive(tail(fqn)).unwrap_or_else(|| Node::Loc(fqn.to_owned()))
}

/// A self or parent relative path, dispatched segment by segment on `ξ`. `ρ` is
/// the parent link, so `ξ.ρ.ρ.x` walks up twice, and `φ` is the decoratee.
fn scope(base: &str, site: &Site) -> Node {
    let mut segments = base.split('.');
    let mut node = Node::Name(segments.next().unwrap_or("ξ").to_owned());
    for segment in segments {
        node = Node::Dispatch {
            obj: Box::new(node),
            label: if segment == "φ" { "@" } else { segment }.to_owned(),
            site: site.clone(),
        };
    }
    node
}

/// Where an element says it was written.
fn site(el: Elem) -> Site {
    Site {
        at: el
            .attribute("line")
            .and_then(|text| text.parse().ok())
            .map(|line| Spot {
                line,
                pos: el
                    .attribute("pos")
                    .and_then(|text| text.parse().ok())
                    .unwrap_or(0),
            }),
        loc: el.attribute("loc").map(str::to_owned),
    }
}

/// Set an attribute, replacing whatever was bound under that name before.
fn bind(binds: &mut Vec<(String, Node)>, label: &str, node: Node) {
    if let Some(slot) = binds.iter_mut().find(|(name, _)| name == label) {
        slot.1 = node;
    } else {
        binds.push((label.to_owned(), node));
    }
}

/// A formation: either an atom that declares its whole type, or an object whose
/// attributes get inferred.
fn formation(el: Elem) -> Node {
    let declared = elements(el).find_map(|child| child.attribute("atom"));
    if let Some(ret) = declared {
        if generic(ret) || elements(el).any(|child| child.attribute("type").is_some()) {
            return Node::Atom {
                voids: elements(el)
                    .filter(|child| child.attribute("base") == Some("∅"))
                    .map(|child| Void {
                        label: child.attribute("name").unwrap_or_default().to_owned(),
                        kind: if let Some(own) = child.attribute("type") {
                            Kind::Own(own.to_owned())
                        } else if let Some(args) = child.attribute("args") {
                            Kind::Args(args.to_owned())
                        } else {
                            Kind::Plain
                        },
                    })
                    .collect(),
                ret: Some(ret.to_owned()),
            };
        }
    }
    let mut params = Vec::new();
    let mut binds = Vec::new();
    let mut body = None;
    for child in elements(el) {
        let label = child.attribute("name");
        if let Some(atom) = child.attribute("atom") {
            body = Some(forma(atom));
        } else if child.attribute("base") == Some("∅") {
            params.push(label.unwrap_or_default().to_owned());
        } else if label == Some("φ") {
            body = Some(convert(child));
        } else if let Some(label) = label {
            if !label.starts_with('+') && !label.starts_with('-') {
                bind(&mut binds, label, convert(child));
            }
        }
    }
    let name = el.attribute("name").map(str::to_owned);
    if params.is_empty() {
        if let Some(body) = body {
            bind(&mut binds, "@", body);
        }
        Node::Obj { name, binds }
    } else {
        Node::Lam {
            name,
            params,
            body: body.map(Box::new),
            binds,
        }
    }
}

/// One `<o>` element as a node of the tree.
fn convert(el: Elem) -> Node {
    let Some(base) = el.attribute("base") else {
        if elements(el).next().is_none() && el.text().is_some_and(|text| !text.trim().is_empty()) {
            return Node::Bytes;
        }
        return formation(el);
    };
    let name = tail(base);
    let (receiver, args) = split(el);
    if receiver.is_none() {
        if let Some(literal) = primitive(name) {
            return literal;
        }
    }
    let here = site(el);
    let mut node = if base.starts_with('.') {
        Node::Dispatch {
            obj: Box::new(receiver.map_or(Node::Bytes, convert)),
            label: if name == "φ" { "@" } else { name }.to_owned(),
            site: here,
        }
    } else if base == "ξ" || base.starts_with("ξ.") {
        scope(base, &here)
    } else {
        Node::Name(name.to_owned())
    };
    for arg in args {
        node = Node::Apply {
            fun: Box::new(node),
            arg: Box::new(convert(arg)),
        };
    }
    node
}

#[cfg(test)]
mod tests {
    use super::{Kind, Node, generic, tail};
    use roxmltree::Document;

    fn only(xml: &str) -> Node {
        let document = Document::parse(xml).expect("the fixture is not well-formed");
        super::convert(
            document
                .root_element()
                .children()
                .find(roxmltree::Node::is_element)
                .expect("the fixture has no object"),
        )
    }

    #[test]
    fn takes_the_last_segment_of_a_qualified_base() {
        assert_eq!(
            tail("ξ.ρ.if"),
            "if",
            "a qualified base dont reduce to its last segment"
        );
    }

    #[test]
    fn reads_a_dispatch_as_a_dispatch() {
        let node = only(r#"<p><o base=".plus" line="7" loc="Φ.k.φ"><o base="Φ.number"/></o></p>"#);
        assert!(
            matches!(node, Node::Dispatch { label, site, .. }
                if label == "plus"
                    && site.at.map(|at| at.line) == Some(7)
                    && site.loc.as_deref() == Some("Φ.k.φ")),
            "a dispatch dont read back with its attribute and where it was written"
        );
    }

    #[test]
    fn reads_an_argument_as_an_application() {
        let node =
            only(r#"<p><o base=".plus"><o base="Φ.number"/><o base="Φ.number" as="α0"/></o></p>"#);
        assert!(
            matches!(node, Node::Apply { .. }),
            "an argument dont turn the dispatch into an application"
        );
    }

    #[test]
    fn reads_a_void_as_a_parameter() {
        let node = only(r#"<p><o name="f"><o base="∅" name="x"/><o name="φ" base="ξ.x"/></o></p>"#);
        assert!(
            matches!(node, Node::Lam { ref params, .. } if params == &["x".to_owned()]),
            "a void dont become a parameter of the formation"
        );
    }

    #[test]
    fn reads_a_formation_without_voids_as_an_object() {
        let node = only(r#"<p><o name="k"><o name="φ" base="Φ.number"/></o></p>"#);
        assert!(
            matches!(node, Node::Obj { ref binds, .. } if binds.iter().any(|(l, _)| l == "@")),
            "a formation without voids dont become an object with a decoratee"
        );
    }

    #[test]
    fn reads_a_declared_atom_from_its_annotations() {
        let node = only(
            r#"<p><o name="r"><o base="∅" name="value" type="A?"/>
               <o base="∅" name="alternative" type="A"/><o name="λ" atom="A"/></o></p>"#,
        );
        assert!(
            matches!(node, Node::Atom { ref voids, ref ret }
                if ret.as_deref() == Some("A")
                    && matches!(voids.first().map(|v| &v.kind), Some(Kind::Own(own)) if own == "A?")),
            "a declared atom dont read its return type and its typed voids"
        );
    }

    #[test]
    fn skips_an_attribute_whose_name_marks_it_a_test() {
        let node = only(
            r#"<p><o name="k"><o name="+tests-thing" base="Φ.number"/>
               <o name="real" base="Φ.number"/></o></p>"#,
        );
        assert!(
            matches!(node, Node::Obj { ref binds, .. } if binds.len() == 1),
            "an attribute marked as a test dont get skipped"
        );
    }

    #[test]
    fn reads_a_raw_data_leaf_as_bytes() {
        let node = only(r#"<p><o name="k">DE-AD-BE-EF</o></p>"#);
        assert!(
            matches!(node, Node::Bytes),
            "a leaf carrying only text dont read as raw data"
        );
    }

    #[test]
    fn treats_a_single_letter_as_a_variable() {
        assert!(
            generic("C") && !generic("G"),
            "the letters that quantify dont stop at F"
        );
    }
}
