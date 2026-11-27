// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an `@idempotencyToken` retry's TAGS do to the
//! parameter comparison, driven over the same router the production emulator
//! assembles.
//!
//! The rule under test is about the comparison rather than one recorded request:
//! a retry presenting a token already spent on this operation is answered from
//! the first call's record only when it states the SAME parameters, so two
//! requests that differ in any member — tags included — never compare equal.
//! A modelled `TagValue` carries no pattern at all and a `TagKey` admits `=`
//! (`smithy_query(query="shape", shape="TagsMap,TagKey,TagValue")`), so the
//! characters a rendering of a tag map might use to separate its pairs are
//! characters the tags themselves may contain: the comparison has to tell the
//! pairs apart whatever is inside them.
//!
//! The table spans what the rule generalises over: WHERE the boundary between
//! one pair and the next falls — a separator inside a value, an `=` inside a
//! key, an empty value beside a pair, a value that is nothing but a separator,
//! and a longer split of the same characters — each row a pair of two distinct,
//! both-legal maps, driven in both directions, against every create of this
//! service that carries both a client token and a tag map.
//!
//! The same maps drive the replay side: a retry stating one of them again is
//! the retry it is and answers with the resource the first call minted, which
//! reads back carrying those tags.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The token every create under test is driven with.
const TOKEN: &str = "token-under-test";

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

/// A `CreateRequesterGateway` body, without the client token: every member
/// satisfies the modelled patterns.
fn requester_gateway_base() -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
    })
}

/// A `CreateResponderGateway` body, without the client token, for a gateway of
/// the `EXTERNAL` type — the kind that takes inbound external links.
fn responder_gateway_base() -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "port": 443,
        "protocol": "HTTPS",
        "listenerConfig": { "protocols": ["HTTPS"] },
        "domainName": "responder.example.com",
        "gatewayType": "EXTERNAL",
    })
}

/// `base` with `clientToken` stated, which every create in this suite requires.
fn with_token(mut base: Value, token: &str) -> Value {
    base.as_object_mut()
        .expect("a JSON object body")
        .insert("clientToken".to_string(), json!(token));
    base
}

/// Every create of this service that carries both an `@idempotencyToken` and a
/// `TagsMap`, so the tags a retry states are part of what the token's record is
/// compared against.
#[derive(Clone, Copy, Debug)]
enum Surface {
    RequesterGateway,
    ResponderGateway,
    OutboundExternalLink,
    InboundExternalLink,
    LinkRoutingRule,
}

const SURFACES: [Surface; 5] = [
    Surface::RequesterGateway,
    Surface::ResponderGateway,
    Surface::OutboundExternalLink,
    Surface::InboundExternalLink,
    Surface::LinkRoutingRule,
];

impl Surface {
    /// The operation the mismatch refusal names.
    fn operation(self) -> &'static str {
        match self {
            Self::RequesterGateway => "CreateRequesterGateway",
            Self::ResponderGateway => "CreateResponderGateway",
            Self::OutboundExternalLink => "CreateOutboundExternalLink",
            Self::InboundExternalLink => "CreateInboundExternalLink",
            Self::LinkRoutingRule => "CreateLinkRoutingRule",
        }
    }
}

/// One answered call: the HTTP status, the wire error code (absent on a success)
/// and the parsed body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

impl Answer {
    fn text(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// Where one create under test is sent, and how the resource it mints reads
/// back.
struct Target {
    /// The path the create is POSTed to; the minted resource reads back under it.
    create_path: String,
    /// Every member of the create body besides `clientToken` and `tags`.
    base_body: Value,
    /// The output member carrying the id the create minted.
    id_member: &'static str,
}

impl Target {
    /// The create body for `token`, carrying `tags`.
    fn body(&self, token: &str, tags: &Value) -> Value {
        let mut body = self.base_body.clone();
        let object = body.as_object_mut().expect("a JSON object body");
        object.insert("clientToken".to_string(), json!(token));
        object.insert("tags".to_string(), tags.clone());
        body
    }

    fn read_path(&self, id: &str) -> String {
        format!("{}/{id}", self.create_path)
    }
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 request and return what came back.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Answer { status, code, body }
    }

    /// One prerequisite call, which has to succeed for the create under test to
    /// be reachable; answers the id it minted.
    async fn prerequisite(
        &self,
        case: &str,
        operation: &str,
        path: &str,
        body: Value,
        id_member: &str,
    ) -> String {
        let answered = self.send("POST", path, Some(body)).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected {operation} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered.text(id_member)
    }

    /// The store this surface's create needs under it, and where that create is
    /// sent.
    async fn target(&self, case: &str, surface: Surface) -> Target {
        match surface {
            Surface::RequesterGateway => Target {
                create_path: "/requester-gateway".to_string(),
                base_body: requester_gateway_base(),
                id_member: "gatewayId",
            },
            Surface::ResponderGateway => Target {
                create_path: "/responder-gateway".to_string(),
                base_body: responder_gateway_base(),
                id_member: "gatewayId",
            },
            Surface::OutboundExternalLink => {
                let gateway_id = self
                    .prerequisite(
                        case,
                        "CreateRequesterGateway",
                        "/requester-gateway",
                        with_token(requester_gateway_base(), "token-requester"),
                        "gatewayId",
                    )
                    .await;
                Target {
                    create_path: format!("/requester-gateway/{gateway_id}/outbound-external-link"),
                    base_body: json!({
                        "publicEndpoint": "https://bidder.example.com/rtb",
                        "logSettings": log_settings(),
                    }),
                    id_member: "linkId",
                }
            }
            Surface::InboundExternalLink => {
                let gateway_id = self
                    .prerequisite(
                        case,
                        "CreateResponderGateway",
                        "/responder-gateway",
                        with_token(responder_gateway_base(), "token-responder"),
                        "gatewayId",
                    )
                    .await;
                Target {
                    create_path: format!("/responder-gateway/{gateway_id}/inbound-external-link"),
                    base_body: json!({ "logSettings": log_settings() }),
                    id_member: "linkId",
                }
            }
            Surface::LinkRoutingRule => {
                let requester_id = self
                    .prerequisite(
                        case,
                        "CreateRequesterGateway",
                        "/requester-gateway",
                        with_token(requester_gateway_base(), "token-requester"),
                        "gatewayId",
                    )
                    .await;
                let responder_id = self
                    .prerequisite(
                        case,
                        "CreateResponderGateway",
                        "/responder-gateway",
                        with_token(responder_gateway_base(), "token-responder"),
                        "gatewayId",
                    )
                    .await;
                let link_id = self
                    .prerequisite(
                        case,
                        "CreateLink",
                        &format!("/gateway/{requester_id}/create-link"),
                        json!({
                            "peerGatewayId": responder_id,
                            "logSettings": log_settings(),
                        }),
                        "linkId",
                    )
                    .await;
                Target {
                    create_path: format!(
                        "/responder-gateway/{responder_id}/link/{link_id}/routing-rule"
                    ),
                    base_body: json!({
                        "priority": 10,
                        "conditions": { "pathPrefix": "/bid" },
                    }),
                    id_member: "ruleId",
                }
            }
        }
    }
}

/// Two DISTINCT, both-legal tag maps whose pairs hold the characters a rendering
/// of a map might otherwise read as its own structure.
struct CollisionRow {
    case: &'static str,
    one: fn() -> Value,
    other: fn() -> Value,
}

fn collision_rows() -> Vec<CollisionRow> {
    vec![
        CollisionRow {
            case: "one tag whose value holds a separator and an equals, against two tags",
            one: || json!({ "a": "b,c=d" }),
            other: || json!({ "a": "b", "c": "d" }),
        },
        CollisionRow {
            case: "an equals inside the key, against an equals inside the value",
            one: || json!({ "a=b": "c" }),
            other: || json!({ "a": "b=c" }),
        },
        CollisionRow {
            case: "an empty value beside a pair, against one value holding both",
            one: || json!({ "a": "", "b": "c" }),
            other: || json!({ "a": ",b=c" }),
        },
        CollisionRow {
            case: "a value that is nothing but an equals, against a key ending in one",
            one: || json!({ "k": "=" }),
            other: || json!({ "k=": "" }),
        },
        CollisionRow {
            case: "a longer split of the same characters",
            one: || json!({ "env": "prod,team=rtb" }),
            other: || json!({ "env": "prod", "team": "rtb" }),
        },
    ]
}

/// Drive one surface: create with `first`, retry the same token with `second`,
/// and assert the retry is refused as the parameter mismatch it is while the
/// first call's resource stands untouched.
async fn assert_mismatch_refused(surface: Surface, case: &str, first: Value, second: Value) {
    let operation = surface.operation();
    let case = format!("{operation}: {case} (first {first}, retry {second})");
    let fabric = Fabric::new();
    let target = fabric.target(&case, surface).await;

    let created = fabric
        .send("POST", &target.create_path, Some(target.body(TOKEN, &first)))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected {operation} to answer 200, got {} {:?}",
        created.status, created.body
    );
    let id = created.text(target.id_member);

    let retry = fabric
        .send(
            "POST",
            &target.create_path,
            Some(target.body(TOKEN, &second)),
        )
        .await;
    assert_eq!(
        (retry.status, retry.code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {} {:?} {:?}",
        retry.status,
        retry.code,
        retry.body
    );
    assert_eq!(
        retry.text("message"),
        format!(
            "clientToken {TOKEN} was already used for a {operation} request with different parameters"
        ),
        "case {case:?}: expected the mismatch refusal to state the token and the operation, got {:?}",
        retry.body
    );

    // The refusal stored nothing: the resource the first call minted still
    // reports the tags IT was given, and the retry's map reached no record.
    let read = fabric.send("GET", &target.read_path(&id), None).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected the first call's resource to read back, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["tags"],
        first,
        "case {case:?}: expected the stored resource to carry the first call's tags, got {:?}",
        read.body["tags"]
    );
}

#[tokio::test]
async fn a_retry_stating_a_different_tag_map_is_refused() {
    for surface in SURFACES {
        for row in collision_rows() {
            assert_mismatch_refused(surface, row.case, (row.one)(), (row.other)()).await;
        }
    }
}

#[tokio::test]
async fn the_refusal_does_not_depend_on_which_of_the_two_maps_came_first() {
    for surface in SURFACES {
        for row in collision_rows() {
            assert_mismatch_refused(surface, row.case, (row.other)(), (row.one)()).await;
        }
    }
}

#[tokio::test]
async fn a_retry_stating_the_same_tag_map_answers_with_the_first_call_s_resource() {
    for surface in SURFACES {
        let operation = surface.operation();
        for row in collision_rows() {
            for tags in [(row.one)(), (row.other)()] {
                let case = format!("{operation}: {} (tags {tags})", row.case);
                let fabric = Fabric::new();
                let target = fabric.target(&case, surface).await;

                let created = fabric
                    .send("POST", &target.create_path, Some(target.body(TOKEN, &tags)))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected {operation} to answer 200, got {} {:?}",
                    created.status, created.body
                );
                let id = created.text(target.id_member);

                // The retry states the same map, so it is the retry it claims to
                // be, whatever characters the tags hold.
                let retry = fabric
                    .send("POST", &target.create_path, Some(target.body(TOKEN, &tags)))
                    .await;
                assert_eq!(
                    (retry.status, retry.text(target.id_member)),
                    (200, id.clone()),
                    "case {case:?}: expected the retry to answer with the resource the first call minted, got {} {:?}",
                    retry.status,
                    retry.body
                );

                let read = fabric.send("GET", &target.read_path(&id), None).await;
                assert_eq!(
                    (read.status, read.body["tags"].clone()),
                    (200, tags.clone()),
                    "case {case:?}: expected the stored resource to carry those tags, got {} {:?}",
                    read.status,
                    read.body
                );
            }
        }
    }
}
