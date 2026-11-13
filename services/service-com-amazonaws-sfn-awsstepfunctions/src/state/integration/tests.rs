// SPDX-License-Identifier: CC0-1.0

//! The integration seam's own tests: the pure readings this module is made of — which
//! call a `Resource` names, what a request carries, what an answer is read as, and what a
//! served invocation whose function errored is to the `Task` state — each pinned over a
//! table of the forms the rule spans.
//!
//! Split out of `super`'s file so that file stays inside the project's length limit; the
//! tests are unchanged by the move.

use super::*;

/// One row of the `Resource` table: the case, the `Resource`, the service's
/// signing name and the operation it names when a call is made for it, and
/// whether that call STARTS a job the task is parked at (the run-a-job pattern).
type ResourceRow = (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
    bool,
);

/// One row of the answer table: the case, the status, the error-type header, the
/// body, and the result document or the error name expected of it.
type AnswerRow = (
    &'static str,
    u16,
    Option<&'static str>,
    &'static str,
    Result<&'static str, &'static str>,
);

/// One row of the REST request table: the case, the parameters the state built,
/// and the request-target, bound headers and body expected of it — `None` for
/// parameters no request can be built from.
type RestRow = (
    &'static str,
    Value,
    Option<(&'static str, Vec<(&'static str, &'static str)>, &'static str)>,
);

/// One row of the access-request table: the case, the `Resource`, the request the
/// state built, and the `(action, resource)` pair the call is subject to.
type AccessRow = (
    &'static str,
    &'static str,
    Value,
    Option<(&'static str, String)>,
);

/// One row of the envelope table: the case, the document the operation answered,
/// and the headers its answer carried.
type EnvelopeRow = (&'static str, Value, Vec<(&'static str, &'static str)>);

/// One row of the instant table: the case, the members to convert, the answer and
/// the result reported for it.
type InstantRow = (&'static str, &'static [&'static str], Value, Value);

/// Which call a `Resource` names, over a TABLE of the forms a definition writes
/// one in: the two integration families, the services in [`APIS`] and the forms
/// that name no call this emulator makes.
#[test]
fn a_resource_names_the_call_its_action_does() {
    // (case, the Resource, the signing name and operation it names, parks at a job)
    let rows: [ResourceRow; 34] = [
        (
            "the optimized DynamoDB integration",
            "arn:aws:states:::dynamodb:putItem",
            Some(("dynamodb", "PutItem")),
            false,
        ),
        (
            "another action of the same service, which no code here names",
            "arn:aws:states:::dynamodb:updateItem",
            Some(("dynamodb", "UpdateItem")),
            false,
        ),
        (
            "the AWS SDK integration of the same operation",
            "arn:aws:states:::aws-sdk:dynamodb:putItem",
            Some(("dynamodb", "PutItem")),
            false,
        ),
        (
            "an AWS SDK integration whose service's SDK name is not its ARN namespace",
            "arn:aws:states:::aws-sdk:sfn:listStateMachines",
            Some(("states", "ListStateMachines")),
            false,
        ),
        (
            "an AWS SDK integration with EventBridge, whose SDK name is not 'events'",
            "arn:aws:states:::aws-sdk:eventbridge:putEvents",
            Some(("events", "PutEvents")),
            false,
        ),
        (
            "an ARN namespace the AWS SDK family does not name a service by",
            "arn:aws:states:::aws-sdk:states:listStateMachines",
            None,
            false,
        ),
        (
            "an action whose name is one word",
            "arn:aws:states:::aws-sdk:sqs:receiveMessage",
            Some(("sqs", "ReceiveMessage")),
            false,
        ),
        (
            "an EventBridge action",
            "arn:aws:states:::events:putEvents",
            Some(("events", "PutEvents")),
            false,
        ),
        (
            "a nested execution",
            "arn:aws:states:::states:startExecution",
            Some(("states", "StartExecution")),
            false,
        ),
        (
            "a Kinesis action",
            "arn:aws:states:::aws-sdk:kinesis:putRecord",
            Some(("kinesis", "PutRecord")),
            false,
        ),
        (
            "a Systems Manager action",
            "arn:aws:states:::aws-sdk:ssm:getParameter",
            Some(("ssm", "GetParameter")),
            false,
        ),
        (
            "an ECS action",
            "arn:aws:states:::ecs:runTask",
            Some(("ecs", "RunTask")),
            false,
        ),
        // A service whose protocol carries its request as a FORM: the action names the
        // operation there too, under either family.
        (
            "the optimized SNS publish",
            "arn:aws:states:::sns:publish",
            Some(("sns", "Publish")),
            false,
        ),
        (
            "the AWS SDK integration of that same operation",
            "arn:aws:states:::aws-sdk:sns:publish",
            Some(("sns", "Publish")),
            false,
        ),
        // A service whose protocol is a REST one: the call is made for the
        // operation whose own bindings are carried, and for no other.
        (
            "the optimized Lambda integration",
            "arn:aws:states:::lambda:invoke",
            Some(("lambda", "Invoke")),
            false,
        ),
        (
            "the AWS SDK integration of that same operation",
            "arn:aws:states:::aws-sdk:lambda:invoke",
            Some(("lambda", "Invoke")),
            false,
        ),
        // Another REST service, whose operations bind no payload member at all: the
        // write, a read addressed by its path and a read filtered by its query are
        // each a call, and an operation whose bindings are not carried is not.
        (
            "an AWS SDK integration with AWS Backup",
            "arn:aws:states:::aws-sdk:backup:startBackupJob",
            Some(("backup", "StartBackupJob")),
            false,
        ),
        (
            "a read of the same service, addressed by its own query",
            "arn:aws:states:::aws-sdk:backup:listBackupJobs",
            Some(("backup", "ListBackupJobs")),
            false,
        ),
        (
            "an operation of that service whose bindings are not carried",
            "arn:aws:states:::aws-sdk:backup:listTags",
            None,
            false,
        ),
        // The run-a-job pattern: the ACTION is what is called, and the task is
        // parked at the job that call starts.
        (
            "the run-a-job pattern over a nested execution",
            "arn:aws:states:::states:startExecution.sync",
            Some(("states", "StartExecution")),
            true,
        ),
        (
            "its second revision",
            "arn:aws:states:::states:startExecution.sync:2",
            Some(("states", "StartExecution")),
            true,
        ),
        (
            "the run-a-job pattern over an Athena query",
            "arn:aws:states:::athena:startQueryExecution.sync",
            Some(("athena", "StartQueryExecution")),
            true,
        ),
        // The forms that name no call.
        (
            "an operation of a REST service whose bindings are not carried",
            "arn:aws:states:::aws-sdk:lambda:listFunctions",
            None,
            false,
        ),
        (
            "a service with no entry at all",
            "arn:aws:states:::batch:submitJob",
            None,
            false,
        ),
        (
            "the run-a-job pattern over an integration whose job cannot be reported",
            "arn:aws:states:::dynamodb:putItem.sync",
            None,
            false,
        ),
        // The callback pattern: the ACTION is what is called, and the task is
        // suspended at the token that call carried rather than parked at a job.
        (
            "the callback pattern over an SQS message",
            "arn:aws:states:::sqs:sendMessage.waitForTaskToken",
            Some(("sqs", "SendMessage")),
            false,
        ),
        (
            "the callback pattern over a Lambda invocation",
            "arn:aws:states:::lambda:invoke.waitForTaskToken",
            Some(("lambda", "Invoke")),
            false,
        ),
        (
            "a pattern this service does not name",
            "arn:aws:states:::states:startExecution.whenever",
            None,
            false,
        ),
        (
            "an activity, whose work is a worker's rather than a call",
            "arn:aws:states:us-west-2:000000000000:activity:worker",
            None,
            false,
        ),
        // A LAMBDA FUNCTION's ARN is work in itself: the task invokes that
        // function, through the same operation the optimized integration calls.
        (
            "a Lambda function's ARN",
            "arn:aws:lambda:us-west-2:000000000000:function:f",
            Some(("lambda", "Invoke")),
            false,
        ),
        (
            "the same function at one of its aliases",
            "arn:aws:lambda:us-west-2:000000000000:function:f:live",
            Some(("lambda", "Invoke")),
            false,
        ),
        (
            "a bare resource ARN of a service whose resources are not work",
            "arn:aws:dynamodb:us-west-2:000000000000:table/t",
            None,
            false,
        ),
        (
            "a Lambda ARN naming no function",
            "arn:aws:lambda:us-west-2:000000000000:layer:l",
            None,
            false,
        ),
        ("text that is not an ARN", "putItem", None, false),
    ];

    for (case, resource, expected, parks) in rows {
        match (read(resource), expected) {
            (Ok(call), Some((signing_name, operation))) => {
                assert_eq!(
                    call.api.signing_name, signing_name,
                    "case {case:?}: expected the service {signing_name}, got {}",
                    call.api.signing_name
                );
                assert_eq!(
                    call.operation, operation,
                    "case {case:?}: expected the operation {operation}, got {}",
                    call.operation
                );
                assert_eq!(
                    call.job.is_some(),
                    parks,
                    "case {case:?}: expected parks-at-a-job {parks}, got {:?}",
                    call.job.map(|job| job.starts)
                );
            }
            (Err(why), None) => assert!(
                why.contains("nothing was run for it"),
                "case {case:?}: a resource no call is made for must say nothing ran, got \
                     {why:?}"
            ),
            (Ok(call), None) => panic!(
                "case {case:?}: expected no call, got {}.{}",
                call.api.signing_name, call.operation
            ),
            (Err(why), Some(expected)) => {
                panic!("case {case:?}: expected {expected:?}, got the refusal {why:?}")
            }
        }
    }
}

/// What one answer means for the task, over a TABLE spanning the answer forms a
/// JSON service gives: a document, an empty success, and a refusal in each of
/// the spellings its error code may arrive in.
#[test]
fn an_answer_is_the_result_or_the_error_the_service_named() {
    let api = APIS
        .iter()
        .find(|api| api.namespace == "dynamodb")
        .expect("DynamoDB is a service a task calls");
    // (case, status, the error-type header, the body, the expected answer)
    let rows: [AnswerRow; 11] = [
        (
            "a response document is the state's result",
            200,
            None,
            r#"{"Attributes":{"pk":{"S":"a"}}}"#,
            Ok(r#"{"Attributes":{"pk":{"S":"a"}}}"#),
        ),
        (
            "an operation whose output carries nothing answers the empty document",
            200,
            None,
            "",
            Ok("{}"),
        ),
        (
            "an empty JSON body is itself the empty document",
            200,
            None,
            "{}",
            Ok("{}"),
        ),
        (
            "the error type names the error",
            400,
            Some("ConditionalCheckFailedException"),
            r#"{"message":"the conditional request failed"}"#,
            Err("DynamoDB.ConditionalCheckFailedException"),
        ),
        (
            "a header carrying a shape id and a URL",
            400,
            Some("com.amazonaws.dynamodb#ResourceNotFoundException:http://internal"),
            r#"{"message":"no such table"}"#,
            Err("DynamoDB.ResourceNotFoundException"),
        ),
        (
            "the body's __type when no header names it",
            400,
            None,
            r#"{"__type":"com.amazonaws.dynamodb#ValidationException","message":"bad"}"#,
            Err("DynamoDB.ValidationException"),
        ),
        (
            "a refusal naming no code at all",
            500,
            None,
            "",
            Err(TASK_FAILED_ERROR),
        ),
        (
            "a gap in this emulator is not a refusal of the call",
            501,
            Some("NotImplemented"),
            r#"{"message":"not implemented"}"#,
            Err(super::super::asl::RUNTIME_ERROR),
        ),
        (
            // DynamoDB models `TransactionConflict` without the suffix its SDK
            // class carries, which is the half of the rule a shape already
            // spelled `…Exception` does not exercise.
            "a modelled shape the SDK suffixes",
            400,
            Some("TransactionConflict"),
            r#"{"message":"the transaction conflicted"}"#,
            Err("DynamoDB.TransactionConflictException"),
        ),
        (
            // One of AWS's common errors: no shape of any service's model
            // declares it, so the client's base exception carries it.
            "a common error every service answers",
            400,
            Some("MissingParameter"),
            r#"{"message":"The request must contain the parameter TableName."}"#,
            Err("DynamoDB.AmazonDynamoDBException"),
        ),
        (
            // A parameter-level refusal names the shape of the request framework
            // in front of the service, which belongs to no service's model: the
            // base exception carries that too.
            "a shape of the framework in front of the service",
            400,
            None,
            r#"{"__type":"com.amazon.coral.service#MissingRequiredParameterException",
                    "message":"The request must contain the parameter TableName."}"#,
            Err("DynamoDB.AmazonDynamoDBException"),
        ),
    ];

    for (case, status, error_type, body, expected) in rows {
        let answer = answer_of(
            api,
            "PutItem",
            status,
            error_type,
            body.as_bytes(),
            Some("a-request-id"),
            None,
        );
        match (&answer, expected) {
            (Ok(result), Ok(document)) => {
                let expected: Value =
                    ::serde_json::from_str(document).expect("the expected result is JSON");
                assert_eq!(
                    *result, expected,
                    "case {case:?}: expected the result {expected}, got {result}"
                );
            }
            (Err(failed), Err(error)) => {
                assert_eq!(
                    failed.error, error,
                    "case {case:?}: expected the error {error}, got {failed:?}"
                );
                assert!(
                    !failed.cause.is_empty(),
                    "case {case:?}: a failed task must say what happened, got {failed:?}"
                );
                // A refusal the service itself answered is reported as its SDK
                // exception's own message: the service's sentence, then the
                // metadata that answer carried.
                if error.starts_with("DynamoDB.") {
                    assert!(
                        failed.cause.contains("(Service: AmazonDynamoDBv2; Status Code: ")
                            && failed
                                .cause
                                .ends_with("Request ID: a-request-id; Proxy: null)"),
                        "case {case:?}: expected the SDK exception's own message, got \
                             {failed:?}"
                    );
                }
            }
            _ => panic!("case {case:?}: expected {expected:?}, got {answer:?}"),
        }
    }
}

/// A QUERY-COMPATIBLE service names a refusal twice — by its error shape and by the
/// legacy query code its old vocabulary knows the class as — and each name is read
/// for what it decides: the shape decides the exception class the error name
/// carries, the legacy code is what that exception reports as the code.
#[test]
fn a_query_compatible_refusal_is_named_by_its_shape_and_reports_its_legacy_code() {
    let api = APIS
        .iter()
        .find(|api| api.namespace == "sqs")
        .expect("Amazon SQS is a service a task calls");
    // (case, the error shape, the legacy code, the error name, the code reported)
    let rows: [(&str, &str, &str, &str, &str); 2] = [
        (
            "a shape the service's own model declares",
            "QueueDoesNotExist",
            "AWS.SimpleQueueService.NonExistentQueue",
            "SQS.QueueDoesNotExistException",
            "AWS.SimpleQueueService.NonExistentQueue",
        ),
        (
            "a parameter-level refusal, which belongs to no model of the service",
            "com.amazon.coral.service#MissingRequiredParameterException",
            "MissingParameter",
            "SQS.AmazonSQSException",
            "MissingParameter",
        ),
    ];

    for (case, shape, legacy, error, reported) in rows {
        let body = format!(r#"{{"__type":"{shape}","message":"refused."}}"#);
        let answer = answer_of(
            api,
            "SendMessage",
            400,
            None,
            body.as_bytes(),
            None,
            Some(legacy),
        );
        let failed = answer.expect_err("a 400 is a refusal");
        assert_eq!(
            failed.error, error,
            "case {case:?}: expected the error {error}, got {failed:?}"
        );
        assert_eq!(
            failed.cause,
            format!(
                "refused. (Service: AmazonSQS; Status Code: 400; Error Code: {reported}; \
                     Proxy: null)"
            ),
            "case {case:?}: expected the SDK exception's own message, got {failed:?}"
        );
    }
}

/// What one request looks like on the wire, over a TABLE spanning the two
/// member conventions a model spells its input in, both spellings a definition
/// may write a member under, the nested data that is NOT re-spelled, and the
/// member an integration documents as a document while the operation types it
/// as a string.
#[test]
fn a_request_carries_each_member_as_the_invoked_model_spells_it() {
    // (case, the Resource, the document the state built, the document sent)
    let rows: [(&str, &str, Value, Value); 6] = [
        (
            "a lowerCamel model's member, written in the integration's PascalCase",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"StateMachineArn": "arn:sm", "Name": "run"}),
            ::serde_json::json!({"stateMachineArn": "arn:sm", "name": "run"}),
        ),
        (
            "the same member already written as that model spells it",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"stateMachineArn": "arn:sm"}),
            ::serde_json::json!({"stateMachineArn": "arn:sm"}),
        ),
        (
            "a PascalCase model's members, which the convention leaves alone",
            "arn:aws:states:::dynamodb:putItem",
            ::serde_json::json!({"TableName": "t", "Item": {"pk": {"S": "a"}}}),
            ::serde_json::json!({"TableName": "t", "Item": {"pk": {"S": "a"}}}),
        ),
        (
            "nested data is a caller's own, so no key below the top level is re-spelled",
            "arn:aws:states:::aws-sdk:ecs:runTask",
            ::serde_json::json!({"TaskDefinition": "d", "Overrides": {"ContainerOverrides": []}}),
            ::serde_json::json!({"taskDefinition": "d", "overrides": {"ContainerOverrides": []}}),
        ),
        (
            "the child execution's input, which the operation types as a string",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"StateMachineArn": "arn:sm", "Input": {"a": [1, 2]}}),
            ::serde_json::json!({"stateMachineArn": "arn:sm", "input": r#"{"a":[1,2]}"#}),
        ),
        (
            "that input already written as the text the operation types",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"Input": "{\"a\":1}"}),
            ::serde_json::json!({"input": "{\"a\":1}"}),
        ),
    ];

    for (case, resource, built, expected) in rows {
        let call = read(resource).expect("every row names a call");
        let sent = wire_request(&call, &built);
        assert_eq!(
            sent, expected,
            "case {case:?}: expected the request {expected}, got {sent}"
        );
    }
}

/// An AWS SDK integration converts nothing: its parameters ARE the invoked
/// operation's own members, so the member the optimized integration takes as a
/// document is the operation's string there.
#[test]
fn an_aws_sdk_integration_carries_the_operations_own_members() {
    let call = read("arn:aws:states:::aws-sdk:sfn:startExecution").expect("this names a call");
    let sent = wire_request(&call, &::serde_json::json!({"Input": "{\"a\":1}"}));
    assert_eq!(
        sent,
        ::serde_json::json!({"input": "{\"a\":1}"}),
        "case \"the AWS SDK family\": expected the operation's own members, got {sent}"
    );
    assert!(
        call.json_text.is_empty(),
        "case \"the AWS SDK family\": nothing is converted for one, got {:?}",
        call.json_text
    );
}

/// A chain of in-process calls already at [`MAX_CALL_CHAIN_DEPTH`] makes NO
/// further call: the depth decides BEFORE the handover, so the blocking thread
/// this walk holds is the last one the chain pins, and the task reports that
/// nothing was run rather than a result nothing produced.
///
/// The rows are the depths at and past the ceiling; every call the end-to-end
/// suites make runs below it, which is the other side of the same rule.
#[test]
fn a_call_from_a_chain_at_the_depth_ceiling_is_not_made() {
    let call = read("arn:aws:states:::dynamodb:putItem").expect("this names a call");
    let (calls, mut handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    let rows: [(&str, u8); 2] = [
        ("a chain at the ceiling", MAX_CALL_CHAIN_DEPTH),
        ("a chain past the ceiling", MAX_CALL_CHAIN_DEPTH + 1),
    ];

    for (case, depth) in rows {
        let dispatcher = Dispatcher {
            calls: calls.clone(),
            depth,
            role: None,
            handovers: None,
            map_runs: None,
        };
        let answer = dispatcher.perform(&call, &::serde_json::json!({}), None);
        let Err(failed) = answer else {
            panic!("case {case:?}: expected no call to be made, got {answer:?}")
        };
        assert_eq!(
            failed.error,
            super::super::asl::RUNTIME_ERROR,
            "case {case:?}: expected {}, got {failed:?}",
            super::super::asl::RUNTIME_ERROR
        );
        assert!(
            failed.cause.contains("nothing was run for it"),
            "case {case:?}: the cause must say nothing ran, got {failed:?}"
        );
        assert!(
            handovers.try_recv().is_err(),
            "case {case:?}: no call may be handed over at this depth"
        );
    }
}

/// A call that could not be HANDED OVER at all is reported as work that did not
/// run: [`super::super::asl::RUNTIME_ERROR`] and a cause saying nothing ran, the
/// same failure every other call this module declines to make carries, rather than
/// ASL's timeout name — nothing was waiting for a bound that could expire.
///
/// The single buffered slot is occupied here, which is what the walk sees while the
/// operation serving it is still inside a previous call that timed out server-side.
#[test]
fn a_call_that_cannot_be_handed_over_is_not_reported_as_a_timeout() {
    let call = read("arn:aws:states:::dynamodb:putItem").expect("this names a call");
    let (calls, _handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    // The one slot is taken, and nothing drains it: the next handover cannot be made.
    let (answer, _answered) = ::std::sync::mpsc::channel();
    calls
        .try_send(Handover {
            call: call.clone(),
            request: ::serde_json::json!({}),
            answer,
        })
        .expect("the one buffered slot is free");
    let dispatcher = Dispatcher {
        calls,
        depth: 0,
        role: None,
        handovers: None,
        map_runs: None,
    };

    let answer = dispatcher.perform(&call, &::serde_json::json!({}), None);

    let Err(failed) = answer else {
        panic!("case \"a call with nowhere to go\": expected no call to be made, got {answer:?}")
    };
    assert_eq!(
        failed.error,
        super::super::asl::RUNTIME_ERROR,
        "case \"a call with nowhere to go\": expected {}, got {failed:?}",
        super::super::asl::RUNTIME_ERROR
    );
    assert!(
        failed.cause.contains("nothing was run for it"),
        "case \"a call with nowhere to go\": the cause must say nothing ran, got {failed:?}"
    );
}

/// A call that does not answer inside the span the task gave it times the task
/// out and RELEASES the thread it was waiting on, so a state's
/// `TimeoutSeconds` bounds the span it documents rather than only being
/// compared with it afterwards.
///
/// The handover is never served here, which is what a stalled call looks like
/// from the waiting side.
#[test]
fn a_call_that_does_not_answer_inside_its_bound_times_the_task_out() {
    let call = read("arn:aws:states:::dynamodb:putItem").expect("this names a call");
    // The serving half is held, never drained: the send succeeds and no answer
    // ever comes back.
    let (calls, _handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    let dispatcher = Dispatcher {
        calls,
        depth: 0,
        role: None,
        handovers: None,
        map_runs: None,
    };
    let within = Duration::from_millis(50);

    let began = ::std::time::Instant::now();
    let answer = dispatcher.perform(&call, &::serde_json::json!({}), Some(within));
    let waited = began.elapsed();

    let Err(failed) = answer else {
        panic!("case \"a call that never answers\": expected a failure, got {answer:?}")
    };
    assert_eq!(
        failed.error,
        super::super::asl::TIMEOUT_ERROR,
        "case \"a call that never answers\": expected {}, got {failed:?}",
        super::super::asl::TIMEOUT_ERROR
    );
    assert!(
        waited >= within,
        "case \"a call that never answers\": the task must wait the span it was given, \
             waited {waited:?}"
    );
    assert!(
        waited < MAX_CALL_DURATION,
        "case \"a call that never answers\": the state's own bound must decide, not the \
             {MAX_CALL_DURATION:?} ceiling, waited {waited:?}"
    );
}

/// The result of a request-response OPTIMIZED integration is the literal SDK
/// response of the call: the operation's own members, plus the envelope the SDK
/// puts that response's HTTP facts in, with every value read off the answer.
///
/// The rows span what the rule generalises over: a response with members and one
/// with none, a repeated header, an answer carrying no request id at all, and a
/// body that is not an object.
#[test]
fn an_optimized_integrations_result_carries_the_sdk_response_envelope() {
    // (case, the document the operation answered, the headers it carried)
    let rows: [EnvelopeRow; 4] = [
        (
            "a response with members of its own",
            ::serde_json::json!({"ExecutionArn": "arn:execution", "StartDate": 1}),
            vec![("x-amzn-requestid", "req-1"), ("content-type", "text/json")],
        ),
        (
            "a response carrying no member at all",
            ::serde_json::json!({}),
            vec![("x-amzn-RequestId", "req-2")],
        ),
        (
            "a header the answer repeated",
            ::serde_json::json!({"Attributes": {}}),
            vec![
                ("x-amzn-requestid", "req-3"),
                ("set-cookie", "a=1"),
                ("set-cookie", "b=2"),
            ],
        ),
        (
            "an answer naming no request id",
            ::serde_json::json!({"Item": {}}),
            vec![("content-type", "text/json")],
        ),
    ];

    for (case, document, headers) in rows {
        let answered = Answered {
            status: 200,
            error_type: None,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            payload: Vec::new(),
        };
        let members = document.as_object().expect("each row answers a document").clone();
        let result = with_sdk_envelope(document, &answered, None);
        // Every member the operation answered is still there, untouched.
        for (member, value) in &members {
            assert_eq!(
                result.get(member),
                Some(value),
                "case {case:?}: expected the operation's own {member:?}, got {result}"
            );
        }
        assert_eq!(
            result.pointer("/SdkHttpMetadata/HttpStatusCode"),
            Some(&::serde_json::json!(200)),
            "case {case:?}: expected the status the call answered, got {result}"
        );
        for (name, value) in &headers {
            assert!(
                result
                    .pointer(&format!("/SdkHttpMetadata/AllHttpHeaders/{name}"))
                    .and_then(Value::as_array)
                    .is_some_and(|carried| carried.contains(&::serde_json::json!(value))),
                "case {case:?}: expected the header {name:?} among the answer's, got {result}"
            );
        }
        let request_id = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(REQUEST_ID_HEADER))
            .map(|(_, value)| ::serde_json::json!(value));
        assert_eq!(
            result.pointer("/SdkResponseMetadata/RequestId"),
            request_id.as_ref(),
            "case {case:?}: expected the request id the answer named, got {result}"
        );
    }

    // A body that is not an object carries no members to put the envelope beside.
    let answered = Answered {
        status: 200,
        error_type: None,
        headers: Vec::new(),
        payload: Vec::new(),
    };
    let listed = with_sdk_envelope(::serde_json::json!([1, 2]), &answered, None);
    assert_eq!(
        listed,
        ::serde_json::json!([1, 2]),
        "case \"a response that is not an object\": expected it as it stands, got {listed}"
    );
}

/// Every header the envelope reports is named the way the invoked operation BINDS it,
/// whatever case the parsed answer carried it in — and a header that operation binds no
/// member to is reported as it came back.
///
/// The rows span what the rule generalises over: a bound header carried in the lower
/// case an HTTP library normalises field names to, the same header carried in the
/// model's own spelling, another member of the same operation's output, and a header the
/// operation binds nothing to.
#[test]
fn the_envelope_names_a_header_the_way_the_operation_binds_it() {
    let rest = LAMBDA_REST
        .iter()
        .find(|rest| rest.operation == "Invoke")
        .expect("the invocation is a call this emulator makes");
    // (case, the field name the answer carried, the field name the envelope reports)
    let rows = [
        (
            "a bound header normalised to lower case",
            "x-amz-executed-version",
            "X-Amz-Executed-Version",
        ),
        (
            "the same header already in the model's spelling",
            "X-Amz-Executed-Version",
            "X-Amz-Executed-Version",
        ),
        (
            "another member of the same output",
            "x-amz-function-error",
            "X-Amz-Function-Error",
        ),
        (
            "a header the operation binds nothing to",
            "content-type",
            "content-type",
        ),
    ];
    for (case, carried, expected) in rows {
        let answered = Answered {
            status: 200,
            error_type: None,
            headers: vec![(carried.to_string(), "1".to_string())],
            payload: Vec::new(),
        };
        let result = with_sdk_envelope(::serde_json::json!({}), &answered, Some(rest));
        assert_eq!(
            result
                .pointer(&format!("/SdkHttpMetadata/HttpHeaders/{expected}"))
                .and_then(Value::as_str),
            Some("1"),
            "case {case:?}: expected the header under {expected:?}, got {result}"
        );
        assert_eq!(
            result
                .pointer(&format!("/SdkHttpMetadata/AllHttpHeaders/{expected}"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
            "case {case:?}: expected every value under {expected:?}, got {result}"
        );
    }
}

/// The access request a call is subject to: the IAM action the invoked operation
/// publishes, and the ARN the request names as the resource acted on.
/// The rows span what the rule generalises over: both spellings a definition may
/// write the member in, the pattern suffixes (which name the same action), a
/// member that names its resource by NAME rather than by ARN (and the same member
/// carrying an ARN instead), several actions of one service, an operation whose
/// published permission is not its own name, each form in which an integration
/// addresses its target by something other than an ARN ([`TargetNaming`]), an
/// integration the table carries no entry for, the AWS SDK family, and a request
/// naming no resource at all.
#[test]
fn an_access_request_is_the_action_and_the_resource_the_request_names() {
    let region = "us-west-2";
    let child = "arn:aws:states:us-west-2:000000000000:stateMachine:child";
    let orders = ::engine::emulator_arn("dynamodb", region, "table/orders");
    let elsewhere = "arn:aws:dynamodb:us-east-2:000000000000:table/elsewhere";
    let queue_arn = ::engine::emulator_arn(SQS_NAMESPACE, region, "orders");
    let function_arn = ::engine::emulator_arn(LAMBDA_NAMESPACE, region, "function:handler");
    // (case, the Resource, the request the state built, the expected pair)
    let rows: [AccessRow; 20] = [
        (
            "a nested execution, the member in the integration's own spelling",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"StateMachineArn": child}),
            Some(("states:StartExecution", child.to_string())),
        ),
        (
            "the same member as the invoked model spells it",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"stateMachineArn": child}),
            Some(("states:StartExecution", child.to_string())),
        ),
        (
            "the run-a-job pattern, whose action is the same one",
            "arn:aws:states:::states:startExecution.sync:2",
            ::serde_json::json!({"StateMachineArn": child, "Input": {}}),
            Some(("states:StartExecution", child.to_string())),
        ),
        (
            "a member naming its table by name, which is the ARN of that table here",
            "arn:aws:states:::dynamodb:putItem",
            ::serde_json::json!({"TableName": "orders"}),
            Some(("dynamodb:PutItem", orders.clone())),
        ),
        (
            "another action of the same service, over the same table",
            "arn:aws:states:::dynamodb:deleteItem",
            ::serde_json::json!({"TableName": "orders"}),
            Some(("dynamodb:DeleteItem", orders.clone())),
        ),
        (
            "the AWS SDK family of that call, whose member its model spells lower",
            "arn:aws:states:::aws-sdk:dynamodb:updateItem",
            ::serde_json::json!({"tableName": "orders"}),
            Some(("dynamodb:UpdateItem", orders)),
        ),
        (
            "the same member carrying an ARN, which names the table it names",
            "arn:aws:states:::dynamodb:getItem",
            ::serde_json::json!({"TableName": elsewhere}),
            Some(("dynamodb:GetItem", elsewhere.to_string())),
        ),
        (
            "an integration no entry is carried for",
            "arn:aws:states:::events:putEvents",
            ::serde_json::json!({"Entries": []}),
            None,
        ),
        (
            "the AWS SDK family of a nested execution, which acts on the same resource",
            "arn:aws:states:::aws-sdk:sfn:startExecution",
            ::serde_json::json!({"stateMachineArn": child}),
            Some(("states:StartExecution", child.to_string())),
        ),
        (
            "a request naming no resource at all",
            "arn:aws:states:::states:startExecution",
            ::serde_json::json!({"Input": {}}),
            None,
        ),
        // A queue is addressed by its url, and the access request is about the
        // queue's ARN.
        (
            "a send to a queue, addressed by its url",
            "arn:aws:states:::sqs:sendMessage",
            ::serde_json::json!({
                "QueueUrl": "https://sqs.us-west-2.amazonaws.com/000000000000/orders",
                "MessageBody": "{}",
            }),
            Some(("sqs:SendMessage", queue_arn.clone())),
        ),
        (
            "the same queue named by its ARN, which is the resource itself",
            "arn:aws:states:::sqs:sendMessage",
            ::serde_json::json!({"QueueUrl": queue_arn.clone()}),
            Some(("sqs:SendMessage", queue_arn.clone())),
        ),
        (
            "the AWS SDK family of that send, which acts on the same queue",
            "arn:aws:states:::aws-sdk:sqs:sendMessage",
            ::serde_json::json!({
                "queueUrl": "https://sqs.us-west-2.amazonaws.com/000000000000/orders",
            }),
            Some(("sqs:SendMessage", queue_arn)),
        ),
        (
            // The queue's own Region is the one its endpoint host names, which is
            // not always the Region the call is made in.
            "a send to a queue in another Region",
            "arn:aws:states:::sqs:sendMessage",
            ::serde_json::json!({
                "QueueUrl": "https://sqs.eu-west-1.amazonaws.com/000000000000/orders",
            }),
            Some((
                "sqs:SendMessage",
                ::engine::emulator_arn(SQS_NAMESPACE, "eu-west-1", "orders"),
            )),
        ),
        // A function is invoked under `lambda:InvokeFunction`, which is not the
        // operation's own name, on the function's ARN whichever form names it.
        (
            "an invocation, the function named by its bare name",
            "arn:aws:states:::lambda:invoke",
            ::serde_json::json!({"FunctionName": "handler", "Payload": {}}),
            Some(("lambda:InvokeFunction", function_arn.clone())),
        ),
        (
            "the same function named by its partial ARN",
            "arn:aws:states:::lambda:invoke",
            ::serde_json::json!({"FunctionName": "000000000000:function:handler"}),
            Some(("lambda:InvokeFunction", function_arn.clone())),
        ),
        (
            "the same function named by its ARN",
            "arn:aws:states:::lambda:invoke",
            ::serde_json::json!({"FunctionName": function_arn.clone()}),
            Some(("lambda:InvokeFunction", function_arn.clone())),
        ),
        // A `Resource` that names the FUNCTION makes the same call on the same
        // function, so it is subject to the same access request — and the document
        // the state built is that invocation's EVENT, which names no resource of
        // its own whatever it happens to carry.
        (
            "a function named by the Resource itself",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
            ::serde_json::json!({"order": 1}),
            Some(("lambda:InvokeFunction", function_arn.clone())),
        ),
        (
            "the same, where the event names a function of its own",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
            ::serde_json::json!({"FunctionName": "elsewhere"}),
            Some(("lambda:InvokeFunction", function_arn.clone())),
        ),
        (
            "a function named by the Resource at one of its aliases",
            "arn:aws:lambda:us-west-2:000000000000:function:handler:live",
            ::serde_json::json!({}),
            Some((
                "lambda:InvokeFunction",
                format!("{function_arn}:live"),
            )),
        ),
    ];

    for (case, resource, request, expected) in rows {
        let call = read(resource).expect("every row names a call");
        let asked = access_request(&call, &request, region);
        let expected = expected.map(|(action, named)| (action.to_string(), named));
        assert_eq!(
            asked, expected,
            "case {case:?}: expected {expected:?}, got {asked:?}"
        );
    }
}

/// An instant the invoked operation answered is reported in whole epoch
/// MILLISECONDS, which is the unit the SDK response a `Task` state reads renders a
/// date in — the `awsJson` wire form is epoch seconds with a fractional part.
///
/// The rows span what the rule generalises over: a fractional figure, a whole one,
/// the epoch itself, both spellings the member may come back under, a member the
/// response does not carry, one carrying something that is not a number, and a
/// member no entry names.
#[test]
fn an_instant_in_a_result_is_reported_in_epoch_millis() {
    // (case, the members to convert, the answer, the result)
    let rows: [InstantRow; 7] = [
        (
            "a timestamp carrying a fractional second",
            &["startDate"],
            ::serde_json::json!({"executionArn": "arn:e", "startDate": 1_793_241_840.725}),
            ::serde_json::json!({"executionArn": "arn:e", "startDate": 1_793_241_840_725i64}),
        ),
        (
            "a whole number of seconds",
            &["startDate"],
            ::serde_json::json!({"startDate": 1_793_241_840i64}),
            ::serde_json::json!({"startDate": 1_793_241_840_000i64}),
        ),
        (
            "the epoch itself",
            &["startDate"],
            ::serde_json::json!({"startDate": 0}),
            ::serde_json::json!({"startDate": 0}),
        ),
        (
            "the member under the integration's own spelling",
            &["startDate"],
            ::serde_json::json!({"StartDate": 1.5}),
            ::serde_json::json!({"StartDate": 1_500i64}),
        ),
        (
            "a member the response does not carry",
            &["startDate"],
            ::serde_json::json!({"executionArn": "arn:e"}),
            ::serde_json::json!({"executionArn": "arn:e"}),
        ),
        (
            "a member carrying something that is not a number",
            &["startDate"],
            ::serde_json::json!({"startDate": "2026-10-28T20:13:41Z"}),
            ::serde_json::json!({"startDate": "2026-10-28T20:13:41Z"}),
        ),
        (
            "an integration whose entry names no member",
            &[],
            ::serde_json::json!({"startDate": 1.5}),
            ::serde_json::json!({"startDate": 1.5}),
        ),
    ];

    for (case, members, answer, expected) in rows {
        let reported = in_epoch_millis(answer, members);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected}, got {reported}"
        );
    }

    // The catalogue names the member for the integration whose result carries an
    // instant, so the rule reaches a task through `read` and not only through the
    // conversion above.
    let call = read("arn:aws:states:::states:startExecution").expect("this names a call");
    assert!(
        call.epoch_millis.contains(&"startDate"),
        "case \"a nested execution\": expected the start date among the instants its result \
             reports, got {:?}",
        call.epoch_millis
    );
}

/// A REST integration's request carries each member where that operation's own
/// binding puts it: the path label, the query, the headers and the one payload.
///
/// The rows span what the rule generalises over: a label named by each form the
/// member admits and one needing encoding to stay a single segment, a payload given
/// as a document, as the text of one, as a string that is no document and absent
/// altogether, a header member present and absent, the query parameter, and parameters
/// naming no label at all.
#[test]
fn a_rest_integrations_request_binds_each_member_where_the_operation_does() {
    let call = read("arn:aws:states:::lambda:invoke").expect("this names a call");
    let path = "/2015-03-31/functions";
    let rows: [RestRow; 10] = [
        (
            "a function named by its bare name, the payload a document",
            ::serde_json::json!({"FunctionName": "handler", "Payload": {"a": 1}}),
            Some((
                "/2015-03-31/functions/handler/invocations",
                Vec::new(),
                r#"{"a":1}"#,
            )),
        ),
        (
            "the same function named by its ARN, whose colons a segment admits",
            ::serde_json::json!({
                "FunctionName": "arn:aws:lambda:us-west-2:000000000000:function:handler",
            }),
            Some((
                "/2015-03-31/functions/arn:aws:lambda:us-west-2:000000000000:function:handler\
                     /invocations",
                Vec::new(),
                "",
            )),
        ),
        (
            "a name carrying a character a request-target does not carry as one \
                 component",
            ::serde_json::json!({"FunctionName": "two words"}),
            None,
        ),
        (
            // What a state wrote for the payload member is a JSON VALUE of the document
            // it built, so the bytes the member carries are that value's own text: a
            // string arrives as the JSON string literal it is in that document, which
            // is the event a function invoked with a string is handed.
            "the payload given as the text of a document",
            ::serde_json::json!({"FunctionName": "handler", "Payload": "{\"a\":1}"}),
            Some((
                "/2015-03-31/functions/handler/invocations",
                Vec::new(),
                r#""{\"a\":1}""#,
            )),
        ),
        (
            "the payload given as a string that is no document at all",
            ::serde_json::json!({"FunctionName": "handler", "Payload": "plain"}),
            Some((
                "/2015-03-31/functions/handler/invocations",
                Vec::new(),
                r#""plain""#,
            )),
        ),
        (
            "a header member the parameters name",
            ::serde_json::json!({"FunctionName": "handler", "InvocationType": "Event"}),
            Some((
                "/2015-03-31/functions/handler/invocations",
                vec![("X-Amz-Invocation-Type", "Event")],
                "",
            )),
        ),
        (
            "every header member the operation binds",
            ::serde_json::json!({
                "FunctionName": "handler",
                "InvocationType": "RequestResponse",
                "LogType": "Tail",
                "ClientContext": "e30=",
            }),
            Some((
                "/2015-03-31/functions/handler/invocations",
                vec![
                    ("X-Amz-Invocation-Type", "RequestResponse"),
                    ("X-Amz-Log-Type", "Tail"),
                    ("X-Amz-Client-Context", "e30="),
                ],
                "",
            )),
        ),
        (
            "the query parameter the operation binds",
            ::serde_json::json!({"FunctionName": "handler", "Qualifier": "$LATEST"}),
            Some((
                "/2015-03-31/functions/handler/invocations?Qualifier=$LATEST",
                Vec::new(),
                "",
            )),
        ),
        (
            // `Qualifier`'s own pattern admits the empty string, so the parameter
            // is carried with nothing after its `=` rather than refused.
            "the same parameter given as the empty string its member admits",
            ::serde_json::json!({"FunctionName": "handler", "Qualifier": ""}),
            Some((
                "/2015-03-31/functions/handler/invocations?Qualifier=",
                Vec::new(),
                "",
            )),
        ),
        (
            "parameters naming no function, which the path cannot be built without",
            ::serde_json::json!({"Payload": {}}),
            None,
        ),
    ];

    for (case, parameters, expected) in rows {
        match (wired(&call, &parameters), expected) {
            (Ok(built), Some((uri, headers, body))) => {
                assert_eq!(
                    built.uri, uri,
                    "case {case:?}: expected the request-target {uri:?}, got {:?}",
                    built.uri
                );
                let carried: Vec<(&str, &str)> = built
                    .headers
                    .iter()
                    .map(|(field, value)| (*field, value.as_str()))
                    .collect();
                assert_eq!(
                    carried, headers,
                    "case {case:?}: expected the headers {headers:?}, got {carried:?}"
                );
                let sent = String::from_utf8_lossy(&built.body).to_string();
                assert_eq!(
                    sent, body,
                    "case {case:?}: expected the body {body:?}, got {sent:?}"
                );
                assert!(
                    built.target.is_none(),
                    "case {case:?}: a REST request names its operation by method and path, \
                         so it carries no target header, got {:?}",
                    built.target
                );
                assert!(
                    built.uri.starts_with(path),
                    "case {case:?}: expected the operation's own path, got {:?}",
                    built.uri
                );
            }
            (Err(why), None) => assert!(
                why.contains("'FunctionName'"),
                "case {case:?}: expected the member the request cannot be built from to be \
                     named, got {why:?}"
            ),
            (Ok(built), None) => panic!(
                "case {case:?}: expected no request, got {} {}",
                built.method, built.uri
            ),
            (Err(why), Some(_)) => {
                panic!("case {case:?}: expected a request, got the refusal {why:?}")
            }
        }
    }
}

/// A 2xx answer to a REST call is read into the members that operation's output
/// binds: the status, the headers and the body.
#[test]
fn a_rest_answer_is_read_into_the_members_its_output_binds() {
    let rest = LAMBDA_REST
        .iter()
        .find(|rest| rest.operation == "Invoke")
        .expect("the invocation is a call this emulator makes");
    // The answer a function's invocation carries: the status, the version that ran
    // and the payload it returned.
    let answered = Answered {
        status: 200,
        error_type: None,
        headers: vec![
            ("x-amz-executed-version".to_string(), "$LATEST".to_string()),
            ("x-amzn-requestid".to_string(), "r-1".to_string()),
        ],
        payload: br#"{"ok":true}"#.to_vec(),
    };
    let result = rest.result(&answered);
    assert_eq!(
        result,
        ::serde_json::json!({
            "StatusCode": 200,
            "ExecutedVersion": "$LATEST",
            "Payload": {"ok": true},
        }),
        "case \"an invocation that returned a document\": got {result}"
    );

    // A body that is not a document is reported as its own text, and a header the
    // answer did not carry is reported by nothing.
    let answered = Answered {
        status: 202,
        error_type: None,
        headers: Vec::new(),
        payload: b"not json".to_vec(),
    };
    let result = rest.result(&answered);
    assert_eq!(
        result,
        ::serde_json::json!({ "StatusCode": 202, "Payload": "not json" }),
        "case \"an answer carrying no document and no headers\": got {result}"
    );
}

/// One row of the document-body table: the case, the `Resource`, the parameters the
/// state built, and the request-target and request body expected of it — `None` for a
/// request that carries no body.
type DocumentRow = (&'static str, &'static str, Value, &'static str, Option<Value>);

/// A REST operation that binds NO payload member carries the document of every member
/// its path, query and headers do not, which is what that protocol gives a structure.
///
/// The rows span what the rule generalises over: a write whose every member is the
/// body, a write whose name is a path label and whose remaining members are the body,
/// a read whose only member is its path label, a read whose members are query
/// parameters, and a read with two labels the second of which is an ARN.
#[test]
fn a_rest_operation_with_no_payload_member_carries_its_members_as_the_document() {
    let vault = "nightly";
    let point = "arn:aws:backup:us-west-2:000000000000:recovery-point:1";
    let rows: [DocumentRow; 5] = [
        (
            "a write whose every member is the body",
            "arn:aws:states:::aws-sdk:backup:startBackupJob",
            ::serde_json::json!({
                "BackupVaultName": vault,
                "ResourceArn": "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
                "IamRoleArn": "arn:aws:iam::000000000000:role/Backup",
            }),
            "/backup-jobs",
            Some(::serde_json::json!({
                "BackupVaultName": vault,
                "ResourceArn": "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
                "IamRoleArn": "arn:aws:iam::000000000000:role/Backup",
            })),
        ),
        (
            // The member the PATH carries is on the wire there, so it is not in the
            // body as well.
            "a write whose name is its path and whose tags are its body",
            "arn:aws:states:::aws-sdk:backup:createBackupVault",
            ::serde_json::json!({
                "BackupVaultName": vault,
                "BackupVaultTags": { "team": "platform" },
            }),
            "/backup-vaults/nightly",
            Some(::serde_json::json!({ "BackupVaultTags": { "team": "platform" } })),
        ),
        (
            "a read whose only member is its path label",
            "arn:aws:states:::aws-sdk:backup:describeBackupJob",
            ::serde_json::json!({ "BackupJobId": "job-1" }),
            "/backup-jobs/job-1",
            None,
        ),
        (
            "a read whose members are its query parameters",
            "arn:aws:states:::aws-sdk:backup:listBackupJobs",
            ::serde_json::json!({
                "MaxResults": 5,
                "ByResourceArn": "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
            }),
            "/backup-jobs?maxResults=5&resourceArn=arn:aws:dynamodb:us-west-2:000000000000\
             :table/orders",
            None,
        ),
        (
            "a read with two path labels, the second an ARN",
            "arn:aws:states:::aws-sdk:backup:describeRecoveryPoint",
            ::serde_json::json!({ "BackupVaultName": vault, "RecoveryPointArn": point }),
            "/backup-vaults/nightly/recovery-points/arn:aws:backup:us-west-2:000000000000\
             :recovery-point:1",
            None,
        ),
    ];

    for (case, resource, parameters, uri, body) in rows {
        let call = read(resource).unwrap_or_else(|why| {
            panic!("case {case:?}: expected {resource:?} to name a call, got {why:?}")
        });
        let built = wired(&call, &parameters)
            .unwrap_or_else(|why| panic!("case {case:?}: expected a request, got {why:?}"));
        assert_eq!(
            built.uri, uri,
            "case {case:?}: expected the request-target {uri:?}, got {:?}",
            built.uri
        );
        let sent: Option<Value> = match built.body.is_empty() {
            true => None,
            false => Some(::serde_json::from_slice(&built.body).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&built.body)
                )
            })),
        };
        assert_eq!(
            sent, body,
            "case {case:?}: expected the body {body:?}, got {sent:?}"
        );
    }
}

/// A 2xx answer to a REST call whose output binds no payload member is read as the
/// output DOCUMENT itself: the members the body holds are the result's own.
#[test]
fn a_rest_answer_with_no_payload_member_is_the_output_document() {
    let rest = BACKUP_REST
        .iter()
        .find(|rest| rest.operation == "StartBackupJob")
        .expect("starting a backup job is a call this emulator makes");
    let answered = Answered {
        status: 200,
        error_type: None,
        headers: vec![("x-amzn-requestid".to_string(), "r-1".to_string())],
        payload: br#"{"BackupJobId":"job-1","IsParent":false,"CreationDate":1.5}"#.to_vec(),
    };
    let result = rest.result(&answered);
    assert_eq!(
        result,
        ::serde_json::json!({
            "BackupJobId": "job-1",
            "IsParent": false,
            "CreationDate": 1.5,
        }),
        "case \"a started backup job\": the members the answer carried are the result's, got \
         {result}"
    );

    // An operation whose output carries no member answers an empty body, which is the
    // empty document rather than a member invented for it.
    let answered = Answered {
        status: 200,
        error_type: None,
        headers: Vec::new(),
        payload: Vec::new(),
    };
    let result = rest.result(&answered);
    assert_eq!(
        result,
        ::serde_json::json!({}),
        "case \"an answer carrying no body\": got {result}"
    );
}

/// The two ends of a task whose `Resource` names a Lambda FUNCTION: the state's
/// document is the EVENT the function is invoked with, and what the task reports is
/// the function's own returned document.
///
/// The rows span what the rule generalises over: the shape of the event (an object,
/// an array, a bare value, the empty object), and the shape of what the function
/// returned (a document, a bare value, nothing at all).
#[test]
fn a_function_resource_carries_the_event_and_reports_what_the_function_returned() {
    let function = "arn:aws:lambda:us-west-2:000000000000:function:handler";
    let call = read(function).expect("a function's ARN names an invocation");
    // (case, the document the state built, the body the invocation is to carry)
    let events: [(&str, Value, &str); 4] = [
        (
            "an event that is an object",
            ::serde_json::json!({"mode": "ok", "n": 0}),
            r#"{"mode":"ok","n":0}"#,
        ),
        (
            "an event that is a list",
            ::serde_json::json!([1, 2]),
            "[1,2]",
        ),
        (
            "an event that is a bare value",
            ::serde_json::json!(7),
            "7",
        ),
        ("the empty event", ::serde_json::json!({}), "{}"),
    ];

    for (case, event, body) in events {
        let input = call.operation_input(&event);
        assert_eq!(
            input.get(FUNCTION_NAME_MEMBER),
            Some(&Value::String(function.to_string())),
            "case {case:?}: expected the invocation to name the function the Resource did, \
                 got {input}"
        );
        assert_eq!(
            input.get(PAYLOAD_MEMBER),
            Some(&event),
            "case {case:?}: expected the state's document as the event, got {input}"
        );
        let built = wired(&call, &event).expect("the invocation is a request");
        let sent = String::from_utf8_lossy(&built.body).to_string();
        assert_eq!(
            sent, body,
            "case {case:?}: expected the event as the request body, got {sent:?}"
        );
        assert_eq!(
            built.uri,
            format!("/2015-03-31/functions/{function}/invocations"),
            "case {case:?}: expected the invocation of that function, got {:?}",
            built.uri
        );
    }

    // (case, the payload the function returned, the result the task reports)
    let returned: [(&str, &str, Value); 3] = [
        (
            "a function that returned a document",
            r#"{"ok":true,"count":2}"#,
            ::serde_json::json!({"ok": true, "count": 2}),
        ),
        (
            "a function that returned a bare value",
            "42",
            ::serde_json::json!(42),
        ),
        (
            "a function that returned nothing",
            "",
            ::serde_json::Value::Null,
        ),
    ];

    for (case, payload, expected) in returned {
        let answered = Answered {
            status: 200,
            error_type: None,
            headers: vec![("x-amz-executed-version".to_string(), "$LATEST".to_string())],
            payload: payload.as_bytes().to_vec(),
        };
        let document = call
            .rest
            .expect("the invocation carries its own bindings")
            .result(&answered);
        let payload = invocation_payload(&call, &document);
        let reported = reported_result(&call, &payload);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the function's own document {expected}, got {reported}"
        );
    }

    // The optimized integration of the same operation reports the SDK response
    // instead, so the two forms differ in exactly that.
    let optimized = read("arn:aws:states:::lambda:invoke").expect("this names a call");
    let answer = ::serde_json::json!({"StatusCode": 200, "Payload": {"ok": true}});
    let reported = reported_result(&optimized, &answer);
    assert_eq!(
        reported, answer,
        "case \"the optimized integration\": expected the invocation's own members, got \
             {reported}"
    );
}

/// An invocation Lambda SERVED whose function ERRORED is a failed task, named by the
/// class the run's error document carries and carrying that document as its cause;
/// any other answer of that call, and every call of another operation, is no failure
/// of its own.
///
/// The rows span what the rule generalises over: the signal present with a document
/// naming a class, with one naming none, with none at all and with a payload that is
/// text rather than a document; the signal absent, empty, and on an answer that also
/// carries a result; and the same document answered by another operation.
#[test]
fn a_served_invocation_whose_function_errored_is_a_failed_task() {
    /// One row: the answer the call came back with, the error the failure it is to
    /// the state is named by (`None` for an answer that is no failure), and the cause
    /// it carries where the DOCUMENT the run produced decides it.
    struct Row {
        case: &'static str,
        answer: Value,
        error: Option<&'static str>,
        cause: Option<Value>,
    }

    let rows = [
        Row {
            case: "a run that raised, its class named by the document",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "Unhandled",
                "Payload": {"errorType": "CustomBoomError", "errorMessage": "no"},
            }),
            error: Some("CustomBoomError"),
            cause: Some(::serde_json::json!({
                "errorType": "CustomBoomError",
                "errorMessage": "no",
            })),
        },
        Row {
            case: "a run the runtime itself caught, which names its class the same way",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "Handled",
                "Payload": {"errorType": "ValueError", "errorMessage": "bad"},
            }),
            error: Some("ValueError"),
            cause: Some(::serde_json::json!({
                "errorType": "ValueError",
                "errorMessage": "bad",
            })),
        },
        Row {
            case: "a document naming no class at all",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "Unhandled",
                "Payload": {"errorMessage": "nothing names it"},
            }),
            error: Some(LAMBDA_UNKNOWN_ERROR),
            cause: Some(::serde_json::json!({"errorMessage": "nothing names it"})),
        },
        Row {
            case: "a document whose class is empty text",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "Unhandled",
                "Payload": {"errorType": ""},
            }),
            error: Some(LAMBDA_UNKNOWN_ERROR),
            cause: Some(::serde_json::json!({"errorType": ""})),
        },
        Row {
            case: "a payload that is text rather than a document, reported as it came back",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "Unhandled",
                "Payload": "not a document",
            }),
            error: Some(LAMBDA_UNKNOWN_ERROR),
            cause: Some(::serde_json::json!("not a document")),
        },
        Row {
            case: "the signal with no payload beside it",
            answer: ::serde_json::json!({"StatusCode": 200, "FunctionError": "Unhandled"}),
            error: Some(LAMBDA_UNKNOWN_ERROR),
            // The failure is still the state's, under the same name. What its cause
            // says is left to the code: an answer carrying no document at all is one
            // no run of this emulator's own runtimes produces, so there is no AWS
            // wording for it to pin here.
            cause: None,
        },
        Row {
            case: "a run that returned, which carries no signal",
            answer: ::serde_json::json!({"StatusCode": 200, "Payload": {"ok": true}}),
            error: None,
            cause: None,
        },
        Row {
            case: "an empty signal, which is no signal",
            answer: ::serde_json::json!({
                "StatusCode": 200,
                "FunctionError": "",
                "Payload": {"ok": true},
            }),
            error: None,
            cause: None,
        },
    ];

    let optimized = read("arn:aws:states:::lambda:invoke").expect("this names a call");
    let named_function =
        read("arn:aws:lambda:us-west-2:000000000000:function:handler").expect("so does this");
    for Row {
        case,
        answer,
        error,
        cause,
    } in rows
    {
        // Both forms of a `Resource` that invokes a function read the same signal:
        // the work either names is the function's RUN.
        for call in [&optimized, &named_function] {
            let read = function_error(call, &answer);
            assert_eq!(
                read.as_ref().map(|failed| failed.error.as_str()),
                error,
                "case {case:?}: expected {error:?} of {}, got {read:?}",
                call.operation
            );
            if let Some(expected) = &cause {
                let carried = read
                    .as_ref()
                    .map(|failed| failed.cause.clone())
                    .unwrap_or_default();
                // A payload that is a DOCUMENT is carried as that document, so it is
                // compared as one: which order a serialized object spells its members
                // in is not what the rule is about. A payload that is text is carried
                // as it came back.
                let reported = match expected {
                    Value::String(_) => Value::String(carried.clone()),
                    _ => ::serde_json::from_str(&carried).unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: expected the document itself as the cause, got \
                                 {error} for {carried:?}"
                        )
                    }),
                };
                assert_eq!(
                    &reported,
                    expected,
                    "case {case:?}: expected the payload itself as the cause of {}, got \
                         {reported}",
                    call.operation
                );
            }
        }
    }

    // A call of another operation reports no function error whatever its answer
    // happens to carry, because the signal is `Invoke`'s own output member.
    let elsewhere = read("arn:aws:states:::sqs:sendMessage").expect("this names a call");
    let answer = ::serde_json::json!({
        "FunctionError": "Unhandled",
        "Payload": {"errorType": "CustomBoomError"},
    });
    let read = function_error(&elsewhere, &answer);
    assert!(
        read.is_none(),
        "case \"another operation's answer\": expected no failure, got {read:?}"
    );
}

/// The `Payload` a Lambda task's PARAMETERS name carries the JSON document the function
/// is invoked with, so parameters naming a bare JSON scalar for it name no invocation
/// the call can be built from — and nothing else is refused here.
///
/// The rows span what the rule generalises over: each JSON type the parameter may be
/// given (object, array, number, string, boolean), the null document (which names no
/// value, exactly as an absent member does), parameters naming no `Payload` at all, and
/// the two forms that carry no parameter for it — a `Resource` that names the FUNCTION,
/// whose event is the state's own data, and a call of another operation.
#[test]
fn a_payload_parameter_that_is_not_a_document_names_no_invocation() {
    let optimized = read("arn:aws:states:::lambda:invoke").expect("this names a call");
    // (case, the Payload the parameters name, whether they build a call)
    let rows: &[(&str, Option<Value>, bool)] = &[
        ("an object", Some(::serde_json::json!({"n": 1})), true),
        ("an array", Some(::serde_json::json!([1, 2])), true),
        ("the empty object", Some(::serde_json::json!({})), true),
        ("the null document", Some(Value::Null), true),
        ("no Payload at all", None, true),
        ("a number", Some(::serde_json::json!(42)), false),
        ("a string", Some(::serde_json::json!("text")), false),
        ("a boolean", Some(::serde_json::json!(true)), false),
    ];
    for (case, payload, usable) in rows {
        let mut parameters = ::serde_json::Map::new();
        parameters.insert(
            FUNCTION_NAME_MEMBER.to_string(),
            Value::String("handler".to_string()),
        );
        if let Some(payload) = payload {
            parameters.insert(PAYLOAD_MEMBER.to_string(), payload.clone());
        }
        let request = Value::Object(parameters);
        let refused = unusable_parameters(&optimized, &request);
        match usable {
            true => assert!(
                refused.is_none(),
                "case {case:?}: these parameters name an invocation, got {refused:?}"
            ),
            false => {
                let Some(failed) = refused else {
                    panic!("case {case:?}: expected a refusal, got none")
                };
                assert_eq!(
                    failed.error,
                    super::super::asl::RUNTIME_ERROR,
                    "case {case:?}: expected the runtime error, got {failed:?}"
                );
                assert!(
                    failed.cause.starts_with(UNUSABLE_PARAMETERS_OPENING)
                        && failed.cause.contains(PAYLOAD_MEMBER),
                    "case {case:?}: the diagnosis must name the parameters and the member that \
                     could not be used, got {:?}",
                    failed.cause
                );
            }
        }
    }

    // A `Resource` that named the FUNCTION carries the state's data as the EVENT rather
    // than as a parameter, so a bare value is the event it is and nothing is refused.
    let direct = read("arn:aws:lambda:us-west-2:000000000000:function:handler")
        .expect("a function's ARN names an invocation");
    let event = ::serde_json::json!(7);
    let refused = unusable_parameters(&direct, &direct.operation_input(&event));
    assert!(
        refused.is_none(),
        "case \"a Resource naming the function\": the event is the state's own data, got \
         {refused:?}"
    );

    // Another operation's parameters are judged by nothing here: `Payload` is `Invoke`'s
    // own input member.
    let elsewhere = read("arn:aws:states:::sqs:sendMessage").expect("this names a call");
    let refused = unusable_parameters(&elsewhere, &::serde_json::json!({"Payload": 42}));
    assert!(
        refused.is_none(),
        "case \"another operation's parameters\": expected no refusal, got {refused:?}"
    );
}

/// A FORM request carries one field per LEAF of the operation's input: a map member is
/// spread over the numbered `entry` fields that protocol names a map's pairs by, under
/// the names that map's own key and value members are written with — and a collection
/// whose field names this emulator does not carry is still not bound, so no call is
/// made for it.
///
/// The rows differ along the shape of the member: a scalar, a map of one pair, a map of
/// several (which must be numbered rather than overwrite one another), a map key and
/// value carrying the form's own delimiters, and the two collections no entry names.
#[test]
fn a_form_request_spreads_a_map_member_over_its_numbered_entries() {
    let maps = &[FormMap {
        member: "MessageAttributes",
        key: "Name",
        value: "Value",
        structured: true,
    }];
    // (case, the document the state built, the fields expected after the action and the
    //  version — `None` for a document no request is built from)
    let rows: &[(&str, Value, Option<&[&str]>)] = &[
        (
            "a scalar member",
            ::serde_json::json!({ "Message": "hello" }),
            Some(&["Message=hello"]),
        ),
        (
            "a map of one pair, whose value is a structure",
            ::serde_json::json!({
                "MessageAttributes": {
                    "kind": { "DataType": "String", "StringValue": "created" },
                },
            }),
            Some(&[
                "MessageAttributes.entry.1.Name=kind",
                "MessageAttributes.entry.1.Value.DataType=String",
                "MessageAttributes.entry.1.Value.StringValue=created",
            ]),
        ),
        (
            "a map of two pairs, each under its own number",
            ::serde_json::json!({
                "MessageAttributes": {
                    "a": { "DataType": "String", "StringValue": "one" },
                    "b": { "DataType": "Number", "StringValue": "2" },
                },
            }),
            Some(&[
                "MessageAttributes.entry.1.Name=a",
                "MessageAttributes.entry.1.Value.DataType=String",
                "MessageAttributes.entry.1.Value.StringValue=one",
                "MessageAttributes.entry.2.Name=b",
                "MessageAttributes.entry.2.Value.DataType=Number",
                "MessageAttributes.entry.2.Value.StringValue=2",
            ]),
        ),
        (
            "a key and a value carrying the form's own delimiters",
            ::serde_json::json!({
                "MessageAttributes": {
                    "a&b": { "DataType": "String", "StringValue": "c=d" },
                },
            }),
            Some(&[
                "MessageAttributes.entry.1.Name=a%26b",
                "MessageAttributes.entry.1.Value.DataType=String",
                "MessageAttributes.entry.1.Value.StringValue=c%3Dd",
            ]),
        ),
        (
            "a list member no entry names",
            ::serde_json::json!({ "PolicyArns": ["arn:aws:iam::1:policy/p"] }),
            None,
        ),
        (
            "a structure member no entry names",
            ::serde_json::json!({ "Tags": { "Key": "k" } }),
            None,
        ),
    ];
    for (case, document, expected) in rows {
        let built = form_body("Publish", "2010-03-31", document, maps);
        match (built, expected) {
            (Ok(body), Some(fields)) => {
                let mut wanted = vec!["Action=Publish".to_string(), "Version=2010-03-31".to_string()];
                wanted.extend(fields.iter().map(|field| (*field).to_string()));
                let carried: Vec<String> = body.split('&').map(str::to_string).collect();
                assert_eq!(
                    carried, wanted,
                    "case {case:?}: expected {wanted:?}, got {carried:?}"
                );
            }
            (Err(why), None) => assert!(
                why.contains("form-field protocol"),
                "case {case:?}: a member no field carries must say so, got {why:?}"
            ),
            (Ok(body), None) => {
                panic!("case {case:?}: expected no request, got {body:?}")
            }
            (Err(why), Some(fields)) => {
                panic!("case {case:?}: expected {fields:?}, got the refusal {why:?}")
            }
        }
    }
}
