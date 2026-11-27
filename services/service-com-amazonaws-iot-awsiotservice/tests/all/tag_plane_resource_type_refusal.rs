// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of how the TAG plane refuses a `resourceArn` whose resource
//! TYPE carries no tag set, and of the ARNs it does act on.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see.
//!
//! The rule under test: `TagResource`, `UntagResource` and `ListTagsForResource` act on a
//! resource by ARN alone, and an ARN of this namespace whose resource TYPE is not one they
//! keep tags for is refused HTTP 400 `InvalidRequestException` naming THAT TYPE — the fixed
//! text `Invalid resource type in ARN: ` with the type read out of the request's own ARN as
//! the only variable part, because what the caller has to change is the kind of resource it
//! addressed and not the ARN's spelling. An ARN of a type they DO keep tags for reads and
//! modifies that resource's tags.
//!
//! The table spans what the rule generalises over rather than one recorded ARN: several
//! untaggable IoT resource types (a thing, a certificate, a job, a topic-rule destination,
//! an index), the resource the ARN names being present or absent, and all three operations
//! of the plane — beside a taggable type, which must still work.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The one query-value percent-encoder this test crate has (`tests/all/describe_endpoint.rs`):
// `automod` declares every suite as a sibling module, so the `resourceArn` bound to the query
// below travels through that helper rather than through a second copy of the same rule.
use crate::describe_endpoint::percent_encode;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for.
const REGION: &str = "us-west-2";

/// One emulator: the shared engine plus a router with this service mounted.
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
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|code| code.split(':').next().unwrap_or(code).to_string());
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `ListTagsForResource`: `GET /tags?resourceArn={resourceArn}`.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/tags?resourceArn={}", percent_encode(resource_arn)),
            None,
        )
        .await
    }

    /// `TagResource`: `POST /tags`.
    async fn tag(&self, resource_arn: &str, key: &str, value: &str) -> Reply {
        self.call(
            "POST",
            "/tags",
            Some(&json!({
                "resourceArn": resource_arn,
                "tags": [{ "Key": key, "Value": value }],
            })),
        )
        .await
    }

    /// `UntagResource`: `POST /untag`.
    async fn untag(&self, resource_arn: &str, key: &str) -> Reply {
        self.call(
            "POST",
            "/untag",
            Some(&json!({ "resourceArn": resource_arn, "tagKeys": [key] })),
        )
        .await
    }
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `(Key, Value)` pairs a `ListTagsForResource` answer reported.
    fn tags(&self) -> Vec<(String, String)> {
        self.body
            .get("tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        let member = |name: &str| {
                            entry
                                .get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (member("Key"), member("Value"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// An ARN of this account's IoT namespace naming `resource`.
fn iot_arn(resource: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// All three operations of the plane refuse an ARN whose resource TYPE carries no tag set by
/// naming that type, whether or not the resource it goes on to name exists.
#[tokio::test]
async fn the_tag_plane_refuses_an_untaggable_resource_type_by_naming_the_type() {
    let emulator = Emulator::new();

    // A thing that EXISTS, so the refusal cannot be a lookup miss in disguise: a thing is
    // not a taggable resource of this plane whatever the registry holds.
    let created = emulator
        .call("POST", "/things/sensor-1", Some(&json!({})))
        .await;
    assert_eq!(
        created.status, 200,
        "case the registered thing: CreateThing must answer 200, got {created:?}"
    );
    let thing_arn = created
        .body
        .get("thingArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no thingArn in {created:?}"))
        .to_string();

    // The rows span the IoT resource TYPES this plane keeps no tag set for, each with the
    // type its refusal has to name: the registered thing above, a thing that does not
    // exist, a device certificate, a job, a job execution's own ARN form, a topic-rule
    // destination and an index.
    let cases: &[(&str, String, &str)] = &[
        ("the registered thing", thing_arn, "thing"),
        ("a thing the registry does not hold", iot_arn("thing/ghost"), "thing"),
        (
            "a device certificate",
            iot_arn(&format!("cert/{}", "ab".repeat(32))),
            "cert",
        ),
        ("a job", iot_arn("job/rollout-7"), "job"),
        ("an index", iot_arn("index/AWS_Things"), "index"),
        (
            "a topic rule destination",
            iot_arn("destination/vpc/vpce-0a1b2c3d"),
            "destination",
        ),
    ];

    for (case, resource_arn, resource_type) in cases {
        let expected = format!("Invalid resource type in ARN: {resource_type}");
        let replies = [
            ("ListTagsForResource", emulator.list_tags(resource_arn).await),
            ("TagResource", emulator.tag(resource_arn, "stage", "test").await),
            ("UntagResource", emulator.untag(resource_arn, "stage").await),
        ];
        for (operation, reply) in replies {
            assert_eq!(
                reply.status, 400,
                "case {case} {operation}: expected HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("InvalidRequestException"),
                "case {case} {operation}: expected InvalidRequestException, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case} {operation}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }
}

/// The same three operations act on an ARN of a type the plane DOES keep tags for: the
/// refusal above is about the resource type and not about the plane.
#[tokio::test]
async fn the_tag_plane_reads_and_modifies_a_taggable_resource_type() {
    let emulator = Emulator::new();

    let created = emulator
        .call("POST", "/thing-groups/fleet-alpha", Some(&json!({})))
        .await;
    assert_eq!(
        created.status, 200,
        "case the thing group: CreateThingGroup must answer 200, got {created:?}"
    );
    let group_arn = created
        .body
        .get("thingGroupArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no thingGroupArn in {created:?}"))
        .to_string();

    let tagged = emulator.tag(&group_arn, "stage", "test").await;
    assert_eq!(
        tagged.status, 200,
        "case the thing group: TagResource must answer 200, got {tagged:?}"
    );
    let listed = emulator.list_tags(&group_arn).await;
    assert_eq!(
        listed.status, 200,
        "case the thing group: ListTagsForResource must answer 200, got {listed:?}"
    );
    assert_eq!(
        listed.tags(),
        vec![("stage".to_string(), "test".to_string())],
        "case the thing group: expected the written tag, got {listed:?}"
    );

    let untagged = emulator.untag(&group_arn, "stage").await;
    assert_eq!(
        untagged.status, 200,
        "case the thing group: UntagResource must answer 200, got {untagged:?}"
    );
    let emptied = emulator.list_tags(&group_arn).await;
    assert_eq!(
        emptied.tags(),
        Vec::<(String, String)>::new(),
        "case the thing group: expected no tags after the untag, got {emptied:?}"
    );
}
