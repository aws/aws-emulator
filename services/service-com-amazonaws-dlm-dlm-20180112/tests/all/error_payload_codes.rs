// SPDX-License-Identifier: CC0-1.0

//! The `Code` member of EVERY error Amazon Data Lifecycle Manager answers, on
//! the wire.
//!
//! One rule, asserted over every error class this service can answer rather than
//! the one payload an issue cited:
//!
//! > Every error this service answers reports each member its shape declares,
//! > and the `Code` member reports the error code of that exception CLASS — the
//! > member a client branches on beside the shape name. It is reported whichever
//! > class the error belongs to and whichever operation answers it: a client that
//! > reads `$.Code` never gets `null`.
//!
//! The four error shapes the service answers are `InvalidRequestException` (400),
//! `ResourceNotFoundException` (404), `LimitExceededException` (429) and the
//! service-common `InternalServerException` (500)
//! (`smithy_query(query="digest")`), and the model declares `Code` on all four
//! (`smithy_query(query="shape", shape="LimitExceededException")`: `Message`,
//! `Code`, `ResourceType`; `smithy_query(query="shape",
//! shape="InternalServerException")`: `Message`, `Code`).
//!
//! The rows differ along the class, the operation answering it, the HTTP method
//! and the shape of the request that provokes it — and, for the 429, along where
//! in the store the policies that reached the bound live.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most requests in this file are signed for.
const REGION: &str = "us-west-2";

/// Two more regions, for the rows that show the store's bound spans the whole
/// store rather than one region's slice of it.
const SECOND_REGION: &str = "eu-west-1";
const THIRD_REGION: &str = "ap-northeast-1";

/// The error classes this service answers: the shape a restJson1 client resolves,
/// its `@httpError` status, and the `Code` the service reports for the class.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

const LIMIT_EXCEEDED_EXCEPTION: &str = "LimitExceededException";
const LIMIT_EXCEEDED_STATUS: u16 = 429;
const LIMIT_EXCEEDED_CODE: &str = "LimitExceeded";

const INTERNAL_SERVER_EXCEPTION: &str = "InternalServerException";
const INTERNAL_SERVER_STATUS: u16 = 500;
const INTERNAL_ERROR_CODE: &str = "InternalError";

/// The `ResourceType` token a lifecycle policy carries, which the two shapes
/// that declare the member report.
const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// An engine that is not the shared in-memory engine this service keeps its
/// policies in, so every handler mounted over it finds no store to serve a
/// request from — the one condition the service's 500 reports.
#[derive(Debug)]
struct EngineWithoutAStore;

/// An emulator whose handlers can reach no store at all.
fn emulator_without_a_store() -> MultiServiceRouter {
    let engine: Arc<EngineWithoutAStore> = Arc::new(EngineWithoutAStore);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the `x-amzn-errortype` header
/// and the JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error shape this answer names, as a restJson1 client resolves it: the
    /// `x-amzn-errortype` header, falling back to the body's `__type`, with any
    /// `namespace#` prefix stripped.
    fn error_shape(&self) -> &str {
        let named = if self.error_type.is_empty() {
            self.body["__type"].as_str().unwrap_or_default()
        } else {
            self.error_type.as_str()
        };
        named.rsplit('#').next().unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned.
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// One request a client sends: the method, the URI and the body, as the operation
/// it names binds them (`smithy_query(query="digest")`).
#[derive(Debug)]
struct Call {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// One row of the client-error table: a request, and the error class it must be
/// answered with.
#[derive(Debug)]
struct Refusal {
    call: Call,
    /// The error shape, its `@httpError` status and the `Code` of its class.
    shape: &'static str,
    status: u16,
    code: &'static str,
}

/// A SigV4 credential scope naming this service in `region`; it is what routes a
/// request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    call_in(router, REGION, method, uri, body).await
}

/// Send one table row, in `REGION`.
async fn send(router: &MultiServiceRouter, request: &Call) -> Answer {
    call(
        router,
        request.method,
        &request.uri,
        request.body.clone(),
    )
    .await
}

/// The role every policy here names: one this account may pass.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// The ARN of the policy `policy_id` names in `region`, as `PolicyArn`'s
/// `@pattern` spells it.
fn policy_arn(region: &str, policy_id: &str) -> String {
    format!(
        "arn:aws:dlm:{region}:{}:policy/{policy_id}",
        engine::emulator_account_id()
    )
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// A well-formed `CreateLifecyclePolicy` request, as a client sends it.
fn create_request(description: &str) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": {
            "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
            "ResourceTypes": ["VOLUME"],
            "TargetTags": [{"Key": "backup", "Value": "daily"}],
            "Schedules": [{
                "Name": "Daily",
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }],
        },
    })
}

/// Assert `answer` is the modelled error of `shape`, with that class's status and
/// `Code`, and a `Message` the shape declares.
fn assert_error(case: &str, answer: &Answer, shape: &str, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_shape(),
        shape,
        "case {case:?}: expected the modelled {shape}, got {:?} with {:?}",
        answer.error_type,
        answer.body
    );
    assert_eq!(
        answer.body["Code"],
        json!(code),
        "case {case:?}: expected Code {code:?}, got {:?}",
        answer.body
    );
    assert!(
        answer.body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: the modelled Message member must state the refusal, got {:?}",
        answer.body
    );
}

/// The 400 and the 404 — the two classes with a parameter or a resource to name —
/// each report the `Code` of their class, over the operations and methods that
/// answer them.
#[tokio::test]
async fn the_client_error_classes_report_their_code() {
    let router = emulator();
    let created = call(&router, "POST", "/policies", Some(create_request("codes"))).await;
    assert_eq!(
        created.status, 200,
        "the subject policy must be created, got {} with {:?}",
        created.status, created.body
    );
    let policy_id = created.policy_id();

    // A policy id, and an ARN, the store resolves to nothing: the far end of the
    // id space, which the store mints from `policy-0000000000000001` upwards.
    let missing_id = "policy-0fffffffffffffff";
    let missing_arn = policy_arn(REGION, missing_id);

    let rows = vec![
        Refusal {
            call: Call {
                case: "CreateLifecyclePolicy with a Description outside its @pattern",
                method: "POST",
                uri: "/policies".to_string(),
                body: Some(json!({
                    "ExecutionRoleArn": role_arn(),
                    // `PolicyDescription` is `^[0-9A-Za-z _-]+$`.
                    "Description": "no commas, please",
                    "State": "ENABLED",
                    "PolicyDetails": {"ResourceTypes": ["VOLUME"]},
                })),
            },
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        Refusal {
            call: Call {
                case: "CreateLifecyclePolicy describing no policy at all",
                method: "POST",
                uri: "/policies".to_string(),
                body: Some(json!({"ExecutionRoleArn": role_arn(), "State": "ENABLED"})),
            },
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        Refusal {
            call: Call {
                case: "UpdateLifecyclePolicy with a State outside the enum",
                method: "PATCH",
                uri: format!("/policies/{policy_id}"),
                body: Some(json!({"State": "PAUSED"})),
            },
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        Refusal {
            call: Call {
                // A `TagResource` row for the modelled 400 class: the reserved
                // tag-key prefix is NOT one, being answered in the
                // malformed-input class that declares no `Code` member (see
                // `error_paths::a_reserved_aws_tag_key_may_not_be_set`), so the
                // row that keeps this operation in the table is a `ResourceArn`
                // outside `PolicyArn`'s `@pattern` instead.
                case: "TagResource with a ResourceArn outside its @pattern",
                method: "POST",
                uri: tags_uri(&policy_arn(REGION, "policy.not-an-id")),
                body: Some(json!({"Tags": {"team": "backups"}})),
            },
            shape: INVALID_REQUEST_EXCEPTION,
            status: INVALID_REQUEST_STATUS,
            code: INVALID_PARAMETER_CODE,
        },
        Refusal {
            call: Call {
                case: "GetLifecyclePolicy for an id that resolves to nothing",
                method: "GET",
                uri: format!("/policies/{missing_id}"),
                body: None,
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        Refusal {
            call: Call {
                case: "DeleteLifecyclePolicy for an id that resolves to nothing",
                method: "DELETE",
                uri: format!("/policies/{missing_id}"),
                body: None,
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        Refusal {
            call: Call {
                case: "UpdateLifecyclePolicy for an id that resolves to nothing",
                method: "PATCH",
                uri: format!("/policies/{missing_id}"),
                body: Some(json!({"State": "DISABLED"})),
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        Refusal {
            call: Call {
                case: "ListTagsForResource for an ARN that resolves to nothing",
                method: "GET",
                uri: tags_uri(&missing_arn),
                body: None,
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        Refusal {
            call: Call {
                case: "TagResource for an ARN that resolves to nothing",
                method: "POST",
                uri: tags_uri(&missing_arn),
                body: Some(json!({"Tags": {"owner": "team"}})),
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
        Refusal {
            call: Call {
                case: "UntagResource for an ARN that resolves to nothing",
                method: "DELETE",
                uri: format!("{}?tagKeys=owner", tags_uri(&missing_arn)),
                body: None,
            },
            shape: RESOURCE_NOT_FOUND_EXCEPTION,
            status: RESOURCE_NOT_FOUND_STATUS,
            code: RESOURCE_NOT_FOUND_CODE,
        },
    ];

    for row in &rows {
        let case = row.call.case;
        let answer = send(&router, &row.call).await;
        assert_error(case, &answer, row.shape, row.status, row.code);
        if row.shape == RESOURCE_NOT_FOUND_EXCEPTION {
            assert_eq!(
                answer.body["ResourceType"],
                json!(POLICY_RESOURCE_TYPE),
                "case {case:?}: the shape declares ResourceType too, got {:?}",
                answer.body
            );
        }
    }
}

/// The 500 — the only server fault this service answers, and the one error class
/// every operation models — reports the `Code` of its class on every one of the
/// eight operations.
#[tokio::test]
async fn the_server_fault_reports_its_code_on_every_operation() {
    let router = emulator_without_a_store();

    // A well-formed id and ARN: these rows must reach the handler, so nothing
    // here may be refused by the constraint walk ahead of it.
    let policy_id = "policy-0000000000000001";
    let arn = policy_arn(REGION, policy_id);

    // Every operation the service binds (`smithy_query(query="digest")`), each
    // with a request the modelled constraints accept.
    let rows = vec![
        Call {
            case: "CreateLifecyclePolicy",
            method: "POST",
            uri: "/policies".to_string(),
            body: Some(create_request("no store")),
        },
        Call {
            case: "GetLifecyclePolicies",
            method: "GET",
            uri: "/policies".to_string(),
            body: None,
        },
        Call {
            case: "GetLifecyclePolicy",
            method: "GET",
            uri: format!("/policies/{policy_id}"),
            body: None,
        },
        Call {
            case: "UpdateLifecyclePolicy",
            method: "PATCH",
            uri: format!("/policies/{policy_id}"),
            body: Some(json!({"State": "DISABLED"})),
        },
        Call {
            case: "DeleteLifecyclePolicy",
            method: "DELETE",
            uri: format!("/policies/{policy_id}"),
            body: None,
        },
        Call {
            case: "ListTagsForResource",
            method: "GET",
            uri: tags_uri(&arn),
            body: None,
        },
        Call {
            case: "TagResource",
            method: "POST",
            uri: tags_uri(&arn),
            body: Some(json!({"Tags": {"owner": "team"}})),
        },
        Call {
            case: "UntagResource",
            method: "DELETE",
            uri: format!("{}?tagKeys=owner", tags_uri(&arn)),
            body: None,
        },
    ];

    for row in &rows {
        let operation = row.case;
        let answer = send(&router, row).await;
        assert_error(
            operation,
            &answer,
            INTERNAL_SERVER_EXCEPTION,
            INTERNAL_SERVER_STATUS,
            INTERNAL_ERROR_CODE,
        );
        // The shape declares `Message` and `Code` and nothing else, so no
        // not-found or limit member belongs to it.
        for absent in ["ResourceType", "ResourceIds", "RequiredParameters"] {
            assert_eq!(
                answer.body[absent],
                Value::Null,
                "case {operation:?}: {absent} is not a member of {INTERNAL_SERVER_EXCEPTION}, \
                 got {:?}",
                answer.body
            );
        }
    }
}

/// The 429 reports the `Code` of its class beside the `ResourceType` it already
/// reported — whichever region the create that reaches the bound was signed for,
/// and whichever regions hold the policies that reached it.
#[tokio::test]
async fn the_limit_refusal_reports_its_code() {
    // The store's own bound, read from the crate rather than restated here.
    let quota = service_com_amazonaws_dlm_dlm_20180112::state::POLICY_QUOTA;
    let router = emulator();

    // Fill the store to its bound from TWO regions: the bound is on the store, so
    // what the regions hold adds up.
    let in_second_region = quota / 4;
    for index in 0..quota {
        let region = if index < in_second_region {
            SECOND_REGION
        } else {
            REGION
        };
        let answer = call_in(
            &router,
            region,
            "POST",
            "/policies",
            Some(create_request(&format!("filler {index}"))),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a create below the bound must be accepted, got {} with {:?}",
            format!("policy {index} of {quota} in {region}"),
            answer.status,
            answer.body
        );
    }

    // Past the bound every create is refused, in a region holding policies and in
    // one holding none.
    for case in [REGION, SECOND_REGION, THIRD_REGION] {
        let answer = call_in(
            &router,
            case,
            "POST",
            "/policies",
            Some(create_request("past the bound")),
        )
        .await;
        assert_error(
            case,
            &answer,
            LIMIT_EXCEEDED_EXCEPTION,
            LIMIT_EXCEEDED_STATUS,
            LIMIT_EXCEEDED_CODE,
        );
        assert_eq!(
            answer.body["ResourceType"],
            json!(POLICY_RESOURCE_TYPE),
            "case {case:?}: the shape declares the ResourceType the limit was reached for, \
             got {:?}",
            answer.body
        );
        assert!(
            answer.body["Message"]
                .as_str()
                .is_some_and(|message| message.contains(&quota.to_string())),
            "case {case:?}: the Message must name the limit reached, got {:?}",
            answer.body
        );
    }

    // The bound is on what the store HOLDS, so releasing one record admits one
    // more create — and the refusal above was not a latched state.
    let listed = call_in(&router, SECOND_REGION, "GET", "/policies", None).await;
    let released = listed.body["Policies"][0]["PolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let deleted = call_in(
        &router,
        SECOND_REGION,
        "DELETE",
        &format!("/policies/{released}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the filler policy {released:?} must be deletable, got {} with {:?}",
        deleted.status, deleted.body
    );
    let accepted = call_in(
        &router,
        THIRD_REGION,
        "POST",
        "/policies",
        Some(create_request("back under the bound")),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a create back under the bound must be accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
}
