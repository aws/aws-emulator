// SPDX-License-Identifier: CC0-1.0

//! The DNS Firewall RULE slice of this service's state contract, driven end-to-end over the
//! service's own `restJson1` wire through the same `MultiServiceRouter` the production
//! emulator dispatches with.
//!
//! Every test states a RULE over a set of inputs rather than one recorded shape: a created
//! rule is read back by `GetFirewallRule` member for member, a rule's own cross-field
//! requirements are settled BEFORE any id the request names is resolved, a value outside an
//! enum shape is refused, an unnamed priority is assigned from the view's own rules, a
//! `clientToken` replay answers with the rule it minted, and a deleted resolver takes the
//! rules of its views with it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_route53globalresolver_ec2dnsglobalresolvercustomerapi as resolver_service;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the request's SigV4 credential scope names. Route 53 Global Resolver's own docs
/// tell callers to use US East (Ohio).
const REGION: &str = "us-east-2";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "route53globalresolver";

/// How long a poll for a status the clock owes keeps looking. Generous on purpose: the test
/// waits for the CONDITION, and a loaded host only makes it look again.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A well-formed `ResourceId` (`^[-.a-zA-Z0-9]+$`) this store never mints, for the rows that
/// name a resource that is not there.
const ABSENT_ID: &str = "dnsv-ffffffffffff";

/// One emulator holding exactly this service, dispatched over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

/// A dispatched response: status, the `x-amzn-errortype` header (empty on success) and the
/// parsed JSON body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    fn text(&self, name: &str) -> String {
        self.member(name).as_str().unwrap_or_default().to_string()
    }

    /// The MODELLED members this answer's body carries, sorted. `__type` is the protocol's own
    /// error discriminator rather than a member of the shape, so it is left out: what is left
    /// is exactly what the service put on the wire.
    fn modelled_members(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .body
            .as_object()
            .map(|members| {
                members
                    .keys()
                    .filter(|name| name.as_str() != "__type")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Emulator {
    fn new() -> Self {
        // The SINGLE shared engine, exactly as `router/src/lib.rs` builds it.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(resolver_service::EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        );
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `CreateGlobalResolver` — the root every DNS View hangs off.
    async fn create_resolver(&self, name: &str) -> String {
        let answer = self
            .call(
                "POST",
                "/global-resolver",
                Some(json!({ "name": name, "regions": [REGION] })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "setup: CreateGlobalResolver({name:?}) must succeed, got {} with {}",
            answer.status, answer.body
        );
        answer.text("id")
    }

    /// `CreateDNSView` — the parent every firewall rule hangs off.
    async fn create_view(&self, resolver_id: &str, name: &str) -> String {
        let answer = self
            .call(
                "POST",
                &format!("/dns-views/{resolver_id}"),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "setup: CreateDNSView({name:?}) must succeed, got {} with {}",
            answer.status, answer.body
        );
        answer.text("id")
    }

    /// `CreateFirewallDomainList` — one of a rule's two targets.
    async fn create_list(&self, resolver_id: &str, name: &str) -> String {
        let answer = self
            .call(
                "POST",
                &format!("/firewall-domain-lists/{resolver_id}"),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "setup: CreateFirewallDomainList({name:?}) must succeed, got {} with {}",
            answer.status, answer.body
        );
        answer.text("id")
    }

    async fn create_rule(&self, body: Value) -> Answer {
        self.call("POST", "/firewall-rules", Some(body)).await
    }

    async fn get_rule(&self, rule_id: &str) -> Answer {
        self.call("GET", &format!("/firewall-rules/{rule_id}"), None)
            .await
    }

    async fn delete_resolver(&self, resolver_id: &str) -> Answer {
        self.call("DELETE", &format!("/global-resolver/{resolver_id}"), None)
            .await
    }
}

/// A `ResourceNotFoundException` body of this service, as every path that resolves an id
/// words it: the service's own bracketed code, the kind the id was looked up as, and the id
/// the REQUEST named.
fn not_found_message(kind: &str, id: &str) -> String {
    format!("[GR-ERR00309] No {kind} found with the ID: {id}")
}

/// A created rule is reported by `GetFirewallRule` with the members the store holds, whatever
/// combination of the optional ones the create carried.
#[tokio::test]
async fn a_created_firewall_rule_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("rule-root").await;
    let view_id = emulator.create_view(&resolver_id, "rule-parent").await;
    let list_id = emulator.create_list(&resolver_id, "rule-target").await;

    // Rows differ along what a rule may BE: which of the two targets it names, which action it
    // takes, which response a BLOCK answers with, and which of the optional members the
    // request carries. The enum values are the model's own catalogs
    // (`smithy_query(query="shape",shape="FirewallRuleAction,FirewallBlockResponse,DnsAdvancedProtection,ConfidenceThreshold,BlockOverrideDnsQueryType")`).
    let rows: Vec<(&str, Value)> = vec![
        (
            "the smallest rule: a domain list target and ALLOW",
            json!({ "name": "allow-min", "action": "ALLOW", "firewallDomainListId": list_id }),
        ),
        (
            "ALERT with every optional member a domain-list rule may carry",
            json!({
                "name": "alert-full",
                "action": "ALERT",
                "firewallDomainListId": list_id,
                "description": "alerts on the listed domains",
                "priority": 42,
                "qType": "AAAA",
            }),
        ),
        (
            "BLOCK answering NXDOMAIN",
            json!({
                "name": "block-nxdomain",
                "action": "BLOCK",
                "firewallDomainListId": list_id,
                "blockResponse": "NXDOMAIN",
            }),
        ),
        (
            "BLOCK answering NODATA",
            json!({
                "name": "block-nodata",
                "action": "BLOCK",
                "firewallDomainListId": list_id,
                "blockResponse": "NODATA",
            }),
        ),
        (
            "BLOCK answering a custom OVERRIDE record",
            json!({
                "name": "block-override",
                "action": "BLOCK",
                "firewallDomainListId": list_id,
                "blockResponse": "OVERRIDE",
                "blockOverrideDomain": "parked.example.com",
                "blockOverrideDnsType": "CNAME",
                "blockOverrideTtl": 300,
            }),
        ),
        (
            "an advanced-protection target: DNS tunneling, at HIGH confidence",
            json!({
                "name": "tunneling",
                "action": "ALERT",
                "dnsAdvancedProtection": "DNS_TUNNELING",
                "confidenceThreshold": "HIGH",
            }),
        ),
        (
            "an advanced-protection target the cited case did not name: dictionary DGA",
            json!({
                "name": "dictionary-dga",
                "action": "BLOCK",
                "dnsAdvancedProtection": "DICTIONARY_DGA",
                "blockResponse": "NODATA",
                "confidenceThreshold": "LOW",
                "qType": "A",
            }),
        ),
        (
            "an empty qType, which the member's 0..16 length allows",
            json!({
                "name": "empty-qtype",
                "action": "ALLOW",
                "dnsAdvancedProtection": "DGA",
                "qType": "",
            }),
        ),
    ];

    for (case, mut body) in rows {
        body["dnsViewId"] = json!(view_id);
        let created = emulator.create_rule(body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            created.status, created.body
        );
        let id = created.text("id");
        assert!(
            id.starts_with("fr-"),
            "case {case:?}: expected a minted rule id, got {id:?}"
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {case:?}: a create answers the non-terminal status, got {:?}",
            created.text("status")
        );

        let fetched = emulator.get_rule(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the created rule must read back, got {} with {}",
            fetched.status, fetched.body
        );
        // The members the request named come back verbatim, under the names the OUTPUT
        // declares (`qType` is carried out as `queryType`).
        for (member, requested) in body.as_object().expect("a request body object") {
            let member = match member.as_str() {
                "qType" => "queryType",
                other => other,
            };
            assert_eq!(
                fetched.member(member),
                requested,
                "case {case:?}: expected {member} {requested:?}, got {:?}",
                fetched.member(member)
            );
            assert_eq!(
                created.member(member),
                requested,
                "case {case:?}: the create and the get must agree on {member}, got {:?}",
                created.member(member)
            );
        }
        // A rule is created with a priority whether the request named one or not.
        let priority = fetched.member("priority").as_i64().unwrap_or_default();
        assert!(
            (1..=10_000).contains(&priority),
            "case {case:?}: expected a priority inside the modelled range, got {priority}"
        );
        // The status the get reports is the one the clock owes: still CREATING, or
        // OPERATIONAL once the provisioning span has passed. Nothing here asserts which.
        let settled = fetched.text("status");
        assert!(
            settled == "CREATING" || settled == "OPERATIONAL",
            "case {case:?}: expected a status the create path produces, got {settled:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                !fetched.member(member).is_null(),
                "case {case:?}: expected a {member} on the stored rule, got {:?}",
                fetched.member(member)
            );
        }
        assert_eq!(
            fetched.member("createdAt"),
            created.member("createdAt"),
            "case {case:?}: the creation instant is the stored one, got {:?}",
            fetched.member("createdAt")
        );
        // An optional member the request did not name is ABSENT, not invented.
        let unnamed: Vec<String> = fetched
            .modelled_members()
            .into_iter()
            .filter(|name| {
                !body.as_object().expect("a request body object").keys().any(|requested| {
                    requested == name || (requested == "qType" && name == "queryType")
                })
            })
            .collect();
        let minted: BTreeSet<&str> = ["id", "priority", "status", "createdAt", "updatedAt"]
            .into_iter()
            .collect();
        for name in &unnamed {
            assert!(
                minted.contains(name.as_str()),
                "case {case:?}: {name} was not requested and is not minted by the service, \
                 got {:?}",
                fetched.body
            );
        }
    }
}

/// One row of the refusal table: the case, the request body, and what the service must answer.
struct RefusalRow {
    case: &'static str,
    body: Value,
    status: u16,
    error_type: &'static str,
    message: String,
}

/// A rule's own cross-field requirements are settled from the request alone, so they are
/// reported BEFORE any id the request names is resolved; only a request that does describe a
/// rule reaches the DNS View lookup.
#[tokio::test]
async fn a_rule_definition_is_refused_before_the_ids_it_names_are_resolved() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("refusal-root").await;
    let view_id = emulator.create_view(&resolver_id, "refusal-parent").await;
    let list_id = emulator.create_list(&resolver_id, "refusal-target").await;

    let target_rule = "[GR-ERR04003] Input should contain exactly one of FirewallDomainListId \
                       or DnsAdvancedProtection."
        .to_string();
    let block_rule = "[GR-ERR04001] BlockResponse must be provided if Action is BLOCK.".to_string();

    // Every row that breaks a rule-level requirement names an ABSENT dnsViewId, so an answer
    // that resolved the id first would be the 404 instead — which is what the last rows show
    // the service does answer once the definition itself holds.
    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "no target at all",
            body: json!({ "name": "no-target", "action": "ALLOW", "dnsViewId": ABSENT_ID }),
            status: 400,
            error_type: "ValidationException",
            message: target_rule.clone(),
        },
        RefusalRow {
            case: "no target, and a BLOCK with no response either",
            body: json!({ "name": "no-target-block", "action": "BLOCK", "dnsViewId": ABSENT_ID }),
            status: 400,
            error_type: "ValidationException",
            message: target_rule.clone(),
        },
        RefusalRow {
            case: "both targets",
            body: json!({
                "name": "two-targets",
                "action": "ALERT",
                "firewallDomainListId": list_id,
                "dnsAdvancedProtection": "DGA",
                "dnsViewId": ABSENT_ID,
            }),
            status: 400,
            error_type: "ValidationException",
            message: target_rule.clone(),
        },
        RefusalRow {
            case: "a BLOCK rule that does not say what to answer with",
            body: json!({
                "name": "block-no-response",
                "action": "BLOCK",
                "firewallDomainListId": list_id,
                "dnsViewId": ABSENT_ID,
            }),
            status: 400,
            error_type: "ValidationException",
            message: block_rule.clone(),
        },
        RefusalRow {
            case: "an OVERRIDE response carrying no custom domain",
            body: json!({
                "name": "override-no-domain",
                "action": "BLOCK",
                "dnsAdvancedProtection": "DGA",
                "blockResponse": "OVERRIDE",
                "blockOverrideDnsType": "CNAME",
                "blockOverrideTtl": 60,
                "dnsViewId": ABSENT_ID,
            }),
            status: 400,
            error_type: "ValidationException",
            message: "blockOverrideDomain: must be provided when blockResponse is OVERRIDE"
                .to_string(),
        },
        RefusalRow {
            case: "an OVERRIDE response carrying no record type",
            body: json!({
                "name": "override-no-type",
                "action": "BLOCK",
                "dnsAdvancedProtection": "DGA",
                "blockResponse": "OVERRIDE",
                "blockOverrideDomain": "parked.example.com",
                "blockOverrideTtl": 60,
                "dnsViewId": ABSENT_ID,
            }),
            status: 400,
            error_type: "ValidationException",
            message: "blockOverrideDnsType: must be provided when blockResponse is OVERRIDE"
                .to_string(),
        },
        RefusalRow {
            case: "an OVERRIDE response carrying no TTL",
            body: json!({
                "name": "override-no-ttl",
                "action": "BLOCK",
                "dnsAdvancedProtection": "DGA",
                "blockResponse": "OVERRIDE",
                "blockOverrideDomain": "parked.example.com",
                "blockOverrideDnsType": "CNAME",
                "dnsViewId": ABSENT_ID,
            }),
            status: 400,
            error_type: "ValidationException",
            message: "blockOverrideTtl: must be provided when blockResponse is OVERRIDE"
                .to_string(),
        },
        RefusalRow {
            case: "a rule that does describe a rule, under a DNS View that is not there",
            body: json!({
                "name": "orphan",
                "action": "ALLOW",
                "dnsAdvancedProtection": "DGA",
                "dnsViewId": ABSENT_ID,
            }),
            status: 404,
            error_type: "ResourceNotFoundException",
            message: not_found_message("DnsView", ABSENT_ID),
        },
        RefusalRow {
            case: "a domain-list target that is not there, under a DNS View that is",
            body: json!({
                "name": "orphan-target",
                "action": "ALLOW",
                "firewallDomainListId": "fdl-ffffffffffff",
                "dnsViewId": view_id,
            }),
            status: 404,
            error_type: "ResourceNotFoundException",
            message: not_found_message("FirewallDomainList", "fdl-ffffffffffff"),
        },
    ];

    for row in rows {
        let answer = emulator.create_rule(row.body.clone()).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected {}, got {} with {}",
            row.case, row.status, answer.status, answer.body
        );
        assert!(
            answer.error_type.starts_with(row.error_type),
            "case {:?}: expected {}, got {:?}",
            row.case,
            row.error_type,
            answer.error_type
        );
        assert_eq!(
            answer.text("message"),
            row.message,
            "case {:?}: expected the refusal to state {:?}, got {:?}",
            row.case,
            row.message,
            answer.text("message")
        );
    }
}

/// A value outside what a member's own shape allows — the set an enum declares, or the range a
/// number declares — is refused on that member, ahead of the rule-level requirements and of
/// every id the request names.
#[tokio::test]
async fn a_value_outside_a_member_shape_is_refused_on_that_member() {
    let emulator = Emulator::new();

    /// A rule definition that satisfies every rule-level requirement, so the only thing wrong
    /// with a row is the member it is testing.
    fn sound_definition(name: &str) -> Value {
        json!({
            "name": name,
            "action": "ALERT",
            "dnsAdvancedProtection": "DGA",
            // Absent from this store on purpose: a refusal of the MEMBER cannot have resolved
            // it, so an answer naming the member proves the ordering.
            "dnsViewId": ABSENT_ID,
        })
    }

    // Rows: one per constrained member of this request, each carrying a value its shape
    // rejects. The sets and the bounds are the model's own
    // (`smithy_query(query="shape",shape="FirewallRuleAction,FirewallBlockResponse,BlockOverrideDnsQueryType,ConfidenceThreshold,DnsAdvancedProtection,FirewallRulePriority,BlockOverrideTtl")`),
    // and each range is tested one past each published bound.
    let rows: Vec<(&str, Value)> = vec![
        ("action", json!("DENY")),
        ("blockResponse", json!("REFUSED")),
        ("blockOverrideDnsType", json!("TXT")),
        ("confidenceThreshold", json!("CERTAIN")),
        ("dnsAdvancedProtection", json!("PHISHING")),
        ("priority", json!(0)),
        ("priority", json!(10_001)),
        ("blockOverrideTtl", json!(-1)),
        ("blockOverrideTtl", json!(604_801)),
    ];

    for (member, value) in rows {
        let case = format!("{member} = {value}");
        let mut body = sound_definition("shape-row");
        body[member] = value.clone();
        let answer = emulator.create_rule(body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.starts_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_type
        );
        let message = answer.text("message");
        assert!(
            message.contains(&format!("at '{member}'")),
            "case {case:?}: expected the refusal to name the member, got {message:?}"
        );
        assert!(
            !message.contains(ABSENT_ID),
            "case {case:?}: a member refusal cannot have resolved the DNS View, got {message:?}"
        );
    }

    // The published bounds themselves are accepted, so the refusals above are about being
    // OUTSIDE the range and not about the range's edges.
    let resolver_id = emulator.create_resolver("bounds-root").await;
    let view_id = emulator.create_view(&resolver_id, "bounds-parent").await;
    for (index, (case, member, value)) in [
        ("the lowest priority", "priority", json!(1)),
        ("the highest priority", "priority", json!(10_000)),
        ("the shortest override TTL", "blockOverrideTtl", json!(0)),
        ("the longest override TTL", "blockOverrideTtl", json!(604_800)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut body = json!({
            "name": "bounds-row",
            "action": "BLOCK",
            "dnsAdvancedProtection": "DGA",
            "blockResponse": "OVERRIDE",
            "blockOverrideDomain": "parked.example.com",
            "blockOverrideDnsType": "CNAME",
            "blockOverrideTtl": 60,
            // Named on every row, so a row testing a TTL bound is not also exercising the
            // priority the service would have assigned it.
            "priority": index as i64 + 1,
            "dnsViewId": view_id,
        });
        body[member] = value.clone();
        let answer = emulator.create_rule(body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the published bound to be accepted, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member(member),
            &value,
            "case {case:?}: expected {member} {value:?} to be stored, got {:?}",
            answer.member(member)
        );
    }
}

/// A request that names no priority is given the one after the highest its DNS View holds, and
/// a view's priorities are its own.
#[tokio::test]
async fn an_unnamed_priority_follows_the_highest_the_view_holds() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("priority-root").await;
    let first_view = emulator.create_view(&resolver_id, "priority-one").await;
    let second_view = emulator.create_view(&resolver_id, "priority-two").await;

    let unnamed = |view: &str, name: &str| {
        json!({
            "name": name,
            "action": "ALLOW",
            "dnsAdvancedProtection": "DGA",
            "dnsViewId": view,
        })
    };

    // Rows: the first three rules of a view, each naming no priority.
    for (index, expected) in [(0, 1_i64), (1, 2), (2, 3)] {
        let case = format!("rule {index} of the first view");
        let answer = emulator
            .create_rule(unnamed(&first_view, &format!("auto-{index}")))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("priority").as_i64(),
            Some(expected),
            "case {case:?}: expected priority {expected}, got {:?}",
            answer.member("priority")
        );
    }

    // A priority the request DOES name is kept, and the next unnamed one follows it.
    let mut named = unnamed(&first_view, "named");
    named["priority"] = json!(500);
    let answer = emulator.create_rule(named).await;
    assert_eq!(
        answer.member("priority").as_i64(),
        Some(500),
        "case {:?}: a named priority is kept, got {:?}",
        "the named priority",
        answer.member("priority")
    );
    let answer = emulator.create_rule(unnamed(&first_view, "after-named")).await;
    assert_eq!(
        answer.member("priority").as_i64(),
        Some(501),
        "case {:?}: expected the priority after the highest the view holds, got {:?}",
        "after the named priority",
        answer.member("priority")
    );

    // Another view's rules are counted separately: its first rule starts over at 1.
    let answer = emulator
        .create_rule(unnamed(&second_view, "other-view"))
        .await;
    assert_eq!(
        answer.member("priority").as_i64(),
        Some(1),
        "case {:?}: a view's priorities are its own, got {:?}",
        "the other view",
        answer.member("priority")
    );
}

/// The same `clientToken` answers with the rule it already minted; the same token with
/// different parameters conflicts instead.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_rule_it_minted() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("token-root").await;
    let view_id = emulator.create_view(&resolver_id, "token-parent").await;

    // Rows differ along which target the replayed request names, so the replay is not keyed to
    // one shape of request.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an advanced-protection rule",
            json!({ "name": "replayed-dga", "action": "ALERT", "dnsAdvancedProtection": "DGA" }),
        ),
        (
            "a BLOCK rule with an OVERRIDE record",
            json!({
                "name": "replayed-override",
                "action": "BLOCK",
                "dnsAdvancedProtection": "DNS_TUNNELING",
                "blockResponse": "OVERRIDE",
                "blockOverrideDomain": "parked.example.com",
                "blockOverrideDnsType": "CNAME",
                "blockOverrideTtl": 120,
            }),
        ),
    ];

    for (case, mut body) in rows {
        body["dnsViewId"] = json!(view_id);
        body["clientToken"] = json!(format!("token-for-{}", body["name"].as_str().unwrap()));
        let first = emulator.create_rule(body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            first.status, first.body
        );
        let replay = emulator.create_rule(body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: a replay is served, got {} with {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("id"),
            first.text("id"),
            "case {case:?}: a replay answers with the rule it minted, got {:?}",
            replay.text("id")
        );

        // The same token asking for something else is a conflict, and it mints nothing.
        let mut changed = body.clone();
        changed["name"] = json!("changed");
        let conflict = emulator.create_rule(changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected 409, got {} with {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.text("resourceId"),
            first.text("id"),
            "case {case:?}: the conflict names the rule the token minted, got {:?}",
            conflict.text("resourceId")
        );
    }
}

/// The clock alone moves a rule out of its non-terminal status; a read reports what is due
/// without being what made it due.
#[tokio::test]
async fn the_clock_alone_turns_a_creating_rule_operational() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("settling-root").await;
    let view_id = emulator.create_view(&resolver_id, "settling-parent").await;
    let created = emulator
        .create_rule(json!({
            "name": "settling",
            "action": "ALLOW",
            "dnsAdvancedProtection": "DGA",
            "dnsViewId": view_id,
        }))
        .await;
    assert_eq!(
        created.text("status"),
        "CREATING",
        "case {:?}: the create answers the non-terminal status, got {:?}",
        "the create",
        created.text("status")
    );
    let id = created.text("id");

    // The transition is owed by the clock, not by the read: this looks again until the
    // condition holds, and nothing here asserts on how long it took.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let fetched = emulator.get_rule(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: the rule must stay readable, got {} with {}",
            "the settled read", fetched.status, fetched.body
        );
        let status = fetched.text("status");
        if status == "OPERATIONAL" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected OPERATIONAL once the provisioning span passed, got {status:?}",
            "the settled read"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A deleted Global Resolver takes the rules of its views with it, so no later reader sees a
/// rule whose view is gone.
#[tokio::test]
async fn a_deleted_resolver_takes_the_rules_of_its_views_with_it() {
    let emulator = Emulator::new();
    // Rows: one rule under the doomed resolver, and three, so the cascade is not a
    // single-child special case.
    for doomed_count in [1usize, 3] {
        let case = format!("{doomed_count} rule(s) under the deleted resolver");
        let keeper = emulator.create_resolver("keeper").await;
        let doomed = emulator.create_resolver("doomed").await;
        let keeper_view = emulator.create_view(&keeper, "keeper-view").await;
        let doomed_view = emulator.create_view(&doomed, "doomed-view").await;

        let kept = emulator
            .create_rule(json!({
                "name": "kept",
                "action": "ALLOW",
                "dnsAdvancedProtection": "DGA",
                "dnsViewId": keeper_view,
            }))
            .await;
        let kept_id = kept.text("id");
        let mut doomed_ids = Vec::new();
        for index in 0..doomed_count {
            let created = emulator
                .create_rule(json!({
                    "name": format!("doomed-{index}"),
                    "action": "ALLOW",
                    "dnsAdvancedProtection": "DGA",
                    "dnsViewId": doomed_view,
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: rule {index} must be created, got {} with {}",
                created.status, created.body
            );
            doomed_ids.push(created.text("id"));
        }

        let deleted = emulator.delete_resolver(&doomed).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the resolver must delete, got {} with {}",
            deleted.status, deleted.body
        );

        for orphan in &doomed_ids {
            let answer = emulator.get_rule(orphan).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: the rule of a deleted view names nothing, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.text("message"),
                not_found_message("FirewallRule", orphan),
                "case {case:?}: expected this service's not-found wording, got {:?}",
                answer.text("message")
            );
        }
        let survivor = emulator.get_rule(&kept_id).await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: the other resolver's rule survives, got {} with {}",
            survivor.status, survivor.body
        );
    }
}
