// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service domain lifecycle: `CreateDomain`,
//! `DescribeDomain`, `DescribeDomainConfig`, `ListDomainNames`, `UpdateDomainConfig`
//! and `DeleteDomain`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the domain
//! name constraint over its published length bounds and one past each plus the
//! shapes its pattern refuses, `EngineType` over the whole `EngineType` catalog and
//! one value past it, and the option-merge rule over a table of eight different
//! option containers (a structure, a string, two maps, an enum) — each row both
//! applied and dry-run.
//!
//! The wire harness below ([`Emulator`], [`Answer`] and the helpers around them) is
//! this crate's ONE test harness: a sibling suite reaches it as
//! `super::domain_lifecycle::…` rather than growing a second copy.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::{
    ComAmazonawsOpensearchAmazonopensearchserviceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The API version every path of this service is prefixed with.
pub(crate) const PREFIX: &str = "/2021-01-01";

/// The Region every request below is signed for — what a domain's ARN names.
pub(crate) const REGION: &str = "us-east-1";

/// The access key id a request carries unless a suite names another: the long-term key
/// this emulator publishes, which names no session and so speaks for the account's own
/// identity.
pub(crate) const DEFAULT_ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// Long enough for the LONGEST span this emulator owes a clock move (the 3 s work on
/// a domain takes — its provisioning, a configuration change, an engine version
/// upgrade or a service software deployment — and the 3 s a change to a service-managed
/// VPC endpoint takes; every other span of this service is shorter) to have fallen due,
/// so the next read materialises the settled record.
pub(crate) const SETTLE_WAIT: Duration = Duration::from_millis(3_300);

/// The HTTP status this service's RECORDED wire answer carries EVERY missing-resource
/// refusal on, whatever status the error shape's own `@httpError(409)` declares.
///
/// Stated here, independently of the constant the implementation decides it by, so an
/// expectation asserts the recorded status rather than whatever the handler currently
/// answers: a change to that constant moves no expectation with it.
pub(crate) const RECORDED_NOT_FOUND_STATUS: u16 = 404;

/// One emulator with this service mounted on a fresh shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service holds, so a suite can read or set a
    /// precondition in this service's store directly — the way a handler reaches it.
    engine: std::sync::Arc<engine::InMemoryEngine>,
}

/// One answered request: the HTTP status, the `x-amzn-errortype` error code when the
/// answer is a failure, and the parsed JSON body (`Value::Null` for an empty body).
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error: Option<String>,
    pub(crate) body: Value,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise the service, not SigV4: requests carry a credential scope
        // so the router can name the target service, but no real signature.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(std::sync::Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator with the Cognito user-pool service mounted on the SAME shared
    /// engine, so a suite can create the user pool a domain's Dashboards authentication
    /// names through that service's own `CreateUserPool` — the store this service
    /// resolves a `CognitoOptions$UserPoolId` in — rather than by hand-building a pool
    /// record.
    pub(crate) fn with_user_pools() -> Self {
        let mut emulator = Self::new();
        emulator.router.register_service(
            service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
                ::EmulatedService::new(std::sync::Arc::clone(&emulator.engine)),
        );
        emulator
    }

    /// The same emulator with the Amazon CloudWatch Logs service mounted on the SAME
    /// shared engine, so a suite can create the log group a domain's logs are published
    /// to through that service's own `CreateLogGroup` — the store this service resolves
    /// a `LogPublishingOption$CloudWatchLogsLogGroupArn` in — rather than by hand-building
    /// a log-group record.
    pub(crate) fn with_log_groups() -> Self {
        let mut emulator = Self::new();
        emulator.router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(
                std::sync::Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// The same emulator with the AWS KMS service mounted on the SAME shared engine, so a
    /// suite can create, disable and schedule the deletion of the keys a domain's
    /// encryption at rest names through that service's OWN operations — the store this
    /// service resolves an `EncryptionAtRestOptions$KmsKeyId` in — rather than by
    /// hand-building a key record.
    pub(crate) fn with_kms() -> Self {
        let mut emulator = Self::new();
        emulator.router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(std::sync::Arc::clone(
                &emulator.engine,
            )),
        );
        emulator
    }

    /// The same emulator with the Certificate Manager service mounted on the SAME shared
    /// engine, so a suite can bring the certificate a domain's custom endpoint is served
    /// under into that service through its own `ImportCertificate` — the store this service
    /// resolves a `DomainEndpointOptions$CustomEndpointCertificateArn` in — rather than by
    /// hand-building a certificate record.
    pub(crate) fn with_certificates() -> Self {
        let mut emulator = Self::new();
        emulator.router.register_service(
            service_com_amazonaws_acm_certificatemanager::EmulatedService::new(
                std::sync::Arc::clone(&emulator.engine),
            ),
        );
        emulator
    }

    /// This emulator's store, reached exactly as a handler reaches it. A suite uses it
    /// to put the record into a state no implemented operation can reach yet — the
    /// service software a landed deployment leaves behind, say, whose deploying
    /// operation is still a stub.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsOpensearchAmazonopensearchserviceState> {
        service_com_amazonaws_opensearch_amazonopensearchservice::state::service_state(&*self.engine)
            .expect("the shared engine holds this service's store")
    }

    /// The SAME engine the mounted service holds, so a suite can seed the store of
    /// ANOTHER service mounted on it — the STS session and IAM policies an
    /// authorization decision is read from.
    pub(crate) fn engine(&self) -> &std::sync::Arc<engine::InMemoryEngine> {
        &self.engine
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.call_as(DEFAULT_ACCESS_KEY_ID, method, uri, body).await
    }

    /// The same request, signed with `access_key_id` — the credential whose principal
    /// the service resolves the caller from.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "es.us-east-1.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/es/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            )
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// Answer a request the suite built itself — the seam a suite reaching ANOTHER
    /// service mounted on this router drives its own protocol through, so the answer is
    /// read the same way whichever service served it.
    pub(crate) async fn dispatch(&self, request: http::Request<Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// The same dispatch, answering the body as the BYTES it was served as — for a surface
    /// whose answer is not JSON, which [`Self::dispatch`] can only report as absent.
    pub(crate) async fn dispatch_bytes(&self, request: http::Request<Bytes>) -> (u16, String) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(crate) async fn create(&self, body: Value) -> Answer {
        self.call("POST", &format!("{PREFIX}/opensearch/domain"), Some(body))
            .await
    }

    pub(crate) async fn describe(&self, name: &str) -> Answer {
        self.call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}"),
            None,
        )
        .await
    }

    pub(crate) async fn describe_config(&self, name: &str) -> Answer {
        self.call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/config"),
            None,
        )
        .await
    }

    pub(crate) async fn update_config(&self, name: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/config"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn delete(&self, name: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{name}"),
            None,
        )
        .await
    }

    async fn list(&self, engine_type: Option<&str>) -> Answer {
        let uri = match engine_type {
            None => format!("{PREFIX}/domain"),
            Some(value) => format!("{PREFIX}/domain?engineType={value}"),
        };
        self.call("GET", &uri, None).await
    }

    /// Call `operation` of the Amazon CloudWatch Logs service mounted on the same router
    /// ([`Emulator::with_log_groups`]), the way an SDK drives it: `POST /` with
    /// `X-Amz-Target: Logs_20140328.<Op>`, signed for that service so the router can
    /// name it.
    pub(crate) async fn logs_call(&self, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("logs.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={DEFAULT_ACCESS_KEY_ID}/20260101/{REGION}/logs/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// Create the log group `name` through the CloudWatch Logs service's own
    /// `CreateLogGroup`, so the destination ARN naming it resolves in the store this
    /// service reads an enabled log-publishing option against.
    pub(crate) async fn created_log_group(&self, name: &str) {
        let answer = self
            .logs_call("CreateLogGroup", json!({ "logGroupName": name }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case CreateLogGroup({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    /// Delete the log group `name` through the CloudWatch Logs service's own
    /// `DeleteLogGroup`, so its ARN names no group afterwards.
    pub(crate) async fn deleted_log_group(&self, name: &str) {
        let answer = self
            .logs_call("DeleteLogGroup", json!({ "logGroupName": name }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case DeleteLogGroup({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    /// Call `operation` of the AWS KMS service mounted on the same router
    /// ([`Emulator::with_kms`]) in `region`, the way an SDK drives it: `POST /` with
    /// `X-Amz-Target: TrentService.<Op>`, signed for that service in that Region — keys
    /// are regional, so the Region a call is signed for is the Region its key lives in.
    pub(crate) async fn kms_call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kms.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={DEFAULT_ACCESS_KEY_ID}/20260101/{region}/kms/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// Create a customer managed key in `region` through KMS's own `CreateKey`, answering
    /// the key ARN it minted — the value an `EncryptionAtRestOptions$KmsKeyId` names it by.
    pub(crate) async fn created_key(&self, region: &str) -> String {
        let answer = self.kms_call(region, "CreateKey", json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case CreateKey({region:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        answer.body["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case CreateKey({region:?}): expected a key ARN"))
            .to_string()
    }

    /// Move the key `key_id` of `region` through one of KMS's own key-state operations
    /// (`DisableKey`, `EnableKey`, `ScheduleKeyDeletion`), so what this service resolves is
    /// the state that service wrote.
    pub(crate) async fn kms_key_operation(&self, region: &str, operation: &str, body: Value) {
        let answer = self.kms_call(region, operation, body).await;
        assert_eq!(
            answer.status, 200,
            "case {operation}({region:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    /// Call `operation` of the Certificate Manager service mounted on the same router
    /// ([`Emulator::with_certificates`]) in `region`, the way an SDK drives it: `POST /`
    /// with `X-Amz-Target: CertificateManager.<Op>`, signed for that service in that
    /// Region — a certificate is a regional resource, so the Region a call is signed for is
    /// the Region its certificate lives in.
    pub(crate) async fn acm_call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("acm.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("CertificateManager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={DEFAULT_ACCESS_KEY_ID}/20260101/{region}/acm/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        self.dispatch(request).await
    }
}

/// The ARN of the CloudWatch Logs group `name` in this account and Region — the value
/// `LogPublishingOption$CloudWatchLogsLogGroupArn` carries (`LogGroup$logGroupArn`).
pub(crate) fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        ::engine::emulator_account_id()
    )
}

/// The log group the `LogPublishingOptions` row of [`change_rows`] publishes to. An
/// enabled option's destination has to be a group that exists, so a suite driving that
/// row creates it first ([`Emulator::created_log_group`]).
pub(crate) const CHANGE_LOG_GROUP: &str = "/aws/opensearch/slow";

/// The wire error code of a failed answer: the `x-amzn-errortype` header when the
/// path that answered sets one, else the `__type` of the error body.
pub(crate) fn error_code(answer: &Answer) -> String {
    if let Some(code) = answer.error.as_deref() {
        return code.to_string();
    }
    let raw = answer.body["__type"].as_str().unwrap_or_default();
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// A required string member of a response body, with the case named on failure.
pub(crate) fn string_member(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member}, got {body:?}"))
        .to_owned()
}

/// One option's `UpdateVersion`, from the `*Status` container `DomainConfig` reports
/// it in.
fn option_version(config: &Value, option: &str, case: &str) -> i64 {
    config[option]["Status"]["UpdateVersion"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected {option}.Status.UpdateVersion, got {:?}",
                config[option]
            )
        })
}

/// One option's `OptionState`, from the `*Status` container `DomainConfig` reports it
/// in.
fn option_state(config: &Value, option: &str, case: &str) -> String {
    config[option]["Status"]["State"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected {option}.Status.State, got {:?}",
                config[option]
            )
        })
        .to_owned()
}

/// The option containers every domain in the merge tables is created with, each at
/// revision 1. One row per kind of value an option carries: a structure, a policy
/// string, two maps keyed by a free-form key and by a `LogType`, and an enum.
fn baseline_options() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ClusterConfig",
            json!({ "InstanceType": "r6g.large.search", "InstanceCount": 2 }),
        ),
        (
            "EBSOptions",
            json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 }),
        ),
        ("AccessPolicies", json!("{\"Version\":\"2012-10-17\"}")),
        (
            "AdvancedOptions",
            json!({ "rest.action.multi.allow_explicit_index": "true" }),
        ),
        (
            "SnapshotOptions",
            json!({ "AutomatedSnapshotStartHour": 0 }),
        ),
        ("IPAddressType", json!("ipv4")),
        (
            "LogPublishingOptions",
            json!({ "SEARCH_SLOW_LOGS": { "Enabled": false } }),
        ),
        (
            "SoftwareUpdateOptions",
            json!({ "AutoSoftwareUpdateEnabled": false }),
        ),
    ]
}

/// One change to make: the option it names, the container to send, and the
/// `DryRunResults$DeploymentType` applying it is — `Blue/Green` where it changes the
/// shape of the provisioned cluster, `DynamicUpdate` where it is a setting on the
/// running one.
///
/// The log-publishing row enables publishing to [`CHANGE_LOG_GROUP`], so a suite that
/// drives these rows mounts the CloudWatch Logs service ([`Emulator::with_log_groups`])
/// and creates that group first.
pub(crate) fn change_rows() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "ClusterConfig",
            json!({ "InstanceType": "r6g.xlarge.search", "InstanceCount": 4 }),
            "Blue/Green",
        ),
        (
            "EBSOptions",
            json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 40 }),
            "Blue/Green",
        ),
        (
            "AccessPolicies",
            json!("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
            "DynamicUpdate",
        ),
        (
            "AdvancedOptions",
            json!({
                "rest.action.multi.allow_explicit_index": "false",
                "indices.fielddata.cache.size": "40",
            }),
            "DynamicUpdate",
        ),
        (
            "SnapshotOptions",
            json!({ "AutomatedSnapshotStartHour": 3 }),
            "DynamicUpdate",
        ),
        ("IPAddressType", json!("dualstack"), "Blue/Green"),
        (
            "LogPublishingOptions",
            json!({
                // The destination has to be a log group that EXISTS, so a suite driving
                // this row creates [`CHANGE_LOG_GROUP`] first.
                "INDEX_SLOW_LOGS": {
                    "Enabled": true,
                    "CloudWatchLogsLogGroupArn": log_group_arn(CHANGE_LOG_GROUP),
                },
            }),
            "DynamicUpdate",
        ),
        (
            "SoftwareUpdateOptions",
            json!({
                "AutoSoftwareUpdateEnabled": true,
                "UseLatestServiceSoftwareForBlueGreen": true,
            }),
            "DynamicUpdate",
        ),
    ]
}

/// The engine release a `CreateDomain` that names no `EngineVersion` runs:
/// `CreateDomainRequest$EngineVersion` documents "Defaults to the latest version of
/// OpenSearch", so it is the newest OpenSearch release of the catalog the service
/// publishes. Read off that catalog, so an expectation follows the catalog rather than
/// naming a release that goes stale as it grows.
pub(crate) fn default_engine_version() -> &'static str {
    service_com_amazonaws_opensearch_amazonopensearchservice::state::published_engine_versions()
        .into_iter()
        .find(|candidate| {
            service_com_amazonaws_opensearch_amazonopensearchservice::state::engine_type_of(
                candidate,
            ) == Some("OpenSearch")
        })
        .expect("the published catalog carries an OpenSearch release")
}

/// The release NUMBER of that version — the `3.7` of `OpenSearch_3.7` — which is what
/// the cluster's own APIs report as the engine they run.
pub(crate) fn default_engine_release() -> &'static str {
    default_engine_version()
        .split_once('_')
        .map(|(_, release)| release)
        .expect("a published version is <engine>_<release>")
}

/// The container the service HOLDS for `option` once a request has named `sent`: the
/// members the caller sent, completed with the value the service fills in for a member
/// the request did not name.
///
/// The service provisions a domain with a WHOLE configuration, so a request that names
/// part of an option block is stored with the rest of that block filled in; the rows
/// below are the blocks the tables in this suite send, with the defaults the service
/// documents or is recorded setting for them.
pub(crate) fn completed(option: &str, sent: &Value) -> Value {
    let defaults: Value = match option {
        "ClusterConfig" => json!({
            "ColdStorageOptions": { "Enabled": false },
            "DedicatedMasterEnabled": false,
            "MultiAZWithStandbyEnabled": false,
            "WarmEnabled": false,
            "ZoneAwarenessEnabled": false,
        }),
        "AdvancedOptions" => json!({
            "override_main_response_version": "false",
            "rest.action.multi.allow_explicit_index": "true",
        }),
        "SoftwareUpdateOptions" => json!({
            "AutoSoftwareUpdateEnabled": false,
            "UseLatestServiceSoftwareForBlueGreen": true,
        }),
        // `gp3` is the one volume type whose performance is provisioned per volume, so
        // it is the one whose baseline IOPS and throughput the service fills in.
        "EBSOptions" if sent["VolumeType"] == json!("gp3") => {
            json!({ "Iops": 3000, "Throughput": 125 })
        }
        _ => json!({}),
    };
    let mut completed = sent.clone();
    for (member, value) in defaults
        .as_object()
        .expect("every row above is a JSON object")
    {
        if completed.get(member).is_none() {
            completed[member] = value.clone();
        }
    }
    completed
}

/// A `CreateDomain` body for `name` carrying every [`baseline_options`] container.
pub(crate) fn baseline_create(name: &str) -> Value {
    let mut body = json!({ "DomainName": name, "EngineVersion": "OpenSearch_2.11" });
    for (option, value) in baseline_options() {
        body[option] = value;
    }
    body
}

/// How long [`settled_domain`] waits for a domain's provisioning to fall due before it
/// gives up.
///
/// Generous on purpose: the span itself is the service's, and a case that waits for a
/// settled domain is not testing how long that takes — it waits for the CONDITION, and
/// this is only the point at which waiting is called a failure.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long [`settled_domain`] waits between two looks at a domain.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait until `name` reports itself created with nothing in flight — the condition a
/// client polls for — and answer the `DomainStatus` that read saw.
///
/// Polls the condition rather than sleeping for the provisioning span, so a case that
/// needs a settled domain cannot depend on how fast the host runs.
pub(crate) async fn settled_domain(emulator: &Emulator, name: &str) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let answer = emulator.describe(name).await;
        let status = answer.body["DomainStatus"].clone();
        if status["Created"].as_bool() == Some(true)
            && status["Processing"].as_bool() == Some(false)
        {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case DescribeDomain({name:?}): the domain never reported itself settled, last \
             read {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The service software release label every domain this harness creates runs.
///
/// A release of the service software is built FOR one engine version, so the label an
/// OpenSearch domain reports names that version before the release it is a build of; the
/// bodies above create every domain on `OpenSearch_2.11`.
pub(crate) const BASELINE_SERVICE_SOFTWARE: &str = "OpenSearch_2_11_R20250101";

/// `CreateDomain` with [`baseline_create`], asserting it was accepted.
pub(crate) async fn create_baseline(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator.create(baseline_create(name)).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

#[tokio::test]
async fn a_created_domain_is_persisted_and_read_back_member_for_member() {
    let emulator = Emulator::new();
    let name = "my-domain";
    let created = create_baseline(&emulator, name).await;

    assert_eq!(
        created["DomainName"].as_str(),
        Some(name),
        "case CreateDomain({name:?}): expected the requested DomainName, got {:?}",
        created["DomainName"]
    );
    assert_eq!(
        created["DomainId"].as_str(),
        Some(format!("{}/{name}", engine::emulator_account_id()).as_str()),
        "case CreateDomain({name:?}): expected DomainId <account>/<name>, got {:?}",
        created["DomainId"]
    );
    assert_eq!(
        created["ARN"].as_str(),
        Some(
            format!(
                "arn:aws:es:us-east-1:{}:domain/{name}",
                engine::emulator_account_id()
            )
            .as_str()
        ),
        "case CreateDomain({name:?}): expected the domain ARN, got {:?}",
        created["ARN"]
    );
    assert_eq!(
        (
            created["Created"].as_bool(),
            created["Deleted"].as_bool(),
            created["Processing"].as_bool()
        ),
        (Some(true), Some(false), Some(true)),
        "case CreateDomain({name:?}): a new domain is created, not deleted and still \
         processing, got {created:?}"
    );
    assert_eq!(
        created["DomainProcessingStatus"].as_str(),
        Some("Creating"),
        "case CreateDomain({name:?}): expected DomainProcessingStatus Creating, got {:?}",
        created["DomainProcessingStatus"]
    );

    // A domain that is still being provisioned publishes no address for its data plane:
    // `Endpoint` is the host index and search requests go to, and there is no cluster
    // behind it yet. The address it publishes ONCE IT IS UP, and that the address is
    // served, is the rule [`super::domain_endpoint_publication`] asserts.
    assert!(
        created["Endpoint"].is_null()
            && created["EndpointV2"].is_null()
            && created["Endpoints"].is_null(),
        "case CreateDomain(endpoint): a domain still Creating publishes no address, got {created:?}"
    );

    // Defaults the model documents: `MIXED` when no use case is given, `GENERAL`
    // when no engine mode is.
    assert_eq!(
        (created["UseCase"].as_str(), created["EngineMode"].as_str()),
        (Some("MIXED"), Some("GENERAL")),
        "case CreateDomain(defaults): expected UseCase MIXED and EngineMode GENERAL, got {created:?}"
    );

    // The read reports the persisted record, not a body synthesised from the
    // request: every member the create answered comes back identical.
    let fetched = emulator.describe(name).await;
    assert_eq!(
        fetched.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        fetched.status, fetched.body
    );
    let described = &fetched.body["DomainStatus"];
    for member in [
        "DomainId",
        "DomainName",
        "ARN",
        "Created",
        "Deleted",
        "Endpoint",
        "EngineVersion",
        "ClusterConfig",
        "EBSOptions",
        "AccessPolicies",
        "AdvancedOptions",
        "SnapshotOptions",
        "IPAddressType",
        "LogPublishingOptions",
        "SoftwareUpdateOptions",
        "UseCase",
        "EngineMode",
    ] {
        assert_eq!(
            described[member], created[member],
            "case DescribeDomain({member}): expected the persisted {:?}, got {:?}",
            created[member], described[member]
        );
    }

    // A read does not change what the next caller sees.
    let again = emulator.describe(name).await;
    assert_eq!(
        again.body["DomainStatus"], *described,
        "case DescribeDomain(twice): a read must not change the record: {:?} then {:?}",
        described, again.body["DomainStatus"]
    );

    // Every option the create carried is at revision 1 and reports a real
    // `OptionStatus` built from the record's own history.
    let config = emulator.describe_config(name).await;
    assert_eq!(
        config.status, 200,
        "case DescribeDomainConfig({name:?}): expected 200, got {} with {:?}",
        config.status, config.body
    );
    let config = &config.body["DomainConfig"];
    for (option, value) in baseline_options() {
        assert_eq!(
            config[option]["Options"],
            completed(option, &value),
            "case DescribeDomainConfig({option}): expected the persisted {value:?} completed with \
             the service's own defaults, got {:?}",
            config[option]["Options"]
        );
        assert_eq!(
            option_version(config, option, option),
            2,
            "case DescribeDomainConfig({option}): a create records the configuration it was \
             asked for and then the one it provisions the domain with, so a created option is \
             at revision 2, got {:?}",
            config[option]["Status"]
        );
        assert!(
            config[option]["Status"]["CreationDate"].is_number()
                && config[option]["Status"]["UpdateDate"].is_number(),
            "case DescribeDomainConfig({option}): expected CreationDate/UpdateDate timestamps, \
             got {:?}",
            config[option]["Status"]
        );
    }
    // An option the request named no value for is reported with the value the SERVICE
    // sets for it: the domain is provisioned with a whole configuration, so a caller
    // diffing desired configuration against actual reads a container for every option.
    for (option, options) in [
        ("VPCOptions", json!({})),
        ("CognitoOptions", json!({ "Enabled": false })),
        ("EncryptionAtRestOptions", json!({ "Enabled": false })),
    ] {
        assert_eq!(
            config[option]["Options"], options,
            "case DescribeDomainConfig({option}): an option the create named no value for is \
             reported with the service's own default {options:?}, got {:?}",
            config[option]
        );
    }
}

#[tokio::test]
async fn a_domain_name_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    // `DomainName` is `@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\\-]+$")`:
    // each published bound and one past it, plus one row per character class the
    // pattern refuses.
    let twenty_eight = "a".repeat(28);
    let twenty_nine = "a".repeat(29);
    let cases: Vec<(&str, bool)> = vec![
        ("abc", true),
        ("a-b", true),
        ("a0-z9", true),
        (twenty_eight.as_str(), true),
        ("ab", false),
        ("a", false),
        (twenty_nine.as_str(), false),
        ("1abc", false),
        ("-abc", false),
        ("Abc", false),
        ("a_bc", false),
        ("a.bc", false),
        ("abc!", false),
    ];

    let emulator = Emulator::new();
    for (name, accepted) in cases {
        let answer = emulator
            .create(json!({ "DomainName": name, "ClusterConfig": { "InstanceCount": 1 } }))
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case CreateDomain({name:?}): expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            let read = emulator.describe(name).await;
            assert_eq!(
                read.status, 200,
                "case DescribeDomain({name:?}): an accepted name is persisted, got {} with {:?}",
                read.status, read.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case CreateDomain({name:?}): expected 400 outside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case CreateDomain({name:?}): expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn engine_type_is_derived_from_the_engine_version_prefix() {
    // `EngineType` has exactly two values, and `VersionString`'s pattern allows
    // exactly two prefixes; the rows span both prefixes over several versions, plus
    // the row where the request names no version at all.
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("opensearch-one", Some("OpenSearch_1.0"), "OpenSearch"),
        ("opensearch-two", Some("OpenSearch_2.11"), "OpenSearch"),
        ("opensearch-three", Some("OpenSearch_3.1"), "OpenSearch"),
        (
            "elasticsearch-seven",
            Some("Elasticsearch_7.10"),
            "Elasticsearch",
        ),
        (
            "elasticsearch-six",
            Some("Elasticsearch_6.8"),
            "Elasticsearch",
        ),
        (
            "elasticsearch-five",
            Some("Elasticsearch_5.1"),
            "Elasticsearch",
        ),
        ("default-version", None, "OpenSearch"),
    ];

    let emulator = Emulator::new();
    for (name, version, _) in &cases {
        let mut body = json!({ "DomainName": name });
        if let Some(version) = version {
            body["EngineVersion"] = json!(version);
        }
        let answer = emulator.create(body).await;
        assert_eq!(
            answer.status, 200,
            "case CreateDomain({name:?}, {version:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // A request that names no version gets the LATEST OpenSearch release the
        // service publishes — `EngineVersion` documents "Defaults to the latest version
        // of OpenSearch" — and that stored value is what the engine type is derived
        // from.
        let stored = answer.body["DomainStatus"]["EngineVersion"].clone();
        assert_eq!(
            stored.as_str(),
            Some(version.unwrap_or_else(default_engine_version)),
            "case CreateDomain({name:?}, {version:?}): expected the stored EngineVersion, got {stored:?}"
        );
    }

    // Unfiltered, every domain is listed with the engine type its version implies.
    let all = emulator.list(None).await;
    assert_eq!(
        all.status, 200,
        "case ListDomainNames(no filter): expected 200, got {} with {:?}",
        all.status, all.body
    );
    let listed = all.body["DomainNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case ListDomainNames(no filter): expected DomainNames, got {:?}", all.body))
        .clone();
    assert_eq!(
        listed.len(),
        cases.len(),
        "case ListDomainNames(no filter): expected every created domain, got {listed:?}"
    );
    for (name, version, engine_type) in &cases {
        let entry = listed
            .iter()
            .find(|entry| entry["DomainName"].as_str() == Some(*name))
            .unwrap_or_else(|| {
                panic!("case ListDomainNames({name:?}): expected an entry, got {listed:?}")
            });
        assert_eq!(
            entry["EngineType"].as_str(),
            Some(*engine_type),
            "case ListDomainNames({name:?}, {version:?}): expected EngineType {engine_type:?}, \
             got {entry:?}"
        );
    }

    // Filtered, the listing holds exactly the domains of that engine type.
    for engine_type in ["OpenSearch", "Elasticsearch"] {
        let filtered = emulator.list(Some(engine_type)).await;
        assert_eq!(
            filtered.status, 200,
            "case ListDomainNames({engine_type}): expected 200, got {} with {:?}",
            filtered.status, filtered.body
        );
        let names: Vec<String> = filtered.body["DomainNames"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case ListDomainNames({engine_type}): expected DomainNames, got {:?}",
                    filtered.body
                )
            })
            .iter()
            .map(|entry| entry["DomainName"].as_str().unwrap_or_default().to_owned())
            .collect();
        let mut expected: Vec<String> = cases
            .iter()
            .filter(|(_, _, kind)| *kind == engine_type)
            .map(|(name, _, _)| (*name).to_owned())
            .collect();
        expected.sort();
        let mut got = names.clone();
        got.sort();
        assert_eq!(
            got, expected,
            "case ListDomainNames({engine_type}): expected {expected:?}, got {names:?}"
        );
    }

    // One value past the catalog is refused, not served as an empty page.
    let bogus = emulator.list(Some("Lucene")).await;
    assert_eq!(
        bogus.status, 400,
        "case ListDomainNames(Lucene): expected 400 outside the EngineType catalog, got {} with {:?}",
        bogus.status, bogus.body
    );
    assert_eq!(
        error_code(&bogus),
        "ValidationException",
        "case ListDomainNames(Lucene): expected ValidationException, got {:?}",
        bogus.body
    );
}

#[tokio::test]
async fn an_updated_option_replaces_only_itself_and_bumps_only_its_own_revision() {
    let emulator = Emulator::with_log_groups();
    // The log-publishing row publishes to a log group, which has to exist before an
    // option enabling it is accepted.
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    // The domains are created first so ONE wait settles every one of them: a domain
    // that is still being provisioned has work in flight, and the update under test
    // is a further change, which such a domain refuses.
    let rows = change_rows();
    for index in 0..rows.len() {
        create_baseline(&emulator, &format!("merge-{index}")).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for (index, (option, value, _)) in rows.into_iter().enumerate() {
        let name = format!("merge-{index}");

        let case = format!("UpdateDomainConfig({option})");
        let answer = emulator
            .update_config(&name, json!({ option: value.clone() }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let answered = &answer.body["DomainConfig"];
        assert!(
            answer.body["DryRunResults"].is_null(),
            "case {case}: an applied update reports no dry-run results, got {:?}",
            answer.body["DryRunResults"]
        );

        // The update's own answer, the config read and the status read all report the
        // one stored record.
        let read_config = emulator.describe_config(&name).await;
        assert_eq!(
            read_config.status, 200,
            "case {case}: DescribeDomainConfig expected 200, got {} with {:?}",
            read_config.status, read_config.body
        );
        let stored = &read_config.body["DomainConfig"];
        let read_status = emulator.describe(&name).await;
        let status = &read_status.body["DomainStatus"];

        for (label, config) in [("answered", answered), ("stored", stored)] {
            assert_eq!(
                config[option]["Options"],
                completed(option, &value),
                "case {case} ({label}): a supplied container replaces the stored value: \
                 expected {value:?}, got {:?}",
                config[option]["Options"]
            );
            assert_eq!(
                option_version(config, option, &case),
                3,
                "case {case} ({label}): the changed option moves past the two revisions the \
                 create left it at, got {:?}",
                config[option]["Status"]
            );
            assert_eq!(
                option_state(config, option, &case),
                "Processing",
                "case {case} ({label}): the option the change is applying is Processing, got {:?}",
                config[option]["Status"]
            );
            // Every option the request did NOT name keeps its value and its revision.
            for (other, baseline) in baseline_options() {
                if other == option {
                    continue;
                }
                assert_eq!(
                    config[other]["Options"],
                    completed(other, &baseline),
                    "case {case} ({label}): {other} was not named and must keep {baseline:?}, \
                     got {:?}",
                    config[other]["Options"]
                );
                assert_eq!(
                    option_version(config, other, &case),
                    2,
                    "case {case} ({label}): {other} was not named and stays at the revision the \
                     create left it at, got {:?}",
                    config[other]["Status"]
                );
                assert_eq!(
                    option_state(config, other, &case),
                    "Active",
                    "case {case} ({label}): {other} was not named and stays Active, got {:?}",
                    config[other]["Status"]
                );
            }
        }

        assert_eq!(
            status[option],
            completed(option, &value),
            "case {case}: DescribeDomain reports the merged {option}: expected {value:?}, got {:?}",
            status[option]
        );
        assert_eq!(
            status["DomainProcessingStatus"].as_str(),
            Some("Modifying"),
            "case {case}: a domain applying a change is Modifying, got {:?}",
            status["DomainProcessingStatus"]
        );
    }
}

#[tokio::test]
async fn a_dry_run_reports_the_change_it_would_make_and_stores_nothing() {
    let emulator = Emulator::with_log_groups();
    // The log-publishing row publishes to a log group, which has to exist before an
    // option enabling it is accepted — a dry run is judged by the same rules as the
    // change it reports on.
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    let rows = change_rows();

    // One domain per row, all created up front so a single wait settles them: a dry
    // run against a domain still provisioning cannot predict a deployment type, and
    // that case is asserted separately below.
    for index in 0..rows.len() {
        create_baseline(&emulator, &format!("dry-{index}")).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for (index, (option, value, deployment)) in rows.into_iter().enumerate() {
        let name = format!("dry-{index}");
        let case = format!("UpdateDomainConfig({option}, DryRun)");
        let answer = emulator
            .update_config(
                &name,
                json!({ option: value.clone(), "DryRun": true }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["DomainConfig"][option]["Options"],
            completed(option, &value),
            "case {case}: a dry run reports the change it would make: expected {value:?}, got {:?}",
            answer.body["DomainConfig"][option]["Options"]
        );
        assert_eq!(
            answer.body["DryRunResults"]["DeploymentType"].as_str(),
            Some(deployment),
            "case {case}: expected DeploymentType {deployment:?}, got {:?}",
            answer.body["DryRunResults"]
        );

        // Nothing was stored: the option is still at its baseline value and revision.
        let stored = emulator.describe_config(&name).await;
        let config = &stored.body["DomainConfig"];
        let baseline = baseline_options()
            .into_iter()
            .find(|(candidate, _)| *candidate == option)
            .map(|(_, baseline)| baseline)
            .unwrap_or(Value::Null);
        assert_eq!(
            config[option]["Options"],
            completed(option, &baseline),
            "case {case}: a dry run must not mutate: expected the baseline {baseline:?}, got {:?}",
            config[option]["Options"]
        );
        assert_eq!(
            option_version(config, option, &case),
            2,
            "case {case}: a dry run must not bump a revision, got {:?}",
            config[option]["Status"]
        );
    }

    // "None - The request doesn't include any configuration changes."
    let empty = emulator
        .update_config("dry-0", json!({ "DryRun": true }))
        .await;
    assert_eq!(
        empty.body["DryRunResults"]["DeploymentType"].as_str(),
        Some("None"),
        "case UpdateDomainConfig(no change, DryRun): expected DeploymentType None, got {:?}",
        empty.body["DryRunResults"]
    );

    // "Undetermined - The domain is in the middle of an update and can't predict the
    // deployment type."
    create_baseline(&emulator, "dry-unsettled").await;
    let unsettled = emulator
        .update_config(
            "dry-unsettled",
            json!({ "ClusterConfig": { "InstanceCount": 4 }, "DryRun": true }),
        )
        .await;
    assert_eq!(
        unsettled.body["DryRunResults"]["DeploymentType"].as_str(),
        Some("Undetermined"),
        "case UpdateDomainConfig(while processing, DryRun): expected DeploymentType Undetermined, \
         got {:?}",
        unsettled.body["DryRunResults"]
    );
}

#[tokio::test]
async fn a_configuration_change_settles_from_processing_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let name = "settling-domain";
    create_baseline(&emulator, name).await;

    // Straight after the create the domain is still provisioning, and so is every
    // option the create set a setting in force for. The log-publishing configuration
    // the baseline sends names a log type with publishing switched OFF, which is nothing
    // for the service to apply to a cluster, so that option is `Active` from the start —
    // the state of an option that is not being changed.
    let before = emulator.describe_config(name).await;
    let before = &before.body["DomainConfig"];
    for (option, _) in baseline_options() {
        let expected = if option == "LogPublishingOptions" {
            "Active"
        } else {
            "Processing"
        };
        assert_eq!(
            option_state(before, option, "before settling"),
            expected,
            "case before settling({option}): expected {expected}, got {:?}",
            before[option]["Status"]
        );
    }

    // Once the provisioning the clock owes the domain has fallen due — polled for, not
    // waited out — the domain and every option of it are settled.
    let after_status = settled_domain(&emulator, name).await;
    let after_status = &after_status;
    assert_eq!(
        (
            after_status["Processing"].as_bool(),
            after_status["DomainProcessingStatus"].as_str()
        ),
        (Some(false), Some("Active")),
        "case after settling: expected Processing false and DomainProcessingStatus Active, \
         got {after_status:?}"
    );

    let after = emulator.describe_config(name).await;
    let after = &after.body["DomainConfig"];
    for (option, value) in baseline_options() {
        assert_eq!(
            option_state(after, option, "after settling"),
            "Active",
            "case after settling({option}): expected Active, got {:?}",
            after[option]["Status"]
        );
        // Settling moves the state, nothing else: the values and revisions stand.
        assert_eq!(
            after[option]["Options"],
            completed(option, &value),
            "case after settling({option}): expected the persisted {value:?}, got {:?}",
            after[option]["Options"]
        );
        assert_eq!(
            option_version(after, option, "after settling"),
            2,
            "case after settling({option}): settling must not bump a revision, got {:?}",
            after[option]["Status"]
        );
    }
}

#[tokio::test]
async fn a_duplicate_domain_name_is_refused_and_the_first_record_stands() {
    let emulator = Emulator::new();
    for name in ["dup-one", "dup-two", "dup-three"] {
        let first = create_baseline(&emulator, name).await;
        let again = emulator
            .create(json!({
                "DomainName": name,
                "ClusterConfig": { "InstanceType": "m5.large.search", "InstanceCount": 9 },
            }))
            .await;
        assert_eq!(
            again.status, 409,
            "case CreateDomain({name:?} twice): expected 409, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ResourceAlreadyExistsException",
            "case CreateDomain({name:?} twice): expected ResourceAlreadyExistsException, got {:?}",
            again.body
        );
        let read = emulator.describe(name).await;
        assert_eq!(
            read.body["DomainStatus"]["ClusterConfig"], first["ClusterConfig"],
            "case CreateDomain({name:?} twice): the refused create must not touch the record: \
             expected {:?}, got {:?}",
            first["ClusterConfig"], read.body["DomainStatus"]["ClusterConfig"]
        );
    }
}

#[tokio::test]
async fn a_deleted_domain_is_gone_from_every_read_path() {
    let emulator = Emulator::new();
    for name in ["keep-one", "to-delete", "keep-two"] {
        create_baseline(&emulator, name).await;
    }

    let deleted = emulator.delete("to-delete").await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(to-delete): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    let status = &deleted.body["DomainStatus"];
    assert_eq!(
        (status["Deleted"].as_bool(), status["DomainName"].as_str()),
        (Some(true), Some("to-delete")),
        "case DeleteDomain(to-delete): expected the final status Deleted true, got {status:?}"
    );

    // Every read path answers a domain this account no longer holds with the
    // missing-resource refusal, on the one status this service carries that shape on.
    for (label, answer) in [
        ("DescribeDomain", emulator.describe("to-delete").await),
        (
            "DescribeDomainConfig",
            emulator.describe_config("to-delete").await,
        ),
        ("DeleteDomain again", emulator.delete("to-delete").await),
    ] {
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {label}(deleted): expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {label}(deleted): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    let listed = emulator.list(None).await;
    let names: Vec<String> = listed.body["DomainNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case ListDomainNames(after delete): expected DomainNames, got {:?}", listed.body))
        .iter()
        .map(|entry| entry["DomainName"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        names,
        vec!["keep-one".to_string(), "keep-two".to_string()],
        "case ListDomainNames(after delete): expected only the surviving domains, got {names:?}"
    );
}

#[tokio::test]
async fn an_unknown_domain_name_is_not_found_on_every_path_that_resolves_one() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "present-domain").await;

    for name in ["never-created", "also-missing", "gone-domain"] {
        for (label, answer) in [
            ("DescribeDomain", emulator.describe(name).await),
            ("DescribeDomainConfig", emulator.describe_config(name).await),
            (
                "UpdateDomainConfig",
                emulator
                    .update_config(name, json!({ "ClusterConfig": { "InstanceCount": 2 } }))
                    .await,
            ),
            ("DeleteDomain", emulator.delete(name).await),
        ] {
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case {label}({name:?}): expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {label}({name:?}): expected ResourceNotFoundException, got {:?}",
                answer.body
            );
        }
    }
}
