// SPDX-License-Identifier: CC0-1.0

//! The shared harness every suite in this crate drives the service through.
//!
//! One in-process emulator behind a real `restJson1` request/response walk: a test
//! asserts on the HTTP answer only, never by reaching into `crate::state`, so a handler
//! that answered from a hardcoded value instead of from the store fails. Every suite
//! (`asset_lifecycle`, `asset_types`, `domain_project_lifecycle`, `domain_unit_hierarchy`,
//! `glossary_lifecycle`, `update_project`) reuses this one instead of copying it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The span `crate::state::LIFECYCLE_SETTLE_MS` gives a domain lifecycle step, plus
/// slack, so a test that waits for a settled domain is not racing the clock.
pub(crate) const SETTLED: std::time::Duration = std::time::Duration::from_millis(600);

/// A GENEROUS deadline for a transition the clock owes a domain: far longer than
/// `crate::state::LIFECYCLE_SETTLE_MS`, so a loaded host still reaches it. A wait is for the
/// CONDITION; this only bounds how long it is looked for.
const LIFECYCLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two looks at a condition the clock owes.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The access key id the calls in these suites present unless a case names another: a
/// well-formed key id no IAM record names, so a request carrying it speaks for the one
/// principal this emulator publishes.
pub(crate) const DEFAULT_ACCESS_KEY_ID: &str = "AKIAEXAMPLETESTKEY";

/// One emulator: a single shared engine behind the DataZone service, driven over HTTP.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

/// What an anonymous FETCH of a URL this emulator published answered: a client holding such
/// a URL follows it with no credential and no target header at all, exactly as a browser
/// does.
pub(crate) struct Fetched {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: String,
}

/// What a call answered: the HTTP status, the parsed body (`Value::Null` for an empty
/// one) and the wire error code when the answer was a failure.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) error_code: String,
}

impl Answer {
    /// A member of the response body as a string, or `"<absent>"` so an assertion
    /// message says which member was missing.
    pub(crate) fn s(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    }
}

/// The `domainExecutionRole` the creates in these suites name.
///
/// DataZone requires the member though the model leaves it optional, so every create has to
/// carry one; this is a role ARN of this account, which is what `RoleArn`'s modelled pattern
/// admits. No IAM is mounted beside DataZone here, so nothing is shown about the role and the
/// create is admitted — the resolution itself is exercised in
/// [`crate::domain_execution_role`], which mounts IAM too.
pub(crate) fn domain_execution_role() -> String {
    ::engine::emulator_arn("iam", "", "role/service-role/AmazonDataZoneDomainExecution")
}

/// `body` as a `CreateDomain` request that names an execution role: the body as written when
/// it already names one, so a case about the member itself still states it.
pub(crate) fn with_domain_execution_role(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        object
            .entry("domainExecutionRole")
            .or_insert_with(|| json!(domain_execution_role()));
    }
    body
}

/// `count` distinct accounts for an account pool's `accountSource`, each with one supported
/// region: the account ids are twelve digits (`AwsAccountId`, `^\d{12}$`) built from the
/// index.
///
/// Shared by the account-pool suites ([`crate::account_pools`],
/// [`crate::update_account_pool`]) so the fixture cannot drift between them.
pub(crate) fn accounts(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "awsAccountId": format!("{:012}", 100_000_000_000u64 + index as u64),
                "supportedRegions": ["us-east-1"],
            })
        })
        .collect()
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::served_at(smithy_server_runtime::UNSERVED_URL_BASE)
    }

    /// Send one `CreateDomain`, filling in the [`domain_execution_role`] when `body` does
    /// not name one: a suite that is not about that member still has to carry it.
    pub(crate) async fn create_domain(&self, body: Value) -> Answer {
        self.call(
            "POST",
            "/v2/domains",
            Some(with_domain_execution_role(body)),
        )
        .await
    }

    /// One emulator with DataZone AND IAM mounted on one shared engine, plus that engine.
    ///
    /// A domain create resolves the roles it names against this account's own IAM over the
    /// emulator's in-process callback, so IAM has to be MOUNTED for it to answer at all —
    /// which is why every other suite here, mounting DataZone alone, still creates domains
    /// with roles no IAM record backs.
    pub(crate) fn with_iam() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_datazone_datazone::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        (Self { router }, engine)
    }

    /// One emulator with DataZone alone, plus the engine it is mounted on.
    ///
    /// A suite reaches for the engine only when its FIXTURE needs a resource no
    /// implemented operation creates yet (`crate::subscription_request_decisions` needs a
    /// subscription target, whose create is still a scaffold). Every ASSERTION there is
    /// still made on the HTTP answer alone.
    pub(crate) fn with_engine() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_datazone_datazone::EmulatedService::new(Arc::clone(&engine)),
        );
        (Self { router }, engine)
    }

    /// One emulator whose own endpoint is `base`: the base URL
    /// `OperationContext::endpoint` reports, which is what the data port publishes when the
    /// emulator is served on one and what every URL a handler hands out therefore names.
    pub(crate) fn served_at(base: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_datazone_datazone::EmulatedService::new(engine),
        );
        router.set_url_base(
            smithy_server_runtime::EndpointUrl::parse(base)
                .unwrap_or_else(|| panic!("{base:?} is a usable endpoint base")),
        );
        Self { router }
    }

    /// Follow `url` the way a client that was handed it does: the address as published,
    /// carrying no credential, no target header and no body.
    pub(crate) async fn fetch(&self, method: &str, url: &str) -> Fetched {
        let parsed: http::Uri = url.parse().expect("a published URL parses as a URI");
        let authority = parsed
            .authority()
            .map(|authority| authority.as_str().to_string())
            .unwrap_or_default();
        let target = parsed
            .path_and_query()
            .map(|path| path.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());
        let request = http::Request::builder()
            .method(method)
            .uri(target)
            .header("host", authority)
            .body(bytes::Bytes::new())
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        Fetched {
            status,
            content_type,
            body,
        }
    }

    /// Send one request. The `Authorization` header carries a SigV4 credential scope,
    /// which is both how the router picks this service and where the handlers read the
    /// caller's region and principal from.
    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.call_as(DEFAULT_ACCESS_KEY_ID, method, uri, body).await
    }

    /// Send one request whose credential scope names `access_key_id`.
    ///
    /// The key id is what every service resolves the CALLING PRINCIPAL from, so this is how
    /// a suite calls as an identity other than the account's own: a key id the account's
    /// IAM holds for one of its users speaks for that user, and one no store holds a record
    /// of speaks for the principal this emulator publishes ([`DEFAULT_ACCESS_KEY_ID`]).
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map(|b| serde_json::to_vec(&b).expect("a JSON body serializes"));
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "datazone.us-west-2.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20261021/us-west-2/datazone/aws4_request, SignedHeaders=host, Signature=0"
                ),
            );
        if payload.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(bytes::Bytes::from(payload.unwrap_or_default()))
            .expect("a valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if error_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            error_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    /// Create a domain and wait until it has settled to `AVAILABLE`, which is the
    /// precondition every project case needs.
    pub(crate) async fn available_domain(&self, name: &str) -> String {
        let created = self.create_domain(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "creating domain {name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.s("id");
        self.await_domain_status(&id, "AVAILABLE").await;
        id
    }

    /// Look again until `GetDomain` reports `domain` in `status`, or a generous deadline
    /// passes. Waits for the CONDITION the clock owes, never for a fixed time.
    pub(crate) async fn await_domain_status(&self, domain: &str, status: &str) -> Answer {
        let started = Instant::now();
        loop {
            let answer = self.call("GET", &format!("/v2/domains/{domain}"), None).await;
            if answer.status == 200 && answer.s("status") == status {
                return answer;
            }
            assert!(
                started.elapsed() < LIFECYCLE_DEADLINE,
                "domain {domain:?} never reached {status:?}: last answer {} {:?}",
                answer.status,
                answer.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Look again until the domain a delete removed is GONE — the identifier names no
    /// domain any more, so even its own read is refused — or a generous deadline passes.
    pub(crate) async fn await_domain_gone(&self, domain: &str) -> Answer {
        let started = Instant::now();
        loop {
            let answer = self.call("GET", &format!("/v2/domains/{domain}"), None).await;
            if answer.status != 200 {
                return answer;
            }
            assert!(
                started.elapsed() < LIFECYCLE_DEADLINE,
                "domain {domain:?} was never removed: last answer {} {:?}",
                answer.status,
                answer.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Look again until `GetListing` reports `listing` in `status`, or a generous deadline
    /// passes. Waits for the CONDITION the clock owes — the publish a change set started —
    /// never for a fixed time, and answers the listing as it then stands.
    pub(crate) async fn await_listing_status(
        &self,
        domain: &str,
        listing: &str,
        status: &str,
    ) -> Answer {
        let started = Instant::now();
        loop {
            let answer = self
                .call("GET", &format!("/v2/domains/{domain}/listings/{listing}"), None)
                .await;
            if answer.status == 200 && answer.s("status") == status {
                return answer;
            }
            assert!(
                started.elapsed() < LIFECYCLE_DEADLINE,
                "listing {listing:?} never reached {status:?}: last answer {} {:?}",
                answer.status,
                answer.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Create a project in a settled domain — the precondition every asset case needs.
    pub(crate) async fn project(&self, domain_id: &str, name: &str) -> String {
        let created = self
            .call(
                "POST",
                &format!("/v2/domains/{domain_id}/projects"),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "creating project {name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        created.s("id")
    }
}

/// Attribute the stored project `project` of `domain` to the DataZone user
/// `user_profile_id`, as though that user had created it.
///
/// A FIXTURE seam, shared by every suite that needs a project the CALLER does not own: a
/// project is attributed to the DataZone user that created it, and this emulator
/// authenticates every caller as one user, so no wire call records a project another user
/// owns. Nothing is asserted from the store.
pub(crate) fn attribute_project_to(
    engine: &Arc<engine::InMemoryEngine>,
    domain: &str,
    project: &str,
    user_profile_id: &str,
) {
    let state = service_com_amazonaws_datazone_datazone::state::service_state(&**engine)
        .expect("the DataZone store is reachable");
    let mut store = state.lock();
    let stored = store
        .domains
        .get_mut(domain)
        .and_then(|domain| domain.projects.get_mut(project))
        .expect("the fixture's project is stored");
    stored.created_by = user_profile_id.to_string();
}
