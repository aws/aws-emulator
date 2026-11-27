// SPDX-License-Identifier: CC0-1.0

//! A stack that wires SEVERAL streamed DynamoDB tables to ONE Lambda function, read the way a
//! client reads it.
//!
//! Two rules, both about a stack that declares SEVERAL stream triggers onto one function:
//!
//! 1. **The create reaches a TERMINAL status, and that status is `CREATE_COMPLETE`.** A client
//!    that deploys a DynamoDB-Streams consumer by template and polls `DescribeStacks` gets a
//!    status to branch on — `com.amazonaws.cloudformation#StackStatus` enumerates
//!    `CREATE_IN_PROGRESS` -> `CREATE_COMPLETE`, with `ROLLBACK_COMPLETE` only after a
//!    resource create fails — so a declaration every owning service accepts never leaves the
//!    stack short of one.
//! 2. **Every declared mapping of the stack ends up polling.** Once the stack settles
//!    `CREATE_COMPLETE`, `ListEventSourceMappings(FunctionName=<function>)` reports one mapping
//!    per declared event source, each carrying the members its declaration asked for, and a
//!    write to ANY of those tables is handed to the one function — whose per-record output
//!    lands in the default log group `/aws/lambda/<function name>`.
//!
//! The rows differ along what the rules generalise over: how MANY streamed tables one function
//! is wired to in one stack (one, two, three), and so along how many mappings the one function
//! carries and which table a write goes to. Every mapping reads from `LATEST`, the position that
//! hands over only what was written after it armed, so each table's delivery is driven by writes
//! until the handler's line for one of them lands — which is how a client arms such a mapping.
//!
//! The axis that is new here is SEVERAL streamed tables onto ONE function in one stack:
//! `all/cloudformation_ddb_stream_lambda_trigger_estate` owns the single-stream estate over the
//! same six services (every `StreamViewType`, both billing modes, with and without a sort key),
//! `all/cfn_ddb_stream_esm_lambda_pipeline` owns what the handler does with the records it is
//! handed, and `all/cloudformation_event_source_mapping_stabilisation` owns the state ONE
//! declared mapping has reached when the stack first reports complete. None of them is repeated
//! here.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// Every wire caller comes from the suite that owns it rather than from a second copy here.
use crate::cfn_ddb_stream_esm_lambda_pipeline::{create_failures, settled};
use crate::cloudformation_apigatewayv2_resources::cloudformation;
use crate::cloudformation_ddb_stream_lambda_trigger_estate::log_messages;
use crate::cloudformation_dynamodb_table_in_place_update::{described_table, dynamodb};
// `ListEventSourceMappings` is reached at the URI the suite that owns the mapping vocabulary
// states ([`MAPPINGS_URI`]: `GET /2015-03-31/event-source-mappings`).
use crate::cloudformation_provisions_lambda_event_source_mappings::MAPPINGS_URI;
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::router as emulator;
use crate::logs_subscription_filter_to_lambda::{lambda, LAMBDA_LOG_GROUP_PREFIX};
use crate::query_wire::{block_of, members_of, outputs_of};

/// The `StackStatus` the rules are stated in, and the suffix every status a stack operation is
/// still running reports ends in (`com.amazonaws.cloudformation#StackStatus`).
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const IN_PROGRESS_SUFFIX: &str = "_IN_PROGRESS";

/// The partition key each declared table is keyed by, and the attribute a write sets.
const KEY_ATTRIBUTE: &str = "pk";
const VALUE_ATTRIBUTE: &str = "v";

/// The mapping members every row declares, which are what the listing has to report back
/// (`CreateEventSourceMapping`: `StartingPosition`, `BatchSize`, `MaximumRetryAttempts`).
const STARTING_POSITION: &str = "LATEST";
const BATCH_SIZE: i64 = 1;
const MAXIMUM_RETRY_ATTEMPTS: i64 = 3;

/// AWS's own published execution-role policy for a function that reads a DynamoDB stream, which
/// grants the stream reads Lambda's poller makes on the function's behalf.
const DYNAMODB_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaDynamoDBExecutionRole";

/// The runtime every declared function asks for.
const RUNTIME: &str = "python3.12";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run needs
/// on any host, because what is under test is the wiring and not how fast an interpreter gets
/// through a handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long ONE row waits, in all, for the work the clock owes it — a stack settling, the
/// mappings' pollers arming, the delivery of a batch and the records the run wrote. One deadline
/// is taken at the start of a row and every wait looks again until its condition holds or that
/// instant passes, so a row that will fail is bounded by this span rather than by the sum of its
/// waits.
///
/// A generous WALL-CLOCK deadline rather than a count of reads: the spans the services behind a
/// stack choose take the time they take, so a loaded host cannot decide the outcome, and all the
/// deadline is for is that the wait TERMINATES — an order of magnitude over what a passing row
/// needs, so a broken one still reports promptly. Nothing here asserts on elapsed time.
const WAIT: Duration = Duration::from_secs(120);
/// See [`WAIT`].
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How many looks at the log group one arming write is given before another is made
/// ([`armed_delivery`]).
const LOOKS_PER_WRITE: usize = 20;

/// The marker the handler writes one line per record under, so the assertion reads the
/// function's OWN output rather than the runtime's framing lines.
const RECORD_MARKER: &str = "onyx-record";

/// The handler the template archives into the function's deployment package: one line per record
/// of the batch it was handed, naming the event source the record came from and the key it
/// changed.
const RECORD_HANDLER: &str = r#"
def handler(event, context):
    for record in event["Records"]:
        print(
            "onyx-record "
            + record["eventSourceARN"]
            + " "
            + record["dynamodb"]["Keys"]["pk"]["S"]
            + " "
            + record["eventName"]
        )
    return {"records": len(event["Records"])}
"#;

/// The logical ids the role and the function are declared under.
const ROLE_ID: &str = "HandlerRole";
const FUNCTION_ID: &str = "Handler";

/// One row: how many streamed tables one function is wired to in one stack.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The name of each source the stack wires up, which its table and mapping are declared
    /// under.
    sources: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one streamed table wired to the handler",
            stack: "xsvc-multi-stream-onyx-single",
            sources: &["Requests"],
        },
        Row {
            // The recorded shape: two tables' streams, one handler, two mappings.
            case: "two streamed tables wired to one handler",
            stack: "xsvc-multi-stream-onyx-pair",
            sources: &["Requests", "RegressionRequests"],
        },
        Row {
            case: "three streamed tables wired to one handler",
            stack: "xsvc-multi-stream-onyx-triple",
            sources: &["Requests", "RegressionRequests", "ReplayRequests"],
        },
    ]
}

impl Row {
    /// The logical id the table of `source` is declared under.
    fn table_id(source: &str) -> String {
        format!("{source}Table")
    }

    /// The logical id the mapping polling that table's stream is declared under.
    fn mapping_id(source: &str) -> String {
        format!("{source}Mapping")
    }

    /// The name each declared table is created under, which the declaration STATES — so every
    /// expected value below is computed from the template rather than read back out of the
    /// answer under test.
    fn table_names(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| format!("{}-{source}", self.stack))
            .collect()
    }
}

/// The whole estate as one template: one streamed table per source — all of them independent of
/// one another — the execution role carrying AWS's own stream-reader policy, the handler, and one
/// mapping per table's stream.
fn template(row: &Row) -> String {
    let mut resources = Map::new();
    for (source, name) in row.sources.iter().zip(row.table_names()) {
        resources.insert(
            Row::table_id(source),
            json!({
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": name,
                    "AttributeDefinitions": [
                        {"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}
                    ],
                    "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
                    "BillingMode": "PAY_PER_REQUEST",
                    "StreamSpecification": {"StreamViewType": "NEW_AND_OLD_IMAGES"},
                },
            }),
        );
    }
    resources.insert(
        ROLE_ID.to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "lambda.amazonaws.com"},
                        "Action": "sts:AssumeRole",
                    }],
                },
                "ManagedPolicyArns": [DYNAMODB_EXECUTION_POLICY],
            },
        }),
    );
    resources.insert(
        FUNCTION_ID.to_string(),
        json!({
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "Runtime": RUNTIME,
                "Handler": "index.handler",
                "Role": {"Fn::GetAtt": [ROLE_ID, "Arn"]},
                "Timeout": FUNCTION_TIMEOUT,
                "Code": {"ZipFile": RECORD_HANDLER},
            },
        }),
    );
    for source in row.sources {
        resources.insert(
            Row::mapping_id(source),
            json!({
                "Type": "AWS::Lambda::EventSourceMapping",
                "Properties": {
                    "EventSourceArn": {"Fn::GetAtt": [Row::table_id(source), "StreamArn"]},
                    "FunctionName": {"Ref": FUNCTION_ID},
                    "StartingPosition": STARTING_POSITION,
                    "BatchSize": BATCH_SIZE,
                    "MaximumRetryAttempts": MAXIMUM_RETRY_ATTEMPTS,
                },
            }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "several streamed tables read by one Lambda handler",
        "Resources": Value::Object(resources),
        "Outputs": {
            "FunctionName": {"Value": {"Ref": FUNCTION_ID}},
        },
    })
    .to_string()
}

/// Submit one row's stack, and answer the status it settled in with the `DescribeStacks` answer
/// that status was read from.
///
/// The wait for a TERMINAL status is the sibling suite's ([`settled`]), which is the one reading
/// of "the operation has finished, in whatever status it finished in" in this crate.
async fn deploy(router: &MultiServiceRouter, row: &Row, giving_up_at: Instant) -> (String, String) {
    let case = row.case;
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", &template(row)),
            // The template declares an IAM role, which a create acknowledges before
            // CloudFormation provisions it.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );
    settled(router, row.stack, giving_up_at).await
}

/// What `ListEventSourceMappings` reports for `function`.
///
/// `pub(crate)` because a sibling suite that deploys a stack of stream mappings and then reads
/// them back needs exactly this read, and a second copy of it drifts from this one — which is
/// also why [`write_item`], [`table_stream`] and [`armed_delivery`] are reachable.
pub(crate) async fn listed_mappings(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Vec<Value> {
    let answered = lambda(
        router,
        "GET",
        &format!("{MAPPINGS_URI}?FunctionName={function}"),
        None,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from ListEventSourceMappings for {function:?}, got {} \
         with body {:?}",
        answered.status,
        answered.text()
    );
    answered
        .json()
        .get("EventSourceMappings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Write one item to `table`, as a client drives the pipeline.
///
/// `pub(crate)` for the reason [`listed_mappings`] is.
pub(crate) async fn write_item(router: &MultiServiceRouter, case: &str, table: &str, key: &str) {
    let (status, answered) = dynamodb(
        router,
        "PutItem",
        &json!({
            "TableName": table,
            "Item": {
                KEY_ATTRIBUTE: {"S": key},
                VALUE_ATTRIBUTE: {"S": "written"},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from PutItem on {table:?} under {key:?}, got {status} \
         with body {answered}"
    );
}

/// The stream one of the stack's tables publishes — the ARN its mapping has to poll.
///
/// `pub(crate)` for the reason [`listed_mappings`] is.
pub(crate) async fn table_stream(router: &MultiServiceRouter, case: &str, table: &str) -> String {
    let (status, described) = described_table(router, table).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTable for {table:?}, got {status} with \
         body {described}"
    );
    described
        .pointer("/Table/LatestStreamArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Write to `table` until the handler's line for one of those writes is in the log group, so a
/// mapping reading from `LATEST` — which hands over only what was written after it armed — is
/// ARMED rather than raced. Answers the messages the group last held.
///
/// `marker` is what the handler of the estate under test writes its own lines under, which is
/// the only part of "a record of this stream reached the handler" that differs between the
/// suites sharing this loop ([`listed_mappings`] says why they share it).
pub(crate) async fn armed_delivery(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    table: &str,
    stream: &str,
    marker: &str,
    giving_up_at: Instant,
) -> Vec<String> {
    let mut attempt = 0usize;
    let mut seen = Vec::new();
    while Instant::now() < giving_up_at {
        attempt = attempt.saturating_add(1);
        write_item(router, case, table, &format!("arming-{attempt}")).await;
        // One write is given a bounded number of looks before the next arming write, so a
        // mapping that armed only after a later write is still reached.
        for _ in 0..LOOKS_PER_WRITE {
            seen = log_messages(router, function).await;
            if seen
                .iter()
                .any(|line| line.contains(marker) && line.contains(stream))
            {
                return seen;
            }
            if Instant::now() >= giving_up_at {
                return seen;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
    seen
}

/// RULE 1: the create of this estate reaches a TERMINAL status, and that status is
/// `CREATE_COMPLETE` with every declared resource reported created — so a client that deploys
/// the consumer by template and waits for it has a status to branch on rather than a call that
/// neither succeeds nor is refused.
#[tokio::test]
async fn the_declared_multi_stream_estate_reaches_create_complete() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let giving_up_at = Instant::now() + WAIT;

        let (reached, described) = deploy(&router, &row, giving_up_at).await;
        assert!(
            !reached.ends_with(IN_PROGRESS_SUFFIX) && !reached.is_empty(),
            "case {case:?}: expected the create to reach a terminal status, got {reached:?} with \
             {described}"
        );
        assert_eq!(
            reached,
            CREATE_COMPLETE,
            "case {case:?}: expected the stack to settle {CREATE_COMPLETE}, got {described} with \
             resource failures {:?}",
            create_failures(&router, row.stack).await
        );

        // ...and every declaration of the template is a resource the stack reports created: the
        // tables, the role, the function and one mapping per table's stream.
        let (status, resources) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported: Vec<(String, String)> = members_of(&resources)
            .into_iter()
            .filter_map(|member| {
                Some((
                    block_of(member, "LogicalResourceId")?.to_string(),
                    block_of(member, "ResourceStatus")?.to_string(),
                ))
            })
            .collect();
        let mut declared: Vec<String> = vec![ROLE_ID.to_string(), FUNCTION_ID.to_string()];
        for source in row.sources {
            declared.push(Row::table_id(source));
            declared.push(Row::mapping_id(source));
        }
        for logical_id in declared {
            let status = reported
                .iter()
                .find(|(reported_id, _)| *reported_id == logical_id)
                .map(|(_, status)| status.as_str());
            assert_eq!(
                status,
                Some(CREATE_COMPLETE),
                "case {case:?}: expected the stack to report {logical_id} {CREATE_COMPLETE}, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE 2: every declared mapping of a multi-source stack is created and polling once the stack
/// settles, and a write to each source is delivered to the one function, whose per-record output
/// lands in its default log group.
#[tokio::test]
async fn every_declared_stream_mapping_of_one_stack_delivers_to_its_function() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let giving_up_at = Instant::now() + WAIT;

        let (reached, described) = deploy(&router, &row, giving_up_at).await;
        assert_eq!(
            reached,
            CREATE_COMPLETE,
            "case {case:?}: expected the stack to settle {CREATE_COMPLETE}, got {described} with \
             resource failures {:?}",
            create_failures(&router, row.stack).await
        );

        let outputs = outputs_of(&described);
        let function = outputs
            .get("FunctionName")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no FunctionName output in {outputs:?}"));
        let mut tables = Vec::new();
        for table in row.table_names() {
            let stream = table_stream(&router, case, &table).await;
            tables.push((table, stream));
        }

        // Every declared mapping is one Lambda lists for the function, with the members the
        // declaration asked for and the stream of its own table.
        let listed = listed_mappings(&router, case, &function).await;
        assert_eq!(
            listed.len(),
            row.sources.len(),
            "case {case:?}: expected {} mappings for {function:?}, got {listed:?}",
            row.sources.len()
        );
        for (table, stream) in &tables {
            let polling = listed.iter().find(|mapping| {
                mapping.get("EventSourceArn").and_then(Value::as_str) == Some(stream.as_str())
            });
            let polling = polling.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a mapping polling {table:?}'s stream {stream:?}, got \
                     {listed:?}"
                )
            });
            assert_eq!(
                (
                    polling.get("StartingPosition").and_then(Value::as_str),
                    polling.get("BatchSize").and_then(Value::as_i64),
                    polling.get("MaximumRetryAttempts").and_then(Value::as_i64),
                ),
                (
                    Some(STARTING_POSITION),
                    Some(BATCH_SIZE),
                    Some(MAXIMUM_RETRY_ATTEMPTS)
                ),
                "case {case:?}: expected the mapping on {stream:?} to carry the declared \
                 ({STARTING_POSITION}, {BATCH_SIZE}, {MAXIMUM_RETRY_ATTEMPTS}), got {polling}"
            );
        }

        // Each source really delivers: a write to it reaches the one handler, and the handler's
        // own line for it is in the function's default log group.
        for (table, stream) in &tables {
            let seen = armed_delivery(
                &router,
                case,
                &function,
                table,
                stream,
                RECORD_MARKER,
                giving_up_at,
            )
            .await;
            assert!(
                seen.iter()
                    .any(|line| line.contains(RECORD_MARKER) && line.contains(stream)),
                "case {case:?}: expected a {RECORD_MARKER} line naming {stream:?} in \
                 {LAMBDA_LOG_GROUP_PREFIX}{function}, got {seen:?}"
            );
        }
    }
}
