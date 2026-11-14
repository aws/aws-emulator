// SPDX-License-Identifier: CC0-1.0

//! The `id(…)` spelling of a filter expression's ENTITY SCOPES, end to end over the
//! wire (the harness in `xray_wire`).
//!
//! The RULE under test: **the complex keywords `service(…)` and `edge(…)` name their
//! entities in either of the two spellings AWS's filter-expression reference gives —
//! the names alone (`service("name", "type")`, `edge("source", "destination")`) or one
//! `id(…)` function per entity (`service(id(name: "name", type: "type"))`,
//! `edge(id(name: "source"), id(name: "destination"))`) — and both spellings name the
//! same scope, so an expression written either way selects the same traces and scopes
//! the keywords of its `{ … }` block to the same segments.**
//!
//! The rows differ along what the rule generalises over: which keyword is scoped
//! (`service`, `edge`), which members the `id(…)` labels and in which order (`name`
//! alone, `name` and `type`, `type` before `name`), whether the scope carries a block,
//! whether it is negated, and whether the entity it names was recorded at all. The
//! table asserts BOTH spellings per row, so neither can drift from the other.
//!
//! The grammar is one parser's, so the spelling is asserted on the surface that
//! EVALUATES an expression (`GetTraceSummaries`) and on the one that only STORES it
//! (`CreateGroup`), and an argument list of a shape the scope cannot take is refused
//! where it is evaluated rather than answered with an empty selection.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::xray_wire::{array, trace_id, Xray};

/// The window every query below covers, chosen to hold every seeded trace.
const WINDOW: (f64, f64) = (900.0, 2_000.0);

/// The `origin` of each seeded service, which is the `type` a scope selects a service
/// by (`SegmentRecord::service_type`).
const STAGE_ORIGIN: &str = "AWS::ApiGateway::Stage";
const STATE_MACHINE_ORIGIN: &str = "AWS::StepFunctions::StateMachine";

/// One seeded trace: the label a row names it by, its id, and its documents.
struct Seeded {
    label: &'static str,
    id: String,
    documents: Vec<String>,
}

/// The corpus every row runs against: a trace of two segments (so one edge runs
/// through it), a trace of one typed segment, and a trace whose segment carries no
/// origin at all (so a scope naming a type cannot select it).
fn corpus() -> Vec<Seeded> {
    let edged = trace_id(71);
    let workflow = trace_id(72);
    let untyped = trace_id(73);
    vec![
        Seeded {
            label: "edged",
            id: edged.clone(),
            documents: vec![
                json!({
                    "trace_id": edged,
                    "id": "0000000000000071",
                    "name": "gateway",
                    "origin": STAGE_ORIGIN,
                    "start_time": 1_000.0_f64,
                    "end_time": 1_000.5_f64,
                    "http": { "response": { "status": 200 } },
                })
                .to_string(),
                json!({
                    "trace_id": edged,
                    "id": "0000000000000171",
                    "parent_id": "0000000000000071",
                    "name": "ledger",
                    "start_time": 1_000.1_f64,
                    "end_time": 1_000.4_f64,
                    "fault": true,
                })
                .to_string(),
            ],
        },
        Seeded {
            label: "workflow",
            id: workflow.clone(),
            documents: vec![json!({
                "trace_id": workflow,
                "id": "0000000000000072",
                "name": "orders",
                "origin": STATE_MACHINE_ORIGIN,
                "start_time": 1_100.0_f64,
                "end_time": 1_100.5_f64,
            })
            .to_string()],
        },
        Seeded {
            label: "untyped",
            id: untyped.clone(),
            documents: vec![json!({
                "trace_id": untyped,
                "id": "0000000000000073",
                "name": "orders",
                "start_time": 1_200.0_f64,
                "end_time": 1_200.5_f64,
            })
            .to_string()],
        },
    ]
}

/// Ingest the corpus through `PutTraceSegments`, so every row below is answered from
/// stored segment documents.
async fn seeded() -> (Xray, Vec<Seeded>) {
    let xray = Xray::new();
    let corpus = corpus();
    for trace in &corpus {
        let put = xray.put_segments(&trace.documents).await;
        assert_eq!(
            put.status, 200,
            "seeding {:?} must succeed, got {put:?}",
            trace.label
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "seeding {:?} must store every document, got {:?}",
            trace.label,
            put.get("UnprocessedTraceSegments")
        );
    }
    (xray, corpus)
}

/// Which corpus traces `expression` selects.
async fn selects(xray: &Xray, corpus: &[Seeded], expression: &str) -> BTreeSet<&'static str> {
    let got = xray.filtered(WINDOW.0, WINDOW.1, expression).await;
    assert_eq!(
        got.status, 200,
        "case {expression:?}: an expression the language admits is served, got {got:?}"
    );
    array(&got.body, "TraceSummaries")
        .iter()
        .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
        .map(|found| {
            corpus
                .iter()
                .find(|trace| trace.id == found)
                .map_or("unknown", |trace| trace.label)
        })
        .collect()
}

/// The labels in `labels`, as a selection to compare against.
fn set(labels: &[&'static str]) -> BTreeSet<&'static str> {
    labels.iter().copied().collect()
}

/// Both spellings of one scope select exactly the traces the rule says, and each
/// other's.
#[tokio::test]
async fn a_scope_named_with_id_functions_selects_what_the_named_spelling_does() {
    let (xray, corpus) = seeded().await;

    // (the names spelling, the `id(…)` spelling, the traces both must select)
    let rows: Vec<(&str, &str, Vec<&'static str>)> = vec![
        (
            "service(\"gateway\")",
            "service(id(name: \"gateway\"))",
            vec!["edged"],
        ),
        // A name two traces recorded, one of them typed: the name alone selects both.
        (
            "service(\"orders\")",
            "service(id(name: \"orders\"))",
            vec!["workflow", "untyped"],
        ),
        // Narrowed by the type the service was REPORTED with, which is the segment's
        // `origin`: the untyped twin carries none, so only the typed one is selected.
        (
            "service(\"orders\", \"AWS::StepFunctions::StateMachine\")",
            "service(id(name: \"orders\", type: \"AWS::StepFunctions::StateMachine\"))",
            vec!["workflow"],
        ),
        // The labels name the members, so their ORDER is the caller's.
        (
            "service(\"orders\", \"AWS::StepFunctions::StateMachine\")",
            "service(id(type: \"AWS::StepFunctions::StateMachine\", name: \"orders\"))",
            vec!["workflow"],
        ),
        (
            "service(\"orders\", \"AWS::ApiGateway::Stage\")",
            "service(id(name: \"orders\", type: \"AWS::ApiGateway::Stage\"))",
            vec![],
        ),
        (
            "service(\"nobody\")",
            "service(id(name: \"nobody\"))",
            vec![],
        ),
        // An edge names its source and its destination, one `id(…)` each, and is
        // directed.
        (
            "edge(\"gateway\", \"ledger\")",
            "edge(id(name: \"gateway\"), id(name: \"ledger\"))",
            vec!["edged"],
        ),
        (
            "edge(\"ledger\", \"gateway\")",
            "edge(id(name: \"ledger\"), id(name: \"gateway\"))",
            vec![],
        ),
        // A block scopes its keywords to the selected segments, whichever spelling
        // selected them.
        (
            "service(\"gateway\") { http.status = 200 }",
            "service(id(name: \"gateway\")) { http.status = 200 }",
            vec!["edged"],
        ),
        (
            "service(\"gateway\") { http.status = 500 }",
            "service(id(name: \"gateway\")) { http.status = 500 }",
            vec![],
        ),
        (
            "service(\"gateway\") { fault }",
            "service(id(name: \"gateway\")) { fault }",
            vec![],
        ),
        (
            "edge(\"gateway\", \"ledger\") { fault }",
            "edge(id(name: \"gateway\"), id(name: \"ledger\")) { fault }",
            vec!["edged"],
        ),
        // Negated, and composed, exactly as the named spelling is.
        (
            "!service(\"gateway\")",
            "!service(id(name: \"gateway\"))",
            vec!["workflow", "untyped"],
        ),
        (
            "service(\"gateway\") OR service(\"orders\")",
            "service(id(name: \"gateway\")) OR service(id(name: \"orders\"))",
            vec!["edged", "workflow", "untyped"],
        ),
    ];

    for (named, with_ids, expected) in rows {
        let by_name = selects(&xray, &corpus, named).await;
        assert_eq!(
            by_name,
            set(&expected),
            "case {named:?}: expected {:?}, got {by_name:?}",
            set(&expected)
        );
        let by_id = selects(&xray, &corpus, with_ids).await;
        assert_eq!(
            by_id,
            set(&expected),
            "case {with_ids:?}: expected {:?}, got {by_id:?}",
            set(&expected)
        );
        assert_eq!(
            by_id, by_name,
            "case {with_ids:?}: expected the same selection as {named:?} ({by_name:?}), got \
             {by_id:?}"
        );
    }
}

/// The surface that only STORES an expression reads the same grammar, so a group is
/// created with a scope written either way — and refused for an argument list the
/// scope cannot take.
#[tokio::test]
async fn a_group_stores_a_scope_named_with_id_functions() {
    let xray = Xray::new();

    for (case, name, expression) in [
        (
            "a service named by one id function",
            "by-id",
            "service(id(name: \"orders\", type: \"AWS::StepFunctions::StateMachine\"))",
        ),
        (
            "a service named by an id function with a block",
            "by-id-block",
            "service(id(name: \"orders\")) { fault = true }",
        ),
        (
            "an edge named by two id functions",
            "edge-by-id",
            "edge(id(name: \"gateway\"), id(name: \"ledger\"))",
        ),
    ] {
        let created = xray
            .call(
                "/CreateGroup",
                &json!({ "GroupName": name, "FilterExpression": expression }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the group created for {expression:?}, got {created:?}"
        );
        assert_eq!(
            created
                .get("Group")
                .get("FilterExpression")
                .and_then(Value::as_str),
            Some(expression),
            "case {case:?}: expected the expression stored as written, got {created:?}"
        );
    }

    for (case, name, expression) in [
        (
            "an id function labelling no member",
            "empty-id",
            "service(id())",
        ),
        (
            "an id function labelling a member no entity has",
            "bogus-label",
            "service(id(bogus: \"a\"))",
        ),
        (
            "an id function whose value is not text",
            "number-value",
            "service(id(name: 1))",
        ),
        (
            "an id function the expression never closes",
            "unclosed-id",
            "service(id(name: \"a\")",
        ),
    ] {
        let refused = xray
            .call(
                "/CreateGroup",
                &json!({ "GroupName": name, "FilterExpression": expression }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected {expression:?} refused, got {refused:?}"
        );
        assert!(
            refused.message().is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a refusal that says why, got {refused:?}"
        );
    }
}

/// An argument list a scope cannot take is REFUSED where the expression is evaluated,
/// rather than answered with an empty selection: a caller is not told "no traces
/// match" for a question that was never asked.
#[tokio::test]
async fn a_scope_whose_id_functions_name_no_scope_is_refused() {
    let xray = Xray::new();

    for (case, expression) in [
        (
            "a service naming two entities",
            "service(id(name: \"a\"), id(name: \"b\"))",
        ),
        (
            "a service mixing the two spellings",
            "service(id(name: \"a\"), \"b\")",
        ),
        (
            "an id function that labels no name",
            "service(id(type: \"AWS::StepFunctions::StateMachine\"))",
        ),
        ("an edge naming one entity", "edge(id(name: \"a\"))"),
        (
            "an edge naming three entities",
            "edge(id(name: \"a\"), id(name: \"b\"), id(name: \"c\"))",
        ),
        (
            "a scope narrowed by an entity's account",
            "service(id(name: \"a\", account.id: \"000000000000\"))",
        ),
    ] {
        let refused = xray.filtered(WINDOW.0, WINDOW.1, expression).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected {expression:?} refused, got {refused:?}"
        );
        assert!(
            refused.message().is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a refusal that says why, got {refused:?}"
        );
    }
}
