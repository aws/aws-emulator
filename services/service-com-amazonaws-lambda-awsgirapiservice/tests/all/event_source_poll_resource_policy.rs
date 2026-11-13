// SPDX-License-Identifier: CC0-1.0

//! What an ENABLED event source mapping may take out of its event source while the policy
//! the SOURCE ITSELF carries allows — or denies — the reads that mapping's poller makes.
//!
//! RULE: a mapping's poller reads the event source ACTING AS the invoked function's
//! execution role — "Lambda reads items from the event source and invokes the function"
//! (`CreateEventSourceMapping`) as "the ARN of the function's execution role"
//! (`FunctionConfiguration$Role`) — so each of those reads is a call on another service's
//! resource and is decided by the RESOURCE-based policy that resource carries beside the
//! identity-based documents in force on the role, in IAM's own order: an explicit `Deny`
//! about one of the reads — in the source's own document or in the role's — withholds it
//! whatever the other grants, and a grant in either is enough within one account, because
//! "the resource-based policy and the identity-based policy are evaluated together, and
//! access is granted if either allows it". A pass whose read is withheld takes nothing out of
//! the source: nothing is handed over, invoked or deleted, and the source keeps everything it
//! holds until the document in force says otherwise.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! and the source's own service mounted beside Lambda on ONE engine: the role is written
//! through IAM's own `CreateRole`/`PutRolePolicy`, the source's policy through the owning
//! service's own API (`SetQueueAttributes` for a queue's `Policy` attribute,
//! `PutResourcePolicy` for a data stream's), each message is sent and counted through Amazon
//! SQS's own API and each record is put through Kinesis's own — so a pass that read what the
//! source's own document denies fails here.
//!
//! The rows differ along what the rule generalises over: WHICH half of the decision states
//! something (the source's document alone, the role's alone, both), WHAT the source's
//! document states (a deny of one of the poller's reads, a deny of every action of the
//! service, a deny of an action the poller never makes, a grant of every read), WHO it names
//! (the role itself, the role's whole account, another principal of the account), and WHICH
//! kind of source is polled (a queue, whose reads are its own API's calls, and a data stream,
//! which is read with three calls of its service's).

use std::time::Instant;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{
    allow, create_function_as, given_role, logs_only, QUEUE_READS,
};
use crate::event_source_mappings::{
    call_sqs, create_queue, get_mapping, processing_result_within_budget, queue_message_counts,
    send_message,
};
use crate::event_source_poll_authorization::{
    driven_emulator_with_queues, emulator_with_streams, given_enabled_mapping, wait_until_active,
    BLOCKED_PASSES, BLOCKED_WINDOW, LOOK_AGAIN, SHARD_READS, STATE_ENABLED,
};
use crate::kinesis_event_source_delivery::{given_stream, kinesis, put_records};

/// Which kind of event source a row polls. The two differ in the calls consuming them takes
/// and in the API their own policy is written through; everything else about the rule is one
/// rule.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// An Amazon SQS queue, whose resource policy is its `Policy` attribute.
    Queue,
    /// An Amazon Kinesis data stream, whose resource policy is attached with
    /// `PutResourcePolicy`.
    Stream,
}

impl Kind {
    /// The IAM action namespace the source service's actions are named in, which is also the
    /// vendor field of its ARNs.
    fn namespace(self) -> &'static str {
        match self {
            Self::Queue => "sqs",
            Self::Stream => "kinesis",
        }
    }

    /// The reads a mapping onto this kind of source is decided on.
    fn reads(self) -> &'static [&'static str] {
        match self {
            Self::Queue => &QUEUE_READS,
            Self::Stream => &SHARD_READS,
        }
    }

    /// One action of the source service the poller never makes, so a statement about it is
    /// about none of the reads this decision is made on. Source: the WRITE half of each
    /// service's own API (`SendMessage`, `PutRecord`), which no consumer of the source
    /// calls.
    fn unrelated_action(self) -> &'static str {
        match self {
            Self::Queue => "SendMessage",
            Self::Stream => "PutRecord",
        }
    }
}

/// What the SOURCE's own policy states while the records arrive.
#[derive(Clone, Copy, Debug)]
enum SourcePolicy {
    /// Nothing at all: the source carries no resource-based policy.
    None,
    /// An explicit deny of ONE of the poller's reads, naming the execution role.
    DenyReadToRole(usize),
    /// An explicit deny of every action of the source service, naming the role.
    DenyEveryActionToRole,
    /// An explicit deny of one of the poller's reads, naming the role's whole ACCOUNT —
    /// which denies each of that account's principals.
    DenyReadToAccount(usize),
    /// An explicit deny of an action the poller never makes, naming the role.
    DenyUnrelatedActionToRole,
    /// An explicit deny of one of the poller's reads, naming ANOTHER principal of the
    /// account.
    DenyReadToAnotherPrincipal(usize),
    /// A grant of every read the poller makes, naming the role.
    AllowEveryReadToRole,
}

impl SourcePolicy {
    /// The document this row attaches to the source, or `None` when it attaches none.
    ///
    /// `other_principal` is another principal of the same account, for the row whose
    /// statement names the wrong one.
    fn document(
        self,
        kind: Kind,
        role_arn: &str,
        source_arn: &str,
        other_principal: &str,
    ) -> Option<Value> {
        let namespace = kind.namespace();
        let reads = kind.reads();
        let statement = |effect: &str, actions: Vec<String>, principal: &str| {
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": effect,
                    "Principal": { "AWS": principal },
                    "Action": actions,
                    "Resource": source_arn,
                }],
            }))
        };
        let named = |read: usize| vec![format!("{namespace}:{}", reads[read])];
        match self {
            Self::None => None,
            Self::DenyReadToRole(read) => statement("Deny", named(read), role_arn),
            Self::DenyEveryActionToRole => {
                statement("Deny", vec![format!("{namespace}:*")], role_arn)
            }
            Self::DenyReadToAccount(read) => {
                statement("Deny", named(read), &account_root_of(role_arn))
            }
            Self::DenyUnrelatedActionToRole => statement(
                "Deny",
                vec![format!("{namespace}:{}", kind.unrelated_action())],
                role_arn,
            ),
            Self::DenyReadToAnotherPrincipal(read) => {
                statement("Deny", named(read), other_principal)
            }
            Self::AllowEveryReadToRole => statement(
                "Allow",
                reads
                    .iter()
                    .map(|read| format!("{namespace}:{read}"))
                    .collect(),
                role_arn,
            ),
        }
    }
}

/// The root-user ARN of the account `arn` belongs to, which is one of the two spellings a
/// statement names that whole ACCOUNT by.
fn account_root_of(arn: &str) -> String {
    let account = arn
        .split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("the ARN {arn:?} carries no account"));
    format!("arn:aws:iam::{account}:root")
}

/// One row: what the source's own policy states, and whether the mapping may then read it.
struct Row {
    case: &'static str,
    policy: SourcePolicy,
    /// Whether the pass must take what the source holds.
    polls: bool,
}

/// The catalog, for either kind of source. Every row's ROLE is granted every read on the
/// source, so what each row changes is the document the SOURCE carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no policy on the source at all",
            policy: SourcePolicy::None,
            polls: true,
        },
        Row {
            case: "the source denies the first read the poller makes to the role",
            policy: SourcePolicy::DenyReadToRole(0),
            polls: false,
        },
        Row {
            case: "the source denies the last read the poller makes to the role",
            policy: SourcePolicy::DenyReadToRole(2),
            polls: false,
        },
        Row {
            case: "the source denies every action of its service to the role",
            policy: SourcePolicy::DenyEveryActionToRole,
            polls: false,
        },
        Row {
            case: "the source denies a read to the role's whole account",
            policy: SourcePolicy::DenyReadToAccount(0),
            polls: false,
        },
        Row {
            case: "the source denies an action the poller never makes",
            policy: SourcePolicy::DenyUnrelatedActionToRole,
            polls: true,
        },
        Row {
            case: "the source denies a read to another principal of the account",
            policy: SourcePolicy::DenyReadToAnotherPrincipal(0),
            polls: true,
        },
        Row {
            case: "the source grants every read the poller makes to the role",
            policy: SourcePolicy::AllowEveryReadToRole,
            polls: true,
        },
    ]
}

/// Attach `document` to the queue `url` addresses as its `Policy` attribute, through Amazon
/// SQS's own API — or, for a row that attaches none, leave the queue as it was.
async fn set_queue_policy(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    document: Option<Value>,
) {
    let Some(document) = document else {
        return;
    };
    let (status, answered) = call_sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": document.to_string() } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the row's policy must be written through Amazon SQS, got {status} \
         {answered}"
    );
}

/// The same for a data stream, whose policy is attached with `PutResourcePolicy`.
async fn set_stream_policy(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    document: Option<Value>,
) {
    let Some(document) = document else {
        return;
    };
    let (status, answered) = kinesis(
        router,
        "PutResourcePolicy",
        &json!({ "ResourceARN": arn, "Policy": document.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the row's policy must be written through Amazon Kinesis, got {status} \
         {answered}"
    );
}

#[tokio::test]
async fn a_queue_mapping_polls_only_while_the_queue_s_own_policy_lets_the_poller_read_it() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // The poll passes are this row's own and are awaited, so what the row reads is what
        // the pass it just made did — see [`driven_emulator_with_queues`].
        let (router, mut driven) = driven_emulator_with_queues();
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("source-policy-queue-{index}"),
            &json!({}),
        )
        .await;
        // The role is granted every read on the queue, so the row's own document is the only
        // thing that can withhold one. The second role is the principal the row that names
        // the wrong one names.
        let role_name = format!("source-policy-role-{index}");
        let role = given_role(
            &router,
            case,
            &role_name,
            Some(json!([logs_only(), allow("sqs", &QUEUE_READS, &arn)])),
        )
        .await;
        let other = given_role(
            &router,
            case,
            &format!("source-policy-other-{index}"),
            None,
        )
        .await;
        let function = format!("source-policy-fn-{index}");
        create_function_as(&router, case, &function, &role).await;
        wait_until_active(&router, case, &function).await;
        let uuid = given_enabled_mapping(
            &router,
            case,
            &json!({ "FunctionName": function, "EventSourceArn": arn }),
        )
        .await;

        // Then the row's document, and only then a message to poll: what is in force when the
        // message arrives is what the pass is subject to.
        set_queue_policy(
            &router,
            case,
            &url,
            row.policy.document(Kind::Queue, &role, &arn, &other),
        )
        .await;
        send_message(&router, case, &url, &format!("{case} payload"), None).await;

        if row.polls {
            driven.poll_once().await;
            let counts = queue_message_counts(&router, case, &url).await;
            assert_eq!(
                counts,
                (0, 1, 0),
                "case {case:?}: nothing in force withholds a read the poller makes, so the \
                 message must be taken out of the queue, got {counts:?} as (visible, in \
                 flight, delayed)"
            );
            continue;
        }
        // The complement: a read the queue's own document denies is one the pass does not
        // make, so the message is still the queue's own after every pass — and no attempt
        // at a batch is reported, because no batch was polled.
        for pass in 0..BLOCKED_PASSES {
            driven.poll_once().await;
            let counts = queue_message_counts(&router, case, &url).await;
            assert_eq!(
                counts,
                (1, 0, 0),
                "case {case:?}: the queue's own policy denies one of the reads its poller \
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
            "case {case:?}: no batch was polled, so there is no processing attempt to report, \
             got {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn a_stream_mapping_reads_only_while_the_stream_s_own_policy_lets_the_poller_read_it() {
    // The same rule for the other kind of source, whose reads are three calls of its own
    // service's API rather than a queue's. What a pass came to is read off the mapping
    // itself: a pass that read a batch reports its attempt at it, and a pass that read
    // nothing has no attempt to report.
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let router = emulator_with_streams();
        let stream = format!("source-policy-stream-{index}");
        let arn = given_stream(&router, case, &stream, 1).await;
        let role_name = format!("source-policy-stream-role-{index}");
        let role = given_role(
            &router,
            case,
            &role_name,
            Some(json!([logs_only(), allow("kinesis", &SHARD_READS, &arn)])),
        )
        .await;
        let other = given_role(
            &router,
            case,
            &format!("source-policy-stream-other-{index}"),
            None,
        )
        .await;
        let function = format!("source-policy-stream-fn-{index}");
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

        set_stream_policy(
            &router,
            case,
            &arn,
            row.policy.document(Kind::Stream, &role, &arn, &other),
        )
        .await;
        put_records(&router, case, &stream, 1).await;

        if row.polls {
            let result = processing_result_within_budget(&router, case, &uuid).await;
            assert!(
                result.is_some(),
                "case {case:?}: nothing in force withholds a read the poller makes, so the \
                 batch must be read and the attempt at it reported, got {result:?}"
            );
            continue;
        }
        let watch_until = Instant::now() + BLOCKED_WINDOW;
        while Instant::now() < watch_until {
            let reading = get_mapping(&router, &uuid).await;
            assert!(
                reading.json().get("LastProcessingResult").is_none(),
                "case {case:?}: a read the stream's own policy denies is one the pass does \
                 not make, so no batch is read and no attempt at one is reported, got {}",
                reading.text()
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
}

#[tokio::test]
async fn a_queue_backlog_left_while_the_queue_denied_the_read_is_polled_once_the_deny_is_cleared() {
    // The other side of the rule: the documents in force are read afresh on every pass, so a
    // deny the queue's own policy stated withholds the reads only while it is there — the
    // backlog it left is polled once it is cleared, which is what "the message stays on the
    // queue" means.
    let case = "two messages sent while the queue's policy denies the poller's receive";
    let messages = 2;
    let (router, mut driven) = driven_emulator_with_queues();
    let (url, arn) = create_queue(&router, case, "source-policy-resumes-queue", &json!({})).await;
    let role = given_role(
        &router,
        case,
        "source-policy-resumes-role",
        Some(json!([logs_only(), allow("sqs", &QUEUE_READS, &arn)])),
    )
    .await;
    create_function_as(&router, case, "source-policy-resumes-fn", &role).await;
    wait_until_active(&router, case, "source-policy-resumes-fn").await;
    given_enabled_mapping(
        &router,
        case,
        &json!({
            "FunctionName": "source-policy-resumes-fn",
            "EventSourceArn": arn,
            "BatchSize": messages,
        }),
    )
    .await;

    set_queue_policy(
        &router,
        case,
        &url,
        SourcePolicy::DenyReadToRole(0).document(Kind::Queue, &role, &arn, &role),
    )
    .await;
    for index in 0..messages {
        send_message(&router, case, &url, &format!("{case}-{index}"), None).await;
    }
    for pass in 0..BLOCKED_PASSES {
        driven.poll_once().await;
        let counts = queue_message_counts(&router, case, &url).await;
        assert_eq!(
            counts,
            (u32::try_from(messages).unwrap_or(0), 0, 0),
            "case {case:?}: while the queue's own policy denies the receive, pass {pass} must \
             leave every message visible on the queue, got {counts:?}"
        );
    }

    // And cleared: "you can remove the Policy attribute by setting it to an empty string",
    // which leaves the queue carrying no resource-based statement at all — so the role's own
    // grant decides again and the whole backlog is polled.
    let (status, cleared) = call_sqs(
        &router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": "" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: clearing the queue's policy must answer 200, got {status} {cleared}"
    );
    driven.poll_once().await;
    let counts = queue_message_counts(&router, case, &url).await;
    assert_eq!(
        counts,
        (0, u32::try_from(messages).unwrap_or(0), 0),
        "case {case:?}: once the deny is cleared the mapping must poll the backlog it left, and \
         the queue reports {counts:?} as (visible, in flight, delayed)"
    );
}

#[tokio::test]
async fn a_queue_s_own_grant_is_enough_for_a_role_whose_documents_grant_no_read() {
    // The GRANT half of the rule, which is the same rule read the other way: within one
    // account an `Allow` in either document is enough, so a role whose identity-based
    // policies say nothing about the queue still polls it while the QUEUE's own policy names
    // that role — and without that grant the pair is not even wired, which is what pins the
    // grant to the queue's document rather than to anything the role carries.
    let denied = "a role granted no read of the queue, and no policy on the queue";
    let granted = "a role granted no read of the queue, which the queue's own policy grants";
    let (router, mut driven) = driven_emulator_with_queues();
    let (url, arn) = create_queue(&router, denied, "source-policy-grant-queue", &json!({})).await;
    // The role carries the log writes every execution role carries and nothing else: it is
    // granted no read of the queue at all.
    let role = given_role(
        &router,
        denied,
        "source-policy-grant-role",
        Some(json!([logs_only()])),
    )
    .await;
    create_function_as(&router, denied, "source-policy-grant-fn", &role).await;
    wait_until_active(&router, denied, "source-policy-grant-fn").await;

    let refused = crate::event_source_mappings::create_mapping(
        &router,
        &json!({ "FunctionName": "source-policy-grant-fn", "EventSourceArn": arn }),
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "case {denied:?}: nothing grants the role a read of the queue, so the pair cannot be \
         wired, got {} {}",
        refused.status,
        refused.text()
    );

    // The queue's own document names the role, and the same request is served.
    set_queue_policy(
        &router,
        granted,
        &url,
        SourcePolicy::AllowEveryReadToRole.document(Kind::Queue, &role, &arn, &role),
    )
    .await;
    given_enabled_mapping(
        &router,
        granted,
        &json!({ "FunctionName": "source-policy-grant-fn", "EventSourceArn": arn }),
    )
    .await;
    send_message(&router, granted, &url, "granted by the queue itself", None).await;
    driven.poll_once().await;
    let counts = queue_message_counts(&router, granted, &url).await;
    assert_eq!(
        counts,
        (0, 1, 0),
        "case {granted:?}: the queue's own policy grants the role every read its poller makes, \
         so the message must be taken out of the queue, got {counts:?} as (visible, in flight, \
         delayed)"
    );
}
