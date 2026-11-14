// SPDX-License-Identifier: CC0-1.0

//! The bucket a stack DECLARES with a notification configuration really invokes the function
//! that configuration names, end to end: CloudFormation, Amazon S3, AWS Lambda, Amazon
//! DynamoDB and IAM on ONE engine, the reference serverless estate deployed by one
//! `CreateStack`, an object written into the declared bucket, and the row the handler wrote
//! read back through DynamoDB's own API.
//!
//! RULE under test: **a template of first-party resource types a stack provisions reaches
//! `CREATE_COMPLETE` and publishes its `Outputs`, and each resource it declares exists in the
//! service that OWNS it — so the declared table takes a write, a function declared with INLINE
//! source is packaged under the file its own `Handler` names with the source extension its
//! declared runtime loads by default, and so really runs, and an object write into the declared
//! bucket publishes the event to the function its stored `NotificationConfiguration` names, for
//! every key its key-name `Filter` admits and for no other.**
//!
//! The rows differ along what the rule generalises over: the handler module an inline function
//! is declared under (`index`, `app`) and which Node release it names, the suffix rule of the
//! configuration that admits the written key (one per declared `LambdaConfigurations` entry), a
//! key no rule admits, and a key that differs from an admitted one only in case — the filter
//! being matched on the key's own text.
//!
//! Each row asserts the row's effect where a CLIENT sees it: the item DynamoDB holds under the
//! written key, so a delivery that never happened and a key the filter excluded are told apart.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, dynamodb, failure_reasons};
use crate::hotswapped_function_code::lambda;
use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, member, output_value, router, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::s3;

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The physical names the stack is given as `Parameters`, the way the reference estate supplies
/// them.
const BUCKET: &str = "declared-notification-images";
const TABLE: &str = "declared-notification-labels";
const FUNCTION: &str = "declared-notification-recorder";

/// The second declared function: the same handler on another Node release and under another
/// handler module name, so what an inline declaration is packaged as is read off the
/// declaration rather than fixed to one spelling.
const SECOND_FUNCTION: &str = "declared-notification-second-recorder";

/// The key attribute the declared table is keyed on, and the attribute the handler writes
/// beside it.
const KEY_ATTRIBUTE: &str = "image_name";
const EVENT_ATTRIBUTE: &str = "event_name";

/// The event class the declared configurations are notified about, and the event name a put
/// carries.
const OBJECT_CREATED: &str = "s3:ObjectCreated:*";
const PUT_EVENT_NAME: &str = "ObjectCreated:Put";

/// The suffixes the stack declares one `LambdaConfigurations` entry each for — the three image
/// extensions the reference estate filters on.
const SUFFIXES: [&str; 3] = [".jpg", ".jpeg", ".png"];

/// How long one invocation of the handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long between two readings of something a service's own clock owes, and how many readings
/// before the wait itself is the failure. Every wait below looks again for the CONDITION rather
/// than sleeping a span of this suite's choosing.
const LOOK_AGAIN: Duration = Duration::from_millis(500);
const LOOKS: usize = 120;

/// How many times a row that must see NOTHING looks before concluding nothing came: well past
/// the looks a delivered row takes to leave its item.
const QUIET_LOOKS: usize = 16;

/// The stack one thread of this suite carries, which is the stack the emulator's own request
/// workers carry (`core/router/src/main.rs`): a stack deployment is the deepest chain of
/// in-process calls this emulator makes — CloudFormation provisions the bucket, Amazon S3
/// validates the destination against AWS Lambda, and an object write runs the handler, whose own
/// SDK call reaches Amazon DynamoDB — and a test thread's default stack is far smaller than a
/// worker's.
const DEEP_CALL_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The handler the stack deploys: for each record it is handed it records the key the event
/// named, through the AWS SDK the managed Node runtime supplies.
///
/// ONE source for both declared functions — what the two rows differ in is the handler module
/// and the Node release they are declared with, not the code.
const RECORDING_HANDLER: &str = r#"
const { DynamoDBClient, PutItemCommand } = require('@aws-sdk/client-dynamodb');

exports.handler = async (event) => {
    const ddb = new DynamoDBClient({});
    const recorded = [];
    for (const record of event.Records) {
        const key = record.s3.object.key;
        await ddb.send(new PutItemCommand({
            TableName: process.env.TABLE_NAME,
            Item: {
                image_name: { S: key },
                event_name: { S: record.eventName },
            },
        }));
        recorded.push(key);
    }
    return { recorded: recorded };
};
"#;

/// The reference estate as ONE template: the table the handler writes, the execution role, the
/// function with its inline source, the grant Amazon S3 needs to invoke it, and the bucket
/// whose notification configuration names that function once per declared suffix.
///
/// Every physical name arrives as a `Parameter`, and the bucket `DependsOn` the grant, which is
/// the order the configuration's own destination validation needs.
fn estate_template() -> String {
    let configurations: Vec<Value> = SUFFIXES
        .iter()
        .map(|suffix| {
            json!({
                "Event": OBJECT_CREATED,
                "Function": { "Fn::GetAtt": ["Handler", "Arn"] },
                "Filter": { "S3Key": { "Rules": [{ "Name": "suffix", "Value": suffix }] } },
            })
        })
        .collect();
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "BucketName": { "Type": "String" },
            "TableName": { "Type": "String" },
            "FunctionName": { "Type": "String" },
            "HandlerSource": { "Type": "String" },
        },
        "Resources": {
            "LabelsTable": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": { "Ref": "TableName" },
                    "AttributeDefinitions": [
                        { "AttributeName": KEY_ATTRIBUTE, "AttributeType": "S" },
                    ],
                    "KeySchema": [{ "AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH" }],
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 5,
                        "WriteCapacityUnits": 5,
                    },
                },
            },
            "HandlerRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "record-the-object",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "dynamodb:PutItem",
                                    "dynamodb:GetItem",
                                    "s3:GetObject",
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Handler": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": { "Ref": "FunctionName" },
                    "Runtime": "nodejs20.x",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["HandlerRole", "Arn"] },
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": {
                        "Variables": {
                            "BUCKET_NAME": { "Ref": "BucketName" },
                            "TABLE_NAME": { "Ref": "TableName" },
                        },
                    },
                    "Code": { "ZipFile": { "Ref": "HandlerSource" } },
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Handler", "Arn"] },
                    "Action": "lambda:InvokeFunction",
                    "Principal": "s3.amazonaws.com",
                    "SourceAccount": { "Ref": "AWS::AccountId" },
                    "SourceArn": { "Fn::Sub": "arn:aws:s3:::${BucketName}" },
                },
            },
            // The same handler declared inline on another Node release and under another
            // handler module name: what the inline source is packaged as is read off the
            // declaration, so this one is `app.js` where the first is `index.js`.
            "SecondHandler": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": SECOND_FUNCTION,
                    "Runtime": "nodejs22.x",
                    "Handler": "app.handler",
                    "Role": { "Fn::GetAtt": ["HandlerRole", "Arn"] },
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": {
                        "Variables": { "TABLE_NAME": { "Ref": "TableName" } },
                    },
                    "Code": { "ZipFile": RECORDING_HANDLER },
                },
            },
            "ImageBucket": {
                "Type": "AWS::S3::Bucket",
                "DependsOn": "InvokePermission",
                "Properties": {
                    "BucketName": { "Ref": "BucketName" },
                    "PublicAccessBlockConfiguration": {
                        "BlockPublicAcls": true,
                        "BlockPublicPolicy": true,
                        "IgnorePublicAcls": true,
                        "RestrictPublicBuckets": true,
                    },
                    "NotificationConfiguration": {
                        "LambdaConfigurations": configurations,
                    },
                },
            },
        },
        "Outputs": {
            "BucketName": { "Value": { "Ref": "ImageBucket" } },
            "BucketArn": { "Value": { "Fn::GetAtt": ["ImageBucket", "Arn"] } },
            "TableName": { "Value": { "Ref": "LabelsTable" } },
            "FunctionName": { "Value": { "Ref": "Handler" } },
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Handler", "Arn"] } },
            "SecondFunctionName": { "Value": { "Ref": "SecondHandler" } },
        },
    })
    .to_string()
}

/// Deploy the estate and assert the `Outputs` the completed stack publishes.
async fn deployed(router: &MultiServiceRouter, case: &str, stack: &str) {
    let (status, body) = create_stack(
        router,
        stack,
        &estate_template(),
        &acknowledging_iam(&[
            member("Parameters.member.1.ParameterKey", "BucketName"),
            member("Parameters.member.1.ParameterValue", BUCKET),
            member("Parameters.member.2.ParameterKey", "TableName"),
            member("Parameters.member.2.ParameterValue", TABLE),
            member("Parameters.member.3.ParameterKey", "FunctionName"),
            member("Parameters.member.3.ParameterValue", FUNCTION),
            member("Parameters.member.4.ParameterKey", "HandlerSource"),
            member("Parameters.member.4.ParameterValue", RECORDING_HANDLER),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
    );

    // The stack settles on its own clock, so this waits for the CONDITION — a status that is
    // no longer in progress — rather than for a span.
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) = describe_stack(router, stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} {body}"
        );
        described = body;
        if xml_text(&described, "StackStatus").as_deref() == Some(CREATE_COMPLETE) {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    let reported = xml_text(&described, "StackStatus");
    let reasons = failure_reasons(router, stack).await;
    assert_eq!(
        reported.as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: expected the declared estate to provision, got {reported:?} because \
         {reasons:?}"
    );

    // Every output the estate publishes resolves: a stack whose resources were created
    // publishes the handles its consumers read.
    for key in [
        "BucketName",
        "BucketArn",
        "TableName",
        "FunctionName",
        "FunctionArn",
        "SecondFunctionName",
    ] {
        let value = output_value(&described, key).unwrap_or_default();
        assert!(
            !value.is_empty(),
            "case {case:?}: expected the stack to publish a non-empty {key}, got {value:?} in \
             {described}"
        );
    }
    assert_eq!(
        (
            output_value(&described, "BucketName"),
            output_value(&described, "TableName"),
            output_value(&described, "FunctionName"),
            output_value(&described, "BucketArn"),
            output_value(&described, "FunctionArn"),
        ),
        (
            Some(BUCKET.to_string()),
            Some(TABLE.to_string()),
            Some(FUNCTION.to_string()),
            Some(format!("arn:aws:s3:::{BUCKET}")),
            Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{FUNCTION}"
            )),
        ),
        "case {case:?}: expected the stack to publish the names it was given and the ARNs AWS \
         publishes for them, got {described}"
    );
}

/// Wait until AWS Lambda reports `function` `Active` — "you cannot invoke … while the function
/// is in the Pending state" — and answer the state it reported last.
async fn function_state(router: &MultiServiceRouter, case: &str, function: &str) -> String {
    let mut last = String::new();
    for _ in 0..LOOKS {
        let answered = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected AWS Lambda to hold the declared function, got {} with {}",
            answered.status, answered.body
        );
        last = answered
            .body
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if last == "Active" {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// Wait until Amazon DynamoDB reports the declared table `ACTIVE` — "you can perform read and
/// write operations only on an ACTIVE table" — and answer the status it reported last.
async fn table_status(router: &MultiServiceRouter) -> String {
    let mut last = String::new();
    for _ in 0..LOOKS {
        last = describe_table(router, TABLE)
            .await
            .and_then(|described| {
                described
                    .get("TableStatus")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if last == "ACTIVE" {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// The item the declared table holds under `key`, read through DynamoDB's own `GetItem`.
async fn held_item(router: &MultiServiceRouter, case: &str, key: &str) -> Option<Value> {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": { KEY_ATTRIBUTE: { "S": key } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetItem for {key:?}, got {status} with {answered}"
    );
    answered.get("Item").cloned()
}

/// Look for the item under `key` until it IS there, and answer the last read.
///
/// A row that must see nothing keeps looking for its whole quiet window and answers `None` only
/// if nothing ever came, so a delivery that merely arrives late is still seen.
async fn awaited_item(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    expected: bool,
) -> Option<Value> {
    let looks = if expected { LOOKS } else { QUIET_LOOKS };
    let mut last = None;
    for _ in 0..looks {
        last = held_item(router, case, key).await;
        if last.is_some() {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    last
}

/// One row: the key written into the declared bucket, and whether a declared configuration's
/// key-name filter admits it.
struct Row {
    case: &'static str,
    key: &'static str,
    delivered: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a key the first declared suffix rule admits",
        key: "uploads/portrait.jpg",
        delivered: true,
    },
    Row {
        case: "a key the second declared suffix rule admits",
        key: "uploads/portrait.jpeg",
        delivered: true,
    },
    Row {
        case: "a key the third declared suffix rule admits",
        key: "uploads/diagram.png",
        delivered: true,
    },
    Row {
        case: "a key no declared suffix rule admits",
        key: "uploads/notes.txt",
        delivered: false,
    },
    Row {
        case: "a key differing from an admitted one only in case",
        key: "uploads/portrait.JPG",
        delivered: false,
    },
];

/// The whole estate: it provisions, its resources exist in the services that own them, and a
/// write into the declared bucket runs the declared function for every key a declared filter
/// admits and for no other.
///
/// It runs on a thread of its own carrying [`DEEP_CALL_STACK_BYTES`], because the chain under
/// test is the deepest one this emulator makes in process.
#[test]
fn a_declared_bucket_notification_runs_the_function_it_names() {
    std::thread::Builder::new()
        .name("cfn-declared-notification-runs-its-function".to_string())
        .stack_size(DEEP_CALL_STACK_BYTES)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(the_declared_estate())
        })
        .expect("the suite's own thread")
        .join()
        .expect("the suite ran to completion");
}

async fn the_declared_estate() {
    let case = "the reference serverless estate, deployed by one CreateStack";
    let router = router();
    deployed(&router, case, "declared-notification-estate").await;

    for (function, state_case) in [
        (FUNCTION, "the function whose handler module is index"),
        (SECOND_FUNCTION, "the function whose handler module is app"),
    ] {
        let state = function_state(&router, state_case, function).await;
        assert_eq!(
            state, "Active",
            "case {case:?}: expected AWS Lambda to report {state_case} Active, got {state:?}"
        );
    }
    let status = table_status(&router).await;
    assert_eq!(
        status, "ACTIVE",
        "case {case:?}: expected DynamoDB to report the declared table ACTIVE, got {status:?}"
    );

    // The configuration the bucket CARRIES, read back through S3's own API: one entry per
    // declared `LambdaConfigurations` entry, each naming the declared function.
    let installed = s3(&router, "GET", &format!("/{BUCKET}?notification"), b"").await;
    assert_eq!(
        installed.status, 200,
        "case {case:?}: expected GetBucketNotificationConfiguration to answer 200, got {} with \
         {}",
        installed.status, installed.body
    );
    let configurations = xml_blocks(&installed.body, "CloudFunctionConfiguration");
    assert_eq!(
        configurations.len(),
        SUFFIXES.len(),
        "case {case:?}: expected one stored configuration per declared suffix, got {}",
        installed.body
    );

    // The declared functions and the declared table really work together: handed the record
    // envelope S3 publishes, each handler records the key in the declared table — which it can
    // only do if the inline source it was declared with was packaged under the file its own
    // `Handler` and runtime name.
    for (function, control) in [
        (FUNCTION, "uploads/index-handler.jpg"),
        (SECOND_FUNCTION, "uploads/app-handler.jpg"),
    ] {
        let answered = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            json!({
                "Records": [{
                    "eventName": PUT_EVENT_NAME,
                    "s3": { "object": { "key": control } },
                }],
            })
            .to_string()
            .as_bytes(),
            &[("x-amz-invocation-type", "RequestResponse")],
        )
        .await;
        assert_eq!(
            (answered.status, answered.function_error()),
            (200, None),
            "case {case:?}: expected {function} to run to completion, got {} with {}",
            answered.status,
            answered.body
        );
        let recorded = awaited_item(&router, case, control, true).await;
        assert_eq!(
            recorded,
            Some(json!({
                KEY_ATTRIBUTE: { "S": control },
                EVENT_ATTRIBUTE: { "S": PUT_EVENT_NAME },
            })),
            "case {case:?}: expected {function}'s own write in the declared table, got \
             {recorded:?}"
        );
    }

    for row in ROWS {
        let case = row.case;
        let written = s3(
            &router,
            "PUT",
            &format!("/{BUCKET}/{}", row.key),
            b"the object the event names",
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected PutObject({:?}) to answer 200, got {} with {}",
            row.key, written.status, written.body
        );

        let held = awaited_item(&router, case, row.key, row.delivered).await;
        if !row.delivered {
            assert_eq!(
                held, None,
                "case {case:?}: no declared filter admits {:?}, so no function may run for it, \
                 got the item {held:?}",
                row.key
            );
            continue;
        }
        assert_eq!(
            held,
            Some(json!({
                KEY_ATTRIBUTE: { "S": row.key },
                EVENT_ATTRIBUTE: { "S": PUT_EVENT_NAME },
            })),
            "case {case:?}: expected the declared function to record the write of {:?}, got \
             {held:?}",
            row.key
        );
    }
}
