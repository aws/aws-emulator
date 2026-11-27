// SPDX-License-Identifier: CC0-1.0

//! WHICH member a refusal of a caller-keyed map blames, over awsJson1_0.
//!
//! One rule, stated by the renderer in `src/state.rs` and reached through
//! `Service::override_refusal` for every operation of this service: a refusal is reported
//! against the member path the validating walk built, and that path is replaced by the
//! member holding a COLLECTION only where the walk really stepped into an element — which
//! it spells as the element's 1-based POSITION followed by `member`
//! (`aws_smithy_schema_server::constraints::path::element`). A map ENTRY carries no such
//! step: the walk names a failing map value by its key alone, and a key is text the CALLER
//! chose, echoed into the path exactly as sent ("The key is a value the CALLER chose, so it
//! is echoed exactly as sent"), so a key that SPELLS the step is still a key and the entry
//! keeps its own path.
//!
//! The rows span what the rule generalises over: the spelling of the caller's key (plain,
//! dotted, ending in the step marker, the marker alone, the marker twice, a key spelling a
//! whole path, and the marker in another case), the operation carrying the map
//! (`TagResource`, whose `tags` is `@required`, and `CreateEnvironment`, whose is optional),
//! how many entries the map carries and which one of them fails, and — on the other side of
//! the rule — a real LIST element, whose refusal IS reported against the member holding it
//! with the element's requirement bundled inside that member's.
//!
//! The constraints the rows break are the members' own, as `smithy_query` reports them for
//! this service's model
//! (`smithy_query(query="shape",shape="RequestTagMap,TagKeys",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`):
//!
//!   RequestTagMap -> map of TagKey (@length 1..128, @pattern ^[\w.:/=+-@]+$ — a dot is
//!       admitted, so a key may spell a dotted path) to TagValue (@length 0..256)
//!   TagKeys       -> list of TagKey (@length 1..128)

use serde_json::{json, Value};

use crate::environment_lifecycle::{call, emulator, settle, Emulator};

/// The modelled ceilings the rows below step one past, from
/// `smithy_query(query="shape",shape="TagValue,TagKey")`.
const MAX_TAG_VALUE_LEN: usize = 256;
const MAX_TAG_KEY_LEN: usize = 128;

/// A tag value one character past the modelled `TagValue` ceiling: the violation every
/// map row below is refused for.
fn over_long_value() -> String {
    "v".repeat(MAX_TAG_VALUE_LEN + 1)
}

/// A tag key one character past the modelled `TagKey` ceiling, which is what the
/// list-element rows are refused for.
fn over_long_key() -> String {
    "k".repeat(MAX_TAG_KEY_LEN + 1)
}

/// The requirement a value past the `TagValue` ceiling failed, as this service words it.
fn value_requirement() -> String {
    format!("Member must have length less than or equal to {MAX_TAG_VALUE_LEN}")
}

/// The requirement a key past the `TagKey` ceiling failed, as this service words it.
fn key_requirement() -> String {
    format!("Member must have length less than or equal to {MAX_TAG_KEY_LEN}")
}

/// One refused request: the member path the refusal must name, and the whole clause it
/// must state for it.
struct Row {
    /// What this row is a case of.
    case: String,
    operation: &'static str,
    body: Value,
    /// The member path the clause names and the `fieldList` entry is filed under.
    member: String,
    /// The requirement half of the clause, already bundled where the failure really is a
    /// collection element's.
    requirement: String,
}

/// Create one environment, settle the transition the clock owes it, and answer its ARN —
/// the `resourceArn` the tagging rows below address.
async fn tagged_environment(router: &Emulator) -> String {
    let (status, created) = call(
        router,
        "CreateEnvironment",
        &router.create_request(Some("tag-path-env")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the environment the rows address: expected 200, got {status} {created}"
    );
    settle(router);
    created["environment"]["environmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the created environment carries an ARN: {created}"))
        .to_string()
}

/// A `TagResource` body carrying one over-long value under `key`, beside `others`.
fn tag_resource_body(resource_arn: &str, key: &str, others: &[(&str, &str)]) -> Value {
    let mut tags = serde_json::Map::new();
    for (other_key, other_value) in others {
        tags.insert((*other_key).to_string(), json!(other_value));
    }
    tags.insert(key.to_string(), json!(over_long_value()));
    json!({ "resourceArn": resource_arn, "tags": Value::Object(tags) })
}

/// The `fieldList` the error body carries, as `(name, message)` pairs.
fn field_list(body: &Value) -> Vec<(String, String)> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every row's assertions: the refusal is this service's `ValidationException`, its one
/// clause names `member` and states `requirement`, and the `fieldList` files it under that
/// same member.
fn assert_refused_at(case: &str, status: u16, body: &Value, member: &str, requirement: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a value outside its modelled constraint is refused with HTTP 400, \
         got {status} with {body}"
    );
    assert_eq!(
        body["__type"].as_str().unwrap_or_default(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    let clause =
        format!("Value at '{member}' failed to satisfy constraint: {requirement}");
    let expected = format!("1 validation error detected: {clause}");
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    // The `fieldList` reports the same failure structurally: the member it is filed under
    // is the path the clause names, and its sentence is that clause.
    let failures = field_list(body);
    assert_eq!(
        failures,
        vec![(member.to_string(), clause.clone())],
        "case {case:?}: expected the fieldList entry {:?}, got {failures:?}",
        (member, clause.as_str())
    );
}

#[tokio::test]
async fn a_map_entry_refusal_keeps_the_entrys_own_path_whatever_the_caller_keyed_it() {
    let router = emulator();
    let arn = tagged_environment(&router).await;

    // Every row fails the SAME constraint on the SAME member — the map's value shape —
    // and differs only in the spelling of the caller's key, which is not structure.
    let keys = [
        ("a plain key", "env"),
        ("a dotted key", "env.other"),
        ("a key ending in the collection step marker", "env.member"),
        ("a key that is the marker alone", "member"),
        ("a key ending in the marker twice", "member.member"),
        ("a key spelling a whole member path", "tags.member"),
        ("a key ending in the marker in another case", "env.MEMBER"),
        ("a key whose last segment merely starts with the marker", "env.members"),
    ];

    let mut rows: Vec<Row> = Vec::new();
    for (case, key) in keys {
        rows.push(Row {
            case: format!("{case}, as the only entry of TagResource's map"),
            operation: "TagResource",
            body: tag_resource_body(&arn, key, &[]),
            member: format!("tags.{key}"),
            requirement: value_requirement(),
        });
        rows.push(Row {
            case: format!("{case}, as the failing entry beside two accepted ones"),
            operation: "TagResource",
            body: tag_resource_body(&arn, key, &[("team", "platform"), ("cost", "42")]),
            member: format!("tags.{key}"),
            requirement: value_requirement(),
        });
        // The same renderer serves every caller-keyed map of this service, so the rule
        // holds for the `tags` of a create as well.
        let mut create = router.create_request(Some("tag-path-create"));
        create["tags"] = json!({ key: over_long_value() });
        rows.push(Row {
            case: format!("{case}, in CreateEnvironment's optional map"),
            operation: "CreateEnvironment",
            body: create,
            member: format!("tags.{key}"),
            requirement: value_requirement(),
        });
    }

    for row in rows {
        let case = &row.case;
        let (status, body) = call(&router, row.operation, &row.body).await;
        assert_refused_at(case, status, &body, &row.member, &row.requirement);
    }
}

#[tokio::test]
async fn a_list_element_refusal_is_still_reported_against_the_member_holding_it() {
    let router = emulator();
    let arn = tagged_environment(&router).await;
    let long_key = over_long_key();

    // The other side of the rule: where the walk really DID step into an element, the
    // step is a 1-based position, the refusal is reported against the member holding the
    // collection, and the element's requirement is bundled inside that member's.
    let bundled = format!("Member must satisfy constraint: [{}]", key_requirement());
    let rows = [
        Row {
            case: "the only element of a list of tag keys".to_string(),
            operation: "UntagResource",
            body: json!({ "resourceArn": arn, "tagKeys": [long_key] }),
            member: "tagKeys".to_string(),
            requirement: bundled.clone(),
        },
        Row {
            case: "the second element of the same list".to_string(),
            operation: "UntagResource",
            body: json!({ "resourceArn": arn, "tagKeys": ["team", long_key] }),
            member: "tagKeys".to_string(),
            requirement: bundled.clone(),
        },
        Row {
            case: "an element of a list whose other entries are accepted".to_string(),
            operation: "UntagResource",
            body: json!({ "resourceArn": arn, "tagKeys": ["team", "cost", long_key] }),
            member: "tagKeys".to_string(),
            requirement: bundled.clone(),
        },
    ];

    for row in rows {
        let case = &row.case;
        let (status, body) = call(&router, row.operation, &row.body).await;
        assert_refused_at(case, status, &body, &row.member, &row.requirement);
    }
}

#[tokio::test]
async fn a_key_spelling_the_collection_step_is_stored_and_read_back_as_a_tag_key() {
    // The keys the rows above are refused OVER are ordinary tag keys: with a value inside
    // the modelled ceiling the same requests are served, and the keys come back exactly as
    // sent — which is what makes the path they appear in caller data rather than structure.
    let router = emulator();
    let arn = tagged_environment(&router).await;
    let tags = json!({
        "env.member": "prod",
        "member": "1",
        "member.member": "nested",
        "tags.member": "spelled",
        "env.MEMBER": "cased",
    });

    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case tagging with keys that spell the collection step: expected 200, got {status} \
         {body}"
    );

    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case reading those tags back: expected 200, got {status} {listed}"
    );
    assert_eq!(
        listed["tags"], tags,
        "case reading those tags back: expected {tags}, got {}",
        listed["tags"]
    );
}
