// SPDX-License-Identifier: CC0-1.0

//! Subscribing a Lambda function to a log group another service VENDS its own logs into.
//!
//! RULE under test: **a subscription filter whose `destinationArn` names a Lambda function is
//! configured only where that function's own resource-based policy lets CloudWatch Logs invoke
//! it for the log group being subscribed; a put that names a function without that grant is
//! refused with `InvalidParameterException` and stores no filter — whatever the log group holds
//! and whoever writes into it — and the refusal changes nothing about the events the group goes
//! on receiving.**
//!
//! A Lambda function is the one destination kind `PutSubscriptionFilter` needs no `roleArn` for
//! ("To perform a PutSubscriptionFilter operation for any destination except a Lambda function,
//! you must also have the iam:PassRole permission"): it is INVOKED rather than written to under
//! an assumed role, so what admits the delivery is the document `lambda:AddPermission` writes,
//! which a client scopes to one log group with the source-ARN condition key.
//!
//! The topology here is the one an observability pipeline has: the log group is the destination
//! of a Step Functions state machine's `loggingConfiguration`, so the events in it are VENDED by
//! that service rather than put by the client, and the subscriber is a Lambda function. Three
//! crates have to agree — CloudWatch Logs holds the group and decides the put, AWS Lambda holds
//! the function and its policy, Step Functions vends the run's history into the group — so the
//! suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: the grant the destination function
//! carries (none at all, one scoped to ANOTHER log group, one scoped to this group, one scoped
//! to this group in the form that covers its streams, one granted to another service, one
//! granting an action that is not the invocation) and the workflow type whose run fills the
//! group (`EXPRESS`, whose executions this service's own reporting APIs do not serve, and
//! `STANDARD`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region every ARN here names.
const REGION: &str = "eu-west-2";

/// The error code a put this service will not take carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The refusal a destination this service may not invoke earns, as the service words it.
const NOT_PERMITTED_MESSAGE: &str =
    "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission \
     to execute your function.";

/// The principal a client grants a log subscription's invocation to, and the action it performs.
const LOGS_PRINCIPAL: &str = "logs.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The role the state machine runs as and the role the destination function names. No IAM is
/// mounted here, so this account holds no record for either — which is no evidence about them,
/// and both services serve the create.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/observability";

/// A workflow that fails at once, so every row's run delivers a failure.
const FAILING_DEFINITION: &str = r#"{"StartAt":"Nope","States":{"Nope":{"Type":"Fail","Error":"Boom","Cause":"the row fails on purpose"}}}"#;

/// The history event every row waits for in the vended group: the run's own failure.
const EXECUTION_FAILED: &str = "ExecutionFailed";

/// How long a row waits for the vended delivery, and how often it looks. The outcome is owed to
/// the clock, so the wait is for the CONDITION; the budget is far longer than the span either
/// service chooses, so a loaded host is slower without the rule being any different.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(25);

/// CloudWatch Logs, AWS Lambda and AWS Step Functions on ONE engine, the shape
/// `core/router/src/lib.rs` mounts them in.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope still travels, because it is what names the Region and the service.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
    /// the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        )
    }

    /// Dispatch one request as it arrives on the wire, answering `(status, parsed body)`.
    async fn dispatch(
        &self,
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Bytes,
    ) -> (u16, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .header("authorization", Self::authorization(signing_name));
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        let response = self
            .router
            .dispatch(builder.body(body).expect("a well-formed request"))
            .await;
        let status = response.status().as_u16();
        let answered = response.into_body();
        let parsed: Value = serde_json::from_slice(&answered).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One CloudWatch Logs call over its `awsJson1_1` wire.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.dispatch(
            "logs",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1".to_string()),
                ("x-amz-target", format!("Logs_20140328.{operation}")),
            ],
            Bytes::from(body.to_string()),
        )
        .await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} with {parsed}"
        );
        parsed
    }

    /// One Step Functions call over its `awsJson1_0` wire that must succeed.
    async fn states_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .dispatch(
                "states",
                "POST",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.0".to_string()),
                    ("x-amz-target", format!("AWSStepFunctions.{operation}")),
                ],
                Bytes::from(body.to_string()),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} with {parsed}"
        );
        parsed
    }

    /// One AWS Lambda call over its `restJson1` wire.
    async fn lambda(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        self.dispatch(
            "lambda",
            method,
            path,
            &[("content-type", "application/json".to_string())],
            Bytes::from(payload),
        )
        .await
    }

    /// Create the function a row subscribes, answering the ARN Lambda assigned it.
    async fn created_function(&self, case: &str, name: &str) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                "/2015-03-31/functions",
                Some(json!({
                    "FunctionName": name,
                    "Role": ROLE_ARN,
                    "Runtime": "python3.12",
                    "Handler": "handler.main",
                    // The package's bytes are never run: this suite is about the function's
                    // POLICY, and a put this service refuses invokes nothing at all.
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
            .unwrap_or_else(|| {
                panic!("case {case:?}: a created function carries a FunctionArn, got {created}")
            })
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

    /// The `type` of every event the log group `group` holds, as a client reading the group sees
    /// them. A group nothing has written to yet is reported as holding nothing.
    async fn vended_event_types(&self, group: &str) -> Vec<String> {
        let (status, page) = self.logs("FilterLogEvents", &json!({"logGroupName": group})).await;
        if status != 200 {
            return Vec::new();
        }
        page["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str().map(str::to_string))
            .filter_map(|message| serde_json::from_str::<Value>(&message).ok())
            .filter_map(|message| message["type"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait until the log group `group` holds the run's failure event, answering every event type
    /// it then holds. Waits for the CONDITION, bounded by [`DELIVERY_DEADLINE`].
    async fn delivered_event_types(&self, case: &str, group: &str) -> Vec<String> {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let types = self.vended_event_types(group).await;
            if types.iter().any(|event| event == EXECUTION_FAILED) {
                return types;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the vended group never held a {EXECUTION_FAILED} event, it holds \
                 {types:?}"
            );
            tokio::time::sleep(POLL).await;
        }
    }
}

/// The qualified ARN of a log group, the form `logGroupArn` takes and the form a function
/// permission's source-ARN condition names a group's streams by.
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        engine::emulator_account_id()
    )
}

/// What one row grants on the function it subscribes.
enum Grant {
    /// No `AddPermission` at all, so the function reports no policy.
    None,
    /// One statement: the principal, the action, and the log group ARN it is scoped to, where
    /// the row scopes it.
    Statement {
        principal: &'static str,
        action: &'static str,
        /// `Some(builder)` scopes the statement to the ARN the builder makes of the row's own
        /// group; `None` leaves it unscoped.
        scope: Option<fn(&str) -> String>,
    },
}

/// One row: the workflow type whose run fills the group, what the destination function grants,
/// and whether the put is therefore configured.
struct Row {
    case: &'static str,
    /// Names this row's log group, state machine, function and filter, so no two rows share one.
    name: &'static str,
    /// `EXPRESS` or `STANDARD`.
    sm_type: &'static str,
    grant: Grant,
    permitted: bool,
}

/// The group being subscribed, in the form that covers its streams — the form the documented
/// permission for this wiring is scoped to.
fn streams_of(group: &str) -> String {
    format!("{}:*", log_group_arn(group))
}

/// The group being subscribed, without the stream-covering suffix.
fn group_itself(group: &str) -> String {
    log_group_arn(group)
}

/// ANOTHER log group of the same account, which is a grant this subscription cannot use.
fn another_group(_group: &str) -> String {
    format!("{}:*", log_group_arn("observability/elsewhere"))
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an express workflow's group and a function with no resource policy at all",
            name: "express-ungranted",
            sm_type: "EXPRESS",
            grant: Grant::None,
            permitted: false,
        },
        Row {
            case: "an express workflow's group and a grant scoped to another log group",
            name: "express-elsewhere",
            sm_type: "EXPRESS",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(another_group),
            },
            permitted: false,
        },
        Row {
            case: "an express workflow's group and a grant scoped to its streams",
            name: "express-granted",
            sm_type: "EXPRESS",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(streams_of),
            },
            permitted: true,
        },
        Row {
            case: "a standard workflow's group and a grant scoped to the group itself",
            name: "standard-granted",
            sm_type: "STANDARD",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: INVOKE_ACTION,
                scope: Some(group_itself),
            },
            permitted: true,
        },
        Row {
            case: "a standard workflow's group and a grant to another service",
            name: "standard-other-service",
            sm_type: "STANDARD",
            grant: Grant::Statement {
                principal: "events.amazonaws.com",
                action: INVOKE_ACTION,
                scope: None,
            },
            permitted: false,
        },
        Row {
            case: "a standard workflow's group and a grant of an action that is not the invocation",
            name: "standard-other-action",
            sm_type: "STANDARD",
            grant: Grant::Statement {
                principal: LOGS_PRINCIPAL,
                action: "lambda:GetFunction",
                scope: None,
            },
            permitted: false,
        },
    ]
}

/// A Lambda subscriber of a vended log group is configured only where the function permits this
/// service to invoke it for that group, and the group goes on taking its vended events either
/// way.
#[tokio::test]
async fn a_vended_groups_lambda_subscriber_needs_the_invoke_grant() {
    for row in rows() {
        let emulator = Emulator::new();
        let case = row.case;
        let group = format!("/aws/vendedlogs/states/{}", row.name);
        let function_name = format!("subscriber-{}", row.name);
        let filter_name = format!("{}-to-lambda", row.name);

        emulator
            .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;
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
                    scope.map(|scope| scope(&group)).as_deref(),
                )
                .await;
        }

        let (status, parsed) = emulator
            .logs(
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": filter_name,
                    "filterPattern": "",
                    "destinationArn": function,
                }),
            )
            .await;

        if row.permitted {
            assert_eq!(
                status, 200,
                "case {case:?}: the function permits the invocation, got {status} with {parsed}"
            );
            assert_eq!(
                emulator.filters_of(case, &group).await,
                vec![filter_name.clone()],
                "case {case:?}: the configured filter is reported back"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the put to be refused, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["__type"]
                    .as_str()
                    .or_else(|| parsed["code"].as_str())
                    .map(|code| code.rsplit('#').next().unwrap_or(code).to_string()),
                Some(INVALID_PARAMETER.to_string()),
                "case {case:?}: expected {INVALID_PARAMETER}, got {parsed}"
            );
            assert_eq!(
                parsed["message"].as_str(),
                Some(NOT_PERMITTED_MESSAGE),
                "case {case:?}: expected {NOT_PERMITTED_MESSAGE:?}, got {parsed}"
            );
            assert!(
                emulator.filters_of(case, &group).await.is_empty(),
                "case {case:?}: a refused put stores no subscription filter"
            );
        }

        // Whatever the put came to, the state machine goes on vending its runs into the group:
        // the events a client reads there are the record of the run, and the refusal above took
        // nothing away from them.
        let created = emulator
            .states_ok(
                case,
                "CreateStateMachine",
                &json!({
                    "name": row.name,
                    "definition": FAILING_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "type": row.sm_type,
                    "loggingConfiguration": {
                        "level": "ALL",
                        "includeExecutionData": true,
                        "destinations": [{
                            "cloudWatchLogsLogGroup": {"logGroupArn": streams_of(&group)},
                        }],
                    },
                }),
            )
            .await;
        let state_machine = created["stateMachineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created machine carries an ARN, got {created}"))
            .to_string();
        emulator
            .states_ok(
                case,
                "StartExecution",
                &json!({"stateMachineArn": state_machine, "name": "run", "input": "{}"}),
            )
            .await;

        let delivered = emulator.delivered_event_types(case, &group).await;
        assert!(
            delivered.iter().any(|event| event == EXECUTION_FAILED),
            "case {case:?}: expected the run's {EXECUTION_FAILED} event in the group, got \
             {delivered:?}"
        );
    }
}
