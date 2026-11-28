// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Global Resolver / DNS View slice, driven over the
//! service's own `restJson1` wire protocol through the same `MultiServiceRouter`
//! the production emulator dispatches with.
//!
//! Every test asserts a RULE over a set of inputs: the recorded shapes in the
//! issue are one row each.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53globalresolver_ec2dnsglobalresolvercustomerapi as resolver_service;
use smithy_server_runtime::MultiServiceRouter;

// The refusal BODIES of this slice — the not-found wording and members, and how a
// validation message quotes a `@pattern` — as a child module of this suite, in
// `tests/all/global_resolver_lifecycle/`. This crate's one test target declares each
// suite DIRECTLY under `tests/all/` from `tests/all.rs`; a child module of an
// already-declared suite is declared by its parent, here, and reuses this file's
// emulator. The `#[path]` is relative to this file's own directory.
#[path = "global_resolver_lifecycle/refusal_bodies.rs"]
mod refusal_bodies;

// The TAG family of this service — the three operations that address their resource by an
// ARN the caller supplies — as a second child module of this suite, in the same directory
// and reusing this file's emulator for the same reason.
#[path = "global_resolver_lifecycle/resource_tagging.rs"]
mod resource_tagging;

// The CORE NETWORK ENDPOINT resource — the five operations that attach a Cloud WAN core
// network to a Global Resolver — as a third child module of this suite, in the same
// directory and reusing this file's emulator for the same reason.
#[path = "global_resolver_lifecycle/core_network_endpoints.rs"]
mod core_network_endpoints;

// The `publicIpStatus` of a Global Resolver — the public IP connectivity a create resolves
// and every read reports — as a fourth child module of this suite, in the same directory
// and reusing this file's emulator for the same reason.
#[path = "global_resolver_lifecycle/public_ip_status.rs"]
mod public_ip_status;

/// The Region the request's SigV4 credential scope names. Route 53 Global
/// Resolver's own docs tell callers to use US East (Ohio).
const REGION: &str = "us-east-2";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "route53globalresolver";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator holding exactly this service, dispatched over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

/// A dispatched response: status, the `x-amzn-errortype` header (empty on
/// success) and the parsed JSON body.
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
        self.member(name)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn list(&self, name: &str) -> Vec<String> {
        self.member(name)
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
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

    async fn create_resolver(&self, body: Value) -> Answer {
        self.call("POST", "/global-resolver", Some(body)).await
    }

    async fn get_resolver(&self, id: &str) -> Answer {
        self.call("GET", &format!("/global-resolver/{id}"), None).await
    }

    async fn list_resolvers(&self, query: &str) -> Answer {
        self.call("GET", &format!("/global-resolver{query}"), None).await
    }

    async fn delete_resolver(&self, id: &str) -> Answer {
        self.call("DELETE", &format!("/global-resolver/{id}"), None)
            .await
    }

    async fn create_view(&self, resolver_id: &str, body: Value) -> Answer {
        self.call("POST", &format!("/dns-views/{resolver_id}"), Some(body))
            .await
    }

    async fn get_view(&self, view_id: &str) -> Answer {
        self.call("GET", &format!("/dns-views/{view_id}"), None).await
    }

    async fn list_views(&self, resolver_id: &str, query: &str) -> Answer {
        self.call(
            "GET",
            &format!("/dns-views/resolver/{resolver_id}{query}"),
            None,
        )
        .await
    }

    /// `TagResource` for `resource_arn`, carrying `tags` as the request's map.
    async fn tag(&self, resource_arn: &str, tags: Value) -> Answer {
        self.call(
            "POST",
            "/tag-resource",
            Some(json!({ "resourceArn": resource_arn, "tags": tags })),
        )
        .await
    }

    /// `UntagResource` for `resource_arn`, naming `tag_keys`.
    async fn untag(&self, resource_arn: &str, tag_keys: &[&str]) -> Answer {
        self.call(
            "POST",
            "/untag-resource",
            Some(json!({ "resourceArn": resource_arn, "tagKeys": tag_keys })),
        )
        .await
    }

    /// `ListTagsForResource` for `resource_arn`.
    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.call(
            "POST",
            "/get-all-tags",
            Some(json!({ "resourceArn": resource_arn })),
        )
        .await
    }
}

/// `ResourceId`'s pattern, `^[-.a-zA-Z0-9]+$`, as a predicate.
fn is_resource_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// One row of the create-and-read-back table: what the request carries, and
/// which address families the response must therefore advertise.
struct CreateRow {
    case: &'static str,
    body: Value,
    /// The `ipAddressType` the resolver resolves to, per the model's documented
    /// default of IPV4 for an absent member.
    ip_address_type: &'static str,
}

fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            case: "two regions, DUAL_STACK, every optional member present",
            body: json!({
                "name": "demo-resolver",
                "regions": ["us-east-2", "eu-west-1"],
                "ipAddressType": "DUAL_STACK",
                "description": "the issue's recorded shape",
                "observabilityRegion": "us-east-2",
                "clientToken": "tok-1"
            }),
            ip_address_type: "DUAL_STACK",
        },
        CreateRow {
            case: "one region, explicit IPV4, no optional member at all",
            body: json!({ "name": "single-region", "regions": ["us-east-2"], "ipAddressType": "IPV4" }),
            ip_address_type: "IPV4",
        },
        CreateRow {
            case: "ipAddressType absent, so the documented IPV4 default applies",
            body: json!({ "name": "defaulted", "regions": ["ap-southeast-2", "sa-east-1"] }),
            ip_address_type: "IPV4",
        },
        CreateRow {
            case: "name at the pattern's edges: one character",
            body: json!({ "name": "a", "regions": ["eu-central-1"], "ipAddressType": "DUAL_STACK" }),
            ip_address_type: "DUAL_STACK",
        },
        CreateRow {
            case: "name with the space, slash and underscore the pattern allows",
            body: json!({
                "name": "team one/resolver_2",
                "regions": ["us-west-2", "us-east-1", "eu-west-2", "ap-northeast-1"],
                "tags": { "owner": "dns-team" }
            }),
            ip_address_type: "IPV4",
        },
    ]
}

#[tokio::test]
async fn a_created_resolver_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_addresses: BTreeSet<String> = BTreeSet::new();

    for row in create_rows() {
        let created = emulator.create_resolver(row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );

        let id = created.text("id");
        assert!(
            is_resource_id(&id),
            "case {:?}: expected an id matching ^[-.a-zA-Z0-9]+$, got {id:?}",
            row.case
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: expected a fresh id, got the already-minted {id:?}",
            row.case
        );
        assert_eq!(
            created.text("arn"),
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:global-resolver/{id}"),
            "case {:?}: expected the ARN of {id}, got {:?}",
            row.case,
            created.text("arn")
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {:?}: a resolver is provisioned before it is operational, got {:?}",
            row.case,
            created.text("status")
        );
        assert_eq!(
            created.text("ipAddressType"),
            row.ip_address_type,
            "case {:?}: expected ipAddressType {}, got {:?}",
            row.case,
            row.ip_address_type,
            created.text("ipAddressType")
        );
        assert!(
            created.text("dnsName").starts_with(&id) && created.text("dnsName").len() <= 128,
            "case {:?}: expected a dnsName naming {id} within Sni's 1..128, got {:?}",
            row.case,
            created.text("dnsName")
        );

        let ipv4 = created.list("ipv4Addresses");
        assert!(
            !ipv4.is_empty(),
            "case {:?}: ipv4Addresses is required and non-empty, got {ipv4:?}",
            row.case
        );
        for address in &ipv4 {
            assert!(
                address.parse::<Ipv4Addr>().is_ok(),
                "case {:?}: expected an IPv4 address, got {address:?}",
                row.case
            );
            assert!(
                seen_addresses.insert(address.clone()),
                "case {:?}: two resolvers must not share the anycast address {address:?}",
                row.case
            );
        }

        let ipv6 = created.member("ipv6Addresses");
        if row.ip_address_type == "DUAL_STACK" {
            let ipv6 = created.list("ipv6Addresses");
            assert!(
                !ipv6.is_empty(),
                "case {:?}: DUAL_STACK must carry ipv6Addresses, got {ipv6:?}",
                row.case
            );
            for address in &ipv6 {
                assert!(
                    address.parse::<Ipv6Addr>().is_ok(),
                    "case {:?}: expected an IPv6 address, got {address:?}",
                    row.case
                );
                assert!(
                    seen_addresses.insert(address.clone()),
                    "case {:?}: two resolvers must not share the anycast address {address:?}",
                    row.case
                );
            }
        } else {
            assert!(
                ipv6.is_null(),
                "case {:?}: ipv6Addresses is populated only for DUAL_STACK, got {ipv6}",
                row.case
            );
        }

        // Every member the request supplied comes back as supplied.
        for (member, requested) in row.body.as_object().expect("an object body") {
            if member == "tags" || member == "ipAddressType" {
                continue;
            }
            assert_eq!(
                created.member(member),
                requested,
                "case {:?}: expected {member} to come back as {requested}, got {}",
                row.case,
                created.member(member)
            );
        }
        if row.body.get("clientToken").is_none() {
            assert!(
                !created.text("clientToken").is_empty(),
                "case {:?}: clientToken is required in the output, so an omitted one is minted",
                row.case
            );
        }

        // The round trip: the get answers the stored record, which is what the
        // create answered.
        let fetched = emulator.get_resolver(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected the created resolver to be gettable, got {} with {}",
            row.case, fetched.status, fetched.body
        );
        for member in [
            "id",
            "arn",
            "clientToken",
            "dnsName",
            "name",
            "description",
            "regions",
            "observabilityRegion",
            "ipAddressType",
            "ipv4Addresses",
            "ipv6Addresses",
            "createdAt",
        ] {
            assert_eq!(
                fetched.member(member),
                created.member(member),
                "case {:?}: expected the get to report the stored {member} {}, got {}",
                row.case,
                created.member(member),
                fetched.member(member)
            );
        }
    }
}

#[tokio::test]
async fn the_clock_alone_turns_a_creating_resolver_operational() {
    let emulator = Emulator::new();
    let created = emulator
        .create_resolver(json!({ "name": "provisioning", "regions": ["us-east-2"] }))
        .await;
    let id = created.text("id");
    assert_eq!(
        created.text("status"),
        "CREATING",
        "case {:?}: the create answers the non-terminal status, got {:?}",
        "create",
        created.text("status")
    );

    // The transition is owed by the clock, not by the read: nothing but time
    // passes here. A blocking sleep is deliberate — this test owns its
    // current-thread runtime and has nothing else to poll, and the workspace's
    // tokio does not enable the `time` feature `tokio::time::sleep` needs.
    std::thread::sleep(resolver_service::state::PROVISIONING_DELAY + Duration::from_millis(250));

    let fetched = emulator.get_resolver(&id).await;
    assert_eq!(
        fetched.text("status"),
        "OPERATIONAL",
        "case {:?}: expected OPERATIONAL once the provisioning span has passed, got {:?}",
        "get after the span",
        fetched.text("status")
    );
    let listed = emulator.list_resolvers("").await;
    assert_eq!(
        listed.member("globalResolvers")[0]["status"],
        json!("OPERATIONAL"),
        "case {:?}: the list reports the same settled status as the get, got {}",
        "list after the span",
        listed.member("globalResolvers")[0]["status"]
    );
}

#[tokio::test]
async fn an_idempotent_create_replayed_with_one_token_mints_one_resolver() {
    let emulator = Emulator::new();
    // Rows differ in the token's form and in how many times it is replayed.
    let rows: Vec<(&str, &str, usize)> = vec![
        ("the issue's token, replayed once", "tok-1", 2),
        ("a uuid-shaped token, replayed twice", "8f2d1c4e-0000-4000-8000-00000000abcd", 3),
        ("a 1-character token", "t", 2),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, token, attempts) in rows {
        let mut ids = BTreeSet::new();
        for attempt in 0..attempts {
            let answer = emulator
                .create_resolver(json!({
                    "name": "idempotent",
                    "regions": ["us-east-2"],
                    "clientToken": token
                }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} attempt {attempt}: expected 200, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.text("clientToken"),
                token,
                "case {case:?} attempt {attempt}: expected the supplied clientToken back, got {:?}",
                answer.text("clientToken")
            );
            ids.insert(answer.text("id"));
        }
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: one token must name one resolver, got {ids:?}"
        );
        minted.extend(ids);
    }

    // Distinct tokens are distinct resolvers, and the store holds exactly those.
    let listed = emulator.list_resolvers("").await;
    let listed_ids: Vec<String> = listed
        .member("globalResolvers")
        .as_array()
        .expect("globalResolvers is required")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    minted.sort();
    assert_eq!(
        listed_ids, minted,
        "case {:?}: expected exactly the resolvers the distinct tokens minted, got {listed_ids:?}",
        "one resolver per distinct token"
    );

    // A create with NO token is never an idempotent replay of another one.
    let first = emulator
        .create_resolver(json!({ "name": "untokened-one", "regions": ["us-east-2"] }))
        .await;
    let second = emulator
        .create_resolver(json!({ "name": "untokened-two", "regions": ["us-east-2"] }))
        .await;
    assert_ne!(
        first.text("id"),
        second.text("id"),
        "case {:?}: two tokenless creates are two resolvers, got {:?} twice",
        "no clientToken",
        first.text("id")
    );
    assert_ne!(
        first.text("clientToken"),
        second.text("clientToken"),
        "case {:?}: a minted clientToken is unique, got {:?} twice",
        "no clientToken",
        first.text("clientToken")
    );
}

#[tokio::test]
async fn a_list_page_honours_max_results_and_resumes_at_its_token() {
    let emulator = Emulator::new();
    let mut all_ids: Vec<String> = Vec::new();
    for index in 0..5 {
        let answer = emulator
            .create_resolver(json!({
                "name": format!("paged-{index}"),
                "regions": ["us-east-2"]
            }))
            .await;
        all_ids.push(answer.text("id"));
    }
    all_ids.sort();

    // Rows: every page size from one item to more than the whole collection, plus
    // the absent member, which asks for everything.
    for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(9)] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (page_size, token.as_deref()) {
                (Some(size), Some(token)) => format!("?max_results={size}&next_token={token}"),
                (Some(size), None) => format!("?max_results={size}"),
                (None, Some(token)) => format!("?next_token={token}"),
                (None, None) => String::new(),
            };
            let answer = emulator.list_resolvers(&query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} with {}",
                answer.status, answer.body
            );
            let items = answer
                .member("globalResolvers")
                .as_array()
                .expect("globalResolvers is required")
                .clone();
            if let Some(size) = page_size {
                assert!(
                    items.len() as i64 <= i64::from(size),
                    "case {case:?}: a page holds at most maxResults items, got {}",
                    items.len()
                );
            }
            walked.extend(
                items
                    .iter()
                    .map(|item| item["id"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= all_ids.len() + 2,
                "case {case:?}: pagination must terminate, walked {pages} pages"
            );
            match answer.member("nextToken") {
                Value::String(next) => token = Some(next.clone()),
                _ => break,
            }
        }
        assert_eq!(
            walked, all_ids,
            "case {case:?}: the pages must concatenate to the whole collection, got {walked:?}"
        );
    }

    // A page size below one is refused, at both the published floor and past it.
    for bad in [0, -1] {
        let answer = emulator
            .list_resolvers(&format!("?max_results={bad}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected 400 for maxResults={bad}, got {} with {}",
            "maxResults below one", answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {:?}: expected a ValidationException for maxResults={bad}, got {:?}",
            "maxResults below one",
            answer.error_type
        );
    }
}

/// One row of the DNS View toggle table: what the request carries for each of the
/// three toggles, and what each must therefore resolve to.
struct ViewRow {
    case: &'static str,
    body: Value,
    dnssec: &'static str,
    edns: &'static str,
    fail_open: &'static str,
}

#[tokio::test]
async fn a_dns_view_resolves_each_toggle_and_is_read_back() {
    let emulator = Emulator::new();
    let resolver = emulator
        .create_resolver(json!({ "name": "view-parent", "regions": ["us-east-2"] }))
        .await;
    let resolver_id = resolver.text("id");

    // The three toggles (`dnssecValidation`, `ednsClientSubnet`,
    // `firewallRulesFailOpen`) each declare DISABLED | ENABLED with @default
    // DISABLED, per smithy_query on CreateDNSView.
    let rows = vec![
        ViewRow {
            case: "the issue's shape: dnssecValidation ENABLED, the others absent",
            body: json!({ "name": "demo-view", "dnssecValidation": "ENABLED" }),
            dnssec: "ENABLED",
            edns: "DISABLED",
            fail_open: "DISABLED",
        },
        ViewRow {
            case: "every toggle absent, so every default applies",
            body: json!({ "name": "all-defaulted" }),
            dnssec: "DISABLED",
            edns: "DISABLED",
            fail_open: "DISABLED",
        },
        ViewRow {
            case: "every toggle ENABLED, with a description and a client token",
            body: json!({
                "name": "all-enabled",
                "description": "every toggle on",
                "clientToken": "view-tok-1",
                "dnssecValidation": "ENABLED",
                "ednsClientSubnet": "ENABLED",
                "firewallRulesFailOpen": "ENABLED"
            }),
            dnssec: "ENABLED",
            edns: "ENABLED",
            fail_open: "ENABLED",
        },
        ViewRow {
            case: "the middle toggle ENABLED and the outer two spelled DISABLED",
            body: json!({
                "name": "edns-only",
                "dnssecValidation": "DISABLED",
                "ednsClientSubnet": "ENABLED",
                "firewallRulesFailOpen": "DISABLED"
            }),
            dnssec: "DISABLED",
            edns: "ENABLED",
            fail_open: "DISABLED",
        },
    ];

    let mut created_ids: Vec<String> = Vec::new();
    for row in &rows {
        let created = emulator.create_view(&resolver_id, row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with {}",
            row.case, created.status, created.body
        );
        let id = created.text("id");
        assert!(
            is_resource_id(&id),
            "case {:?}: expected an id matching ^[-.a-zA-Z0-9]+$, got {id:?}",
            row.case
        );
        assert_eq!(
            created.text("globalResolverId"),
            resolver_id,
            "case {:?}: a view names its parent resolver, got {:?}",
            row.case,
            created.text("globalResolverId")
        );
        assert_eq!(
            created.text("arn"),
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:dns-view/{id}"),
            "case {:?}: expected the ARN of {id}, got {:?}",
            row.case,
            created.text("arn")
        );
        for (member, expected) in [
            ("dnssecValidation", row.dnssec),
            ("ednsClientSubnet", row.edns),
            ("firewallRulesFailOpen", row.fail_open),
        ] {
            assert_eq!(
                created.text(member),
                expected,
                "case {:?}: expected {member} {expected}, got {:?}",
                row.case,
                created.text(member)
            );
        }
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {:?}: a view is provisioned before it is operational, got {:?}",
            row.case,
            created.text("status")
        );

        let fetched = emulator.get_view(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected the created view to be gettable, got {} with {}",
            row.case, fetched.status, fetched.body
        );
        for member in [
            "id",
            "arn",
            "globalResolverId",
            "name",
            "description",
            "clientToken",
            "dnssecValidation",
            "ednsClientSubnet",
            "firewallRulesFailOpen",
            "createdAt",
        ] {
            assert_eq!(
                fetched.member(member),
                created.member(member),
                "case {:?}: expected the get to report the stored {member} {}, got {}",
                row.case,
                created.member(member),
                fetched.member(member)
            );
        }
        created_ids.push(id);
    }

    // The list reports exactly the views created under this resolver, in id order.
    let listed = emulator.list_views(&resolver_id, "").await;
    let listed_ids: Vec<String> = listed
        .member("dnsViews")
        .as_array()
        .expect("dnsViews is required")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected_ids = created_ids.clone();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "case {:?}: expected the views of {resolver_id}, got {listed_ids:?}",
        "ListDNSViews"
    );

    // Paginated the same way as the resolver list.
    let first_page = emulator.list_views(&resolver_id, "?max_results=1").await;
    assert_eq!(
        first_page.member("dnsViews").as_array().map(Vec::len),
        Some(1),
        "case {:?}: expected one item, got {}",
        "ListDNSViews maxResults=1",
        first_page.member("dnsViews")
    );
    let token = first_page.text("nextToken");
    assert!(
        !token.is_empty(),
        "case {:?}: expected a nextToken while views remain, got {}",
        "ListDNSViews maxResults=1",
        first_page.body
    );
    let second_page = emulator
        .list_views(&resolver_id, &format!("?max_results=1&next_token={token}"))
        .await;
    assert_eq!(
        second_page.member("dnsViews")[0]["id"],
        json!(expected_ids[1]),
        "case {:?}: expected the token to resume at {}, got {}",
        "ListDNSViews second page",
        expected_ids[1],
        second_page.member("dnsViews")[0]["id"]
    );

    // A toggle value the model does not declare is refused, not stored.
    for member in ["dnssecValidation", "ednsClientSubnet", "firewallRulesFailOpen"] {
        let mut body = json!({ "name": "bogus-toggle" });
        body[member] = json!("MAYBE");
        let answer = emulator.create_view(&resolver_id, body).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected 400 for an undeclared {member} value, got {} with {}",
            "undeclared toggle value", answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn views_belong_to_their_own_resolver() {
    let emulator = Emulator::new();
    let mut per_resolver: Vec<(String, Vec<String>)> = Vec::new();
    // Rows: resolvers carrying zero, one and two views.
    for (index, view_count) in [0usize, 1, 2].into_iter().enumerate() {
        let resolver = emulator
            .create_resolver(json!({
                "name": format!("owner-{index}"),
                "regions": ["us-east-2"]
            }))
            .await;
        let resolver_id = resolver.text("id");
        let mut views = Vec::new();
        for view in 0..view_count {
            let created = emulator
                .create_view(&resolver_id, json!({ "name": format!("view-{index}-{view}") }))
                .await;
            views.push(created.text("id"));
        }
        views.sort();
        per_resolver.push((resolver_id, views));
    }

    for (resolver_id, views) in &per_resolver {
        let listed = emulator.list_views(resolver_id, "").await;
        let listed_ids: Vec<String> = listed
            .member("dnsViews")
            .as_array()
            .expect("dnsViews is required")
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            &listed_ids, views,
            "case {:?}: expected only its own views, got {listed_ids:?}",
            format!("resolver {resolver_id}")
        );
    }
}

#[tokio::test]
async fn an_id_the_store_does_not_hold_is_the_modelled_404() {
    let emulator = Emulator::new();
    // Rows: every route in this slice that takes a resource id, against ids no
    // create ever minted.
    let unknown = "does-not-exist";
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("GetGlobalResolver", "GET", "/global-resolver/does-not-exist", None),
        ("DeleteGlobalResolver", "DELETE", "/global-resolver/does-not-exist", None),
        ("GetDNSView", "GET", "/dns-views/does-not-exist", None),
        ("ListDNSViews", "GET", "/dns-views/resolver/does-not-exist", None),
        (
            "CreateDNSView",
            "POST",
            "/dns-views/does-not-exist",
            Some(json!({ "name": "orphan" })),
        ),
    ];

    for (case, method, path, body) in cases {
        let answer = emulator.call(method, path, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {unknown}, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {:?} with {}",
            answer.error_type,
            answer.body
        );
    }

    // An empty `regions` list is a refusal, not a resolver.
    let refused = emulator
        .create_resolver(json!({ "name": "no-regions", "regions": [] }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected 400, got {} with {}",
        "empty regions", refused.status, refused.body
    );
    // An ipAddressType the model does not declare is a refusal too.
    let refused = emulator
        .create_resolver(json!({
            "name": "bad-family",
            "regions": ["us-east-2"],
            "ipAddressType": "IPV9"
        }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected 400, got {} with {}",
        "undeclared ipAddressType", refused.status, refused.body
    );
    let listed = emulator.list_resolvers("").await;
    assert_eq!(
        listed.member("globalResolvers"),
        &json!([]),
        "case {:?}: a refused create stores nothing, got {}",
        "refusals store nothing",
        listed.member("globalResolvers")
    );
}

#[tokio::test]
async fn one_list_call_can_never_return_the_whole_store() {
    let emulator = Emulator::new();
    let overflow = 7;
    let held = resolver_service::state::MAX_PAGE_SIZE as usize + overflow;
    let mut all_ids: Vec<String> = Vec::new();
    for index in 0..held {
        let answer = emulator
            .create_resolver(json!({
                "name": format!("capped-{index}"),
                "regions": ["us-east-2"]
            }))
            .await;
        all_ids.push(answer.text("id"));
    }
    all_ids.sort();

    // Rows: the absent member, a size at the cap, and sizes past it — each is
    // served at the cap, never with the whole store.
    for requested in [
        None,
        Some(resolver_service::state::MAX_PAGE_SIZE),
        Some(resolver_service::state::MAX_PAGE_SIZE + 1),
        Some(10_000),
    ] {
        let case = format!("maxResults={requested:?}");
        let query = requested.map_or_else(String::new, |size| format!("?max_results={size}"));
        let answer = emulator.list_resolvers(&query).await;
        let items = answer
            .member("globalResolvers")
            .as_array()
            .expect("globalResolvers is required")
            .len();
        assert_eq!(
            items,
            resolver_service::state::MAX_PAGE_SIZE as usize,
            "case {case:?}: a page is served at the cap, got {items} items"
        );
        assert!(
            matches!(answer.member("nextToken"), Value::String(_)),
            "case {case:?}: items remain, so a nextToken must resume them, got {}",
            answer.body
        );
    }

    // And the rest is still reachable by following the token.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = token
            .as_deref()
            .map_or_else(String::new, |token| format!("?next_token={token}"));
        let answer = emulator.list_resolvers(&query).await;
        walked.extend(
            answer
                .member("globalResolvers")
                .as_array()
                .expect("globalResolvers is required")
                .iter()
                .map(|item| item["id"].as_str().unwrap_or_default().to_string()),
        );
        match answer.member("nextToken") {
            Value::String(next) => token = Some(next.clone()),
            _ => break,
        }
    }
    assert_eq!(
        walked, all_ids,
        "case {:?}: following the tokens must reach every stored resolver, got {} of {}",
        "walk the capped pages",
        walked.len(),
        all_ids.len()
    );
}

#[tokio::test]
async fn a_create_past_the_stores_ceiling_is_the_modelled_402() {
    let emulator = Emulator::new();
    let mut first_id = String::new();
    let mut first_token = String::new();
    for index in 0..resolver_service::state::MAX_GLOBAL_RESOLVERS {
        let answer = emulator
            .create_resolver(json!({
                "name": format!("quota-{index}"),
                "regions": ["us-east-2"]
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: resolver {index} is within the ceiling, got {} with {}",
            "fill to the ceiling", answer.status, answer.body
        );
        if index == 0 {
            first_id = answer.text("id");
            first_token = answer.text("clientToken");
        }
    }

    let refused = emulator
        .create_resolver(json!({ "name": "one-too-many", "regions": ["us-east-2"] }))
        .await;
    assert_eq!(
        refused.status, 402,
        "case {:?}: expected 402 past the ceiling, got {} with {}",
        "one past the ceiling", refused.status, refused.body
    );
    assert!(
        refused
            .error_type
            .contains("ServiceQuotaExceededException"),
        "case {:?}: expected a ServiceQuotaExceededException, got {:?}",
        "one past the ceiling",
        refused.error_type
    );

    // An idempotent replay adds nothing, so the ceiling does not refuse it.
    let replay = emulator
        .create_resolver(json!({
            "name": "quota-0",
            "regions": ["us-east-2"],
            "clientToken": first_token
        }))
        .await;
    assert_eq!(
        replay.status, 200,
        "case {:?}: a replay adds no resource, so it is served at the ceiling, got {} with {}",
        "replay at the ceiling", replay.status, replay.body
    );
    assert_eq!(
        replay.text("id"),
        first_id,
        "case {:?}: the replay answers the resolver its token minted, got {:?}",
        "replay at the ceiling",
        replay.text("id")
    );

    // Freeing a slot makes room again.
    assert_eq!(
        emulator.delete_resolver(&first_id).await.status,
        200,
        "case {:?}: the first resolver is deletable",
        "free a slot"
    );
    let accepted = emulator
        .create_resolver(json!({ "name": "after-the-delete", "regions": ["us-east-2"] }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected 200 once a slot is free, got {} with {}",
        "free a slot", accepted.status, accepted.body
    );

    // A per-resolver ceiling bounds the views under one resolver the same way, in
    // a store of its own so the resolver ceiling above does not get in the way.
    let emulator = Emulator::new();
    let parent = emulator
        .create_resolver(json!({ "name": "view-ceiling", "regions": ["us-east-2"] }))
        .await;
    let parent_id = parent.text("id");
    for index in 0..resolver_service::state::MAX_DNS_VIEWS_PER_RESOLVER {
        let answer = emulator
            .create_view(&parent_id, json!({ "name": format!("view-{index}") }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: view {index} is within the ceiling, got {} with {}",
            "fill the views", answer.status, answer.body
        );
    }
    let refused = emulator
        .create_view(&parent_id, json!({ "name": "one-view-too-many" }))
        .await;
    assert_eq!(
        refused.status, 402,
        "case {:?}: expected 402 past the per-resolver ceiling, got {} with {}",
        "one view past the ceiling", refused.status, refused.body
    );

    // A regions list past its ceiling is refused, not stored. Every entry is a
    // well-formed Region token, so the only rule it breaks is the count.
    let too_many: Vec<String> = (0..=resolver_service::state::MAX_REGIONS_PER_RESOLVER)
        .map(|index| format!("us-east-{}", index % 9 + 1))
        .collect();
    let refused = emulator
        .create_resolver(json!({ "name": "too-many-regions", "regions": too_many }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected 400 past the regions ceiling, got {} with {}",
        "regions past the ceiling", refused.status, refused.body
    );
}

#[tokio::test]
async fn only_a_region_aws_could_name_is_stored() {
    let emulator = Emulator::new();
    // The grammar is `[a-z]{2}(-gov|-iso[a-z]?)?-[a-z]+-\d`, the framework's own
    // published region pattern (REGION_PATTERN_IMPLEMENTED in
    // vendor/emulator-telemetry/src/backend_contract.rs). Rows: accepted tokens
    // from each branch of it, including partitions the issue never cited.
    for accepted in [
        "us-east-2",
        "eu-west-1",
        "ap-southeast-4",
        "sa-east-1",
        "us-gov-west-1",
        "us-iso-east-1",
        "us-isob-east-1",
    ] {
        let answer = emulator
            .create_resolver(json!({
                "name": "region-grammar",
                "regions": [accepted],
                "observabilityRegion": accepted
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {accepted:?}: a Region matching the grammar is accepted, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.list("regions"),
            vec![accepted.to_string()],
            "case {accepted:?}: the Region is stored as given, got {:?}",
            answer.list("regions")
        );
    }

    // Rows: a value from each way the grammar can be broken.
    for refused in [
        "not-a-region",
        "us-east",
        "us-east-22",
        "US-EAST-2",
        "us-east-2 ",
        "us--2",
        "earth-east-1",
    ] {
        let answer = emulator
            .create_resolver(json!({ "name": "bad-region", "regions": [refused] }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {refused:?}: expected 400 for a Region outside the grammar, got {} with {}",
            answer.status, answer.body
        );
        let answer = emulator
            .create_resolver(json!({
                "name": "bad-observability",
                "regions": ["us-east-2"],
                "observabilityRegion": refused
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {refused:?}: expected 400 for an observabilityRegion outside the grammar, \
             got {} with {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_token_replayed_with_different_parameters_is_the_modelled_409() {
    let emulator = Emulator::new();
    let first = emulator
        .create_resolver(json!({
            "name": "conflict-base",
            "regions": ["us-east-2"],
            "description": "the first call",
            "clientToken": "conflict-token"
        }))
        .await;
    assert_eq!(
        first.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "the first call", first.status, first.body
    );

    // Rows: one differing member each, so the mismatch is detected per member and
    // not only on the name.
    let mismatches = vec![
        (
            "a different name",
            json!({ "name": "other", "regions": ["us-east-2"], "description": "the first call", "clientToken": "conflict-token" }),
        ),
        (
            "a different region list",
            json!({ "name": "conflict-base", "regions": ["eu-west-1"], "description": "the first call", "clientToken": "conflict-token" }),
        ),
        (
            "a different description",
            json!({ "name": "conflict-base", "regions": ["us-east-2"], "description": "changed", "clientToken": "conflict-token" }),
        ),
        (
            "a different ipAddressType",
            json!({ "name": "conflict-base", "regions": ["us-east-2"], "description": "the first call", "ipAddressType": "DUAL_STACK", "clientToken": "conflict-token" }),
        ),
    ];
    for (case, body) in mismatches {
        let answer = emulator.create_resolver(body).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409 for a token reused with other parameters, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ConflictException"),
            "case {case:?}: expected a ConflictException, got {:?}",
            answer.error_type
        );
    }

    // The matching replay still answers the same resolver.
    let replay = emulator
        .create_resolver(json!({
            "name": "conflict-base",
            "regions": ["us-east-2"],
            "description": "the first call",
            "clientToken": "conflict-token"
        }))
        .await;
    assert_eq!(
        replay.text("id"),
        first.text("id"),
        "case {:?}: the matching replay answers the same resolver, got {:?}",
        "the matching replay",
        replay.text("id")
    );

    // The same rule on a DNS View, including its parent resolver.
    let resolver_id = first.text("id");
    let other = emulator
        .create_resolver(json!({ "name": "other-parent", "regions": ["us-east-2"] }))
        .await;
    let view = emulator
        .create_view(
            &resolver_id,
            json!({ "name": "conflict-view", "clientToken": "view-conflict-token" }),
        )
        .await;
    assert_eq!(
        view.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "the first view", view.status, view.body
    );
    for (case, parent, body) in [
        (
            "a different toggle",
            resolver_id.clone(),
            json!({ "name": "conflict-view", "dnssecValidation": "ENABLED", "clientToken": "view-conflict-token" }),
        ),
        (
            "a different parent resolver",
            other.text("id"),
            json!({ "name": "conflict-view", "clientToken": "view-conflict-token" }),
        ),
    ] {
        let answer = emulator.create_view(&parent, body).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409 for a view token reused with other parameters, \
             got {} with {}",
            answer.status, answer.body
        );
    }
    let replay = emulator
        .create_view(
            &resolver_id,
            json!({ "name": "conflict-view", "clientToken": "view-conflict-token" }),
        )
        .await;
    assert_eq!(
        replay.text("id"),
        view.text("id"),
        "case {:?}: the matching view replay answers the same view, got {:?}",
        "the matching view replay",
        replay.text("id")
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let resolver = emulator
        .create_resolver(json!({ "name": "token-shapes", "regions": ["us-east-2"] }))
        .await;
    let resolver_id = resolver.text("id");
    emulator
        .create_view(&resolver_id, json!({ "name": "a-view" }))
        .await;

    // Rows: a garbage token, a token of the other list's kind, and a token with a
    // body that is not the id form this service mints.
    for bad in ["garbage", "dnsv-000000000001", "gr-", "gr-not-hex", "0"] {
        let answer = emulator
            .list_resolvers(&format!("?next_token={bad}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {bad:?}: expected 400 for a nextToken this service never issued, got {} with {}",
            answer.status, answer.body
        );
    }
    for bad in ["garbage", "gr-000000000001", "dnsv-", "dnsv-zzz"] {
        let answer = emulator
            .list_views(&resolver_id, &format!("?next_token={bad}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {bad:?}: expected 400 for a nextToken this service never issued, got {} with {}",
            answer.status, answer.body
        );
    }
    // And the token the service DID issue is accepted.
    let page = emulator.list_resolvers("?max_results=1").await;
    assert_eq!(
        page.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "the issued token", page.status, page.body
    );
}

#[tokio::test]
async fn a_deleted_resolver_and_its_views_are_gone_for_the_next_reader() {
    let emulator = Emulator::new();
    // Rows: a resolver with no view, and one with two views, so the cascade is
    // exercised and the no-child case is not special-cased.
    for view_count in [0usize, 2] {
        let case = format!("{view_count} view(s)");
        let keeper = emulator
            .create_resolver(json!({ "name": "keeper", "regions": ["us-east-2"] }))
            .await;
        let keeper_id = keeper.text("id");
        let doomed = emulator
            .create_resolver(json!({
                "name": "doomed",
                "regions": ["us-east-2", "eu-west-1"],
                "ipAddressType": "DUAL_STACK"
            }))
            .await;
        let doomed_id = doomed.text("id");
        let mut view_ids = Vec::new();
        for view in 0..view_count {
            let created = emulator
                .create_view(&doomed_id, json!({ "name": format!("doomed-view-{view}") }))
                .await;
            view_ids.push(created.text("id"));
        }

        let deleted = emulator.delete_resolver(&doomed_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("status"),
            "DELETING",
            "case {case:?}: a delete answers the final status, got {:?}",
            deleted.text("status")
        );
        for member in ["id", "arn", "name", "regions", "ipv4Addresses", "ipv6Addresses", "createdAt"] {
            assert_eq!(
                deleted.member(member),
                doomed.member(member),
                "case {case:?}: the delete answers the stored {member} {}, got {}",
                doomed.member(member),
                deleted.member(member)
            );
        }

        assert_eq!(
            emulator.get_resolver(&doomed_id).await.status,
            404,
            "case {case:?}: the deleted resolver must be gone for the next reader"
        );
        assert_eq!(
            emulator.delete_resolver(&doomed_id).await.status,
            404,
            "case {case:?}: a second delete of {doomed_id} is a 404"
        );
        for view_id in &view_ids {
            assert_eq!(
                emulator.get_view(view_id).await.status,
                404,
                "case {case:?}: view {view_id} goes with its resolver"
            );
        }
        assert_eq!(
            emulator.list_views(&doomed_id, "").await.status,
            404,
            "case {case:?}: listing the views of a deleted resolver is a 404"
        );

        // The delete took nothing else with it.
        let listed = emulator.list_resolvers("").await;
        let listed_ids: Vec<String> = listed
            .member("globalResolvers")
            .as_array()
            .expect("globalResolvers is required")
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed_ids,
            vec![keeper_id.clone()],
            "case {case:?}: expected only {keeper_id} to survive, got {listed_ids:?}"
        );
        assert_eq!(
            emulator.delete_resolver(&keeper_id).await.status,
            200,
            "case {case:?}: the surviving resolver is still deletable"
        );
    }
}
