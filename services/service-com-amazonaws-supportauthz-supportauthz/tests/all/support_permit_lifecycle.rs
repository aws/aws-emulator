// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SupportAuthZ support-permit lifecycle.
//!
//! Every case drives a real HTTP request through the `MultiServiceRouter` — the same
//! path a socket-served request takes — so what is asserted is the wire behaviour, not
//! an internal function call. Nothing is seeded: the only way a permit gets into the
//! store here is `CreateSupportPermit`, and the only way anything is observed is a
//! modeled read.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_supportauthz_supportauthz as supportauthz;
use supportauthz::state::{DELETION_SPAN, MAX_SUPPORT_PERMITS};

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`supportauthz`), which is the routing signal the router reads first, and the
/// region a minted ARN is scoped to. The signature itself is never checked here: the
/// test router runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/eu-west-2/supportauthz/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region the `Authorization` header above signs for.
const REGION: &str = "eu-west-2";

/// The account segment every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A `kmsKey` value that satisfies the modeled `KmsKeyArn` pattern
/// `^[a-zA-Z0-9/:_-]{1,512}$` — note it admits no `.`.
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/1111-2222";

/// What one dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(supportauthz::EmulatedService::new(Arc::clone(engine)));
    router
}

fn new_router() -> MultiServiceRouter {
    build_router(&Arc::new(engine::InMemoryEngine::new()))
}

async fn dispatch(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Reply {
    let payload = match body {
        Some(body) => Bytes::from(serde_json::to_vec(body).expect("serialize the request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "supportauthz.eu-west-2.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            serde_json::from_slice::<Value>(response.body())
                .ok()
                .and_then(|body| {
                    body.get("__type")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        });
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// Percent-encode a value for use as an `@httpLabel` path segment or a query value: an
/// ARN carries a `/`, which would otherwise split the segment and stop the route
/// matching. Everything outside RFC 3986's unreserved set is escaped.
///
/// Hand-written rather than taken from `percent-encoding`: a dev-dependency here is a
/// `Cargo.lock` edit, and `Cargo.lock` is the one file in this change that mainline also
/// rewrites on every landing, which is what the squash merge kept refusing. Keeping the
/// change inside this service crate costs these eight lines.
fn encode_label(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Reply {
    dispatch(router, "POST", "/support-permits", Some(body)).await
}

async fn get_permit(router: &MultiServiceRouter, identifier: &str) -> Reply {
    let uri = format!("/support-permits/{}", encode_label(identifier));
    dispatch(router, "GET", &uri, None).await
}

async fn delete_permit(router: &MultiServiceRouter, identifier: &str) -> Reply {
    let uri = format!("/support-permits/{}", encode_label(identifier));
    dispatch(router, "DELETE", &uri, None).await
}

async fn list_permits(router: &MultiServiceRouter, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/support-permits".to_string()
    } else {
        format!("/support-permits?{query}")
    };
    dispatch(router, "GET", &uri, None).await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value) -> Reply {
    let uri = format!("/tags/{}", encode_label(arn));
    dispatch(router, "POST", &uri, Some(&json!({ "tags": tags }))).await
}

async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect();
    let uri = format!("/tags/{}?{}", encode_label(arn), query.join("&"));
    dispatch(router, "DELETE", &uri, None).await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    let uri = format!("/tags/{}", encode_label(arn));
    dispatch(router, "GET", &uri, None).await
}

/// Faithful check of the modeled `Arn` pattern
/// `^arn:[a-z0-9-]+:[a-z0-9-]+:[a-z0-9-]*:[0-9]{12}:.+$` (source:
/// `smithy_query(query="shape", shape="Arn")`). The resource portion is `.+`, so it may
/// itself contain colons — hence the limited split.
fn matches_arn_pattern(value: &str) -> bool {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let lower_alnum_dash =
        |s: &str| s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (1..=512).contains(&value.len())
        && fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && lower_alnum_dash(fields[1])
        && !fields[2].is_empty()
        && lower_alnum_dash(fields[2])
        && lower_alnum_dash(fields[3])
        && fields[4].len() == 12
        && fields[4].chars().all(|c| c.is_ascii_digit())
        && !fields[5].is_empty()
}

/// Faithful check of the modeled `NextToken` pattern `^[a-zA-Z0-9/+=%_-]+$` with its
/// `@length(1, 2048)` (source: `smithy_query(query="shape", shape="NextToken")`).
fn matches_next_token_pattern(value: &str) -> bool {
    (1..=2048).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/+=%_-".contains(c))
}

/// One `CreateSupportPermit` body, as a case in the tables below.
struct PermitCase {
    case: &'static str,
    name: String,
    permit: Value,
    description: Option<&'static str>,
    support_case_display_id: Option<&'static str>,
    tags: Value,
}

impl PermitCase {
    fn request(&self) -> Value {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(self.name));
        body.insert("permit".to_string(), self.permit.clone());
        body.insert("signingKeyInfo".to_string(), json!({ "kmsKey": KMS_KEY }));
        if let Some(description) = self.description {
            body.insert("description".to_string(), json!(description));
        }
        if let Some(display_id) = self.support_case_display_id {
            body.insert("supportCaseDisplayId".to_string(), json!(display_id));
        }
        body.insert("tags".to_string(), self.tags.clone());
        Value::Object(body)
    }

    fn expected_arn(&self) -> String {
        format!("arn:aws:supportauthz:{REGION}:{ACCOUNT}:support-permit/{}", self.name)
    }
}

/// The cases every lifecycle assertion runs over.
///
/// They differ along exactly what the modeled shapes generalise over: which `ActionSet`
/// / `ResourceSet` union member is set, the size of each list at its published bound,
/// how many `conditions` are present, and which optional members the request carries.
/// The bounds come from the model: `Actions` is `@length(1, 10)`, `Resources` is
/// `@length(1, 5)`, `Conditions` is `@length(max: 2)` and `Name` is
/// `@length(1, 256)` with pattern `^[a-zA-Z0-9]{1,256}$`.
fn permit_cases() -> Vec<PermitCase> {
    vec![
        PermitCase {
            case: "all actions on all resources in the region, nothing optional",
            name: "Broadest".to_string(),
            permit: json!({
                "actions": { "allActions": {} },
                "resources": { "allResourcesInRegion": {} }
            }),
            description: None,
            support_case_display_id: None,
            tags: json!({}),
        },
        PermitCase {
            case: "one action on one resource, every optional member present",
            name: "Narrowest".to_string(),
            permit: json!({
                "actions": { "actions": ["ec2:DescribeInstances"] },
                "resources": { "resources": ["i-0123456789abcdef0"] }
            }),
            description: Some("Read one instance for case 1234."),
            support_case_display_id: Some("case-1234"),
            tags: json!({ "team": "support" }),
        },
        PermitCase {
            case: "lists at their published maxima: 10 actions, 5 resources, 2 conditions",
            name: "AtTheBounds".to_string(),
            permit: json!({
                "actions": { "actions": [
                    "ec2:DescribeInstances",
                    "ec2:DescribeVolumes",
                    "ec2:DescribeSnapshots",
                    "ec2:DescribeImages",
                    "ec2:DescribeTags",
                    "s3:GetObject",
                    "s3:ListBucket",
                    "iam:GetRole",
                    "kms:DescribeKey",
                    "logs:GetLogEvents"
                ] },
                "resources": { "resources": [
                    "i-0123456789abcdef0",
                    "i-0123456789abcdef1",
                    "arn:aws:s3:::support-bucket",
                    "arn:aws:s3:::support-bucket/logs",
                    "vol-0123456789abcdef0"
                ] },
                "conditions": [
                    { "allowAfter": 1_800_000_000_i64 },
                    { "allowBefore": 1_800_003_600_i64 }
                ]
            }),
            description: Some("Everything at its modeled maximum."),
            support_case_display_id: Some("case-5678"),
            tags: json!({ "team": "support", "env": "test", "owner": "oncall" }),
        },
        PermitCase {
            case: "all actions but an explicit resource list, one allowBefore condition",
            name: "AllActionsSomeResources".to_string(),
            permit: json!({
                "actions": { "allActions": {} },
                "resources": { "resources": ["arn:aws:s3:::support-bucket"] },
                "conditions": [{ "allowBefore": 1_900_000_000_i64 }]
            }),
            description: None,
            support_case_display_id: Some("case-9012"),
            tags: json!({ "env": "prod" }),
        },
        PermitCase {
            case: "explicit actions but all resources in the region, one allowAfter condition",
            name: "SomeActionsAllResources".to_string(),
            permit: json!({
                "actions": { "actions": ["s3:GetObject", "s3:ListBucket"] },
                "resources": { "allResourcesInRegion": {} },
                "conditions": [{ "allowAfter": 1_700_000_000_i64 }]
            }),
            description: Some("Read-only S3 across the region."),
            support_case_display_id: None,
            tags: json!({}),
        },
        PermitCase {
            // `Name` is `@length(1, 256)`: this row sits exactly on the maximum.
            case: "a name at the published 256-character maximum",
            name: "N".repeat(256),
            permit: json!({
                "actions": { "actions": ["ec2:DescribeTags"] },
                "resources": { "resources": ["i-0123456789abcdef0"] }
            }),
            description: None,
            support_case_display_id: None,
            tags: json!({ "k": "" }),
        },
    ]
}

/// Compare one condition list leniently on number representation but strictly on
/// which member of the `Condition` union is set and what instant it carries.
fn assert_conditions_echoed(case: &str, expected: Option<&Vec<Value>>, actual: Option<&Vec<Value>>) {
    match (expected, actual) {
        (None, None) => {}
        (Some(expected), Some(actual)) => {
            assert_eq!(
                expected.len(),
                actual.len(),
                "case {case:?}: expected {} conditions, got {:?}",
                expected.len(),
                actual
            );
            for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
                let want_obj = want.as_object().expect("a condition is an object");
                let got_obj = got.as_object().expect("a condition is an object");
                let (want_key, want_value) = want_obj.iter().next().expect("one union member");
                assert_eq!(
                    got_obj.len(),
                    1,
                    "case {case:?}: condition {index} must carry exactly one union member, got {got:?}"
                );
                let (got_key, got_value) = got_obj.iter().next().expect("one union member");
                assert_eq!(
                    (want_key.as_str(), want_value.as_f64()),
                    (got_key.as_str(), got_value.as_f64()),
                    "case {case:?}: condition {index} expected {want:?}, got {got:?}"
                );
            }
        }
        (expected, actual) => panic!(
            "case {case:?}: expected conditions {expected:?}, got {actual:?}"
        ),
    }
}

/// A create answers from the STORE, and both readers answer the same record.
#[tokio::test]
async fn a_created_permit_is_minted_stored_and_read_back_by_arn_and_by_name() {
    for permit_case in permit_cases() {
        let router = new_router();
        let case = permit_case.case;
        let request = permit_case.request();

        let created = create(&router, &request).await;
        assert_eq!(
            (created.status, created.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected HTTP 200 from CreateSupportPermit, got {created:?}"
        );

        let arn = created.body["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            matches_arn_pattern(&arn),
            "case {case:?}: expected a minted arn matching the modeled Arn pattern, got {arn:?}"
        );
        assert_eq!(
            arn,
            permit_case.expected_arn(),
            "case {case:?}: expected the arn to name the permit in the signed region and the \
             emulator account, got {arn:?}"
        );
        assert_eq!(
            created.body["name"].as_str(),
            Some(permit_case.name.as_str()),
            "case {case:?}: expected the stored name echoed, got {:?}",
            created.body["name"]
        );
        assert_eq!(
            created.body["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: expected status ACTIVE on a freshly created permit, got {:?}",
            created.body["status"]
        );
        assert!(
            created.body["createdAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a real createdAt instant, got {:?}",
            created.body["createdAt"]
        );
        assert_eq!(
            created.body["signingKeyInfo"],
            json!({ "kmsKey": KMS_KEY }),
            "case {case:?}: expected the signingKeyInfo union round-tripped, got {:?}",
            created.body["signingKeyInfo"]
        );
        assert_eq!(
            created.body["permit"]["actions"], permit_case.permit["actions"],
            "case {case:?}: expected the ActionSet union round-tripped, got {:?}",
            created.body["permit"]["actions"]
        );
        assert_eq!(
            created.body["permit"]["resources"], permit_case.permit["resources"],
            "case {case:?}: expected the ResourceSet union round-tripped, got {:?}",
            created.body["permit"]["resources"]
        );
        assert_conditions_echoed(
            case,
            permit_case.permit["conditions"].as_array(),
            created.body["permit"]["conditions"].as_array(),
        );
        assert_eq!(
            created.body.get("description").and_then(Value::as_str),
            permit_case.description,
            "case {case:?}: expected description {:?}, got {:?}",
            permit_case.description,
            created.body.get("description")
        );
        assert_eq!(
            created
                .body
                .get("supportCaseDisplayId")
                .and_then(Value::as_str),
            permit_case.support_case_display_id,
            "case {case:?}: expected supportCaseDisplayId {:?}, got {:?}",
            permit_case.support_case_display_id,
            created.body.get("supportCaseDisplayId")
        );
        assert_eq!(
            created.body["tags"], permit_case.tags,
            "case {case:?}: expected the submitted tags stored, got {:?}",
            created.body["tags"]
        );

        // The round trip: both identifiers the modeled label pattern admits resolve to
        // the one stored record, and neither reader invents a member.
        for identifier in [arn.as_str(), permit_case.name.as_str()] {
            let read = get_permit(&router, identifier).await;
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (200, None),
                "case {case:?}: expected HTTP 200 from GetSupportPermit({identifier:?}), got {read:?}"
            );
            assert_eq!(
                read.body, created.body,
                "case {case:?}: expected GetSupportPermit({identifier:?}) to report exactly the \
                 created record, got {:?}",
                read.body
            );
        }

        // And the tags the create stored are the tags the tagging reader reports.
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {tags:?}"
        );
        assert_eq!(
            tags.body["tags"], permit_case.tags,
            "case {case:?}: expected ListTagsForResource to report the created tags, got {:?}",
            tags.body["tags"]
        );
    }
}

/// A second create under a live name is refused; the first permit is untouched.
#[tokio::test]
async fn creating_a_second_permit_under_a_live_name_is_a_conflict() {
    for permit_case in permit_cases() {
        let router = new_router();
        let case = permit_case.case;
        let request = permit_case.request();

        let first = create(&router, &request).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {first:?}"
        );

        // Same name, deliberately different body: the refusal is about the NAME.
        let mut second_request = request.clone();
        second_request["description"] = json!("A different description.");
        let second = create(&router, &second_request).await;
        assert_eq!(
            (second.status, second.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected HTTP 409 ConflictException for a duplicate name, got {second:?}"
        );

        let listed = list_permits(&router, "").await;
        assert_eq!(
            listed.body["supportPermits"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the refused create must not have stored a second permit, got {:?}",
            listed.body["supportPermits"]
        );
        let read = get_permit(&router, permit_case.name.as_str()).await;
        assert_eq!(
            read.body, first.body,
            "case {case:?}: the refused create must not have changed the stored permit, got {:?}",
            read.body
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: the same token with the same body replays
/// the original permit, the same token with any other body is a conflict.
#[tokio::test]
async fn a_client_token_replays_the_same_body_and_conflicts_on_a_different_one() {
    // (case, what differs on the retry, whether the retry replays)
    type Variation = (&'static str, fn(&mut Value), bool);
    let variations: [Variation; 5] = [
        ("byte-identical retry", |_body| {}, true),
        (
            "a different description",
            |body| body["description"] = json!("Changed."),
            false,
        ),
        (
            "a different action list",
            |body| body["permit"]["actions"] = json!({ "actions": ["s3:GetObject"] }),
            false,
        ),
        (
            "a different resource set member",
            |body| body["permit"]["resources"] = json!({ "allResourcesInRegion": {} }),
            false,
        ),
        (
            "one more tag",
            |body| body["tags"] = json!({ "team": "support", "extra": "1" }),
            false,
        ),
    ];

    for (case, mutate, replays) in variations {
        let router = new_router();
        let mut request = json!({
            "name": "Replayable",
            "permit": {
                "actions": { "actions": ["ec2:DescribeInstances"] },
                "resources": { "resources": ["i-0123456789abcdef0"] }
            },
            "signingKeyInfo": { "kmsKey": KMS_KEY },
            "description": "Original.",
            "clientToken": "token-0001",
            "tags": { "team": "support" }
        });

        let first = create(&router, &request).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {first:?}"
        );

        mutate(&mut request);
        let retry = create(&router, &request).await;

        if replays {
            assert_eq!(
                (retry.status, retry.error_type.as_deref()),
                (200, None),
                "case {case:?}: expected the replay to answer HTTP 200, got {retry:?}"
            );
            assert_eq!(
                retry.body, first.body,
                "case {case:?}: expected the replay to return the original record, got {:?}",
                retry.body
            );
        } else {
            assert_eq!(
                (retry.status, retry.error_type.as_deref()),
                (409, Some("ConflictException")),
                "case {case:?}: expected HTTP 409 ConflictException for a changed body under one \
                 clientToken, got {retry:?}"
            );
        }

        let listed = list_permits(&router, "").await;
        assert_eq!(
            listed.body["supportPermits"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: one clientToken must create at most one permit, got {:?}",
            listed.body["supportPermits"]
        );
    }
}

/// Every reader refuses an identifier no permit matches, with the modeled 404 rather
/// than a hollow 200.
#[tokio::test]
async fn an_unknown_identifier_is_refused_with_resource_not_found() {
    let router = new_router();
    let known = create(
        &router,
        &json!({
            "name": "Known",
            "permit": {
                "actions": { "allActions": {} },
                "resources": { "allResourcesInRegion": {} }
            },
            "signingKeyInfo": { "kmsKey": KMS_KEY }
        }),
    )
    .await;
    assert_eq!(known.status, 200, "the seed create must succeed, got {known:?}");

    let missing_arn =
        format!("arn:aws:supportauthz:{REGION}:{ACCOUNT}:support-permit/NoSuchPermit");
    let other_account = "arn:aws:supportauthz:eu-west-2:111111111111:support-permit/Known";

    // (case, identifier)
    let identifiers = [
        ("a name that was never created", "NoSuchPermit".to_string()),
        ("an arn that was never minted", missing_arn.clone()),
        ("an arn in another account", other_account.to_string()),
    ];

    for (case, identifier) in &identifiers {
        let read = get_permit(&router, identifier).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException from GetSupportPermit, \
             got {read:?}"
        );
    }

    // The ARN-typed identifiers: `DeleteSupportPermit` and the three tagging operations
    // all model their identifier as an `Arn`, so a bare name never reaches them.
    for (case, arn) in [
        ("an arn that was never minted", missing_arn.as_str()),
        ("an arn in another account", other_account),
    ] {
        let deleted = delete_permit(&router, arn).await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 from DeleteSupportPermit, got {deleted:?}"
        );

        let tagged = tag_resource(&router, arn, &json!({ "k": "v" })).await;
        assert_eq!(
            (tagged.status, tagged.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 from TagResource, got {tagged:?}"
        );

        let untagged = untag_resource(&router, arn, &["k"]).await;
        assert_eq!(
            (untagged.status, untagged.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 from UntagResource, got {untagged:?}"
        );

        let tags = list_tags(&router, arn).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 from ListTagsForResource, got {tags:?}"
        );
    }

    // And the one permit that does exist is still readable — no reader above
    // disturbed it.
    let read = get_permit(&router, "Known").await;
    assert_eq!(
        read.body, known.body,
        "a refused read must leave the stored permit untouched, got {:?}",
        read.body
    );
}

/// Seed `count` permits named `Permit01`.. and return their ARNs in the stable order
/// `ListSupportPermits` walks.
async fn seed_permits(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut arns = Vec::new();
    for index in 1..=count {
        let reply = create(
            router,
            &json!({
                "name": format!("Permit{index:02}"),
                "permit": {
                    "actions": { "actions": ["ec2:DescribeInstances"] },
                    "resources": { "resources": [format!("i-{index:017}")] }
                },
                "signingKeyInfo": { "kmsKey": KMS_KEY }
            }),
        )
        .await;
        assert_eq!(reply.status, 200, "seeding permit {index} must succeed, got {reply:?}");
        arns.push(reply.body["arn"].as_str().expect("a minted arn").to_string());
    }
    arns
}

/// Paging walks every stored permit exactly once, whatever the page size, and the last
/// page carries no token.
#[tokio::test]
async fn paging_walks_every_stored_permit_exactly_once() {
    const STORED: usize = 7;

    // `maxResults` is `@range(1, 100)`: both published bounds are rows here, with the
    // sizes in between and the absent-parameter case.
    let page_sizes: [Option<i32>; 6] = [None, Some(1), Some(2), Some(3), Some(7), Some(100)];

    for page_size in page_sizes {
        let router = new_router();
        let expected = seed_permits(&router, STORED).await;
        let case = format!("maxResults={page_size:?}");

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(format!("nextToken={}", encode_label(token)));
            }
            let reply = list_permits(&router, &query.join("&")).await;
            assert_eq!(
                (reply.status, reply.error_type.as_deref()),
                (200, None),
                "case {case:?}: expected HTTP 200 from ListSupportPermits, got {reply:?}"
            );
            let page = reply.body["supportPermits"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let limit = page_size.unwrap_or(100) as usize;
            assert!(
                page.len() <= limit,
                "case {case:?}: a page must not exceed maxResults, got {} entries",
                page.len()
            );
            for summary in &page {
                // `SupportPermitSummary` carries neither description nor tags.
                assert!(
                    summary.get("description").is_none() && summary.get("tags").is_none(),
                    "case {case:?}: a summary must carry no description or tags, got {summary:?}"
                );
                assert_eq!(
                    summary["status"].as_str(),
                    Some("ACTIVE"),
                    "case {case:?}: expected a stored ACTIVE permit, got {summary:?}"
                );
                seen.push(summary["arn"].as_str().expect("an arn").to_string());
            }
            pages += 1;
            match reply.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        matches_next_token_pattern(next),
                        "case {case:?}: expected an opaque nextToken matching the modeled \
                         pattern, got {next:?}"
                    );
                    assert!(
                        !page.is_empty(),
                        "case {case:?}: a token must only follow a non-empty page"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= STORED + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every stored permit exactly once in stable order, got {seen:?}"
        );
        let limit = page_size.unwrap_or(100) as usize;
        assert_eq!(
            pages,
            STORED.div_ceil(limit).max(1),
            "case {case:?}: expected {} page(s) for {STORED} permits, got {pages}",
            STORED.div_ceil(limit).max(1)
        );
    }
}

/// An empty store answers with an empty list and no token — not an absent member and
/// not a token that resumes nothing.
#[tokio::test]
async fn an_empty_store_lists_no_permits_and_no_token() {
    let router = new_router();
    let reply = list_permits(&router, "").await;
    assert_eq!(
        (reply.status, reply.error_type.as_deref()),
        (200, None),
        "expected HTTP 200 from ListSupportPermits on an empty store, got {reply:?}"
    );
    assert_eq!(
        reply.body["supportPermits"],
        json!([]),
        "expected an empty supportPermits list, got {:?}",
        reply.body["supportPermits"]
    );
    assert!(
        reply.body.get("nextToken").is_none(),
        "expected no nextToken on an empty store, got {:?}",
        reply.body.get("nextToken")
    );
}

/// Pagination tokens are resolved, never parsed: a token this service did not mint
/// names no page.
#[tokio::test]
async fn a_token_the_service_did_not_mint_is_refused() {
    let router = new_router();
    seed_permits(&router, 3).await;

    // (case, token)
    let tokens = [
        ("an opaque-looking token nobody minted", "deadbeef"),
        ("a base64-looking token nobody minted", "cGVybWl0Lzk%3D"),
        ("a plausible offset", "1"),
    ];

    for (case, token) in tokens {
        let reply = list_permits(&router, &format!("maxResults=1&nextToken={token}")).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected HTTP 400 ValidationException for an unminted nextToken, \
             got {reply:?}"
        );
    }
}

/// `supportPermitStatuses` filters on the stored status, and only the modeled value set
/// is accepted.
#[tokio::test]
async fn listing_filters_on_the_modeled_status_value_set() {
    let router = new_router();
    let stored = seed_permits(&router, 3).await;

    // Put one permit in the transient `DELETING` state so the filter is exercised
    // against a store that really holds more than one status. Every assertion below
    // runs inside `DELETION_SPAN`, before the clock removes it.
    let deleting = stored[1].clone();
    let accepted = delete_permit(&router, &deleting).await;
    assert_eq!(
        (accepted.status, accepted.body["status"].as_str()),
        (200, Some("DELETING")),
        "the seeding delete must report DELETING, got {accepted:?}"
    );
    let active: Vec<String> = stored
        .iter()
        .filter(|arn| **arn != deleting)
        .cloned()
        .collect();

    // Every value `SupportPermitStatus` declares (source:
    // `smithy_query(query="shape", shape="SupportPermitStatus")`), alone and paired,
    // plus one outside the set below. `INACTIVE` matches nothing: no operation in this
    // slice puts a permit in it.
    let rows: [(&str, &[&str], Vec<String>); 6] = [
        ("ACTIVE matches the permits not being deleted", &["ACTIVE"], active.clone()),
        ("DELETING matches the one being deleted", &["DELETING"], vec![deleting.clone()]),
        ("INACTIVE matches none", &["INACTIVE"], Vec::new()),
        ("ACTIVE or DELETING matches every stored permit", &["ACTIVE", "DELETING"], stored.clone()),
        ("ACTIVE or INACTIVE matches the active permits", &["ACTIVE", "INACTIVE"], active.clone()),
        ("DELETING or INACTIVE matches the one being deleted", &["DELETING", "INACTIVE"], vec![deleting.clone()]),
    ];

    for (case, statuses, expected) in rows {
        let query = statuses
            .iter()
            .map(|status| format!("supportPermitStatuses={status}"))
            .collect::<Vec<_>>()
            .join("&");
        let reply = list_permits(&router, &query).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected HTTP 200 from a filtered ListSupportPermits, got {reply:?}"
        );
        assert_eq!(
            page_arns(&reply),
            expected,
            "case {case:?}: expected {expected:?} in stable order, got {:?}",
            reply.body["supportPermits"]
        );
    }

    // One value past the modeled set is refused rather than silently matching nothing.
    let reply = list_permits(&router, "supportPermitStatuses=PENDING").await;
    assert_eq!(
        (reply.status, reply.error_type.as_deref()),
        (400, Some("ValidationException")),
        "expected HTTP 400 ValidationException for a status outside the modeled value set, \
         got {reply:?}"
    );
}

/// Tagging merges, untagging removes the named keys and nothing else, and both are
/// visible to every reader of the permit.
#[tokio::test]
async fn tagging_merges_and_untagging_removes_only_the_named_keys() {
    // (case, tags created with, tags merged, keys removed, expected tags)
    let rows: [(&str, Value, Value, &[&str], Value); 5] = [
        (
            "a merge onto an untagged permit",
            json!({}),
            json!({ "env": "test" }),
            &[],
            json!({ "env": "test" }),
        ),
        (
            "a merge that adds a key",
            json!({ "team": "support" }),
            json!({ "env": "test" }),
            &[],
            json!({ "team": "support", "env": "test" }),
        ),
        (
            "a merge that overwrites one key and leaves the other",
            json!({ "team": "support", "env": "prod" }),
            json!({ "env": "test" }),
            &[],
            json!({ "team": "support", "env": "test" }),
        ),
        (
            "an untag of one of two keys",
            json!({ "team": "support", "env": "prod" }),
            json!({}),
            &["env"],
            json!({ "team": "support" }),
        ),
        (
            "an untag naming a key that was never stored",
            json!({ "team": "support" }),
            json!({ "env": "test" }),
            &["absent", "env"],
            json!({ "team": "support" }),
        ),
    ];

    for (case, created_tags, merged_tags, removed_keys, expected) in rows {
        let router = new_router();
        let created = create(
            &router,
            &json!({
                "name": "Tagged",
                "permit": {
                    "actions": { "allActions": {} },
                    "resources": { "allResourcesInRegion": {} }
                },
                "signingKeyInfo": { "kmsKey": KMS_KEY },
                "tags": created_tags
            }),
        )
        .await;
        assert_eq!(created.status, 200, "case {case:?}: the create must succeed, got {created:?}");
        let arn = created.body["arn"].as_str().expect("a minted arn").to_string();

        if merged_tags.as_object().is_some_and(|map| !map.is_empty()) {
            let tagged = tag_resource(&router, &arn, &merged_tags).await;
            assert_eq!(
                (tagged.status, tagged.error_type.as_deref()),
                (204, None),
                "case {case:?}: expected HTTP 204 from TagResource, got {tagged:?}"
            );
        }
        if !removed_keys.is_empty() {
            let untagged = untag_resource(&router, &arn, removed_keys).await;
            assert_eq!(
                (untagged.status, untagged.error_type.as_deref()),
                (204, None),
                "case {case:?}: expected HTTP 204 from UntagResource, got {untagged:?}"
            );
        }

        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags.body["tags"], expected,
            "case {case:?}: expected ListTagsForResource to report {expected:?}, got {:?}",
            tags.body["tags"]
        );
        let read = get_permit(&router, &arn).await;
        assert_eq!(
            read.body["tags"], expected,
            "case {case:?}: expected GetSupportPermit to report the same tags, got {:?}",
            read.body["tags"]
        );
    }
}

/// The ARNs `ListSupportPermits` reports for one request, in the order it reported them.
fn page_arns(reply: &Reply) -> Vec<String> {
    reply.body["supportPermits"]
        .as_array()
        .map(|page| {
            page.iter()
                .filter_map(|summary| summary["arn"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Page through the whole store at the modeled maximum page size.
async fn collect_all_arns(router: &MultiServiceRouter) -> Vec<String> {
    let mut arns = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = vec!["maxResults=100".to_string()];
        if let Some(token) = token.as_deref() {
            query.push(format!("nextToken={}", encode_label(token)));
        }
        let reply = list_permits(router, &query.join("&")).await;
        assert_eq!(reply.status, 200, "listing the whole store must succeed, got {reply:?}");
        arns.extend(page_arns(&reply));
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    arns
}

/// A delete is observable by the next reader: it reports the `DELETING` status its
/// modeled `status` member documents, the permit stays readable in that transient state
/// for the span the clock owes, and then it leaves the store.
#[tokio::test]
async fn a_deleted_permit_reports_deleting_and_then_leaves_the_store() {
    const STORED: usize = 3;

    // Delete each stored position in turn — first, middle, last — so the assertion is
    // about removal, not about one convenient index.
    for position in 0..STORED {
        let router = new_router();
        let arns = seed_permits(&router, STORED).await;
        let case = format!("deleting position {position} of {STORED}");
        let arn = arns[position].as_str();
        let created = get_permit(&router, arn).await;

        let deleted = delete_permit(&router, arn).await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected HTTP 200 from DeleteSupportPermit, got {deleted:?}"
        );
        assert_eq!(
            deleted.body["arn"].as_str(),
            Some(arn),
            "case {case:?}: expected the record of the deleted permit, got {:?}",
            deleted.body
        );
        // `DeleteSupportPermitOutput.status` documents: "Returns DELETING."
        assert_eq!(
            deleted.body["status"].as_str(),
            Some("DELETING"),
            "case {case:?}: expected DeleteSupportPermit to report DELETING, got {:?}",
            deleted.body["status"]
        );
        assert_eq!(
            deleted.body["createdAt"], created.body["createdAt"],
            "case {case:?}: the record must keep the permit's own createdAt, got {:?}",
            deleted.body["createdAt"]
        );
        // The delete output shape declares no `tags` member.
        assert!(
            deleted.body.get("tags").is_none(),
            "case {case:?}: DeleteSupportPermit declares no tags member, got {:?}",
            deleted.body.get("tags")
        );

        // `@idempotent`: a second delete inside the span answers the same record and
        // does not push the removal out.
        let again = delete_permit(&router, arn).await;
        assert_eq!(
            (again.status, again.body["status"].as_str()),
            (200, Some("DELETING")),
            "case {case:?}: expected a repeated delete to answer the same record, got {again:?}"
        );

        // The transient state is observable: a caller acting straight after the delete
        // still reads the permit, in DELETING, and sees it under that status filter.
        let read = get_permit(&router, arn).await;
        assert_eq!(
            (read.status, read.body["status"].as_str()),
            (200, Some("DELETING")),
            "case {case:?}: expected the permit still readable in DELETING, got {read:?}"
        );
        let mut expected_record = created.body.clone();
        expected_record["status"] = json!("DELETING");
        assert_eq!(
            read.body, expected_record,
            "case {case:?}: the delete must change only the status, got {:?}",
            read.body
        );
        let deleting = list_permits(&router, "supportPermitStatuses=DELETING").await;
        assert_eq!(
            page_arns(&deleting),
            vec![arn.to_string()],
            "case {case:?}: expected the deleting permit under the DELETING filter, got {:?}",
            deleting.body["supportPermits"]
        );

        // The removal is a transition the CLOCK owes; nothing sleeps in the handler, so
        // the test waits the span out itself.
        std::thread::sleep(DELETION_SPAN + Duration::from_millis(100));

        let read = get_permit(&router, arn).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 once the removal fell due, got {read:?}"
        );

        let remaining = page_arns(&list_permits(&router, "").await);
        let expected: Vec<String> = arns
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != position)
            .map(|(_, a)| a.clone())
            .collect();
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the removed permit gone and the rest untouched, \
             got {remaining:?}"
        );

        // Deleting again finds nothing: the permit really left the store.
        let again = delete_permit(&router, arn).await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected HTTP 404 from a delete after the removal, got {again:?}"
        );

        // And the name is free again, which is what makes the delete a real removal.
        let recreated = create(
            &router,
            &json!({
                "name": format!("Permit{:02}", position + 1),
                "permit": {
                    "actions": { "allActions": {} },
                    "resources": { "allResourcesInRegion": {} }
                },
                "signingKeyInfo": { "kmsKey": KMS_KEY }
            }),
        )
        .await;
        assert_eq!(
            (recreated.status, recreated.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected the freed name to be creatable again, got {recreated:?}"
        );
        assert_eq!(
            recreated.body["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the recreated permit must start ACTIVE, got {:?}",
            recreated.body["status"]
        );
    }
}

/// The store holds a bounded number of permits, and the bound is a live count, not a
/// high-water mark: the operation's own modeled quota error is the answer past it.
#[tokio::test]
async fn the_permit_store_is_capped_and_answers_the_modeled_quota_error() {
    let router = new_router();

    let permit = json!({
        "actions": { "allActions": {} },
        "resources": { "allResourcesInRegion": {} }
    });
    let body = |name: String| {
        json!({
            "name": name,
            "permit": permit.clone(),
            "signingKeyInfo": { "kmsKey": KMS_KEY }
        })
    };

    for index in 0..MAX_SUPPORT_PERMITS {
        let reply = create(&router, &body(format!("Capped{index:04}"))).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (200, None),
            "create {index} of the {MAX_SUPPORT_PERMITS} the cap allows must succeed, got {reply:?}"
        );
    }
    assert_eq!(
        collect_all_arns(&router).await.len(),
        MAX_SUPPORT_PERMITS,
        "the store must hold exactly the capped number of permits"
    );

    let over = create(&router, &body("OnePastTheCap".to_string())).await;
    assert_eq!(
        (over.status, over.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "expected HTTP 402 ServiceQuotaExceededException one past the cap, got {over:?}"
    );
    let read = get_permit(&router, "OnePastTheCap").await;
    assert_eq!(
        (read.status, read.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "the refused create must not have stored anything, got {read:?}"
    );
    assert_eq!(
        collect_all_arns(&router).await.len(),
        MAX_SUPPORT_PERMITS,
        "the refused create must not have grown the store"
    );

    // Freeing one slot makes room again — the cap counts what is stored now.
    let doomed = format!("arn:aws:supportauthz:{REGION}:{ACCOUNT}:support-permit/Capped0000");
    let deleted = delete_permit(&router, &doomed).await;
    assert_eq!(deleted.status, 200, "the delete must succeed, got {deleted:?}");
    std::thread::sleep(DELETION_SPAN + Duration::from_millis(100));

    let after = create(&router, &body("OnePastTheCap".to_string())).await;
    assert_eq!(
        (after.status, after.error_type.as_deref()),
        (200, None),
        "expected a create to succeed once a slot was freed, got {after:?}"
    );
}

/// One permit's tag set is capped at the model's own `Tags` bound: a merge past it
/// writes nothing, and an overwrite at the cap still works.
#[tokio::test]
async fn one_permits_tag_set_is_capped_at_the_modeled_tags_bound() {
    // `Tags` is `@length(0, 50)` (source: `smithy_query(query="shape", shape="Tags")`),
    // which is both the per-request bound the boundary validator enforces and the
    // per-permit total this service enforces.
    const CAP: usize = 50;

    // (case, how many tags the create stores, the keys TagResource merges, outcome)
    let rows: [(&str, usize, Vec<String>, u16); 5] = [
        ("a merge that lands exactly on the cap", CAP - 2, vec!["new0".into(), "new1".into()], 204),
        ("a merge one key past the cap", CAP, vec!["new0".into()], 400),
        ("a merge many keys past the cap", CAP - 1, vec!["new0".into(), "new1".into()], 400),
        // An overwrite adds no key, so it is allowed even with the permit full.
        ("an overwrite of a stored key at the cap", CAP, vec!["tag00".into()], 204),
        ("a merge well inside the cap", 1, vec!["new0".into()], 204),
    ];

    for (case, created, merged_keys, expected_status) in rows {
        let router = new_router();
        let created_tags: Map<String, Value> = (0..created)
            .map(|index| (format!("tag{index:02}"), json!("original")))
            .collect();
        let create_reply = create(
            &router,
            &json!({
                "name": "Capped",
                "permit": {
                    "actions": { "allActions": {} },
                    "resources": { "allResourcesInRegion": {} }
                },
                "signingKeyInfo": { "kmsKey": KMS_KEY },
                "tags": Value::Object(created_tags.clone())
            }),
        )
        .await;
        assert_eq!(
            create_reply.status, 200,
            "case {case:?}: the create must succeed, got {create_reply:?}"
        );
        let arn = create_reply.body["arn"].as_str().expect("a minted arn").to_string();

        let merged: Map<String, Value> = merged_keys
            .iter()
            .map(|key| (key.clone(), json!("merged")))
            .collect();
        let reply = tag_resource(&router, &arn, &Value::Object(merged.clone())).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from TagResource, got {reply:?}"
        );

        let stored = list_tags(&router, &arn).await;
        let stored = stored.body["tags"].as_object().cloned().unwrap_or_default();
        if expected_status == 204 {
            let mut expected = created_tags.clone();
            expected.extend(merged.clone());
            assert_eq!(
                Value::Object(stored),
                Value::Object(expected),
                "case {case:?}: expected the merge applied"
            );
        } else {
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a modeled ValidationException, got {reply:?}"
            );
            // All-or-nothing: a refused merge writes NOTHING, not a partial set.
            assert_eq!(
                Value::Object(stored),
                Value::Object(created_tags.clone()),
                "case {case:?}: a refused merge must leave the stored tags untouched"
            );
        }
    }
}

/// The operations this slice does not implement still answer the scaffold's 501 — never
/// an empty 200.
#[tokio::test]
async fn the_operations_this_slice_leaves_unimplemented_still_answer_501() {
    let router = new_router();

    // (case, method, uri)
    let rows = [
        ("GetAction", "GET", "/actions/ec2:DescribeInstances"),
        ("ListActions", "GET", "/actions?service=ec2"),
        (
            "RejectSupportPermitRequest",
            "PUT",
            "/support-permit-requests/request-1/reject",
        ),
    ];

    for (case, method, uri) in rows {
        let reply = dispatch(&router, method, uri, None).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (501, Some("NotImplemented")),
            "case {case:?}: expected the scaffold's HTTP 501 NotImplemented, got {reply:?}"
        );
    }
}

/// Every permit operation is SERVED, from shapes this crate declares itself.
///
/// The four operations below are the ones the op-stub generator declines, so nothing in
/// the generated surface carries their shapes: each is hand-authored, and so are the
/// `Permit` shapes they exchange. This is what fails first if those declarations ever go
/// out of scope again — the operations stop being bound and answer the framework's
/// "modeled but not served" refusal instead of a permit.
#[tokio::test]
async fn every_permit_operation_is_served_rather_than_declined() {
    let router = new_router();
    let created = create(
        &router,
        &json!({
            "name": "Served",
            "permit": {
                "actions": { "allActions": {} },
                "resources": { "allResourcesInRegion": {} }
            },
            "signingKeyInfo": { "kmsKey": KMS_KEY }
        }),
    )
    .await;
    let arn = created.body["arn"].as_str().unwrap_or_default().to_string();

    // (case, the reply)
    let replies = [
        ("CreateSupportPermit", created),
        ("GetSupportPermit", get_permit(&router, "Served").await),
        ("ListSupportPermits", list_permits(&router, "").await),
        ("DeleteSupportPermit", delete_permit(&router, &arn).await),
    ];

    for (case, reply) in replies {
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the operation to be served with HTTP 200, got {reply:?}"
        );
        assert_ne!(
            reply.error_type.as_deref(),
            Some("NotImplemented"),
            "case {case:?}: the operation must not answer the scaffold's 501, got {reply:?}"
        );
    }
}

/// A `Permit` union carries EXACTLY one member on the wire.
///
/// Both unions of a permit, the `Condition` entries under it and `signingKeyInfo` are
/// declared in this crate's own source (the generator declines the operations that reach
/// them), so the one-member rule is this service's to enforce. A body that sets none or
/// two is refused, and nothing is stored for it.
#[tokio::test]
async fn a_permit_union_on_the_wire_must_carry_exactly_one_member() {
    // (case, the member to replace under `permit`/`signingKeyInfo`, the replacement,
    //  the expected status)
    let rows: [(&str, &str, Value, u16); 11] = [
        // The control row: one member set in every union.
        ("one member in every union", "actions", json!({ "allActions": {} }), 200),
        ("the other actions arm", "actions", json!({ "actions": ["s3:GetObject"] }), 200),
        ("no member in actions", "actions", json!({}), 400),
        (
            "two members in actions",
            "actions",
            json!({ "allActions": {}, "actions": ["s3:GetObject"] }),
            400,
        ),
        ("no member in resources", "resources", json!({}), 400),
        (
            "two members in resources",
            "resources",
            json!({ "allResourcesInRegion": {}, "resources": ["arn:aws:s3:::b"] }),
            400,
        ),
        ("the other resources arm", "resources", json!({ "resources": ["arn:aws:s3:::b"] }), 200),
        ("no member in a condition", "conditions", json!([{}]), 400),
        (
            "two members in a condition",
            "conditions",
            json!([{ "allowAfter": 1_700_000_000_i64, "allowBefore": 1_900_000_000_i64 }]),
            400,
        ),
        (
            "one member in a condition",
            "conditions",
            json!([{ "allowAfter": 1_700_000_000_i64 }]),
            200,
        ),
        ("no member in signingKeyInfo", "signingKeyInfo", json!({}), 400),
    ];

    for (case, member, replacement, expected_status) in rows {
        let router = new_router();
        let mut body = json!({
            "name": "UnionArity",
            "permit": {
                "actions": { "allActions": {} },
                "resources": { "allResourcesInRegion": {} }
            },
            "signingKeyInfo": { "kmsKey": KMS_KEY }
        });
        if member == "signingKeyInfo" {
            body["signingKeyInfo"] = replacement.clone();
        } else {
            body["permit"][member] = replacement.clone();
        }

        let reply = create(&router, &body).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {member} = {replacement}, \
             got {reply:?}"
        );

        // A refused body stores nothing: the permit must not be readable afterwards.
        let read = get_permit(&router, "UnionArity").await;
        let expected_read = if expected_status == 200 { 200 } else { 404 };
        assert_eq!(
            read.status, expected_read,
            "case {case:?}: expected HTTP {expected_read} reading the permit back, got {read:?}"
        );
    }
}

/// The overlay model restates the snapshot EXACTLY, and adds only the prelude shape the
/// snapshot omits.
///
/// `smithy-model/local-additions.json` exists for one reason: the Smithy JSON AST does
/// not serialize `smithy.api#Unit`, which both `Permit` unions target, so codegen cannot
/// classify either union from the snapshot alone (see `smithy-model/LOCAL-ADDITIONS.md`).
/// Every OTHER shape in the overlay is a restatement of the snapshot's own definition, so
/// a refreshed snapshot that changes a member, a target or a constraint must fail here
/// rather than leave the two definitions silently diverged.
#[test]
fn the_local_overlay_restates_the_upstream_model_and_adds_only_the_prelude_unit_shape() {
    let shapes_of = |source: &str, label: &str| -> Map<String, Value> {
        serde_json::from_str::<Value>(source)
            .unwrap_or_else(|error| panic!("{label} must be JSON: {error}"))
            .get("shapes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(|| panic!("{label} must carry a shapes object"))
    };

    let snapshot = shapes_of(
        include_str!("../../smithy-model/com-amazonaws-supportauthz-supportauthz.json"),
        "the model snapshot",
    );
    let overlay = shapes_of(
        include_str!("../../smithy-model/local-additions.json"),
        "the overlay model",
    );

    // The one shape the overlay ADDS: the prelude structure with no members. It is in the
    // overlay precisely because the snapshot does not serialize it.
    assert_eq!(
        overlay.get("smithy.api#Unit"),
        Some(&json!({ "type": "structure", "members": {} })),
        "the overlay must declare smithy.api#Unit as the structure with no members, got {:?}",
        overlay.get("smithy.api#Unit")
    );
    assert!(
        !snapshot.contains_key("smithy.api#Unit"),
        "the snapshot now declares smithy.api#Unit, so the overlay should give way to it"
    );

    // Every other shape is a verbatim restatement of the snapshot's definition.
    let restated: Vec<&String> = overlay
        .keys()
        .filter(|id| id.as_str() != "smithy.api#Unit")
        .collect();
    assert!(
        restated.len() >= 6,
        "expected the overlay to restate the two unions and the shapes their members \
         target, got {restated:?}"
    );
    for id in restated {
        assert_eq!(
            overlay.get(id),
            snapshot.get(id),
            "case {id:?}: the overlay must restate the snapshot's definition exactly; \
             overlay {:?}, snapshot {:?}",
            overlay.get(id),
            snapshot.get(id)
        );
    }

    // And the two unions really are the shapes that need it: each has a member targeting
    // the prelude shape.
    for id in [
        "com.amazonaws.supportauthz#ActionSet",
        "com.amazonaws.supportauthz#ResourceSet",
    ] {
        let members = overlay
            .get(id)
            .and_then(|shape| shape.get("members"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {id:?}: the overlay must declare this union"));
        assert!(
            members
                .values()
                .any(|member| member.get("target") == Some(&json!("smithy.api#Unit"))),
            "case {id:?}: expected a member targeting smithy.api#Unit, got {members:?}"
        );
    }
}
