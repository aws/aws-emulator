// SPDX-License-Identifier: CC0-1.0

//! The deploy-wait-use-delete workflow a SCRIPT drives, over the whole assembled emulator:
//! `CreateStack`, poll `DescribeStacks` for a terminal status, read the queue url out of the
//! stack's `Outputs`, send and receive a message through it, `DeleteStack`, poll again, and
//! find the queue gone.
//!
//! **The rule under test: every stack operation publishes a TERMINAL status to the read a
//! waiter polls — `DescribeStacks` — within the span the operation's own clock owes, and the
//! handles the completed stack publishes address resources the owning service really holds, so
//! a client that polls `DescribeStacks` after `CreateStack` and after `DeleteStack` always
//! reaches `CREATE_COMPLETE` and then `DELETE_COMPLETE` rather than reading
//! `*_IN_PROGRESS` for ever.** That is what an SDK or CLI waiter does: it has no other signal
//! than the status the read reports, so a status that never settles is a workflow that never
//! returns.
//!
//! Two crates have to agree for it to hold (CloudFormation drives the operation and makes the
//! calls, Amazon SQS owns the queue), and the router is the whole production mount
//! (`aws_emulator_router::build_router`), which is the seam a scripted client reaches — so the
//! suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the queue
//! or leaves the name to CloudFormation, how many resources the stack declares and whether the
//! output read is a `Ref` or an `Fn::GetAtt`. Every expected value is computed from what the
//! row declares or read from the OWNING service, never out of the answer under test.
//!
//! Nothing here sleeps for a span: every wait is a poll for the CONDITION with a generous
//! deadline, so a loaded host is slow rather than wrong.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_stack_resources::{outputs_of, xml_text};
use crate::query_wire::form_encode;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How long a case polls for a stack operation's terminal status.
///
/// GENEROUS on purpose: what is under test is WHICH status the operation reaches, never how
/// fast — so the deadline leaves room for a loaded host while still failing a status that
/// never settles.
const STATUS_DEADLINE: Duration = Duration::from_secs(60);

/// How long a case polls for the message it sent to be receivable.
const MESSAGE_DEADLINE: Duration = Duration::from_secs(30);

/// How long each poll below waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as the binary assembles it: a scripted client
/// reaches that mount, so which service claims each call — CloudFormation's own in-process
/// call into Amazon SQS included — is decided by the production assembly.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an SQS refusal: awsJson1_0 carries it as the body's `__type`.
fn sqs_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Poll `DescribeStacks` until the stack rests in `wanted`, exactly as an SDK or CLI waiter
/// does, and answer the body that reported it.
///
/// The CONDITION is what is waited for, never a span — and a status the operation could settle
/// in but did not is reported with the whole body, so a failure says which status the stack is
/// parked in. A read that stops resolving is reported with the stack's own record, addressable
/// by the id the create answered whether the stack is live or deleted.
async fn stack_resting_in(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    stack_id: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + STATUS_DEADLINE;
    loop {
        let (status_code, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        if status_code == 200 {
            let reported = xml_text(&body, "StackStatus").unwrap_or_default();
            assert!(
                !reported.is_empty(),
                "case {case:?}: expected DescribeStacks for {stack:?} to report a StackStatus, \
                 got {body}"
            );
            if reported == wanted {
                return body;
            }
        } else {
            // A NAME stops resolving the moment a deletion completes ("Deleted stacks: you
            // must specify the unique stack ID"), which is the very answer a delete waiter
            // stops on — so the stack's own record under its id is what says whether the
            // operation reached the status the rule says it reaches.
            let (by_id_status, by_id) =
                cloudformation(router, "DescribeStacks", &[("StackName", stack_id)]).await;
            assert_eq!(
                by_id_status, 200,
                "case {case:?}: expected the stack's own record under {stack_id:?} to be \
                 readable, got {by_id_status} with body {by_id}"
            );
            let reported = xml_text(&by_id, "StackStatus").unwrap_or_default();
            if reported == wanted {
                return by_id;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the stack to publish {wanted:?}, got {reported:?} \
                 under its id with body {by_id} while its name answered {status_code} {body}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack to publish {wanted:?} to the read a waiter \
             polls, got body {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the table: the template a scripted deploy submits, the output it reads the
/// queue's url out of, and the queue name the row expects to find in SQS — `None` where
/// CloudFormation generates it, which the row then reads out of the queue url the output
/// published.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The template body, built from the queue name the row declares (if any).
    template: fn(&str) -> String,
    /// The `OutputKey` the queue url is published under.
    output_key: &'static str,
    /// The `QueueName` the template declares, or `None` for a generated one.
    declared_name: Option<&'static str>,
}

/// The one-resource template the recorded script deploys: a named queue whose url is the
/// stack's output, which is what `Ref` on an `AWS::SQS::Queue` returns.
fn named_queue_template(queue_name: &str) -> String {
    format!(
        r#"{{"Resources":{{"Q":{{"Type":"AWS::SQS::Queue","Properties":{{"QueueName":"{queue_name}"}}}}}},
             "Outputs":{{"QueueUrl":{{"Value":{{"Ref":"Q"}}}}}}}}"#
    )
}

/// The same template with the queue's name left to CloudFormation.
fn generated_queue_template(_queue_name: &str) -> String {
    r#"{"Resources":{"Q":{"Type":"AWS::SQS::Queue"}},
         "Outputs":{"QueueUrl":{"Value":{"Ref":"Q"}}}}"#
        .to_string()
}

/// A two-resource template whose queue url is published through `Fn::GetAtt … QueueUrl` and
/// whose second resource is a topic of another service, so the walk the stack settles after
/// spans two owning services.
fn two_resource_template(queue_name: &str) -> String {
    format!(
        r#"{{"Resources":{{
               "Q":{{"Type":"AWS::SQS::Queue","Properties":{{"QueueName":"{queue_name}"}}}},
               "T":{{"Type":"AWS::SNS::Topic","Properties":{{"DisplayName":"scripted-deploy"}}}}}},
             "Outputs":{{"Url":{{"Value":{{"Fn::GetAtt":["Q","QueueUrl"]}}}}}}}}"#
    )
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one named queue, the url published by Ref",
            stack: "scripted-deploy-named",
            template: named_queue_template,
            output_key: "QueueUrl",
            declared_name: Some("scripted-deploy-named-queue"),
        },
        Row {
            case: "one queue whose name CloudFormation generates",
            stack: "scripted-deploy-generated",
            template: generated_queue_template,
            output_key: "QueueUrl",
            declared_name: None,
        },
        Row {
            case: "two resources of two services, the url published by Fn::GetAtt",
            stack: "scripted-deploy-pair",
            template: two_resource_template,
            output_key: "Url",
            declared_name: Some("scripted-deploy-pair-queue"),
        },
    ]
}

/// RULE: see the module header. Every row drives the whole scripted workflow and must get
/// through it.
#[tokio::test]
async fn a_scripted_deploy_waits_uses_and_deletes_without_a_status_that_never_settles() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let declared = row.declared_name.unwrap_or_default();
        let template = (row.template)(declared);

        let (status_code, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status_code} with body \
             {body}"
        );
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert!(
            stack_id.starts_with(&format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/")),
            "case {case:?}: expected CreateStack to answer this Region's stack id, got {body}"
        );
        // The Region's own listing: a stack this Region holds is one every read of it
        // resolves, so a create that answered a stack id must be IN that listing before the
        // waiter's first poll.
        let (status_code, listed) = cloudformation(&router, "DescribeStacks", &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected HTTP 200 from the Region's DescribeStacks, got \
             {status_code} with body {listed}"
        );
        assert!(
            listed.contains(row.stack),
            "case {case:?}: expected the created stack in the Region's listing, got {listed}"
        );

        // The create waiter: poll the one read it has until the stack publishes the terminal
        // status.
        let described =
            stack_resting_in(&router, case, row.stack, &stack_id, "CREATE_COMPLETE").await;

        // The handle the completed stack publishes, which the script drives the next call
        // with.
        let outputs = outputs_of(&described);
        let url = outputs.get(row.output_key).cloned().unwrap_or_default();
        assert!(
            !url.is_empty(),
            "case {case:?}: expected the stack to publish the queue url under {:?}, got \
             {outputs:?}",
            row.output_key
        );

        // ...and it addresses the queue AMAZON SQS holds: the url that service resolves for
        // the queue's own name is the url the stack published.
        let queue_name = match row.declared_name {
            Some(name) => name.to_string(),
            // A generated name is CloudFormation's to choose, so the row reads it out of the
            // url and asks SQS to resolve that name independently.
            None => url
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        assert_eq!(
            url,
            format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{queue_name}"),
            "case {case:?}: expected the published url to be the queue's own, got {url:?}"
        );
        let resolved = sqs_ok(
            &router,
            case,
            "GetQueueUrl",
            json!({"QueueName": queue_name}),
        )
        .await;
        assert_eq!(
            resolved.get("QueueUrl").and_then(Value::as_str),
            Some(url.as_str()),
            "case {case:?}: expected SQS to resolve the queue the stack published, got \
             {resolved}"
        );

        // The probe message the script round-trips through the deployed queue.
        let probe = format!("probe for {case}");
        sqs_ok(
            &router,
            case,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": probe}),
        )
        .await;
        let deadline = Instant::now() + MESSAGE_DEADLINE;
        let received = loop {
            let answer = sqs_ok(
                &router,
                case,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 1}),
            )
            .await;
            let bodies: Vec<String> = answer
                .get("Messages")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter_map(|message| {
                            message.get("Body").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(body) = bodies.into_iter().next() {
                break body;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the probe message to be receivable from the deployed \
                 queue, got {answer}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            received, probe,
            "case {case:?}: expected the queue to answer the body that was sent, got \
             {received:?}"
        );

        // The teardown half: the delete waiter has the same one read, and the queue is gone
        // in SQS once it returns.
        let (status_code, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status_code} with body \
             {body}"
        );
        // A deleted stack is addressable by its id, and by its NAME while the deletion is in
        // flight, which is what the waiter polls.
        let deleted =
            stack_resting_in(&router, case, row.stack, &stack_id, "DELETE_COMPLETE").await;
        assert_eq!(
            xml_text(&deleted, "StackName").as_deref(),
            Some(row.stack),
            "case {case:?}: expected the deleted stack to still report its name, got {deleted}"
        );
        // ...and the NAME no longer resolves: "deleted stacks don't show up in the
        // DescribeStacks operation if the deletion has been completed successfully", and "if
        // the stack doesn't exist, a ValidationError is returned" — the refusal a delete
        // waiter reads as the deletion having gone through.
        let (status_code, refused) =
            cloudformation(&router, "DescribeStacks", &[("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the deleted stack's NAME to resolve to nothing, got \
             {status_code} with body {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the ValidationError a delete waiter stops on, got \
             {refused}"
        );

        let (status_code, refusal) =
            sqs(&router, "GetQueueUrl", &json!({"QueueName": queue_name})).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected SQS to refuse the deleted stack's queue, got \
             {status_code} with {refusal}"
        );
        let code = sqs_error_code(&refusal);
        assert_eq!(
            code, "QueueDoesNotExist",
            "case {case:?}: expected the queue to be gone in SQS, got {code} with {refusal}"
        );
    }
}
