// SPDX-License-Identifier: CC0-1.0

//! The `ActivationIds` filter of `DescribeInstanceProperties`, end to end over the front
//! door a client uses.
//!
//! RULE: that filter selects on a member the model constrains by pattern — `ActivationId`
//! is `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
//! (`smithy_query(query="shape", shape="ActivationId")`) — so a value outside that shape
//! names no activation that could ever exist and is refused, before any managed node is
//! looked at, with the operation's own modelled activation-id error
//! (`smithy_query(query="operation", operation="DescribeInstanceProperties")`:
//! `InvalidActivationId`, HTTP 400) naming the values it rejected. A value the pattern
//! DOES admit is served, whether or not a node registered with that activation: an
//! activation nothing registered under is a filter that matches nothing, which is a page
//! and not a refusal.
//!
//! The rows span what the rule generalises over: which part of the id's shape a value
//! violates (the group count, a group's width, its alphabet, its case, the separator),
//! how many values the filter carries, where the bad value sits among good ones, and
//! which of the two filter members the request names.
//!
//! The front door is the crate's one test harness (`all/parameter_store.rs`); the managed
//! nodes are seeded the way the agent's own registration will write them, through this
//! service's public state API, because registering a node is the agent's call and not part
//! of this service's model.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::InstanceProperty;
use service_com_amazonaws_ssm_amazonssm::state::{ComAmazonawsSsmAmazonssmState, ManagedNodeRecord};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message, router_with_engine, REGION};

/// The operation under test.
const OPERATION: &str = "DescribeInstanceProperties";

/// The activation the seeded node registered with: a value of the modelled `ActivationId`
/// shape, written here rather than minted so the filter has something to select on.
const REGISTERED_ACTIVATION: &str = "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// Another well-formed activation id, which nothing registered with: the complement that
/// makes an empty page meaningful.
const UNUSED_ACTIVATION: &str = "ffffffff-0000-1111-2222-333333333333";

/// The node the seeded inventory holds, in one of the forms the modelled `InstanceId`
/// pattern admits.
const SEEDED_NODE: &str = "mi-0123456789abcdef0";

/// A router with this service mounted, holding one managed node that registered with
/// [`REGISTERED_ACTIVATION`].
fn seeded_router() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    let record = ManagedNodeRecord {
        properties: InstanceProperty {
            instance_id: Some(SEEDED_NODE.to_string()),
            activation_id: Some(REGISTERED_ACTIVATION.to_string()),
            resource_type: Some("ManagedInstance".to_string()),
            platform_type: Some("Linux".to_string()),
            ping_status: Some("Online".to_string()),
            ..InstanceProperty::default()
        },
        ..ManagedNodeRecord::default()
    };
    assert!(
        seed(&engine, record),
        "a seeded node must carry the node id it is stored under"
    );
    router
}

/// Put one managed node into [`REGION`]'s store, the way the agent's registration will.
fn seed(engine: &Arc<engine::InMemoryEngine>, record: ManagedNodeRecord) -> bool {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .insert_managed_node(REGION, record)
}

/// A request carrying one `ActivationIds` filter in the operator-carrying member.
fn with_operator(values: &[&str]) -> Value {
    json!({ "FiltersWithOperator": [{ "Key": "ActivationIds", "Values": values }] })
}

/// A request carrying one `ActivationIds` filter in the modelled filter list, whose
/// members are the lowercase `key` / `valueSet` pair that list models.
fn modelled_list(values: &[&str]) -> Value {
    json!({ "InstancePropertyFilterList": [{ "key": "ActivationIds", "valueSet": values }] })
}

/// The node ids a served page reports, in the order it reported them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["InstanceProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("a served page reports InstanceProperties, got {body}"))
        .iter()
        .map(|property| {
            property["InstanceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The refusal a rejected activation-id filter carries, written out here independently of
/// the service: the fixed text plus the values the request supplied, in the order it
/// listed them.
fn expected_refusal(rejected: &[&str]) -> String {
    format!("Invalid Activation Ids {}", rejected.join(", "))
}

/// RULE: a value the `ActivationId` pattern does not admit is refused with HTTP 400
/// `InvalidActivationId`, naming the rejected values, through either filter member.
#[tokio::test]
async fn a_malformed_activation_id_filter_value_is_refused() {
    let router = seeded_router();

    // (case, the values the one filter carries, the values the refusal must name)
    let rows: &[(&str, &[&str], &[&str])] = &[
        (
            "a value that is not an id at all",
            &["not-a-valid-activation-id"],
            &["not-a-valid-activation-id"],
        ),
        (
            "four groups instead of five",
            &["0a1b2c3d-4e5f-6a7b-8c9d"],
            &["0a1b2c3d-4e5f-6a7b-8c9d"],
        ),
        (
            "six groups instead of five",
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d-0000"],
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d-0000"],
        ),
        (
            "a first group one character short",
            &["0a1b2c3-4e5f-6a7b-8c9d-0e1f2a3b4c5d"],
            &["0a1b2c3-4e5f-6a7b-8c9d-0e1f2a3b4c5d"],
        ),
        (
            "a last group one character long",
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d0"],
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d0"],
        ),
        (
            "a character outside the hexadecimal alphabet",
            &["0a1b2c3g-4e5f-6a7b-8c9d-0e1f2a3b4c5d"],
            &["0a1b2c3g-4e5f-6a7b-8c9d-0e1f2a3b4c5d"],
        ),
        (
            "the id in upper case",
            &["0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D"],
            &["0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D"],
        ),
        (
            "the groups undelimited",
            &["0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d"],
            &["0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d"],
        ),
        (
            "a trailing space after a well-formed id",
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d "],
            &["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d "],
        ),
        (
            "a malformed value after the registered one",
            &[REGISTERED_ACTIVATION, "not-an-activation"],
            &["not-an-activation"],
        ),
        (
            "a malformed value before the registered one",
            &["not-an-activation", REGISTERED_ACTIVATION],
            &["not-an-activation"],
        ),
        (
            "two malformed values among well-formed ones",
            &[REGISTERED_ACTIVATION, "first-bad", UNUSED_ACTIVATION, "second-bad"],
            &["first-bad", "second-bad"],
        ),
    ];

    for (case, values, rejected) in rows {
        for body in [with_operator(values), modelled_list(values)] {
            let case = format!("{case} in {body}");
            let (status, parsed) = call(&router, OPERATION, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "InvalidActivationId",
                "case {case:?}: expected InvalidActivationId, got {parsed}"
            );
            let expected = expected_refusal(rejected);
            assert_eq!(
                error_message(&parsed),
                expected,
                "case {case:?}: expected the refusal {expected:?}, got {:?}",
                error_message(&parsed)
            );
        }
    }
}

/// RULE, the other side: a value the pattern admits is SERVED — the nodes that registered
/// with that activation, and an empty page for one nothing registered with.
#[tokio::test]
async fn a_well_formed_activation_id_filter_value_is_served() {
    let router = seeded_router();

    // (case, the values the filter carries, the node ids the store answers with)
    let rows: &[(&str, &[&str], &[&str])] = &[
        (
            "the activation the seeded node registered with",
            &[REGISTERED_ACTIVATION],
            &[SEEDED_NODE],
        ),
        (
            "a well-formed activation nothing registered with",
            &[UNUSED_ACTIVATION],
            &[],
        ),
        (
            "both, which a filter's values are alternatives of",
            &[REGISTERED_ACTIVATION, UNUSED_ACTIVATION],
            &[SEEDED_NODE],
        ),
        (
            "an id of nothing but zeroes",
            &["00000000-0000-0000-0000-000000000000"],
            &[],
        ),
        (
            "an id of nothing but f",
            &["ffffffff-ffff-ffff-ffff-ffffffffffff"],
            &[],
        ),
    ];

    for (case, values, expected) in rows {
        for body in [with_operator(values), modelled_list(values)] {
            let case = format!("{case} in {body}");
            let (status, parsed) = call(&router, OPERATION, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {parsed}"
            );
            let reported = reported_ids(&parsed);
            assert_eq!(
                reported,
                expected.iter().map(|id| (*id).to_string()).collect::<Vec<String>>(),
                "case {case:?}: expected the nodes {expected:?}, got {parsed}"
            );
        }
    }
}

/// RULE: the refusal happens before any node is looked at, so it does not depend on what
/// the store holds — an inventory with no node at all refuses the same value the same way,
/// and a refused request leaves the inventory exactly as it was.
#[tokio::test]
async fn the_refusal_does_not_depend_on_the_inventory() {
    let (empty, _engine) = router_with_engine();
    let malformed = "not-an-activation";
    let expected = expected_refusal(&[malformed]);

    let (status, parsed) = call(&empty, OPERATION, &with_operator(&[malformed])).await;
    assert_eq!(
        status, 400,
        "case \"an empty inventory\": expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "InvalidActivationId",
        "case \"an empty inventory\": expected InvalidActivationId, got {parsed}"
    );
    assert_eq!(
        error_message(&parsed),
        expected,
        "case \"an empty inventory\": expected {expected:?}, got {:?}",
        error_message(&parsed)
    );

    // And over the seeded inventory: the refused request reports no node, and the node the
    // store does hold is still reported afterwards.
    let seeded = seeded_router();
    let (status, _) = call(&seeded, OPERATION, &with_operator(&[malformed])).await;
    assert_eq!(
        status, 400,
        "case \"a seeded inventory\": expected HTTP 400, got {status}"
    );
    let (status, parsed) = call(&seeded, OPERATION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"after the refusal\": expected HTTP 200, got {status} with {parsed}"
    );
    assert_eq!(
        reported_ids(&parsed),
        vec![SEEDED_NODE.to_string()],
        "case \"after the refusal\": the inventory must be untouched, got {parsed}"
    );
}
