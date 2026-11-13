// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the shape a TAG KEY has to be inside, which no request boundary
//! applies.
//!
//! The rule under test: a tag key the request carries is judged against the constraints the
//! model declares for the tag-key shape — 1 to 128 characters of the Unicode letter,
//! separator and number categories plus `_.:/=+-@` — and a map carrying a key outside them
//! is refused with one clause at the map member, before anything is written. The framework's
//! constraint walk captures a map's key as the path segment of that entry's VALUE and never
//! checks it, so every operation that accepts a tag map applies the key shape itself.
//!
//! Every assertion drives real HTTP requests through the router, and every write that is
//! accepted or refused is read back through `ListTags` or `GetFunction`, so nothing is
//! trusted to the write's own answer.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, function_arn, json_call, package_bytes, router_on, zip_file, Answer, ROLE,
};

/// Where the tag operations are bound (`/2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// The refusal class a broken constraint travels in: the request's SHAPE is what is wrong,
/// so it is answered the way the framework's own constraint walk answers one.
const VALIDATION: &str = "ValidationException";

/// The one clause a tag map carrying a key outside its shape is refused with: the map the
/// offending key was held in, and every requirement a key of it has.
///
/// Built from what the model declares for the key of a `Tags` map
/// (`smithy_query(query="shape", shape="Tags,TagKey")`: `@length(min: 1, max: 128)` and the
/// pattern below), with the expression rendered the way every clause this service states
/// renders one — without the anchors a whole-value pattern is written with.
fn tag_key_clause(quoted_map: &str) -> String {
    format!(
        "1 validation error detected: Value '{quoted_map}' at 'tags' failed to satisfy \
         constraint: Map keys must satisfy constraint: [Member must have length less than or \
         equal to 128, Member must have length greater than or equal to 1, Member must \
         satisfy regular expression pattern: ([\\p{{L}}\\p{{Z}}\\p{{N}}_.:/=+\\-@]*)]"
    )
}

/// The body of an answer, for a failure message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The `message` of a wire error, however the protocol spells the member.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// A one-entry `Tags` map under `key`, built directly so a key carrying any character is
/// sent exactly as the row states it.
fn one_tag(key: &str) -> Value {
    let mut map = ::serde_json::Map::new();
    map.insert(key.to_string(), json!("v"));
    Value::Object(map)
}

/// `TagResource` on one resource ARN.
async fn tag_resource(router: &MultiServiceRouter, resource: &str, tags: &Value) -> Answer {
    let uri = format!("{TAGS_URI}/{resource}");
    json_call(router, "POST", &uri, &json!({ "Tags": tags })).await
}

/// `ListTags` on one resource ARN — the read every write below is checked against.
async fn list_tags(router: &MultiServiceRouter, resource: &str) -> Answer {
    let uri = format!("{TAGS_URI}/{resource}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetFunction` on one function name.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// Create one function with the request members `extras` adds.
async fn create_named(router: &MultiServiceRouter, name: &str, extras: &Value) -> Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    create_function(router, &body).await
}

/// One tag key and whether its shape admits it.
struct Key {
    /// The case name.
    case: &'static str,
    /// The key the request carries.
    key: String,
    /// Whether the modelled key shape admits it.
    admitted: bool,
}

/// The keys the rule is asserted over: each character class the pattern admits, a character
/// outside every one of them, and both `@length` bounds with one past each.
fn keys() -> Vec<Key> {
    let longest = 128;
    vec![
        Key {
            case: "letters",
            key: "Owner".to_string(),
            admitted: true,
        },
        Key {
            case: "letters and numbers",
            key: "team7".to_string(),
            admitted: true,
        },
        Key {
            case: "a separator",
            key: "cost centre".to_string(),
            admitted: true,
        },
        Key {
            case: "every punctuation character the pattern names",
            key: "_.:/=+-@".to_string(),
            admitted: true,
        },
        Key {
            case: "a non-ASCII letter",
            key: "équipe".to_string(),
            admitted: true,
        },
        Key {
            case: "the longest admitted key",
            key: "k".repeat(longest),
            admitted: true,
        },
        Key {
            case: "one character past the longest admitted key",
            key: "k".repeat(longest + 1),
            admitted: false,
        },
        Key {
            case: "an exclamation mark",
            key: "bad!key".to_string(),
            admitted: false,
        },
        Key {
            case: "a percent sign",
            key: "cost%".to_string(),
            admitted: false,
        },
        Key {
            case: "a brace",
            key: "{owner}".to_string(),
            admitted: false,
        },
        Key {
            case: "a comma",
            key: "team,owner".to_string(),
            admitted: false,
        },
        Key {
            case: "a tab",
            key: "team\towner".to_string(),
            admitted: false,
        },
    ]
}

/// RULE: a tag key outside the shape the model declares for it is refused with the clause
/// stating every requirement a key has, and the resource keeps the tag set it had; a key
/// inside the shape is stored and read back.
#[tokio::test]
async fn a_tag_key_outside_its_modelled_shape_is_refused_by_tag_resource() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let created = create_named(&router, "tag-shape-fn", &json!({})).await;
    assert_eq!(
        created.status,
        201,
        "setup: expected 201 from CreateFunction, got {} {}",
        created.status,
        body_text(&created)
    );
    let resource = function_arn("tag-shape-fn");

    let mut stored: Vec<String> = Vec::new();
    for row in keys() {
        let answered = tag_resource(&router, &resource, &one_tag(&row.key)).await;
        if row.admitted {
            assert_eq!(
                answered.status,
                204,
                "case {:?}: expected 204 from TagResource, got {} {}",
                row.case,
                answered.status,
                body_text(&answered)
            );
            stored.push(row.key.clone());
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, VALIDATION.to_string()),
                "case {:?}: expected 400 {VALIDATION}, got {} {}",
                row.case,
                answered.status,
                body_text(&answered)
            );
            let reported = message_of(&answered);
            let expected = tag_key_clause(&format!("{{{}=v}}", row.key));
            assert_eq!(
                reported, expected,
                "case {:?}: expected the message {expected:?}, got {reported:?}",
                row.case
            );
        }

        // Whatever the row, the resource holds exactly the keys the ACCEPTED rows stored:
        // a refused write changes nothing a later read sees.
        let listed = list_tags(&router, &resource).await;
        assert_eq!(
            listed.status,
            200,
            "case {:?}: expected 200 from ListTags, got {} {}",
            row.case,
            listed.status,
            body_text(&listed)
        );
        let tags = listed.json()["Tags"].clone();
        let held = tags.as_object().cloned().unwrap_or_default();
        let mut names: Vec<String> = held.keys().cloned().collect();
        names.sort();
        let mut expected = stored.clone();
        expected.sort();
        assert_eq!(
            names, expected,
            "case {:?}: expected the resource to hold {expected:?}, got {names:?}",
            row.case
        );
    }
}

/// RULE: the same shape binds the tags a CREATE carries, so a create naming a key outside
/// it is refused and no function is stored — the rule is the tag map's, not one
/// operation's.
#[tokio::test]
async fn a_tag_key_outside_its_modelled_shape_is_refused_by_create_function() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, row) in keys().into_iter().enumerate() {
        let name = format!("create-tag-{index}");
        let answered = create_named(&router, &name, &json!({ "Tags": one_tag(&row.key) })).await;
        let read = get_function(&router, &name).await;
        if row.admitted {
            assert_eq!(
                answered.status,
                201,
                "case {:?}: expected 201 from CreateFunction, got {} {}",
                row.case,
                answered.status,
                body_text(&answered)
            );
            assert_eq!(
                read.json()["Tags"][row.key.as_str()],
                json!("v"),
                "case {:?}: expected GetFunction to report the stored tag, got {}",
                row.case,
                body_text(&read)
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, VALIDATION.to_string()),
                "case {:?}: expected 400 {VALIDATION}, got {} {}",
                row.case,
                answered.status,
                body_text(&answered)
            );
            let reported = message_of(&answered);
            let expected = tag_key_clause(&format!("{{{}=v}}", row.key));
            assert_eq!(
                reported, expected,
                "case {:?}: expected the message {expected:?}, got {reported:?}",
                row.case
            );
            assert_eq!(
                read.status,
                404,
                "case {:?}: expected the refused create to store no function, got {} {}",
                row.case,
                read.status,
                body_text(&read)
            );
        }
    }
}
