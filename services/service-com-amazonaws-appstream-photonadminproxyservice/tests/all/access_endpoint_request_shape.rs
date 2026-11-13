// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a write of an `AccessEndpoints` list settles from the REQUEST
//! alone, before the interface VPC endpoint each entry references is resolved against the
//! service that owns it (`tests/all/stack_access_endpoints.rs` drives that hop).
//!
//! RULE: an `AccessEndpoints` entry references an interface VPC endpoint by its `VpceId`,
//! so an entry that carries no id references nothing and is refused as an absent member,
//! and a list naming one `EndpointType` twice asks for two endpoints of a type a stack is
//! reached through once and is refused as an incorrect combination of parameters. Both are
//! answered from the request, so neither reaches the endpoint owner and neither write
//! stores anything.
//!
//! The rows differ along what the rule generalises over: WHICH entry of the list carries no
//! id (the first, a later one, several at once — the refusal names each position), HOW MANY
//! entries repeat a type, and the WRITE PATH the list arrives on (`CreateStack` and the
//! `UpdateStack` that sets it). The positive control is a list whose entries each name an
//! id: nothing in the request refuses it.
//!
//! No owner of VPC endpoints is mounted on this suite's engine, which is exactly what
//! isolates the request-shape readings: with the hop unanswerable nothing is refused for
//! the id's sake, so a refusal here is the request's own shape and nothing else.

use serde_json::{json, Value};

use crate::refusal_messages::refusal;
use crate::stack_fleet_lifecycle::Emulator;

/// The only `AccessEndpointType` the model declares
/// (`smithy_query(query="shape", shape="AccessEndpointType")`).
const STREAMING: &str = "STREAMING";

/// The status every refusal of this service leaves with.
const REFUSAL_STATUS: u16 = 400;

/// A well-formed endpoint id. Nothing resolves it in this suite — no owner of VPC endpoints
/// is mounted — so a row carrying it is refused only if the REQUEST is wrong.
const SOME_ENDPOINT_ID: &str = "vpce-0a1b2c3d4e5f60718";

/// The stack `UpdateStack`'s rows are applied to, created with one endpoint reference so a
/// refused update has something to leave in place.
const STORED_STACK: &str = "stored-stack";

/// AWS's envelope over `count` validation clauses: the count word is singular for one.
fn validation_envelope(clauses: &[String]) -> String {
    let noun = if clauses.len() == 1 {
        "1 validation error detected"
    } else {
        return format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        );
    };
    format!("{noun}: {}", clauses.join("; "))
}

/// AWS's clause for an absent member at the member path of the `position`-th (1-based)
/// `AccessEndpoints` entry's `VpceId`.
fn absent_vpce_id_clause(position: usize) -> String {
    format!(
        "Value null at 'accessEndpoints.{position}.member.vpceId' failed to satisfy \
         constraint: Member must not be null"
    )
}

/// AWS's sentence for a list naming one endpoint type twice.
fn duplicate_type_sentence(endpoint_type: &str) -> String {
    format!(
        "You specified duplicate VPC endpoint types in the request. \
         Specify {endpoint_type} once, and try again."
    )
}

/// An `AccessEndpoints` entry that references an endpoint.
fn entry_with_id(vpce_id: &str) -> Value {
    json!({ "EndpointType": STREAMING, "VpceId": vpce_id })
}

/// An `AccessEndpoints` entry that references none: `VpceId` is absent.
fn entry_without_id() -> Value {
    json!({ "EndpointType": STREAMING })
}

/// One list a write must refuse, with the wire code and the message it is refused with.
struct RefusedList {
    case: &'static str,
    access_endpoints: Value,
    code: &'static str,
    message: String,
}

/// Lists spanning the class: which entries carry no id (so which positions the clauses
/// name), and how many entries repeat the one endpoint type the model declares.
fn refused_lists() -> Vec<RefusedList> {
    vec![
        RefusedList {
            case: "the only entry carries no id",
            access_endpoints: json!([entry_without_id()]),
            code: "ValidationException",
            message: validation_envelope(&[absent_vpce_id_clause(1)]),
        },
        RefusedList {
            case: "the second of two entries carries no id",
            access_endpoints: json!([entry_with_id(SOME_ENDPOINT_ID), entry_without_id()]),
            code: "ValidationException",
            message: validation_envelope(&[absent_vpce_id_clause(2)]),
        },
        RefusedList {
            case: "both entries carry no id",
            access_endpoints: json!([entry_without_id(), entry_without_id()]),
            code: "ValidationException",
            message: validation_envelope(&[
                absent_vpce_id_clause(1),
                absent_vpce_id_clause(2),
            ]),
        },
        RefusedList {
            case: "the one endpoint type named twice",
            access_endpoints: json!([
                entry_with_id(SOME_ENDPOINT_ID),
                entry_with_id(SOME_ENDPOINT_ID),
            ]),
            code: "InvalidParameterCombinationException",
            message: duplicate_type_sentence(STREAMING),
        },
        RefusedList {
            case: "the one endpoint type named three times, each a different endpoint",
            access_endpoints: json!([
                entry_with_id("vpce-0a1b2c3d4e5f60711"),
                entry_with_id("vpce-0a1b2c3d4e5f60712"),
                entry_with_id("vpce-0a1b2c3d4e5f60713"),
            ]),
            code: "InvalidParameterCombinationException",
            message: duplicate_type_sentence(STREAMING),
        },
    ]
}

/// The `VpceId`s `DescribeStacks` reports for the stack `name`.
async fn reported_endpoints(emulator: &Emulator, case: &str, name: &str) -> Vec<String> {
    let described = emulator
        .ok(case, "DescribeStacks", json!({ "Names": [name] }))
        .await;
    described["Stacks"][0]
        .get("AccessEndpoints")
        .and_then(Value::as_array)
        .map(|endpoints| {
            endpoints
                .iter()
                .filter_map(|endpoint| endpoint.get("VpceId").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_refuses_an_access_endpoints_list_its_own_shape_settles() {
    let emulator = Emulator::new();

    for (index, row) in refused_lists().into_iter().enumerate() {
        let name = format!("refused-list-{index}");
        let message = refusal(
            &emulator,
            row.case,
            "CreateStack",
            json!({ "Name": &name, "AccessEndpoints": row.access_endpoints }),
            REFUSAL_STATUS,
            row.code,
        )
        .await;
        assert_eq!(
            message, row.message,
            "case {:?}: expected message {:?}, got {message:?}",
            row.case, row.message
        );

        // Nothing was stored: the name is still one this account holds no stack under.
        refusal(
            &emulator,
            row.case,
            "DescribeStacks",
            json!({ "Names": [&name] }),
            REFUSAL_STATUS,
            "ResourceNotFoundException",
        )
        .await;
    }

    // The control: a list whose entry names an endpoint is not refused for its shape.
    emulator
        .ok(
            "one entry naming an endpoint",
            "CreateStack",
            json!({
                "Name": "accepted-stack",
                "AccessEndpoints": [entry_with_id(SOME_ENDPOINT_ID)],
            }),
        )
        .await;
    let reported = reported_endpoints(&emulator, "one entry naming an endpoint", "accepted-stack")
        .await;
    assert_eq!(
        reported,
        vec![SOME_ENDPOINT_ID.to_string()],
        "case \"one entry naming an endpoint\": expected the stack to report the endpoint it \
         was created with, got {reported:?}"
    );
}

#[tokio::test]
async fn an_update_refuses_the_same_lists_and_replaces_nothing() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "setup",
            "CreateStack",
            json!({
                "Name": STORED_STACK,
                "AccessEndpoints": [entry_with_id(SOME_ENDPOINT_ID)],
            }),
        )
        .await;

    for row in refused_lists() {
        let message = refusal(
            &emulator,
            row.case,
            "UpdateStack",
            json!({ "Name": STORED_STACK, "AccessEndpoints": row.access_endpoints }),
            REFUSAL_STATUS,
            row.code,
        )
        .await;
        assert_eq!(
            message, row.message,
            "case {:?}: expected message {:?}, got {message:?}",
            row.case, row.message
        );

        // The stack keeps the endpoint it already referenced: a refused update replaces
        // nothing.
        let reported = reported_endpoints(&emulator, row.case, STORED_STACK).await;
        assert_eq!(
            reported,
            vec![SOME_ENDPOINT_ID.to_string()],
            "case {:?}: a refused update must leave the stored endpoint, got {reported:?}",
            row.case
        );
    }

    // The control: a list whose entry names an endpoint replaces the stored reference.
    let replacement = "vpce-0a1b2c3d4e5f60999";
    emulator
        .ok(
            "one entry naming another endpoint",
            "UpdateStack",
            json!({ "Name": STORED_STACK, "AccessEndpoints": [entry_with_id(replacement)] }),
        )
        .await;
    let reported = reported_endpoints(&emulator, "one entry naming another endpoint", STORED_STACK)
        .await;
    assert_eq!(
        reported,
        vec![replacement.to_string()],
        "case \"one entry naming another endpoint\": expected the stack to report the \
         endpoint the update named, got {reported:?}"
    );
}
