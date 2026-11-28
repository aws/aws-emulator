// SPDX-License-Identifier: CC0-1.0

//! An Amazon SQS event source mapping under its OWN running poller: which queue it consumes,
//! and who makes sure the poller the mapping's reported `State` implies is running at all.
//!
//! Two rules, both about the same claim — that a mapping a client is told is `Enabled` is one
//! that READS from its source and hands what it reads to its function ("when true, the event
//! source mapping is active. When false, Lambda pauses polling and invocation",
//! `smithy_query(query="operation", operation="CreateEventSourceMapping")`):
//!
//! 1. **WHICH source.** A mapping reads the one event source its `EventSourceArn` names — of
//!    which "Amazon Simple Queue Service – The ARN of the queue" is this suite's kind — so the
//!    message sent to that queue reaches the mapped function on its own, while a message sent
//!    to any other queue of the account is never delivered and stays waiting where it was sent.
//!    What this adds to `tests/all/event_source_mapping_source_selection.rs`, which pins the
//!    same selection pass by pass, is the DRIVER: the passes here are the mapping's own
//!    background poller, so what is asserted is that the mapping polls with nobody driving it.
//!
//! 2. **WHO ARMS IT.** The reading is done by a background task per mapping, and a mapping that
//!    has none is a mapping nothing polls whatever its `State` says — so every operation that
//!    REPORTS a mapping arms the pollers this store's mappings ask for
//!    (`crate::state::arm_mapping_pollers`), and a mapping left reporting `Enabled` with no
//!    task consumes its queue again from the next read of it onwards. The rows span both
//!    readers — the read of ONE mapping and the LISTING of them — and the other side of the
//!    rule: a mapping reporting `Disabled` is armed by neither, so nothing is delivered for it.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with
//! Lambda, Amazon SQS and (for the role the poller reads the source as) IAM on ONE engine, as
//! `core/router` assembles them. The execution role's grants are the policies AWS PUBLISHES
//! for a queue-triggered function, attached by ARN through IAM's own `AttachRolePolicy`, and
//! the function is a real `python3.12` .zip whose handler forwards each record it is handed
//! with the SDK its managed runtime supplies — so a body read back here came through the
//! poller's reads as that role and a handler that ran.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    service_state, EVENT_SOURCE_MAPPING_STATE_ENABLED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{given_role, iam};
use crate::event_source_mappings::{
    await_mapping_state, create_mapping, create_queue, create_running_function_as, get_mapping,
    list_mappings, queue_attribute, router_on, send_message, taken_bodies, FORWARDING_HANDLER,
    LOOK_AGAIN, RESULTS_QUEUE_URL, STATE_DISABLED, STATE_ENABLED, VISIBLE_MESSAGES,
};

/// The policy AWS publishes for the execution role of a queue-triggered function: the three
/// calls Lambda makes to consume a queue on the function's behalf, beside the log writes.
const QUEUE_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaSQSQueueExecutionRole";

/// The published policy covering the ONE send the handler itself makes, which no event
/// source's execution-role policy grants. AWS files it at the account root rather than under
/// the service-role path.
const HANDLER_SEND_POLICY: &str = "arn:aws:iam::aws:policy/AmazonSQSFullAccess";

/// The function's `Timeout` in seconds, below the default `VisibilityTimeout` of every queue
/// below, which is the pairing `CreateEventSourceMapping` accepts.
const FUNCTION_TIMEOUT: i64 = 10;

/// How long a wait on a CONDITION may take before it is a failure. Generous against the span
/// one delivery takes, so a loaded host delays a row instead of weakening it.
const BUDGET: Duration = Duration::from_secs(60);

/// How many looks an absence leg takes: several of the poller's own intervals
/// (`crate::state::POLL_INTERVAL`), so a delivery the rule forbids had every chance to be made.
const QUIET_LOOKS: usize = 20;

/// One queue of a selection row: the suffix its name is built from and the body sent to it.
struct Queue {
    label: &'static str,
    body: &'static str,
}

/// One selection row: the queues that exist beside the results queue, which of them the
/// mapping names, and the `BatchSize` it reads that one under.
struct Row {
    case: &'static str,
    queues: &'static [Queue],
    /// The index in `queues` of the queue the mapping's `EventSourceArn` names.
    mapped: usize,
    /// "The maximum number of records in each batch that Lambda pulls from your stream or
    /// queue" (`CreateEventSourceMapping$BatchSize`).
    batch_size: i64,
}

/// The selection catalog: the mapped queue last of two and first of three, under one record
/// per batch and under a ceiling above everything waiting.
const ROWS: [Row; 2] = [
    Row {
        case: "two queues, the second mapped, one record per batch",
        queues: &[
            Queue {
                label: "unnamed",
                body: "sent-to-a-queue-the-mapping-does-not-name",
            },
            Queue {
                label: "named",
                body: "sent-to-the-mapped-queue",
            },
        ],
        mapped: 1,
        batch_size: 1,
    },
    Row {
        case: "three queues, the first mapped, a ceiling above what waits",
        queues: &[
            Queue {
                label: "named",
                body: "sent-to-the-queue-the-mapping-names",
            },
            Queue {
                label: "unnamed-one",
                body: "sent-to-another-queue",
            },
            Queue {
                label: "unnamed-two",
                body: "sent-to-a-third-queue",
            },
        ],
        mapped: 0,
        batch_size: 10,
    },
];

/// Lambda over the SAME engine as Amazon SQS and IAM: the mapping resolves its role in the
/// service that owns roles and its queue in the service that owns queues, exactly what
/// `core/router` assembles.
fn router_with_queues_and_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
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
    (router, engine)
}

/// The consumer of a row: a role carrying the two PUBLISHED policies — the source's own reads
/// and the handler's own send — and a running function forwarding to `results_url`.
async fn given_consumer(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
    results_url: &str,
) -> String {
    let role_name = format!("live-poller-role-{label}");
    let role_arn = given_role(router, case, &role_name, None).await;
    for policy in [QUEUE_EXECUTION_POLICY, HANDLER_SEND_POLICY] {
        let (status, attached) = iam(
            router,
            "AttachRolePolicy",
            &[("RoleName", &role_name), ("PolicyArn", policy)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: AWS publishes {policy}, so attaching it must be served, got \
             {status}: {attached}"
        );
    }
    let function = format!("live-poller-fn-{label}");
    create_running_function_as(
        router,
        case,
        &function,
        &role_arn,
        FORWARDING_HANDLER,
        FUNCTION_TIMEOUT,
        &json!({ RESULTS_QUEUE_URL: results_url }),
    )
    .await;
    function
}

/// Every body the handler has forwarded to `results_url` so far, looked for until `wanted` is
/// among them or [`BUDGET`] is out; what it did forward, either way.
async fn forwarded_until(
    router: &MultiServiceRouter,
    case: &str,
    results_url: &str,
    wanted: &str,
) -> BTreeSet<String> {
    let deadline = Instant::now() + BUDGET;
    let mut forwarded: BTreeSet<String> = BTreeSet::new();
    loop {
        forwarded.extend(taken_bodies(router, case, results_url).await);
        if forwarded.contains(wanted) || Instant::now() >= deadline {
            return forwarded;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every body forwarded to `results_url` over [`QUIET_LOOKS`] looks, for a leg that asserts
/// nothing is.
async fn forwarded_while_quiet(
    router: &MultiServiceRouter,
    case: &str,
    results_url: &str,
) -> BTreeSet<String> {
    let mut forwarded: BTreeSet<String> = BTreeSet::new();
    for _ in 0..QUIET_LOOKS {
        forwarded.extend(taken_bodies(router, case, results_url).await);
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    forwarded
}

/// RULE 1: a mapping left to its own poller consumes the one queue its `EventSourceArn` names
/// — its message reaches the running handler — and never a message of another queue, which
/// stays waiting on the queue it was sent to.
#[tokio::test]
async fn a_mapping_left_to_its_own_poller_consumes_only_the_queue_it_names() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let (router, _engine) = router_with_queues_and_iam();

        // The queue the handler forwards to, which no mapping names: where the records that
        // reached the function's own run are read back.
        let (results_url, _results_arn) = create_queue(
            &router,
            case,
            &format!("live-poller-{index}-results"),
            &json!({}),
        )
        .await;
        let mut queues: Vec<(String, String)> = Vec::new();
        for queue in row.queues {
            queues.push(
                create_queue(
                    &router,
                    case,
                    &format!("live-poller-{index}-{}", queue.label),
                    &json!({}),
                )
                .await,
            );
        }
        let (mapped_url, mapped_arn) = queues[row.mapped].clone();
        let function = given_consumer(&router, case, &index.to_string(), &results_url).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": mapped_arn,
                "BatchSize": row.batch_size,
                "Enabled": true,
            }),
        )
        .await;
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
        await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

        // What is waiting: every queue the mapping does not name first, so a message it must
        // leave alone has been waiting for longer than the one it must deliver.
        for (position, (queue, (url, _))) in row.queues.iter().zip(&queues).enumerate() {
            if position == row.mapped {
                continue;
            }
            send_message(&router, case, url, queue.body, None).await;
        }
        let mapped_body = row.queues[row.mapped].body;
        send_message(&router, case, &mapped_url, mapped_body, None).await;

        // The positive control, which comes first: nothing drives the mapping, so a body on
        // the results queue is one its own poller delivered to a handler that ran.
        let forwarded = forwarded_until(&router, case, &results_url, mapped_body).await;
        assert!(
            forwarded.contains(mapped_body),
            "case {case:?}: the mapping must hand its function the message its own queue was \
             sent, so {mapped_body:?} must be forwarded; the handler forwarded {forwarded:?} \
             and the mapping reads {}",
            get_mapping(&router, &uuid).await.text()
        );

        // And the absence, measured against that same pipeline: no body of a queue the
        // mapping does not name is ever forwarded, and every such queue still holds its own.
        let quiet = forwarded_while_quiet(&router, case, &results_url).await;
        for (position, (queue, (url, _))) in row.queues.iter().zip(&queues).enumerate() {
            if position == row.mapped {
                continue;
            }
            assert!(
                !forwarded.contains(queue.body) && !quiet.contains(queue.body),
                "case {case:?}: a mapping reads only the queue it names, so the body of the \
                 queue {:?} may never be forwarded; the handler forwarded {forwarded:?} and \
                 then {quiet:?}",
                queue.label
            );
            let visible = queue_attribute(&router, case, url, VISIBLE_MESSAGES).await;
            assert_eq!(
                visible.as_deref(),
                Some("1"),
                "case {case:?}: the queue {:?} is not the mapping's event source, so its \
                 message must still be waiting on it, got {visible:?}",
                queue.label
            );
        }
    }
}

/// One row of the supervision table: how the mapping is REPORTED, and whether the mapping's
/// own setting asks to be polled at the instant it is reported.
struct Reported {
    case: &'static str,
    label: &'static str,
    /// Whether the mapping is left asking to be polled — `Enabled`, which is a mapping whose
    /// poller has to be running — or paused, which is a mapping nothing may poll.
    polling: bool,
    /// Whether the mapping is reported by the LISTING rather than by the read of one mapping.
    listed: bool,
}

/// The supervision catalog: both readers of a mapping report an `Enabled` one, and the read
/// of one mapping reports a paused one — which is the leg that must deliver nothing.
const REPORTED: [Reported; 3] = [
    Reported {
        case: "an enabled mapping with no poller, reported by GetEventSourceMapping",
        label: "get",
        polling: true,
        listed: false,
    },
    Reported {
        case: "an enabled mapping with no poller, reported by ListEventSourceMappings",
        label: "list",
        polling: true,
        listed: true,
    },
    Reported {
        case: "a paused mapping, reported by GetEventSourceMapping",
        label: "paused",
        polling: false,
        listed: false,
    },
];

/// RULE 2: the operations that REPORT a mapping make sure of the poller its state implies, so
/// a mapping left reporting `Enabled` with no poller consumes its queue from the next read
/// onwards — and a mapping reporting `Disabled` is armed by neither reader.
///
/// The mapping is made `Enabled=false`, which arms no poller at all, and the store is then
/// left in the state the symptom describes: a mapping whose own setting asks to be polled with
/// no task reading its source. That is the state a mapping whose poller ended for any other
/// reason is in, and nothing but a client's own call can be in a position to notice it.
#[tokio::test]
async fn reporting_a_mapping_makes_sure_of_the_poller_its_state_implies() {
    for row in &REPORTED {
        let case = row.case;
        let (router, engine) = router_with_queues_and_iam();

        let (results_url, _results_arn) = create_queue(
            &router,
            case,
            &format!("supervision-{}-results", row.label),
            &json!({}),
        )
        .await;
        let (source_url, source_arn) = create_queue(
            &router,
            case,
            &format!("supervision-{}-source", row.label),
            &json!({}),
        )
        .await;
        let function = given_consumer(&router, case, row.label, &results_url).await;

        // A mapping asked to be PAUSED arms no poller, which is how this row reaches the
        // state it is about without reaching inside the poller.
        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": source_arn,
                "BatchSize": 1,
                "Enabled": false,
            }),
        )
        .await;
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
        await_mapping_state(&router, case, &uuid, STATE_DISABLED).await;

        // The state the symptom describes: the mapping's own setting asks to be polled and
        // its `State` says so, while no task is reading its source.
        if row.polling {
            let state = service_state(&*engine)
                .unwrap_or_else(|| panic!("case {case:?}: the Lambda store must be reachable"));
            let mut store = state.lock();
            let record = store
                .event_source_mappings
                .get_mut(&uuid)
                .unwrap_or_else(|| panic!("case {case:?}: the mapping must be stored"));
            record.enabled = true;
            record.state = EVENT_SOURCE_MAPPING_STATE_ENABLED.to_string();
        }

        let body = format!("delivered-after-the-{}-read", row.label);
        send_message(&router, case, &source_url, &body, None).await;

        // Nothing polls it yet, whatever it reports, so nothing of its queue reaches the
        // function: this is the inert trigger the rule is about.
        let inert = forwarded_while_quiet(&router, case, &results_url).await;
        assert!(
            inert.is_empty(),
            "case {case:?}: a mapping with no poller delivers nothing, got {inert:?}"
        );

        // The read a client makes: it reports the mapping, and the mapping it reports as
        // polling is one whose poller is running from here on.
        let reading = if row.listed {
            list_mappings(&router, &format!("?FunctionName={function}")).await
        } else {
            get_mapping(&router, &uuid).await
        };
        assert_eq!(
            reading.status,
            200,
            "case {case:?}: the read must answer 200, got {} {}",
            reading.status,
            reading.text()
        );

        if row.polling {
            let forwarded = forwarded_until(&router, case, &results_url, &body).await;
            assert!(
                forwarded.contains(&body),
                "case {case:?}: the mapping reported as polling must consume its queue from \
                 that read onwards, so {body:?} must be forwarded; the handler forwarded \
                 {forwarded:?} and the mapping reads {}",
                get_mapping(&router, &uuid).await.text()
            );
        } else {
            let still_quiet = forwarded_while_quiet(&router, case, &results_url).await;
            assert!(
                still_quiet.is_empty(),
                "case {case:?}: a paused mapping is armed by no read, so nothing of its queue \
                 may be delivered, got {still_quiet:?}"
            );
            let visible = queue_attribute(&router, case, &source_url, VISIBLE_MESSAGES).await;
            assert_eq!(
                visible.as_deref(),
                Some("1"),
                "case {case:?}: the message sent to a paused mapping's queue must still be \
                 waiting on it, got {visible:?}"
            );
        }
    }
}
