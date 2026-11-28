// SPDX-License-Identifier: CC0-1.0

//! The DNS Firewall domain list slice of this service's state contract, driven
//! end-to-end over the service's own `restJson1` wire through the same
//! `MultiServiceRouter` the production emulator dispatches with.
//!
//! Every test states a RULE over a set of inputs rather than one recorded shape:
//! a create is observable to the next `ListFirewallDomainLists` with the members
//! the store holds, a create the service REFUSED is observable to nobody, and a
//! page is scoped, ordered and resumable.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_route53globalresolver_ec2dnsglobalresolvercustomerapi as resolver_service;
use smithy_server_runtime::MultiServiceRouter;

// The domain MEMBERSHIP of a list, and the delete of the list itself, as a child
// module of this suite in `tests/all/state_roundtrip/firewall_domain_lists/`. A
// child module of an already-declared suite is declared by its parent, here, and
// reuses this file's emulator. The `#[path]` is relative to this file's own
// directory.
#[path = "firewall_domain_lists/domains.rs"]
mod domains;

/// The Region the request's SigV4 credential scope names. Route 53 Global
/// Resolver's own docs tell callers to use US East (Ohio).
const REGION: &str = "us-east-2";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "route53globalresolver";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// How long a poll for a status the clock owes keeps looking. Generous on
/// purpose: the test waits for the CONDITION, and a loaded host only makes it
/// look again.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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
        self.member(name).as_str().unwrap_or_default().to_string()
    }

    /// The MODELLED members this answer's body carries, sorted. `__type` is the
    /// protocol's own error discriminator rather than a member of the shape, so it
    /// is left out: what is left is exactly what the service put on the wire.
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

    /// The `firewallDomainLists` ids this answer carries, in the order they came.
    fn listed_ids(&self) -> Vec<String> {
        self.member("firewallDomainLists")
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `firewallDomainLists` item with this id, if the answer carries one.
    fn listed_item(&self, id: &str) -> Option<Value> {
        self.member("firewallDomainLists")
            .as_array()?
            .iter()
            .find(|item| item["id"].as_str() == Some(id))
            .cloned()
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

    /// `CreateGlobalResolver` — the parent every domain list hangs off.
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

    async fn delete_resolver(&self, resolver_id: &str) -> Answer {
        self.call("DELETE", &format!("/global-resolver/{resolver_id}"), None)
            .await
    }

    async fn create_list(&self, resolver_id: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/firewall-domain-lists/{resolver_id}"),
            Some(body),
        )
        .await
    }

    async fn list_lists(&self, query: &str) -> Answer {
        self.call("GET", &format!("/firewall-domain-lists{query}"), None)
            .await
    }

    async fn get_list(&self, list_id: &str) -> Answer {
        self.call("GET", &format!("/firewall-domain-lists/{list_id}"), None)
            .await
    }

    /// Every id the store holds for this scope, walked across as many pages as it
    /// takes, so an assertion about the whole collection is never a one-page one.
    async fn walk(&self, query: &str) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let page = match token.as_deref() {
                Some(token) => {
                    self.list_lists(&format!("{query}{separator}next_token={token}"))
                        .await
                }
                None => self.list_lists(query).await,
            };
            assert_eq!(
                page.status, 200,
                "walking {query:?}: expected 200, got {} with {}",
                page.status, page.body
            );
            ids.extend(page.listed_ids());
            token = page.member("nextToken").as_str().map(str::to_string);
            if token.is_none() {
                return ids;
            }
        }
    }
}

/// One row of the create table: the request body, and what the store should hold
/// for it.
struct CreateRow {
    case: &'static str,
    body: Value,
    description: Option<&'static str>,
}

#[tokio::test]
async fn a_created_domain_list_is_reported_by_the_list_member_for_member() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("list-parent").await;

    // Rows differ along what the create request may carry: the optional
    // `description`, the optional `clientToken`, the optional `tags`, and the
    // forms `ResourceName`'s own pattern (^(?!^[0-9]+$)([a-zA-Z0-9-_/' ']+)$, from
    // smithy_query on CreateFirewallDomainList) admits.
    let rows = vec![
        CreateRow {
            case: "name only",
            body: json!({ "name": "blocked-domains" }),
            description: None,
        },
        CreateRow {
            case: "a description as well",
            body: json!({ "name": "allowed_domains", "description": "the allow list" }),
            description: Some("the allow list"),
        },
        CreateRow {
            case: "a caller-supplied clientToken",
            body: json!({ "name": "tokened/list", "clientToken": "fdl-token-1" }),
            description: None,
        },
        CreateRow {
            case: "tags, which no member of this output carries",
            body: json!({
                "name": "tagged list",
                "description": "tagged",
                "tags": { "team": "dns" }
            }),
            description: Some("tagged"),
        },
        CreateRow {
            case: "a name of digits and letters, which the pattern admits",
            body: json!({ "name": "list123" }),
            description: None,
        },
    ];

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let created = emulator.create_list(&resolver_id, row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with {}",
            row.case, created.status, created.body
        );
        let id = created.text("id");
        assert!(
            id.starts_with(resolver_service::state::FIREWALL_DOMAIN_LIST_ID_PREFIX),
            "case {:?}: a minted id carries this service's own prefix, got {id:?}",
            row.case
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: every create mints its own id, got {id:?} twice",
            row.case
        );
        assert_eq!(
            created.text("status"),
            "CREATING",
            "case {:?}: the create answers the non-terminal status, got {:?}",
            row.case,
            created.text("status")
        );
        assert_eq!(
            created.member("domainCount"),
            &json!(0),
            "case {:?}: a new list holds no domains, got {:?}",
            row.case,
            created.member("domainCount")
        );
        assert_eq!(
            created.text("arn"),
            format!(
                "arn:aws:{}:{REGION}:{ACCOUNT_ID}:{}/{id}",
                resolver_service::state::ARN_SERVICE,
                resolver_service::state::FIREWALL_DOMAIN_LIST_RESOURCE
            ),
            "case {:?}: the arn names this resource in the request's Region, got {:?}",
            row.case,
            created.text("arn")
        );
        assert_eq!(
            created.member("description").as_str(),
            row.description,
            "case {:?}: expected description {:?}, got {:?}",
            row.case,
            row.description,
            created.member("description")
        );

        // The round trip: the list reports the stored record, so every member the
        // create answered comes back unchanged.
        let listed = emulator.list_lists("").await;
        let item = listed.listed_item(&id).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the list to report {id:?}, got {:?}",
                row.case,
                listed.member("firewallDomainLists")
            )
        });
        for member in ["id", "arn", "name", "description", "globalResolverId", "createdAt"] {
            assert_eq!(
                item.get(member).unwrap_or(&Value::Null),
                created.member(member),
                "case {:?}: member {member:?} read back as {:?}, created as {:?}",
                row.case,
                item.get(member).unwrap_or(&Value::Null),
                created.member(member)
            );
        }
        assert_eq!(
            item["globalResolverId"].as_str(),
            Some(resolver_id.as_str()),
            "case {:?}: the list is reported under its own parent, got {:?}",
            row.case,
            item["globalResolverId"]
        );

        // The same record, read by id: a create is observable to the next get as
        // well as to the next list.
        let fetched = emulator.get_list(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected 200 reading {id:?} back, got {} with {}",
            row.case, fetched.status, fetched.body
        );
        for member in [
            "id",
            "arn",
            "name",
            "description",
            "globalResolverId",
            "createdAt",
            "domainCount",
        ] {
            assert_eq!(
                fetched.member(member),
                created.member(member),
                "case {:?}: member {member:?} fetched as {:?}, created as {:?}",
                row.case,
                fetched.member(member),
                created.member(member)
            );
        }
        assert!(
            !fetched.text("clientToken").is_empty(),
            "case {:?}: every stored list carries the clientToken of the call that \
             minted it, got {:?}",
            row.case,
            fetched.member("clientToken")
        );
    }

    let walked = emulator.walk("").await;
    let expected: Vec<String> = seen_ids.iter().cloned().collect();
    assert_eq!(
        walked, expected,
        "case {:?}: expected every created list in id order, got {walked:?}",
        "the whole collection"
    );
}

/// One row of the refusal table: a create this service must refuse, and the
/// status and error type it refuses it with.
struct RefusalRow {
    case: &'static str,
    resolver_id: Option<&'static str>,
    body: Value,
    status: u16,
    error_type: &'static str,
}

#[tokio::test]
async fn a_refused_create_leaves_nothing_for_the_next_list() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("refusal-parent").await;
    let kept = emulator
        .create_list(&resolver_id, json!({ "name": "the-one-that-sticks" }))
        .await;
    assert_eq!(
        kept.status, 200,
        "setup: the first create must succeed, got {} with {}",
        kept.status, kept.body
    );
    let kept_ids = vec![kept.text("id")];

    // Rows: one per way a create is refused — an unknown parent, and each bound
    // `CreateFirewallDomainList`'s own members declare (name 1..64 and its
    // pattern, description 1..256), from smithy_query on the operation.
    let long_name = "n".repeat(65);
    let long_description = "d".repeat(257);
    let rows = vec![
        RefusalRow {
            case: "a parent id the store does not hold",
            resolver_id: Some("gr-ffffffffffff"),
            body: json!({ "name": "orphan" }),
            status: 404,
            error_type: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "a name past its 64-character bound",
            resolver_id: None,
            body: json!({ "name": long_name }),
            status: 400,
            error_type: "ValidationException",
        },
        RefusalRow {
            case: "an empty description, under its 1-character bound",
            resolver_id: None,
            body: json!({ "name": "empty-description", "description": "" }),
            status: 400,
            error_type: "ValidationException",
        },
        RefusalRow {
            case: "an empty name, under its 1-character bound",
            resolver_id: None,
            body: json!({ "name": "" }),
            status: 400,
            error_type: "ValidationException",
        },
        RefusalRow {
            case: "no name at all, which the model requires",
            resolver_id: None,
            body: json!({ "description": "nameless" }),
            status: 400,
            error_type: "ValidationException",
        },
        RefusalRow {
            case: "a description past its 256-character bound",
            resolver_id: None,
            body: json!({ "name": "long-description", "description": long_description }),
            status: 400,
            error_type: "ValidationException",
        },
    ];

    for row in rows {
        let parent = row.resolver_id.unwrap_or(resolver_id.as_str());
        let refused = emulator.create_list(parent, row.body.clone()).await;
        assert_eq!(
            refused.status, row.status,
            "case {:?}: expected {}, got {} with {}",
            row.case, row.status, refused.status, refused.body
        );
        assert!(
            refused.error_type.contains(row.error_type),
            "case {:?}: expected a {}, got {:?} with {}",
            row.case,
            row.error_type,
            refused.error_type,
            refused.body
        );
        assert_eq!(
            emulator.walk("").await,
            kept_ids,
            "case {:?}: a refused create leaves the collection as it was",
            row.case
        );
    }

    // A `clientToken` spent on a refused create minted nothing, so the next call
    // carrying it is served on its own merits rather than from a record of the
    // refusal.
    let refused = emulator
        .create_list(
            "gr-ffffffffffff",
            json!({ "name": "masked", "clientToken": "reused-after-a-refusal" }),
        )
        .await;
    assert_eq!(
        refused.status, 404,
        "case {:?}: expected 404 for an unknown parent, got {} with {}",
        "the token's first call", refused.status, refused.body
    );
    let accepted = emulator
        .create_list(
            &resolver_id,
            json!({ "name": "masked", "clientToken": "reused-after-a-refusal" }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "the token's second call", accepted.status, accepted.body
    );
    let mut expected = kept_ids.clone();
    expected.push(accepted.text("id"));
    expected.sort();
    assert_eq!(
        emulator.walk("").await,
        expected,
        "case {:?}: the accepted create is the only one the collection gained",
        "the token's second call"
    );
}

#[tokio::test]
async fn a_page_is_scoped_to_the_resolver_the_caller_names() {
    let emulator = Emulator::new();
    // Rows: resolvers carrying zero, one and three domain lists, so neither the
    // empty scope nor the many-item one is special-cased.
    let mut per_resolver: Vec<(String, Vec<String>)> = Vec::new();
    for (index, list_count) in [0usize, 1, 3].into_iter().enumerate() {
        let resolver_id = emulator.create_resolver(&format!("owner-{index}")).await;
        let mut ids = Vec::new();
        for list in 0..list_count {
            let created = emulator
                .create_list(
                    &resolver_id,
                    json!({ "name": format!("list-{index}-{list}") }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "setup: list {list} under resolver {index} must be created, got {} with {}",
                created.status, created.body
            );
            ids.push(created.text("id"));
        }
        ids.sort();
        per_resolver.push((resolver_id, ids));
    }

    for (resolver_id, ids) in &per_resolver {
        let scoped = emulator
            .walk(&format!("?global_resolver_id={resolver_id}"))
            .await;
        assert_eq!(
            &scoped, ids,
            "case {:?}: a scoped page holds exactly that resolver's lists, got {scoped:?}",
            format!("scope {resolver_id}")
        );
    }

    let mut every_id: Vec<String> = per_resolver
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();
    every_id.sort();
    assert_eq!(
        emulator.walk("").await,
        every_id,
        "case {:?}: an unscoped page holds every list the store has",
        "no globalResolverId"
    );

    let unknown = emulator
        .list_lists("?global_resolver_id=gr-ffffffffffff")
        .await;
    assert_eq!(
        unknown.status, 404,
        "case {:?}: expected 404 for a parent the store does not hold, got {} with {}",
        "an unknown scope", unknown.status, unknown.body
    );
    assert!(
        unknown.error_type.contains("ResourceNotFoundException"),
        "case {:?}: expected a ResourceNotFoundException, got {:?}",
        "an unknown scope",
        unknown.error_type
    );
}

#[tokio::test]
async fn a_page_honours_max_results_and_resumes_at_its_token() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("paging-parent").await;
    let mut all_ids: Vec<String> = Vec::new();
    for index in 0..7 {
        let created = emulator
            .create_list(&resolver_id, json!({ "name": format!("paged-{index}") }))
            .await;
        all_ids.push(created.text("id"));
    }
    all_ids.sort();

    // Rows: every page size from one item to more than the whole collection, plus
    // the absent member, which asks for everything.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(11)] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = String::new();
            if let Some(size) = page_size {
                query.push_str(&format!("?max_results={size}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("next_token={token}"));
            }
            let page = emulator.list_lists(&query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200, got {} with {}",
                page.status, page.body
            );
            let ids = page.listed_ids();
            if let Some(size) = page_size {
                assert!(
                    ids.len() <= size.min(resolver_service::state::MAX_PAGE_SIZE as usize),
                    "case {case:?}: a page never exceeds the size asked for, got {}",
                    ids.len()
                );
            }
            walked.extend(ids);
            token = page.member("nextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all_ids,
            "case {case:?}: the walked pages are the whole collection in id order, got {walked:?}"
        );
    }

    // Rows: a page size no page can serve.
    for refused_size in [0, -1, -7] {
        let answer = emulator
            .list_lists(&format!("?max_results={refused_size}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected 400 for maxResults={refused_size}, got {} with {}",
            "a non-positive maxResults", answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {:?}: expected a ValidationException for maxResults={refused_size}, got {:?}",
            "a non-positive maxResults",
            answer.error_type
        );
    }

    // Rows: a cursor this service never issued — garbage, another list's kind of
    // id, and the right prefix with a body that is not the form it mints.
    for bad in ["garbage", "dnsv-000000000001", "fdl-", "fdl-not-hex", "0"] {
        let answer = emulator.list_lists(&format!("?next_token={bad}")).await;
        assert_eq!(
            answer.status, 400,
            "case {bad:?}: expected 400 for a token this service never issued, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {bad:?}: expected a ValidationException, got {:?}",
            answer.error_type
        );
    }
}

/// One row of the missing-parent table: the operation's name, its HTTP method,
/// the request it makes against a `globalResolverId`, and the message the service
/// words that refusal with.
struct ParentRoute {
    operation: &'static str,
    method: &'static str,
    request: fn(&str) -> (String, Option<Value>),
    message: fn(&str) -> String,
}

/// The body of a not-found refusal of a resource addressed by its id: the
/// service's own error code, the kind the id was looked up as, and the id the
/// request named.
fn not_found_by_id(kind: &str, id: &str) -> String {
    format!("[GR-ERR00309] No {kind} found with the ID: {id}")
}

#[tokio::test]
async fn a_global_resolver_id_that_names_nothing_is_worded_one_way_everywhere() {
    let emulator = Emulator::new();
    // One row per route in this slice that resolves a `globalResolverId`, so the
    // refusal of a parent that does not exist is the same error wherever it is
    // reported — including the two operations this change implements.
    let routes = vec![
        ParentRoute {
            operation: "GetGlobalResolver",
            method: "GET",
            request: |id| (format!("/global-resolver/{id}"), None),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        ParentRoute {
            operation: "DeleteGlobalResolver",
            method: "DELETE",
            request: |id| (format!("/global-resolver/{id}"), None),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        ParentRoute {
            operation: "CreateDNSView",
            method: "POST",
            request: |id| (format!("/dns-views/{id}"), Some(json!({ "name": "a-view" }))),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        ParentRoute {
            operation: "ListDNSViews",
            method: "GET",
            request: |id| (format!("/dns-views/resolver/{id}"), None),
            // The DNS View list path words its missing parent with its OWN error
            // code, and states the id without a colon before it.
            message: |id| format!("[GR-ERR02300] No GlobalResolver found with ID {id}"),
        },
        ParentRoute {
            operation: "CreateFirewallDomainList",
            method: "POST",
            request: |id| {
                (
                    format!("/firewall-domain-lists/{id}"),
                    Some(json!({ "name": "a-list" })),
                )
            },
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        ParentRoute {
            operation: "ListFirewallDomainLists",
            method: "GET",
            request: |id| {
                (
                    format!("/firewall-domain-lists?global_resolver_id={id}"),
                    None,
                )
            },
            message: |id| not_found_by_id("GlobalResolver", id),
        },
    ];

    // Rows: ids of different forms, none of them in the store. The id in the
    // message is the one the REQUEST named, never a stored one.
    for missing in ["gr-ffffffffffff", "gr-000000000099", "no-such-resolver"] {
        for route in &routes {
            let case = format!("{} against {missing:?}", route.operation);
            let (path, body) = (route.request)(missing);
            let answer = emulator.call(route.method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.text("message"),
                (route.message)(missing),
                "case {case:?}: expected the service's own wording for a missing Global \
                 Resolver, got {:?}",
                answer.text("message")
            );
            assert_eq!(
                answer.modelled_members(),
                vec![
                    "message".to_string(),
                    "resourceId".to_string(),
                    "resourceType".to_string()
                ],
                "case {case:?}: the refusal body states the kind and the id beside the \
                 sentence, got {:?}",
                answer.modelled_members()
            );
            assert_eq!(
                answer.text("resourceId"),
                missing,
                "case {case:?}: expected the id the request named, got {:?}",
                answer.text("resourceId")
            );
            assert_eq!(
                answer.text("resourceType"),
                "GlobalResolver",
                "case {case:?}: expected the kind the id was looked up as, got {:?}",
                answer.text("resourceType")
            );
        }
    }
}

#[tokio::test]
async fn a_domain_list_id_that_names_nothing_is_the_modelled_404() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("get-parent").await;
    let created = emulator
        .create_list(&resolver_id, json!({ "name": "the-only-list" }))
        .await;
    let id = created.text("id");

    // Rows: ids of each form a caller might send — one this service could have
    // minted, one of another resource's kind, and one of no kind at all.
    for missing in ["fdl-ffffffffffff", "gr-000000000001", "no-such-list"] {
        let answer = emulator.get_list(missing).await;
        assert_eq!(
            answer.status, 404,
            "case {missing:?}: expected 404 for an id the store does not hold, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException"),
            "case {missing:?}: expected a ResourceNotFoundException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.text("message"),
            not_found_by_id("FirewallDomainList", missing),
            "case {missing:?}: expected the service's own wording naming the id the request \
             carried, got {:?}",
            answer.text("message")
        );
        assert_eq!(
            answer.modelled_members(),
            vec![
                "message".to_string(),
                "resourceId".to_string(),
                "resourceType".to_string()
            ],
            "case {missing:?}: the refusal body states the kind and the id beside the \
             sentence, got {:?}",
            answer.modelled_members()
        );
        assert_eq!(
            answer.text("resourceType"),
            "FirewallDomainList",
            "case {missing:?}: expected the kind the id was looked up as, got {:?}",
            answer.text("resourceType")
        );
    }

    assert_eq!(
        emulator.get_list(&id).await.status,
        200,
        "case {:?}: the id the store does hold still reads back",
        "the stored id"
    );
}

#[tokio::test]
async fn the_clock_alone_turns_a_creating_domain_list_operational() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("settling-parent").await;
    let created = emulator
        .create_list(&resolver_id, json!({ "name": "settling" }))
        .await;
    let id = created.text("id");
    assert_eq!(
        created.text("status"),
        "CREATING",
        "case {:?}: the create answers the non-terminal status, got {:?}",
        "the create",
        created.text("status")
    );

    // The transition is owed by the clock, not by the read: this looks again until
    // the condition holds, and nothing here asserts on how long it took.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let status = loop {
        let listed = emulator.list_lists("").await;
        let item = listed.listed_item(&id).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the list to report {id:?}, got {:?}",
                "the settled read",
                listed.member("firewallDomainLists")
            )
        });
        let status = item["status"].as_str().unwrap_or_default().to_string();
        if status == "OPERATIONAL" {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected OPERATIONAL once the provisioning span passed, got {status:?}",
            "the settled read"
        );
        std::thread::sleep(POLL_INTERVAL);
    };
    assert_eq!(
        status, "OPERATIONAL",
        "case {:?}: the clock alone settles a list, got {status:?}",
        "the settled read"
    );
}

#[tokio::test]
async fn a_deleted_resolver_takes_its_domain_lists_with_it() {
    let emulator = Emulator::new();
    // Rows: a resolver with one list and one with three, so the cascade is not a
    // single-child special case.
    for doomed_count in [1usize, 3] {
        let case = format!("{doomed_count} list(s) under the deleted resolver");
        let keeper_id = emulator.create_resolver("keeper").await;
        let doomed_id = emulator.create_resolver("doomed").await;
        let kept = emulator
            .create_list(&keeper_id, json!({ "name": "kept-list" }))
            .await;
        let kept_id = kept.text("id");
        let mut doomed_ids = Vec::new();
        for index in 0..doomed_count {
            let created = emulator
                .create_list(&doomed_id, json!({ "name": format!("doomed-{index}") }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: list {index} must be created, got {} with {}",
                created.status, created.body
            );
            doomed_ids.push(created.text("id"));
        }

        let deleted = emulator.delete_resolver(&doomed_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the resolver must delete, got {} with {}",
            deleted.status, deleted.body
        );

        let remaining = emulator.walk("").await;
        assert!(
            remaining.contains(&kept_id),
            "case {case:?}: the other resolver's list survives, got {remaining:?}"
        );
        let scoped = emulator
            .walk(&format!("?global_resolver_id={keeper_id}"))
            .await;
        assert_eq!(
            scoped,
            vec![kept_id.clone()],
            "case {case:?}: the keeper still holds exactly its own list, got {scoped:?}"
        );
        let orphans = emulator
            .list_lists(&format!("?global_resolver_id={doomed_id}"))
            .await;
        assert_eq!(
            orphans.status, 404,
            "case {case:?}: the deleted resolver names nothing, got {} with {}",
            orphans.status, orphans.body
        );
        for orphan in &doomed_ids {
            let answer = emulator.get_list(orphan).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: the deleted resolver's list {orphan:?} is gone for the next \
                 reader, got {} with {}",
                answer.status, answer.body
            );
        }

        // Clean up this row's keeper, so the next row starts from its own state.
        assert_eq!(
            emulator.delete_resolver(&keeper_id).await.status,
            200,
            "case {case:?}: the keeper must delete too"
        );
        assert!(
            emulator.walk("").await.is_empty(),
            "case {case:?}: both resolvers gone leaves no domain list behind"
        );
    }
}
