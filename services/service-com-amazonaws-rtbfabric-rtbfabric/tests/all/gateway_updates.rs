// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListUpdates`: the ledger of the changes a gateway has
//! accepted.
//!
//! The rules under test are about state, not about one recorded request. Every
//! call this service accepts that changes a gateway, a link one of its ends is,
//! or a routing rule of such a link mints an `updateId` and records the change on
//! the gateway it was addressed to; the change is `IN_PROGRESS` while it is in
//! flight and `SUCCESSFUL` once the clock has made its commit due. `ListUpdates`
//! reports those records, filtered by the link, the rule, the kind and the status
//! the caller states — a filter the caller leaves out selects every record — and
//! paged by the one page walk this service's listings share. A `ruleId` without a
//! `linkId` addresses nothing, so it is refused; a gateway this service never
//! stored is not found rather than an empty page.
//!
//! The tables span what those rules generalise over: every operation of this
//! service that mints an update id, every member of the modelled `UpdateType` and
//! of `UpdateStatus` as a filter, both kinds of resource a change is recorded
//! against (a link, and the gateway itself), and each published bound of the
//! modelled `maxResults` range.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::state::{
    ComAmazonawsRtbfabricRtbfabricState, LINK_STATUS_ACTIVE, RESOURCE_NOT_FOUND_MESSAGE,
    UPDATES_RETAINED_PER_GATEWAY, UPDATE_STATUS_IN_PROGRESS, UPDATE_STATUS_SUCCESSFUL,
    UPDATE_TYPE_GATEWAY_CONFIG, UPDATE_TYPE_LINK_CONFIG, UPDATE_TYPE_LINK_FLOW,
    UPDATE_TYPE_ROUTING_RULE,
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

/// The members of the modelled `UpdateType`
/// (`smithy_query(query="shape", shape="UpdateType")`).
const UPDATE_TYPE_VALUES: &[&str] = &[
    UPDATE_TYPE_GATEWAY_CONFIG,
    UPDATE_TYPE_LINK_CONFIG,
    UPDATE_TYPE_LINK_FLOW,
    UPDATE_TYPE_ROUTING_RULE,
];

/// The members of the modelled `UpdateStatus`
/// (`smithy_query(query="shape", shape="UpdateStatus")`).
const UPDATE_STATUS_VALUES: &[&str] = &["IN_PROGRESS", "SUCCESSFUL", "FAILED"];

/// `LinkLogSettings` with its one required member, as
/// `smithy_query(query="shape", shape="LinkLogSettings")` declares it.
fn log_settings() -> Value {
    json!({ "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } } })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// The entries of a reported list member.
fn entries(body: &Value, member: &str) -> Vec<Value> {
    body[member].as_array().cloned().unwrap_or_default()
}

/// The ids of the updates a listing reports, in the order it reports them.
fn reported_ids(body: &Value) -> Vec<String> {
    entries(body, "updates")
        .iter()
        .map(|update| text(update, "updateId"))
        .collect()
}

/// One change a call accepted, as the test expects the ledger to hold it.
struct Accepted {
    /// What the call was, for a failing assertion to name.
    case: &'static str,
    /// The gateway the call was addressed to, and so the one whose ledger holds
    /// the record.
    gateway_id: String,
    /// The id the call answered with.
    update_id: String,
    /// What the ledger held the instant after the call, as `(the status, whether
    /// the clock owes the change a commit)`: captured there rather than later,
    /// because any later call materialises the commits the clock has made due.
    in_flight: (String, bool),
    /// The member of `UpdateType` the change is.
    update_type: &'static str,
    /// The link the change was accepted on, when it was a change to a link or to
    /// one of its rules.
    link_id: Option<String>,
    /// The rule the change was accepted on.
    rule_id: Option<String>,
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

    /// Send one request, asserting it answered 200, and answer with its body.
    async fn accepted(&self, case: &str, method: &str, path: &str, body: Value) -> Value {
        let (status, _, answered) = self.send(method, path, Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {method} {path:?} to answer 200, got {status} {answered:?}"
        );
        answered
    }

    /// A requester gateway, a responder gateway and a link between them that is
    /// up, all over the wire.
    async fn active_link(&self, token: &str) -> (String, String, String) {
        let requester = self
            .accepted(
                "CreateRequesterGateway",
                "POST",
                "/requester-gateway",
                json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "clientToken": format!("{token}-requester"),
                }),
            )
            .await;
        let responder = self
            .accepted(
                "CreateResponderGateway",
                "POST",
                "/responder-gateway",
                json!({
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                    "port": 443,
                    "protocol": "HTTPS",
                    "domainName": "responder.example.com",
                    "clientToken": format!("{token}-responder"),
                }),
            )
            .await;
        let requester_id = text(&requester, "gatewayId");
        let responder_id = text(&responder, "gatewayId");
        let link = self
            .accepted(
                "CreateLink",
                "POST",
                &format!("/gateway/{requester_id}/create-link"),
                json!({ "peerGatewayId": responder_id, "logSettings": log_settings() }),
            )
            .await;
        let link_id = text(&link, "linkId");
        self.accepted(
            "AcceptLink",
            "POST",
            &format!("/gateway/{responder_id}/link/{link_id}/accept"),
            json!({ "logSettings": log_settings() }),
        )
        .await;
        self.poll_link_status(&requester_id, &link_id, LINK_STATUS_ACTIVE)
            .await;
        (requester_id, responder_id, link_id)
    }

    /// Read a link until it reports `expected`.
    ///
    /// A settled status is owed by the clock, so the test waits for the
    /// CONDITION rather than for a span.
    async fn poll_link_status(&self, gateway_id: &str, link_id: &str, expected: &str) {
        let path = format!("/gateway/{gateway_id}/link/{link_id}");
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let (status, _, body) = self.send("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "expected GetLink on {path:?} to answer 200, got {status} {body:?}"
            );
            if text(&body, "status") == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {path:?} to reach {expected:?}, got {:?}",
                body["status"]
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `ListUpdates` over the wire, asserting the listing answered.
    async fn updates(&self, case: &str, gateway_id: &str, query: &str) -> Value {
        let path = format!("/gateway/{gateway_id}/updates{query}");
        let (status, _, body) = self.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {path:?} to answer 200, got {status} {body:?}"
        );
        body
    }

    /// Read the listing until the update `update_id` reports `expected`.
    async fn poll_update_status(&self, gateway_id: &str, update_id: &str, expected: &str) -> Value {
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let body = self
                .updates("polling for a commit", gateway_id, "?maxResults=100")
                .await;
            let found = entries(&body, "updates")
                .into_iter()
                .find(|update| text(update, "updateId") == update_id);
            if let Some(found) = &found {
                if text(found, "status") == expected {
                    return found.clone();
                }
            }
            assert!(
                Instant::now() < deadline,
                "expected update {update_id:?} to reach {expected:?}, got {found:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The ledger record of `update_id`, read straight out of the store as the
    /// accepting call left it.
    ///
    /// Read from state rather than over the wire: a reporting call materialises
    /// the commits the clock has made due, so a wire read taken on a slow host
    /// could report the commit this assertion is about, while the store holds
    /// exactly what the accepting call wrote until something else runs.
    fn stored_update(&self, update_id: &str) -> (String, bool) {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let store = state.lock();
        let update = store
            .updates
            .iter()
            .find(|update| update.update_id == update_id)
            .expect("the accepting call recorded the update it answered with");
        (update.status.clone(), update.settles_at.is_some())
    }

    /// How many update records the store holds in all, read straight out of state:
    /// what bounds the ledger is a property of the store rather than of one
    /// listing's page.
    fn updates_held(&self) -> usize {
        let state = self.engine.state::<ComAmazonawsRtbfabricRtbfabricState>();
        let store = state.lock();
        store.updates.len()
    }
}

/// Every call that mints an update id records that change on the gateway it was
/// addressed to, and `ListUpdates` reports it: in flight at first, committed once
/// the clock has made the commit due.
#[tokio::test]
async fn every_accepted_change_is_recorded_and_then_commits() {
    let fabric = Fabric::new();
    let (requester_id, responder_id, link_id) = fabric.active_link("ledger").await;

    // A rule to change, created on the responder gateway the link answers to.
    let rule = fabric
        .accepted(
            "CreateLinkRoutingRule",
            "POST",
            &format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule"),
            json!({
                "clientToken": "ledger-rule",
                "priority": 10,
                "conditions": { "pathPrefix": "/bid" },
            }),
        )
        .await;
    let rule_id = text(&rule, "ruleId");

    // Each row is one operation of this service that mints an update id: the
    // call, and the kind of change its record is.
    let accepted: Vec<Accepted> = vec![
        {
            let answered = fabric
                .accepted(
                    "UpdateLink",
                    "PATCH",
                    &format!("/gateway/{requester_id}/link/{link_id}"),
                    json!({ "timeoutInMillis": 1_000, "clientToken": "ledger-update-link" }),
                )
                .await;
            Accepted {
                case: "UpdateLink",
                gateway_id: requester_id.clone(),
                update_id: text(&answered, "updateId"),
                in_flight: fabric.stored_update(&text(&answered, "updateId")),
                update_type: UPDATE_TYPE_LINK_CONFIG,
                link_id: Some(link_id.clone()),
                rule_id: None,
            }
        },
        {
            let answered = fabric
                .accepted(
                    "UpdateLinkModuleFlow",
                    "POST",
                    &format!("/gateway/{requester_id}/link/{link_id}/module-flow"),
                    json!({
                        "clientToken": "ledger-module-flow",
                        "modules": [{ "name": "Observe" }],
                    }),
                )
                .await;
            Accepted {
                case: "UpdateLinkModuleFlow",
                gateway_id: requester_id.clone(),
                update_id: text(&answered, "updateId"),
                in_flight: fabric.stored_update(&text(&answered, "updateId")),
                update_type: UPDATE_TYPE_LINK_FLOW,
                link_id: Some(link_id.clone()),
                rule_id: None,
            }
        },
        {
            // The precondition is read here, after the changes above: each of
            // them made the token a reader handed out before it stale.
            let link_lock_token = text(
                &fabric
                    .send(
                        "GET",
                        &format!("/gateway/{requester_id}/link/{link_id}"),
                        None,
                    )
                    .await
                    .2,
                "lockToken",
            );
            let answered = fabric
                .accepted(
                    "PutLinkFlows",
                    "PUT",
                    &format!("/gateway/{requester_id}/link/{link_id}/flows"),
                    json!({
                        "lockToken": link_lock_token,
                        "flows": [],
                        "clientToken": "ledger-flows",
                    }),
                )
                .await;
            Accepted {
                case: "PutLinkFlows",
                gateway_id: requester_id.clone(),
                update_id: text(&answered, "updateId"),
                in_flight: fabric.stored_update(&text(&answered, "updateId")),
                update_type: UPDATE_TYPE_LINK_FLOW,
                link_id: Some(link_id.clone()),
                rule_id: None,
            }
        },
        {
            let answered = fabric
                .accepted(
                    "UpdateLinkRoutingRule",
                    "PUT",
                    &format!(
                        "/responder-gateway/{responder_id}/link/{link_id}/routing-rule/{rule_id}"
                    ),
                    json!({
                        "priority": 20,
                        "conditions": { "pathPrefix": "/bid/openrtb" },
                        "clientToken": "ledger-rule-update",
                    }),
                )
                .await;
            Accepted {
                case: "UpdateLinkRoutingRule",
                gateway_id: responder_id.clone(),
                update_id: text(&answered, "updateId"),
                in_flight: fabric.stored_update(&text(&answered, "updateId")),
                update_type: UPDATE_TYPE_ROUTING_RULE,
                link_id: Some(link_id.clone()),
                rule_id: Some(rule_id.clone()),
            }
        },
        {
            let answered = fabric
                .accepted(
                    "UpdateResponderGateway",
                    "POST",
                    &format!("/responder-gateway/{responder_id}/update"),
                    json!({
                        "clientToken": "ledger-gateway",
                        "port": 443,
                        "protocol": "HTTPS",
                        "description": "a changed description",
                    }),
                )
                .await;
            Accepted {
                case: "UpdateResponderGateway",
                gateway_id: responder_id.clone(),
                update_id: text(&answered, "updateId"),
                in_flight: fabric.stored_update(&text(&answered, "updateId")),
                update_type: UPDATE_TYPE_GATEWAY_CONFIG,
                link_id: None,
                rule_id: None,
            }
        },
    ];

    for change in &accepted {
        let case = change.case;
        assert!(
            change.update_id.starts_with("update-"),
            "case {case:?}: expected the modelled UpdateId spelling, got {:?}",
            change.update_id
        );
        // What the accepting call wrote: the change is in flight, with the
        // instant the clock owes its commit at.
        let (status, due) = &change.in_flight;
        assert_eq!(
            status, UPDATE_STATUS_IN_PROGRESS,
            "case {case:?}: expected the accepted change to be in flight"
        );
        assert!(
            *due,
            "case {case:?}: expected the clock to owe the change its commit"
        );

        // And what the listing reports once the clock has made that commit due.
        let committed = fabric
            .poll_update_status(&change.gateway_id, &change.update_id, UPDATE_STATUS_SUCCESSFUL)
            .await;
        assert_eq!(
            text(&committed, "type"),
            change.update_type,
            "case {case:?}: expected the record to name the kind of change, got {committed:?}"
        );
        assert!(
            committed["createdAt"].is_number(),
            "case {case:?}: expected the record to carry the instant it was accepted, got {committed:?}"
        );

        // The record names the resource the change was accepted on, which is
        // what the link and rule filters select by.
        if let Some(link_id) = &change.link_id {
            let selected = fabric
                .updates(
                    case,
                    &change.gateway_id,
                    &format!("?maxResults=100&linkId={link_id}"),
                )
                .await;
            assert!(
                reported_ids(&selected).contains(&change.update_id),
                "case {case:?}: expected the link filter to select the change accepted on it, got {selected:?}"
            );
        }
        if let (Some(link_id), Some(rule_id)) = (&change.link_id, &change.rule_id) {
            let selected = fabric
                .updates(
                    case,
                    &change.gateway_id,
                    &format!("?maxResults=100&linkId={link_id}&ruleId={rule_id}"),
                )
                .await;
            assert_eq!(
                reported_ids(&selected),
                vec![change.update_id.clone()],
                "case {case:?}: expected the rule filter to select exactly the change accepted on it"
            );
        }
    }

    // The updates of each gateway are the changes accepted on it, each reported
    // exactly once, oldest first — which is the order their ids were minted in.
    for gateway_id in [&requester_id, &responder_id] {
        let case = format!("the ledger of {gateway_id}");
        let body = fabric.updates(&case, gateway_id, "?maxResults=100").await;
        let reported = reported_ids(&body);
        let expected: Vec<String> = accepted
            .iter()
            .filter(|change| change.gateway_id == *gateway_id)
            .map(|change| change.update_id.clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly that gateway's changes, oldest first"
        );
    }
}

/// A filter the caller states selects exactly the records it names; one it leaves
/// out selects every record.
#[tokio::test]
async fn a_stated_filter_selects_exactly_the_records_it_names() {
    let fabric = Fabric::new();
    let (_requester_id, responder_id, link_id) = fabric.active_link("ledger-filters").await;
    let rule = fabric
        .accepted(
            "CreateLinkRoutingRule",
            "POST",
            &format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule"),
            json!({
                "clientToken": "filters-rule",
                "priority": 10,
                "conditions": { "pathPrefix": "/bid" },
            }),
        )
        .await;
    let rule_id = text(&rule, "ruleId");

    // Two changes on the responder gateway: one on a rule of the link, one on
    // the gateway itself, so the link and rule filters have both a record they
    // select and one they do not.
    let rule_update = text(
        &fabric
            .accepted(
                "UpdateLinkRoutingRule",
                "PUT",
                &format!("/responder-gateway/{responder_id}/link/{link_id}/routing-rule/{rule_id}"),
                json!({
                    "priority": 20,
                    "conditions": { "pathPrefix": "/bid/openrtb" },
                    "clientToken": "filters-rule-update",
                }),
            )
            .await,
        "updateId",
    );
    let gateway_update = text(
        &fabric
            .accepted(
                "UpdateResponderGateway",
                "POST",
                &format!("/responder-gateway/{responder_id}/update"),
                json!({
                    "clientToken": "filters-gateway",
                    "port": 443,
                    "protocol": "HTTPS",
                    "description": "a changed description",
                }),
            )
            .await,
        "updateId",
    );
    // Both changes have committed before the status filters below are read, so
    // what each filter selects does not depend on the host's speed.
    fabric
        .poll_update_status(&responder_id, &rule_update, UPDATE_STATUS_SUCCESSFUL)
        .await;
    fabric
        .poll_update_status(&responder_id, &gateway_update, UPDATE_STATUS_SUCCESSFUL)
        .await;

    // Each row is a filter and the records it must select.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter at all",
            String::new(),
            vec![rule_update.clone(), gateway_update.clone()],
        ),
        (
            "the link the rule routes for",
            format!("&linkId={link_id}"),
            vec![rule_update.clone()],
        ),
        (
            "the rule itself, through its link",
            format!("&linkId={link_id}&ruleId={rule_id}"),
            vec![rule_update.clone()],
        ),
        (
            "a link id nothing minted",
            "&linkId=link-000000000000ffff".to_string(),
            Vec::new(),
        ),
        (
            "a rule id the link does not carry",
            format!("&linkId={link_id}&ruleId=rule-000000000000ffff"),
            Vec::new(),
        ),
    ];

    for (case, filter, expected) in rows {
        let body = fabric
            .updates(case, &responder_id, &format!("?maxResults=100{filter}"))
            .await;
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: expected the filter to select exactly those records"
        );
    }

    // Every member of the modelled `UpdateType`, and of `UpdateStatus`: a filter
    // selects the records in that kind or status, and none of the others.
    for wanted in UPDATE_TYPE_VALUES {
        let body = fabric
            .updates(wanted, &responder_id, &format!("?maxResults=100&type={wanted}"))
            .await;
        for update in entries(&body, "updates") {
            assert_eq!(
                text(&update, "type"),
                *wanted,
                "case {wanted:?}: expected every selected record to be of that kind"
            );
        }
        let expected: Vec<String> = match *wanted {
            UPDATE_TYPE_ROUTING_RULE => vec![rule_update.clone()],
            UPDATE_TYPE_GATEWAY_CONFIG => vec![gateway_update.clone()],
            _ => Vec::new(),
        };
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {wanted:?}: expected the kind filter to select exactly those records"
        );
    }
    for wanted in UPDATE_STATUS_VALUES {
        let body = fabric
            .updates(wanted, &responder_id, &format!("?maxResults=100&status={wanted}"))
            .await;
        let reported = reported_ids(&body);
        if *wanted == UPDATE_STATUS_SUCCESSFUL {
            assert_eq!(
                reported,
                vec![rule_update.clone(), gateway_update.clone()],
                "case {wanted:?}: expected the committed changes to be selected"
            );
        } else {
            assert!(
                reported.is_empty(),
                "case {wanted:?}: no change stands in that status, got {reported:?}"
            );
        }
    }
}

/// The walk over one gateway's ledger reports every record exactly once, in
/// order, whatever page size the caller asks for.
#[tokio::test]
async fn a_walk_over_the_ledger_reports_every_record_once() {
    let fabric = Fabric::new();
    let (requester_id, _, link_id) = fabric.active_link("ledger-paging").await;

    // Several changes to one link, so there is more than one page to walk.
    let mut minted: Vec<String> = Vec::new();
    for timeout in [200, 300, 400, 500, 600] {
        let answered = fabric
            .accepted(
                "UpdateLink",
                "PATCH",
                &format!("/gateway/{requester_id}/link/{link_id}"),
                json!({
                    "timeoutInMillis": timeout,
                    "clientToken": format!("paging-{timeout}"),
                }),
            )
            .await;
        minted.push(text(&answered, "updateId"));
    }

    // The modelled `maxResults` range is 1..100, so the rows are its minimum, a
    // value inside it and its maximum.
    for per_page in [1usize, 2, 100] {
        let case = format!("maxResults={per_page}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &next_token {
                Some(token) => format!("?maxResults={per_page}&nextToken={token}"),
                None => format!("?maxResults={per_page}"),
            };
            let body = fabric.updates(&case, &requester_id, &query).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= per_page,
                "case {case:?}: expected at most {per_page} records on a page, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 32,
                "case {case:?}: the walk did not terminate, {walked:?} so far"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, minted,
            "case {case:?}: expected the walk to report every record once, oldest first"
        );
    }
}

/// One gateway's ledger is BOUNDED: it retains at most
/// `UPDATES_RETAINED_PER_GATEWAY` records whatever a caller does, letting go of
/// the oldest as a fresh change is accepted — so no sequence of accepted changes,
/// however long, grows the store without end.
#[tokio::test]
async fn the_ledger_of_one_gateway_is_bounded() {
    let fabric = Fabric::new();
    let (requester_id, _, link_id) = fabric.active_link("ledger-bound").await;

    // More changes than the gateway retains, each a request of its own (no
    // `clientToken`, so none is a retry of another).
    let accepted = UPDATES_RETAINED_PER_GATEWAY + 7;
    let mut minted: Vec<String> = Vec::new();
    for at in 0..accepted {
        let answered = fabric
            .accepted(
                "UpdateLink",
                "PATCH",
                &format!("/gateway/{requester_id}/link/{link_id}"),
                json!({ "timeoutInMillis": 100 + (at % 100) as i64 }),
            )
            .await;
        minted.push(text(&answered, "updateId"));
    }

    assert_eq!(
        fabric.updates_held(),
        UPDATES_RETAINED_PER_GATEWAY,
        "expected the store to retain exactly the gateway's bound after {accepted} accepted changes"
    );
    let reported = reported_ids(
        &fabric
            .updates("the bounded ledger", &requester_id, "?maxResults=100")
            .await,
    );
    assert_eq!(
        reported,
        minted[accepted - UPDATES_RETAINED_PER_GATEWAY..],
        "expected the ledger to report the newest records it retains, oldest first"
    );
    for forgotten in &minted[..accepted - UPDATES_RETAINED_PER_GATEWAY] {
        assert!(
            !reported.contains(forgotten),
            "expected the oldest record {forgotten:?} to have been let go of"
        );
    }
}

/// A `ruleId` without a `linkId` addresses nothing, so it is refused; a gateway
/// this service never stored is not found rather than an empty page.
#[tokio::test]
async fn a_listing_that_addresses_nothing_is_refused() {
    let fabric = Fabric::new();
    let (requester_id, _, link_id) = fabric.active_link("ledger-refusals").await;

    let (status, code, body) = fabric
        .send(
            "GET",
            &format!("/gateway/{requester_id}/updates?ruleId=rule-000000000000ffff"),
            None,
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a ruleId without a linkId to be refused, got {status} {body:?}"
    );
    assert_eq!(
        code, "ValidationException",
        "expected the modelled client-input code, got {code:?}"
    );
    assert!(
        text(&body, "message").contains("ruleId") && text(&body, "message").contains("linkId"),
        "expected the refusal to name both members, got {body:?}"
    );

    // The same rule filter WITH a link is an address, so it is answered.
    let (status, _, body) = fabric
        .send(
            "GET",
            &format!(
                "/gateway/{requester_id}/updates?linkId={link_id}&ruleId=rule-000000000000ffff"
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected a rule filter stated through its link to be answered, got {status} {body:?}"
    );

    let (status, code, body) = fabric
        .send("GET", "/gateway/rtb-gw-000000000000ffff/updates", None)
        .await;
    assert_eq!(
        status, 404,
        "expected a gateway nothing minted to be not found, got {status} {body:?}"
    );
    assert_eq!(
        code, "ResourceNotFoundException",
        "expected the modelled not-found code, got {code:?}"
    );
    assert_eq!(
        text(&body, "message"),
        RESOURCE_NOT_FOUND_MESSAGE,
        "expected this service's one not-found wording, got {body:?}"
    );
}
