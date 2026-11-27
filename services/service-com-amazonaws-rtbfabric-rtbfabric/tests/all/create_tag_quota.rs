// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the per-resource tag bound on the CREATE side, driven over the
//! wire through the router the production emulator uses.
//!
//! The rule under test is about the RESOURCE, not about one operation: a record holds
//! at most `TAGS_PER_RESOURCE_QUOTA` tags whichever call assigned them, so every call
//! that writes a record's `tags` map admits at most that many and refuses more with the
//! modelled `ValidationException` (HTTP 400) having stored nothing. A create is one of
//! those calls, so no create can mint a record the tagging surface would then refuse to
//! add a single tag to.
//!
//! The table spans what the rule generalises over: which create writes the map (both
//! gateway creates, the link create, the routing-rule create and both external-link
//! creates), how many tags the call carries (none, one, one below the bound, the bound
//! exactly, one past it and well past it) and what the keys and values look like —
//! plain keys, keys spanning every punctuation character the modelled `TagKey` pattern
//! admits, a key at the published 128-character maximum, a value at the published
//! 1600-character maximum and a value at the published minimum of none
//! (`smithy_query(query="shape", shape="TagsMap")`, whose map carries no member-count
//! constraint of its own, so nothing refuses an over-filled map before the handler).

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::state::TAGS_PER_RESOURCE_QUOTA;
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tagging::tags_path;

/// The region the test signs for, and so the region every ARN it builds names.
const REGION: &str = "us-west-2";

/// The answer a call asking to write more tags than one record holds must carry.
const OVER_BOUND_STATUS: u16 = 400;
const OVER_BOUND_CODE: &str = "ValidationException";

/// The whole message such a refusal states, written out here rather than read back
/// from the crate under test, so a handler that moved to other wording fails this
/// suite. The bound itself is the variable part.
fn over_bound_message() -> String {
    format!("a resource holds at most {TAGS_PER_RESOURCE_QUOTA} tags")
}

/// One answered call: the status, the wire error code (absent on a success) and the
/// parsed body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

impl Answer {
    fn message(&self) -> String {
        text(&self.body, "message")
    }
}

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// A `tags` member as a sorted map, so two tag sets compare independently of the order
/// they were written in. An absent or null member is the empty map: a record carrying
/// no tags is reported by leaving the member out.
fn tags_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    if tags.is_null() {
        return BTreeMap::new();
    }
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags map, got {tags:?}"));
    object
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected the tag {key:?} to be a string, got {value:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// What the keys and values of a row's tag map look like. Every form stays inside the
/// published bounds of `TagKey` (length 1..128, pattern
/// `^(resourceArn|internalId|[a-zA-Z0-9+\-=._:/@]+)$`) and `TagValue` (length 0..1600),
/// from `smithy_query(query="shape", shape="TagsMap")`, so what a refusal is about is
/// the number of entries and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Short alphanumeric keys and values.
    Plain,
    /// Keys carrying every punctuation character the key pattern admits, and values at
    /// the published minimum length of none.
    Punctuated,
    /// Keys at the published 128-character maximum, with the first value at the
    /// published 1600-character maximum.
    AtBounds,
}

/// `count` distinct tags of the given form.
fn tag_rows(count: usize, shape: Shape) -> Vec<(String, String)> {
    (0..count)
        .map(|n| match shape {
            Shape::Plain => (format!("key-{n}"), format!("value-{n}")),
            Shape::Punctuated => (format!("k+{n}=a.b_c:d/e@f-g"), String::new()),
            Shape::AtBounds => {
                let stem = format!("key-{n}");
                let key = format!("{stem}{}", "a".repeat(128 - stem.len()));
                let value = if n == 0 {
                    "v".repeat(1600)
                } else {
                    format!("value-{n}")
                };
                (key, value)
            }
        })
        .collect()
}

fn tags_json(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

fn expected_map(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// Which create writes the `tags` map under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Create {
    RequesterGateway,
    ResponderGateway,
    /// A link between the two stored gateways.
    Link,
    /// A routing rule on a link the scaffold already opened.
    LinkRoutingRule,
    /// A link from the requester gateway to an address outside the fabric.
    OutboundExternalLink,
    /// A link arriving at the responder gateway from outside the fabric.
    InboundExternalLink,
}

/// Every create that writes a record's `tags` map: the whole set the rule covers.
const EVERY_CREATE: [Create; 6] = [
    Create::RequesterGateway,
    Create::ResponderGateway,
    Create::Link,
    Create::LinkRoutingRule,
    Create::OutboundExternalLink,
    Create::InboundExternalLink,
];

/// The records a create under test needs before it can run.
struct Scaffold {
    requester_id: String,
    responder_id: String,
    /// The link a routing-rule create hangs its rule on; empty for the other creates.
    link_id: String,
}

/// Where an accepted create's record is read back: through its own reader and through
/// the `/tags/{resourceArn}` surface.
struct Minted {
    reader_path: String,
    resource_arn: String,
}

/// The one emulated service, behind the same `MultiServiceRouter` the production router
/// assembles, so these calls take the real deserialize -> validate -> handle ->
/// serialize path.
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

    /// The untagged records `kind`'s create needs: a requester gateway, an EXTERNAL
    /// responder gateway (the only kind that takes an inbound external link), and — for
    /// the routing-rule create only — a link between the two.
    async fn scaffold(&self, case: &str, kind: Create) -> Scaffold {
        let requester = self
            .send(
                "POST",
                "/requester-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": "scaffold-requester",
                })),
            )
            .await;
        assert_eq!(
            requester.status, 200,
            "case {case:?}: expected the scaffold's CreateRequesterGateway to answer 200, got {} {:?}",
            requester.status, requester.body
        );
        let responder = self
            .send(
                "POST",
                "/responder-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 443,
                    "protocol": "HTTPS",
                    "listenerConfig": { "protocols": ["HTTPS"] },
                    "domainName": "responder.example.com",
                    "gatewayType": "EXTERNAL",
                    "clientToken": "scaffold-responder",
                })),
            )
            .await;
        assert_eq!(
            responder.status, 200,
            "case {case:?}: expected the scaffold's CreateResponderGateway to answer 200, got {} {:?}",
            responder.status, responder.body
        );
        let requester_id = text(&requester.body, "gatewayId");
        let responder_id = text(&responder.body, "gatewayId");

        // Only the routing-rule create needs a link to hang its rule on; the link
        // create under test needs the pair NOT to be linked yet.
        let link_id = if kind == Create::LinkRoutingRule {
            let link = self
                .send(
                    "POST",
                    &format!("/gateway/{requester_id}/create-link"),
                    Some(json!({
                        "peerGatewayId": responder_id,
                        "logSettings": log_settings(),
                    })),
                )
                .await;
            assert_eq!(
                link.status, 200,
                "case {case:?}: expected the scaffold's CreateLink to answer 200, got {} {:?}",
                link.status, link.body
            );
            text(&link.body, "linkId")
        } else {
            String::new()
        };

        Scaffold {
            requester_id,
            responder_id,
            link_id,
        }
    }

    /// Drive `kind`'s create carrying `tags`, under one and the same client token, so a
    /// retry after a refusal is the SAME create rather than another one.
    async fn create(
        &self,
        scaffold: &Scaffold,
        kind: Create,
        tags: &[(String, String)],
    ) -> Answer {
        let requester_id = &scaffold.requester_id;
        let responder_id = &scaffold.responder_id;
        let link_id = &scaffold.link_id;
        let (path, mut body) = match kind {
            Create::RequesterGateway => (
                "/requester-gateway".to_string(),
                json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": "under-test",
                }),
            ),
            Create::ResponderGateway => (
                "/responder-gateway".to_string(),
                json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 8443,
                    "protocol": "HTTPS",
                    "listenerConfig": { "protocols": ["HTTPS"] },
                    "domainName": "tagged.example.com",
                    "clientToken": "under-test",
                }),
            ),
            Create::Link => (
                format!("/gateway/{requester_id}/create-link"),
                json!({
                    "peerGatewayId": responder_id,
                    "logSettings": log_settings(),
                }),
            ),
            Create::LinkRoutingRule => (
                format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule"),
                json!({
                    "clientToken": "under-test",
                    "priority": 1,
                    "conditions": { "pathPrefix": "/bid" },
                }),
            ),
            Create::OutboundExternalLink => (
                format!("/requester-gateway/{requester_id}/outbound-external-link"),
                json!({
                    "clientToken": "under-test",
                    "publicEndpoint": "https://bidder.example.com/rtb",
                    "logSettings": log_settings(),
                }),
            ),
            Create::InboundExternalLink => (
                format!("/responder-gateway/{responder_id}/inbound-external-link"),
                json!({
                    "clientToken": "under-test",
                    "logSettings": log_settings(),
                }),
            ),
        };
        // An absent `tags` member and an empty map are two different requests; the row
        // carrying no tag at all sends neither.
        if !tags.is_empty() {
            body["tags"] = tags_json(tags);
        }
        self.send("POST", &path, Some(body)).await
    }

    /// `ListTagsForResource` on `resource_arn`, asserting it succeeded, as a sorted map.
    async fn listed_tags(&self, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
        let answered = self.send("GET", &tags_path(resource_arn), None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected ListTagsForResource on {resource_arn:?} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        tags_map(case, &answered.body["tags"])
    }
}

/// Where the record an accepted create minted is read back, for each create.
fn minted(kind: Create, scaffold: &Scaffold, answered: &Answer) -> Minted {
    let requester_id = &scaffold.requester_id;
    let responder_id = &scaffold.responder_id;
    let link_id = &scaffold.link_id;
    let (reader_path, resource) = match kind {
        Create::RequesterGateway => {
            let id = text(&answered.body, "gatewayId");
            (
                format!("/requester-gateway/{id}"),
                format!("gateway/{id}"),
            )
        }
        Create::ResponderGateway => {
            let id = text(&answered.body, "gatewayId");
            (
                format!("/responder-gateway/{id}"),
                format!("gateway/{id}"),
            )
        }
        Create::Link => {
            let id = text(&answered.body, "linkId");
            (
                format!("/gateway/{requester_id}/link/{id}"),
                format!("gateway/{requester_id}/link/{id}"),
            )
        }
        Create::LinkRoutingRule => {
            let id = text(&answered.body, "ruleId");
            (
                format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule/{id}"),
                format!("gateway/{responder_id}/link/{link_id}/routing-rule/{id}"),
            )
        }
        Create::OutboundExternalLink => {
            let id = text(&answered.body, "linkId");
            (
                format!("/requester-gateway/{requester_id}/outbound-external-link/{id}"),
                format!("gateway/{requester_id}/link/{id}"),
            )
        }
        Create::InboundExternalLink => {
            let id = text(&answered.body, "linkId");
            (
                format!("/responder-gateway/{responder_id}/inbound-external-link/{id}"),
                format!("gateway/{responder_id}/link/{id}"),
            )
        }
    };
    Minted {
        reader_path,
        resource_arn: ::engine::emulator_arn("rtbfabric", REGION, &resource),
    }
}

/// Assert `answered` is the refusal of a map carrying more tags than one record holds.
fn assert_over_bound(case: &str, answered: &Answer) {
    assert_eq!(
        answered.status, OVER_BOUND_STATUS,
        "case {case:?}: expected the create to be refused with {OVER_BOUND_STATUS}, got {} {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.code, OVER_BOUND_CODE,
        "case {case:?}: expected the code {OVER_BOUND_CODE}, got {:?}",
        answered.code
    );
    assert_eq!(
        answered.message(),
        over_bound_message(),
        "case {case:?}: expected the message {:?}, got {:?}",
        over_bound_message(),
        answered.message()
    );
}

/// A create carrying at most the bound stores exactly the map it carried, and one
/// carrying more is refused and stores nothing — for every create that writes a
/// record's `tags` map.
#[tokio::test]
async fn a_create_admits_at_most_the_per_resource_tag_bound() {
    // The counts the rule turns on: none, one, one below the bound, the bound exactly,
    // one past it, and well past it.
    let counts: [usize; 6] = [
        0,
        1,
        TAGS_PER_RESOURCE_QUOTA - 1,
        TAGS_PER_RESOURCE_QUOTA,
        TAGS_PER_RESOURCE_QUOTA + 1,
        TAGS_PER_RESOURCE_QUOTA + 10,
    ];

    for kind in EVERY_CREATE {
        for count in counts {
            let case = format!("{kind:?} carrying {count} tags");
            let fabric = Fabric::new();
            let scaffold = fabric.scaffold(&case, kind).await;
            let carried = tag_rows(count, Shape::Plain);
            let answered = fabric.create(&scaffold, kind, &carried).await;

            if count > TAGS_PER_RESOURCE_QUOTA {
                assert_over_bound(&case, &answered);

                // Nothing of the refused call was stored: the very same create, with
                // its map trimmed to the bound, is accepted. A record the refused call
                // had minted would have taken this create's client token, the gateway
                // pair's one link or the link's priority, and this retry would be
                // refused instead.
                let trimmed = tag_rows(TAGS_PER_RESOURCE_QUOTA, Shape::Plain);
                let retried = fabric.create(&scaffold, kind, &trimmed).await;
                assert_eq!(
                    retried.status, 200,
                    "case {case:?}: expected the retry at the bound to be accepted, got {} {:?}",
                    retried.status, retried.body
                );
                let record = minted(kind, &scaffold, &retried);
                let listed = fabric.listed_tags(&case, &record.resource_arn).await;
                assert_eq!(
                    listed,
                    expected_map(&trimmed),
                    "case {case:?}: expected the retried create to store exactly its {TAGS_PER_RESOURCE_QUOTA} tags, got {} entries",
                    listed.len()
                );
                continue;
            }

            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected the create to be accepted, got {} {:?}",
                answered.status, answered.body
            );
            let record = minted(kind, &scaffold, &answered);

            // The tags live on the record the create minted: the tagging surface and
            // the resource's own reader report the same map.
            let listed = fabric.listed_tags(&case, &record.resource_arn).await;
            assert_eq!(
                listed,
                expected_map(&carried),
                "case {case:?}: expected ListTagsForResource to report the {count} created tags, got {} entries",
                listed.len()
            );
            let read = fabric.send("GET", &record.reader_path, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the record at {:?} to be readable, got {} {:?}",
                record.reader_path, read.status, read.body
            );
            assert_eq!(
                tags_map(&case, &read.body["tags"]),
                expected_map(&carried),
                "case {case:?}: expected the record's own reader to report the {count} created tags, got {:?}",
                read.body["tags"]
            );
        }
    }
}

/// What a refusal is about is how many entries the map carries, not what its keys and
/// values look like: every form inside the published `TagKey` and `TagValue` bounds is
/// accepted at the bound and refused one past it.
#[tokio::test]
async fn the_bound_counts_entries_whatever_the_keys_and_values_look_like() {
    for shape in [Shape::Plain, Shape::Punctuated, Shape::AtBounds] {
        for kind in [Create::RequesterGateway, Create::Link] {
            for count in [TAGS_PER_RESOURCE_QUOTA, TAGS_PER_RESOURCE_QUOTA + 1] {
                let case = format!("{kind:?} carrying {count} {shape:?} tags");
                let fabric = Fabric::new();
                let scaffold = fabric.scaffold(&case, kind).await;
                let carried = tag_rows(count, shape);
                let answered = fabric.create(&scaffold, kind, &carried).await;

                if count > TAGS_PER_RESOURCE_QUOTA {
                    assert_over_bound(&case, &answered);
                    continue;
                }

                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: expected the create to be accepted, got {} {:?}",
                    answered.status, answered.body
                );
                let record = minted(kind, &scaffold, &answered);
                let listed = fabric.listed_tags(&case, &record.resource_arn).await;
                assert_eq!(
                    listed,
                    expected_map(&carried),
                    "case {case:?}: expected the created map to be stored key for key, got {} entries",
                    listed.len()
                );
            }
        }
    }
}

/// The create path and the tagging surface are held to ONE bound, so no created record
/// is ever past it: a record created below the bound still takes a tag, and one created
/// at the bound refuses the next exactly as a record filled through `TagResource` does.
#[tokio::test]
async fn a_created_record_is_never_past_the_bound_the_tagging_surface_enforces() {
    for kind in EVERY_CREATE {
        for created in [TAGS_PER_RESOURCE_QUOTA - 1, TAGS_PER_RESOURCE_QUOTA] {
            let case = format!("{kind:?} created with {created} tags");
            let fabric = Fabric::new();
            let scaffold = fabric.scaffold(&case, kind).await;
            let carried = tag_rows(created, Shape::Plain);
            let answered = fabric.create(&scaffold, kind, &carried).await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected the create to be accepted, got {} {:?}",
                answered.status, answered.body
            );
            let record = minted(kind, &scaffold, &answered);

            let assigned = fabric
                .send(
                    "POST",
                    &tags_path(&record.resource_arn),
                    Some(json!({ "tags": { "assigned-later": "v" } })),
                )
                .await;
            let mut expected = expected_map(&carried);
            if created < TAGS_PER_RESOURCE_QUOTA {
                assert_eq!(
                    assigned.status, 204,
                    "case {case:?}: expected one more tag to fit, got {} {:?}",
                    assigned.status, assigned.body
                );
                expected.insert("assigned-later".to_string(), "v".to_string());
            } else {
                assert_over_bound(&case, &assigned);
            }
            let listed = fabric.listed_tags(&case, &record.resource_arn).await;
            assert_eq!(
                listed,
                expected,
                "case {case:?}: expected the record to hold {} tags afterwards, got {}",
                expected.len(),
                listed.len()
            );
        }
    }
}
