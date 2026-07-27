//! The contract frozen in `conformance/examples.json`, run against the engine.
//!
//! Each example is built as a tree rather than read from XMIR, because these
//! predate any file format: they are the minimal behavioral spec the paper
//! argues for. Each is named by the id the contract gives it, so a row without a
//! tree and a tree without a row are both failures — that is what makes the
//! document a guard rather than a record of whatever got written.

use std::fs;

use eoti::infer::{Engine, Env};
use eoti::types::Site;
use eoti::xmir::Node;

/// The contract, parsed.
fn contract() -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/examples.json"
        ))
        .expect("cannot read the conformance contract"),
    )
    .expect("cannot parse the conformance contract")
}

/// What the contract records about an example under one field, if anything.
fn recorded(id: &str, field: &str) -> Option<String> {
    contract()["examples"]
        .as_array()
        .expect("the contract dont list its examples")
        .iter()
        .find(|row| row["id"] == id)?[field]
        .as_str()
        .map(str::to_owned)
}

/// Every id the contract lists.
fn listed() -> Vec<String> {
    contract()["examples"]
        .as_array()
        .expect("the contract dont list its examples")
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect()
}

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

/// A parameter used once, so a call site has an accumulated requirement to meet.
fn wants(label: &str) -> Node {
    formation("main", "a", apply(dot(name("a"), label), vec![Node::Num]))
}

/// A conditional over two branches, for demanding something of its result.
fn branching(left: Node, right: Node) -> Node {
    apply(dot(name("bool"), "if"), vec![left, right])
}

/// Every example of the contract, by the id the contract knows it as.
fn examples() -> Vec<(&'static str, Node)> {
    vec![
        (
            "decorated-number-plus",
            apply(dot(object("my-num", Node::Num), "plus"), vec![Node::Num]),
        ),
        (
            "decorated-string-plus",
            apply(dot(object("my-num", Node::Str), "plus"), vec![Node::Num]),
        ),
        (
            "recovered-fragile-number",
            Node::Recovered {
                value: Box::new(Node::Maybe(Box::new(Node::Num))),
                alt: Box::new(Node::Num),
            },
        ),
        (
            "unrecovered-fragile-plus",
            apply(
                dot(Node::Maybe(Box::new(Node::Num)), "plus"),
                vec![Node::Num],
            ),
        ),
        ("bool-if-same-branches", branching(Node::Num, Node::Num)),
        ("bool-if-union-branches", branching(Node::Num, Node::Bytes)),
        (
            "bool-if-result-dispatch",
            apply(
                dot(branching(Node::Num, Node::Num), "plus"),
                vec![Node::Num],
            ),
        ),
        (
            "bool-if-result-misuse",
            apply(
                dot(branching(Node::Num, Node::Str), "plus"),
                vec![Node::Num],
            ),
        ),
        (
            "structural-requirement",
            formation("main", "a", dot(name("a"), "x")),
        ),
        (
            "requirement-met-at-call-site",
            apply(wants("plus"), vec![Node::Num]),
        ),
        (
            "requirement-broken-at-call-site",
            apply(wants("plus"), vec![Node::Str]),
        ),
        (
            "self-recursion-fixpoint",
            formation(
                "rec",
                "n",
                apply(
                    dot(apply(dot(name("n"), "eq"), vec![Node::Num]), "if"),
                    vec![
                        Node::Num,
                        apply(
                            name("rec"),
                            vec![apply(dot(name("n"), "minus"), vec![Node::Num])],
                        ),
                    ],
                ),
            ),
        ),
        (
            "recursive-tuple-walk",
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
            ),
        ),
    ]
}

/// The example the contract knows by this name.
fn example(id: &str) -> Node {
    match examples().into_iter().find(|(known, _)| *known == id) {
        Some((_, node)) => node,
        None => panic!("the contract knows no example called {id}"),
    }
}

fn accepted(node: &Node) -> bool {
    Engine::strict().infer(node, &Env::new()).is_ok()
}

/// The verdict the engine reaches, and the machine string it refuses with.
fn verdict(node: &Node) -> (&'static str, Option<&'static str>) {
    match Engine::strict().infer(node, &Env::new()) {
        Ok(_) => ("ok", None),
        Err(clash) => ("reject", Some(clash.code())),
    }
}

#[test]
fn freezes_every_reference_example() {
    assert_eq!(
        listed().len(),
        13,
        "the contract dont freeze all 13 reference examples"
    );
}

#[test]
fn expects_a_verdict_of_every_example() {
    assert!(
        listed()
            .iter()
            .all(|id| matches!(recorded(id, "expect").as_deref(), Some("ok" | "reject"))),
        "an example dont expect either verdict"
    );
}

#[test]
fn builds_every_example_the_contract_lists() {
    let unbuilt: Vec<String> = listed()
        .into_iter()
        .filter(|id| !examples().iter().any(|(known, _)| known == id))
        .collect();
    assert_eq!(
        unbuilt,
        Vec::<String>::new(),
        "an example the contract lists dont get built and run"
    );
}

#[test]
fn reaches_the_verdict_the_contract_froze() {
    let wrong: Vec<&str> = examples()
        .iter()
        .filter(|(id, node)| recorded(id, "expect").as_deref() != Some(verdict(node).0))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        wrong,
        Vec::<&str>::new(),
        "an example dont reach the verdict the contract froze for it"
    );
}

#[test]
fn refuses_with_the_machine_string_the_contract_recorded() {
    let wrong: Vec<&str> = examples()
        .iter()
        .filter(|(_, node)| verdict(node).1.is_some())
        .filter(|(id, node)| recorded(id, "code").as_deref() != verdict(node).1)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        wrong,
        Vec::<&str>::new(),
        "a refusal dont carry the machine string the contract recorded"
    );
}

#[test]
fn rejects_nothing_of_the_runtime() {
    assert_eq!(
        contract()["runtime"]["rejected"].as_u64(),
        Some(0),
        "the contract dont demand a clean runtime"
    );
}

#[test]
fn dispatches_through_a_decoratee() {
    assert!(
        accepted(&example("decorated-number-plus")),
        "an attribute of the decoratee dont reach through the object wrapping it"
    );
}

#[test]
fn refuses_an_attribute_the_decoratee_lacks() {
    assert!(
        !accepted(&example("decorated-string-plus")),
        "wrapping a string dont stop it from claiming an attribute it lacks"
    );
}

#[test]
fn recovers_a_value_that_can_be_bottom() {
    assert!(
        accepted(&example("recovered-fragile-number")),
        "recovering a maybe-bottom value dont give back a definite one"
    );
}

#[test]
fn refuses_a_plain_dispatch_on_a_value_that_can_be_bottom() {
    assert!(
        !accepted(&example("unrecovered-fragile-plus")),
        "dispatching on a maybe-bottom value dont get refused"
    );
}

#[test]
fn takes_both_branches_of_a_conditional() {
    assert!(
        accepted(&example("bool-if-same-branches")),
        "a conditional whose branches agree dont type"
    );
}

#[test]
fn joins_branches_of_unlike_shape() {
    assert!(
        accepted(&example("bool-if-union-branches")),
        "branches of unlike shape dont join without a spurious error"
    );
}

#[test]
fn reaches_both_branches_with_a_demand_on_the_result() {
    assert!(
        accepted(&example("bool-if-result-dispatch")),
        "a demand on a conditional's result dont reach branches that satisfy it"
    );
}

#[test]
fn refuses_a_demand_one_branch_cannot_meet() {
    assert!(
        !accepted(&example("bool-if-result-misuse")),
        "a demand no branch can meet dont get refused"
    );
}

#[test]
fn leaves_an_unmet_requirement_open() {
    assert!(
        accepted(&example("structural-requirement")),
        "a requirement on a parameter dont stay open in an open world"
    );
}

#[test]
fn accepts_a_call_site_that_meets_the_requirement() {
    assert!(
        accepted(&example("requirement-met-at-call-site")),
        "a call site that meets the accumulated requirement dont get accepted"
    );
}

#[test]
fn refuses_a_call_site_that_cannot_meet_the_requirement() {
    assert!(
        !accepted(&example("requirement-broken-at-call-site")),
        "a call site that cannot meet the requirement dont get refused"
    );
}

#[test]
fn settles_a_self_recursive_formation() {
    assert!(
        accepted(&example("self-recursion-fixpoint")),
        "a self-recursive formation dont settle on a fixpoint"
    );
}

#[test]
fn settles_a_recursive_walk_over_a_cyclic_object() {
    assert!(
        accepted(&example("recursive-tuple-walk")),
        "walking a cyclic object dont terminate"
    );
}
