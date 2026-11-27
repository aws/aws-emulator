// SPDX-License-Identifier: CC0-1.0

//! The ONE `@idempotencyToken` replay decision every create of this service that
//! mints an id takes, driven end to end through the router the production emulator
//! uses.
//!
//! The rule under test is one rule over a set of creates rather than one recorded
//! request: a `clientToken` is scoped to the operation it was presented to, a token
//! presented again with the same parameters answers with the resource the first call
//! minted instead of minting a second one, the same token presented with DIFFERENT
//! parameters is refused with the modelled `ValidationException` (400) and stores
//! nothing, a fresh token carrying those other parameters is a request of its own,
//! and a create that would retain one more record than the store's bound allows is
//! refused with the modelled `ServiceQuotaExceededException` (402) while a replay of
//! a record already held is still answered.
//!
//! The table spans every create that keeps its record in the store's own
//! client-token map — the two gateway creates, the two external-link creates and the
//! routing-rule create — and every row presents the SAME token string, so a row that
//! is served fresh is what shows the token is scoped per operation. Each row varies
//! a parameter of its own (a description, a public endpoint, a log-sampling rate, a
//! priority), which is what the mismatch is read from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    idempotency_key, ComAmazonawsRtbfabricRtbfabricState, IdempotentCreate, IDEMPOTENCY_QUOTA,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The one token every row of the table presents to its own create: a token is
/// scoped to the operation it was presented to, so spending it on one create leaves
/// it unspent on the next.
const SHARED_TOKEN: &str = "a-token-every-create-is-given";

/// A token no call has presented, used to show that the other parameters a refused
/// retry carried are servable in a request of their own.
const FRESH_TOKEN: &str = "a-token-no-call-has-presented";

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// Everything an answer carries except the status: the members a replay repeats
/// unchanged, whether or not the clock has settled the record in between.
fn identity(body: &Value) -> Value {
    let mut identity = body.clone();
    if let Some(members) = identity.as_object_mut() {
        members.remove("status");
    }
    identity
}

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings(error_log: f64) -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": error_log, "filterLog": 0.0 } } })
}

/// A `CreateRequesterGateway` body whose members satisfy the modelled patterns.
fn requester_gateway_body() -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
    })
}

/// A `CreateResponderGateway` body whose members satisfy the modelled patterns, of
/// `gateway_type` when one is named.
fn responder_gateway_body(gateway_type: Option<&str>) -> Value {
    let mut body = json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "port": 443,
        "protocol": "HTTPS",
        "listenerConfig": { "protocols": ["HTTPS"] },
        "domainName": "responder.example.com",
    });
    if let Some(gateway_type) = gateway_type {
        body["gatewayType"] = json!(gateway_type);
    }
    body
}

/// One answered call: the HTTP status, the wire error code (absent on a success) and
/// the parsed body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

/// How much the store holds: the resources these creates mint, and the client-token
/// records retained beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Census {
    resources: usize,
    tokens: usize,
}

impl Census {
    /// The census with `resources` more resources and `tokens` more records.
    fn plus(self, resources: usize, tokens: usize) -> Self {
        Self {
            resources: self.resources + resources,
            tokens: self.tokens + tokens,
        }
    }
}

/// One row of the table: a create, the parameters a retry repeats, and the one
/// parameter of its own a varied retry differs in.
struct Row {
    case: &'static str,
    /// The operation name the refusal names, as the model spells it.
    operation: &'static str,
    path: String,
    /// The parameters the first call presents, without the `clientToken`.
    first: Value,
    /// The same call with one parameter of its own changed.
    varied: Value,
    /// The member the answer reports the minted id in.
    id_member: &'static str,
}

impl Row {
    /// `parameters` as a request body presenting `client_token`.
    fn body(parameters: &Value, client_token: &str) -> Value {
        let mut body = parameters.clone();
        body["clientToken"] = json!(client_token);
        body
    }
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate -> handle
/// -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
    /// The same shared engine the router hands every handler, so the test can count
    /// what the store holds without making a call of its own.
    engine: Arc<engine::InMemoryEngine>,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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

    /// Send one create and assert it was accepted.
    async fn create(&self, case: &str, path: &str, body: &Value) -> Answer {
        let answered = self.send("POST", path, Some(body.clone())).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected POST {path} to answer 200, got {} {:?}",
            answered.status, answered.body
        );
        answered
    }

    /// What the store holds. Taking the state lock is not a call into the service,
    /// so nothing here settles or releases a record.
    fn census(&self) -> Census {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let store = state.lock();
        Census {
            resources: store.requester_gateways.len()
                + store.responder_gateways.len()
                + store.outbound_external_links.len()
                + store.inbound_external_links.len()
                + store.routing_rules.len(),
            tokens: store.idempotency.len(),
        }
    }

    /// Retain client-token records, each pointing at `resource_id`, until the store
    /// holds its bound of them: what the create under test then asks for is one
    /// record more than the bound allows.
    fn fill_client_token_records(&self, resource_id: &str) {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let mut store = state.lock();
        let mut seeded = 0u64;
        while store.idempotency.len() < IDEMPOTENCY_QUOTA {
            seeded += 1;
            let key = idempotency_key("ARetainedRecord", &seeded.to_string());
            store.idempotency.insert(
                key,
                IdempotentCreate {
                    resource_id: resource_id.to_string(),
                    fingerprint: [0u8; 32],
                },
            );
        }
    }
}

/// The resources the rows of the table are created on.
struct Fixtures {
    requester_id: String,
    external_responder_id: String,
    responder_id: String,
    link_id: String,
}

/// Create the gateways and the link the table's rows hang off, each under a token of
/// its own.
async fn fixtures(fabric: &Fabric) -> Fixtures {
    let case = "the fixtures";
    let requester = fabric
        .create(
            case,
            "/requester-gateway",
            &Row::body(&requester_gateway_body(), "fixture-requester"),
        )
        .await;
    let external_responder = fabric
        .create(
            case,
            "/responder-gateway",
            &Row::body(
                &responder_gateway_body(Some("EXTERNAL")),
                "fixture-external-responder",
            ),
        )
        .await;
    let responder = fabric
        .create(
            case,
            "/responder-gateway",
            &Row::body(&responder_gateway_body(None), "fixture-responder"),
        )
        .await;
    let requester_id = text(&requester.body, "gatewayId");
    let responder_id = text(&responder.body, "gatewayId");
    let link = fabric
        .create(
            case,
            &format!("/gateway/{requester_id}/create-link"),
            &json!({ "peerGatewayId": responder_id, "logSettings": log_settings(100.0) }),
        )
        .await;
    Fixtures {
        requester_id,
        external_responder_id: text(&external_responder.body, "gatewayId"),
        responder_id,
        link_id: text(&link.body, "linkId"),
    }
}

/// Every create of this service that mints an id and keeps its client-token record
/// in the store's own map.
fn rows(fixtures: &Fixtures) -> Vec<Row> {
    let Fixtures {
        requester_id,
        external_responder_id,
        responder_id,
        link_id,
    } = fixtures;
    vec![
        Row {
            case: "a requester gateway",
            operation: "CreateRequesterGateway",
            path: "/requester-gateway".to_string(),
            first: requester_gateway_body(),
            varied: {
                let mut body = requester_gateway_body();
                body["description"] = json!("a description the first call did not carry");
                body
            },
            id_member: "gatewayId",
        },
        Row {
            case: "a responder gateway",
            operation: "CreateResponderGateway",
            path: "/responder-gateway".to_string(),
            first: responder_gateway_body(None),
            varied: {
                let mut body = responder_gateway_body(None);
                body["description"] = json!("a description the first call did not carry");
                body
            },
            id_member: "gatewayId",
        },
        Row {
            case: "an outbound external link",
            operation: "CreateOutboundExternalLink",
            path: format!("/requester-gateway/{requester_id}/outbound-external-link"),
            first: json!({
                "publicEndpoint": "https://bidder.example.com/rtb",
                "logSettings": log_settings(100.0),
            }),
            varied: json!({
                "publicEndpoint": "https://other-bidder.example.com/rtb",
                "logSettings": log_settings(100.0),
            }),
            id_member: "linkId",
        },
        Row {
            case: "an inbound external link",
            operation: "CreateInboundExternalLink",
            path: format!("/responder-gateway/{external_responder_id}/inbound-external-link"),
            first: json!({ "logSettings": log_settings(100.0) }),
            varied: json!({ "logSettings": log_settings(50.0) }),
            id_member: "linkId",
        },
        Row {
            case: "a link routing rule",
            operation: "CreateLinkRoutingRule",
            path: format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule"),
            first: json!({ "priority": 7, "conditions": { "pathPrefix": "/bid" } }),
            varied: json!({ "priority": 8, "conditions": { "pathPrefix": "/bid" } }),
            id_member: "ruleId",
        },
    ]
}

/// Every create that mints an id answers a repeated `clientToken` from the record it
/// retained, refuses the same token carrying other parameters, and serves those
/// other parameters under a token of their own.
#[tokio::test]
async fn a_repeated_client_token_replays_its_create_and_a_changed_one_is_refused() {
    let fabric = Fabric::new();
    let fixtures = fixtures(&fabric).await;
    let mut minted: Vec<String> = Vec::new();

    for row in rows(&fixtures) {
        let case = row.case;

        // The token is scoped to the operation it was presented to, so the row is
        // served as a request of its own even though every earlier row spent the
        // same token on a create of its own.
        let before = fabric.census();
        let created = fabric
            .create(case, &row.path, &Row::body(&row.first, SHARED_TOKEN))
            .await;
        let id = text(&created.body, row.id_member);
        assert!(
            !id.is_empty(),
            "case {case:?}: expected the create to report the id it minted, got {:?}",
            created.body
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a create under a token spent on another operation to mint an id of its own, got {id:?} among {minted:?}"
        );
        assert_eq!(
            fabric.census(),
            before.plus(1, 1),
            "case {case:?}: expected the create to store one resource and retain one record"
        );
        minted.push(id.clone());

        // The same token with the same parameters: the stored resource, not a second
        // one, and nothing added to the store.
        let replayed = fabric
            .create(case, &row.path, &Row::body(&row.first, SHARED_TOKEN))
            .await;
        assert_eq!(
            identity(&replayed.body),
            identity(&created.body),
            "case {case:?}: expected the replay to answer for the resource the first call minted, got {:?} after {:?}",
            replayed.body,
            created.body
        );
        assert_eq!(
            fabric.census(),
            before.plus(1, 1),
            "case {case:?}: expected the replay to store nothing further"
        );

        // The same token with other parameters: refused, in the one wording every
        // surface of this service words it in, and nothing stored.
        let refused = fabric
            .send("POST", &row.path, Some(Row::body(&row.varied, SHARED_TOKEN)))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a token replayed with other parameters to be refused with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code, "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {:?}",
            refused.code
        );
        assert_eq!(
            text(&refused.body, "message"),
            format!(
                "clientToken {SHARED_TOKEN} was already used for a {} request with different parameters",
                row.operation
            ),
            "case {case:?}: unexpected refusal wording, got {:?}",
            refused.body
        );
        assert_eq!(
            fabric.census(),
            before.plus(1, 1),
            "case {case:?}: expected the refused retry to store nothing"
        );

        // Those same other parameters under a token of their own are a request of
        // their own: the record is keyed by the token, not by the parameters.
        let varied = fabric
            .create(case, &row.path, &Row::body(&row.varied, FRESH_TOKEN))
            .await;
        let varied_id = text(&varied.body, row.id_member);
        assert_ne!(
            varied_id, id,
            "case {case:?}: expected a fresh token to mint a resource of its own, got {varied_id:?} twice"
        );
        assert_eq!(
            fabric.census(),
            before.plus(2, 2),
            "case {case:?}: expected the fresh token to store a second resource and retain a second record"
        );
    }
}

/// At the store's bound on retained client-token records, every create that would
/// retain one more is refused with the modelled `ServiceQuotaExceededException`,
/// while a replay of a record the store already holds is still answered.
#[tokio::test]
async fn a_create_that_would_retain_one_record_past_the_bound_is_refused() {
    let fabric = Fabric::new();
    let fixtures = fixtures(&fabric).await;
    let rows = rows(&fixtures);

    // One resource per row, retained under a token the store holds, created while
    // there is still room for a record.
    for row in &rows {
        fabric
            .create(row.case, &row.path, &Row::body(&row.first, SHARED_TOKEN))
            .await;
    }
    fabric.fill_client_token_records(&fixtures.requester_id);
    let at_the_bound = fabric.census();

    for row in &rows {
        let case = row.case;
        let refused = fabric
            .send("POST", &row.path, Some(Row::body(&row.varied, FRESH_TOKEN)))
            .await;
        assert_eq!(
            refused.status, 402,
            "case {case:?}: expected a create past the bound of {IDEMPOTENCY_QUOTA} retained records to be refused with 402, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code, "ServiceQuotaExceededException",
            "case {case:?}: expected the modelled ServiceQuotaExceededException, got {:?}",
            refused.code
        );
        assert_eq!(
            text(&refused.body, "message"),
            format!("this emulator holds its limit of {IDEMPOTENCY_QUOTA} client-token records"),
            "case {case:?}: unexpected refusal wording, got {:?}",
            refused.body
        );
        assert_eq!(
            fabric.census(),
            at_the_bound,
            "case {case:?}: expected the refused create to store nothing"
        );

        // The bound refuses a record the store does not hold, not a replay of one it
        // does.
        let replayed = fabric
            .create(case, &row.path, &Row::body(&row.first, SHARED_TOKEN))
            .await;
        assert!(
            !text(&replayed.body, row.id_member).is_empty(),
            "case {case:?}: expected the replay at the bound to answer for the stored resource, got {:?}",
            replayed.body
        );
        assert_eq!(
            fabric.census(),
            at_the_bound,
            "case {case:?}: expected the replay at the bound to store nothing further"
        );
    }
}
