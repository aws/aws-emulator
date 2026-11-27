// SPDX-License-Identifier: CC0-1.0

//! WHICH document authorizes a rule's delivery to a target whose own service governs the
//! call by a resource-based policy — the target's own policy, or the ROLE the target names —
//! end to end over the real `MultiServiceRouter` with EventBridge, Lambda, Amazon SQS and IAM
//! mounted on one engine, so each half of the decision is read where its own service's
//! handlers wrote it.
//!
//! The RULE asserted here: EventBridge invokes a rule's target as the identity the target
//! names — `Target$RoleArn` when it names one, otherwise EventBridge's own service principal —
//! so the documents in force over THAT identity decide the delivery. For the kinds whose call
//! AWS otherwise governs by the target's own document ("for Lambda functions, Amazon SNS
//! topics and Amazon SQS queues, EventBridge relies on resource-based policies",
//! `PutTargets`), a target naming no role is admitted by that document alone, because a
//! service principal holds no identity-based policies; a target naming a role is reached AS
//! that role, and within one account "the resource-based policy and the identity-based policy
//! are evaluated together, and access is granted if either allows it" — so such a target is
//! delivered to when the role may make the call, however little its own document grants, and
//! a role EventBridge cannot assume for the invoking rule fails the handover before the call
//! is reached.
//!
//! Only a cross-service suite can state this: the rule, the match and the handover are
//! EventBridge's, the function and its asynchronous invocation queue Lambda's, the queue and
//! its `Policy` Amazon SQS's, and the role, its trust document and its permissions IAM's.
//!
//! The rows differ along what the rule generalises over: the target's SERVICE (a function and
//! a queue — one kind of each of the two decisions), whether the target names a role at all,
//! what that role TRUSTS (this service, or another service, so it cannot be assumed), what it
//! is ALLOWED (the target's own call on the target itself, that call on every resource, another
//! service's call, or nothing at all), and what the TARGET's own document grants (the
//! delivery, or nothing).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode, path_encode};

/// The Region every request below is signed for and every ARN carries.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries, and what each
/// rule's pattern names.
const SOURCE: &str = "com.demo.role.governed";
const DETAIL_TYPE: &str = "Role Governed Probe";

/// The service principal EventBridge delivers as when a target names no role, and the one a
/// target role's trust policy has to admit — written as the literal AWS publishes.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// A principal that is not this service: a role trusting only it cannot be assumed to deliver.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The action `sts:AssumeRole` a trust document has to allow, as the literal AWS publishes.
const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// The IAM action each kind of delivery performs.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";

/// The conditions EventBridge stamps on a delivery it could not make, as the literals it
/// writes: the role it could not assume, the permission it did not hold — the condition of a
/// refusal this service decided itself — and the class of an answer a target's own service
/// gave under its NOT-FOUND code, which is what Amazon SQS answers a send it will not
/// disclose the queue for.
const FAILED_TO_ASSUME_ROLE: &str = "FAILED_TO_ASSUME_ROLE";
const NO_PERMISSIONS: &str = "NO_PERMISSIONS";
const NO_RESOURCE: &str = "NO_RESOURCE";

/// A deployment package as `Code.ZipFile` carries it. What a handler computes is not what this
/// suite is about: what it asserts is which deliveries REACH the function at all.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// A well-formed execution role ARN for the function, which is not the role any target names.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long a delivery's observable end state is waited for, and how often it is looked at.
/// Generous: no row measures how fast the host runs, only what the delivery came to.
const DELIVERY_WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row that expects NOTHING to be delivered keeps looking before concluding the
/// delivery was withheld. The delivering rows prove the same wiring delivers in a fraction of
/// it, so this is the one span a slow host may spend without changing a verdict.
const WITHHELD_WINDOW: Duration = Duration::from_secs(3);

/// The state a function reaches by the clock before it can be invoked, as `GetFunction`
/// reports it (`FunctionConfiguration$State`).
const FUNCTION_ACTIVE: &str = "Active";

/// EventBridge, Lambda, Amazon SQS and IAM over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "events",
        "application/x-amz-json-1.1",
        &format!("AWSEvents.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call (`awsJson1_0`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "sqs",
        "application/x-amz-json-1.0",
        &format!("AmazonSQS.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One IAM `awsQuery` call, as its status and XML body.
async fn iam(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("authorization", authorization("iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM {action}, got {status} with {text}"
    );
    text
}

/// Which service the row's target belongs to, and so which decision governs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An AWS Lambda function: invoked with `lambda:InvokeFunction`, and reached either by
    /// the function's own resource-based policy or as the role the target names.
    Function,
    /// An Amazon SQS queue: sent to with `sqs:SendMessage`, under the same pair of documents.
    Queue,
}

/// What the row's target role is allowed to do, when the row gives it a policy at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allows {
    /// The target's own call, on the target itself — the narrowest grant that covers it.
    CallOnTarget,
    /// The target's own call, on every resource.
    CallOnEverything,
    /// Another service's call, which says nothing about this delivery.
    AnotherCall,
}

/// The role the row's target names, when it names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Role {
    /// The service principal the role's trust document admits.
    trusts: &'static str,
    /// The inline policy it carries, or `None` for a role carrying none at all.
    allows: Option<Allows>,
}

/// What a row expects of the handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The target received the event.
    Delivered,
    /// Nothing was delivered, and this is the condition EventBridge recorded for it.
    Withheld(&'static str),
}

/// What the row's TARGET carries as its own resource-based document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetPolicy {
    /// No document at all: the target has granted this service nothing.
    None,
    /// A document granting the delivery to this service's principal.
    Grants,
    /// A document that explicitly DENIES it, which no grant overturns.
    Denies,
}

/// One row: the target's kind, the role it names, what the target's own document states, and
/// what becomes of the handover.
struct Row {
    case: &'static str,
    kind: Kind,
    role: Option<Role>,
    target_policy: TargetPolicy,
    expected: Expect,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function target under a role trusting this service and allowed the invoke \
                   on the function",
            kind: Kind::Function,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::CallOnTarget),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Delivered,
        },
        Row {
            case: "a function target under such a role allowed the invoke on every resource",
            kind: Kind::Function,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::CallOnEverything),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Delivered,
        },
        Row {
            case: "a function whose own policy DENIES the invoke refuses it whatever the role \
                   allows",
            kind: Kind::Function,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::CallOnEverything),
            }),
            target_policy: TargetPolicy::Denies,
            expected: Expect::Withheld(NO_PERMISSIONS),
        },
        Row {
            case: "a function target under a role that does not trust this service",
            kind: Kind::Function,
            role: Some(Role {
                trusts: OTHER_PRINCIPAL,
                allows: Some(Allows::CallOnTarget),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Withheld(FAILED_TO_ASSUME_ROLE),
        },
        Row {
            case: "a function target under an assumable role allowed another service's call",
            kind: Kind::Function,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::AnotherCall),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Withheld(NO_PERMISSIONS),
        },
        Row {
            case: "a function target under an assumable role carrying no permissions, whose own \
                   policy grants the invoke",
            kind: Kind::Function,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: None,
            }),
            target_policy: TargetPolicy::Grants,
            expected: Expect::Delivered,
        },
        Row {
            case: "a function target naming no role, whose own policy grants the invoke",
            kind: Kind::Function,
            role: None,
            target_policy: TargetPolicy::Grants,
            expected: Expect::Delivered,
        },
        Row {
            case: "a function target naming no role and carrying no policy",
            kind: Kind::Function,
            role: None,
            target_policy: TargetPolicy::None,
            expected: Expect::Withheld(NO_PERMISSIONS),
        },
        Row {
            case: "a queue target under a role trusting this service and allowed the send on \
                   the queue",
            kind: Kind::Queue,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::CallOnTarget),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Delivered,
        },
        Row {
            case: "a queue whose own policy DENIES the send refuses it whatever the role allows",
            kind: Kind::Queue,
            role: Some(Role {
                trusts: EVENTS_PRINCIPAL,
                allows: Some(Allows::CallOnTarget),
            }),
            target_policy: TargetPolicy::Denies,
            expected: Expect::Withheld(NO_RESOURCE),
        },
        Row {
            case: "a queue target under a role that does not trust this service",
            kind: Kind::Queue,
            role: Some(Role {
                trusts: OTHER_PRINCIPAL,
                allows: Some(Allows::CallOnTarget),
            }),
            target_policy: TargetPolicy::None,
            expected: Expect::Withheld(FAILED_TO_ASSUME_ROLE),
        },
        Row {
            case: "a queue target naming no role, whose own policy grants the send",
            kind: Kind::Queue,
            role: None,
            target_policy: TargetPolicy::Grants,
            expected: Expect::Delivered,
        },
        Row {
            case: "a queue target naming no role and carrying no policy",
            kind: Kind::Queue,
            role: None,
            target_policy: TargetPolicy::None,
            expected: Expect::Withheld(NO_RESOURCE),
        },
    ]
}

/// A trust document admitting one service principal, as a client writes it.
fn trust_document(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": ASSUME_ROLE_ACTION,
        }],
    })
    .to_string()
}

/// The inline document the row's role carries: one statement allowing `action` on `resource`.
fn identity_document(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
    .to_string()
}

/// The resource-based document the row's TARGET carries: the service principal allowed the
/// delivery's own call on that target.
fn resource_document(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridgeDelivery",
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": action,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// The resource-based document a row writes to DENY the delivery: the same statement with the
/// opposite `Effect`, which no grant anywhere overturns.
fn deny_document(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "DenyEventBridgeDelivery",
            "Effect": "Deny",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": action,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// What EventBridge recorded for each handover of a routed event.
fn handovers(engine: &Arc<engine::InMemoryEngine>) -> Vec<InvocationOutcome> {
    let state = engine.state::<service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .invocations
        .iter()
        .map(|invocation| invocation.outcome.clone())
        .collect()
}

/// Every asynchronous invocation Lambda holds for `function` — the function's OWN record of
/// what reached it.
fn invocations(engine: &Arc<engine::InMemoryEngine>, function: &str) -> Vec<AsyncInvocationOutcome> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| entry.outcome.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// How many messages are waiting on the queue at `url` — read through Amazon SQS's own
/// `ReceiveMessage`, where a consumer reads them.
async fn received(router: &MultiServiceRouter, case: &str, url: &str) -> usize {
    let answered = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    answered["Messages"].as_array().map_or(0, Vec::len)
}

/// Every row: which document authorized the delivery, observed where the target's own service
/// records it.
#[tokio::test]
async fn a_target_role_grants_the_delivery_its_own_policy_withholds() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();

        // The target: a function, or a queue, each carrying its own document only where the
        // row says its own document grants the delivery.
        let function_name = format!("role-governed-{index}");
        let (status, created) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            json!({
                "FunctionName": function_name,
                "Role": EXECUTION_ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} with {created}"
        );
        let function_arn = created["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
            .to_string();

        let queue_name = format!("role-governed-{index}");
        let queue = sqs(
            &router,
            case,
            "CreateQueue",
            &json!({ "QueueName": queue_name }),
        )
        .await;
        let queue_url = queue["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {queue}"))
            .to_string();
        let attributes = sqs(
            &router,
            case,
            "GetQueueAttributes",
            &json!({ "QueueUrl": &queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
        let queue_arn = attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
            .to_string();

        let (target_arn, action) = match row.kind {
            Kind::Function => (function_arn.clone(), INVOKE_ACTION),
            Kind::Queue => (queue_arn.clone(), SEND_MESSAGE_ACTION),
        };

        match (row.target_policy, row.kind) {
            (TargetPolicy::None, _) => {}
            // "For Lambda functions … EventBridge relies on resource-based policies", and
            // `AddPermission` is what writes a GRANT; a document stating anything else — an
            // explicit deny among them — is written whole by `PutResourcePolicy`.
            (TargetPolicy::Grants, Kind::Function) => {
                let (status, granted) = lambda(
                    &router,
                    "POST",
                    &format!("/2015-03-31/functions/{function_name}/policy"),
                    json!({
                        "StatementId": "events-invoke",
                        "Action": INVOKE_ACTION,
                        "Principal": EVENTS_PRINCIPAL,
                    }),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected HTTP 201 from AddPermission, got {status} with \
                     {granted}"
                );
            }
            (TargetPolicy::Denies, Kind::Function) => {
                let (status, written) = lambda(
                    &router,
                    "PUT",
                    &format!(
                        "/2026-07-09/resource-policy/{}",
                        path_encode(&function_arn)
                    ),
                    json!({
                        "Policy": deny_document(INVOKE_ACTION, &function_arn),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from PutResourcePolicy, got {status} with \
                     {written}"
                );
            }
            (policy, Kind::Queue) => {
                let document = if policy == TargetPolicy::Denies {
                    deny_document(SEND_MESSAGE_ACTION, &queue_arn)
                } else {
                    resource_document(SEND_MESSAGE_ACTION, &queue_arn)
                };
                sqs(
                    &router,
                    case,
                    "SetQueueAttributes",
                    &json!({
                        "QueueUrl": &queue_url,
                        "Attributes": { "Policy": document },
                    }),
                )
                .await;
            }
        }

        // The role the target names, when the row names one: its trust document decides
        // whether EventBridge may act as it, and its inline policy what it may then do.
        let role_arn = match row.role {
            None => None,
            Some(role) => {
                let role_name = format!("role-governed-{index}");
                let document = trust_document(role.trusts);
                let answered = iam(
                    &router,
                    case,
                    "CreateRole",
                    &[
                        ("RoleName", &role_name),
                        ("AssumeRolePolicyDocument", &document),
                    ],
                )
                .await;
                let arn = element(&answered, "Arn");
                if let Some(allows) = role.allows {
                    let inline = match allows {
                        Allows::CallOnTarget => identity_document(action, &target_arn),
                        Allows::CallOnEverything => identity_document(action, "*"),
                        Allows::AnotherCall => identity_document("s3:PutObject", "*"),
                    };
                    iam(
                        &router,
                        case,
                        "PutRolePolicy",
                        &[
                            ("RoleName", &role_name),
                            ("PolicyName", "delivery"),
                            ("PolicyDocument", &inline),
                        ],
                    )
                    .await;
                }
                Some(arn)
            }
        };

        // The rule and its ONE target.
        let rule = format!("role-governed-rule-{index}");
        events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let mut target = json!({ "Id": "probe", "Arn": &target_arn });
        if let Some(role_arn) = &role_arn {
            target["RoleArn"] = json!(role_arn);
        }
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": [target] }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target to be accepted, got {wired}"
        );

        // A created function leaves `Pending` by the clock, so the handover is made once
        // Lambda reports it `Active` — looked for in a loop rather than waited out.
        let deadline = std::time::Instant::now() + DELIVERY_WAIT;
        loop {
            let (status, configuration) = lambda(
                &router,
                "GET",
                &format!("/2015-03-31/functions/{function_name}"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetFunction, got {status} with \
                 {configuration}"
            );
            if configuration["Configuration"]["State"].as_str() == Some(FUNCTION_ACTIVE) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: the created function never became {FUNCTION_ACTIVE}, last \
                 configuration {configuration}"
            );
            tokio::time::sleep(LOOK_INTERVAL).await;
        }

        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "probe": case }).to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the event to be accepted, got {published}"
        );

        // What the TARGET's own service holds, waited for on the condition rather than on the
        // clock: one asynchronous invocation on the function, or one message on the queue.
        let deadline = std::time::Instant::now()
            + match row.expected {
                Expect::Delivered => DELIVERY_WAIT,
                Expect::Withheld(_) => WITHHELD_WINDOW,
            };
        // The queue is READ as the loop goes, and a message read once is not read again, so
        // what reached it is accumulated; the function's own record is a count of everything
        // it holds.
        let mut enqueued = 0usize;
        let reached = loop {
            let reached = match row.kind {
                Kind::Function => invocations(&engine, &function_name).len(),
                Kind::Queue => {
                    enqueued += received(&router, case, &queue_url).await;
                    enqueued
                }
            };
            if reached > 0 && row.expected == Expect::Delivered {
                break reached;
            }
            if std::time::Instant::now() >= deadline {
                break reached;
            }
            tokio::time::sleep(LOOK_INTERVAL).await;
        };

        match row.expected {
            Expect::Delivered => {
                assert_eq!(
                    reached, 1,
                    "case {case:?}: expected the matched event to reach the target, got \
                     {reached} on it and {:?} recorded",
                    handovers(&engine)
                );
                let recorded = handovers(&engine);
                assert_eq!(
                    recorded,
                    vec![InvocationOutcome::Invoked],
                    "case {case:?}: expected the handover to be recorded as invoked, got \
                     {recorded:?}"
                );
            }
            Expect::Withheld(condition) => {
                assert_eq!(
                    reached, 0,
                    "case {case:?}: expected nothing to reach the target, got {reached} on it"
                );
                let recorded = handovers(&engine);
                let codes: Vec<String> = recorded
                    .iter()
                    .map(|outcome| match outcome {
                        InvocationOutcome::Failed { code, .. } => code.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                assert_eq!(
                    codes,
                    vec![condition.to_string()],
                    "case {case:?}: expected the handover to fail under {condition}, got \
                     {recorded:?}"
                );
            }
        }
    }
}
