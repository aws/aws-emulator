// SPDX-License-Identifier: CC0-1.0

//! The PROCESSING-FAILURE lifecycle of a declared DynamoDB-stream consumer, read the way an
//! operator diagnosing one reads it: six crates have to agree for a verdict to exist at all —
//! CloudFormation deploys the estate and archives the inline handler into the function's
//! deployment package, Amazon DynamoDB holds the streamed tables, DynamoDB Streams serves their
//! shards, IAM holds the execution role the stream is read as, AWS Lambda polls the stream and
//! RUNS the handler, and CloudWatch Logs takes the log that run produced.
//!
//! Two rules, both about a consumer whose handler CANNOT process one of the records it is
//! handed:
//!
//! 1. **The attempts one batch gets are the mapping's own, and the shard reads ON once they are
//!    spent.** `MaximumRetryAttempts` is "(Kinesis, DynamoDB Streams, Amazon MSK, and
//!    self-managed Apache Kafka) Discard records after the specified number of retries"
//!    (`smithy_query(query="shape", shape="EventSourceMappingConfiguration")`, member
//!    `MaximumRetryAttempts`, `@range(min: -1, max: 10000)` with -1 the infinite default), so a
//!    mapping that states a finite count hands one batch to its function the first time plus
//!    that many times and no more — after which the records BEHIND that batch are handed over,
//!    rather than the one record its function cannot process stopping the stream for good.
//! 2. **Every phase of such an estate reaches a TERMINAL status, whatever ran through it.** The
//!    create settles `CREATE_COMPLETE`, and the destroy settles `DELETE_COMPLETE` with every
//!    declared resource gone from the service that held it — the mapping from Lambda, the
//!    function from Lambda, the table from DynamoDB — whether the estate processed nothing, a
//!    record it could handle, or a record it had to give up on. A client that deploys a consumer
//!    by template, exercises it and tears it down then has a status to branch on at each step
//!    instead of a call that neither succeeds nor is refused.
//!
//! The rows differ along what each rule generalises over: rule 1 along how many retries the
//! mapping allows one batch (none, one, the three a declared consumer states), rule 2 along what
//! ran through the estate before the teardown (nothing, a record the handler processed, a record
//! it could not).
//!
//! What the neighbours own and this suite does not restate:
//! [`crate::ddb_stream_lambda_record_images_and_unmodified_writes`] owns WHAT a record carries
//! per `StreamViewType`, [`crate::cfn_multi_stream_lambda_logged_lifecycle`] owns the create of a
//! stack wiring SEVERAL streamed tables to one function (and the readers and arming loop reused
//! here), [`crate::ddb_stream_lambda_esm_provisioning`] owns that a stream-enabled table is
//! mappable at all, [`crate::kinesis_esm_bucket_failure_destination`] owns where the batch a
//! KINESIS mapping gave up on is recorded, and the CloudFormation crate's own
//! `stream_consumer_stack_teardown` suite owns the teardown of such an estate per resource — that
//! a resource is `DELETE_COMPLETE` only once the service owning it no longer holds it, and that a
//! role is emptied before it goes. What is new here is the DynamoDB-stream consumer whose handler
//! FAILS: the bound on the attempts its batch gets, the delivery that follows them, and that the
//! estate reaches each of its terminal statuses whatever ran through it — the axis that neighbour
//! does not vary.
//!
//! Nothing waits a span of its own choosing: every wait looks again for the CONDITION it asserts
//! until that condition holds or one generous deadline passes, and nothing asserts on elapsed
//! time.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// Every wire caller comes from the suite that owns it rather than from a second copy here.
use crate::cfn_ddb_stream_esm_lambda_pipeline::{create_failures, settled};
// The stream estate's own readers and its arming loop belong to the sibling suite that deploys
// a stack of stream mappings; this suite drives them with its own handler's marker.
use crate::cfn_multi_stream_lambda_logged_lifecycle::{
    armed_delivery, listed_mappings, table_stream, write_item,
};
use crate::cloudformation_apigatewayv2_resources::cloudformation;
use crate::cloudformation_ddb_stream_lambda_trigger_estate::log_messages;
use crate::cloudformation_dynamodb_table_in_place_update::described_table;
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudformation_provisions_lambda_event_source_mappings::MAPPINGS_URI;
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::router as emulator;
use crate::logs_subscription_filter_to_lambda::lambda;
use crate::query_wire::{opt_element, outputs_of};

/// The `StackStatus` values the rules are stated in (`com.amazonaws.cloudformation#StackStatus`).
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
/// See [`CREATE_COMPLETE`].
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The partition key every declared table is keyed by.
const KEY_ATTRIBUTE: &str = "pk";

/// The position every declared mapping reads from, and the batch size it reads in: one record
/// per batch, so the batch the handler cannot process is exactly one record and the record
/// behind it is a batch of its own.
const STARTING_POSITION: &str = "LATEST";
/// See [`STARTING_POSITION`].
const BATCH_SIZE: i64 = 1;

/// AWS's own published execution-role policy for a function that reads a DynamoDB stream, which
/// grants the stream reads Lambda's poller makes on the function's behalf.
const DYNAMODB_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaDynamoDBExecutionRole";

/// The runtime every declared function asks for.
const RUNTIME: &str = "python3.12";

/// How long ONE invocation is given, as the function's own `Timeout`: well past what a run needs
/// on any host, because what is under test is the hand-over and not how fast an interpreter gets
/// through a handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long ONE row waits, in all, for the work the clock owes it — a stack settling, a
/// mapping's poller arming, the attempts at a batch its function errors on, and the teardown of
/// what the create made. One deadline is taken at the start of a row and every wait looks again
/// until its condition holds or that instant passes, so a row that will fail is bounded by this
/// span rather than by the sum of its waits.
///
/// A generous WALL-CLOCK deadline rather than a count of reads: the spans the services behind a
/// stack choose take the time they take, so a loaded host cannot decide the outcome, and all the
/// deadline is for is that the wait TERMINATES. Nothing here asserts on elapsed time.
const WAIT: Duration = Duration::from_secs(180);
/// See [`WAIT`].
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The marker the handler writes one line per record it was HANDED under, so a count of those
/// lines is a count of the attempts the mapping made — and a marker the refusal below does not
/// repeat, so the run's own error output cannot be read as another attempt.
const HANDED: &str = "onyx-handled";
/// What the handler raises on a record it cannot process, which is a different word from
/// [`HANDED`] for that reason.
const REFUSED: &str = "onyx-refused";

/// The key prefix of the record this handler cannot process, and of the one written behind it.
const POISON_KEY: &str = "poison";
/// See [`POISON_KEY`].
const BEHIND_KEY: &str = "behind";
/// The key of a record the handler processes, for the row that exercises the estate the happy
/// way.
const PROCESSED_KEY: &str = "processed";

/// The handler the template archives into the function's deployment package: one line per record
/// of the batch it was handed, naming the stream the record came from and the key it changed, and
/// an error on the record whose key marks it as one this consumer cannot process — which is the
/// processing failure an operator diagnoses.
const RECORD_HANDLER: &str = r#"
def handler(event, context):
    for record in event["Records"]:
        key = record["dynamodb"]["Keys"]["pk"]["S"]
        print("onyx-handled " + record["eventSourceARN"] + " " + key)
        if key.startswith("poison"):
            raise RuntimeError("onyx-refused " + record["eventSourceARN"])
    return {"records": len(event["Records"])}
"#;

/// The logical ids the role and the function are declared under.
const ROLE_ID: &str = "HandlerRole";
/// See [`ROLE_ID`].
const FUNCTION_ID: &str = "Handler";

/// One source the estate declares: a streamed table, and the mapping polling its stream with the
/// retries that mapping allows one batch.
struct Source {
    /// How the row this source belongs to is named in every assertion.
    case: &'static str,
    /// The name the table and the mapping are declared under.
    name: &'static str,
    /// The `MaximumRetryAttempts` the mapping declares. Stated by every row rather than left
    /// out, because a mapping that declares none takes the member's documented default of -1 —
    /// infinite retries — which is not a count of attempts to assert at all.
    retries: i64,
}

impl Source {
    /// The logical id this source's table is declared under.
    fn table_id(&self) -> String {
        format!("{}Table", self.name)
    }

    /// The logical id the mapping polling that table's stream is declared under.
    fn mapping_id(&self) -> String {
        format!("{}Mapping", self.name)
    }

    /// The name the table is created under, which the declaration STATES — so every expected
    /// value below is computed from the template rather than read out of the answer under test.
    fn table(&self, stack: &str) -> String {
        format!("{stack}-{}", self.name)
    }

    /// How many times one batch is handed to the function: the first attempt plus the retries
    /// the mapping allows ("Discard records after the specified number of retries").
    fn attempts(&self) -> usize {
        let retries = usize::try_from(self.retries.max(0)).unwrap_or_default();
        retries.saturating_add(1)
    }
}

/// The estate as one template: one streamed table per source — all of them independent of one
/// another — the execution role carrying AWS's own stream-reader policy, the handler, and one
/// mapping per table's stream.
fn template(stack: &str, sources: &[Source]) -> String {
    let mut resources = Map::new();
    for source in sources {
        resources.insert(
            source.table_id(),
            json!({
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": source.table(stack),
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
    for source in sources {
        let properties = json!({
            "EventSourceArn": {"Fn::GetAtt": [source.table_id(), "StreamArn"]},
            "FunctionName": {"Ref": FUNCTION_ID},
            "StartingPosition": STARTING_POSITION,
            "BatchSize": BATCH_SIZE,
            "MaximumRetryAttempts": source.retries,
        });
        resources.insert(
            source.mapping_id(),
            json!({"Type": "AWS::Lambda::EventSourceMapping", "Properties": properties}),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a DynamoDB-stream consumer whose handler cannot process every record",
        "Resources": Value::Object(resources),
        "Outputs": {
            "FunctionName": {"Value": {"Ref": FUNCTION_ID}},
        },
    })
    .to_string()
}

/// Submit the estate and answer the function it published, with the stack id the teardown is
/// read by — the create has to settle `CREATE_COMPLETE` for either to mean anything.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    sources: &[Source],
    giving_up_at: Instant,
) -> (String, String) {
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(stack, sources)),
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
    let stack_id = opt_element(&answered, "StackId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateStack published no StackId, got {answered}")
    });
    let (reached, described) = settled(router, stack, giving_up_at).await;
    assert_eq!(
        reached,
        CREATE_COMPLETE,
        "case {case:?}: expected the estate to settle {CREATE_COMPLETE}, got {described} with \
         resource failures {:?}",
        create_failures(router, stack).await
    );
    let function = outputs_of(&described)
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionName output in {described}"));
    (function, stack_id)
}

/// One line the handler wrote per record it was HANDED off `stream` under a key starting with
/// `key`, as the function's own default log group holds them.
async fn handed_over(
    router: &MultiServiceRouter,
    function: &str,
    stream: &str,
    key: &str,
) -> Vec<String> {
    log_messages(router, function)
        .await
        .into_iter()
        .filter(|line| line.contains(&format!("{HANDED} {stream} {key}")))
        .collect()
}

/// Drive `table` until its mapping's poller has handed a record of `stream` to the handler, so
/// a mapping reading from `LATEST` — which hands over only what was written after it armed — is
/// ARMED rather than raced. The loop is the sibling suite's ([`armed_delivery`]); what this one
/// adds is that an estate that never armed fails HERE, naming the step that did not complete.
async fn arm(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    table: &str,
    stream: &str,
    giving_up_at: Instant,
) {
    let seen = armed_delivery(router, case, function, table, stream, HANDED, giving_up_at).await;
    assert!(
        seen.iter()
            .any(|line| line.contains(HANDED) && line.contains(stream)),
        "case {case:?}: expected the mapping on {table:?}'s stream {stream:?} to arm — a \
         {HANDED:?} line naming it in the handler's log group — got {seen:?}"
    );
}

/// Look again until the handler has been handed a record off `stream` under `key`, answering
/// whether it ever was.
async fn awaited(
    router: &MultiServiceRouter,
    function: &str,
    stream: &str,
    key: &str,
    giving_up_at: Instant,
) -> bool {
    while Instant::now() < giving_up_at {
        if !handed_over(router, function, stream, key).await.is_empty() {
            return true;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    false
}

/// RULE 1: the attempts one batch gets are the mapping's own — the first hand-over plus the
/// retries `MaximumRetryAttempts` allows — and once they are spent the shard reads ON, so the
/// record written behind the one its function cannot process is handed over.
#[tokio::test]
async fn a_batch_the_handler_cannot_process_is_attempted_as_often_as_its_mapping_allows() {
    // One estate, one function, one source per row: the rows differ only in how many retries
    // their own mapping allows one batch, and each row's records name their own stream.
    let sources = [
        Source {
            case: "a mapping that allows no retry",
            name: "NoRetry",
            retries: 0,
        },
        Source {
            case: "a mapping that allows one retry",
            name: "OneRetry",
            retries: 1,
        },
        Source {
            // The shape a declared stream consumer states.
            case: "a mapping that allows three retries",
            name: "ThreeRetries",
            retries: 3,
        },
    ];
    let stack = "xsvc-stream-processing-failure-retries";
    let router = emulator();
    let giving_up_at = Instant::now() + WAIT;
    let (function, _) = deploy(&router, "the estate", stack, &sources, giving_up_at).await;

    // Every row's mapping is one Lambda lists for the function, carrying the retries its own
    // declaration asked for.
    let listed = listed_mappings(&router, "the estate", &function).await;
    let mut streams = Vec::new();
    for source in &sources {
        let case = source.case;
        let table = source.table(stack);
        let stream = table_stream(&router, case, &table).await;
        let polling = listed
            .iter()
            .find(|mapping| {
                mapping.get("EventSourceArn").and_then(Value::as_str) == Some(stream.as_str())
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a mapping polling {table:?}'s stream {stream:?}, got \
                     {listed:?}"
                )
            });
        assert_eq!(
            polling.get("MaximumRetryAttempts").and_then(Value::as_i64),
            Some(source.retries),
            "case {case:?}: expected the mapping on {stream:?} to carry the declared \
             MaximumRetryAttempts {}, got {polling}",
            source.retries
        );
        arm(&router, case, &function, &table, &stream, giving_up_at).await;
        streams.push((table, stream));
    }

    // The record the handler cannot process, and one written behind it in the same shard.
    for (table, _) in &streams {
        write_item(&router, "the estate", table, POISON_KEY).await;
        write_item(&router, "the estate", table, BEHIND_KEY).await;
    }

    for (source, (table, stream)) in sources.iter().zip(&streams) {
        let case = source.case;
        // The record BEHIND the one its function cannot process is the condition the attempts
        // are counted at: a shard is read in order, so that record is handed over only once the
        // mapping has given the one in front of it up.
        let read_on = awaited(&router, &function, stream, BEHIND_KEY, giving_up_at).await;
        assert!(
            read_on,
            "case {case:?}: expected {BEHIND_KEY:?} of {table:?} to be handed over once the \
             attempts at {POISON_KEY:?} were spent, got {:?}",
            log_messages(&router, &function).await
        );
        let attempts = handed_over(&router, &function, stream, POISON_KEY).await;
        assert_eq!(
            attempts.len(),
            source.attempts(),
            "case {case:?}: expected {POISON_KEY:?} of {table:?} to be handed over {} time(s) — \
             the first attempt plus the {} retries the mapping allows — got {attempts:?}",
            source.attempts(),
            source.retries
        );
        // ...and what the handler could not process said so on every one of those attempts. A
        // lower bound, because one run's error is the runtime's to frame in as many lines as it
        // frames it in; what is pinned is that no attempt was silent.
        let refusals = log_messages(&router, &function)
            .await
            .into_iter()
            .filter(|line| line.contains(&format!("{REFUSED} {stream}")))
            .count();
        assert!(
            refusals >= source.attempts(),
            "case {case:?}: expected each of the {} attempts at {POISON_KEY:?} to report \
             {REFUSED:?} for {stream:?}, got {refusals}",
            source.attempts()
        );
    }
}

/// What ran through the estate before its teardown, which is what rule 2 generalises over.
struct Exercised {
    case: &'static str,
    stack: &'static str,
    /// The key written to the estate's table, or `None` for the estate nothing was written to.
    written: Option<&'static str>,
}

/// RULE 2: the estate reaches a terminal status at each phase — `CREATE_COMPLETE`, then
/// `DELETE_COMPLETE` with the mapping, the function and the table gone from the services that
/// held them — whatever ran through it in between, which is the axis this suite adds to the
/// per-resource teardown the CloudFormation crate's `stream_consumer_stack_teardown` owns.
#[tokio::test]
async fn the_streamed_estate_is_destroyed_whatever_ran_through_it() {
    let rows = [
        Exercised {
            case: "an estate nothing was written to",
            stack: "xsvc-stream-failure-teardown-idle",
            written: None,
        },
        Exercised {
            case: "an estate whose handler processed a record",
            stack: "xsvc-stream-failure-teardown-processed",
            written: Some(PROCESSED_KEY),
        },
        Exercised {
            case: "an estate whose handler could not process a record",
            stack: "xsvc-stream-failure-teardown-refused",
            written: Some(POISON_KEY),
        },
    ];
    for row in rows {
        let case = row.case;
        let sources = [Source {
            case,
            name: "Requests",
            retries: 0,
        }];
        let router = emulator();
        let giving_up_at = Instant::now() + WAIT;
        let (function, stack_id) = deploy(&router, case, row.stack, &sources, giving_up_at).await;
        let table = sources[0].table(row.stack);
        let stream = table_stream(&router, case, &table).await;
        let uuid = listed_mappings(&router, case, &function)
            .await
            .first()
            .and_then(|mapping| {
                mapping
                    .get("UUID")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| panic!("case {case:?}: no mapping listed for {function:?}"));

        if let Some(key) = row.written {
            arm(&router, case, &function, &table, &stream, giving_up_at).await;
            write_item(&router, case, &table, key).await;
            let handled = awaited(&router, &function, &stream, key, giving_up_at).await;
            assert!(
                handled,
                "case {case:?}: expected {key:?} of {table:?} to reach the handler, got {:?}",
                log_messages(&router, &function).await
            );
        }

        // The teardown: one call, and a terminal status to branch on.
        let (status, answered) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {answered}"
        );
        let (reached, described) = wait_for_stack_status(&router, &stack_id, DELETE_COMPLETE).await;
        assert_eq!(
            reached, DELETE_COMPLETE,
            "case {case:?}: expected the teardown to settle {DELETE_COMPLETE}, got {described}"
        );

        // ...and every declared resource is gone from the service that held it.
        let reported = lambda(&router, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None).await;
        assert_eq!(
            reported.status,
            404,
            "case {case:?}: expected the mapping {uuid} to be gone from Lambda, got {} with body \
             {:?}",
            reported.status,
            reported.text()
        );
        let configured = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{function}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            configured.status,
            404,
            "case {case:?}: expected the function {function:?} to be gone from Lambda, got {} \
             with body {:?}",
            configured.status,
            configured.text()
        );
        let (described_status, description) = described_table(&router, &table).await;
        assert_eq!(
            described_status, 400,
            "case {case:?}: expected the table {table:?} to be gone from DynamoDB, got \
             {described_status} with body {description}"
        );
    }
}
