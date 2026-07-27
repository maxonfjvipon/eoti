//! The accept and reject cases frozen in `conformance/examples.json`.
//!
//! Each is built as a tree rather than read from XMIR, because these predate any
//! file format: they are the minimal behavioral spec the paper argues for.

use eoti::infer::{Engine, Env};
use eoti::types::Site;
use eoti::xmir::Node;

fn name(what: &str) -> Node {
    Node::Name(what.to_owned())
}

fn dot(obj: Node, label: &str) -> Node {
    Node::Dispatch {
        obj: Box::new(obj),
        label: label.to_owned(),
        site: Site::default(),
    }
}

fn apply(fun: Node, args: Vec<Node>) -> Node {
    args.into_iter().fold(fun, |fun, arg| Node::Apply {
        fun: Box::new(fun),
        arg: Box::new(arg),
    })
}

fn object(what: &str, decoratee: Node) -> Node {
    Node::Obj {
        name: Some(what.to_owned()),
        binds: vec![("@".to_owned(), decoratee)],
    }
}

fn formation(what: &str, param: &str, body: Node) -> Node {
    Node::Lam {
        name: Some(what.to_owned()),
        params: vec![param.to_owned()],
        body: Some(Box::new(body)),
        binds: Vec::new(),
    }
}

fn accepted(node: &Node) -> bool {
    Engine::strict().infer(node, &Env::new()).is_ok()
}

/// `last = [tup] > (tup.tail.length.eq 0).if tup.head (last tup.tail)` — the
/// recursive object that once made the solver loop forever.
fn last() -> Node {
    formation(
        "last",
        "tup",
        apply(
            dot(
                apply(
                    dot(dot(dot(name("tup"), "tail"), "length"), "eq"),
                    vec![Node::Num],
                ),
                "if",
            ),
            vec![
                dot(name("tup"), "head"),
                apply(name("last"), vec![dot(name("tup"), "tail")]),
            ],
        ),
    )
}

#[test]
fn dispatches_through_a_decoratee() {
    assert!(
        accepted(&apply(
            dot(object("my-num", Node::Num), "plus"),
            vec![Node::Num]
        )),
        "an attribute of the decoratee dont reach through the object wrapping it"
    );
}

#[test]
fn refuses_an_attribute_the_decoratee_lacks() {
    assert!(
        !accepted(&apply(
            dot(object("my-num", Node::Str), "plus"),
            vec![Node::Num]
        )),
        "wrapping a string dont stop it from claiming an attribute it lacks"
    );
}

#[test]
fn recovers_a_value_that_can_be_bottom() {
    assert!(
        accepted(&Node::Recovered {
            value: Box::new(Node::Maybe(Box::new(Node::Num))),
            alt: Box::new(Node::Num),
        }),
        "recovering a maybe-bottom value dont give back a definite one"
    );
}

#[test]
fn refuses_a_plain_dispatch_on_a_value_that_can_be_bottom() {
    assert!(
        !accepted(&apply(
            dot(Node::Maybe(Box::new(Node::Num)), "plus"),
            vec![Node::Num]
        )),
        "dispatching on a maybe-bottom value dont get refused"
    );
}

#[test]
fn takes_both_branches_of_a_conditional() {
    assert!(
        accepted(&apply(dot(name("bool"), "if"), vec![Node::Num, Node::Num])),
        "a conditional whose branches agree dont type"
    );
}

#[test]
fn joins_branches_of_unlike_shape() {
    assert!(
        accepted(&apply(
            dot(name("bool"), "if"),
            vec![Node::Num, Node::Bytes]
        )),
        "branches of unlike shape dont join without a spurious error"
    );
}

#[test]
fn reaches_both_branches_with_a_demand_on_the_result() {
    assert!(
        accepted(&apply(
            dot(
                apply(dot(name("bool"), "if"), vec![Node::Num, Node::Num]),
                "plus"
            ),
            vec![Node::Num]
        )),
        "a demand on a conditional's result dont reach branches that satisfy it"
    );
}

#[test]
fn refuses_a_demand_one_branch_cannot_meet() {
    assert!(
        !accepted(&apply(
            dot(
                apply(dot(name("bool"), "if"), vec![Node::Num, Node::Str]),
                "plus"
            ),
            vec![Node::Num]
        )),
        "a demand no branch can meet dont get refused"
    );
}

#[test]
fn leaves_an_unmet_requirement_open() {
    assert!(
        accepted(&formation("main", "a", dot(name("a"), "x"))),
        "a requirement on a parameter dont stay open in an open world"
    );
}

#[test]
fn accepts_a_call_site_that_meets_the_requirement() {
    assert!(
        accepted(&apply(
            formation("main", "a", apply(dot(name("a"), "plus"), vec![Node::Num])),
            vec![Node::Num]
        )),
        "a call site that meets the accumulated requirement dont get accepted"
    );
}

#[test]
fn refuses_a_call_site_that_cannot_meet_the_requirement() {
    assert!(
        !accepted(&apply(
            formation("main", "a", apply(dot(name("a"), "plus"), vec![Node::Num])),
            vec![Node::Str]
        )),
        "a call site that cannot meet the requirement dont get refused"
    );
}

#[test]
fn settles_a_self_recursive_formation() {
    assert!(
        accepted(&formation(
            "rec",
            "n",
            apply(
                dot(apply(dot(name("n"), "eq"), vec![Node::Num]), "if"),
                vec![
                    Node::Num,
                    apply(
                        name("rec"),
                        vec![apply(dot(name("n"), "minus"), vec![Node::Num])]
                    ),
                ],
            ),
        )),
        "a self-recursive formation dont settle on a fixpoint"
    );
}

#[test]
fn settles_a_recursive_walk_over_a_cyclic_object() {
    assert!(accepted(&last()), "walking a cyclic object dont terminate");
}
