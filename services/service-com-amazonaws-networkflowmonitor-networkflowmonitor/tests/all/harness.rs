// SPDX-License-Identifier: CC0-1.0

//! The one HTTP harness this crate's integration suites share: an emulator holding
//! exactly this service, dispatched through the same `MultiServiceRouter` the
//! production router builds, plus the request shapes every suite needs.
//!
//! Every suite under `tests/all/` drives the service over its own `restJson1` wire
//! protocol through here, so no suite builds a second router, a second engine or a
//! second signing header.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkflowmonitor_networkflowmonitor as nfm_service;
use smithy_server_runtime::MultiServiceRouter;

/// This service's SigV4 signing name, which is also the router's route key.
pub const SIGNING_NAME: &str = "networkflowmonitor";

/// The canonical emulator account id every ARN carries.
pub const ACCOUNT_ID: &str = "000000000000";

/// The Region the request's SigV4 credential scope names in most tests.
/// Deliberately not `us-east-1` (the store's fallback), so an ARN built from the
/// request's scope is distinguishable from a hardcoded one.
pub const REGION: &str = "us-west-2";

/// Regions OTHER than [`REGION`] a test signs for when ONE store has to hold more
/// than one scope: a scope is a per-account/Region resource and the account holds
/// at most `state::SCOPE_QUOTA` of them per Region, so every extra scope of a
/// store is created under its own credential scope. The codes are AWS commercial
/// Region codes, and none of them is [`REGION`].
pub const EXTRA_REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "af-south-1",
    "ap-east-1",
    "ap-south-1",
    "ap-south-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "sa-east-1",
    "ap-southeast-5",
    "ap-southeast-7",
];

/// The Region the `index`-th extra scope of a store is created in. Two scopes of
/// one store must not share a Region (they would share a quota slot), so an index
/// past the catalog fails here rather than in a confusing refusal later.
pub fn extra_region(index: usize) -> &'static str {
    assert!(
        index < EXTRA_REGIONS.len(),
        "case index {index}: this catalog names {} Regions, so a store cannot hold more \
         scopes than that",
        EXTRA_REGIONS.len()
    );
    EXTRA_REGIONS[index]
}

/// Longer than `state::PROVISIONING_DELAY`, so a call after this wait is one the
/// clock already owes the terminal status to.
pub const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// One emulator holding exactly this service, dispatched over HTTP.
pub struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

/// A dispatched response: status, the `x-amzn-errortype` header (empty on success),
/// the id the response names its request by (empty when it names none) and the parsed
/// JSON body.
pub struct Answer {
    pub status: u16,
    pub error_type: String,
    pub request_id: String,
    pub body: Value,
}

impl Answer {
    pub fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    pub fn text(&self, name: &str) -> String {
        self.member(name).as_str().unwrap_or_default().to_string()
    }

    pub fn number(&self, name: &str) -> f64 {
        self.member(name).as_f64().unwrap_or(f64::NAN)
    }

    /// Whether this answer carries the error code `code`, in whichever of the two
    /// places the protocol renders it (the `x-amzn-errortype` header, or the
    /// body's `__type`).
    pub fn is_error(&self, code: &str) -> bool {
        self.error_type.contains(code) || self.text("__type").contains(code)
    }

    /// The error class this answer names, as the shape NAME alone.
    ///
    /// `__type` may be namespace-qualified (`<namespace>#<name>`) and the header may carry a
    /// trailing `:` qualifier, so a suite asserting the class the model DECLARES compares this
    /// rather than a substring of whichever rendering was used — [`Answer::is_error`] is the
    /// substring form, for a suite that only has to recognise a class it already knows.
    pub fn class(&self) -> String {
        let rendered = if self.text("__type").is_empty() {
            self.error_type.clone()
        } else {
            self.text("__type")
        };
        let after_namespace = rendered.split('#').next_back().unwrap_or(&rendered);
        after_namespace
            .split(':')
            .next()
            .unwrap_or(after_namespace)
            .trim()
            .to_string()
    }

    /// The SENTENCE a refusal words: its `message`, whole.
    ///
    /// A refused call carries the sentence the service worded and nothing after it — the
    /// envelope a client reads around it is the SDK's own rendering of facts its call
    /// already knows, and no byte of it reaches the wire — so what a suite asserting how
    /// one condition is worded compares is the message itself. The id of the request
    /// travels in the header, read as [`Answer::request_id`].
    pub fn sentence(&self) -> String {
        self.text("message")
    }
}

impl Emulator {
    pub fn new() -> Self {
        Self::new_in(REGION)
    }

    pub fn new_in(region: &str) -> Self {
        // The SINGLE shared engine, exactly as `router/src/lib.rs` builds it.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(nfm_service::EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        self.call_signed_for(&self.region, method, path, body).await
    }

    /// The same dispatch with the request signed for `region` instead of this
    /// emulator's own: how one store comes to hold a scope in more than one
    /// Region.
    pub async fn call_signed_for(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/{SIGNING_NAME}/aws4_request, \
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
        // The id this response names its request by, in the header AWS reports it in.
        let request_id = response
            .headers()
            .get("x-amzn-requestid")
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
            request_id,
            body,
        }
    }

    pub async fn create_scope(&self, body: Value) -> Answer {
        self.call("POST", "/scopes", Some(body)).await
    }

    /// `CreateScope` signed for `region`: the account's scope slot in THAT Region,
    /// so a store can hold one scope per Region without passing the quota.
    pub async fn create_scope_in(&self, region: &str, body: Value) -> Answer {
        self.call_signed_for(region, "POST", "/scopes", Some(body))
            .await
    }

    pub async fn get_scope(&self, scope_id: &str) -> Answer {
        self.call("GET", &format!("/scopes/{scope_id}"), None).await
    }

    /// `UpdateScope`: `PATCH /scopes/{scopeId}`, from the operation's `@http`
    /// trait in the model.
    pub async fn update_scope(&self, scope_id: &str, body: Value) -> Answer {
        self.call("PATCH", &format!("/scopes/{scope_id}"), Some(body))
            .await
    }

    pub async fn delete_scope(&self, scope_id: &str) -> Answer {
        self.call("DELETE", &format!("/scopes/{scope_id}"), None)
            .await
    }

    /// `ListScopes`: `GET /scopes`, with `query` carrying the `@httpQuery` members
    /// (`?maxResults=..&nextToken=..`) or empty for none.
    pub async fn list_scopes(&self, query: &str) -> Answer {
        self.call("GET", &format!("/scopes{query}"), None).await
    }

    pub async fn create_monitor(&self, body: Value) -> Answer {
        self.call("POST", "/monitors", Some(body)).await
    }

    pub async fn get_monitor(&self, name: &str) -> Answer {
        self.call("GET", &format!("/monitors/{name}"), None).await
    }

    /// `UpdateMonitor`: `PATCH /monitors/{monitorName}`, from the operation's
    /// `@http` trait in the model.
    pub async fn update_monitor(&self, name: &str, body: Value) -> Answer {
        self.call("PATCH", &format!("/monitors/{name}"), Some(body))
            .await
    }

    pub async fn list_monitors(&self, query: &str) -> Answer {
        self.call("GET", &format!("/monitors{query}"), None).await
    }

    pub async fn delete_monitor(&self, name: &str) -> Answer {
        self.call("DELETE", &format!("/monitors/{name}"), None).await
    }

    /// `CreatePerformanceMonitor`: `POST /performance-monitors`, from the
    /// operation's `@http` trait in the model.
    pub async fn create_performance_monitor(&self, body: Value) -> Answer {
        self.call("POST", "/performance-monitors", Some(body)).await
    }

    /// `GetPerformanceMonitor`: `GET /performance-monitors/{monitorName}`.
    pub async fn get_performance_monitor(&self, name: &str) -> Answer {
        self.call("GET", &format!("/performance-monitors/{name}"), None)
            .await
    }

    /// `UpdatePerformanceMonitor`: `PATCH /performance-monitors/{monitorName}`.
    pub async fn update_performance_monitor(&self, name: &str, body: Value) -> Answer {
        self.call(
            "PATCH",
            &format!("/performance-monitors/{name}"),
            Some(body),
        )
        .await
    }

    /// `DeletePerformanceMonitor`: `DELETE /performance-monitors/{monitorName}`.
    pub async fn delete_performance_monitor(&self, name: &str) -> Answer {
        self.call("DELETE", &format!("/performance-monitors/{name}"), None)
            .await
    }

    /// `ListPerformanceMonitors`: `GET /performance-monitors`, with `query`
    /// carrying the `@httpQuery` members or empty for none.
    pub async fn list_performance_monitors(&self, query: &str) -> Answer {
        self.call("GET", &format!("/performance-monitors{query}"), None)
            .await
    }

    /// `StartQueryPerformanceMonitor`: `POST
    /// /performance-monitors/{monitorName}/queries`.
    pub async fn start_performance_query(&self, name: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/performance-monitors/{name}/queries"),
            Some(body),
        )
        .await
    }

    /// `GetQueryStatusPerformanceMonitor`.
    pub async fn performance_query_status(&self, name: &str, query_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/performance-monitors/{name}/queries/{query_id}/status"),
            None,
        )
        .await
    }

    /// `GetQueryResultsPerformanceMonitor`, with `query` carrying the
    /// `@httpQuery` members or empty for none.
    pub async fn performance_query_results(
        &self,
        name: &str,
        query_id: &str,
        query: &str,
    ) -> Answer {
        self.call(
            "GET",
            &format!("/performance-monitors/{name}/queries/{query_id}/results{query}"),
            None,
        )
        .await
    }

    /// `StopQueryPerformanceMonitor`.
    pub async fn stop_performance_query(&self, name: &str, query_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/performance-monitors/{name}/queries/{query_id}"),
            None,
        )
        .await
    }

    /// `GetMonitorScopeSuggestions`: `GET /monitor-scope-suggestions`, with `query`
    /// carrying the `@httpQuery` members (`monitorArn` is `@required`).
    pub async fn monitor_scope_suggestions(&self, query: &str) -> Answer {
        self.call("GET", &format!("/monitor-scope-suggestions{query}"), None)
            .await
    }

    /// A scope holding one target, and the `scopeArn` it was minted with — the
    /// parent every monitor test needs.
    ///
    /// It carries a fixed `clientToken`, so a test that needs the parent scope
    /// more than once is answered with the SAME scope (the `@idempotent` replay)
    /// rather than asking for a second one the account's quota has no slot for.
    pub async fn scope_for_monitors(&self) -> String {
        let created = self
            .create_scope(json!({
                "targets": targets(&[("123456789012", "us-east-1")]),
                "clientToken": client_token(0),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "the parent scope must be created: got {} with body {}",
            created.status, created.body
        );
        created.text("scopeArn")
    }
}

/// Whether `value` is one of the UUID-shaped ids this service mints: 8-4-4-4-12
/// lowercase hex.
///
/// Neither `ScopeId` nor `queryId` carries a pattern, but both are minted
/// UUID-shaped (`state::scope_id`, `state::query_id`), so every suite that asserts
/// a minted id asserts it through this one predicate.
pub fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// A `clientToken` matching `UuidString`'s pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, derived from
/// `seed` so each one in a test is distinct.
pub fn client_token(seed: u32) -> String {
    format!("{seed:08x}-abcd-4ef0-8123-{seed:012x}")
}

/// `targets` as the wire carries them: a `TargetResource` per (account, Region)
/// pair, with the only `targetType` the model declares (`ACCOUNT`) and the only
/// `TargetId` member it declares (`accountId`).
pub fn targets(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(account_id, region)| {
                json!({
                    "targetIdentifier": {
                        "targetId": { "accountId": account_id },
                        "targetType": "ACCOUNT"
                    },
                    "region": region
                })
            })
            .collect(),
    )
}
