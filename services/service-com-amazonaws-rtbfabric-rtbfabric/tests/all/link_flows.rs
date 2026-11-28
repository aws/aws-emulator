// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `PutLinkFlows` and the concurrency precondition the link
//! readers hand out.
//!
//! The rules under test are about state, not about one recorded request. The
//! flows a call states are the WHOLE configuration — they replace whatever
//! configuration the link had taken, and an empty list removes every flow — and
//! the change applies asynchronously: the call is accepted, the link keeps
//! serving its prior flows, and the change commits as the link reaches service,
//! which is when a reader reports it. Every kind of link this service holds takes
//! a configuration the same way, since a link is addressed through the gateway it
//! is on. The `lockToken` a read hands out is the precondition the write presents
//! back: it is the same across reads that changed nothing, it goes stale when a
//! change is accepted by any operation, and a call presenting one that is not the
//! link's current one is refused and changes nothing. A retry under one
//! `clientToken` reports the update the first call started rather than starting a
//! second, and the same token stating other flows is refused.
//!
//! The tables span what those rules generalise over: the shape of the pipeline a
//! flow states (sequential, phased, several modules) and the empty
//! configuration, each of the three kinds of link, each way a presented
//! precondition can fail to be the current one, and a link in a status nothing
//! moves it out of.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    listed_modules, ComAmazonawsRtbfabricRtbfabricState, LINK_STATUS_ACTIVE,
    LINK_STATUS_PENDING_UPDATE, RESOURCE_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// How long a poll waits for a status the clock owes before it gives up. Far
/// above any span this service owes a transition, so a slow or loaded host
/// changes how many times the loop looks, never whether it succeeds.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// The ARN of an attachable version of a module this emulator's catalog holds,
/// which is what a flow's module names. Read out of the catalog the module reads
/// answer from, so a flow states a version that really exists.
fn attachable_module_version_arn(at: usize) -> String {
    let modules = listed_modules();
    let module = modules[at % modules.len()];
    let version = module
        .listed_versions()
        .into_iter()
        .next()
        .expect("a catalog module has a listed version");
    format!(
        "arn:aws:rtbfabric:{REGION}::modules/{}/versions/{}",
        module.module_id, version.version
    )
}

/// One module of a flow's pipeline.
fn flow_module(name: &str, at: usize) -> Value {
    json!({
        "name": name,
        "moduleVersionArn": attachable_module_version_arn(at),
        "selectedActions": ["OBSERVE"],
        "failureMode": "OPEN",
    })
}

/// A flow whose modules run in list order.
fn sequential_flow(name: &str, modules: &[&str]) -> Value {
    let modules: Vec<Value> = modules
        .iter()
        .enumerate()
        .map(|(at, module)| flow_module(module, at))
        .collect();
    json!({ "name": name, "pipeline": { "sequential": { "modules": modules } } })
}

/// A flow whose phases run in order, each phase's modules concurrently.
fn phased_flow(name: &str, phases: &[&[&str]]) -> Value {
    let phases: Vec<Value> = phases
        .iter()
        .enumerate()
        .map(|(phase, modules)| {
            let modules: Vec<Value> = modules
                .iter()
                .map(|module| flow_module(module, phase))
                .collect();
            json!({ "modules": modules })
        })
        .collect();
    json!({ "name": name, "pipeline": { "phased": { "phases": phases } } })
}

/// The names of the flows a reading reports, and for each the names of the
/// modules its pipeline holds, in the order they are reported.
type ReportedFlows = Vec<(String, Vec<String>)>;

/// One row of the configuration table: what the call states, and the flows a
/// reader reports once the change has committed.
type FlowRow = (&'static str, Vec<Value>, ReportedFlows);

/// The flows a reading reports, as [`ReportedFlows`] holds them.
fn reported_flows(body: &Value) -> ReportedFlows {
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| {
            let pipeline = &flow["pipeline"];
            let mut modules: Vec<String> = Vec::new();
            if let Some(stated) = pipeline["sequential"]["modules"].as_array() {
                modules.extend(stated.iter().map(|module| text(module, "name")));
            }
            for phase in pipeline["phased"]["phases"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                if let Some(stated) = phase["modules"].as_array() {
                    modules.extend(stated.iter().map(|module| text(module, "name")));
                }
            }
            (text(flow, "name"), modules)
        })
        .collect()
}

/// The one emulated service, behind the same `MultiServiceRouter` the production
/// router assembles, so these calls take the real deserialize -> validate ->
/// handle -> serialize path.
struct Fabric {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one restJson1 request and return `(status, the error code, parsed
    /// body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String, Value) {
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
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, code, parsed)
    }

    /// A requester gateway and a responder gateway that takes links from outside
    /// the fabric, both over the wire, as `(the requester's id, the responder's
    /// id)`.
    async fn gateway_pair(&self, token: &str) -> (String, String) {
        let (status, _, requester) = self
            .send(
                "POST",
                "/requester-gateway",
                Some(json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": format!("{token}-requester"),
                })),
            )
            .await;
        assert_eq!(status, 200, "CreateRequesterGateway: {status} {requester:?}");
        let (status, _, responder) = self
            .send(
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
                    "clientToken": format!("{token}-responder"),
                })),
            )
            .await;
        assert_eq!(status, 200, "CreateResponderGateway: {status} {responder:?}");
        (text(&requester, "gatewayId"), text(&responder, "gatewayId"))
    }

    /// A link between a fresh gateway pair, brought up: the peer accepts the
    /// request and the clock then makes `ACTIVE` due.
    async fn active_link(&self, token: &str) -> (String, String) {
        let (requester_id, responder_id) = self.gateway_pair(token).await;
        let (status, _, link) = self
            .send(
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                Some(json!({
                    "peerGatewayId": responder_id,
                    "logSettings": log_settings(),
                })),
            )
            .await;
        assert_eq!(status, 200, "CreateLink: {status} {link:?}");
        let link_id = text(&link, "linkId");
        let (status, _, accepted) = self
            .send(
                "POST",
                &format!("/gateway/{responder_id}/link/{link_id}/accept"),
                Some(json!({ "logSettings": log_settings() })),
            )
            .await;
        assert_eq!(status, 200, "AcceptLink: {status} {accepted:?}");
        self.poll_status(
            &format!("/gateway/{requester_id}/link/{link_id}"),
            LINK_STATUS_ACTIVE,
        )
        .await;
        (requester_id, link_id)
    }

    /// Read a link's reading, asserting the read itself answered.
    async fn read(&self, case: &str, path: &str) -> Value {
        let (status, _, body) = self.send("GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of {path:?} to answer 200, got {status} {body:?}"
        );
        body
    }

    /// Read a link until it reports `expected`, and answer with that reading.
    ///
    /// A settled status is owed by the clock, so the test waits for the
    /// CONDITION rather than for a span: it looks again until the status holds,
    /// giving up only once [`POLL_DEADLINE`] has passed.
    async fn poll_status(&self, path: &str, expected: &str) -> Value {
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let (status, _, body) = self.send("GET", path, None).await;
            assert_eq!(
                status, 200,
                "expected a read of {path:?} to answer 200, got {status} {body:?}"
            );
            if text(&body, "status") == expected {
                return body;
            }
            assert!(
                Instant::now() < deadline,
                "expected {path:?} to reach {expected:?}, got {:?}",
                body["status"]
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Call `PutLinkFlows` over the wire.
    async fn put_flows(
        &self,
        gateway_id: &str,
        link_id: &str,
        lock_token: &str,
        flows: &[Value],
        client_token: Option<&str>,
    ) -> (u16, String, Value) {
        let mut body = json!({ "lockToken": lock_token, "flows": flows });
        if let Some(client_token) = client_token {
            body["clientToken"] = json!(client_token);
        }
        self.send(
            "PUT",
            &format!("/gateway/{gateway_id}/link/{link_id}/flows"),
            Some(body),
        )
        .await
    }

    /// Put `status` on every stored link between two gateways.
    ///
    /// The statuses a link reaches beyond its own request and acceptance are
    /// driven by operations this crate has not implemented yet, so the state is
    /// set directly to reach a status nothing moves the link out of.
    fn force_link_status(&self, status: &str) {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let mut store = state.lock();
        for link in store.links.values_mut() {
            link.status = status.to_string();
            link.settles_at = None;
        }
    }

    /// How many updates the store holds in all.
    fn updates_held(&self) -> usize {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let store = state.lock();
        store.updates.len()
    }
}

/// A flow configuration is accepted, the link serves its prior flows until the
/// change commits, and the commit is what a reader reports — whatever shape the
/// configuration has, the empty one included.
#[tokio::test]
async fn a_flow_configuration_commits_as_the_link_reaches_service() {
    let fabric = Fabric::new();
    let (gateway_id, link_id) = fabric.active_link("flow-commit").await;
    let path = format!("/gateway/{gateway_id}/link/{link_id}");

    // Each row is the configuration a call states and the flows a reader reports
    // once the change has committed. The rows differ in the shape of the
    // pipeline, in how many modules it holds and in whether any flow is stated
    // at all.
    let rows: Vec<FlowRow> = vec![
        (
            "a sequential pipeline of one module",
            vec![sequential_flow("Main", &["Observe"])],
            vec![("Main".to_string(), vec!["Observe".to_string()])],
        ),
        (
            "a sequential pipeline of several modules, in the order stated",
            vec![sequential_flow("Main", &["First", "Second", "Third"])],
            vec![(
                "Main".to_string(),
                vec![
                    "First".to_string(),
                    "Second".to_string(),
                    "Third".to_string(),
                ],
            )],
        ),
        (
            "a phased pipeline",
            vec![phased_flow("Main", &[&["Early"], &["Late", "Later"]])],
            vec![(
                "Main".to_string(),
                vec![
                    "Early".to_string(),
                    "Late".to_string(),
                    "Later".to_string(),
                ],
            )],
        ),
        (
            "another flow name, replacing the configuration in service",
            vec![sequential_flow("Replacement", &["Observe"])],
            vec![("Replacement".to_string(), vec!["Observe".to_string()])],
        ),
        ("no flow at all", Vec::new(), Vec::new()),
    ];

    for (case, stated, expected) in rows {
        let before = fabric.read(case, &path).await;
        let lock_token = text(&before, "lockToken");
        assert!(
            !lock_token.is_empty(),
            "case {case:?}: expected a read to hand out a lockToken, got {before:?}"
        );
        assert_eq!(
            lock_token,
            text(&fabric.read(case, &path).await, "lockToken"),
            "case {case:?}: expected a read that changed nothing to hand out the same token"
        );
        let served_before = reported_flows(&before);

        let (status, _, accepted) = fabric
            .put_flows(&gateway_id, &link_id, &lock_token, &stated, None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutLinkFlows to answer 200, got {status} {accepted:?}"
        );
        assert_eq!(
            text(&accepted, "status"),
            LINK_STATUS_PENDING_UPDATE,
            "case {case:?}: expected a live link to report the change as pending, got {accepted:?}"
        );
        let update_id = text(&accepted, "updateId");
        assert!(
            update_id.starts_with("update-"),
            "case {case:?}: expected the modelled UpdateId spelling, got {update_id:?}"
        );

        let pending = fabric.read(case, &path).await;
        assert_eq!(
            reported_flows(&pending),
            served_before,
            "case {case:?}: expected the link to serve its prior flows until the change commits"
        );
        assert_ne!(
            text(&pending, "lockToken"),
            lock_token,
            "case {case:?}: expected the accepted change to make the presented token stale"
        );

        let committed = fabric.poll_status(&path, LINK_STATUS_ACTIVE).await;
        assert_eq!(
            reported_flows(&committed),
            expected,
            "case {case:?}: expected the committed configuration to be the one served, got {:?}",
            committed["flows"]
        );
    }
}

/// Every kind of link this service holds takes a flow configuration the same
/// way, since a link is addressed through the gateway it is on — and each kind's
/// own reader reports both the configuration and the precondition.
#[tokio::test]
async fn every_kind_of_link_takes_a_flow_configuration() {
    let fabric = Fabric::new();
    let (requester_id, responder_id) = fabric.gateway_pair("flow-kinds").await;

    // A link between two gateways, brought up.
    let (status, _, link) = fabric
        .send(
            "POST",
            &format!("/gateway/{requester_id}/create-link"),
            Some(json!({ "peerGatewayId": responder_id, "logSettings": log_settings() })),
        )
        .await;
    assert_eq!(status, 200, "CreateLink: {status} {link:?}");
    let between_id = text(&link, "linkId");
    let (status, _, accepted) = fabric
        .send(
            "POST",
            &format!("/gateway/{responder_id}/link/{between_id}/accept"),
            Some(json!({ "logSettings": log_settings() })),
        )
        .await;
    assert_eq!(status, 200, "AcceptLink: {status} {accepted:?}");

    // A link with one end outside the fabric, in each direction.
    let (status, _, inbound) = fabric
        .send(
            "POST",
            &format!("/responder-gateway/{responder_id}/inbound-external-link"),
            Some(json!({ "logSettings": log_settings(), "clientToken": "flow-kinds-inbound" })),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateInboundExternalLink: {status} {inbound:?}"
    );
    let inbound_id = text(&inbound, "linkId");
    let (status, _, outbound) = fabric
        .send(
            "POST",
            &format!("/requester-gateway/{requester_id}/outbound-external-link"),
            Some(json!({
                "publicEndpoint": "https://exchange.example.com",
                "logSettings": log_settings(),
                "clientToken": "flow-kinds-outbound",
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateOutboundExternalLink: {status} {outbound:?}"
    );
    let outbound_id = text(&outbound, "linkId");

    // Each row is a kind of link: the gateway the flows call addresses it
    // through, its id, and the path its own reader answers on.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a link between two gateways",
            requester_id.as_str(),
            between_id.as_str(),
            format!("/gateway/{requester_id}/link/{between_id}"),
        ),
        (
            "a link whose traffic arrives from outside the fabric",
            responder_id.as_str(),
            inbound_id.as_str(),
            format!("/responder-gateway/{responder_id}/inbound-external-link/{inbound_id}"),
        ),
        (
            "a link whose traffic leaves the fabric",
            requester_id.as_str(),
            outbound_id.as_str(),
            format!("/requester-gateway/{requester_id}/outbound-external-link/{outbound_id}"),
        ),
    ];

    for (case, gateway_id, link_id, read_path) in rows {
        // The link is up, so a change to its flows is a change under live
        // traffic.
        let up = fabric.poll_status(&read_path, LINK_STATUS_ACTIVE).await;
        let lock_token = text(&up, "lockToken");
        assert!(
            !lock_token.is_empty(),
            "case {case:?}: expected every link reader to hand out a lockToken, got {up:?}"
        );
        assert!(
            reported_flows(&up).is_empty(),
            "case {case:?}: expected a link nothing configured to report no flows, got {up:?}"
        );

        let stated = vec![sequential_flow("Main", &["Observe"])];
        let (status, _, answered) = fabric
            .put_flows(gateway_id, link_id, &lock_token, &stated, None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutLinkFlows to answer 200, got {status} {answered:?}"
        );
        assert_eq!(
            text(&answered, "linkId"),
            link_id,
            "case {case:?}: expected the answer to name the link addressed, got {answered:?}"
        );

        let committed = fabric.poll_status(&read_path, LINK_STATUS_ACTIVE).await;
        assert_eq!(
            reported_flows(&committed),
            vec![("Main".to_string(), vec!["Observe".to_string()])],
            "case {case:?}: expected the committed configuration to be served, got {:?}",
            committed["flows"]
        );
    }
}

/// A precondition that is not the link's current one is refused, and the refused
/// call changes nothing — whichever way the presented token fails to be current.
#[tokio::test]
async fn a_precondition_that_is_not_current_is_refused() {
    let fabric = Fabric::new();
    let (gateway_id, link_id) = fabric.active_link("flow-precondition").await;
    let path = format!("/gateway/{gateway_id}/link/{link_id}");
    let (other_gateway_id, other_link_id) = fabric.active_link("flow-precondition-other").await;
    let other_token = text(
        &fabric
            .read("another link", &format!("/gateway/{other_gateway_id}/link/{other_link_id}"))
            .await,
        "lockToken",
    );

    // One accepted change, so the token read before it is no longer current.
    let first = fabric.read("the first read", &path).await;
    let spent = text(&first, "lockToken");
    let stated = vec![sequential_flow("Main", &["Observe"])];
    let (status, _, accepted) = fabric
        .put_flows(&gateway_id, &link_id, &spent, &stated, None)
        .await;
    assert_eq!(status, 200, "PutLinkFlows: {status} {accepted:?}");
    let committed = fabric.poll_status(&path, LINK_STATUS_ACTIVE).await;
    let current = text(&committed, "lockToken");
    let served = reported_flows(&committed);
    let updates_before = fabric.updates_held();

    for (case, presented) in [
        ("the token an accepted change made stale", spent.as_str()),
        ("another link's current token", other_token.as_str()),
        ("a token no read of this store handed out", "not-a-lock-token"),
    ] {
        let (status, code, body) = fabric
            .put_flows(
                &gateway_id,
                &link_id,
                presented,
                &[sequential_flow("Refused", &["Observe"])],
                None,
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a stale precondition to be refused with 409, got {status} {body:?}"
        );
        assert_eq!(
            code, "ConflictException",
            "case {case:?}: expected the modelled conflict code, got {code:?}"
        );
        assert!(
            !text(&body, "message").contains(presented),
            "case {case:?}: a refusal must not echo the token presented, got {body:?}"
        );

        let after = fabric.read(case, &path).await;
        assert_eq!(
            reported_flows(&after),
            served,
            "case {case:?}: expected the refused call to change nothing, got {:?}",
            after["flows"]
        );
        assert_eq!(
            text(&after, "lockToken"),
            current,
            "case {case:?}: expected the refused call to leave the precondition usable"
        );
        assert_eq!(
            fabric.updates_held(),
            updates_before,
            "case {case:?}: expected the refused call to start no update"
        );
    }

    // The current token still works, which is what makes a failed call safe to
    // retry.
    let (status, _, accepted) = fabric
        .put_flows(
            &gateway_id,
            &link_id,
            &current,
            &[sequential_flow("Retried", &["Observe"])],
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the current precondition to be accepted, got {status} {accepted:?}"
    );
}

/// A retry under one `clientToken` reports the update the first call started
/// rather than starting a second, and the same token stating other flows is
/// refused.
#[tokio::test]
async fn a_retry_under_one_client_token_reports_the_update_it_started() {
    let fabric = Fabric::new();
    let (gateway_id, link_id) = fabric.active_link("flow-token").await;
    let path = format!("/gateway/{gateway_id}/link/{link_id}");
    let lock_token = text(&fabric.read("the first read", &path).await, "lockToken");
    let stated = vec![sequential_flow("Main", &["Observe"])];

    let (status, _, first) = fabric
        .put_flows(&gateway_id, &link_id, &lock_token, &stated, Some("token-1"))
        .await;
    assert_eq!(status, 200, "PutLinkFlows: {status} {first:?}");
    let update_id = text(&first, "updateId");
    let updates_after_first = fabric.updates_held();

    // The retry carries the token the first call made stale, which is exactly
    // what a client that lost the first answer sends: the retry is answered from
    // the update it started, before the precondition is looked at.
    let (status, _, retried) = fabric
        .put_flows(&gateway_id, &link_id, &lock_token, &stated, Some("token-1"))
        .await;
    assert_eq!(status, 200, "the retry: {status} {retried:?}");
    assert_eq!(
        text(&retried, "updateId"),
        update_id,
        "expected the retry to report the update the first call started, got {retried:?}"
    );
    assert_eq!(
        fabric.updates_held(),
        updates_after_first,
        "expected the retry to start no second update"
    );

    // The same token stating other flows is a different request, and is refused
    // rather than answered for flows the caller did not send.
    let (status, code, body) = fabric
        .put_flows(
            &gateway_id,
            &link_id,
            &lock_token,
            &[sequential_flow("Other", &["Observe"])],
            Some("token-1"),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected one token with other parameters to be refused, got {status} {body:?}"
    );
    assert_eq!(
        code, "ValidationException",
        "expected the modelled client-input code, got {code:?}"
    );
    assert!(
        text(&body, "message").contains("token-1"),
        "expected the refusal to name the token presented, got {body:?}"
    );

    // A token of its own is a request of its own.
    let current = text(
        &fabric.poll_status(&path, LINK_STATUS_ACTIVE).await,
        "lockToken",
    );
    let (status, _, second) = fabric
        .put_flows(
            &gateway_id,
            &link_id,
            &current,
            &[sequential_flow("Second", &["Observe"])],
            Some("token-2"),
        )
        .await;
    assert_eq!(status, 200, "the second change: {status} {second:?}");
    assert_ne!(
        text(&second, "updateId"),
        update_id,
        "expected a change of its own to start an update of its own, got {second:?}"
    );
}

/// An id that names no link on the addressed gateway is not found, and a link in
/// a status nothing moves it out of has no flows left to configure.
#[tokio::test]
async fn a_link_that_cannot_take_a_configuration_refuses_the_call() {
    let fabric = Fabric::new();
    let (gateway_id, link_id) = fabric.active_link("flow-refusals").await;
    let path = format!("/gateway/{gateway_id}/link/{link_id}");
    let lock_token = text(&fabric.read("the first read", &path).await, "lockToken");
    let stated = vec![sequential_flow("Main", &["Observe"])];

    for (case, gateway, link) in [
        (
            "a link id nothing minted",
            gateway_id.as_str(),
            "link-000000000000ffff",
        ),
        (
            "a gateway id nothing minted",
            "rtb-gw-000000000000ffff",
            link_id.as_str(),
        ),
    ] {
        let (status, code, body) = fabric
            .put_flows(gateway, link, &lock_token, &stated, None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a not-found address to answer 404, got {status} {body:?}"
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found code, got {code:?}"
        );
        assert_eq!(
            text(&body, "message"),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected this service's one not-found wording, got {body:?}"
        );
    }

    // Each row is a status nothing moves a link out of: the peer declined the
    // request, the link could not be established, or it is gone.
    for case in ["REJECTED", "FAILED", "DELETED"] {
        fabric.force_link_status(case);
        let (status, code, body) = fabric
            .put_flows(&gateway_id, &link_id, &lock_token, &stated, None)
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a link in that status to refuse the call, got {status} {body:?}"
        );
        assert_eq!(
            code, "ConflictException",
            "case {case:?}: expected the modelled conflict code, got {code:?}"
        );
        assert!(
            text(&body, "message").contains(case)
                && text(&body, "message").contains(&link_id)
                && text(&body, "message").contains("cannot be configured"),
            "case {case:?}: expected the refusal to name the link and the status refusing it, got {body:?}"
        );
    }
}
