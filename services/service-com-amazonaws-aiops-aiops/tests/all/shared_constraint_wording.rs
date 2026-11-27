// SPDX-License-Identifier: CC0-1.0

//! The ONE home of the sentences a refusal of this service is assembled from.
//!
//! The rule under test: the REQUIREMENT half of a constraint clause, and the bracketed
//! constraint SET a clause states at a map member about that map's KEYS or its VALUES,
//! are each composed by one shared function — `service_common::validation_message`'s
//! `requirement`, `map_key_set_requirement` and `map_value_set_requirement` — so a
//! refusal this service's handlers state and one the framework renders at the request
//! boundary cannot word a constraint two ways. No response byte differs from before:
//! every row below is the sentence the recordings show, expressed as the shared home
//! composes it, so a rewording on either side fails here instead of drifting silently.
//!
//! Every wire row drives the real path — the production `MultiServiceRouter`, restJson1
//! deserialization (including the `@httpLabel` `resourceArn`/`identifier`), the
//! centralized constraint walk and this service's handlers — so what is asserted is what
//! a signed AWS SDK request observes. The rows span an operation for each place a
//! requirement or a constraint set is stated:
//!
//! * the walk's own map clause, for a `tags` VALUE and for an element of a
//!   `chatbotNotificationChannel` entry;
//! * the handler-applied KEY rule, for `tags` and for `chatbotNotificationChannel`;
//! * the handler-applied value-COLLECTION rule, for a `chatbotNotificationChannel`
//!   entry's size;
//! * a plain scalar clause, present (`roleArn`, `name`) and absent (`roleArn`), where the
//!   requirement stands on its own;
//!
//! across `CreateInvestigationGroup`, `UpdateInvestigationGroup` and `TagResource`, and
//! one request breaking two of these rules at once, which is two clauses in one envelope.
//!
//! An account holds ONE investigation group per Region, so the rows that create a group
//! sign for a Region of their own.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state;
use service_common::validation_message::{
    map_key_set_requirement, map_value_set_requirement, requirement,
};
use smithy_server_runtime::{MultiServiceRouter, ViolationKind};

/// The Region holding the group the update and tagging rows drive.
const REGION: &str = "us-west-2";

/// The Region every `CreateInvestigationGroup` row signs for: its own, so a refusal can
/// only have come from the rule under test and never from the per-Region group quota.
const CREATE_REGION: &str = "eu-central-1";

/// A role ARN that satisfies the modelled `RoleArn` (`@length` 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// An SNS topic ARN a `chatbotNotificationChannel` entry may be keyed by, satisfying the
/// modelled `SNSTopicArn` (`@length` 20..2048, `^arn:.*$`).
const TOPIC_ARN: &str = "arn:aws:sns:us-west-2:000000000000:my-private-topic";

/// The error code every modelled-constraint refusal carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `TagKey`'s modelled constraints (`smithy_query(query="shape",shape="Tags")`: the map's
/// key targets `TagKey`, `@length` 1..128, `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`), the expression as a clause reports it — without
/// the anchors a model spells a whole-string match with.
const TAG_KEY_MIN_LENGTH: i64 = 1;
const TAG_KEY_MAX_LENGTH: i64 = 128;
const TAG_KEY_PATTERN: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]+)";

/// `TagValue`'s modelled constraints (`smithy_query(query="shape",shape="Tags")`: the
/// map's value targets `TagValue`, `@length` 1..256, `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
const TAG_VALUE_MIN_LENGTH: i64 = 1;
const TAG_VALUE_MAX_LENGTH: i64 = 256;
const TAG_VALUE_PATTERN: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";

/// `SNSTopicArn`'s modelled constraints
/// (`smithy_query(query="shape",shape="ChatbotNotificationChannel")`: the map's key
/// targets `SNSTopicArn`, `@length` 20..2048, `@pattern` `^arn:.*$`).
const TOPIC_ARN_MIN_LENGTH: i64 = 20;
const TOPIC_ARN_MAX_LENGTH: i64 = 2048;

/// `ChatConfigurationArn`'s modelled constraints
/// (`smithy_query(query="shape",shape="ChatConfigurationArns")`: the list member targets
/// `ChatConfigurationArn`, `@length` 20..2048, `@pattern` `^arn:.*$`) — the shape a map
/// entry's ELEMENTS satisfy.
const CHAT_ARN_MIN_LENGTH: i64 = 20;
const CHAT_ARN_MAX_LENGTH: i64 = 2048;

/// The ARN `@pattern` both shapes above declare, as a clause reports it.
const ARN_PATTERN: &str = "arn:.*";

/// `ChatConfigurationArns`' own modelled `@length`
/// (`smithy_query(query="shape",shape="ChatConfigurationArns")`: 1..5) — the bound an
/// ENTRY of `chatbotNotificationChannel` satisfies.
const CHAT_ARNS_MIN_LENGTH: i64 = 1;
const CHAT_ARNS_MAX_LENGTH: i64 = 5;

/// `name`'s modelled `@pattern`
/// (`smithy_query(query="shape",shape="StringWithPatternAndLengthLimits")`), the one
/// expression this service reports a wider form of.
const MODEL_NAME_PATTERN: &str = r"^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status, error code, `message` and parsed body of one call.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, String, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let header_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(header_type)
        .unwrap_or_default()
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message, parsed)
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A `CreateInvestigationGroup` body carrying `members` beside the required ones.
fn create_body(members: Vec<(&str, Value)>) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!("shared-wording"));
    body.insert("roleArn".to_string(), json!(ROLE_ARN));
    for (member, value) in members {
        body.insert(member.to_string(), value);
    }
    Value::Object(body)
}

/// Create the one group of `region` and answer its ARN.
async fn create_group(router: &MultiServiceRouter, region: &str) -> String {
    let (status, code, message, body) = call(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&create_body(Vec::new())),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the group in {region}: expected 201, got {status} / {code} / {message:?}"
    );
    body.get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("creating the group in {region}: no arn in {body:?}"))
        .to_string()
}

/// `size` distinct configuration ARNs, each satisfying the modelled
/// `ChatConfigurationArn`, so an entry of that size breaks only its own `@length`.
fn chat_configuration_arns(size: usize) -> Vec<String> {
    (0..size)
        .map(|index| {
            format!("arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops-{index}")
        })
        .collect()
}

/// One clause of a `ValidationException` message: the member, and the requirement it
/// violated.
fn clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// One clause for a member the request did not carry at all.
fn absent_clause(member_path: &str, requirement: &str) -> String {
    format!("Value null at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The AWS envelope over the clauses of one message.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [one] => format!("1 validation error detected: {one}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The requirements `TagKey` states, as a clause bundles them.
fn tag_key_requirements() -> Vec<String> {
    vec![
        requirement(ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH)),
        requirement(ViolationKind::MinLength(TAG_KEY_MIN_LENGTH)),
        requirement(ViolationKind::Pattern(TAG_KEY_PATTERN.to_string())),
    ]
}

/// The requirements `TagValue` states, as a clause bundles them.
fn tag_value_requirements() -> Vec<String> {
    vec![
        requirement(ViolationKind::MaxLength(TAG_VALUE_MAX_LENGTH)),
        requirement(ViolationKind::MinLength(TAG_VALUE_MIN_LENGTH)),
        requirement(ViolationKind::Pattern(TAG_VALUE_PATTERN.to_string())),
    ]
}

/// The requirements `SNSTopicArn` states, as a clause bundles them.
fn topic_key_requirements() -> Vec<String> {
    vec![
        requirement(ViolationKind::MaxLength(TOPIC_ARN_MAX_LENGTH)),
        requirement(ViolationKind::MinLength(TOPIC_ARN_MIN_LENGTH)),
        requirement(ViolationKind::Pattern(ARN_PATTERN.to_string())),
    ]
}

/// The requirements `ChatConfigurationArn` states, as a clause bundles them.
fn chat_arn_requirements() -> Vec<String> {
    vec![
        requirement(ViolationKind::MaxLength(CHAT_ARN_MAX_LENGTH)),
        requirement(ViolationKind::MinLength(CHAT_ARN_MIN_LENGTH)),
        requirement(ViolationKind::Pattern(ARN_PATTERN.to_string())),
    ]
}

/// The requirements `ChatConfigurationArns` itself states, as a clause bundles them.
fn chat_arns_requirements() -> Vec<String> {
    vec![
        requirement(ViolationKind::MaxLength(CHAT_ARNS_MAX_LENGTH)),
        requirement(ViolationKind::MinLength(CHAT_ARNS_MIN_LENGTH)),
    ]
}

/// One wire row: the case, the request, and the message the service must answer with.
struct Case {
    name: String,
    region: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    expected: String,
}

#[tokio::test]
async fn the_shared_home_spells_every_requirement_and_constraint_set_this_service_states() {
    // The requirement half of a clause, per violation class: the shared sentence, the
    // service's own restatement of it, and the text both must spell. The rows differ
    // along the CLASS of constraint (each length bound, each value bound, a character
    // class, a value set, an absent member), which is what the derivation generalises
    // over.
    for (case, kind, expected) in [
        (
            "a length maximum",
            ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH),
            "Member must have length less than or equal to 128".to_string(),
        ),
        (
            "a length minimum",
            ViolationKind::MinLength(TAG_KEY_MIN_LENGTH),
            "Member must have length greater than or equal to 1".to_string(),
        ),
        (
            "a value maximum",
            ViolationKind::MaxValue("5".to_string()),
            "Member must have value less than or equal to 5".to_string(),
        ),
        (
            "a value minimum",
            ViolationKind::MinValue("1".to_string()),
            "Member must have value greater than or equal to 1".to_string(),
        ),
        (
            "a character class with no override",
            ViolationKind::Pattern(ARN_PATTERN.to_string()),
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
        ),
        (
            "a value set",
            ViolationKind::EnumValue(vec!["AWS_OWNED_KEY".to_string(), "CUSTOMER_MANAGED_KMS_KEY".to_string()]),
            "Member must satisfy enum value set: [AWS_OWNED_KEY, CUSTOMER_MANAGED_KMS_KEY]"
                .to_string(),
        ),
        (
            "a member the request left out",
            ViolationKind::MissingRequiredMember,
            "Member must not be null".to_string(),
        ),
    ] {
        let shared = requirement(kind.clone());
        assert_eq!(
            shared, expected,
            "case {case:?}: expected {expected:?}, got {shared:?}"
        );
        let stated = state::requirement_text(kind);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The one member whose live class is wider than the model's keeps its restatement:
    // the sentence is still the shared one, over the expression this service applies.
    let applied = state::applied_pattern(MODEL_NAME_PATTERN);
    let stated = state::requirement_text(ViolationKind::Pattern(MODEL_NAME_PATTERN.to_string()));
    let expected = requirement(ViolationKind::Pattern(applied.to_string()));
    assert_eq!(
        stated, expected,
        "case {:?}: expected {expected:?}, got {stated:?}",
        "the applied name class"
    );

    // The bracketed constraint SET, keys and values, over sets of one and of several
    // requirements.
    for (case, requirements, keys, values) in [
        (
            "a one-requirement set",
            vec!["Member must not be null".to_string()],
            "Map keys must satisfy constraint: [Member must not be null]",
            "Map value must satisfy constraint: [Member must not be null]",
        ),
        (
            "a two-requirement set",
            chat_arns_requirements(),
            "Map keys must satisfy constraint: [Member must have length less than or equal to 5, \
             Member must have length greater than or equal to 1]",
            "Map value must satisfy constraint: [Member must have length less than or equal to 5, \
             Member must have length greater than or equal to 1]",
        ),
    ] {
        let stated_keys = map_key_set_requirement(&requirements);
        assert_eq!(
            stated_keys, keys,
            "case {case:?}: expected {keys:?}, got {stated_keys:?}"
        );
        let stated_values = map_value_set_requirement(&requirements);
        assert_eq!(
            stated_values, values,
            "case {case:?}: expected {values:?}, got {stated_values:?}"
        );
    }
}

#[tokio::test]
async fn every_refusal_states_the_clause_the_shared_home_composes() {
    let router = emulator();
    let arn = create_group(&router, REGION).await;
    let tags_uri = format!("/tags/{}", label(&arn));
    let group_uri = format!("/investigationGroups/{}", label(&arn));
    let mut cases: Vec<Case> = Vec::new();

    // The handler-applied KEY rule, on both maps and three operations.
    let tag_key_clause = clause("tags", &map_key_set_requirement(&tag_key_requirements()));
    cases.push(Case {
        name: "TagResource with a key past the modelled maximum".to_string(),
        region: REGION,
        method: "POST",
        uri: tags_uri.clone(),
        body: json!({ "tags": { "k".repeat(TAG_KEY_MAX_LENGTH as usize + 1): "v" } }),
        expected: envelope(std::slice::from_ref(&tag_key_clause)),
    });
    cases.push(Case {
        name: "CreateInvestigationGroup with a key below the modelled minimum".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: create_body(vec![("tags", json!({ "": "v" }))]),
        expected: envelope(std::slice::from_ref(&tag_key_clause)),
    });
    let topic_key_clause = clause(
        "chatbotNotificationChannel",
        &map_key_set_requirement(&topic_key_requirements()),
    );
    cases.push(Case {
        name: "UpdateInvestigationGroup with a channel key that is no ARN".to_string(),
        region: REGION,
        method: "PATCH",
        uri: group_uri.clone(),
        body: json!({
            "chatbotNotificationChannel": {
                "not-an-arn-but-long-enough": chat_configuration_arns(1),
            },
        }),
        expected: envelope(&[topic_key_clause]),
    });

    // The walk's own map clause: a VALUE the request boundary refused, stated at the
    // member with the value shape's whole set.
    cases.push(Case {
        name: "TagResource with a value past the modelled maximum".to_string(),
        region: REGION,
        method: "POST",
        uri: tags_uri.clone(),
        body: json!({
            "tags": { "CostCentre-4711": "v".repeat(TAG_VALUE_MAX_LENGTH as usize + 1) },
        }),
        expected: envelope(&[clause(
            "tags",
            &map_value_set_requirement(&tag_value_requirements()),
        )]),
    });
    cases.push(Case {
        name: "UpdateInvestigationGroup with an element below the modelled minimum".to_string(),
        region: REGION,
        method: "PATCH",
        uri: group_uri.clone(),
        body: json!({ "chatbotNotificationChannel": { TOPIC_ARN: ["arn:short"] } }),
        expected: envelope(&[clause(
            "chatbotNotificationChannel",
            &map_value_set_requirement(&chat_arn_requirements()),
        )]),
    });

    // The handler-applied value-COLLECTION rule: an entry whose own size the boundary
    // cannot tell from the enclosing map's, stated with the same value wording.
    let entry_size_clause = clause(
        "chatbotNotificationChannel",
        &map_value_set_requirement(&chat_arns_requirements()),
    );
    cases.push(Case {
        name: "UpdateInvestigationGroup with an entry past the modelled maximum".to_string(),
        region: REGION,
        method: "PATCH",
        uri: group_uri.clone(),
        body: json!({
            "chatbotNotificationChannel": {
                TOPIC_ARN: chat_configuration_arns(CHAT_ARNS_MAX_LENGTH as usize + 1),
            },
        }),
        expected: envelope(std::slice::from_ref(&entry_size_clause)),
    });
    cases.push(Case {
        name: "CreateInvestigationGroup with an entry below the modelled minimum".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: create_body(vec![(
            "chatbotNotificationChannel",
            json!({ TOPIC_ARN: chat_configuration_arns(CHAT_ARNS_MIN_LENGTH as usize - 1) }),
        )]),
        expected: envelope(std::slice::from_ref(&entry_size_clause)),
    });

    // A plain scalar clause, where the requirement stands on its own: present, absent,
    // and the member whose character class this service reports wider than the model's.
    cases.push(Case {
        name: "CreateInvestigationGroup with a roleArn below the modelled minimum".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: json!({ "name": "shared-wording", "roleArn": "arn:aws" }),
        expected: envelope(&[clause(
            "roleArn",
            &requirement(ViolationKind::MinLength(TOPIC_ARN_MIN_LENGTH)),
        )]),
    });
    cases.push(Case {
        name: "CreateInvestigationGroup with no roleArn at all".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: json!({ "name": "shared-wording" }),
        expected: envelope(&[absent_clause(
            "roleArn",
            &requirement(ViolationKind::MissingRequiredMember),
        )]),
    });
    cases.push(Case {
        name: "CreateInvestigationGroup with a name outside the applied class".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: json!({ "name": "bad*name", "roleArn": ROLE_ARN }),
        expected: envelope(&[clause(
            "name",
            &requirement(ViolationKind::Pattern(
                state::applied_pattern(MODEL_NAME_PATTERN).to_string(),
            )),
        )]),
    });

    // Two of these rules broken at once: one clause each, counted by the envelope, the
    // key clause before the value clause as an entry is written.
    cases.push(Case {
        name: "CreateInvestigationGroup with a broken tag key and a broken entry size".to_string(),
        region: CREATE_REGION,
        method: "POST",
        uri: "/investigationGroups".to_string(),
        body: create_body(vec![
            ("tags", json!({ "bad key!": "v" })),
            (
                "chatbotNotificationChannel",
                json!({ TOPIC_ARN: chat_configuration_arns(CHAT_ARNS_MAX_LENGTH as usize + 1) }),
            ),
        ]),
        expected: envelope(&[tag_key_clause, entry_size_clause]),
    });

    for case in cases {
        let Case {
            name,
            region,
            method,
            uri,
            body,
            expected,
        } = case;
        let (status, code, message, _) = call(&router, region, method, &uri, Some(&body)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} / {code} / {message:?}"
        );
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {name:?}: expected {VALIDATION_EXCEPTION}, got {code} / {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
    }
}
