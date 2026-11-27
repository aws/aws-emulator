// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH Region a tagging ARN has to name: the Region the
//! resource it addresses was created in, never the Region the calling request was
//! signed for.
//!
//! A resource of this service lives in one Region — the one the call that created it
//! addressed — and `ListTagsForResource`, `TagResource` and `UntagResource` resolve a
//! `resourceArn` against THAT Region, so one record answers the same whoever asks
//! about it, exactly as the resource's own read does. An ARN whose Region field names
//! any other Region names a resource this store does not hold, and is refused alike
//! by all three, whichever Region the caller signed for.
//!
//! The table spans what the rule generalises over: the Region the resource was
//! created in (several values, including one carrying `_` — admitted by the modelled
//! ARN Region field but not by a DNS label), which kind of resource the ARN names (a
//! gateway of either kind, a link between two gateways, an inbound external link),
//! the Region the tagging call is signed for (the resource's own and another), and
//! the Region the ARN spells (the resource's own and another). The sub-resources are
//! created from a Region OTHER than their gateway's, so a record that took its
//! Region from the creating call rather than from its gateway is caught too.
//!
//! The last suite pins the other half: a Region is spelled verbatim in an ARN, while
//! every DNS name this service assigns stays inside the modelled `DomainName`
//! grammar, and an inbound external link's name is built under the inbound name of
//! the gateway it was created on rather than under the caller's Region.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// Whether a value is a `DomainName` the model admits, read through this crate's one
// reading of that pattern (`smithy_query(query="shape", shape="DomainName")`), which
// the lifecycle suite owns: lowercase alphanumerics and `-` per label, never a
// leading or trailing `-`, two or more labels.
use crate::gateway_link_lifecycle::is_domain_name as is_modelled_domain_name;

/// The Regions this suite creates resources in.
///
/// Several values, because the rule is about which Region a RECORD belongs to rather
/// than about any one Region. The last carries `_`: the modelled
/// `RtbTaggableResourceArn` pattern admits it in the Region field
/// (`^arn:aws:rtbfabric:[a-zA-Z0-9_-]+:…`, from `smithy_query(query="shape",
/// shape="RtbTaggableResourceArn")`) while a label of the modelled `DomainName`
/// (`[a-z0-9-]`) does not, so a Region narrowed to a DNS label would be one no ARN of
/// that resource could ever spell.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2", "us-east-1_alt"];

/// The answer a tagging call naming no stored resource carries: the status, the wire
/// error code and the whole message, written out here rather than read back from the
/// crate under test, so a handler that moved to another refusal fails this suite.
const UNRESOLVED_STATUS: u16 = 500;
const UNRESOLVED_CODE: &str = "InternalServerErrorException";
const UNRESOLVED_MESSAGE: &str = "Internal server error.";

/// One answered call: the status, the wire error code (absent on a success) and the
/// parsed body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate -> handle ->
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

    /// Send one restJson1 request whose SigV4 credential scope names `region`, and
    /// return what came back. The scope Region is the only thing that varies between
    /// two otherwise identical calls of this suite.
    async fn send(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Answer {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            // One endpoint for every call: the service is routed by the credential
            // scope's service field, so the Region under test travels in the scope
            // rather than in the host.
            .uri(format!("https://rtbfabric.us-east-1.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/rtbfabric/aws4_request, \
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

    /// Create a requester gateway from a call signed for `region`, and answer the
    /// whole response body.
    async fn requester_gateway(&self, case: &str, region: &str) -> Value {
        let answered = self
            .send(
                region,
                "POST",
                "/requester-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": "token-requester",
                })),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected CreateRequesterGateway signed for {region:?} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered.body
    }

    /// Create an `EXTERNAL` responder gateway from a call signed for `region`, and
    /// answer the whole response body. `EXTERNAL`, because this suite also addresses
    /// the inbound external links only that gateway type takes.
    async fn responder_gateway(&self, case: &str, region: &str) -> Value {
        let answered = self
            .send(
                region,
                "POST",
                "/responder-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 443,
                    "protocol": "HTTPS",
                    "domainName": "responder.example.com",
                    "gatewayType": "EXTERNAL",
                    "clientToken": "token-responder",
                })),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected CreateResponderGateway signed for {region:?} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered.body
    }

    /// Open a link between two stored gateways from a call signed for `region`, and
    /// answer the id the service minted.
    async fn link(&self, case: &str, region: &str, gateway_id: &str, peer_id: &str) -> String {
        let answered = self
            .send(
                region,
                "POST",
                &format!("/gateway/{gateway_id}/create-link"),
                Some(json!({
                    "peerGatewayId": peer_id,
                    "logSettings": { "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } },
                })),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected CreateLink signed for {region:?} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        text(&answered.body, "linkId")
    }

    /// Create an inbound external link on a stored responder gateway from a call
    /// signed for `region`, and answer the whole response body.
    async fn inbound_external_link(&self, case: &str, region: &str, gateway_id: &str) -> Value {
        let answered = self
            .send(
                region,
                "POST",
                &format!("/responder-gateway/{gateway_id}/inbound-external-link"),
                Some(json!({
                    "clientToken": "token-inbound-external-link",
                    "logSettings": { "applicationLogs": { "sampling": { "errorLog": 0.0, "filterLog": 100.0 } } },
                })),
            )
            .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected CreateInboundExternalLink signed for {region:?} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered.body
    }
}

/// Percent-encode a value the way an AWS SDK serializes a non-greedy `@httpLabel` or
/// an `@httpQuery` value, so an ARN's `:` and `/` arrive as ONE path segment.
fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// The `/tags/{resourceArn}` path for `resource_arn`.
fn tags_path(resource_arn: &str) -> String {
    format!("/tags/{}", encoded(resource_arn))
}

/// The `UntagResource` path: the resource in the label, the keys in the repeated
/// `tagKeys` query parameter the model binds.
fn untag_path(resource_arn: &str, tag_keys: &[&str]) -> String {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect();
    format!("{}?{}", tags_path(resource_arn), query.join("&"))
}

/// The ARN of a resource of this service in `region`, in the account this emulator
/// serves.
fn resource_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn("rtbfabric", region, resource)
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// A `tags` member as a sorted map, so two tag sets compare independently of the
/// order they were written in. A record holding no tag reports the empty map.
fn tags_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    match tags {
        Value::Null => BTreeMap::new(),
        Value::Object(object) => object
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
            .collect(),
        other => panic!("case {case:?}: expected a tags map, got {other:?}"),
    }
}

/// The Region after `region` in [`REGIONS`], wrapping: the "another Region" of every
/// row, so no row has to name one itself.
fn another_region(region: &str) -> &'static str {
    let at = REGIONS
        .iter()
        .position(|candidate| *candidate == region)
        .unwrap_or_default();
    REGIONS[(at + 1) % REGIONS.len()]
}

/// Which kind of resource the row's ARN names.
#[derive(Debug, Clone, Copy)]
enum Target {
    RequesterGateway,
    ResponderGateway,
    Link,
    InboundExternalLink,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::RequesterGateway => "a requester gateway",
            Target::ResponderGateway => "a responder gateway",
            Target::Link => "a link between two gateways",
            Target::InboundExternalLink => "an inbound external link",
        }
    }
}

/// Create the row's resource in `region` and answer the ARN resource field that
/// names it, plus the path the resource's OWN read answers on.
///
/// Every sub-resource is created from a call signed for ANOTHER Region than its
/// gateway's, so a record taking its Region from the creating call instead of from
/// its gateway cannot pass.
async fn provision(fabric: &Fabric, case: &str, region: &str, target: Target) -> (String, String) {
    let elsewhere = another_region(region);
    match target {
        Target::RequesterGateway => {
            let gateway_id = text(&fabric.requester_gateway(case, region).await, "gatewayId");
            (
                format!("gateway/{gateway_id}"),
                format!("/requester-gateway/{gateway_id}"),
            )
        }
        Target::ResponderGateway => {
            let gateway_id = text(&fabric.responder_gateway(case, region).await, "gatewayId");
            (
                format!("gateway/{gateway_id}"),
                format!("/responder-gateway/{gateway_id}"),
            )
        }
        Target::Link => {
            let requester_id = text(&fabric.requester_gateway(case, region).await, "gatewayId");
            let responder_id = text(&fabric.responder_gateway(case, region).await, "gatewayId");
            let link_id = fabric
                .link(case, elsewhere, &requester_id, &responder_id)
                .await;
            (
                format!("gateway/{requester_id}/link/{link_id}"),
                format!("/gateway/{requester_id}/link/{link_id}"),
            )
        }
        Target::InboundExternalLink => {
            let gateway_id = text(&fabric.responder_gateway(case, region).await, "gatewayId");
            let link_id = text(
                &fabric
                    .inbound_external_link(case, elsewhere, &gateway_id)
                    .await,
                "linkId",
            );
            (
                format!("gateway/{gateway_id}/link/{link_id}"),
                format!("/responder-gateway/{gateway_id}/inbound-external-link/{link_id}"),
            )
        }
    }
}

/// Assert `answered` is the one refusal the surface gives an ARN naming no record
/// this store holds.
fn assert_unresolved(case: &str, operation: &str, answered: &Answer) {
    assert_eq!(
        answered.status, UNRESOLVED_STATUS,
        "case {case:?}: expected {operation} to answer {UNRESOLVED_STATUS}, got {} {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.code, UNRESOLVED_CODE,
        "case {case:?}: expected {operation} to answer the code {UNRESOLVED_CODE}, got {:?}",
        answered.code
    );
    assert_eq!(
        text(&answered.body, "message"),
        UNRESOLVED_MESSAGE,
        "case {case:?}: expected {operation} to state {UNRESOLVED_MESSAGE:?}, got {:?}",
        answered.body
    );
}

#[tokio::test]
async fn the_tagging_surface_resolves_an_arn_by_the_resources_own_region() {
    for region in REGIONS {
        for target in [
            Target::RequesterGateway,
            Target::ResponderGateway,
            Target::Link,
            Target::InboundExternalLink,
        ] {
            let case = format!("{} created in {region:?}", target.name());
            let case = case.as_str();
            let fabric = Fabric::new();
            let (resource, reader_path) = provision(&fabric, case, region, target).await;
            let own_arn = resource_arn(region, &resource);
            let elsewhere = another_region(region);
            let other_arn = resource_arn(elsewhere, &resource);

            // Signed for the resource's own Region, and signed for another one: the
            // Region the CALLER addressed decides nothing here.
            for signed_for in [*region, elsewhere] {
                // The ARN naming the Region the resource was created in resolves,
                // and the write lands on the resource's own record.
                let assigned = fabric
                    .send(
                        signed_for,
                        "POST",
                        &tags_path(&own_arn),
                        Some(json!({ "tags": { "owner": "ads" } })),
                    )
                    .await;
                assert_eq!(
                    assigned.status, 204,
                    "case {case:?}: expected TagResource signed for {signed_for:?} on the ARN naming {region:?} to answer 204, got {} {:?}",
                    assigned.status, assigned.body
                );
                let listed = fabric
                    .send(signed_for, "GET", &tags_path(&own_arn), None)
                    .await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected ListTagsForResource signed for {signed_for:?} on the ARN naming {region:?} to answer 200, got {} {:?}",
                    listed.status, listed.body
                );
                assert_eq!(
                    tags_map(case, &listed.body["tags"]),
                    BTreeMap::from([("owner".to_string(), "ads".to_string())]),
                    "case {case:?}: expected the listing signed for {signed_for:?} to report the assigned tag, got {:?}",
                    listed.body["tags"]
                );
                let read = fabric.send(signed_for, "GET", &reader_path, None).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected the resource at {reader_path:?} to read 200, got {} {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    tags_map(case, &read.body["tags"]),
                    BTreeMap::from([("owner".to_string(), "ads".to_string())]),
                    "case {case:?}: expected the resource's own reader to report the assigned tag, got {:?}",
                    read.body["tags"]
                );

                // An ARN naming any other Region names no record this store holds,
                // whichever Region the call is signed for — including the caller's
                // own, which is what a check against the credential scope would let
                // through.
                for (operation, method, path, body) in [
                    (
                        "ListTagsForResource",
                        "GET",
                        tags_path(&other_arn),
                        None,
                    ),
                    (
                        "TagResource",
                        "POST",
                        tags_path(&other_arn),
                        Some(json!({ "tags": { "stage": "beta" } })),
                    ),
                    (
                        "UntagResource",
                        "DELETE",
                        untag_path(&other_arn, &["owner"]),
                        None,
                    ),
                ] {
                    let answered = fabric.send(signed_for, method, &path, body).await;
                    assert_unresolved(
                        &format!(
                            "{case}, {operation} signed for {signed_for:?} on the ARN naming {elsewhere:?}"
                        ),
                        operation,
                        &answered,
                    );
                }

                // Those refusals wrote nothing and removed nothing: the record still
                // holds exactly the tag the resolved call assigned.
                let listed = fabric
                    .send(signed_for, "GET", &tags_path(&own_arn), None)
                    .await;
                assert_eq!(
                    tags_map(case, &listed.body["tags"]),
                    BTreeMap::from([("owner".to_string(), "ads".to_string())]),
                    "case {case:?}: expected the refused calls to have changed no tag, got {:?}",
                    listed.body["tags"]
                );

                // And the removal through the resource's own ARN takes the tag off,
                // so the next round of this loop starts from an untagged record.
                let removed = fabric
                    .send(signed_for, "DELETE", &untag_path(&own_arn, &["owner"]), None)
                    .await;
                assert_eq!(
                    removed.status, 204,
                    "case {case:?}: expected UntagResource signed for {signed_for:?} on the ARN naming {region:?} to answer 204, got {} {:?}",
                    removed.status, removed.body
                );
                let listed = fabric
                    .send(signed_for, "GET", &tags_path(&own_arn), None)
                    .await;
                assert!(
                    tags_map(case, &listed.body["tags"]).is_empty(),
                    "case {case:?}: expected the removal to leave no tag, got {:?}",
                    listed.body["tags"]
                );
            }
        }
    }
}

#[tokio::test]
async fn a_region_an_arn_can_spell_but_a_dns_label_cannot_still_names_its_resources() {
    for region in REGIONS {
        let case = format!("resources created in {region:?}");
        let case = case.as_str();
        let fabric = Fabric::new();
        let elsewhere = another_region(region);

        // The Region reaches a DNS name only through the narrower label grammar, so
        // every name this service assigns is one the model admits however the Region
        // is spelled.
        let requester = fabric.requester_gateway(case, region).await;
        let domain_name = text(&requester, "domainName");
        assert!(
            is_modelled_domain_name(&domain_name),
            "case {case:?}: expected the gateway's domainName to be a modelled DomainName, got {domain_name:?}"
        );
        let responder = fabric.responder_gateway(case, region).await;
        let endpoint = text(&responder, "externalInboundEndpoint");
        assert!(
            is_modelled_domain_name(&endpoint),
            "case {case:?}: expected the gateway's externalInboundEndpoint to be a modelled DomainName, got {endpoint:?}"
        );

        // An inbound external link is reached under the inbound name of the gateway
        // it was created on, so the Region that name carries is the GATEWAY's — this
        // link is created from another Region entirely.
        let responder_id = text(&responder, "gatewayId");
        let link = fabric
            .inbound_external_link(case, elsewhere, &responder_id)
            .await;
        let link_id = text(&link, "linkId");
        let link_domain_name = text(&link, "domainName");
        assert_eq!(
            link_domain_name,
            format!("{link_id}.{endpoint}"),
            "case {case:?}: expected the link's domainName to sit under the gateway's own inbound endpoint {endpoint:?}, got {link_domain_name:?}"
        );

        // And the Region itself is spelled in an ARN exactly as the credential scope
        // named it: the ARN naming the resource's Region resolves, while the one
        // carrying the Region reduced to a DNS label — all that a narrower reading
        // would leave of it — names nothing.
        let own_arn = resource_arn(region, &format!("gateway/{responder_id}"));
        let listed = fabric.send(elsewhere, "GET", &tags_path(&own_arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource on the ARN naming {region:?} to answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let dns_spelling: String = region
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if dns_spelling != *region {
            let narrowed = resource_arn(&dns_spelling, &format!("gateway/{responder_id}"));
            let answered = fabric.send(elsewhere, "GET", &tags_path(&narrowed), None).await;
            assert_unresolved(
                &format!("{case}, the ARN naming {dns_spelling:?}"),
                "ListTagsForResource",
                &answered,
            );
        }
    }
}
