// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TagResource` and `UntagResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The first rule under test: `TagResource` MERGES the request's key-value pairs into
//! the tags of the ONE resource `ResourceArn` names — a named key takes the request's
//! value whether or not the resource already carried it, and every key the request does
//! not name is left as it was — so a later `GetConfigurationManager` reports the merged
//! set and repeating the same request answers the same way (the operation is
//! `@idempotent`). An ARN naming no stored resource earns the modelled 404 with the fixed
//! sentence the tag plane's mutations state (`tests/all/target_authorization.rs`), and a
//! request that would leave the resource carrying more tags than the service accepts is
//! the modelled 400 with nothing stored.
//!
//! The second: `UntagResource` removes from the ONE resource `ResourceArn`
//! names exactly the tag keys the request lists, matched whole and case-sensitively,
//! leaves every other stored tag and every other resource as they were, and treats a
//! key the resource does not carry as nothing to do — so a later
//! `GetConfigurationManager` reports what is left and repeating the same request answers
//! the same way (the operation is `@idempotent`). An ARN naming no stored resource earns
//! that same modelled 404, and a request naming no key at all names no route of this
//! endpoint — `TagKeys` is `@required` and bound to the `tagKeys` query parameter — so it
//! is refused by the front door ahead of the handler, with nothing removed
//! (`tests/all/unroutable_requests.rs` owns that rule).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{MAX_TAGS, QUICK_SETUP_TYPE_CATALOG};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A well-formed `ManagerArn` (the modelled pattern ends in a uuid) that no create ever
/// minted, for the not-found cases.
const UNKNOWN_MANAGER_ARN: &str =
    "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/\
     00000000-0000-4000-8000-000000000000";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
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

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, which is what an SDK writes for an `@httpQuery` member.
///
/// `+` in particular HAS to be escaped: a query value is decoded with `+` meaning a
/// space, so an unescaped `+` would name a different tag key than the stored one.
/// [`label`] is the path-label encoder and cannot serve a query value, so the two live
/// side by side.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource`. `TagKeys` is an
/// `@httpQuery` list, so each key travels as its own repeated `tagKeys=` parameter.
async fn untag<K: AsRef<str>>(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[K],
) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key.as_ref())))
        .collect();
    let uri = if query.is_empty() {
        format!("/tags/{}", label(resource_arn))
    } else {
        format!("/tags/{}?{}", label(resource_arn), query.join("&"))
    };
    call(router, "DELETE", &uri, None).await
}

/// `PUT /tags/{ResourceArn}` — `TagResource`. `Tags` is a body member, so the map
/// travels in the restJson1 payload.
async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let mut tag_map = Map::new();
    for (key, value) in tags {
        tag_map.insert(key.clone(), Value::String(value.clone()));
    }
    let body = json!({ "Tags": Value::Object(tag_map) });
    call(
        router,
        "PUT",
        &format!("/tags/{}", label(resource_arn)),
        Some(&body),
    )
    .await
}

/// `GET /configurationManager/{ManagerArn}` — the implemented reader this suite reads
/// every assignment and every removal back through.
async fn get_manager(router: &MultiServiceRouter, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await
}

/// Create one configuration manager carrying `tags`, answering its ARN.
async fn create_manager(
    router: &MultiServiceRouter,
    case: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let mut tag_map = Map::new();
    for (key, value) in tags {
        tag_map.insert(key.clone(), Value::String(value.clone()));
    }
    // `Type` must name a configuration type this service publishes; the catalog's first
    // entry is the one every case here deploys, since no case is about the type. Its
    // parameters name somewhere to deploy to and the Region to deploy into, and provide
    // every parameter the table documented for that type marks `(Required)`, which every
    // accepted definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": "tagged-manager",
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
        "Tags": Value::Object(tag_map),
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The tags `GetConfigurationManager` reports for `manager_arn`.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = get_manager(router, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    let tags = answer
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {answer:?}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a map, got {tags:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} must be a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// The `LastModifiedAt` stamp `GetConfigurationManager` reports.
async fn last_modified_at(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = get_manager(router, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("LastModifiedAt")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LastModifiedAt, got {answer:?}"))
}

/// Tags as the table below writes them.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every character a tag key may carry, from `TagsMap`'s key constraints —
/// `@length(1, 128)` and `^[A-Za-z0-9 _=@:.+-/]+$`
/// (`smithy_query(query="shape",shape="TagsMap")`). One key per character class the
/// pattern admits beyond the alphanumerics, so a removal is exercised over keys whose
/// wire form needs percent-encoding as well as over plain ones.
const PUNCTUATED_KEYS: &[&str] = &[
    "cost center",
    "owner_group",
    "key=value",
    "owner@example",
    "ns:component",
    "release.candidate",
    "blue+green",
    "team/platform",
];

/// One row of the assignment table: what the resource was created carrying, which pairs
/// the request assigns, and what must be stored afterwards.
struct TagRow {
    name: &'static str,
    created: BTreeMap<String, String>,
    assign: BTreeMap<String, String>,
    expected: BTreeMap<String, String>,
}

fn tag_rows() -> Vec<TagRow> {
    let three = tags(&[("Env", "prod"), ("Team", "platform"), ("Owner", "ops")]);
    // The longest key and the longest value `TagsMap`'s `@length(1, 128)` key and
    // `@length(max 256)` value admit.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let punctuated: BTreeMap<String, String> = PUNCTUATED_KEYS
        .iter()
        .enumerate()
        .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
        .collect();
    // The most tags one resource may carry, assigned in one request to an untagged one.
    let full: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let mut overwrite_and_add = three.clone();
    overwrite_and_add.insert("Env".to_string(), "staging".to_string());
    overwrite_and_add.insert("Tier".to_string(), "gold".to_string());
    let mut case_distinct = tags(&[("Env", "prod")]);
    case_distinct.insert("env".to_string(), "dev".to_string());
    let mut punctuated_added = three.clone();
    punctuated_added.extend(punctuated.clone());
    vec![
        TagRow {
            name: "a key the resource does not carry",
            created: tags(&[("Env", "prod")]),
            assign: tags(&[("Team", "platform")]),
            expected: tags(&[("Env", "prod"), ("Team", "platform")]),
        },
        TagRow {
            name: "a key the resource already carries",
            created: tags(&[("Env", "prod"), ("Team", "platform")]),
            assign: tags(&[("Env", "staging")]),
            expected: tags(&[("Env", "staging"), ("Team", "platform")]),
        },
        TagRow {
            name: "an overwritten key beside a new one",
            created: three.clone(),
            assign: tags(&[("Env", "staging"), ("Tier", "gold")]),
            expected: overwrite_and_add,
        },
        TagRow {
            name: "re-assigning the value a key already holds",
            created: three.clone(),
            assign: tags(&[("Env", "prod")]),
            expected: three.clone(),
        },
        TagRow {
            name: "no pair at all (an empty map)",
            created: three.clone(),
            assign: BTreeMap::new(),
            expected: three.clone(),
        },
        TagRow {
            name: "a resource created with no tags at all",
            created: BTreeMap::new(),
            assign: tags(&[("Env", "prod")]),
            expected: tags(&[("Env", "prod")]),
        },
        TagRow {
            name: "a key differing from a stored one only in case",
            created: tags(&[("Env", "prod")]),
            assign: tags(&[("env", "dev")]),
            expected: case_distinct,
        },
        TagRow {
            name: "keys carrying every punctuation the tag-key form admits",
            created: three.clone(),
            assign: punctuated,
            expected: punctuated_added,
        },
        TagRow {
            name: "the longest key and value the published bounds admit",
            created: tags(&[("Env", "prod")]),
            assign: tags(&[(longest_key.as_str(), longest_value.as_str())]),
            expected: tags(&[
                ("Env", "prod"),
                (longest_key.as_str(), longest_value.as_str()),
            ]),
        },
        TagRow {
            name: "as many pairs as one resource may carry",
            created: BTreeMap::new(),
            assign: full.clone(),
            expected: full,
        },
    ]
}

/// Each row: the assignment answers 200 with an empty body, and the stored tags a later
/// read reports are exactly the row's expectation.
#[tokio::test]
async fn tag_resource_merges_exactly_the_named_pairs() {
    for row in tag_rows() {
        let router = emulator();
        let arn = create_manager(&router, row.name, &row.created).await;

        let (status, answer) = tag(&router, &arn, &row.assign).await;
        assert_eq!(
            status, 200,
            "case {:?}: TagResource must answer 200, got {status} / {answer:?}",
            row.name
        );
        assert_eq!(
            answer,
            Value::Null,
            "case {:?}: TagResource's modelled output is empty, got {answer:?}",
            row.name
        );

        let stored = stored_tags(&router, row.name, &arn).await;
        assert_eq!(
            stored, row.expected,
            "case {:?}: expected the stored tags {:?}, got {stored:?}",
            row.name, row.expected
        );
    }
}

/// Repeating an assignment changes nothing further: the second call succeeds, the tags
/// it leaves are the ones the first left, and the stamp a read reports does not move for
/// a request that stored no new value.
#[tokio::test]
async fn tag_resource_is_idempotent() {
    let case = "repeated assignment";
    let router = emulator();
    let arn = create_manager(&router, case, &tags(&[("Env", "prod")])).await;

    let assign = tags(&[("Env", "staging"), ("Team", "platform")]);
    let (first, answer) = tag(&router, &arn, &assign).await;
    assert_eq!(
        first, 200,
        "case {case:?}: the first TagResource must answer 200, got {first} / {answer:?}"
    );
    let after_first = stored_tags(&router, case, &arn).await;
    let stamp_after_first = last_modified_at(&router, case, &arn).await;

    let (second, answer) = tag(&router, &arn, &assign).await;
    assert_eq!(
        second, 200,
        "case {case:?}: repeating TagResource must answer 200, got {second} / {answer:?}"
    );
    let after_second = stored_tags(&router, case, &arn).await;
    assert_eq!(
        after_second, after_first,
        "case {case:?}: expected the repeat to leave {after_first:?}, got {after_second:?}"
    );
    assert_eq!(
        after_second, assign,
        "case {case:?}: expected the assigned pairs {assign:?} to be what is stored, \
         got {after_second:?}"
    );
    let stamp_after_second = last_modified_at(&router, case, &arn).await;
    assert_eq!(
        stamp_after_second, stamp_after_first,
        "case {case:?}: an assignment that stored no new value must not move \
         LastModifiedAt, expected {stamp_after_first:?}, got {stamp_after_second:?}"
    );
}

/// An ARN naming no stored resource earns this operation's own modelled refusal — HTTP
/// 404 `ResourceNotFoundException`, with the fixed sentence the tag plane's mutations
/// state for an absent resource, whatever form that ARN takes — and an assignment against
/// one stores nothing.
#[tokio::test]
async fn tag_resource_rejects_an_arn_naming_no_resource() {
    let router = emulator();
    // A manager that exists, so the refusals below are about the ARN rather than an empty
    // store.
    let live = create_manager(&router, "a live manager", &tags(&[("Env", "prod")])).await;
    let deleted = create_manager(&router, "a deleted manager", &tags(&[("Env", "prod")])).await;
    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/configurationManager/{}", label(&deleted)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );
    // A delete starts an asynchronous teardown, so the manager it named is gone from
    // the store once that teardown comes due: wait for that condition, not for a
    // duration (`tests/all/target_authorization.rs` owns the one wait).
    super::target_authorization::poll_until_absent(&router, "a deleted manager", &deleted).await;

    for (case, arn) in [
        (
            "a well-formed ARN no create minted",
            UNKNOWN_MANAGER_ARN.to_string(),
        ),
        ("the ARN of a deleted manager", deleted),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
        (
            "an ARN of another service",
            "arn:aws:ssm:us-west-2:000000000000:document/doc".to_string(),
        ),
    ] {
        let (status, answer) = tag(&router, &arn, &tags(&[("Team", "platform")])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let expected = "Resource arn not found.";
        assert_eq!(
            answer.get("Message").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }

    let stored = stored_tags(&router, "a live manager", &live).await;
    assert_eq!(
        stored,
        tags(&[("Env", "prod")]),
        "the live manager's tags must be untouched by the refused assignments, got {stored:?}"
    );
}

/// A `Tags` map this service cannot serve is the modelled 400, and nothing is stored for
/// it — whether the request itself is too large or the merged result would be.
#[tokio::test]
async fn tag_resource_rejects_an_unservable_tags_map() {
    let one_too_many: BTreeMap<String, String> = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let full: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    // Each refused row keeps the tags it was created with: `expected` is `created`.
    let cases = vec![
        TagRow {
            name: "one pair more than the service accepts in a single request",
            created: tags(&[("Env", "prod")]),
            assign: one_too_many,
            expected: tags(&[("Env", "prod")]),
        },
        TagRow {
            name: "a new key beyond the most tags a resource may carry",
            created: full.clone(),
            assign: tags(&[("Env", "prod")]),
            expected: full.clone(),
        },
    ];

    for row in cases {
        let case = row.name;
        let router = emulator();
        let arn = create_manager(&router, case, &row.created).await;

        let (status, answer) = tag(&router, &arn, &row.assign).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );

        let stored = stored_tags(&router, case, &arn).await;
        assert_eq!(
            stored, row.expected,
            "case {case:?}: a refused assignment must store nothing, expected {:?}, \
             got {stored:?}",
            row.expected
        );
    }

    // A resource already holding the most tags it may carry still takes a value for a
    // key it ALREADY carries: that assignment adds no entry.
    let router = emulator();
    let case = "overwriting a key on a resource already at the bound";
    let arn = create_manager(&router, case, &full).await;
    let (status, answer) = tag(&router, &arn, &tags(&[("key-0", "replaced")])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} / {answer:?}"
    );
    let stored = stored_tags(&router, case, &arn).await;
    let mut expected = full;
    expected.insert("key-0".to_string(), "replaced".to_string());
    assert_eq!(
        stored, expected,
        "case {case:?}: expected the overwritten value to be stored, got {stored:?}"
    );
}

/// An assignment reaches only the resource it names: another manager keeps its own tags.
#[tokio::test]
async fn tag_resource_touches_only_the_named_resource() {
    let case = "two managers carrying the same keys";
    let router = emulator();
    let shared = tags(&[("Env", "prod"), ("Team", "platform")]);
    let first = create_manager(&router, case, &shared).await;
    let second = create_manager(&router, case, &shared).await;

    let (status, answer) = tag(&router, &first, &tags(&[("Env", "staging"), ("Tier", "gold")])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must answer 200, got {status} / {answer:?}"
    );

    let stored_first = stored_tags(&router, case, &first).await;
    assert_eq!(
        stored_first,
        tags(&[("Env", "staging"), ("Team", "platform"), ("Tier", "gold")]),
        "case {case:?}: the named manager must carry the merged set, got {stored_first:?}"
    );
    let stored_second = stored_tags(&router, case, &second).await;
    assert_eq!(
        stored_second, shared,
        "case {case:?}: the other manager must keep {shared:?}, got {stored_second:?}"
    );
}

/// A tag this service stores is removable again: `TagResource` and `UntagResource` meet
/// on the one stored record, so a key assigned by one is gone after the other names it.
#[tokio::test]
async fn an_assigned_tag_is_removable_again() {
    let case = "assign then remove";
    let router = emulator();
    let arn = create_manager(&router, case, &BTreeMap::new()).await;

    let (status, answer) = tag(&router, &arn, &tags(&[("Env", "prod"), ("Team", "platform")])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must answer 200, got {status} / {answer:?}"
    );
    let (status, answer) = untag(&router, &arn, &["Env"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must answer 200, got {status} / {answer:?}"
    );

    let stored = stored_tags(&router, case, &arn).await;
    assert_eq!(
        stored,
        tags(&[("Team", "platform")]),
        "case {case:?}: expected only the untouched assigned tag to remain, got {stored:?}"
    );
}

/// One row of the removal table: what the resource was created carrying, which keys the
/// request names, and what must be left.
struct Row {
    name: &'static str,
    created: BTreeMap<String, String>,
    remove: Vec<String>,
    expected: BTreeMap<String, String>,
}

fn rows() -> Vec<Row> {
    let three = tags(&[("Env", "prod"), ("Team", "platform"), ("Owner", "ops")]);
    // The longest key `TagsMap`'s key `@length(1, 128)` admits.
    let longest = "k".repeat(128);
    let punctuated: BTreeMap<String, String> = PUNCTUATED_KEYS
        .iter()
        .enumerate()
        .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
        .collect();
    let punctuated_removed: Vec<String> = PUNCTUATED_KEYS
        .iter()
        .step_by(2)
        .map(|key| (*key).to_string())
        .collect();
    let punctuated_left: BTreeMap<String, String> = punctuated
        .iter()
        .filter(|(key, _)| !punctuated_removed.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    vec![
        Row {
            name: "one of several keys",
            created: three.clone(),
            remove: vec!["Team".to_string()],
            expected: tags(&[("Env", "prod"), ("Owner", "ops")]),
        },
        Row {
            name: "two keys in one request",
            created: three.clone(),
            remove: vec!["Env".to_string(), "Owner".to_string()],
            expected: tags(&[("Team", "platform")]),
        },
        Row {
            name: "every key the resource carries",
            created: three.clone(),
            remove: vec![
                "Env".to_string(),
                "Team".to_string(),
                "Owner".to_string(),
            ],
            expected: BTreeMap::new(),
        },
        Row {
            name: "a key the resource does not carry",
            created: three.clone(),
            remove: vec!["Absent".to_string()],
            expected: three.clone(),
        },
        Row {
            name: "a present key beside an absent one",
            created: three.clone(),
            remove: vec!["Env".to_string(), "Absent".to_string()],
            expected: tags(&[("Team", "platform"), ("Owner", "ops")]),
        },
        Row {
            name: "the same key named twice in one request",
            created: three.clone(),
            remove: vec!["Env".to_string(), "Env".to_string()],
            expected: tags(&[("Team", "platform"), ("Owner", "ops")]),
        },
        Row {
            name: "a key differing from a stored one only in case",
            created: tags(&[("Env", "prod")]),
            remove: vec!["env".to_string()],
            expected: tags(&[("Env", "prod")]),
        },
        Row {
            name: "keys carrying every punctuation the tag-key form admits",
            created: punctuated,
            remove: punctuated_removed,
            expected: punctuated_left,
        },
        Row {
            name: "the longest key the published bound admits",
            created: tags(&[(longest.as_str(), "long"), ("Env", "prod")]),
            remove: vec![longest.clone()],
            expected: tags(&[("Env", "prod")]),
        },
        Row {
            name: "a key no stored tag may carry (the empty string)",
            created: three.clone(),
            remove: vec![String::new()],
            expected: three.clone(),
        },
        Row {
            name: "a resource created with no tags at all",
            created: BTreeMap::new(),
            remove: vec!["Env".to_string()],
            expected: BTreeMap::new(),
        },
    ]
}

/// Each row: the removal answers 200 with an empty body, and the stored tags a later
/// read reports are exactly the row's expectation.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    for row in rows() {
        let router = emulator();
        let arn = create_manager(&router, row.name, &row.created).await;

        let (status, answer) = untag(&router, &arn, &row.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagResource must answer 200, got {status} / {answer:?}",
            row.name
        );
        assert_eq!(
            answer,
            Value::Null,
            "case {:?}: UntagResource's modelled output is empty, got {answer:?}",
            row.name
        );

        let left = stored_tags(&router, row.name, &arn).await;
        assert_eq!(
            left, row.expected,
            "case {:?}: expected the stored tags {:?}, got {left:?}",
            row.name, row.expected
        );
    }
}

/// Repeating a removal changes nothing further: the second call succeeds, the tags it
/// leaves are the ones the first left, and the stamp a read reports does not move for a
/// request that removed nothing.
#[tokio::test]
async fn untag_resource_is_idempotent() {
    let case = "repeated removal";
    let router = emulator();
    let arn = create_manager(
        &router,
        case,
        &tags(&[("Env", "prod"), ("Team", "platform")]),
    )
    .await;

    let (first, answer) = untag(&router, &arn, &["Env"]).await;
    assert_eq!(
        first, 200,
        "case {case:?}: the first UntagResource must answer 200, got {first} / {answer:?}"
    );
    let after_first = stored_tags(&router, case, &arn).await;
    let stamp_after_first = last_modified_at(&router, case, &arn).await;

    let (second, answer) = untag(&router, &arn, &["Env"]).await;
    assert_eq!(
        second, 200,
        "case {case:?}: repeating UntagResource must answer 200, got {second} / {answer:?}"
    );
    let after_second = stored_tags(&router, case, &arn).await;
    assert_eq!(
        after_second, after_first,
        "case {case:?}: expected the repeat to leave {after_first:?}, got {after_second:?}"
    );
    let stamp_after_second = last_modified_at(&router, case, &arn).await;
    assert_eq!(
        stamp_after_second, stamp_after_first,
        "case {case:?}: a removal that removed nothing must not move LastModifiedAt, \
         expected {stamp_after_first:?}, got {stamp_after_second:?}"
    );
    assert_eq!(
        after_second,
        tags(&[("Team", "platform")]),
        "case {case:?}: expected only the untouched tag to remain, got {after_second:?}"
    );
}

/// An ARN naming no stored resource earns this operation's own modelled refusal — HTTP
/// 404 `ResourceNotFoundException`, with the fixed sentence the tag plane's mutations
/// state for an absent resource, whatever form that ARN takes — and a removal against one
/// stores nothing.
#[tokio::test]
async fn untag_resource_rejects_an_arn_naming_no_resource() {
    let router = emulator();
    // A manager that exists, so the refusals below are about the ARN rather than an empty
    // store.
    let live = create_manager(&router, "a live manager", &tags(&[("Env", "prod")])).await;
    let deleted = create_manager(&router, "a deleted manager", &tags(&[("Env", "prod")])).await;
    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/configurationManager/{}", label(&deleted)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );
    // A delete starts an asynchronous teardown, so the manager it named is gone from
    // the store once that teardown comes due: wait for that condition, not for a
    // duration (`tests/all/target_authorization.rs` owns the one wait).
    super::target_authorization::poll_until_absent(&router, "a deleted manager", &deleted).await;

    for (case, arn) in [
        ("a well-formed ARN no create minted", UNKNOWN_MANAGER_ARN.to_string()),
        ("the ARN of a deleted manager", deleted),
        ("a string that is no ARN at all", "not-an-arn".to_string()),
        (
            "an ARN of another service",
            "arn:aws:ssm:us-west-2:000000000000:document/doc".to_string(),
        ),
    ] {
        let (status, answer) = untag(&router, &arn, &["Env"]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let expected = "Resource arn not found.";
        assert_eq!(
            answer.get("Message").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }

    let left = stored_tags(&router, "a live manager", &live).await;
    assert_eq!(
        left,
        tags(&[("Env", "prod")]),
        "the live manager's tags must be untouched by the refused removals, got {left:?}"
    );
}

/// A `TagKeys` list this service cannot serve is the modelled 400, and nothing is
/// removed for it.
#[tokio::test]
async fn untag_resource_rejects_an_unservable_tag_key_list() {
    let stored = tags(&[("Env", "prod"), ("Team", "platform")]);
    let too_many: Vec<String> = (0..=MAX_TAGS).map(|index| format!("key-{index}")).collect();
    let cases: Vec<(&str, Vec<String>)> =
        vec![("one key more than the service accepts", too_many)];

    for (case, keys) in cases {
        let router = emulator();
        let arn = create_manager(&router, case, &stored).await;

        let (status, answer) = untag(&router, &arn, &keys).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );

        let left = stored_tags(&router, case, &arn).await;
        assert_eq!(
            left, stored,
            "case {case:?}: a refused removal must store nothing, expected {stored:?}, got {left:?}"
        );
    }
}

/// A removal reaches only the resource it names: another manager carrying the same tag
/// keys keeps them.
#[tokio::test]
async fn untag_resource_touches_only_the_named_resource() {
    let case = "two managers carrying the same keys";
    let router = emulator();
    let shared = tags(&[("Env", "prod"), ("Team", "platform")]);
    let first = create_manager(&router, case, &shared).await;
    let second = create_manager(&router, case, &shared).await;

    let (status, answer) = untag(&router, &first, &["Env", "Team"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must answer 200, got {status} / {answer:?}"
    );

    let left_first = stored_tags(&router, case, &first).await;
    assert_eq!(
        left_first,
        BTreeMap::new(),
        "case {case:?}: the named manager must have lost both keys, got {left_first:?}"
    );
    let left_second = stored_tags(&router, case, &second).await;
    assert_eq!(
        left_second, shared,
        "case {case:?}: the other manager must keep {shared:?}, got {left_second:?}"
    );
}
