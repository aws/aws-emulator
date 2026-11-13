// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the grant a subscription filter's LAMBDA destination has to carry
//! before `PutSubscriptionFilter` will configure it.
//!
//! The rule under test: **a Lambda destination is authorised by the FUNCTION's own
//! resource-based policy — the document `lambda:AddPermission` writes — so a
//! `PutSubscriptionFilter` whose `destinationArn` names a function that does not let CloudWatch
//! Logs invoke it for THAT log group is refused with `InvalidParameterException` and no filter
//! is stored.** A function is the one destination kind the operation needs no `roleArn` for
//! ("To perform a PutSubscriptionFilter operation for any destination except a Lambda function,
//! you must also have the iam:PassRole permission"), because it is invoked rather than written
//! to under an assumed role; a client scopes that permission to the log group it is subscribing
//! with the source-ARN condition key.
//!
//! CloudWatch Logs and AWS Lambda are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so every function a row names is a function Lambda itself holds,
//! carrying the policy Lambda's own `AddPermission` wrote, and no part of this suite tells
//! CloudWatch Logs anything about it.
//!
//! The rows span what the rule generalises over: a function with no policy at all, one whose
//! permission names another service, one scoped to ANOTHER log group, one scoped to this log
//! group in each of the two ARN forms a log group is named by, one with no scope at all, one
//! granting an action that is not the invocation, and a destination ARN naming no function
//! Lambda holds.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for, which is the Region the ARNs carry.
const REGION: &str = "eu-west-2";

/// The error code every refusal of this rule carries, as the operation declares it for a
/// parameter it will not take.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The principal a client grants a log subscription's invocation to.
const LOGS_PRINCIPAL: &str = "logs.amazonaws.com";

/// The IAM action the invocation performs, which is what a grant has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The execution role a created function names. No IAM is mounted here, so no record is held
/// for it — which is no evidence about the role, and Lambda serves the create.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/fan-out";

/// CloudWatch Logs and AWS Lambda on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what
    /// routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        )
    }

    /// One CloudWatch Logs call over its `awsJson1_1` wire: its status and parsed body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("logs.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header("authorization", Self::authorization("logs"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = response.into_body();
        let parsed: Value = serde_json::from_slice(&answered).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One AWS Lambda call over its `restJson1` wire: its status and parsed body.
    async fn lambda(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("lambda"))
            .body(bytes::Bytes::from(payload))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = response.into_body();
        let parsed: Value = serde_json::from_slice(&answered).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Create the function a row delivers to, answering the ARN Lambda assigned it.
    async fn created_function(&self, case: &str, name: &str) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                "/2015-03-31/functions",
                Some(json!({
                    "FunctionName": name,
                    "Role": ROLE,
                    "Runtime": "python3.12",
                    "Handler": "handler.main",
                    // The package's bytes are never run here: this suite is about the
                    // function's POLICY, and `PutSubscriptionFilter` invokes nothing.
                    "Code": {"ZipFile": aws_smithy_types::base64::encode("deployment package")},
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} {created}"
        );
        created["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created function carries a FunctionArn, got {created}"))
            .to_string()
    }

    /// Write one statement into the function's resource-based policy through `AddPermission`,
    /// which is how a client grants this service the invocation.
    async fn added_permission(
        &self,
        case: &str,
        function: &str,
        principal: &str,
        action: &str,
        source_arn: Option<&str>,
    ) {
        let mut body = json!({
            "StatementId": "cloudwatch-logs-subscription",
            "Action": action,
            "Principal": principal,
        });
        if let Some(source_arn) = source_arn {
            body["SourceArn"] = json!(source_arn);
        }
        let (status, answered) = self
            .lambda(
                "POST",
                &format!("/2015-03-31/functions/{function}/policy"),
                Some(body),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} {answered}"
        );
    }

    /// The filter names CloudWatch Logs reports for `group`.
    async fn filters_of(&self, case: &str, group: &str) -> Vec<String> {
        let page = self
            .logs_ok(
                case,
                "DescribeSubscriptionFilters",
                &json!({"logGroupName": group}),
            )
            .await;
        page["subscriptionFilters"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|filter| filter["filterName"].as_str().map(str::to_string))
            .collect()
    }
}

/// A log group's ARN in the form a function permission's source-ARN condition names it by.
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        engine::emulator_account_id()
    )
}

/// The refusal a destination this service may not invoke earns: fixed text for the class,
/// naming neither the function nor the log group.
const NOT_PERMITTED_MESSAGE: &str =
    "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission \
     to execute your function.";

/// Which log group a row's permission is scoped to, and in which of the two ARN forms a log
/// group is named by — the `arn` form ending in the `:*` that covers the group's streams (the
/// form the AWS documentation scopes this wiring's permission to) and the `logGroupArn` form
/// without it.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// The group being subscribed, in the form covering its streams.
    SubscribedStreams,
    /// The group being subscribed, without the stream-covering suffix.
    Subscribed,
    /// Another log group of the same account, in the form covering its streams.
    OtherStreams,
}

impl Scope {
    /// The `SourceArn` a permission scoped this way carries.
    fn source_arn(self) -> String {
        match self {
            Self::SubscribedStreams => format!("{}:*", log_group_arn(SUBSCRIBED_GROUP)),
            Self::Subscribed => log_group_arn(SUBSCRIBED_GROUP),
            Self::OtherStreams => format!("{}:*", log_group_arn(OTHER_GROUP)),
        }
    }
}

/// What one row grants on the function it delivers to.
enum Grant {
    /// No `AddPermission` at all, so the function reports no policy.
    None,
    /// One statement: the principal, the action and the log group it is scoped to, where the
    /// row scopes it.
    Statement {
        principal: &'static str,
        action: &'static str,
        scope: Option<Scope>,
    },
}

/// One row: what the function grants, and whether the put is configured.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The grant the function carries.
    grant: Grant,
    /// Whether the destination resolves to a function this service may invoke.
    permitted: bool,
}

/// The log group each row subscribes, and another one a permission may be scoped to.
const SUBSCRIBED_GROUP: &str = "subscription/lambda";
const OTHER_GROUP: &str = "subscription/elsewhere";

/// The rows, in the order they are driven.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a function with no resource policy at all",
            grant: Grant::None,
            permitted: false,
        },
        Row {
            case: "a permission scoped to this log group, in the form covering its streams",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(Scope::SubscribedStreams),
            },
            permitted: true,
        },
        Row {
            case: "a permission scoped to this log group, without the stream suffix",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(Scope::Subscribed),
            },
            permitted: true,
        },
        Row {
            case: "a permission scoped to another log group",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(Scope::OtherStreams),
            },
            permitted: false,
        },
        Row {
            case: "a permission with no source-ARN scope",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: None,
            },
            permitted: true,
        },
        Row {
            case: "a permission granted to another service",
            grant: Grant::Statement {
                principal: "events.amazonaws.com",
                action: INVOKE_ACTION,
                scope: None,
            },
            permitted: false,
        },
        Row {
            case: "a permission granting an action that is not the invocation",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: "lambda:GetFunction",
                scope: None,
            },
            permitted: false,
        },
    ]
}

/// A put is configured only for a destination whose own policy lets this service invoke it for
/// the log group being subscribed, and a refused put stores no filter.
#[tokio::test]
async fn a_lambda_destination_is_configured_only_when_it_permits_the_invocation() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let case = row.case;
        let group = SUBSCRIBED_GROUP;
        emulator
            .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;

        let function_name = format!("fan-out-{index}");
        let function = emulator.created_function(case, &function_name).await;
        if let Grant::Statement {
            principal,
            action,
            scope,
        } = row.grant
        {
            emulator
                .added_permission(
                    case,
                    &function_name,
                    principal,
                    action,
                    scope.map(Scope::source_arn).as_deref(),
                )
                .await;
        }

        let (status, parsed) = emulator
            .logs(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": "to-lambda",
                    "filterPattern": "",
                    "destinationArn": function,
                }),
            )
            .await;

        if row.permitted {
            assert_eq!(
                status, 200,
                "case {case:?}: the function permits the invocation, got {status} {parsed}"
            );
            assert_eq!(
                emulator.filters_of(case, group).await,
                vec!["to-lambda".to_string()],
                "case {case:?}: the configured filter is reported back"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the put to be refused, got {status} {parsed}"
        );
        assert_eq!(
            crate::harness::error_code(&parsed),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(NOT_PERMITTED_MESSAGE),
            "case {case:?}: expected {NOT_PERMITTED_MESSAGE:?}, got {parsed}"
        );
        assert!(
            emulator.filters_of(case, group).await.is_empty(),
            "case {case:?}: a refused put stores no subscription filter"
        );
    }
}

/// A destination ARN naming a function Lambda holds no record for is refused the same way: there
/// is no document granting this service the invocation.
#[tokio::test]
async fn a_destination_naming_no_function_is_refused() {
    let emulator = Emulator::new();
    let case = "a function that was never created";
    let group = SUBSCRIBED_GROUP;
    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    let absent = format!(
        "arn:aws:lambda:{REGION}:{}:function:never-created",
        engine::emulator_account_id()
    );
    let (status, parsed) = emulator
        .logs(
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "to-lambda",
                "filterPattern": "",
                "destinationArn": absent,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the put to be refused, got {status} {parsed}"
    );
    assert_eq!(
        crate::harness::error_code(&parsed),
        INVALID_PARAMETER,
        "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
    );
    assert!(
        emulator.filters_of(case, group).await.is_empty(),
        "case {case:?}: a refused put stores no subscription filter"
    );
}

/// A put on a group whose filter the function already carries a scoped permission for is
/// configured, while the SAME function refuses the group its permission does not name: the gate
/// is per log group rather than per function.
#[tokio::test]
async fn one_scoped_permission_configures_only_the_group_it_names() {
    let emulator = Emulator::new();
    let case = "one function, two log groups";
    for group in [SUBSCRIBED_GROUP, OTHER_GROUP] {
        emulator
            .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    let function_name = "fan-out-scoped";
    let function = emulator.created_function(case, function_name).await;
    emulator
        .added_permission(
            case,
            function_name,
            LOGS_PRINCIPAL,
            INVOKE_ACTION,
            Some(&Scope::SubscribedStreams.source_arn()),
        )
        .await;

    let put = |group: &'static str| {
        json!({
            "logGroupName": group,
            "filterName": "to-lambda",
            "filterPattern": "",
            "destinationArn": function.clone(),
        })
    };

    emulator
        .logs_ok(case, "PutSubscriptionFilter", &put(SUBSCRIBED_GROUP))
        .await;
    assert_eq!(
        emulator.filters_of(case, SUBSCRIBED_GROUP).await,
        vec!["to-lambda".to_string()],
        "case {case:?}: the group the permission names is configured"
    );

    let (status, parsed) = emulator
        .logs("PutSubscriptionFilter", &put(OTHER_GROUP))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the group the permission does not name is refused, got {status} {parsed}"
    );
    assert!(
        emulator.filters_of(case, OTHER_GROUP).await.is_empty(),
        "case {case:?}: no filter is stored on the group the permission does not name"
    );
}
