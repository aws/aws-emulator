// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WRITING a Greengrass V2 resource's tags.
//!
//! The rules under test:
//!
//! * `TagResource` writes the tags it carries onto the resource its `resourceArn` names —
//!   adding keys the resource does not carry and updating the value of keys it does — so
//!   the tag listing and the resource's own reader report what was written; and
//! * a `resourceArn` inside the modelled `GenericV2ARN` pattern that names no resource
//!   this service holds is refused `404` with the tagging route's OWN error code,
//!   `NotFoundException`, and one fixed sentence that reports no value of the request —
//!   the code and sentence a client of the tagging route sees, which differ from the
//!   `ResourceNotFoundException` the id-keyed component and deployment operations carry.
//!   The rule is the ROUTE's, so the write and the read of a resource's tags refuse it
//!   identically.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct, so a tag write that
//! was discarded instead of persisted fails here.
//!
//! The rows differ along what the rules generalise over — the resource KIND tagged, how
//! many tags a write carries and whether its keys are new or already present, the FORM of
//! a key and a value, and the way an ARN can name nothing (a deleted version, a version
//! never published, another Region, another account, a resource kind nothing registers) —
//! not along the resource names, which the rules say nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::{
    MAX_TAGS_PER_RESOURCE, TAGGING_NOT_FOUND_CODE, TAGGING_NOT_FOUND_MESSAGE,
};
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

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
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
fn tag_object(tags: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// Owned pairs from borrowed ones.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `TagResource` on `arn` (`POST /tags/{resourceArn}`).
async fn tag(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    post(
        router,
        &format!("/tags/{}", encoded_label(arn)),
        &json!({ "tags": tag_object(tags) }),
    )
    .await
}

/// `ListTagsForResource` on `arn` (`GET /tags/{resourceArn}`), the reader that must report
/// what a write persisted.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encoded_label(arn)), Vec::new()).await
}

/// The smallest recipe the parser accepts for `name`@`version`: the three keys the IoT
/// Greengrass component recipe reference requires.
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// Publish `name`@`version`.
async fn create_component(router: &MultiServiceRouter, name: &str, version: &str) {
    let (status, body) = post(
        router,
        "/greengrass/v2/createComponentVersion",
        &json!({ "inlineRecipe": base64::encode(recipe(name, version).as_bytes()) }),
    )
    .await;
    assert_eq!(
        status, 201,
        "publishing {name}:{version} must answer 201, got {status} {body}"
    );
}

/// `CreateDeployment` for `target`; answers the new deployment's id.
async fn create_deployment(router: &MultiServiceRouter, target: &str) -> String {
    let (status, created) = post(
        router,
        "/greengrass/v2/deployments",
        &json!({ "targetArn": target }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {created}"
    );
    text(&created, "deploymentId")
}

/// One row of the write table: the tag sets to write, in order, and the set the resource
/// must hold once they have all been written.
#[derive(Debug)]
struct WriteCase {
    label: &'static str,
    writes: &'static [&'static [(&'static str, &'static str)]],
    expected: &'static [(&'static str, &'static str)],
}

/// The rows differ in how many tags a write carries, whether its keys are new or already
/// present (an update rather than a second entry), and the form of a key and a value —
/// `TagKey` is `length 1..128` and `TagValue` `length 0..256`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`), so the empty value is one a
/// write may carry.
const WRITES: &[WriteCase] = &[
    WriteCase {
        label: "one tag onto an untagged resource",
        writes: &[&[("owner", "platform-team")]],
        expected: &[("owner", "platform-team")],
    },
    WriteCase {
        label: "several tags at once",
        writes: &[&[("owner", "platform-team"), ("tier", "test"), ("wave", "one")]],
        expected: &[("owner", "platform-team"), ("tier", "test"), ("wave", "one")],
    },
    WriteCase {
        label: "a second write adds keys and keeps the first write's",
        writes: &[&[("owner", "platform-team")], &[("tier", "test")]],
        expected: &[("owner", "platform-team"), ("tier", "test")],
    },
    WriteCase {
        label: "a key written twice is updated, not duplicated",
        writes: &[
            &[("owner", "platform-team"), ("tier", "test")],
            &[("owner", "fleet-team")],
        ],
        expected: &[("owner", "fleet-team"), ("tier", "test")],
    },
    WriteCase {
        label: "a value emptied by a later write",
        writes: &[&[("retired", "no")], &[("retired", "")]],
        expected: &[("retired", "")],
    },
    WriteCase {
        label: "keys and values carrying punctuation and non-ASCII",
        writes: &[&[
            ("aws:cost/center", "line=42"),
            ("ünïcode", "naïve value"),
            ("spaced key", "spaced value"),
        ]],
        expected: &[
            ("aws:cost/center", "line=42"),
            ("ünïcode", "naïve value"),
            ("spaced key", "spaced value"),
        ],
    },
];

/// Which kind of resource a row tags. The model's `GenericV2ARN` pattern admits
/// `components`, `deployments` and `coreDevices`; the first two are the resources this
/// emulator holds.
#[derive(Debug, Clone, Copy)]
enum Kind {
    ComponentVersion,
    Deployment,
}

/// Create a resource of `kind` named for `row`, and answer the ARN its tags are written
/// by plus the path of the resource's OWN reader, which must report the same tag set.
async fn create_resource(
    router: &MultiServiceRouter,
    kind: Kind,
    row: usize,
) -> (String, String) {
    match kind {
        Kind::ComponentVersion => {
            let name = format!("com.example.Tagged{row}");
            create_component(router, &name, "1.0.0").await;
            let arn = version_arn(&name, "1.0.0");
            let reader = format!("/greengrass/v2/components/{}/metadata", encoded_label(&arn));
            (arn, reader)
        }
        Kind::Deployment => {
            let id = create_deployment(router, &thing_target(&format!("TaggedCore{row}"))).await;
            assert!(
                !id.is_empty(),
                "CreateDeployment must answer a deployment id for row {row}"
            );
            (deployment_arn(&id), format!("/greengrass/v2/deployments/{id}"))
        }
    }
}

#[tokio::test]
async fn tagging_a_resource_persists_exactly_what_the_write_carried() {
    let router = emulator();

    for kind in [Kind::ComponentVersion, Kind::Deployment] {
        for (row, case) in WRITES.iter().enumerate() {
            let (arn, reader) = create_resource(&router, kind, row).await;

            for (attempt, write) in case.writes.iter().enumerate() {
                let (status, body) = tag(&router, &arn, &pairs(write)).await;
                assert_eq!(
                    status, TAG_WRITE_STATUS,
                    "case {:?} on {kind:?}, write {attempt}: expected \
                     {TAG_WRITE_STATUS}, got {status} {body}",
                    case.label
                );
                // The success carries no content at all: the modelled output declares no
                // member, so there is nothing to parse and no body to parse it from.
                assert!(
                    body.is_null() || body.as_object().is_none_or(Map::is_empty),
                    "case {:?} on {kind:?}, write {attempt}: expected no content, got {body}",
                    case.label
                );
            }

            let expected = tag_object(&pairs(case.expected));
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {:?} on {kind:?}: the listing must answer 200, got {status} {listed}",
                case.label
            );
            assert_eq!(
                listed["tags"], expected,
                "case {:?} on {kind:?}: the listing must report exactly what was written, \
                 got {listed}",
                case.label
            );

            // The listing and the resource's own reader report ONE tag set, because the
            // write landed on the resource rather than beside it.
            let (status, stored) = call(&router, "GET", &reader, Vec::new()).await;
            assert_eq!(
                status, 200,
                "case {:?} on {kind:?}: the resource's own reader must answer 200, \
                 got {status} {stored}",
                case.label
            );
            assert_eq!(
                stored["tags"], expected,
                "case {:?} on {kind:?}: the resource must report the written tags, got {stored}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn a_resource_arn_naming_nothing_is_refused_as_the_tagging_route_words_it() {
    let router = emulator();

    // One resource of each kind exists, and one component version is created and then
    // deleted, so every row below differs from a live resource only in what it names.
    create_component(&router, "com.example.Present", "1.0.0").await;
    let present_deployment = create_deployment(&router, &thing_target("PresentCore")).await;
    create_component(&router, "com.example.Doomed", "2.0.0").await;
    let doomed = version_arn("com.example.Doomed", "2.0.0");
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/greengrass/v2/components/{}", encoded_label(&doomed)),
        Vec::new(),
    )
    .await;
    assert_eq!(
        status, 204,
        "DeleteComponent must answer 204, got {status} {body}"
    );

    let rows = [
        ("a component version that was just deleted", doomed.clone()),
        (
            "a version never published of a component that exists",
            version_arn("com.example.Present", "9.9.9"),
        ),
        (
            "a component version of a component never published",
            version_arn("com.example.Absent", "1.0.0"),
        ),
        (
            "a published component version in another Region",
            format!(
                "arn:aws:greengrass:eu-west-1:{ACCOUNT}:components:com.example.Present:versions:1.0.0"
            ),
        ),
        (
            "a published component version under another account",
            format!(
                "arn:aws:greengrass:{REGION}:111111111111:components:com.example.Present:versions:1.0.0"
            ),
        ),
        (
            "a component ARN with no version, which names no taggable resource",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:components:com.example.Present"),
        ),
        (
            "a deployment id nothing created",
            deployment_arn("44444444-4444-4444-4444-444444444444"),
        ),
        (
            "a core device, which no operation registers",
            format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:coreDevices:PresentCore"),
        ),
    ];

    for (case, arn) in rows {
        // The rule is the ROUTE's, not one operation's: the write and the read of a
        // resource's tags refuse an ARN that names nothing identically.
        for (operation, answer) in [
            ("TagResource", tag(&router, &arn, &pairs(&[("probe", "x")])).await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: an ARN naming no resource must answer 404, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                TAGGING_NOT_FOUND_CODE,
                "case {case:?} on {operation}: expected the tagging route's not-found code, \
                 got {body}"
            );
            assert_eq!(
                text(&body, "message"),
                TAGGING_NOT_FOUND_MESSAGE,
                "case {case:?} on {operation}: expected the tagging route's fixed sentence, \
                 got {body}"
            );
        }
    }

    // The live resources are still taggable, so every row above was refused for what it
    // named and not because the tagging route stopped working.
    for (case, arn) in [
        (
            "the published component version",
            version_arn("com.example.Present", "1.0.0"),
        ),
        ("the created deployment", deployment_arn(&present_deployment)),
    ] {
        let (status, body) = tag(&router, &arn, &pairs(&[("probe", "x")])).await;
        assert_eq!(
            status, TAG_WRITE_STATUS,
            "case {case:?}: the live resource must still be taggable, got {status} {body}"
        );
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, listed["tags"].clone()),
            (200, tag_object(&pairs(&[("probe", "x")]))),
            "case {case:?}: the write must be readable, got {status} {listed}"
        );
    }
}

#[tokio::test]
async fn a_resource_cannot_be_grown_past_the_tag_sets_own_bound() {
    let router = emulator();
    let id = create_deployment(&router, &thing_target("BoundCore")).await;
    let arn = deployment_arn(&id);

    // Exactly the bound is stored: `TagMap`'s own `length 1..200` is what one request may
    // carry, and a resource's stored set is a `TagMap` too.
    let at_the_bound: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let (status, body) = tag(&router, &arn, &at_the_bound).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case exactly {MAX_TAGS_PER_RESOURCE} tags: expected {TAG_WRITE_STATUS}, \
         got {status} {body}"
    );

    // Rewriting a key the resource already carries stays at the bound, so it is served.
    let (status, body) = tag(&router, &arn, &at_the_bound[..1]).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case rewriting a key already stored: expected {TAG_WRITE_STATUS}, got {status} {body}"
    );

    // One key past it is refused, and the stored set is left as it was.
    let (status, body) = tag(&router, &arn, &pairs(&[("one-too-many", "x")])).await;
    assert_eq!(
        status, 400,
        "case one tag past the bound: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case one tag past the bound: expected ValidationException, got {body}"
    );

    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(status, 200, "the listing must answer 200, got {status} {listed}");
    assert_eq!(
        listed["tags"].as_object().map(Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "the refused write must leave the stored set at {MAX_TAGS_PER_RESOURCE}, got {listed}"
    );
    assert!(
        listed["tags"].get("one-too-many").is_none(),
        "the refused write must store nothing, got {listed}"
    );
}
