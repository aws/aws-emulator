// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that `TagResource` writes onto the ONE resource its `resourceArn`
//! names, and onto no other.
//!
//! The rule under test: a tag write is scoped to the resource the ARN resolves to and
//! MERGES into the tag set that resource already carries — adding keys it lacks and
//! replacing the value of keys it has — so every other resource this service holds keeps
//! the tag set it had, whatever keys the two sets share and whichever resource the ARN
//! shares a parent, a kind or a target with.
//!
//! That is what tagging one resource at a time and reading it straight back does not
//! pin: a handler writing into a single map shared by the whole service, or replacing a
//! resource's set instead of merging into it, would pass such a check and fail here.
//!
//! Every assertion drives the real `restJson1` front door: requests are built the way an
//! SDK builds them, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct.
//!
//! The rows differ along what the rule generalises over — the resource KIND tagged
//! (a component version, a deployment), whether a row shares its component with another
//! row, whether the resource carried tags from its create call, whether the write adds
//! keys or updates ones already stored, whether two resources share a tag KEY or a whole
//! key/value pair, and whether a resource is named by any write at all — not along the
//! resource names, which the rule says nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The status a successful tag write answers: `TagResourceResponse` declares no member,
/// and this surface answers such a success `204 No Content`.
const TAG_WRITE_STATUS: u16 = 204;

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and return its status and parsed JSON body.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Vec<u8>) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("a serializable request body");
    call(router, "POST", path, bytes).await
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The component-version ARN this emulator mints for `name`@`version`.
fn version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:{name}:versions:{version}")
}

/// The deployment ARN of `deployment_id`.
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// A tag map as a JSON object.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

/// The smallest recipe the parser accepts for `name`@`version`: the three keys the IoT
/// Greengrass component recipe reference requires.
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// `TagResource` on `arn` (`POST /tags/{resourceArn}`).
async fn tag(router: &MultiServiceRouter, arn: &str, tags: &[(&str, &str)]) -> (u16, Value) {
    post(
        router,
        &format!("/tags/{}", encoded_label(arn)),
        &json!({ "tags": tag_object(tags) }),
    )
    .await
}

/// `ListTagsForResource` on `arn` (`GET /tags/{resourceArn}`).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encoded_label(arn)), Vec::new()).await
}

/// Which resource a row tags, and how it overlaps the other rows.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// A version of a component. Two rows naming the same `component` with different
    /// `version`s share a parent, so a write to one must not reach the other.
    Version {
        component: &'static str,
        version: &'static str,
    },
    /// A deployment created for the named IoT thing.
    Deployment { target: &'static str },
}

/// One row: the resource, the tags its CREATE carried, the tags one `TagResource` write
/// carries (empty for a row no write names), and the set the resource must hold once
/// every row's write has been made.
#[derive(Debug)]
struct ScopeCase {
    label: &'static str,
    resource: Resource,
    at_create: &'static [(&'static str, &'static str)],
    write: &'static [(&'static str, &'static str)],
    expected: &'static [(&'static str, &'static str)],
}

/// `TagKey` is `length 1..128` and `TagValue` `length 0..256`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`), so the empty value is one a
/// write may carry; `TagMap` is `length 1..200`, so a row that names no write sends no
/// request at all rather than an empty map.
const CASES: &[ScopeCase] = &[
    ScopeCase {
        label: "a version whose sibling version is tagged too, created with tags",
        resource: Resource::Version {
            component: "com.example.Scoped",
            version: "1.0.0",
        },
        at_create: &[("stage", "one")],
        write: &[("owner", "alpha")],
        expected: &[("owner", "alpha"), ("stage", "one")],
    },
    ScopeCase {
        label: "the sibling version of that same component, created untagged",
        resource: Resource::Version {
            component: "com.example.Scoped",
            version: "2.0.0",
        },
        at_create: &[],
        write: &[("owner", "beta")],
        expected: &[("owner", "beta")],
    },
    ScopeCase {
        label: "a version of another component, whose write updates a create-time key",
        resource: Resource::Version {
            component: "com.example.Other",
            version: "1.0.0",
        },
        at_create: &[("stage", "one"), ("keep", "me")],
        write: &[("stage", "two")],
        expected: &[("keep", "me"), ("stage", "two")],
    },
    ScopeCase {
        label: "a deployment whose write updates one key, adds an empty-valued one",
        resource: Resource::Deployment {
            target: "ScopedCoreA",
        },
        at_create: &[("owner", "alpha"), ("wave", "1")],
        write: &[("wave", "2"), ("extra", "")],
        expected: &[("extra", ""), ("owner", "alpha"), ("wave", "2")],
    },
    ScopeCase {
        label: "a deployment to another target, carrying a pair another row also writes",
        resource: Resource::Deployment {
            target: "ScopedCoreB",
        },
        at_create: &[],
        write: &[("owner", "alpha")],
        expected: &[("owner", "alpha")],
    },
    ScopeCase {
        label: "a version no write names, which keeps the tags its create carried",
        resource: Resource::Version {
            component: "com.example.Untouched",
            version: "1.0.0",
        },
        at_create: &[("stage", "one"), ("owner", "alpha")],
        write: &[],
        expected: &[("owner", "alpha"), ("stage", "one")],
    },
];

/// Create `case`'s resource with the tags its create carries, and answer the ARN its
/// tags are written by plus the path of the resource's OWN reader.
async fn create_resource(router: &MultiServiceRouter, case: &ScopeCase) -> (String, String) {
    let mut body = Map::new();
    if !case.at_create.is_empty() {
        body.insert("tags".to_string(), tag_object(case.at_create));
    }
    match case.resource {
        Resource::Version { component, version } => {
            body.insert(
                "inlineRecipe".to_string(),
                Value::String(base64::encode(recipe(component, version).as_bytes())),
            );
            let (status, created) = post(
                router,
                "/greengrass/v2/createComponentVersion",
                &Value::Object(body),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: publishing {component}:{version} must answer 201, got {status} {created}",
                case.label
            );
            let arn = version_arn(component, version);
            let reader = format!("/greengrass/v2/components/{}/metadata", encoded_label(&arn));
            (arn, reader)
        }
        Resource::Deployment { target } => {
            body.insert(
                "targetArn".to_string(),
                Value::String(thing_target(target)),
            );
            let (status, created) =
                post(router, "/greengrass/v2/deployments", &Value::Object(body)).await;
            assert_eq!(
                status, 201,
                "case {:?}: CreateDeployment must answer 201, got {status} {created}",
                case.label
            );
            let id = text(&created, "deploymentId");
            assert!(
                !id.is_empty(),
                "case {:?}: CreateDeployment must answer a deployment id, got {created}",
                case.label
            );
            (deployment_arn(&id), format!("/greengrass/v2/deployments/{id}"))
        }
    }
}

/// Assert every row reports exactly its own expected tag set, through the tag listing and
/// through the resource's own reader.
async fn assert_every_row_holds_its_own_tags(
    router: &MultiServiceRouter,
    rows: &[(&ScopeCase, String, String)],
    pass: &str,
) {
    for (case, arn, reader) in rows {
        let expected = tag_object(case.expected);

        let (status, listed) = list_tags(router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?} ({pass}): the tag listing must answer 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"], expected,
            "case {:?} ({pass}): expected {expected}, got {listed}",
            case.label
        );

        let (status, stored) = call(router, "GET", reader, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {:?} ({pass}): the resource's own reader must answer 200, got {status} {stored}",
            case.label
        );
        assert_eq!(
            stored["tags"], expected,
            "case {:?} ({pass}): the resource must report {expected}, got {stored}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_tag_write_lands_on_the_named_resource_and_on_no_other() {
    let router = emulator();

    // Every resource exists BEFORE any write, so each write is made with all the others
    // in the store and a write that leaked would be visible on them.
    let mut rows = Vec::new();
    for case in CASES {
        let (arn, reader) = create_resource(&router, case).await;
        rows.push((case, arn, reader));
    }

    for (case, arn, _) in &rows {
        if case.write.is_empty() {
            continue;
        }
        let (status, body) = tag(&router, arn, case.write).await;
        assert_eq!(
            status, TAG_WRITE_STATUS,
            "case {:?}: the write must answer {TAG_WRITE_STATUS}, got {status} {body}",
            case.label
        );
        // The success carries no content: the modelled output declares no member.
        assert!(
            body.is_null() || body.as_object().is_none_or(Map::is_empty),
            "case {:?}: the write must answer no content, got {body}",
            case.label
        );
    }

    assert_every_row_holds_its_own_tags(&router, &rows, "after one write each").await;

    // Writing the same set a second time changes nothing — on the resource named or on
    // any other — because a write merges a key/value pair in rather than appending.
    for (case, arn, _) in &rows {
        if case.write.is_empty() {
            continue;
        }
        let (status, body) = tag(&router, arn, case.write).await;
        assert_eq!(
            status, TAG_WRITE_STATUS,
            "case {:?}: the repeated write must answer {TAG_WRITE_STATUS}, got {status} {body}",
            case.label
        );
    }

    assert_every_row_holds_its_own_tags(&router, &rows, "after the writes were repeated").await;
}
