// SPDX-License-Identifier: CC0-1.0

//! What this service's tag writes do with the `@required` collection they act with when it
//! arrives EMPTY.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see: the status, the `x-amzn-errortype` header it resolves the error shape
//! from, the `message` it surfaces, and the state the call left behind.
//!
//! The rule under test: a write whose `@required` collection member is present and holds
//! nothing states no work to do, so it is refused on the INPUT — the modelled
//! `InvalidRequestException` (HTTP 400,
//! `smithy_query(query="operation",operation="TagResource,UntagResource")`) with the
//! collection validator's fixed sentence — ahead of the resource the request addresses and
//! ahead of any write, rather than answered with a success that changed nothing. Both tag
//! writes act with such a collection: `TagResource$tags` and `UntagResource$tagKeys`.
//!
//! The tables span what that rule generalises over rather than one recorded request: both
//! writes, four resource families (two tag STORAGE shapes: the `TagList` families and the
//! software package's `TagMap`), a resource that carries tags and one that carries none,
//! and the three classes of `resourceArn` the write can name — one the account holds, a
//! well-formed one it does not, and a value that is no ARN of this namespace at all —
//! because the collection is read before any of them is resolved.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::EMPTY_TAG_COLLECTION_MESSAGE;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::describe_endpoint::percent_encode;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// `InvalidRequestException` as this service names it in `x-amzn-errortype`: the shape name
/// with the Coral locator AWS IoT appends to it.
const INVALID_REQUEST: &str = "InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/";

/// A policy document the policy family's create takes.
const POLICY_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:Connect","Resource":"*"}]}"#;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region. The signature itself is never checked here — the harness turns request
/// authentication off, exactly as an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The `message` the body carries, or `""` when it carries none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The tag set `ListTagsForResource` reports, as `(key, value)` pairs in the order it
    /// reported them.
    fn tags(&self) -> Vec<(String, Option<String>)> {
        self.body
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry
                                .get("Key")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            entry
                                .get("Value")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `TagResource`: `POST /tags`, with the `tags` the case states, verbatim.
    async fn tag(&self, resource_arn: &str, tags: Value) -> Reply {
        self.call(
            "POST",
            "/tags",
            Some(&json!({ "resourceArn": resource_arn, "tags": tags })),
        )
        .await
    }

    /// `UntagResource`: `POST /untag`, with the `tagKeys` the case states, verbatim.
    async fn untag(&self, resource_arn: &str, tag_keys: Value) -> Reply {
        self.call(
            "POST",
            "/untag",
            Some(&json!({ "resourceArn": resource_arn, "tagKeys": tag_keys })),
        )
        .await
    }

    /// `ListTagsForResource`: `GET /tags?resourceArn=…`, the reader both writes are checked
    /// against.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/tags?resourceArn={}", percent_encode(resource_arn)),
            None,
        )
        .await
    }

    /// Run a family's create, asserting it succeeded.
    async fn create(&self, case: &str, method: &str, path: &str, body: Value) {
        let created = self.call(method, path, Some(&body)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup {method} {path} must answer 200, got {created:?}"
        );
    }
}

/// An ARN of this account's IoT namespace over `resource`.
fn iot_arn(resource: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// One resource family a tag write addresses by ARN.
struct Family {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Value,
    arn: String,
    /// The tags its create recorded, as the tag read reports them.
    created: Vec<(String, Option<String>)>,
}

/// The families under test: both tag storage shapes this service keeps — the `TagList`
/// families (thing group, authorizer, policy) and the software package's `TagMap` — and a
/// resource created with no tags at all beside ones created with tags.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "a thing group carrying one tag",
            method: "POST",
            path: "/thing-groups/untag-validation-group",
            body: json!({ "tags": [{ "Key": "stage", "Value": "test" }] }),
            arn: iot_arn("thinggroup/untag-validation-group"),
            created: vec![("stage".to_string(), Some("test".to_string()))],
        },
        Family {
            case: "an authorizer carrying one tag",
            method: "POST",
            path: "/authorizer/untag_validation_authorizer",
            body: json!({
                "authorizerFunctionArn":
                    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:auth"),
                "tags": [{ "Key": "stage", "Value": "test" }],
            }),
            arn: iot_arn("authorizer/untag_validation_authorizer"),
            created: vec![("stage".to_string(), Some("test".to_string()))],
        },
        Family {
            case: "a policy created with NO tags",
            method: "POST",
            path: "/policies/untag-validation-policy",
            body: json!({ "policyDocument": POLICY_DOCUMENT }),
            arn: iot_arn("policy/untag-validation-policy"),
            created: vec![],
        },
        Family {
            case: "a software package, whose tags are stored as a TagMap",
            method: "PUT",
            path: "/packages/untag-validation-package",
            body: json!({ "tags": { "stage": "test" } }),
            arn: iot_arn("package/untag-validation-package"),
            created: vec![("stage".to_string(), Some("test".to_string()))],
        },
    ]
}

/// Assert that `reply` is the refusal an empty tag collection draws, and nothing else.
fn assert_empty_collection_refusal(case: &str, write: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?} / {write}: an empty collection must be refused 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(INVALID_REQUEST),
        "case {case:?} / {write}: expected {INVALID_REQUEST}, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        EMPTY_TAG_COLLECTION_MESSAGE,
        "case {case:?} / {write}: expected the collection validator's sentence, got {reply:?}"
    );
}

#[tokio::test]
async fn an_empty_tag_collection_is_refused_and_changes_no_resources_tags() {
    let emulator = Emulator::new();

    for Family {
        case,
        method,
        path,
        body,
        arn,
        created,
    } in families()
    {
        emulator.create(case, method, path, body).await;
        let before = emulator.list_tags(&arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the tag read of a held resource must answer 200, got {before:?}"
        );
        assert_eq!(
            before.tags(),
            created,
            "case {case:?}: the created tags must be readable, got {before:?}"
        );

        // Both writes state a collection, and both refuse one that is there and empty.
        for (write, reply) in [
            ("UntagResource", emulator.untag(&arn, json!([])).await),
            ("TagResource", emulator.tag(&arn, json!([])).await),
        ] {
            assert_empty_collection_refusal(case, write, &reply);
        }

        // The refusals wrote nothing: the resource carries exactly the tags it did.
        let after = emulator.list_tags(&arn).await;
        assert_eq!(
            after.tags(),
            created,
            "case {case:?}: the refused writes must leave the tag set untouched, got {after:?}"
        );

        // A collection that names something is accepted, which is what makes the refusal
        // about EMPTINESS and not about the write: one key added and then removed again.
        let tagged = emulator
            .tag(&arn, json!([{ "Key": "owner", "Value": "fleet" }]))
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: a stated tag must be accepted, got {tagged:?}"
        );
        let read = emulator.list_tags(&arn).await;
        assert!(
            read.tags()
                .iter()
                .any(|(key, value)| key == "owner" && value.as_deref() == Some("fleet")),
            "case {case:?}: the written tag must be readable, got {read:?}"
        );
        let untagged = emulator.untag(&arn, json!(["owner"])).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: a stated key must be removed, got {untagged:?}"
        );
        let read = emulator.list_tags(&arn).await;
        assert_eq!(
            read.tags(),
            created,
            "case {case:?}: removing the written key must leave the created tags, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_empty_collection_is_refused_whatever_resource_the_write_names() {
    let emulator = Emulator::new();

    // The collection is read on the INPUT, so which resource the ARN names cannot change
    // the refusal: one the account holds, a well-formed one it does not, and a value that
    // addresses no resource of this namespace at all.
    emulator
        .create(
            "setup",
            "POST",
            "/thing-groups/arn-class-group",
            json!({ "tags": [{ "Key": "stage", "Value": "test" }] }),
        )
        .await;

    let arns = [
        ("an ARN the account holds", iot_arn("thinggroup/arn-class-group")),
        (
            "a well-formed ARN of a resource the account does not hold",
            iot_arn("thinggroup/no-such-group"),
        ),
        (
            "an ARN of a resource type this service does not tag",
            iot_arn("thing/some-thing"),
        ),
        ("a value that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in arns {
        for (write, reply) in [
            ("UntagResource", emulator.untag(&arn, json!([])).await),
            ("TagResource", emulator.tag(&arn, json!([])).await),
        ] {
            assert_empty_collection_refusal(case, write, &reply);
        }
    }

    // The resource the first row names is untouched by any of it.
    let read = emulator
        .list_tags(&iot_arn("thinggroup/arn-class-group"))
        .await;
    assert_eq!(
        read.tags(),
        vec![("stage".to_string(), Some("test".to_string()))],
        "the held resource's tags must be unchanged, got {read:?}"
    );
}
