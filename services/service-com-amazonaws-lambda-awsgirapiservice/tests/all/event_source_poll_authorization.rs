// SPDX-License-Identifier: CC0-1.0

//! What an ENABLED event source mapping may take out of its event source, while the
//! execution role of the function it invokes is granted — or no longer granted — the reads
//! its poller makes on that source.
//!
//! RULE: Lambda reads an event source ACTING AS the invoked function's execution role —
//! "Lambda reads items from the event source and invokes the function"
//! (`CreateEventSourceMapping`) as "the ARN of the function's execution role"
//! (`FunctionConfiguration$Role`) — so while the identity-based policies in force on that
//! role withhold one of the reads consuming that KIND of source takes, the mapping's pass
//! takes nothing out of the source: no record is handed over, no invocation is attempted and
//! nothing is deleted, so the source keeps everything it holds. The question is the SAME one
//! the call that wires the pair together is refused for, asked of the policies in force at
//! the instant of each pass rather than of the ones in force when the pair was wired — so a
//! grant taken away afterwards stops the polling, and a grant put back resumes it with the
//! backlog intact.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! and the source's own service mounted beside Lambda on ONE engine: each role is written
//! through the real `CreateRole`/`PutRolePolicy`/`DeleteRolePolicy`, each message is sent
//! and counted through Amazon SQS's own API, each record is put through Kinesis's own, and
//! the mapping is read back through `GetEventSourceMapping` — so a pass that polled what it
//! may not read fails here.
//!
//! The rows differ along what the rule generalises over: WHICH of the source's reads the
//! role still holds when the records arrive (all of them, the set short of each one in turn,
//! none at all), HOW the grant names the action and the resource (exactly, through a
//! namespace wildcard, on another source of the same kind), whether a grant is overturned by
//! an explicit `Deny` beside it, and WHICH kind of source is polled (a queue, whose reads are
//! its own API's calls, and a shard source, which is read with three calls of its service's).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{
    allow, create_function_as, given_role, iam, logs_only, KINESIS_READS, QUEUE_READS,
};
use crate::event_source_mappings::{
    call, call_sqs, create_mapping, create_queue, get_mapping, processing_result_within_budget,
    queue_message_counts, router_on, DrivenPolling,
};
// The stream fixture and the Kinesis calls are the ones the shard-source suites already
// publish, so every suite over that surface drives the same wire.
use crate::kinesis_event_source_delivery::{given_stream, put_records};

/// The calls consuming a SHARD source takes, which a mapping onto one is decided on — the
/// `decided_on_source` entries of that source's catalog entry in
/// `src/state/event_source_authorization.rs`, which is narrower than the whole grant
/// [`KINESIS_READS`] advises.
pub(crate) const SHARD_READS: [&str; 3] = ["GetRecords", "GetShardIterator", "DescribeStream"];

/// The `State` a mapping reports once it is polling its event source.
pub(crate) const STATE_ENABLED: &str = "Enabled";

/// The name every role below carries its one inline policy under, so a row REPLACES that
/// document rather than adding a second one beside it.
const POLICY_NAME: &str = "event-source-reads";

/// How long a row waits for work the clock owes — a function going `Active`, a mapping
/// settling into `Enabled`, a pass taking a batch — before it is a failure.
pub(crate) const BUDGET: Duration = Duration::from_secs(20);

/// How long between two looks while waiting.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long a row watches a source a blocked mapping may not read. Several of either
/// poller's intervals (half a second for a queue, a tenth for a shard source), so a mapping
/// that was going to poll has had many passes in which to do it.
pub(crate) const BLOCKED_WINDOW: Duration = Duration::from_secs(3);

/// Where `GetFunctionConfiguration` is bound, which is where a row reads a function's
/// `State`.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// How many poll passes a row gives a mapping that may read NOTHING, before it reads what
/// the source still holds: several, so a mapping that was going to poll has had more than one
/// pass in which to do it.
pub(crate) const BLOCKED_PASSES: usize = 3;

/// Lambda, IAM and Amazon SQS on ONE engine — the role is written where IAM holds it, the
/// queue where Amazon SQS holds it, and the mapping polls both in-process, the shape
/// `core/router` assembles — with
/// the poll passes of its mappings the suite's own ([`DrivenPolling`]).
///
/// A pass is background work, and nothing a caller can read says whether one has happened
/// yet, so a row that waited a span of its own choosing for one would be betting on how fast
/// the host runs — handing a batch over goes through `Invoke`, whose wait for this process's
/// run capacity is not a span this suite chooses. A row makes each pass itself and awaits it
/// instead, and then reads what that pass did.
pub(crate) fn driven_emulator_with_queues() -> (MultiServiceRouter, DrivenPolling) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let driven = DrivenPolling::claiming(&engine, &router);
    (router, driven)
}

/// The same, with Amazon Kinesis in place of Amazon SQS, for the shard-source rows.
pub(crate) fn emulator_with_streams() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// Replace the role's one inline policy with `statements`, or DELETE it outright when the
/// row grants nothing — both through IAM's own API, so what the poller reads is what a
/// client wrote.
async fn write_policy(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    statements: Option<Value>,
) {
    let (status, answered) = match statements {
        Some(statements) => {
            let document =
                json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
            iam(
                router,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name),
                    ("PolicyName", POLICY_NAME),
                    ("PolicyDocument", &document),
                ],
            )
            .await
        }
        None => {
            iam(
                router,
                "DeleteRolePolicy",
                &[("RoleName", role_name), ("PolicyName", POLICY_NAME)],
            )
            .await
        }
    };
    assert_eq!(
        status, 200,
        "case {case:?}: the row's policy must be written through IAM, got {status}: {answered}"
    );
}

/// Wait until the function `name` reports `Active`: "Lambda reads items from the event
/// source and invokes the function", so a mapping polls nothing until its function can be
/// invoked at all — a row has to be past that before it can read anything into a pass that
/// did not poll.
pub(crate) async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        if read.string("State").as_deref() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never became Active: {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create the mapping `body` asks for and wait until it reports `Enabled`, answering its
/// UUID: a mapping still settling reads nothing, so every row starts from a mapping that
/// polls.
pub(crate) async fn given_enabled_mapping(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> String {
    let created = create_mapping(router, body).await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = get_mapping(router, &uuid).await;
        if read.string("State").as_deref() == Some(STATE_ENABLED) {
            return uuid;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping never reported {STATE_ENABLED}: {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `SendMessage` of one body to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// Which of the queue's reads the role carries when the message arrives, and how the grant
/// names them.
enum Grant {
    /// Every read a reader of the queue makes, on the queue itself.
    EveryRead,
    /// Those reads short of the one named.
    Short(&'static str),
    /// Every read, with the one named explicitly denied beside the grant.
    DeniedBeside(&'static str),
    /// Every read, granted on ANOTHER queue of the same account.
    OnAnotherSource,
    /// Every action of the source service's namespace, on every resource.
    NamespaceWildcard,
    /// Nothing: the role's inline policy is deleted outright.
    Nothing,
}

impl Grant {
    /// The statements this grant writes for a source of `arn`, beside the log writes every
    /// execution role carries; `None` deletes the policy instead.
    fn statements(&self, namespace: &str, reads: &[&str], arn: &str, other: &str) -> Option<Value> {
        let allowed = |actions: Vec<&str>, resource: &str| {
            Some(json!([logs_only(), allow(namespace, &actions, resource)]))
        };
        match self {
            Self::EveryRead => allowed(reads.to_vec(), arn),
            Self::Short(withheld) => allowed(
                reads
                    .iter()
                    .filter(|read| *read != withheld)
                    .copied()
                    .collect(),
                arn,
            ),
            Self::DeniedBeside(denied) => Some(json!([
                logs_only(),
                allow(namespace, reads, arn),
                {
                    "Effect": "Deny",
                    "Action": format!("{namespace}:{denied}"),
                    "Resource": arn,
                },
            ])),
            Self::OnAnotherSource => allowed(reads.to_vec(), other),
            Self::NamespaceWildcard => allowed(vec!["*"], "*"),
            Self::Nothing => None,
        }
    }
}

/// One row: the grant in force when the records arrive, and whether the mapping may then
/// read them.
struct Row {
    case: String,
    grant: Grant,
    /// Whether the pass must take what the source holds.
    polls: bool,
}

/// The catalog of grants, for either kind of source: the whole set, the set short of each
/// read in turn, the two forms of grant that still cover the source, and the three ways a
/// read ends up withheld.
fn rows(reads: &[&'static str]) -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "every read granted on the source itself".to_string(),
            grant: Grant::EveryRead,
            polls: true,
        },
        Row {
            case: "every action of the service granted through a namespace wildcard"
                .to_string(),
            grant: Grant::NamespaceWildcard,
            polls: true,
        },
        Row {
            case: "no inline policy at all".to_string(),
            grant: Grant::Nothing,
            polls: false,
        },
        Row {
            case: "every read granted on another source of the same kind".to_string(),
            grant: Grant::OnAnotherSource,
            polls: false,
        },
    ];
    // One row per read the decision is made on, each withheld two ways: absent from the
    // grant, and explicitly denied beside it.
    for read in reads {
        rows.push(Row {
            case: format!("the grant short of {read}"),
            grant: Grant::Short(read),
            polls: false,
        });
    }
    rows.push(Row {
        case: format!("every read granted and {} denied beside it", reads[0]),
        grant: Grant::DeniedBeside(reads[0]),
        polls: false,
    });
    rows
}

#[tokio::test]
async fn a_queue_mapping_polls_only_while_its_execution_role_may_read_the_queue() {
    for (index, row) in rows(&QUEUE_READS).into_iter().enumerate() {
        let case = row.case.as_str();
        let (router, mut driven) = driven_emulator_with_queues();
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("poll-auth-queue-{index}"),
            &json!({}),
        )
        .await;
        // Another queue of the same account, for the row whose grant names the wrong one.
        let (_, other_arn) = create_queue(
            &router,
            case,
            &format!("poll-auth-other-{index}"),
            &json!({}),
        )
        .await;
        // The pair is WIRED with every read granted, so what each row changes is the policy
        // in force while the mapping polls — not whether the mapping could be created.
        let role_name = format!("poll-auth-role-{index}");
        let role = given_role(
            &router,
            case,
            &role_name,
            Some(json!([logs_only(), allow("sqs", &QUEUE_READS, &arn)])),
        )
        .await;
        let function = format!("poll-auth-fn-{index}");
        create_function_as(&router, case, &function, &role).await;
        wait_until_active(&router, case, &function).await;
        let uuid = given_enabled_mapping(
            &router,
            case,
            &json!({ "FunctionName": function, "EventSourceArn": arn }),
        )
        .await;

        // Then the row's own grant, and only then a message to poll: the policies in force
        // when the message arrives are what the pass is subject to.
        write_policy(
            &router,
            case,
            &role_name,
            row.grant.statements("sqs", &QUEUE_READS, &arn, &other_arn),
        )
        .await;
        send_message(&router, case, &url, &format!("{case} payload")).await;

        if row.polls {
            // ONE pass, awaited: it reads the queue as the role, takes the batch, hands it to
            // the function and records what the attempt came to, all before it answers.
            driven.poll_once().await;
            let counts = queue_message_counts(&router, case, &url).await;
            assert_eq!(
                counts,
                (0, 1, 0),
                "case {case:?}: the mapping's role may make every read its poller makes, so \
                 the message must be taken out of the queue, got {counts:?} as (visible, in \
                 flight, delayed)"
            );
            let reading = get_mapping(&router, &uuid).await;
            assert_eq!(
                reading.string("LastProcessingResult").as_deref(),
                Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
                "case {case:?}: nothing here runs a deployment package, so the attempt the \
                 pass made on the batch it polled must be reported as the unsuccessful one \
                 it was, got {}",
                reading.text()
            );
            continue;
        }

        // The complement: a read the role may not make is one the pass does not make, so
        // the message is still the queue's own after every pass — and no attempt at a
        // batch is reported, because no batch was polled.
        for pass in 0..BLOCKED_PASSES {
            driven.poll_once().await;
            let counts = queue_message_counts(&router, case, &url).await;
            assert_eq!(
                counts,
                (1, 0, 0),
                "case {case:?}: the mapping's role may not make one of the reads its poller \
                 makes, so pass {pass} must leave the message visible on the queue, got \
                 {counts:?}"
            );
        }
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.string("State").as_deref(),
            Some(STATE_ENABLED),
            "case {case:?}: a mapping that may not read its source is still the enabled \
             mapping it was, got {}",
            reading.text()
        );
        assert!(
            reading.json().get("LastProcessingResult").is_none(),
            "case {case:?}: no batch was polled, so there is no processing attempt to \
             report, got {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn a_queue_backlog_a_blocked_mapping_left_is_polled_once_the_grant_is_back() {
    // The other side of the rule: the decision is made afresh on every pass, so a grant put
    // back resumes the polling — with everything the blocked passes left on the queue.
    let case = "three messages sent while the role may not receive, then the grant restored";
    let messages = 3;
    let (router, mut driven) = driven_emulator_with_queues();
    let (url, arn) = create_queue(&router, case, "poll-auth-resumes-queue", &json!({})).await;
    let role_name = "poll-auth-resumes-role";
    let role = given_role(
        &router,
        case,
        role_name,
        Some(json!([logs_only(), allow("sqs", &QUEUE_READS, &arn)])),
    )
    .await;
    create_function_as(&router, case, "poll-auth-resumes-fn", &role).await;
    wait_until_active(&router, case, "poll-auth-resumes-fn").await;
    given_enabled_mapping(
        &router,
        case,
        &json!({
            "FunctionName": "poll-auth-resumes-fn",
            "EventSourceArn": arn,
            "BatchSize": messages,
        }),
    )
    .await;

    // The grant goes away, and the backlog builds up behind the blocked passes.
    write_policy(&router, case, role_name, None).await;
    for index in 0..messages {
        send_message(&router, case, &url, &format!("{case}-{index}")).await;
    }
    for pass in 0..BLOCKED_PASSES {
        driven.poll_once().await;
        let counts = queue_message_counts(&router, case, &url).await;
        assert_eq!(
            counts,
            (u32::try_from(messages).unwrap_or(0), 0, 0),
            "case {case:?}: while the role may not receive, pass {pass} must leave every \
             message visible on the queue, got {counts:?}"
        );
    }

    // And back: the same role, granted the same reads again, polls the whole backlog — one
    // pass, since the mapping's `BatchSize` is the whole of it.
    write_policy(
        &router,
        case,
        role_name,
        Some(json!([logs_only(), allow("sqs", &QUEUE_READS, &arn)])),
    )
    .await;
    driven.poll_once().await;
    let counts = queue_message_counts(&router, case, &url).await;
    assert_eq!(
        counts,
        (0, u32::try_from(messages).unwrap_or(0), 0),
        "case {case:?}: once the grant is back the mapping must poll the backlog it left, and \
         the queue reports {counts:?} as (visible, in flight, delayed)"
    );
}

#[tokio::test]
async fn a_stream_mapping_reads_only_while_its_execution_role_may_read_the_stream() {
    // The same rule for the other kind of source, whose reads are three calls of its own
    // service's API rather than a queue's. What a pass came to is read off the mapping
    // itself: a pass that read a batch reports its attempt at it, and a pass that read
    // nothing has no attempt to report.
    for (index, row) in rows(&SHARD_READS).into_iter().enumerate() {
        let case = row.case.as_str();
        let router = emulator_with_streams();
        let stream = format!("poll-auth-stream-{index}");
        let arn = given_stream(&router, case, &stream, 1).await;
        let other_arn =
            given_stream(&router, case, &format!("poll-auth-other-{index}"), 1).await;
        let role_name = format!("poll-auth-stream-role-{index}");
        let role = given_role(
            &router,
            case,
            &role_name,
            Some(json!([logs_only(), allow("kinesis", &KINESIS_READS, &arn)])),
        )
        .await;
        let function = format!("poll-auth-stream-fn-{index}");
        create_function_as(&router, case, &function, &role).await;
        wait_until_active(&router, case, &function).await;
        let uuid = given_enabled_mapping(
            &router,
            case,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "StartingPosition": "TRIM_HORIZON",
            }),
        )
        .await;

        write_policy(
            &router,
            case,
            &role_name,
            row.grant
                .statements("kinesis", &SHARD_READS, &arn, &other_arn),
        )
        .await;
        put_records(&router, case, &stream, 1).await;

        if row.polls {
            let result = processing_result_within_budget(&router, case, &uuid).await;
            assert!(
                result.is_some(),
                "case {case:?}: the role may make every read consuming the stream takes, so \
                 the batch must be read and the attempt at it reported, got {result:?}"
            );
            continue;
        }
        let watch_until = Instant::now() + BLOCKED_WINDOW;
        while Instant::now() < watch_until {
            let reading = get_mapping(&router, &uuid).await;
            assert!(
                reading.json().get("LastProcessingResult").is_none(),
                "case {case:?}: a read the role may not make is one the pass does not make, \
                 so no batch is read and no attempt at one is reported, got {}",
                reading.text()
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
}
