// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GuardDuty detector lifecycle and the findings read loop.
//!
//! Every request goes over the wire through [`MultiServiceRouter::dispatch`], the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A detector read reports what the create/update stored, and a delete is observable
//!    by a later read.
//! 2. GuardDuty allows exactly one detector per account per Region; a replayed
//!    `ClientToken` returns the detector the first create minted.
//! 3. A reporting operation handed an id the store does not hold answers HTTP 400
//!    under the `InvalidInputException` class, never a hollow 200.
//! 4. Everything about a generated sample finding is derived from the finding TYPE,
//!    following GuardDuty's documented finding-type naming convention.
//! 5. Paging a list with any `maxResults` yields each item exactly once, in order.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

pub(crate) type Router = MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to `guardduty` in `us-east-1`. The router
/// resolves the target service from this scope, and the handlers read the Region the
/// request was signed for from it. Request authentication is switched OFF for these
/// tests, so the signature itself is never verified.
const AUTH: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTGUARDDUTY/20261021/us-east-1/guardduty/aws4_request, SignedHeaders=host, Signature=0000";

/// The same header, scoped to an arbitrary Region.
fn auth_for(region: &str) -> String {
    format!("AWS4-HMAC-SHA256 Credential=AKIDTESTGUARDDUTY/20261021/{region}/guardduty/aws4_request, SignedHeaders=host, Signature=0000")
}

/// A well-formed `DetectorId` (32 lowercase hex characters) that no create ever
/// minted.
pub(crate) const ABSENT_DETECTOR_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

/// A router carrying only this service, over its own engine.
pub(crate) fn new_router() -> Router {
    router_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`new_router`], handing back the engine it was built over, for a suite that needs
/// both the wire and a handle on the store the handlers write — the seam a
/// relationship no administrator request can reach is driven through.
pub(crate) fn router_with_engine() -> (Router, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// A router carrying only this service over `engine`, for a suite that needs a handle
/// on the store the handlers write. [`new_router`] is this with an engine of its own,
/// so the two cannot drift.
pub(crate) fn router_over(engine: Arc<engine::InMemoryEngine>) -> Router {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body).
pub(crate) async fn call(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_signed(router, AUTH, method, path, body).await
}

/// [`call`], signed for a chosen Region.
pub(crate) async fn call_in(
    router: &Router,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_signed(router, &auth_for(region), method, path, body).await
}

/// [`call`], sent to the endpoint authority `host` rather than to this harness's
/// default one.
///
/// A suite asserting that a rule does NOT depend on the endpoint a request arrived at
/// has to be able to choose that authority.
pub(crate) async fn call_at(
    router: &Router,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_signed_at(router, AUTH, host, method, path, body).await
}

async fn call_signed(
    router: &Router,
    authorization: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_signed_at(router, authorization, DEFAULT_HOST, method, path, body).await
}

/// The endpoint authority every request of this harness addresses unless a suite
/// names another ([`call_at`]).
const DEFAULT_HOST: &str = "guardduty.us-east-1.amazonaws.com";

/// [`call_in`], answering with the response body's RAW bytes as the service
/// serialized them rather than with a parsed `Value`.
///
/// A suite asserting the serialized FORM of a member — its JSON type, its case, the
/// whitespace around it — has to read the bytes, because parsing collapses exactly
/// those differences. Built on the same dispatch [`call`] uses, so the two cannot
/// drift.
pub(crate) async fn call_bytes_in(
    router: &Router,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String) {
    let (status, _, bytes) =
        dispatch_at(router, &auth_for(region), DEFAULT_HOST, method, path, body).await;
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("a UTF-8 response body"),
    )
}

async fn call_signed_at(
    router: &Router,
    authorization: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, error_type, bytes) =
        dispatch_at(router, authorization, host, method, path, body).await;
    let mut parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    // The `x-amzn-errortype` header is folded into the body under a key of its own,
    // so an assertion can read the error SHAPE it carries beside the error CLASS the
    // body's `__type` carries. The two are different facts on this service.
    if !error_type.is_empty() {
        if let Some(object) = parsed.as_object_mut() {
            object.insert("__errorType".to_string(), Value::String(error_type));
        }
    }
    (status, parsed)
}

/// Send one request over [`MultiServiceRouter::dispatch`] and return its status, its
/// `x-amzn-errortype` header and the response body's raw bytes.
async fn dispatch_at(
    router: &Router,
    authorization: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, bytes::Bytes) {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, error_type, response.into_body())
}

/// The error CLASS an error response carries: the body's `__type` discriminator,
/// which is what a client switches on.
pub(crate) fn error_code(body: &Value) -> String {
    for key in ["__type", "__errorType", "code"] {
        if let Some(value) = body.get(key).and_then(Value::as_str) {
            return value
                .rsplit('#')
                .next()
                .unwrap_or(value)
                .split(':')
                .next()
                .unwrap_or(value)
                .to_string();
        }
    }
    String::new()
}

/// The error SHAPE an error response resolves to: the `x-amzn-errortype` header,
/// which restJson1 — not the body — is what an SDK unmarshals the response with.
pub(crate) fn error_shape(body: &Value) -> String {
    body.get("__errorType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) async fn create_detector(router: &Router, body: Value) -> (u16, Value) {
    call(router, "POST", "/detector", Some(body)).await
}

/// Create an enabled detector and return its id, failing the test if the create did
/// not succeed.
pub(crate) async fn enabled_detector(router: &Router) -> String {
    let (status, body) = create_detector(router, json!({"enable": true})).await;
    assert_eq!(status, 200, "create-detector: expected 200, got {status} {body:?}");
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {body:?}"))
        .to_string()
}

/// The `RuleId`s GuardDuty publishes, read over the wire through
/// `ListCustomDetectionRules` (`POST /custom-detection-rule/rule` —
/// `smithy_query(operation="ListCustomDetectionRules")`), so a suite spanning the
/// catalogue cannot drift from it. Lives beside [`call`] because more than one suite
/// needs the same read.
pub(crate) async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call(router, "POST", "/custom-detection-rule/rule", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ruleId"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.len() > 2,
        "list-custom-detection-rules: expected a catalogue of more than two rules, got {ids:?}"
    );
    ids
}

/// Designate the account this emulator serves as the GuardDuty delegated
/// administrator of the organization `region`'s endpoint serves, through
/// `EnableOrganizationAdminAccount`.
///
/// The `*CustomDetectionRuleOrgConfiguration` operations are available only to that
/// delegated administrator, so a suite that expects one of them to be SERVED
/// establishes the standing first. Designating the account a Region already holds is
/// the same designation and answers success, so this may be called as often as a
/// suite needs. Lives beside [`call`] because several suites need it.
pub(crate) async fn as_delegated_admin(router: &Router, region: &str) {
    let (status, body) = crate::organization_admin_accounts::enable_admin(
        router,
        region,
        &service_com_amazonaws_guardduty_guarddutyapiservice::state::administrator_account_id(),
    )
    .await;
    assert_eq!(
        status, 200,
        "enable-organization-admin-account in {region}: expected 200, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1 + 2: create -> read -> update -> read -> delete -> read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_detector_lifecycle_is_observable_through_stored_state() {
    let router = new_router();

    // Before any create, the list is empty — not absent, not canned.
    let (status, body) = call(&router, "GET", "/detector", None).await;
    assert_eq!(status, 200, "list-detectors on an empty store: got {status} {body:?}");
    assert_eq!(
        body["detectorIds"],
        json!([]),
        "list-detectors on an empty store: expected [], got {body:?}"
    );

    let (status, created) = create_detector(
        &router,
        json!({
            "enable": true,
            "clientToken": "token-one",
            "findingPublishingFrequency": "SIX_HOURS",
            "tags": {"team": "security", "env": "test"},
        }),
    )
    .await;
    assert_eq!(status, 200, "create-detector: got {status} {created:?}");
    let detector_id = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector: expected a detectorId, got {created:?}"))
        .to_string();
    assert_eq!(
        detector_id.len(),
        32,
        "create-detector: expected a 32-character DetectorId, got {detector_id:?}"
    );
    assert!(
        detector_id.chars().all(|c| c.is_ascii_hexdigit()),
        "create-detector: expected a hex DetectorId, got {detector_id:?}"
    );

    // The minted id is STORED: the list reports exactly it.
    let (_, body) = call(&router, "GET", "/detector", None).await;
    assert_eq!(
        body["detectorIds"],
        json!([detector_id.clone()]),
        "list-detectors after one create: got {body:?}"
    );

    // A read reports what the create stored.
    let (status, got) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 200, "get-detector: got {status} {got:?}");
    assert_eq!(got["status"], json!("ENABLED"), "get-detector status: got {got:?}");
    assert_eq!(
        got["findingPublishingFrequency"],
        json!("SIX_HOURS"),
        "get-detector findingPublishingFrequency: got {got:?}"
    );
    assert_eq!(
        got["tags"],
        json!({"team": "security", "env": "test"}),
        "get-detector tags: got {got:?}"
    );
    let service_role = got["serviceRole"].as_str().unwrap_or_default();
    assert!(
        service_role.starts_with("arn:aws:iam::") && service_role.contains("guardduty"),
        "get-detector serviceRole: expected GuardDuty's service-linked role ARN, got {service_role:?}"
    );
    let created_at = got["createdAt"].as_str().unwrap_or_default().to_string();
    assert!(
        created_at.starts_with("20") && created_at.ends_with('Z'),
        "get-detector createdAt: expected an ISO-8601 instant, got {created_at:?}"
    );
    // The millisecond field is fixed-width, so these timestamps compare in time
    // order as text — which is all a client has, since they are modelled as String.
    let fraction = created_at
        .rsplit_once('.')
        .map(|(_, fraction)| fraction.to_string())
        .unwrap_or_default();
    assert!(
        fraction.len() == 4
            && fraction.ends_with('Z')
            && fraction[..3].chars().all(|c| c.is_ascii_digit()),
        "get-detector createdAt: expected a fixed three-digit millisecond field, got {created_at:?}"
    );
    assert_eq!(
        got["updatedAt"].as_str().unwrap_or_default(),
        created_at,
        "get-detector on a detector nothing has updated: expected updatedAt == createdAt, got {got:?}"
    );

    // A replayed ClientToken returns the detector the first create minted.
    let (status, replayed) = create_detector(
        &router,
        json!({"enable": true, "clientToken": "token-one"}),
    )
    .await;
    assert_eq!(status, 200, "create-detector replay: got {status} {replayed:?}");
    assert_eq!(
        replayed["detectorId"],
        json!(detector_id.clone()),
        "create-detector replay: expected the first detector id, got {replayed:?}"
    );

    // A second create with a FRESH token is refused: one detector per account/Region.
    let (status, refused) =
        create_detector(&router, json!({"enable": true, "clientToken": "token-two"})).await;
    assert_eq!(status, 400, "second create-detector: got {status} {refused:?}");
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "second create-detector: got {refused:?}"
    );
    let (_, body) = call(&router, "GET", "/detector", None).await;
    assert_eq!(
        body["detectorIds"],
        json!([detector_id.clone()]),
        "a refused create must not mint a second detector: got {body:?}"
    );

    // An update MUTATES the stored detector and bumps UpdatedAt.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"enable": false, "findingPublishingFrequency": "FIFTEEN_MINUTES"})),
    )
    .await;
    assert_eq!(status, 200, "update-detector: got {status} {body:?}");
    let (_, got) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        got["status"],
        json!("DISABLED"),
        "get-detector after --no-enable: got {got:?}"
    );
    assert_eq!(
        got["findingPublishingFrequency"],
        json!("FIFTEEN_MINUTES"),
        "get-detector after an update: got {got:?}"
    );
    assert_eq!(
        got["createdAt"].as_str().unwrap_or_default(),
        created_at,
        "an update must not move createdAt: got {got:?}"
    );
    let updated_at = got["updatedAt"].as_str().unwrap_or_default().to_string();
    assert!(
        updated_at > created_at,
        "an update must bump updatedAt: createdAt {created_at:?}, updatedAt {updated_at:?}"
    );

    // A delete is observable by a later read.
    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 200, "delete-detector: got {status} {body:?}");
    let (status, body) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 400, "get-detector after delete: got {status} {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "get-detector after delete: got {body:?}"
    );
    let (_, body) = call(&router, "GET", "/detector", None).await;
    assert_eq!(
        body["detectorIds"],
        json!([]),
        "list-detectors after delete: got {body:?}"
    );
    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 400, "delete-detector twice: got {status} {body:?}");
}

// ---------------------------------------------------------------------------
// Rule 1, over the whole value space of the members a create records.
// ---------------------------------------------------------------------------

/// `FindingPublishingFrequency` is `FIFTEEN_MINUTES | ONE_HOUR | SIX_HOURS`
/// (`smithy_query(shape="FindingPublishingFrequency")`); `None` stands for a request
/// that names no frequency, which takes the default.
#[tokio::test]
async fn a_detector_reports_back_the_configuration_it_was_created_with() {
    // (Enable, requested frequency, expected status, expected reported frequency)
    let cases: [(bool, Option<&str>, &str, &str); 8] = [
        (true, Some("FIFTEEN_MINUTES"), "ENABLED", "FIFTEEN_MINUTES"),
        (true, Some("ONE_HOUR"), "ENABLED", "ONE_HOUR"),
        (true, Some("SIX_HOURS"), "ENABLED", "SIX_HOURS"),
        (true, None, "ENABLED", "SIX_HOURS"),
        (false, Some("FIFTEEN_MINUTES"), "DISABLED", "FIFTEEN_MINUTES"),
        (false, Some("ONE_HOUR"), "DISABLED", "ONE_HOUR"),
        (false, Some("SIX_HOURS"), "DISABLED", "SIX_HOURS"),
        (false, None, "DISABLED", "SIX_HOURS"),
    ];

    for case in cases {
        let (enable, frequency, expected_status, expected_frequency) = case;
        // One detector per account/Region, so each row gets its own store.
        let router = new_router();
        let mut body = json!({"enable": enable});
        if let Some(frequency) = frequency {
            body["findingPublishingFrequency"] = json!(frequency);
        }
        let (status, created) = create_detector(&router, body).await;
        assert_eq!(status, 200, "case {case:?}: create-detector got {status} {created:?}");
        let id = created["detectorId"].as_str().unwrap_or_default().to_string();

        let (status, got) = call(&router, "GET", &format!("/detector/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: get-detector got {status} {got:?}");
        assert_eq!(
            got["status"],
            json!(expected_status),
            "case {case:?}: expected status {expected_status}, got {got:?}"
        );
        assert_eq!(
            got["findingPublishingFrequency"],
            json!(expected_frequency),
            "case {case:?}: expected frequency {expected_frequency}, got {got:?}"
        );
        // The detector's own `status` carries whether the detector is enabled; the
        // dataSources view carries the CONFIGURATION, which survives disabling.
        // CloudTrail management events, VPC flow logs and DNS logs have no switch at
        // all — no `DetectorFeature` member and no `DataSourceConfigurations` member
        // names them — so they report ENABLED for either value of `enable`.
        for source in ["cloudTrail", "dnsLogs", "flowLogs"] {
            assert_eq!(
                got["dataSources"][source]["status"],
                json!("ENABLED"),
                "case {case:?}: dataSources.{source}.status expected ENABLED, got {got:?}"
            );
        }
    }
}

/// The deprecated `DataSources` view and the `Features` view are two names for one
/// switch, so writing through either is read back through both. The three pairings
/// come from the `DataSourceConfigurations` and `DetectorFeature` shapes.
#[tokio::test]
async fn a_data_source_and_the_feature_that_governs_it_are_one_stored_switch() {
    // (feature name, the DataSources body that sets it, the dataSources JSON pointer)
    let cases: [(&str, Value, Vec<&str>); 3] = [
        (
            "S3_DATA_EVENTS",
            json!({"s3Logs": {"enable": false}}),
            vec!["s3Logs", "status"],
        ),
        (
            "EKS_AUDIT_LOGS",
            json!({"kubernetes": {"auditLogs": {"enable": false}}}),
            vec!["kubernetes", "auditLogs", "status"],
        ),
        (
            "EBS_MALWARE_PROTECTION",
            json!({"malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": false}}}),
            vec!["malwareProtection", "scanEc2InstanceWithFindings", "ebsVolumes", "status"],
        ),
    ];

    for case in &cases {
        let (feature, data_sources, pointer) = case;

        // Written through the deprecated DataSources view.
        let router = new_router();
        let (status, created) =
            create_detector(&router, json!({"enable": true, "dataSources": data_sources})).await;
        assert_eq!(status, 200, "case {case:?}: create-detector got {status} {created:?}");
        let id = created["detectorId"].as_str().unwrap_or_default().to_string();
        let (_, got) = call(&router, "GET", &format!("/detector/{id}"), None).await;
        assert_status_at(&got, pointer, "DISABLED", &format!("case {case:?} via dataSources"));
        assert_eq!(
            feature_status(&got, feature),
            "DISABLED",
            "case {case:?}: DataSources wrote the switch, so feature {feature} must read DISABLED, got {got:?}"
        );

        // Written through Features, read back through both views.
        let router = new_router();
        let (status, created) = create_detector(
            &router,
            json!({"enable": true, "features": [{"name": feature, "status": "DISABLED"}]}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: create-detector got {status} {created:?}");
        let id = created["detectorId"].as_str().unwrap_or_default().to_string();
        let (_, got) = call(&router, "GET", &format!("/detector/{id}"), None).await;
        assert_eq!(
            feature_status(&got, feature),
            "DISABLED",
            "case {case:?}: feature {feature} expected DISABLED, got {got:?}"
        );
        assert_status_at(&got, pointer, "DISABLED", &format!("case {case:?} via features"));

        // And an update moves it back, which the next read observes.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{id}"),
            Some(json!({"features": [{"name": feature, "status": "ENABLED"}]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: update-detector got {status} {body:?}");
        let (_, got) = call(&router, "GET", &format!("/detector/{id}"), None).await;
        assert_eq!(
            feature_status(&got, feature),
            "ENABLED",
            "case {case:?}: after re-enabling, feature {feature} expected ENABLED, got {got:?}"
        );
        assert_status_at(&got, pointer, "ENABLED", &format!("case {case:?} after update"));
    }
}

fn assert_status_at(got: &Value, pointer: &[&str], expected: &str, case: &str) {
    let mut cursor = &got["dataSources"];
    for segment in pointer {
        cursor = &cursor[*segment];
    }
    assert_eq!(
        cursor,
        &json!(expected),
        "{case}: dataSources.{} expected {expected}, got {got:?}",
        pointer.join(".")
    );
}

fn feature_status(got: &Value, name: &str) -> String {
    got["features"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|feature| feature["name"] == json!(name))
        .and_then(|feature| feature["status"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Runtime Monitoring already covers Amazon EKS, so the two features cannot both be
/// enabled: "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and
/// Runtime Monitoring (RUNTIME_MONITORING) will cause an error"
/// (`smithy_query(query="doc",shape="CreateDetector")`). A refused request must leave
/// no detector behind.
#[tokio::test]
async fn a_feature_list_the_service_cannot_accept_is_refused() {
    let both = json!([
        {"name": "EKS_RUNTIME_MONITORING", "status": "ENABLED"},
        {"name": "RUNTIME_MONITORING", "status": "ENABLED"},
    ]);

    let router = new_router();
    let (status, body) = create_detector(&router, json!({"enable": true, "features": both})).await;
    assert_eq!(status, 400, "create-detector with both runtime features: got {status} {body:?}");
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "create-detector with both runtime features: got {body:?}"
    );
    let (_, listed) = call(&router, "GET", "/detector", None).await;
    assert_eq!(
        listed["detectorIds"],
        json!([]),
        "a refused create must store nothing: got {listed:?}"
    );

    // The same rule on update, and a refused update must not change the detector.
    let id = enabled_detector(&router).await;
    let (_, before) = call(&router, "GET", &format!("/detector/{id}"), None).await;
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{id}"),
        Some(json!({"features": both})),
    )
    .await;
    assert_eq!(status, 400, "update-detector with both runtime features: got {status} {body:?}");
    let (_, after) = call(&router, "GET", &format!("/detector/{id}"), None).await;
    assert_eq!(
        after["updatedAt"], before["updatedAt"],
        "a refused update must not touch the detector: before {before:?}, after {after:?}"
    );

    // "When you don't specify any features, with an exception to RUNTIME_MONITORING,
    // all the optional features are enabled by default." Runtime Monitoring is that
    // ONE exception — see `crate::detector_data_sources_projection` for the default
    // feature set as a whole.
    assert_eq!(
        feature_status(&before, "RUNTIME_MONITORING"),
        "DISABLED",
        "RUNTIME_MONITORING is opt-in, so a bare create leaves it off: got {before:?}"
    );
    for feature in [
        "S3_DATA_EVENTS",
        "EKS_AUDIT_LOGS",
        "EBS_MALWARE_PROTECTION",
        "RDS_LOGIN_EVENTS",
        "LAMBDA_NETWORK_LOGS",
        "EKS_RUNTIME_MONITORING",
        "AI_PROTECTION",
        "AI_ANALYST",
    ] {
        assert_eq!(
            feature_status(&before, feature),
            "ENABLED",
            "feature {feature} is enabled by default on a bare create: got {before:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a sample finding is derived from its finding TYPE.
// ---------------------------------------------------------------------------

/// GuardDuty's documented severity bands: Low 1.0–3.9, Medium 4.0–6.9, High 7.0–8.9,
/// Critical 9.0–10.0 ("Severity levels for GuardDuty findings").
const BAND_LOW: (f64, f64) = (1.0, 3.9);
const BAND_MEDIUM: (f64, f64) = (4.0, 6.9);
const BAND_HIGH: (f64, f64) = (7.0, 8.9);
const BAND_CRITICAL: (f64, f64) = (9.0, 10.0);

/// A finding type is `ThreatPurpose:ResourceTypeAffected/ThreatFamilyName…`, so the
/// affected resource type follows from the type alone, and the severity is the one
/// GuardDuty publishes for that very type (see `sample_finding_catalog.rs`).
///
/// The rows span the threat purposes and the resource-type abbreviations, and include
/// types the issue did not cite.
#[tokio::test]
async fn a_sample_finding_is_derived_from_its_finding_type() {
    // (finding type, expected resource.resourceType, expected severity band)
    let cases: [(&str, &str, (f64, f64)); 10] = [
        ("Recon:EC2/PortProbeUnprotectedPort", "Instance", BAND_LOW),
        ("Policy:IAMUser/RootCredentialUsage", "AccessKey", BAND_LOW),
        ("Discovery:S3/MaliciousIPCaller", "S3Bucket", BAND_HIGH),
        ("UnauthorizedAccess:EC2/SSHBruteForce", "Instance", BAND_LOW),
        ("Backdoor:EC2/C&CActivity.B", "Instance", BAND_HIGH),
        ("CryptoCurrency:EC2/BitcoinTool.B", "Instance", BAND_HIGH),
        ("Exfiltration:S3/ObjectRead.Unusual", "S3Bucket", BAND_HIGH),
        ("Execution:Kubernetes/ExecInKubeSystemPod", "EKSCluster", BAND_MEDIUM),
        ("Impact:Lambda/AbusedDomainRequest.Reputation", "Lambda", BAND_MEDIUM),
        ("AttackSequence:IAM/CompromisedCredentials", "AccessKey", BAND_CRITICAL),
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let requested: Vec<&str> = cases.iter().map(|(ty, _, _)| *ty).collect();

    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": requested})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings: got {status} {body:?}");

    let (status, listed) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "list-findings: got {status} {listed:?}");
    let ids: Vec<String> = listed["findingIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        ids.len(),
        cases.len(),
        "list-findings: expected one finding per requested type, got {listed:?}"
    );

    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids.clone()})),
    )
    .await;
    assert_eq!(status, 200, "get-findings: got {status} {got:?}");
    let findings = got["findings"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        findings.len(),
        ids.len(),
        "get-findings: expected a finding per requested id, got {got:?}"
    );

    for case in cases {
        let (finding_type, expected_resource, (low, high)) = case;
        let finding = findings
            .iter()
            .find(|f| f["type"] == json!(finding_type))
            .unwrap_or_else(|| panic!("case {case:?}: no finding of that type in {got:?}"));

        let id = finding["id"].as_str().unwrap_or_default();
        assert!(
            ids.iter().any(|listed| listed == id),
            "case {case:?}: get-findings returned id {id:?}, which list-findings never listed: {ids:?}"
        );
        let severity = finding["severity"].as_f64().unwrap_or_default();
        assert!(
            (low..=high).contains(&severity),
            "case {case:?}: expected a severity in the documented band {low}..={high}, got {severity}"
        );
        assert_eq!(
            finding["resource"]["resourceType"],
            json!(expected_resource),
            "case {case:?}: expected resource.resourceType {expected_resource}, got {finding:?}"
        );
        assert_eq!(
            finding["schemaVersion"],
            json!("2.0"),
            "case {case:?}: expected the current finding schema version, got {finding:?}"
        );
        assert_eq!(
            finding["accountId"],
            json!(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: expected the emulator account id, got {finding:?}"
        );
        assert_eq!(
            finding["region"],
            json!("us-east-1"),
            "case {case:?}: expected the Region the request was signed for, got {finding:?}"
        );
        assert_eq!(
            finding["partition"],
            json!("aws"),
            "case {case:?}: expected the aws partition, got {finding:?}"
        );
        let arn = finding["arn"].as_str().unwrap_or_default();
        assert_eq!(
            arn,
            format!(
                "arn:aws:guardduty:us-east-1:{}:detector/{detector_id}/finding/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case:?}: unexpected finding ARN, got {finding:?}"
        );
        assert_eq!(
            finding["service"]["archived"],
            json!(false),
            "case {case:?}: a fresh finding is not archived, got {finding:?}"
        );
        assert_eq!(
            finding["service"]["detectorId"],
            json!(detector_id.clone()),
            "case {case:?}: a finding belongs to the detector it was created under, got {finding:?}"
        );
        assert!(
            finding["createdAt"].as_str().unwrap_or_default().ends_with('Z'),
            "case {case:?}: expected an ISO-8601 createdAt, got {finding:?}"
        );
    }

    // An id the detector does not hold is absent from the result, not an error.
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ["00000000000000000000000000000000"]})),
    )
    .await;
    assert_eq!(status, 200, "get-findings for an unknown id: got {status} {got:?}");
    assert_eq!(
        got["findings"],
        json!([]),
        "get-findings for an unknown id: expected an empty list, got {got:?}"
    );

    // A delete cascades: the detector's findings go with it.
    let (status, _) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 200, "delete-detector");
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 400, "list-findings after the detector was deleted: got {status} {body:?}");
}

/// With no `findingTypes` the service "generates sample findings of all supported
/// finding types" (`smithy_query(query="doc",shape="CreateSampleFindings")`), so the
/// default set is larger than one and every finding in it is distinct.
#[tokio::test]
async fn create_sample_findings_without_finding_types_generates_the_supported_set() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings with no types: got {status} {body:?}");

    let ids = all_finding_ids(&router, &detector_id, 5).await;
    assert!(
        ids.len() > 1,
        "create-sample-findings with no types must generate the supported set, got {} finding(s)",
        ids.len()
    );
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every generated finding needs its own id, got {ids:?}"
    );
}

/// GuardDuty aggregates repeated occurrences of one finding type on one resource
/// into a SINGLE finding: the id and ARN stay put, `Service.Count` counts the
/// occurrences, and `UpdatedAt` moves. That also bounds the finding store, which
/// would otherwise grow without limit across repeated calls.
#[tokio::test]
async fn a_repeated_sample_of_one_finding_type_aggregates_into_one_finding() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let types = json!(["Recon:EC2/PortProbeUnprotectedPort", "Backdoor:EC2/C&CActivity.B"]);

    let mut previous: Option<(Vec<String>, String, i64)> = None;
    // (call number, expected occurrence count on each finding)
    for case in [(1_u32, 1_i64), (2, 2), (3, 3)] {
        let (call_number, expected_count) = case;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/create"),
            Some(json!({"findingTypes": types})),
        )
        .await;
        assert_eq!(status, 200, "case call {call_number}: got {status} {body:?}");

        let (_, listed) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({})),
        )
        .await;
        let mut ids: Vec<String> = listed["findingIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        ids.sort();
        assert_eq!(
            ids.len(),
            2,
            "case call {call_number}: two finding types means two findings, got {listed:?}"
        );

        let (_, got) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/get"),
            Some(json!({"findingIds": ids.clone()})),
        )
        .await;
        let findings = got["findings"].as_array().cloned().unwrap_or_default();
        for finding in &findings {
            assert_eq!(
                finding["service"]["count"],
                json!(expected_count),
                "case call {call_number}: expected an occurrence count of {expected_count}, got {finding:?}"
            );
            assert!(
                finding["createdAt"].as_str().unwrap_or_default()
                    <= finding["updatedAt"].as_str().unwrap_or_default(),
                "case call {call_number}: updatedAt must not precede createdAt, got {finding:?}"
            );
        }
        let created_at = findings
            .first()
            .and_then(|f| f["createdAt"].as_str())
            .unwrap_or_default()
            .to_string();
        let updated_at = findings
            .first()
            .and_then(|f| f["updatedAt"].as_str())
            .unwrap_or_default()
            .to_string();

        if let Some((previous_ids, previous_created, _)) = previous.as_ref() {
            assert_eq!(
                &ids, previous_ids,
                "case call {call_number}: aggregation keeps the same finding ids, got {ids:?}"
            );
            assert_eq!(
                &created_at, previous_created,
                "case call {call_number}: aggregation must not move createdAt, got {created_at:?}"
            );
            assert!(
                updated_at > *previous_created,
                "case call {call_number}: aggregation must move updatedAt past createdAt, got {updated_at:?}"
            );
        }
        previous = Some((ids, created_at, expected_count));
    }
}

// ---------------------------------------------------------------------------
// Rule 5: paging with any maxResults yields each item exactly once, in order.
// ---------------------------------------------------------------------------
/// Walk every page of `ListFindings` and return the ids in order.
pub(crate) async fn all_finding_ids(router: &Router, detector_id: &str, page_size: i64) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..200 {
        let mut body = json!({"maxResults": page_size});
        if let Some(token) = token.as_deref() {
            body["nextToken"] = json!(token);
        }
        let (status, page) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(body),
        )
        .await;
        assert_eq!(status, 200, "list-findings page: got {status} {page:?}");
        ids.extend(
            page["findingIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!("list-findings never ran out of pages for detector {detector_id}");
}

#[tokio::test]
async fn paging_a_findings_list_yields_each_finding_exactly_once() {
    let types = [
        "Recon:EC2/PortProbeUnprotectedPort",
        "Backdoor:EC2/C&CActivity.B",
        "Trojan:EC2/BlackholeTraffic",
        "Policy:IAMUser/RootCredentialUsage",
        "Discovery:S3/MaliciousIPCaller",
        "Stealth:IAMUser/CloudTrailLoggingDisabled",
        "PenTest:IAMUser/KaliLinux",
    ];

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, _) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings");

    // The unpaged order is the reference: `maxResults` only changes how many calls
    // it takes to read it, never the set or the order.
    let (_, listed) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({})),
    )
    .await;
    let reference: Vec<String> = listed["findingIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        reference.len(),
        types.len(),
        "one finding per requested type: got {listed:?}"
    );

    // 1 and 50 are the published bounds of `MaxResults` (range 1..50,
    // `smithy_query(operation="ListFindings")`); the rest sit between them.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 50] {
        let paged = all_finding_ids(&router, &detector_id, page_size).await;
        assert_eq!(
            paged, reference,
            "case maxResults={page_size}: paging must yield the same ids in the same order, got {paged:?}"
        );
    }

    // A maxResults past the published maximum is refused by the modelled range.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({"maxResults": 51})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case maxResults=51: a value past the modelled maximum must be refused, got {status} {body:?}"
    );

    // A token this service never issued is refused rather than silently treated as
    // the first page — whether it is not a token at all, or a numeric one pointing
    // past the end of a list this service would have answered with no nextToken.
    for token in ["not-a-token-this-service-issued", "999999", "7"] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"nextToken": token})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: a foreign token must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case nextToken={token:?}: got {body:?}"
        );
    }
}

/// `ListFindings` applies `FindingCriteria` rather than ignoring it, and refuses a
/// criterion key that names no field of a finding rather than returning findings the
/// caller asked to exclude.
#[tokio::test]
async fn list_findings_applies_the_finding_criteria_it_is_given() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, _) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": [
            "Recon:EC2/PortProbeUnprotectedPort",
            "Backdoor:EC2/C&CActivity.B",
            "Discovery:S3/MaliciousIPCaller",
        ]})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings");

    // (criterion, how many of the three findings it selects)
    //
    // The `Condition` members the model offers only to the filter APIs are not among
    // the rows: a findings read refuses them, which the `filter_only_conditions` suite
    // asserts.
    let cases: [(Value, usize); 7] = [
        (json!({"type": {"equals": ["Recon:EC2/PortProbeUnprotectedPort"]}}), 1),
        (json!({"type": {"notEquals": ["Recon:EC2/PortProbeUnprotectedPort"]}}), 2),
        (json!({"resource.resourceType": {"equals": ["Instance"]}}), 2),
        (json!({"resource.resourceType": {"equals": ["S3Bucket"]}}), 1),
        // Of the three types, only `Recon:EC2/PortProbeUnprotectedPort` is published
        // below the High band.
        (json!({"severity": {"greaterThanOrEqual": 7}}), 2),
        (json!({"severity": {"lessThan": 7}}), 1),
        (json!({"service.archived": {"equals": ["false"]}}), 3),
    ];

    for case in &cases {
        let (criterion, expected) = case;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"findingCriteria": {"criterion": criterion}})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: list-findings got {status} {body:?}");
        let count = body["findingIds"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, *expected,
            "case {case:?}: expected {expected} finding(s), got {count} in {body:?}"
        );
    }

    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({"findingCriteria": {"criterion": {"service.actionType": {"equals": ["DNS_REQUEST"]}}}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "a criterion key that names no finding field must be refused, not ignored: got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "a criterion key naming no finding field: got {body:?}"
    );

    // An ordering condition only applies to an attribute that HAS a numeric form.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({"findingCriteria": {"criterion": {"type": {"greaterThan": 1}}}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "an ordering condition on a text attribute must be refused: got {status} {body:?}"
    );

    // And the refusal must not depend on how much state happens to exist: an empty
    // detector refuses the same criterion.
    let empty_router = new_router();
    let empty_detector = enabled_detector(&empty_router).await;
    for case in [
        json!({"service.actionType": {"equals": ["DNS_REQUEST"]}}),
        json!({"type": {"greaterThan": 1}}),
    ] {
        let (status, body) = call(
            &empty_router,
            "POST",
            &format!("/detector/{empty_detector}/findings"),
            Some(json!({"findingCriteria": {"criterion": case}})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a detector with no findings must refuse the same criterion, got {status} {body:?}"
        );
    }
}

/// `Matches` / `NotMatches` are the two `Condition` members the model offers only to the
/// filter APIs, so a findings read is REFUSED them. Their wildcard semantics, and the
/// refusal on the read side, live in the `filter_only_conditions` suite.
#[tokio::test]
async fn list_findings_refuses_a_condition_the_filter_apis_alone_offer() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, _) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": [
            "Recon:EC2/PortProbeUnprotectedPort",
            "Backdoor:EC2/C&CActivity.B",
            "Discovery:S3/MaliciousIPCaller",
        ]})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings");

    for case in [
        json!({"type": {"matches": ["Recon:*"]}}),
        json!({"type": {"notMatches": ["Recon:*"]}}),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"findingCriteria": {"criterion": case}})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a condition the reads do not offer must be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: got {body:?}"
        );
    }
}

/// `SortCriteria` orders the result, and an attribute that names no field of a finding
/// is refused even when the result would have been empty.
#[tokio::test]
async fn list_findings_orders_the_result_by_the_sort_criteria_it_is_given() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // A sort attribute naming no finding field is refused BEFORE any finding exists,
    // so the refusal is about the attribute and not about the data.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(json!({"sortCriteria": {"attributeName": "service.actionType"}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unsortable attribute on an empty detector must still be refused: got {status} {body:?}"
    );

    let (status, _) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": [
            "Recon:EC2/PortProbeUnprotectedPort",
            "Discovery:S3/MaliciousIPCaller",
            "Backdoor:EC2/C&CActivity.B",
        ]})),
    )
    .await;
    assert_eq!(status, 200, "create-sample-findings");

    // (orderBy, expected severity order)
    let cases: [(&str, [bool; 1]); 2] = [("ASC", [true]), ("DESC", [false])];
    for case in cases {
        let (order_by, [ascending]) = case;
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings"),
            Some(json!({"sortCriteria": {"attributeName": "severity", "orderBy": order_by}})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: list-findings got {status} {body:?}");
        let ids: Vec<String> = body["findingIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let (_, got) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/get"),
            Some(json!({"findingIds": ids, "sortCriteria": {"attributeName": "severity", "orderBy": order_by}})),
        )
        .await;
        let severities: Vec<f64> = got["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| f["severity"].as_f64().unwrap_or_default())
            .collect();
        assert_eq!(
            severities.len(),
            3,
            "case {case:?}: expected three findings, got {got:?}"
        );
        let ordered = severities
            .windows(2)
            .all(|w| if ascending { w[0] <= w[1] } else { w[0] >= w[1] });
        assert!(
            ordered,
            "case {case:?}: expected severities sorted {order_by}, got {severities:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an id the store does not hold is a classified 400, on every operation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_operation_refuses_a_detector_id_the_store_does_not_hold() {
    // (operation, method, path suffix, body)
    let cases: [(&str, &str, &str, Option<Value>); 6] = [
        ("GetDetector", "GET", "", None),
        ("UpdateDetector", "POST", "", Some(json!({"enable": true}))),
        ("DeleteDetector", "DELETE", "", None),
        (
            "CreateSampleFindings",
            "POST",
            "/findings/create",
            Some(json!({"findingTypes": ["Recon:EC2/PortProbeUnprotectedPort"]})),
        ),
        ("ListFindings", "POST", "/findings", Some(json!({}))),
        (
            "GetFindings",
            "POST",
            "/findings/get",
            Some(json!({"findingIds": []})),
        ),
    ];

    // Both with an empty store and with a DIFFERENT detector present, so the refusal
    // is about the id and not about the store being empty.
    for populated in [false, true] {
        let router = new_router();
        if populated {
            let _ = enabled_detector(&router).await;
        }
        for case in &cases {
            let (operation, method, suffix, body) = case;
            let path = format!("/detector/{ABSENT_DETECTOR_ID}{suffix}");
            let (status, response) = call(&router, method, &path, body.clone()).await;
            assert_eq!(
                status, 400,
                "case {operation} (store populated: {populated}): expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidInputException",
                "case {operation} (store populated: {populated}): got {response:?}"
            );
        }
    }
}

/// A detector is a REGIONAL resource: "you must create a detector in each Region
/// where you enable the service" and "you can have only one detector per account per
/// Region" (`smithy_query(query="doc",shape="CreateDetector")`). So the uniqueness
/// limit is per Region, and one Region's endpoint neither lists nor reads another's
/// detector.
#[tokio::test]
async fn a_detector_belongs_to_the_region_its_endpoint_created_it_in() {
    let router = new_router();

    // Several Regions, including the one the other tests use.
    let regions = ["us-east-1", "us-west-2", "eu-central-1"];
    let mut ids = Vec::new();
    for region in regions {
        let (status, created) = call_in(
            &router,
            region,
            "POST",
            "/detector",
            Some(json!({"enable": true, "clientToken": "one-token-per-region"})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: a first create in this Region must succeed, got {status} {created:?}"
        );
        let id = created["detectorId"].as_str().unwrap_or_default().to_string();
        assert!(!id.is_empty(), "case {region}: expected a detectorId, got {created:?}");
        ids.push(id);
    }
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        regions.len(),
        "each Region gets its own detector id, got {ids:?}"
    );

    for (region, id) in regions.iter().zip(&ids) {
        // Its own endpoint lists and reads exactly it.
        let (_, listed) = call_in(&router, region, "GET", "/detector", None).await;
        assert_eq!(
            listed["detectorIds"],
            json!([id.clone()]),
            "case {region}: expected only this Region's detector, got {listed:?}"
        );
        let (status, got) = call_in(&router, region, "GET", &format!("/detector/{id}"), None).await;
        assert_eq!(status, 200, "case {region}: get-detector got {status} {got:?}");

        // A second create against the same Region is refused.
        let (status, refused) = call_in(
            &router,
            region,
            "POST",
            "/detector",
            Some(json!({"enable": true, "clientToken": "a-different-token"})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region}: a second create in this Region must be refused, got {status} {refused:?}"
        );

        // Another Region's endpoint cannot see it.
        for other in regions.iter().filter(|other| *other != region) {
            let (status, body) =
                call_in(&router, other, "GET", &format!("/detector/{id}"), None).await;
            assert_eq!(
                status, 400,
                "case {region} read from {other}: a detector is not visible from another Region, got {status} {body:?}"
            );
        }
    }

    // Deleting one Region's detector leaves the others alone.
    let (status, _) = call_in(
        &router,
        regions[0],
        "DELETE",
        &format!("/detector/{}", ids[0]),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete-detector in {}", regions[0]);
    let (_, listed) = call_in(&router, regions[0], "GET", "/detector", None).await;
    assert_eq!(
        listed["detectorIds"],
        json!([]),
        "after the delete, {} lists nothing: got {listed:?}",
        regions[0]
    );
    let (_, listed) = call_in(&router, regions[1], "GET", "/detector", None).await;
    assert_eq!(
        listed["detectorIds"],
        json!([ids[1].clone()]),
        "a delete in one Region must not touch another: got {listed:?}"
    );
}

// `GetRemainingFreeTrialDays` was the last operation of this service still carrying
// the scaffold marker, and the suite that asserted its 501 lived here. It is
// implemented now, so there is no operation left for such an assertion to be true
// of, and the `free_trial_days` suite is what asserts the behaviour it answers with
// instead. What is left to pin is the CENSUS, which the test below does
// mechanically.

/// No operation of this service is a scaffold any more: no `src/ops/*.rs` carries the
/// marker line that says "nobody has written it yet", so none of them answers HTTP
/// 501.
///
/// The census is a machine-readable record — the marker line is grepped for exactly
/// this — so it is asserted here rather than only described: a re-scaffolded or newly
/// generated operation shows up as a failure with its file named, where before it
/// would have been noticed only by a client receiving a 501.
#[test]
fn no_operation_of_this_service_is_still_a_scaffold() {
    /// The marker line the scaffold generator puts at the top of an unimplemented
    /// operation, and that implementing one deletes.
    const MARKER: &str = "//! SCAFFOLDED, NOT IMPLEMENTED:";

    let ops = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ops");
    let mut scaffolded: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&ops).expect("this crate's src/ops directory") {
        let path = entry.expect("an entry of src/ops").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("the source of an operation module");
        if source.lines().any(|line| line.starts_with(MARKER)) {
            scaffolded.push(
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
    }
    assert!(
        scaffolded.is_empty(),
        "these operation modules still carry the scaffold marker, so they answer 501: {scaffolded:?}"
    );
}
