// SPDX-License-Identifier: CC0-1.0

//! What this service's read projections report: the wire FORM every timestamp travels in,
//! the members a `Service` body always carries, and the mutations a later read has to
//! reflect.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. Every member targeting the prelude `smithy.api#Timestamp` carries no
//!    `@timestampFormat` override, so restJson1 puts it on the wire as an epoch-seconds
//!    number — and the form this service emits carries no more than a millisecond's
//!    resolution, whichever projection the member sits in.
//! 2. A state a resource always holds is always reported: a `Service` body carries its
//!    `dependencyDiscovery` (off is a state, not an absence), its `rerunAssessment` flag
//!    and its `tags` map on every path that projects a service.
//! 3. A tag written onto a resource is what the next read of that resource reports, for
//!    every resource kind this store holds, and a request that would take a resource past
//!    the tags one may hold is refused whole.
//! 4. An update writes the members the caller sent into the stored resource, so the next
//!    listing reports the new values and leaves the rest alone.
//! 5. A delete removes the resource, so the next listing does not report it — and the
//!    listing still reports its `@required` list, empty.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised
//! exactly as a client would exercise them.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    DEPENDENCY_DISCOVERY_DISABLED_MESSAGE, MAX_TAGS_PER_RESOURCE, POLICY_RESOURCE_TYPE,
    SERVICE_FUNCTION_RESOURCE_TYPE, SERVICE_RESOURCE_TYPE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
const CREATE_POLICY: &str = "/v2/create-policy";
const GET_POLICY: &str = "/v2/get-policy";
const UPDATE_POLICY: &str = "/v2/update-policy";
const CREATE_SERVICE: &str = "/v2/create-service";
const GET_SERVICE: &str = "/v2/get-service";
const UPDATE_SERVICE: &str = "/v2/update-service";
const LIST_SERVICES: &str = "/v2/list-services";
const DELETE_POLICY: &str = "/v2/delete-policy";
const DELETE_SERVICE: &str = "/v2/delete-service";
const CREATE_FUNCTION: &str = "/v2/create-service-function";
const UPDATE_FUNCTION: &str = "/v2/update-function";
const DELETE_FUNCTION: &str = "/v2/delete-function";
const LIST_FUNCTIONS: &str = "/v2/list-functions";
const TAGS: &str = "/v2/tags";

/// The Region these suites sign for, and the account every emulated ARN carries.
const REGION: &str = "us-east-1";
const ACCOUNT: &str = "000000000000";

/// The members of this service's projections that target the prelude `smithy.api#Timestamp`.
///
/// SOURCE: `smithy_query(query="shape", shape="Policy,Service,ServiceSummary,ServiceFunction,
/// DependencyDiscoveryConfig,DevOpsAgentConfigurationView")` — `createdAt` / `updatedAt` on
/// the resource shapes, `updatedAt` on the dependency-discovery config and `connectedAt` on
/// the DevOps Agent view, none of them carrying a `@timestampFormat` override.
const TIMESTAMP_MEMBERS: [&str; 3] = ["createdAt", "updatedAt", "connectedAt"];

/// How many fractional digits an epoch-seconds timestamp this service emits may carry: a
/// millisecond's resolution, no finer.
const MAX_FRACTIONAL_DIGITS: usize = 3;

/// How a refusal names a service function in its message: the resource as the service
/// words it in prose, beside the `ServiceFunction` shape name its `resourceType` carries.
const SERVICE_FUNCTION_LABEL: &str = "Service function";

/// The values `ServiceFunctionCriticality` accepts.
///
/// SOURCE: `smithy_query(query="shape", shape="ServiceFunctionCriticality")` — an enum of
/// exactly `PRIMARY` and `SUPPLEMENTAL`.
const CRITICALITY_VALUES: [&str; 2] = ["PRIMARY", "SUPPLEMENTAL"];

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// (`resiliencehub`) and Region. Signature verification is off for these tests, so the
/// value only has to parse.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

/// What the emulator answered: the status, the `x-amzn-errortype` header restJson1 names
/// the error shape in, the RAW body (which is where a number's wire form is visible) and
/// the parsed body.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    error_type: String,
    content_length: Option<String>,
    raw: String,
    body: Value,
}

impl Answer {
    /// The named top-level member of a successful call.
    fn member(&self, case: &str, member: &str) -> Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            self.status, self.body
        );
        let value = &self.body[member];
        assert!(
            !value.is_null(),
            "case {case:?}: the {member} member is absent from {:?}",
            self.body
        );
        value.clone()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        content_length,
        raw,
        body,
    }
}

/// An ARN inside a single `@httpLabel` path segment: its `:` and `/` travel as `%3A` and
/// `%2F`, so the whole ARN stays one segment.
fn label(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

async fn create_service(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", CREATE_SERVICE, Some(body)).await
}

/// A minimal `CreateService` request: the members the model makes `@required`.
fn minimal_service(name: &str) -> Value {
    json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
    })
}

/// Create a service named `name` and answer its ARN.
async fn service_arn(router: &MultiServiceRouter, name: &str) -> String {
    let answer = create_service(router, minimal_service(name)).await;
    answer.member(name, "service")["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {:?}", answer.body))
        .to_string()
}

/// Create a policy named `name` and answer its ARN.
async fn policy_arn(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(router, "POST", CREATE_POLICY, Some(json!({"name": name}))).await;
    answer.member(name, "policy")["policyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no policy.policyArn in {:?}", answer.body))
        .to_string()
}

async fn get_service(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{GET_SERVICE}?serviceArn={arn}"), None).await
}

async fn get_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{GET_POLICY}?policyArn={arn}"), None).await
}

async fn update_service(router: &MultiServiceRouter, arn: &str, patch: &Value) -> Answer {
    let mut body = patch.clone();
    body["serviceArn"] = json!(arn);
    call(router, "POST", UPDATE_SERVICE, Some(body)).await
}

async fn create_function(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", CREATE_FUNCTION, Some(body)).await
}

async fn update_function(
    router: &MultiServiceRouter,
    service: &str,
    function_id: &str,
    patch: &Value,
) -> Answer {
    let mut body = patch.clone();
    body["serviceArn"] = json!(service);
    body["serviceFunctionId"] = json!(function_id);
    call(router, "POST", UPDATE_FUNCTION, Some(body)).await
}

async fn delete_function(
    router: &MultiServiceRouter,
    service: &str,
    function_id: &str,
) -> Answer {
    call(
        router,
        "POST",
        DELETE_FUNCTION,
        Some(json!({"serviceArn": service, "serviceFunctionId": function_id})),
    )
    .await
}

async fn list_functions(router: &MultiServiceRouter, service: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{LIST_FUNCTIONS}?serviceArn={service}"),
        None,
    )
    .await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{TAGS}/{}", label(arn)),
        Some(json!({"tags": tags})),
    )
    .await
}

/// Create one function under `service` and answer its whole `ServiceFunction` body.
async fn one_function(
    router: &MultiServiceRouter,
    service: &str,
    name: &str,
    criticality: &str,
) -> Value {
    let answer = create_function(
        router,
        json!({"serviceArn": service, "name": name, "criticality": criticality}),
    )
    .await;
    answer.member(name, "serviceFunction")
}

/// The `serviceFunctions` array of a successful listing.
fn listed(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListServiceFunctions, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["serviceFunctions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the @required serviceFunctions member is absent from {:?}",
                answer.body
            )
        })
        .clone()
}

/// `n` tags, as a request's `tags` map: `key-<i>` -> `value-<i>`.
fn tag_map(n: usize) -> Value {
    let mut map = Map::new();
    for index in 0..n {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// Assert `answer` is the not-found refusal naming `resource_type` and `resource_id`, with
/// the message wording `label` (the resource as a refusal names it in prose, which is not
/// always its shape name).
fn assert_not_found_as(
    case: &str,
    label: &str,
    resource_type: &str,
    resource_id: &str,
    answer: &Answer,
) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_type.contains("ResourceNotFoundException"),
        "case {case:?}: expected x-amzn-errortype to name ResourceNotFoundException, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.body["resourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
        answer.body["resourceType"]
    );
    assert_eq!(
        answer.body["resourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: expected resourceId {resource_id:?}, got {:?}",
        answer.body["resourceId"]
    );
    let expected = format!("{label} not found: {resource_id}");
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected message {expected:?}, got {:?}",
        answer.body["message"]
    );
}

/// Assert `answer` is the not-found refusal for a resource a refusal names by its shape
/// name.
fn assert_not_found(case: &str, resource_type: &str, resource_id: &str, answer: &Answer) {
    assert_not_found_as(case, resource_type, resource_type, resource_id, answer);
}

/// Assert `answer` is the not-found refusal for a service function: the message names the
/// resource in prose, the `resourceType` names the model's shape.
fn assert_function_not_found(case: &str, service_function_id: &str, answer: &Answer) {
    assert_not_found_as(
        case,
        SERVICE_FUNCTION_LABEL,
        SERVICE_FUNCTION_RESOURCE_TYPE,
        service_function_id,
        answer,
    );
}

/// The resource segment of an ARN — what a refusal reports as its `resourceId`.
fn arn_resource_id(arn: &str) -> &str {
    arn.splitn(6, ':').nth(5).unwrap_or(arn)
}

// ---------------------------------------------------------------------------
// Rule 1: every timestamp is an epoch-seconds number of at most millisecond resolution
// ---------------------------------------------------------------------------

/// Every `"<member>": <number>` a body carries for a timestamp member, as the literal
/// text of the number on the wire — the parsed value would hide the digits emitted.
fn timestamp_literals(raw: &str) -> Vec<(&'static str, String)> {
    let mut found = Vec::new();
    for member in TIMESTAMP_MEMBERS {
        let needle = format!("\"{member}\":");
        let mut rest = raw;
        while let Some(at) = rest.find(&needle) {
            let after = &rest[at + needle.len()..];
            let literal: String = after
                .chars()
                .take_while(|c| {
                    c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')
                })
                .collect();
            rest = &after[literal.len()..];
            found.push((member, literal));
        }
    }
    found
}

/// How many fractional digits a JSON number literal carries.
fn fractional_digits(literal: &str) -> usize {
    literal
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
}

#[tokio::test]
async fn every_timestamp_is_an_epoch_seconds_number_of_at_most_millisecond_resolution() {
    let router = emulator();

    // Rows span every projection in this crate that carries a timestamp, and both the
    // single-resource and the list shapes of each: the policy views, the service views
    // (including a service whose dependency-discovery config and DevOps Agent link carry
    // timestamps of their own) and the service-function views.
    let agent_space = format!("arn:aws:aidevops:us-west-2:{ACCOUNT}:agentspace/space-one");
    let mut service_body = minimal_service("Timestamps");
    service_body["dependencyDiscovery"] = json!("ENABLED");
    service_body["devOpsAgentConfiguration"] = json!({"agentSpaceArn": agent_space});
    let created_service = create_service(&router, service_body).await;
    let service = created_service.member("CreateService", "service")["serviceArn"]
        .as_str()
        .expect("a created service reports its ARN")
        .to_string();
    let created_policy = call(
        &router,
        "POST",
        CREATE_POLICY,
        Some(json!({"name": "Timestamps", "tags": {"team": "payments"}})),
    )
    .await;
    let policy = created_policy.member("CreatePolicy", "policy")["policyArn"]
        .as_str()
        .expect("a created policy reports its ARN")
        .to_string();
    let created_function = create_function(
        &router,
        json!({"serviceArn": service, "name": "Checkout", "criticality": "PRIMARY"}),
    )
    .await;
    let function_id = created_function.member("CreateServiceFunction", "serviceFunction")
        ["serviceFunctionId"]
        .as_str()
        .expect("a created function reports its id")
        .to_string();

    let rows: Vec<(&str, Answer)> = vec![
        ("CreatePolicy", created_policy.clone()),
        ("GetPolicy", get_policy(&router, &policy).await),
        (
            "UpdatePolicy",
            call(
                &router,
                "POST",
                UPDATE_POLICY,
                Some(json!({"policyArn": policy, "description": "touched"})),
            )
            .await,
        ),
        ("CreateService", created_service.clone()),
        ("GetService", get_service(&router, &service).await),
        (
            "UpdateService",
            update_service(&router, &service, &json!({"description": "touched"})).await,
        ),
        ("ListServices", call(&router, "GET", LIST_SERVICES, None).await),
        ("CreateServiceFunction", created_function.clone()),
        (
            "UpdateServiceFunction",
            update_function(
                &router,
                &service,
                &function_id,
                &json!({"criticality": "SUPPLEMENTAL"}),
            )
            .await,
        ),
        ("ListServiceFunctions", list_functions(&router, &service).await),
    ];

    for (case, answer) in rows {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let literals = timestamp_literals(&answer.raw);
        assert!(
            !literals.is_empty(),
            "case {case:?}: expected the body to carry a timestamp member, got {:?}",
            answer.raw
        );
        for (member, literal) in literals {
            let value: f64 = literal.parse().unwrap_or_else(|_| {
                panic!("case {case:?}: {member} must be a JSON number, got {literal:?}")
            });
            assert!(
                value > 0.0,
                "case {case:?}: {member} must be an epoch-seconds reading, got {literal:?}"
            );
            assert!(
                fractional_digits(&literal) <= MAX_FRACTIONAL_DIGITS,
                "case {case:?}: {member} must carry at most {MAX_FRACTIONAL_DIGITS} fractional \
                 digits, got {literal:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a Service body always reports dependencyDiscovery, rerunAssessment and tags
// ---------------------------------------------------------------------------

/// Assert the three always-reported members of a `Service` body.
///
/// `rerunAssessment` is expected false on every row: no operation in this crate schedules
/// an assessment rerun yet (the assessment operations are not implemented), so false is the
/// only value the store can hold, and this row asserts the member is REPORTED. Tighten it
/// to the scheduled value once an operation sets one.
fn assert_service_state(case: &str, service: &Value, expected_tags: &Value, expected_status: &str) {
    assert_eq!(
        &service["tags"], expected_tags,
        "case {case:?}: expected tags {expected_tags:?}, got {:?}",
        service["tags"]
    );
    assert_eq!(
        service["rerunAssessment"],
        json!(false),
        "case {case:?}: a service with no assessment rerun outstanding reports the flag false, \
         got {:?}",
        service["rerunAssessment"]
    );
    let discovery = &service["dependencyDiscovery"];
    assert!(
        discovery.is_object(),
        "case {case:?}: dependencyDiscovery must be reported, got {:?}",
        service["dependencyDiscovery"]
    );
    assert_eq!(
        discovery["status"].as_str(),
        Some(expected_status),
        "case {case:?}: expected dependency-discovery status {expected_status:?}, got {:?}",
        discovery["status"]
    );
    assert!(
        discovery["updatedAt"].is_number(),
        "case {case:?}: the dependency-discovery state must say when it was last set, got {:?}",
        discovery
    );
    // The DISABLED state carries its own fixed status message; the initialization state
    // carries the one for that state.
    if expected_status == "DISABLED" {
        assert_eq!(
            discovery["message"].as_str(),
            Some(DEPENDENCY_DISCOVERY_DISABLED_MESSAGE),
            "case {case:?}: expected the DISABLED status message, got {:?}",
            discovery["message"]
        );
    } else {
        assert!(
            discovery["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a status message for {expected_status}, got {:?}",
            discovery["message"]
        );
    }
}

#[tokio::test]
async fn every_service_body_reports_its_discovery_state_rerun_flag_and_tags() {
    let router = emulator();

    // Rows differ along what the create asked for: nothing beyond the required members,
    // a tag map, dependency discovery turned explicitly off, and turned on (which is
    // reported INITIALIZING until the clock settles it).
    let rows: Vec<(&str, Value, Value, &str)> = vec![
        (
            "no tags, discovery not requested",
            Value::Null,
            tag_map(0),
            "DISABLED",
        ),
        ("one tag", tag_map(1), tag_map(1), "DISABLED"),
        ("three tags", tag_map(3), tag_map(3), "DISABLED"),
        (
            "discovery requested DISABLED",
            Value::Null,
            tag_map(0),
            "DISABLED",
        ),
        (
            "discovery requested ENABLED",
            tag_map(2),
            tag_map(2),
            "INITIALIZING",
        ),
    ];

    for (index, (case, tags, expected_tags, expected_status)) in rows.iter().enumerate() {
        let mut body = minimal_service(&format!("Projected-{index}"));
        if !tags.is_null() {
            body["tags"] = tags.clone();
        }
        if *expected_status == "INITIALIZING" {
            body["dependencyDiscovery"] = json!("ENABLED");
        } else if case.contains("requested DISABLED") {
            body["dependencyDiscovery"] = json!("DISABLED");
        }

        let created = create_service(&router, body).await;
        let service = created.member(case, "service");
        assert_service_state(
            &format!("{case} / CreateService"),
            &service,
            expected_tags,
            expected_status,
        );
        let arn = service["serviceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no serviceArn in {service:?}"))
            .to_string();

        // Every other path that projects a service reports the same three members: the
        // read, an update that touches none of them, and the listing's summary (which
        // carries the discovery state too).
        let read = get_service(&router, &arn).await;
        assert_service_state(
            &format!("{case} / GetService"),
            &read.member(case, "service"),
            expected_tags,
            expected_status,
        );
        let updated = update_service(&router, &arn, &json!({"description": "touched"})).await;
        assert_service_state(
            &format!("{case} / UpdateService"),
            &updated.member(case, "service"),
            expected_tags,
            expected_status,
        );

        let listing = call(&router, "GET", LIST_SERVICES, None).await;
        let summary = listing
            .member(case, "serviceSummaries")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: serviceSummaries must be a list"))
            .iter()
            .find(|summary| summary["serviceArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {arn} is missing from the listing"))
            .clone();
        assert_eq!(
            summary["dependencyDiscovery"]["status"].as_str(),
            Some(*expected_status),
            "case {case:?}: the listing's summary must report the discovery state, got {:?}",
            summary["dependencyDiscovery"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a tag written is what the next read reports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tag_written_onto_a_resource_is_what_the_next_read_reports() {
    let router = emulator();

    // Rows differ along the resource kind the ARN names — both kinds this store holds —
    // and each runs the same sequence of writes: the first tag onto an untagged resource,
    // an overwrite of a key it already carries, several keys at once, and a key whose
    // value is the empty string.
    let service = service_arn(&router, "Tagged").await;
    let policy = policy_arn(&router, "Tagged").await;
    let kinds: Vec<(&str, String, bool)> = vec![
        ("a service", service, true),
        ("a policy", policy, false),
    ];

    let writes: Vec<(&str, Value)> = vec![
        ("the first tag", json!({"team": "payments"})),
        ("an overwrite of a key it carries", json!({"team": "checkout"})),
        ("two more keys at once", json!({"env": "prod", "tier": "1"})),
        ("a key whose value is empty", json!({"note": ""})),
    ];

    for (kind, arn, is_service) in &kinds {
        let mut expected = Map::new();
        for (step, tags) in &writes {
            let case = format!("{kind}: {step}");
            let answer = tag_resource(&router, arn, tags).await;
            assert_eq!(
                answer.status, 204,
                "case {case:?}: expected HTTP 204 from TagResource, got {} {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.raw.is_empty(),
                "case {case:?}: a 204 carries no body, got {:?}",
                answer.raw
            );
            assert_eq!(
                answer.content_length.as_deref(),
                Some("0"),
                "case {case:?}: a no-content answer states the length it has, got {:?}",
                answer.content_length
            );
            for (key, value) in tags.as_object().expect("a tag map is an object") {
                expected.insert(key.clone(), value.clone());
            }
            let expected_tags = Value::Object(expected.clone());

            let read = if *is_service {
                get_service(&router, arn).await.member(&case, "service")
            } else {
                get_policy(&router, arn).await.member(&case, "policy")
            };
            assert_eq!(
                read["tags"], expected_tags,
                "case {case:?}: the read must report the tags last written, got {:?}",
                read["tags"]
            );
        }
    }
}

#[tokio::test]
async fn tagging_a_resource_that_is_not_there_is_refused() {
    let router = emulator();

    // Rows: an ARN of each kind this store holds that names nothing, one whose resource was
    // created and then deleted, and one naming a kind this store does not hold at all.
    let deleted_service = service_arn(&router, "Doomed").await;
    let deleted = call(
        &router,
        "POST",
        DELETE_SERVICE,
        Some(json!({"serviceArn": deleted_service})),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "deleting the service under test got {:?}",
        deleted.body
    );
    let deleted_policy = policy_arn(&router, "Doomed").await;
    let gone = call(
        &router,
        "POST",
        DELETE_POLICY,
        Some(json!({"policyArn": deleted_policy})),
    )
    .await;
    assert_eq!(
        gone.status, 200,
        "deleting the policy under test got {:?}",
        gone.body
    );

    let absent_service =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:service/00000000-0000-8000-8000-000000000000");
    let absent_policy =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:policy/00000000-0000-8000-8000-000000000000");
    let other_kind =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:system/00000000-0000-8000-8000-000000000000");

    let rows: Vec<(&str, &str, &str)> = vec![
        ("a service never created", &absent_service, SERVICE_RESOURCE_TYPE),
        ("a service deleted", &deleted_service, SERVICE_RESOURCE_TYPE),
        ("a policy never created", &absent_policy, POLICY_RESOURCE_TYPE),
        ("a policy deleted", &deleted_policy, POLICY_RESOURCE_TYPE),
        // An ARN naming a kind this store holds nothing of is reported under the kind it
        // names, not under one of the two it does hold.
        ("a kind this store does not hold", &other_kind, "system"),
    ];

    for (case, arn, resource_type) in rows {
        let answer = tag_resource(&router, arn, &json!({"team": "payments"})).await;
        assert_not_found(case, resource_type, arn_resource_id(arn), &answer);
    }
}

#[tokio::test]
async fn a_tag_request_past_the_tags_a_resource_may_hold_is_refused_whole() {
    let router = emulator();

    // One short of the ceiling, so the next single key lands exactly on it and the one
    // after that is past it.
    let mut body = minimal_service("Bounded");
    body["tags"] = tag_map(MAX_TAGS_PER_RESOURCE - 1);
    let created = create_service(&router, body).await;
    let arn = created.member("at the ceiling", "service")["serviceArn"]
        .as_str()
        .expect("a created service reports its ARN")
        .to_string();

    // Exactly the ceiling: accepted.
    let at_ceiling = tag_resource(&router, &arn, &json!({"last": "one"})).await;
    assert_eq!(
        at_ceiling.status, 204,
        "case at the ceiling: expected HTTP 204, got {} {:?}",
        at_ceiling.status, at_ceiling.body
    );
    let read = get_service(&router, &arn).await.member("at the ceiling", "service");
    assert_eq!(
        read["tags"].as_object().map(serde_json::Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "case at the ceiling: expected {MAX_TAGS_PER_RESOURCE} tags, got {:?}",
        read["tags"]
    );

    // A key it already carries does not grow the map, so re-tagging at the ceiling is
    // still accepted.
    let overwrite = tag_resource(&router, &arn, &json!({"last": "again"})).await;
    assert_eq!(
        overwrite.status, 204,
        "case an overwrite at the ceiling: expected HTTP 204, got {} {:?}",
        overwrite.status, overwrite.body
    );
    let before = get_service(&router, &arn)
        .await
        .member("an overwrite at the ceiling", "service");
    assert_eq!(
        before["tags"]["last"].as_str(),
        Some("again"),
        "case an overwrite at the ceiling: expected the overwritten value, got {:?}",
        before["tags"]
    );

    // One past the ceiling: refused, and nothing of the request is applied.
    let past = tag_resource(&router, &arn, &json!({"one": "too many"})).await;
    assert_eq!(
        past.status, 400,
        "case past the ceiling: expected HTTP 400, got {} {:?}",
        past.status, past.body
    );
    assert!(
        past.error_type.contains("ValidationException"),
        "case past the ceiling: expected x-amzn-errortype to name ValidationException, got {:?}",
        past.error_type
    );
    let after = get_service(&router, &arn)
        .await
        .member("past the ceiling", "service");
    assert_eq!(
        after["tags"], before["tags"],
        "case past the ceiling: a refused request must leave the tag map as it was, got {:?}",
        after["tags"]
    );
}

// ---------------------------------------------------------------------------
// Rule 4: an update is what the next listing reports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn updating_a_service_function_is_what_the_next_listing_reports() {
    let router = emulator();
    let service = service_arn(&router, "Updates").await;
    let mut expected = one_function(&router, &service, "Checkout", "PRIMARY").await;
    let function_id = expected["serviceFunctionId"]
        .as_str()
        .expect("a created function reports its id")
        .to_string();

    // Rows differ along WHICH members the request carries: each one on its own, all of
    // them at once, a rename to the name the function already has, and a request carrying
    // nothing but the identifiers (which changes nothing).
    let rows: Vec<(&str, Value)> = vec![
        ("criticality to SUPPLEMENTAL", json!({"criticality": "SUPPLEMENTAL"})),
        ("criticality back to PRIMARY", json!({"criticality": "PRIMARY"})),
        ("description only", json!({"description": "Serves customer checkout"})),
        ("name only", json!({"name": "Checkout Flow"})),
        (
            "every updatable member at once",
            json!({
                "name": "Checkout Core",
                "description": "Takes payment",
                "criticality": "SUPPLEMENTAL",
            }),
        ),
        ("the name it already has", json!({"name": "Checkout Core"})),
        ("nothing but the identifiers", json!({})),
    ];

    for (case, patch) in rows {
        let answer = update_function(&router, &service, &function_id, &patch).await;
        let updated = answer.member(case, "serviceFunction");

        // The members the request carried hold their new values; every other member holds
        // what it held before.
        for (member, value) in patch.as_object().expect("a patch is an object") {
            expected[member.as_str()] = value.clone();
        }
        for member in [
            "serviceArn",
            "serviceFunctionId",
            "name",
            "description",
            "criticality",
            "resourceCount",
            "source",
            "createdAt",
        ] {
            assert_eq!(
                updated[member], expected[member],
                "case {case:?}: expected {member} {:?}, got {:?}",
                expected[member], updated[member]
            );
        }
        assert!(
            updated["updatedAt"].as_f64() >= updated["createdAt"].as_f64(),
            "case {case:?}: updatedAt must not precede createdAt, got {updated:?}"
        );
        expected["updatedAt"] = updated["updatedAt"].clone();

        // The listing projects the stored record, so it reports exactly what the update
        // answered.
        let listing = list_functions(&router, &service).await;
        let entries = listed(case, &listing);
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected the one function, got {entries:?}"
        );
        assert_eq!(
            entries[0], updated,
            "case {case:?}: the listing must report the updated function, got {:?}",
            entries[0]
        );
    }
}

#[tokio::test]
async fn updating_a_service_function_that_is_not_there_or_onto_a_taken_name_is_refused() {
    let router = emulator();
    let service = service_arn(&router, "Refusals").await;
    let first = one_function(&router, &service, "Checkout", "PRIMARY").await;
    let second = one_function(&router, &service, "Search", "SUPPLEMENTAL").await;
    let first_id = first["serviceFunctionId"].as_str().expect("an id").to_string();
    let second_id = second["serviceFunctionId"]
        .as_str()
        .expect("an id")
        .to_string();

    let absent_service =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:service/00000000-0000-8000-8000-000000000000");
    let answer = update_function(
        &router,
        &absent_service,
        &first_id,
        &json!({"criticality": "SUPPLEMENTAL"}),
    )
    .await;
    assert_not_found(
        "a serviceArn naming no service",
        SERVICE_RESOURCE_TYPE,
        arn_resource_id(&absent_service),
        &answer,
    );

    for (case, id) in [
        ("an id no function carries", "not-a-function-id"),
        // The function of ANOTHER service is not reachable through this one.
        ("an id from another service", "00000000-0000-8000-8000-000000000000"),
    ] {
        let answer =
            update_function(&router, &service, id, &json!({"criticality": "PRIMARY"})).await;
        assert_function_not_found(case, id, &answer);
    }

    // A rename onto a name a sibling already holds is a conflict, and nothing is written.
    let conflict = update_function(&router, &service, &first_id, &json!({"name": "Search"})).await;
    assert_eq!(
        conflict.status, 409,
        "case a name a sibling holds: expected HTTP 409, got {} {:?}",
        conflict.status, conflict.body
    );
    assert!(
        conflict.error_type.contains("ConflictException"),
        "case a name a sibling holds: expected x-amzn-errortype to name ConflictException, got \
         {:?}",
        conflict.error_type
    );
    let entries = listed("after the refused rename", &list_functions(&router, &service).await);
    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["Checkout", "Search"],
        "case a name a sibling holds: a refused rename must change nothing, got {entries:?}"
    );
    assert_eq!(
        entries[1]["serviceFunctionId"].as_str(),
        Some(second_id.as_str()),
        "the sibling must still be the function it was, got {:?}",
        entries[1]
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a deleted service function is not listed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_service_function_is_no_longer_listed() {
    let router = emulator();

    // Rows differ along how many functions the service holds and which one is deleted:
    // the only one, the first of three, the middle one and the last one.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("the only function", 1, 0),
        ("the first of three", 3, 0),
        ("the middle of three", 3, 1),
        ("the last of three", 3, 2),
    ];

    for (case, count, target) in rows {
        let service = service_arn(&router, &format!("Deletes-{case}").replace(' ', "-")).await;
        let mut created = Vec::new();
        for index in 0..count {
            created.push(
                one_function(
                    &router,
                    &service,
                    &format!("Function {index}"),
                    CRITICALITY_VALUES[index % CRITICALITY_VALUES.len()],
                )
                .await,
            );
        }
        let doomed = created[target].clone();
        let id = doomed["serviceFunctionId"].as_str().expect("an id").to_string();

        let answer = delete_function(&router, &service, &id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteServiceFunction, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["serviceFunctionId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the delete must report the function it deleted, got {:?}",
            answer.body
        );

        // The listing reports the survivors, in creation order, and nothing else. A
        // service whose only function is gone still reports the @required list, empty.
        let entries = listed(case, &list_functions(&router, &service).await);
        let survivors: Vec<Value> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != target)
            .map(|(_, function)| function.clone())
            .collect();
        assert_eq!(
            entries, survivors,
            "case {case:?}: expected the surviving functions, got {entries:?}"
        );

        // Deleting it again is a refusal, not a second success.
        let again = delete_function(&router, &service, &id).await;
        assert_function_not_found(case, &id, &again);

        // The name the deleted function held is free again, which is only true if the
        // record really left the store.
        let reused = create_function(
            &router,
            json!({
                "serviceArn": service,
                "name": doomed["name"].clone(),
                "criticality": "PRIMARY",
            }),
        )
        .await;
        assert_eq!(
            reused.status, 200,
            "case {case:?}: the deleted function's name must be free again, got {} {:?}",
            reused.status, reused.body
        );
    }
}

#[tokio::test]
async fn deleting_a_service_function_of_a_service_that_is_not_there_is_refused() {
    let router = emulator();
    let absent_service =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:service/00000000-0000-8000-8000-000000000000");
    let answer = delete_function(&router, &absent_service, "any-function-id").await;
    assert_not_found(
        "a serviceArn naming no service",
        SERVICE_RESOURCE_TYPE,
        arn_resource_id(&absent_service),
        &answer,
    );
}
