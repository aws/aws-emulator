// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING function's own AWS SDK can do with the GLUE DATA CATALOG, with the whole
//! emulator assembled.
//!
//! RULE: the AWS SDK a managed runtime ships on a handler's import path carries the service
//! data of every service the runtime's call seam binds requests for, so `boto3.client(<token>)`
//! for any one of them resolves a usable client — the call it makes is built, signed and
//! dispatched into this emulator, answered out of that service's OWN records and refused with
//! that service's own modelled error code — and `botocore.exceptions.UnknownServiceError` is
//! raised only for a token no service answers to, carrying botocore's own sentence. For the
//! data catalog that is what a curating step of a data pipeline does: a handler registers the
//! partition it just wrote (`CreatePartition` — "creates a new partition") and alters it
//! (`UpdatePartition`), and a later reader resolves the table's slices through `GetPartition`
//! and `GetPartitions`, so a handler whose client cannot be resolved curates nothing at all.
//!
//! Three crates have to agree for that to hold — CloudFormation archives the inline handler
//! into a deployment package and deploys the execution role beside it, Lambda runs the handler
//! and supplies it the SDK, and Glue owns the catalog the calls act on — so the suite lives
//! here: a crate-local suite can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: WHICH catalog operation the handler
//! calls (the three writes and the two reads the catalog publishes for a partitioned table),
//! WHICH partition values it addresses (two days of one table, so a single stored slice would
//! be told apart here), whether the entity it names exists at all (an absent partition, an
//! absent table, a database that already exists), and whether the SERVICE TOKEN names a service
//! the seam binds (one it does, and one it does not — the only case botocore's unknown-service
//! sentence is correct for). Everything the handler wrote is read back through Glue's own
//! operations afterwards, so what the calls reached is that service's own record and not the
//! answer the handler echoed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::reachable_services;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for, and so the Region the catalog lives in.
const REGION: &str = "us-west-2";

/// The SDK token a handler builds a data-catalog client with — the service's SDK id, lower
/// cased (`sdkId: "Glue"`,
/// `smithy_query(query="service_info",model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`).
const CATALOG_TOKEN: &str = "glue";

/// A token the RUNTIME's own SDK binds no requests for — a real AWS service whose protocol this
/// crate's call seam does not bind, whichever services the router it runs in mounts — which is
/// the one case botocore's unknown-service sentence is the correct answer for.
const UNBOUND_TOKEN: &str = "polly";

/// The catalog the handler curates: the database, the partitioned table and its one partition
/// key. Names are lower case because the catalog folds a name "to lowercase when it is stored"
/// (`DatabaseInput$Name`, `TableInput$Name`).
const DATABASE: &str = "xsvc-fn-glue-lake";
const TABLE: &str = "curated_orders";
const PARTITION_KEY: &str = "dt";

/// The description the database carries, read back through Glue afterwards.
const DATABASE_DESCRIPTION: &str = "the curated lake a function registers its output in";

/// The two days of the table the rows register, so one stored slice could not stand in for
/// both.
const DAY_ONE: &str = "2026-01-01";
const DAY_TWO: &str = "2026-01-02";

/// A day the catalog holds no partition for, and a table it holds nothing under at all — what
/// the refusal rows name.
const ABSENT_DAY: &str = "2026-02-09";
const ABSENT_TABLE: &str = "no_such_table";

/// The row count the handler first registers on the day-one partition, and the one it alters it
/// to — a partition parameter, so the alteration is observable in the catalog's own record.
const ROWS_AT_FIRST: &str = "3";
const ROWS_AFTER_ALTER: &str = "7";

/// The error codes Glue refuses with, and the status an AWS JSON client reads them at: a
/// database, table or partition the catalog does not hold is `EntityNotFoundException` and a
/// database it already holds is `AlreadyExistsException`, both `httpError` 400
/// (`smithy_query(query="operation",operation="GetPartition",model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`).
const ENTITY_NOT_FOUND: &str = "EntityNotFoundException";
const ALREADY_EXISTS: &str = "AlreadyExistsException";
const REFUSAL_STATUS: u64 = 400;

/// The role the probe function runs as, the function itself and the stack that deploys them.
const EXEC_ROLE: &str = "xsvc-fn-glue-catalog-exec";
const FUNCTION: &str = "xsvc-fn-glue-catalog-probe";
const STACK: &str = "xsvc-fn-glue-catalog-probes";

/// The `StackStatus` that stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is which calls a handler can make and not how
/// fast an interpreter gets through them.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — a stack completing, a function
/// reaching `Active` — and how many looks to take. Every span waited for is a service's own, so
/// nothing here sleeps for a fixed one, and the deadline is generous so a loaded host cannot
/// decide the outcome.
const LOOK: Duration = Duration::from_millis(50);
/// See [`LOOK`].
const LOOKS: usize = 2_400;

/// The handler every row runs: it builds a client for the service token the call names — with
/// no credentials of its own, so the call is signed as the session of the function's execution
/// role — calls the operation the call names with the parameters it carries, and reports what
/// came back: the answer, the service's own refusal, or the SDK's own report of a call it did
/// not make.
const PROBE_HANDLER: &str = r##"
import json

import boto3
from botocore.exceptions import ClientError, UnknownServiceError


def handler(event, context):
    reports = {}
    for asked in event["calls"]:
        client = boto3.client(asked.get("service", "glue"))
        try:
            answered = getattr(client, asked["method"])(**asked.get("parameters", {}))
        except ClientError as refused:
            reports[asked["id"]] = {
                "ok": False,
                "code": refused.response["Error"]["Code"],
                "status": refused.response["ResponseMetadata"]["HTTPStatusCode"],
            }
        except UnknownServiceError as unknown:
            reports[asked["id"]] = {
                "ok": False,
                "raised": "UnknownServiceError",
                "detail": str(unknown),
            }
        except Exception as raised:
            reports[asked["id"]] = {
                "ok": False,
                "raised": type(raised).__name__,
                "detail": str(raised),
            }
        else:
            answered.pop("ResponseMetadata", None)
            reports[asked["id"]] = {
                "ok": True,
                "answered": json.loads(json.dumps(answered, default=str)),
            }
    return {"reports": reports}
"##;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the catalog it acts on is the one this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Glue `awsJson1_1` call, made by this suite as the account's own credential: the
/// operation travels in the target header under the service shape's own name and the input
/// document is the body.
async fn glue(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("glue.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization("glue"))
        .body(Bytes::from(input.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parsed)
}

/// One Glue call that must be served, answered with its document.
async fn served(router: &MultiServiceRouter, case: &str, operation: &str, input: &Value) -> Value {
    let (status, answered) = glue(router, operation, input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Glue {operation} must answer 200, got {status} with {answered}"
    );
    answered
}

/// One Lambda `restJson1` call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The template the suite deploys: the execution role, granted the catalog surface a curating
/// step uses, and the probe function. The inline source is archived into the deployment package
/// by CloudFormation, which is how a function whose handler is written here comes to exist at
/// all.
fn stack_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ExecRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": EXEC_ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "probe-grants",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "logs:CreateLogGroup",
                                        "logs:CreateLogStream",
                                        "logs:PutLogEvents",
                                    ],
                                    "Resource": "*",
                                },
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "glue:CreateDatabase",
                                        "glue:CreateTable",
                                        "glue:CreatePartition",
                                        "glue:UpdatePartition",
                                        "glue:GetDatabase",
                                        "glue:GetTable",
                                        "glue:GetPartition",
                                        "glue:GetPartitions",
                                    ],
                                    "Resource": "*",
                                },
                            ],
                        },
                    }],
                },
            },
            "Probe": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["ExecRole", "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            },
        },
    })
    .to_string()
}

/// Deploy the stack and wait for `CREATE_COMPLETE`, answering the physical id of each declared
/// resource.
async fn deploy(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let template = stack_template();
    let (status, answered) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {answered}"
    );
    let mut reported = String::new();
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", STACK)],
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: DescribeStacks({STACK:?}) must succeed, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        if reported == CREATE_COMPLETE || !reported.ends_with("_IN_PROGRESS") {
            break;
        }
        tokio::time::sleep(LOOK).await;
    }
    let failures = if reported == CREATE_COMPLETE {
        Vec::new()
    } else {
        let (_, events) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", STACK)],
        )
        .await;
        elements(&events, "ResourceStatusReason")
    };
    assert_eq!(
        reported, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {described} with resource failures \
         {failures:?}"
    );
    let (status, resources) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStackResources",
        &[("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DescribeStackResources({STACK:?}) must succeed, got {status} with {resources}"
    );
    physical_ids_of(&resources)
}

/// Wait until Lambda reports the probe function `Active`, which is the state an invocation
/// needs.
async fn wait_until_active(router: &MultiServiceRouter) {
    let mut last = Value::Null;
    for _ in 0..LOOKS {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &[],
        )
        .await;
        if configuration["State"].as_str() == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {FUNCTION:?} Active, last reported {last}");
}

/// Invoke the probe function with the calls of one group and answer the reports it returned,
/// keyed by each call's own id.
async fn probe(router: &MultiServiceRouter, group: &str, calls: Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/invocations"),
        json!({"calls": calls}).to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "group {group:?}: the invocation must be served, got {status} with {payload}"
    );
    let raised = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        raised, None,
        "group {group:?}: the handler must run to completion, got {raised:?} with {payload}"
    );
    payload.get("reports").cloned().unwrap_or_else(|| {
        panic!("group {group:?}: the handler must report every call, got {payload}")
    })
}

/// One call's report, or a failure naming the group it is missing from.
fn report(reports: &Value, case: &str) -> Value {
    reports
        .get(case)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a report for it, got {reports}"))
}

/// The storage location one day's partition is registered at.
fn location(day: &str) -> String {
    format!("s3://{DATABASE}/{TABLE}/{PARTITION_KEY}={day}/")
}

/// The partition values the catalog holds, in order, out of a `GetPartitions` document.
fn held_values(answered: &Value) -> Vec<String> {
    let mut held: Vec<String> = answered["Partitions"]
        .as_array()
        .map(|partitions| {
            partitions
                .iter()
                .filter_map(|partition| partition["Values"][0].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    held.sort();
    held
}

/// Every row: what a handler's own data-catalog client reaches, and what the catalog holds
/// afterwards.
#[tokio::test]
async fn a_handlers_own_sdk_curates_the_glue_data_catalog() {
    let router = build_router();
    let physical = deploy(&router).await;
    let reported = physical.get("Probe").cloned().unwrap_or_default();
    assert_eq!(
        reported, FUNCTION,
        "setup: the stack must report the probe function as {FUNCTION:?}, got {reported:?}"
    );
    wait_until_active(&router).await;

    // ---- The curating sequence: the catalog a pipeline's step declares, the two slices it
    // registers, the alteration it makes and the two reads a later consumer resolves with.
    let reports = probe(
        &router,
        "the curating sequence",
        json!([
            {
                "id": "declare the database",
                "method": "create_database",
                "parameters": {
                    "DatabaseInput": {
                        "Name": DATABASE,
                        "Description": DATABASE_DESCRIPTION,
                    },
                },
            },
            {
                "id": "declare the partitioned table",
                "method": "create_table",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableInput": {
                        "Name": TABLE,
                        "TableType": "EXTERNAL_TABLE",
                        "StorageDescriptor": {
                            "Columns": [
                                {"Name": "order_id", "Type": "string"},
                                {"Name": "amount", "Type": "double"},
                            ],
                            "Location": format!("s3://{DATABASE}/{TABLE}/"),
                        },
                        "PartitionKeys": [{"Name": PARTITION_KEY, "Type": "string"}],
                    },
                },
            },
            {
                "id": "register the first day's partition",
                "method": "create_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": TABLE,
                    "PartitionInput": {
                        "Values": [DAY_ONE],
                        "StorageDescriptor": {"Location": location(DAY_ONE)},
                        "Parameters": {"rows": ROWS_AT_FIRST},
                    },
                },
            },
            {
                "id": "read the partition back in the handler",
                "method": "get_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": TABLE,
                    "PartitionValues": [DAY_ONE],
                },
            },
            {
                "id": "register a second day's partition",
                "method": "create_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": TABLE,
                    "PartitionInput": {
                        "Values": [DAY_TWO],
                        "StorageDescriptor": {"Location": location(DAY_TWO)},
                        "Parameters": {"rows": "1"},
                    },
                },
            },
            {
                "id": "alter the first day's partition",
                "method": "update_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": TABLE,
                    "PartitionValueList": [DAY_ONE],
                    "PartitionInput": {
                        "Values": [DAY_ONE],
                        "StorageDescriptor": {"Location": location(DAY_ONE)},
                        "Parameters": {"rows": ROWS_AFTER_ALTER},
                    },
                },
            },
            {
                "id": "resolve every slice of the table",
                "method": "get_partitions",
                "parameters": {"DatabaseName": DATABASE, "TableName": TABLE},
            },
        ]),
    )
    .await;

    for case in [
        "declare the database",
        "declare the partitioned table",
        "register the first day's partition",
        "read the partition back in the handler",
        "register a second day's partition",
        "alter the first day's partition",
        "resolve every slice of the table",
    ] {
        let answered = report(&reports, case);
        assert_eq!(
            answered["ok"],
            json!(true),
            "case {case:?}: expected the catalog call to be answered, got {answered}"
        );
    }

    // The read the handler made is the catalog's own record of what it had just registered —
    // the values it addressed, the table they belong to, the location it gave and the parameter
    // it carried, plus the one member only the server sets.
    let case = "read the partition back in the handler";
    let partition = report(&reports, case)
        .pointer("/answered/Partition")
        .cloned()
        .unwrap_or(Value::Null);
    for (member, wanted) in [
        ("Values", json!([DAY_ONE])),
        ("DatabaseName", json!(DATABASE)),
        ("TableName", json!(TABLE)),
    ] {
        let answered = partition.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            answered, wanted,
            "case {case:?}: expected the partition's {member} {wanted}, got {answered} in \
             {partition}"
        );
    }
    let answered = partition
        .pointer("/StorageDescriptor/Location")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        answered,
        json!(location(DAY_ONE)),
        "case {case:?}: expected the location the handler registered, got {answered} in \
         {partition}"
    );
    let answered = partition.pointer("/Parameters/rows").cloned().unwrap_or(Value::Null);
    assert_eq!(
        answered,
        json!(ROWS_AT_FIRST),
        "case {case:?}: expected the parameter the handler registered, got {answered} in \
         {partition}"
    );
    let creation_time = partition.get("CreationTime").cloned().unwrap_or(Value::Null);
    assert!(
        creation_time.as_str().is_some_and(|text| {
            text.starts_with("20") && text.contains(':') && text.ends_with("+00:00")
        }),
        "case {case:?}: expected the catalog's own CreationTime on the partition — the instant \
         the SDK handed the handler for a timestamp member, in UTC, as the handler's own report \
         rendered it — got {creation_time} in {partition}"
    );

    // Both slices are resolvable through the paginated read a consumer uses, and the handler
    // reads them as the catalog holds them.
    let case = "resolve every slice of the table";
    let answered = report(&reports, case)
        .get("answered")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        held_values(&answered),
        vec![DAY_ONE.to_string(), DAY_TWO.to_string()],
        "case {case:?}: expected both days the handler registered, got {answered}"
    );

    // ---- What the OWNING SERVICE holds afterwards, read back through Glue's own operations by
    // this suite: the calls reached the catalog's records rather than echoing the handler.
    let case = "the database the catalog holds";
    let answered = served(&router, case, "GetDatabase", &json!({"Name": DATABASE})).await;
    for (member, wanted) in [
        ("/Database/Name", json!(DATABASE)),
        ("/Database/Description", json!(DATABASE_DESCRIPTION)),
    ] {
        let held = answered.pointer(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            held, wanted,
            "case {case:?}: expected {member} {wanted}, got {held} in {answered}"
        );
    }

    let case = "the table the catalog holds";
    let answered = served(
        &router,
        case,
        "GetTable",
        &json!({"DatabaseName": DATABASE, "Name": TABLE}),
    )
    .await;
    let held = answered.pointer("/Table/Name").cloned().unwrap_or(Value::Null);
    assert_eq!(
        held,
        json!(TABLE),
        "case {case:?}: expected the table the handler declared, got {held} in {answered}"
    );
    let held = answered
        .pointer("/Table/PartitionKeys/0/Name")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        held,
        json!(PARTITION_KEY),
        "case {case:?}: expected the partition key the handler declared, got {held} in {answered}"
    );
    let held = answered
        .pointer("/Table/StorageDescriptor/Columns")
        .and_then(|columns| columns.as_array().map(Vec::len))
        .unwrap_or_default();
    assert_eq!(
        held, 2,
        "case {case:?}: expected the two columns the handler declared, got {held} in {answered}"
    );

    let case = "the altered partition the catalog holds";
    let answered = served(
        &router,
        case,
        "GetPartition",
        &json!({
            "DatabaseName": DATABASE,
            "TableName": TABLE,
            "PartitionValues": [DAY_ONE],
        }),
    )
    .await;
    let held = answered
        .pointer("/Partition/Parameters/rows")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        held,
        json!(ROWS_AFTER_ALTER),
        "case {case:?}: expected the alteration the handler made, got {held} in {answered}"
    );

    let case = "every partition the catalog holds";
    let answered = served(
        &router,
        case,
        "GetPartitions",
        &json!({"DatabaseName": DATABASE, "TableName": TABLE}),
    )
    .await;
    assert_eq!(
        held_values(&answered),
        vec![DAY_ONE.to_string(), DAY_TWO.to_string()],
        "case {case:?}: expected both days the handler registered, got {answered}"
    );

    // ---- The refusals: an entity the catalog does not hold and one it already holds are that
    // service's own modelled answers, caught in the handler as the SDK's `ClientError`, while a
    // token no service answers to is the one case botocore's unknown-service sentence is right
    // for.
    let reports = probe(
        &router,
        "the refusals",
        json!([
            {
                "id": "a partition the catalog does not hold",
                "method": "get_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": TABLE,
                    "PartitionValues": [ABSENT_DAY],
                },
            },
            {
                "id": "a table the catalog does not hold",
                "method": "create_partition",
                "parameters": {
                    "DatabaseName": DATABASE,
                    "TableName": ABSENT_TABLE,
                    "PartitionInput": {"Values": [DAY_ONE]},
                },
            },
            {
                "id": "a database the catalog already holds",
                "method": "create_database",
                "parameters": {"DatabaseInput": {"Name": DATABASE}},
            },
            {
                "id": "a token no service answers to",
                "service": UNBOUND_TOKEN,
                "method": "describe_voices",
                "parameters": {},
            },
        ]),
    )
    .await;

    for (case, code) in [
        ("a partition the catalog does not hold", ENTITY_NOT_FOUND),
        ("a table the catalog does not hold", ENTITY_NOT_FOUND),
        ("a database the catalog already holds", ALREADY_EXISTS),
    ] {
        let answered = report(&reports, case);
        assert_eq!(
            answered,
            json!({"ok": false, "code": code, "status": REFUSAL_STATUS}),
            "case {case:?}: expected the catalog's own refusal {code}, got {answered}"
        );
    }

    let case = "a token no service answers to";
    let answered = report(&reports, case);
    assert_eq!(
        answered["raised"],
        json!("UnknownServiceError"),
        "case {case:?}: expected botocore's unknown-service refusal, got {answered}"
    );
    let sentence = format!(
        "Unknown service: '{UNBOUND_TOKEN}'. Valid service names are: {}",
        reachable_services().join(", ")
    );
    assert_eq!(
        answered["detail"].as_str(),
        Some(sentence.as_str()),
        "case {case:?}: expected botocore's own sentence, got {answered}"
    );

    // The inventory the SENTENCE ITSELF lists, read off the sentence rather than off the table
    // behind it: it names the data catalog — which is what makes every row above a call rather
    // than a refusal — it does not name the token it was raised for, and it names each service
    // once, so a guest reads a list of services rather than a list of spellings.
    let listed: Vec<&str> = answered["detail"]
        .as_str()
        .and_then(|detail| detail.split_once("Valid service names are: "))
        .map(|(_opening, inventory)| inventory.split(", ").collect())
        .unwrap_or_default();
    assert!(
        listed.contains(&CATALOG_TOKEN) && !listed.contains(&UNBOUND_TOKEN),
        "case {case:?}: expected the listed inventory to name {CATALOG_TOKEN:?} and not \
         {UNBOUND_TOKEN:?}, got {listed:?}"
    );
    let mut once = listed.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(
        once.len(),
        listed.len(),
        "case {case:?}: expected the listed inventory to name each service once, got {listed:?}"
    );

    // The same two facts about the table the sentence is rendered from, so a token the seam binds
    // requests for but the inventory withholds fails here too.
    let catalog = reachable_services();
    assert!(
        catalog.contains(&CATALOG_TOKEN),
        "case {CATALOG_TOKEN:?}: expected the runtime's SDK inventory to name it, got {catalog:?}"
    );
    assert!(
        !catalog.contains(&UNBOUND_TOKEN),
        "case {UNBOUND_TOKEN:?}: expected the inventory not to name a service the seam binds no \
         requests for, got {catalog:?}"
    );
}
