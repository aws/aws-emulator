// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `ListTagsForResource` reports exactly the tags persisted with
//! the ONE resource `ResourceArn` names — one `TagEntry` per stored key, carrying that
//! key's stored value — and nothing else: no entry for a key the resource does not
//! carry, and no entry belonging to another resource. Being `@readonly` it changes
//! nothing, so repeating it answers identically and every other reported value stays
//! where the last mutating call left it; a request whose ARN names no stored resource
//! earns this operation's own modelled `ResourceNotFoundException`
//! (`tests/all/target_authorization.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
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

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`, the operation under test.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource`, the mutation this suite
/// reads back through so the reported entries are shown to track stored state.
async fn untag<K: AsRef<str>>(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[K],
) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key.as_ref())))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// `GET /configurationManager/{ManagerArn}` — the other reader of the same record, used
/// to show a read changed nothing.
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

/// The `Tags` list of a successful `ListTagsForResource`, as the raw wire entries.
fn tag_entries<'a>(case: &str, answer: &'a Value) -> &'a Vec<Value> {
    answer
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {answer:?}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a list, got {answer:?}"))
}

/// The reported entries as a key/value map, checking each entry carries both modelled
/// members and that no key is reported twice.
fn reported_tags(case: &str, answer: &Value) -> BTreeMap<String, String> {
    let entries = tag_entries(case, answer);
    let mut map = BTreeMap::new();
    for entry in entries {
        let key = entry
            .get("Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: entry {entry:?} must carry a Key string"))
            .to_string();
        let value = entry
            .get("Value")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: entry {entry:?} must carry a Value string"))
            .to_string();
        assert!(
            map.insert(key.clone(), value).is_none(),
            "case {case:?}: key {key:?} is reported more than once, got {entries:?}"
        );
    }
    map
}

/// `ListTagsForResource` on `arn`, asserted to succeed, as a key/value map.
///
/// `pub(crate)` because the sibling suites of this one test binary read tags back through
/// the same reader: `all/reserved_tag_keys.rs` checks that a refused write leaves nothing
/// for it to report, and this is the one place the reported entries are parsed.
pub(crate) async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, answer) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {status} / {answer:?}"
    );
    reported_tags(case, &answer)
}

/// Tags as the table below writes them.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every character class a tag key may carry beyond the alphanumerics, from `TagEntry`'s
/// `Key` constraints — `@length(1, 128)` and `^[A-Za-z0-9 _=@:.+-/]+$`
/// (`smithy_query(query="shape",shape="TagEntry")`). One key per class, so the reported
/// entries are exercised over keys whose wire form needs percent-encoding as well as
/// over plain ones.
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

/// One row of the reporting table: the tags the resource was created carrying.
struct Row {
    name: &'static str,
    created: BTreeMap<String, String>,
}

fn rows() -> Vec<Row> {
    // The published bounds of `TagEntry`'s members: `Key` is `@length(1, 128)` and
    // `Value` is `@length(max 256)`, whose pattern admits no empty string.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let punctuated: BTreeMap<String, String> = PUNCTUATED_KEYS
        .iter()
        .enumerate()
        .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
        .collect();
    // One tag more than any row above, so the reported list is exercised at a size no
    // single-entry case reaches.
    let many: BTreeMap<String, String> = (0..24)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    vec![
        Row {
            name: "a resource created with no tags at all",
            created: BTreeMap::new(),
        },
        Row {
            name: "one tag",
            created: tags(&[("Env", "prod")]),
        },
        Row {
            name: "several tags",
            created: tags(&[("Env", "prod"), ("Team", "platform"), ("Owner", "ops")]),
        },
        Row {
            name: "two keys differing only in case",
            created: tags(&[("Env", "prod"), ("env", "staging")]),
        },
        Row {
            name: "two keys sharing one value",
            created: tags(&[("Env", "shared"), ("Stage", "shared")]),
        },
        Row {
            name: "keys carrying every punctuation the tag-key form admits",
            created: punctuated,
        },
        Row {
            name: "the shortest and longest key the published bounds admit",
            created: tags(&[("k", "short"), (longest_key.as_str(), "long")]),
        },
        Row {
            name: "the longest value the published bound admits",
            created: tags(&[("Env", longest_value.as_str()), ("Team", "p")]),
        },
        Row {
            name: "two dozen tags",
            created: many,
        },
    ]
}

/// Each row: the reported entries are exactly the stored tags, one entry per key, and
/// the list is as long as the stored tag count.
#[tokio::test]
async fn list_tags_for_resource_reports_exactly_the_stored_tags() {
    for row in rows() {
        let router = emulator();
        let arn = create_manager(&router, row.name, &row.created).await;

        let (status, answer) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource must answer 200, got {status} / {answer:?}",
            row.name
        );
        let entries = tag_entries(row.name, &answer).len();
        assert_eq!(
            entries,
            row.created.len(),
            "case {:?}: expected {} entries, got {entries}",
            row.name,
            row.created.len()
        );
        let reported = reported_tags(row.name, &answer);
        assert_eq!(
            reported, row.created,
            "case {:?}: expected the stored tags {:?}, got {reported:?}",
            row.name, row.created
        );
    }
}

/// The read changes nothing: repeating it reports the same entries in the same order,
/// and the other reader of the same record still reports the tags and the stamp the
/// create left.
#[tokio::test]
async fn list_tags_for_resource_changes_nothing() {
    for row in rows() {
        let router = emulator();
        let arn = create_manager(&router, row.name, &row.created).await;

        let (_, before) = get_manager(&router, &arn).await;
        let (status, first) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first ListTagsForResource must answer 200, got {status} / {first:?}",
            row.name
        );
        let (status, second) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the repeat must answer 200, got {status} / {second:?}",
            row.name
        );
        assert_eq!(
            tag_entries(row.name, &second),
            tag_entries(row.name, &first),
            "case {:?}: expected the repeat to report {:?}, got {:?}",
            row.name,
            tag_entries(row.name, &first),
            tag_entries(row.name, &second)
        );

        let (_, after) = get_manager(&router, &arn).await;
        for member in ["Tags", "LastModifiedAt", "CreatedAt"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {:?}: reading the tags must not move {member}, expected {:?}, got {:?}",
                row.name,
                before.get(member),
                after.get(member)
            );
        }
    }
}

/// What the read reports tracks the stored record: after a removal it reports what is
/// left, and the resource created with no tags still reports an empty list.
#[tokio::test]
async fn list_tags_for_resource_tracks_the_stored_record() {
    let case = "a removal read back";
    let router = emulator();
    let created = tags(&[("Env", "prod"), ("Team", "platform"), ("Owner", "ops")]);
    let arn = create_manager(&router, case, &created).await;

    let before = listed(&router, case, &arn).await;
    assert_eq!(
        before, created,
        "case {case:?}: expected the created tags {created:?}, got {before:?}"
    );

    let (status, answer) = untag(&router, &arn, &["Team", "Absent"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must answer 200, got {status} / {answer:?}"
    );

    let after = listed(&router, case, &arn).await;
    assert_eq!(
        after,
        tags(&[("Env", "prod"), ("Owner", "ops")]),
        "case {case:?}: expected only the untouched tags to be reported, got {after:?}"
    );

    let (status, answer) = untag(&router, &arn, &["Env", "Owner"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the second UntagResource must answer 200, got {status} / {answer:?}"
    );
    let emptied = listed(&router, case, &arn).await;
    assert_eq!(
        emptied,
        BTreeMap::new(),
        "case {case:?}: a resource whose every tag was removed must report none, got {emptied:?}"
    );
}

/// The read reports only the resource it names: two managers carrying different tags
/// each report their own.
#[tokio::test]
async fn list_tags_for_resource_reports_only_the_named_resource() {
    let case = "two managers carrying different tags";
    let router = emulator();
    let first_tags = tags(&[("Env", "prod"), ("Team", "platform")]);
    let second_tags = tags(&[("Env", "staging"), ("Owner", "ops")]);
    let first = create_manager(&router, case, &first_tags).await;
    let second = create_manager(&router, case, &second_tags).await;

    let reported_first = listed(&router, case, &first).await;
    assert_eq!(
        reported_first, first_tags,
        "case {case:?}: expected {first_tags:?} for the first manager, got {reported_first:?}"
    );
    let reported_second = listed(&router, case, &second).await;
    assert_eq!(
        reported_second, second_tags,
        "case {case:?}: expected {second_tags:?} for the second manager, got {reported_second:?}"
    );
}

/// An ARN naming no stored resource earns this operation's own modelled refusal: HTTP 404
/// `ResourceNotFoundException`, with the fixed sentence this operation states for an
/// absent resource, whatever form that ARN takes.
#[tokio::test]
async fn list_tags_for_resource_rejects_an_arn_naming_no_resource() {
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
        (
            "the live ARN in a different case",
            live.to_uppercase(),
        ),
    ] {
        let (status, answer) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let expected = "Configuration with requested id does not exist.";
        assert_eq!(
            answer.get("Message").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {answer:?}"
        );
    }

    let reported = listed(&router, "a live manager", &live).await;
    assert_eq!(
        reported,
        tags(&[("Env", "prod")]),
        "the live manager's tags must still be reported after the refused reads, got {reported:?}"
    );
}
