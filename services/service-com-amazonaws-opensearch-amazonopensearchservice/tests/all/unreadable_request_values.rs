// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service refuses a request body it cannot DESERIALIZE:
//! a value whose JSON class the modelled member it is bound to admits no reading of.
//!
//! The rule under test: **a value of a class the member it is bound to cannot be read from
//! never becomes an input, so the request is refused at the deserialization layer — HTTP
//! 400 under the protocol's own `SerializationException`, with a message stating the CLASS
//! of value sent and the modelled type it could not be read into — and nothing is stored.**
//! A value that WAS read and then broke a rule the model declares is the layer below
//! (constraint validation) and keeps its own class, which is the difference a client
//! branches on.
//!
//! What made this visible: a policy document written the way every policy document is
//! written — as the document itself — reaches `CreateDomain$AccessPolicies`, which targets
//! `PolicyDocument`, a STRING of 0..102400 characters
//! (`smithy_query(query="operation", operation="CreateDomain")`). The codec refused it with
//! its own parser diagnostic (`expected string, got object start`) as the service's
//! message: a sentence naming neither the class sent nor the type expected, under the
//! constraint-violation class.
//!
//! The rows differ along what the rule generalises over: the CLASS the request sent (an
//! object, an array, a number, a boolean, a string), the modelled KIND it was bound to (a
//! string, an enumerated type's string, a structure, a list, a map), the LEVEL the value
//! sits at (a member of the body, and the body document itself), and the operation
//! carrying it (a create whose members are all in the body, an update whose name is bound
//! to the URI, and a read whose one member is a list). The recorded case — a policy
//! document object where the access-policy string is modelled — is one row of that table.
//!
//! One class is NOT this layer's: an explicit JSON `null` is absence rather than a class,
//! so a member carrying one is left exactly as the codec answers it — the member the shape
//! requires is the validation layer's refusal.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second copy
//! of it.

use serde_json::{json, Value};

use super::domain_lifecycle::{Emulator, PREFIX};

/// The error code the protocol answers a body it could not read with, and the `__type` and
/// `x-amzn-errortype` a `restJson1` client resolves the error shape from. Restated here
/// rather than imported, so a change to the code fails this suite instead of being
/// asserted against itself.
const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The engine release every row names: one the service's catalogue offers, so the only
/// thing a row can be refused for is the value under test.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The policy document a template writes for a domain — the document itself, not its text.
fn policy_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "*"},
            "Action": "es:ESHttpGet",
            "Resource": "*",
        }],
    })
}

/// A `CreateDomain` body for `name` that the service accepts, which a row then states ONE
/// member of differently.
fn create_body(name: &str) -> Value {
    json!({
        "DomainName": name,
        "EngineVersion": ENGINE_VERSION,
        "ClusterConfig": {"InstanceType": "t3.small.search", "InstanceCount": 1},
        "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10},
    })
}

/// The same body with `member` stated as `value`.
fn create_with(name: &str, member: &str, value: Value) -> Value {
    let mut body = create_body(name);
    body[member] = value;
    body
}

/// One row: the case, the request to make, and the sentence the refusal has to state.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    expected_message: &'static str,
}

/// The rows of the refusal table — one per (class sent, modelled kind) pairing the rule
/// generalises over, across three operations and both levels a value sits at.
fn refused_rows() -> Vec<Row> {
    let domain = format!("{PREFIX}/opensearch/domain");
    vec![
        Row {
            // The recorded case: a policy document where the member is modelled a string.
            case: "a policy document object where the access-policy string is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with("sim-unread-policy-object", "AccessPolicies", policy_document()),
            expected_message: "OBJECT_VALUE can not be converted to a String",
        },
        Row {
            case: "a list of statements where that same string is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with(
                "sim-unread-policy-array",
                "AccessPolicies",
                json!([policy_document()]),
            ),
            expected_message: "ARRAY_VALUE can not be converted to a String",
        },
        Row {
            case: "a number where the engine release string is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with("sim-unread-engine-number", "EngineVersion", json!(2.11)),
            expected_message: "NUMBER_VALUE can not be converted to a String",
        },
        Row {
            case: "a boolean where the domain name string is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with("sim-unread-name-boolean", "DomainName", json!(true)),
            expected_message: "BOOLEAN_VALUE can not be converted to a String",
        },
        Row {
            // `IPAddressType` targets an enumerated type, which travels as a string.
            case: "an object where an enumerated type's string is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with(
                "sim-unread-ip-object",
                "IPAddressType",
                json!({"Type": "dualstack"}),
            ),
            expected_message: "OBJECT_VALUE can not be converted to a String",
        },
        Row {
            case: "a string where the cluster structure is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with(
                "sim-unread-cluster-string",
                "ClusterConfig",
                json!("t3.small.search"),
            ),
            expected_message: "STRING_VALUE can not be converted to a Structure",
        },
        Row {
            case: "an object where the tag list is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with(
                "sim-unread-tags-object",
                "TagList",
                json!({"Key": "owner", "Value": "search"}),
            ),
            expected_message: "OBJECT_VALUE can not be converted to a List",
        },
        Row {
            case: "a list where the advanced options map is modelled",
            method: "POST",
            uri: domain.clone(),
            body: create_with("sim-unread-options-array", "AdvancedOptions", json!([])),
            expected_message: "ARRAY_VALUE can not be converted to a Map",
        },
        Row {
            case: "a body that is not the modelled input structure at all",
            method: "POST",
            uri: domain.clone(),
            body: json!([1, 2]),
            expected_message: "ARRAY_VALUE can not be converted to a Structure",
        },
        Row {
            case: "a bare string where the input structure is modelled",
            method: "POST",
            uri: domain,
            body: json!("sim-unread-bare-string"),
            expected_message: "STRING_VALUE can not be converted to a Structure",
        },
        Row {
            // The update path: the domain's name is bound to the URI, so the only value in
            // the body is the one under test.
            case: "a policy document object on the update, whose name is bound to the URI",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/sim-unread-update/config"),
            body: json!({"AccessPolicies": policy_document()}),
            expected_message: "OBJECT_VALUE can not be converted to a String",
        },
        Row {
            case: "a quoted number where a purchase's instance count integer is modelled",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/purchaseReservedInstanceOffering"),
            body: json!({
                "ReservedInstanceOfferingId": "00000000-0000-0000-0000-000000000000",
                "ReservationName": "sim-unread-reservation",
                "InstanceCount": "2",
            }),
            expected_message: "STRING_VALUE can not be converted to an Integer",
        },
        Row {
            case: "a quoted flag where the update's dry-run boolean is modelled",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/sim-unread-dryrun/config"),
            body: json!({"DryRun": "true"}),
            expected_message: "STRING_VALUE can not be converted to a Boolean",
        },
        Row {
            case: "a string where a read's list of domain names is modelled",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain-info"),
            body: json!({"DomainNames": "sim-unread-one-name"}),
            expected_message: "STRING_VALUE can not be converted to a List",
        },
    ]
}

/// Each row is refused with the protocol's serialization error, naming the class sent and
/// the modelled type it could not be read into — and nothing it asked for is stored.
#[tokio::test]
async fn a_value_the_modelled_member_cannot_be_read_from_is_a_serialization_refusal() {
    let emulator = Emulator::new();

    for row in refused_rows() {
        let Row {
            case,
            method,
            uri,
            body,
            expected_message,
        } = row;
        let answer = emulator.call(method, &uri, Some(body.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some(SERIALIZATION_EXCEPTION),
            "case {case:?}: expected the {SERIALIZATION_EXCEPTION} error code, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.body["__type"].as_str(),
            Some(SERIALIZATION_EXCEPTION),
            "case {case:?}: expected the same code in the envelope, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected_message),
            "case {case:?}: expected the coercion message, got {:?}",
            answer.body
        );
        // A request that never became an input created nothing: the name it asked for is
        // held by no domain afterwards.
        if let Some(name) = body.get("DomainName").and_then(Value::as_str) {
            let read = emulator.describe(name).await;
            assert_ne!(
                read.status, 200,
                "case {case:?}: expected no domain named {name:?}, got {:?}",
                read.body
            );
        }
    }
}

/// A value of a class the member CAN be read from is not a serialization refusal, whatever
/// else is wrong with it: the layer below owns it. The non-vacuity guard for the table
/// above — without it, a layer that refused everything would pass.
#[tokio::test]
async fn a_value_of_a_readable_class_is_not_a_serialization_refusal() {
    let emulator = Emulator::new();
    // (case, the body, whether the service accepts it)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "the policy document as the text the member takes",
            create_with(
                "sim-readable-policy-text",
                "AccessPolicies",
                json!(policy_document().to_string()),
            ),
            true,
        ),
        (
            "an empty policy document",
            create_with("sim-readable-policy-empty", "AccessPolicies", json!("")),
            true,
        ),
        (
            "a member the input shape does not model",
            create_with(
                "sim-readable-unmodelled",
                "NotAModelledMember",
                json!({"nested": true}),
            ),
            true,
        ),
        (
            "a structure where the cluster structure is modelled",
            create_with(
                "sim-readable-cluster-object",
                "ClusterConfig",
                json!({"InstanceType": "t3.small.search", "InstanceCount": 2}),
            ),
            true,
        ),
        (
            "a domain name shorter than its modelled bound",
            create_with("ab", "AccessPolicies", json!(policy_document().to_string())),
            false,
        ),
        (
            "an enumerated value outside its modelled set",
            create_with("sim-readable-ip-unknown", "IPAddressType", json!("sixpv6")),
            false,
        ),
    ];

    for (case, body, accepted) in cases {
        let answer = emulator.create(body).await;
        assert_ne!(
            answer.error.as_deref(),
            Some(SERIALIZATION_EXCEPTION),
            "case {case:?}: a readable class is not a serialization failure, got {} with \
             {:?}",
            answer.status,
            answer.body
        );
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be accepted, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected the constraint class, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// The member stays what the model says it is: the document's TEXT is what a created
/// domain holds, read back through the service's own describe.
#[tokio::test]
async fn the_policy_member_round_trips_as_the_text_the_model_declares() {
    let emulator = Emulator::new();
    let document = policy_document();
    let text = document.to_string();
    let name = "sim-readable-round-trip";

    let created = emulator
        .create(create_with(name, "AccessPolicies", json!(text.clone())))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected 200 from CreateDomain, got {} with {:?}",
        created.status, created.body
    );

    let read = emulator.describe(name).await;
    assert_eq!(
        read.status, 200,
        "case {name:?}: expected 200 from DescribeDomain, got {} with {:?}",
        read.status, read.body
    );
    let carried = read.body["DomainStatus"]["AccessPolicies"].as_str();
    assert_eq!(
        carried,
        Some(text.as_str()),
        "case {name:?}: expected the document's text, got {carried:?}"
    );
    let parsed: Value = carried
        .and_then(|held| serde_json::from_str(held).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        parsed, document,
        "case {name:?}: expected the stored text to parse back to the document, got \
         {carried:?}"
    );
}
