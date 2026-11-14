// SPDX-License-Identifier: CC0-1.0

//! The capacity window a registration declares, enforced on the PROVISIONED CONCURRENCY of the
//! Lambda qualifier a `lambda` scalable target names.
//!
//! RULE: registering a scalable target resolves the capacity the resource holds in the service
//! that owns it and, when that capacity is outside the window the registration declares,
//! Application Auto Scaling moves the resource to the bound it is outside of with no scaling
//! policy involved, recording ONE scaling activity for the movement — "Application Auto Scaling
//! retrieves the current capacity of the resource. If it's below the minimum capacity or above
//! the maximum capacity, Application Auto Scaling adjusts the capacity of the scalable target to
//! place it within these bounds, even if you don't include the MinCapacity or MaxCapacity request
//! parameters". For `lambda:function:ProvisionedConcurrency` the capacity is the provisioned
//! concurrency of ONE qualified function, which the owning service holds as a configuration on
//! that qualifier: a qualifier carrying no configuration holds none of it, and the movement is
//! written where a client reads the concurrency back — through Lambda's own
//! `GetProvisionedConcurrencyConfig`.
//!
//! The rows differ along what the rule generalises over: which bound the capacity is outside of
//! (the floor and the ceiling), the magnitude of the movement, the kind of qualifier the resource
//! id names (an alias and a published version), a capacity already inside the window, which moves
//! nothing, and a ceiling of none at all, which leaves the qualifier holding no provisioned
//! concurrency.
//!
//! This suite builds its OWN emulator rather than using the shared one
//! ([`super::emulator::Emulator`]), because the resource lives in a service the other suites do
//! not mount: AWS Lambda, where the function, its version and its alias are created through that
//! service's own API.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::REGION;

/// The one dimension every row scales.
const DIMENSION: &str = "lambda:function:ProvisionedConcurrency";

/// Where Lambda binds the function operations this suite calls.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where Lambda binds the provisioned-concurrency operations, which are of a later API date
/// than the function operations.
const PROVISIONED_CONCURRENCY_URI: &str = "/2019-09-30/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The alias every function in this suite carries, pointed at its published version.
const ALIAS: &str = "BLUE";

/// The status a provisioned-concurrency allocation that is in effect reports
/// (`ProvisionedConcurrencyStatusEnum`).
const READY: &str = "READY";

/// The status a capacity movement whose change took effect settles in.
const SUCCESSFUL: &str = "Successful";

/// How long a case keeps polling for what the clock owes: well past the span this service gives
/// an activity, the span the owning service gives an allocation and the quarter-second this
/// service's in-process clock waits between passes, so a loaded machine still reaches it.
const POLL_FOR: Duration = Duration::from_secs(30);

/// How long a poll sleeps between reads.
const POLL_EVERY: Duration = Duration::from_millis(100);

/// Application Auto Scaling, AWS Lambda — which owns the provisioned concurrency — and the two
/// services a registration also touches, over ONE engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests; the `Authorization` header is still what routes each
    // request and names its region, as a signed client's would.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// Call one Application Auto Scaling operation.
async fn application_auto_scaling(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AnyScaleFrontendService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/application-autoscaling\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    (status, serde_json::from_slice(&payload).unwrap_or(Value::Null))
}

/// Call one Lambda operation, which that service binds to a REST route.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/lambda\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    (status, serde_json::from_slice(&payload).unwrap_or(Value::Null))
}

/// Create one function, published, with an alias on its published version — the resource a
/// provisioned-concurrency target is registered on — and wait for it to be invocable, which is
/// what the owning service lets a configuration be added to.
async fn given_function(router: &MultiServiceRouter, case: &str, function: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        Some(json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": {"ZipFile": base64::engine::general_purpose::STANDARD.encode(&package)},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: setup CreateFunction expected 201, got {status} {created:?}"
    );
    let (status, alias) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        Some(json!({"Name": ALIAS, "FunctionVersion": "1"})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: setup CreateAlias expected 201, got {status} {alias:?}"
    );
    // The function is provisioned over a span of the owning service's own, so the setup waits
    // for the CONDITION it needs — an active function — rather than for a fixed time.
    let deadline = SystemTime::now() + POLL_FOR;
    let mut reported = function_state(router, case, function).await;
    while reported != "Active" && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        reported = function_state(router, case, function).await;
    }
    assert_eq!(
        reported, "Active",
        "case {case:?}: setup expected the function to reach Active, got {reported:?}"
    );
}

/// The `State` Lambda reports for one function's unpublished configuration.
async fn function_state(router: &MultiServiceRouter, case: &str, function: &str) -> String {
    let (status, body) = lambda(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{function}/configuration"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup GetFunctionConfiguration expected 200, got {status} {body:?}"
    );
    body["State"].as_str().unwrap_or_default().to_string()
}

/// Add a provisioned-concurrency configuration to one qualifier and wait for the allocation to be
/// in effect, so a row starts from a capacity the owning service really holds.
async fn given_provisioned(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    qualifier: &str,
    executions: i64,
) {
    let (status, body) = lambda(
        router,
        "PUT",
        &format!(
            "{PROVISIONED_CONCURRENCY_URI}/{function}/provisioned-concurrency?Qualifier={qualifier}"
        ),
        Some(json!({"ProvisionedConcurrentExecutions": executions})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: setup PutProvisionedConcurrencyConfig expected 202, got {status} {body:?}"
    );
    let wanted = Allocation {
        requested: executions,
        allocated: executions,
        available: executions,
        status: READY.to_string(),
    };
    let reached = poll_until_allocated(router, function, qualifier, &Some(wanted.clone())).await;
    assert_eq!(
        reached,
        Some(wanted.clone()),
        "case {case:?}: setup expected the allocation {wanted:?} to be in effect, got {reached:?}"
    );
}

/// What Lambda reports for one qualifier's provisioned-concurrency configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Allocation {
    requested: i64,
    allocated: i64,
    available: i64,
    status: String,
}

/// The configuration Lambda holds for one qualifier right now, or `None` for a qualifier that
/// holds none at all — the answer `ProvisionedConcurrencyConfigNotFoundException` gives.
async fn reported_allocation(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
) -> Option<Allocation> {
    let (status, body) = lambda(
        router,
        "GET",
        &format!(
            "{PROVISIONED_CONCURRENCY_URI}/{function}/provisioned-concurrency?Qualifier={qualifier}"
        ),
        None,
    )
    .await;
    if status != 200 {
        assert_eq!(
            status, 404,
            "expected a qualifier holding no configuration to answer the modelled 404, got \
             {status} {body:?}"
        );
        return None;
    }
    Some(Allocation {
        requested: body["RequestedProvisionedConcurrentExecutions"]
            .as_i64()
            .unwrap_or(-1),
        allocated: body["AllocatedProvisionedConcurrentExecutions"]
            .as_i64()
            .unwrap_or(-1),
        available: body["AvailableProvisionedConcurrentExecutions"]
            .as_i64()
            .unwrap_or(-1),
        status: body["Status"].as_str().unwrap_or_default().to_string(),
    })
}

/// Poll until Lambda reports `wanted` for one qualifier, or until the budget is up — so a failing
/// case reports what it last saw instead of hanging.
async fn poll_until_allocated(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
    wanted: &Option<Allocation>,
) -> Option<Allocation> {
    let deadline = SystemTime::now() + POLL_FOR;
    let mut last = reported_allocation(router, function, qualifier).await;
    while &last != wanted && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        last = reported_allocation(router, function, qualifier).await;
    }
    last
}

/// The scaling activities of one target, newest first.
async fn activities(router: &MultiServiceRouter, resource_id: &str) -> Vec<Value> {
    let (status, body) = application_auto_scaling(
        router,
        "DescribeScalingActivities",
        json!({
            "ServiceNamespace": "lambda",
            "ResourceId": resource_id,
            "ScalableDimension": DIMENSION,
            "IncludeNotScaledActivities": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll until the target's activities have settled — every movement in its terminal status, and
/// none at all for a row that moves nothing — or until the budget is up.
async fn poll_until_settled(
    router: &MultiServiceRouter,
    resource_id: &str,
    wanted: usize,
) -> Vec<Value> {
    let settled = |listed: &[Value]| {
        listed.len() == wanted
            && listed
                .iter()
                .all(|activity| activity["StatusCode"] == json!(SUCCESSFUL))
    };
    let deadline = SystemTime::now() + POLL_FOR;
    let mut listed = activities(router, resource_id).await;
    while !settled(&listed) && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        listed = activities(router, resource_id).await;
    }
    listed
}

/// Register one scalable target, asserting the call was accepted.
async fn register(router: &MultiServiceRouter, case: &str, resource_id: &str, window: (i64, i64)) {
    let (min, max) = window;
    let (status, body) = application_auto_scaling(
        router,
        "RegisterScalableTarget",
        json!({
            "ServiceNamespace": "lambda",
            "ResourceId": resource_id,
            "ScalableDimension": DIMENSION,
            "MinCapacity": min,
            "MaxCapacity": max,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the registration to be accepted, got {status} {body:?}"
    );
}

/// One registration whose window is enforced on the provisioned concurrency of a qualifier.
struct Row {
    case: &'static str,
    /// The qualifier the resource id names: an alias, or a published version number.
    qualifier: &'static str,
    /// The concurrency the qualifier holds before the registration, or `None` for a qualifier
    /// holding no configuration at all.
    provisioned: Option<i64>,
    /// The window the registration declares.
    window: (i64, i64),
    /// The concurrency the qualifier must hold once the registration has settled, or `None` for
    /// one that must hold no configuration at all.
    expected: Option<i64>,
    /// The `Cause` of the activity the movement is recorded as, or `None` for a row whose
    /// capacity is already inside the window and so moves nothing.
    cause: Option<&'static str>,
}

/// The catalogue.
const ROWS: &[Row] = &[
    Row {
        case: "an alias holding nothing, pulled up to the floor",
        qualifier: ALIAS,
        provisioned: None,
        window: (2, 10),
        expected: Some(2),
        cause: Some("minimum capacity was set to 2"),
    },
    Row {
        case: "an alias holding nothing, pulled up by nine",
        qualifier: ALIAS,
        provisioned: None,
        window: (9, 12),
        expected: Some(9),
        cause: Some("minimum capacity was set to 9"),
    },
    Row {
        case: "an allocation pushed down to the ceiling",
        qualifier: ALIAS,
        provisioned: Some(6),
        window: (1, 2),
        expected: Some(2),
        cause: Some("maximum capacity was set to 2"),
    },
    Row {
        case: "an allocation already inside the window, which moves nothing",
        qualifier: ALIAS,
        provisioned: Some(4),
        window: (2, 7),
        expected: Some(4),
        cause: None,
    },
    Row {
        case: "a published version holding nothing, pulled up to the floor",
        qualifier: "1",
        provisioned: None,
        window: (3, 5),
        expected: Some(3),
        cause: Some("minimum capacity was set to 3"),
    },
    Row {
        case: "a published version pushed down to a ceiling of none at all",
        qualifier: "1",
        provisioned: Some(2),
        window: (0, 0),
        expected: None,
        cause: Some("maximum capacity was set to 0"),
    },
];

/// The function one row's resource id names: one per case, so the rows cannot see each other's
/// provisioned concurrency.
fn function_name(index: usize) -> String {
    format!("pc-case-{index}")
}

/// The allocation a row expects the owning service to report once its registration has settled.
fn expected_allocation(row: &Row) -> Option<Allocation> {
    row.expected.map(|executions| Allocation {
        requested: executions,
        allocated: executions,
        available: executions,
        status: READY.to_string(),
    })
}

#[tokio::test]
async fn a_registration_moves_a_lambda_qualifier_into_the_window_it_declares() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        let function = function_name(index);
        given_function(&router, case, &function).await;
        if let Some(executions) = row.provisioned {
            given_provisioned(&router, case, &function, row.qualifier, executions).await;
        }
        let resource_id = format!("function:{function}:{}", row.qualifier);

        register(&router, case, &resource_id, row.window).await;

        let listed =
            poll_until_settled(&router, &resource_id, usize::from(row.cause.is_some())).await;
        match row.cause {
            None => assert!(
                listed.is_empty(),
                "case {case:?}: expected a capacity already inside the window to record no \
                 activity, got {listed:?}"
            ),
            Some(cause) => {
                assert_eq!(
                    listed.len(),
                    1,
                    "case {case:?}: expected exactly one activity for the movement, got {listed:?}"
                );
                let activity = &listed[0];
                let moved = row.expected.unwrap_or(0);
                let expected = json!({
                    "ServiceNamespace": "lambda",
                    "ResourceId": resource_id,
                    "ScalableDimension": DIMENSION,
                    "Cause": cause,
                    "Description": format!("Setting provisioned concurrency to {moved}."),
                    "StatusCode": SUCCESSFUL,
                    "StatusMessage": format!(
                        "Successfully set provisioned concurrency to {moved}. Change successfully \
                         fulfilled by lambda."
                    ),
                });
                for (member, want) in expected.as_object().expect("an object of members") {
                    assert_eq!(
                        &activity[member], want,
                        "case {case:?}: expected {member} {want:?}, got {activity:?}"
                    );
                }
                assert!(
                    activity["EndTime"].as_f64().is_some(),
                    "case {case:?}: expected a settled activity to report an EndTime, got \
                     {activity:?}"
                );
            }
        }

        let wanted = expected_allocation(row);
        let reached = poll_until_allocated(&router, &function, row.qualifier, &wanted).await;
        assert_eq!(
            reached, wanted,
            "case {case:?}: expected Lambda to report {wanted:?}, got {reached:?} with \
             activities {listed:?}"
        );
    }
}

/// The same rule applied TWICE to one target: the second registration is enforced on the capacity
/// the first one left the resource at, and each movement is an activity of its own.
#[tokio::test]
async fn re_registering_a_narrower_window_moves_the_qualifier_again() {
    let case = "a floor that raises, then a ceiling that lowers";
    let router = router();
    let function = "pc-both-bounds";
    given_function(&router, case, function).await;
    let resource_id = format!("function:{function}:{ALIAS}");

    register(&router, case, &resource_id, (2, 10)).await;
    let raised = Some(Allocation {
        requested: 2,
        allocated: 2,
        available: 2,
        status: READY.to_string(),
    });
    let reached = poll_until_allocated(&router, function, ALIAS, &raised).await;
    assert_eq!(
        reached, raised,
        "case {case:?}: expected the floor to raise the allocation to {raised:?}, got {reached:?}"
    );
    let first = poll_until_settled(&router, &resource_id, 1).await;
    assert_eq!(
        first.len(),
        1,
        "case {case:?}: expected the floor's movement to settle as one activity, got {first:?}"
    );

    register(&router, case, &resource_id, (1, 1)).await;
    let lowered = Some(Allocation {
        requested: 1,
        allocated: 1,
        available: 1,
        status: READY.to_string(),
    });
    let reached = poll_until_allocated(&router, function, ALIAS, &lowered).await;
    assert_eq!(
        reached, lowered,
        "case {case:?}: expected the ceiling to lower the allocation to {lowered:?}, got \
         {reached:?}"
    );
    let both = poll_until_settled(&router, &resource_id, 2).await;
    assert_eq!(
        both.len(),
        2,
        "case {case:?}: expected a second, distinct activity for the ceiling, got {both:?}"
    );
    let causes: Vec<&str> = both
        .iter()
        .map(|activity| activity["Cause"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        causes,
        vec!["maximum capacity was set to 1", "minimum capacity was set to 2"],
        "case {case:?}: expected the two bounds to be the two causes, newest first, got {both:?}"
    );
    assert_ne!(
        both[0]["ActivityId"], both[1]["ActivityId"],
        "case {case:?}: expected two distinct ActivityIds, got {both:?}"
    );
}
