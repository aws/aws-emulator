// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how long the batch an event source mapping is PROCESSING stays out
//! of its queue: `CreateEventSourceMapping` "creates a mapping between an event source and
//! an Lambda Event Function. Lambda reads items from the event source and invokes the
//! function."
//!
//! The rule under test, in one sentence: a poller that takes a batch out of a queue to hand
//! it to a function keeps that batch invisible to every other consumer for as long as the
//! processing attempt it was handed to may last — the queue's own `VisibilityTimeout` or the
//! function's `Timeout`, whichever is the longer, since one attempt cannot outlast "the
//! amount of time that Lambda allows a function to run before stopping it" — so no message
//! is handed over a second time while the first attempt at it may still be running. The hold
//! is that span and no more: once no attempt can still be running, the message is the
//! queue's to hand over again, which the second test below pins by pausing the mapping and
//! watching the message come back.
//!
//! That is what the comparison a queue mapping is ACCEPTED against assumes (a queue whose
//! `VisibilityTimeout` is shorter than the function's `Timeout` is refused); the hold is
//! what keeps it true afterwards, because `SetQueueAttributes` can shorten a queue's
//! visibility timeout at any time and Amazon SQS knows nothing about the mappings that poll
//! it. Every row therefore states the pair the mapping is created on and then sets the
//! queue's timeout to the value under test, exactly as a client would.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with BOTH
//! services on ONE engine — the queue is created, re-configured and read through Amazon SQS
//! exactly as a client does it, and the mapping through Lambda — so a hold applied to
//! anything other than the stored queue fails here. The rows differ along what the rule
//! generalises over: which of the two spans is the longer one, how long each is (including a
//! queue that hides a delivery for no time at all), how many messages are waiting and how
//! many of them one batch may hold.
//!
//! The POLL PASSES are this suite's own ([`crate::event_source_mappings::DrivenPolling`]):
//! each row takes the store's poll claim and then makes one pass per batch, awaiting it. So
//! what a row reads is the state the pass it just made left behind, rather than whatever the
//! background loop had got to by the instant the row happened to look — and the HOLD itself
//! is read as the instant the queue recorded it until, which is the decision the rule is
//! about, instead of being inferred from message counts watched for a span of wall clock. No
//! row sleeps, and nothing any row asserts depends on how fast the host runs: handing a batch
//! over goes through `Invoke`, whose wait for this process's run capacity is not a span this
//! suite chooses, and every row simply waits for it.
//!
//! Nothing here runs a deployment package, so no batch is ever processed: that is the
//! invocation this emulator refuses, and it is why these rows assert where the messages ARE
//! rather than what a function returned.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, call, create_mapping, create_queue, create_timed_function,
    driven_router_with_queues, get_mapping, held_until, queue_attribute, send_message,
    set_visibility_timeout, CLOCK_BUDGET, LOOK_AGAIN, MAPPINGS_URI,
};

/// The `State` a mapping reports once it is polling its event source.
const STATE_ENABLED: &str = "Enabled";

/// The `State` a mapping reports when its `Enabled` setting is false: "when false, Lambda
/// pauses polling and invocation."
const STATE_DISABLED: &str = "Disabled";

/// The `(visible, in flight)` message counts the queue `url` addresses reports.
async fn depths(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> (Option<String>, Option<String>) {
    let visible = queue_attribute(router, case, url, "ApproximateNumberOfMessages").await;
    let in_flight =
        queue_attribute(router, case, url, "ApproximateNumberOfMessagesNotVisible").await;
    (visible, in_flight)
}

/// Which of the messages the queue `arn` names HOLDS it is hiding at `at`, and until when.
///
/// The queue's own bookkeeping, read through
/// [`crate::event_source_mappings::held_until`]: a message with no recorded instant, or one
/// already past, is the queue's to hand over again.
fn hidden(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    arn: &str,
    at: SystemTime,
) -> BTreeMap<String, SystemTime> {
    held_until(engine, arn)
        .into_iter()
        .filter_map(|(body, until)| {
            until
                .filter(|until| *until > at)
                .map(|until| (body, until))
        })
        .collect()
}

/// One row: the queue's `VisibilityTimeout` while it is being polled, the mapped function's
/// `Timeout`, how many messages are waiting and how many one batch may hold.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    function_timeout: i64,
    messages: usize,
    batch_size: usize,
}

impl Row {
    /// The span the batch must stay in flight for: the longer of the two, which is the rule
    /// under test.
    fn hold(&self) -> i64 {
        self.visibility_timeout.max(self.function_timeout)
    }

    /// That span as a duration, which is what a recorded hold is measured against.
    fn hold_span(&self) -> Duration {
        Duration::from_secs(u64::try_from(self.hold()).unwrap_or(0))
    }

    /// How many passes it takes to get every waiting message out of the queue: one batch per
    /// pass, each at most `BatchSize` messages, so a row whose ceiling is below what is
    /// waiting takes several — and no row takes more than that, which is the ceiling itself
    /// being honoured.
    fn passes(&self) -> usize {
        self.messages.div_ceil(self.batch_size.max(1))
    }
}

/// The catalog: the queue's timeout below the function's (including a queue that hides a
/// delivery for no time at all, the published minimum of `VisibilityTimeout`) and above it,
/// one message and several, and a batch ceiling both above and below what is waiting.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue that hides nothing, a five second function",
            visibility_timeout: 0,
            function_timeout: 5,
            messages: 1,
            batch_size: 10,
        },
        Row {
            case: "a one second queue, an eight second function, three messages",
            visibility_timeout: 1,
            function_timeout: 8,
            messages: 3,
            batch_size: 10,
        },
        Row {
            case: "a one second queue, a five second function, two messages one per batch",
            visibility_timeout: 1,
            function_timeout: 5,
            messages: 2,
            batch_size: 1,
        },
        Row {
            case: "a four second queue, a one second function",
            visibility_timeout: 4,
            function_timeout: 1,
            messages: 1,
            batch_size: 10,
        },
    ]
}

#[tokio::test]
async fn a_polled_batch_stays_in_flight_for_as_long_as_its_attempt_may_last() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, mut driven) = driven_router_with_queues();
        let function = format!("esm-hold-fn-{index}");
        // The pair is created the way Lambda accepts one — a queue that outlasts its
        // function — and the timeout under test is set afterwards, which is the only way a
        // client reaches a shorter one.
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-hold-queue-{index}"),
            &json!({ "VisibilityTimeout": row.hold().to_string() }),
        )
        .await;
        create_timed_function(&router, case, &function, row.function_timeout).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": row.batch_size,
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

        // A mapping polls from the moment it reports `Enabled` — a move the clock owes, which
        // every read of the mapping materializes — so the queue is given the timeout under
        // test before anything is waiting on it.
        await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;
        set_visibility_timeout(&router, case, &url, row.visibility_timeout).await;

        for message in 0..row.messages {
            send_message(
                &router,
                case,
                &url,
                &format!("held-{index}-{message}"),
                None,
            )
            .await;
        }

        // One pass per batch, each awaited: when a pass answers, the batch it took is out of
        // the queue and the attempt it was handed to has been decided. Every message that
        // pass took is then hidden for the LONGER of the two spans, counted from the pass —
        // which is the rule, read off the instant the queue recorded rather than guessed at
        // from a count read later.
        let mut taken = 0usize;
        for pass in 0..row.passes() {
            let before = SystemTime::now();
            let hidden_before = hidden(driven.engine(), &arn, before);
            driven.poll_once().await;
            let after = SystemTime::now();
            let spent = after.duration_since(before).unwrap_or_default();
            let fresh: Vec<(String, SystemTime)> = hidden(driven.engine(), &arn, after)
                .into_iter()
                .filter(|(body, _)| !hidden_before.contains_key(body))
                .collect();
            assert!(
                !fresh.is_empty() && fresh.len() <= row.batch_size,
                "case {case:?}: pass {pass} must take a batch of at most the {} message(s) \
                 its BatchSize allows out of the queue, and it took {}",
                row.batch_size,
                fresh.len()
            );
            taken += fresh.len();
            for (body, until) in fresh {
                let measured = until.duration_since(before).unwrap_or_default();
                assert!(
                    measured >= row.hold_span() && measured <= row.hold_span() + spent,
                    "case {case:?}: the message {body:?} must stay out of its queue for the \
                     longer of the queue's {}s VisibilityTimeout and the function's {}s \
                     Timeout — {:?} from the pass that polled it — and the queue hides it \
                     for {measured:?} (the pass itself took {spent:?})",
                    row.visibility_timeout,
                    row.function_timeout,
                    row.hold_span()
                );
            }
        }
        assert_eq!(
            taken, row.messages,
            "case {case:?}: the {} pass(es) a BatchSize of {} takes must get all {} waiting \
             message(s) out of the queue, and they took {taken}",
            row.passes(),
            row.batch_size,
            row.messages
        );

        // Nothing was deleted: a message leaves a queue only once the attempt it was handed
        // to processed it, and no deployment package ran here.
        let held = held_until(driven.engine(), &arn);
        assert_eq!(
            held.len(),
            row.messages,
            "case {case:?}: no attempt processed anything, so the queue must still hold all \
             {} message(s), and it holds {held:?}",
            row.messages
        );

        // The same fact through the queue's own API: nothing visible and every message in
        // flight with the poller. A hold that lapsed between two passes is polled again by
        // the next pass — which is the rule's own answer to it — so the reading is taken
        // after a pass that leaves every message held at once.
        let waiting = row.messages.to_string();
        let deadline = Instant::now() + CLOCK_BUDGET;
        let counts = loop {
            let counts = depths(&router, case, &url).await;
            if (counts.0.as_deref(), counts.1.as_deref()) == (Some("0"), Some(waiting.as_str()))
                || Instant::now() >= deadline
            {
                break counts;
            }
            driven.poll_once().await;
        };
        assert_eq!(
            (counts.0.as_deref(), counts.1.as_deref()),
            (Some("0"), Some(waiting.as_str())),
            "case {case:?}: while an attempt on them may still be running, all {} message(s) \
             must be in flight with the poller and none visible, got {:?} visible and {:?} in \
             flight",
            row.messages,
            counts.0,
            counts.1
        );

        // And the mapping is still the one that holds them: it reports what it polled for,
        // and the attempt it made on the batch — which the pass recorded before it answered,
        // so this is read once rather than waited for.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("State")),
            (200, Some(STATE_ENABLED.to_string())),
            "case {case:?}: the mapping must still report {STATE_ENABLED}, got {} {}",
            reading.status,
            reading.text()
        );
        assert_eq!(
            reading.string("LastProcessingResult").as_deref(),
            Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
            "case {case:?}: nothing here runs a deployment package, so the attempt this \
             mapping made on its batch must be reported as the unsuccessful one it was, got \
             {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn the_hold_lapses_once_the_attempt_can_no_longer_be_running() {
    // The other side of the rule: the hold is the span the attempt may last, so it is
    // BOUNDED by that span rather than indefinite — once it is over, the message is the
    // queue's to hand over again. The mapping is PAUSED first ("when false, Lambda pauses
    // polling and invocation"), because a running poller takes the message straight back
    // the instant it becomes visible and nothing could be observed between the two.
    let case = "a queue that hides nothing, a three second function, paused mid-hold";
    let visibility_timeout = 0;
    let function_timeout = 3;
    let hold = Duration::from_secs(u64::try_from(function_timeout).unwrap_or(0));
    let body = "held-then-released";
    let (router, mut driven) = driven_router_with_queues();
    let (url, arn) = create_queue(
        &router,
        case,
        "esm-hold-lapses-queue",
        &json!({ "VisibilityTimeout": function_timeout.to_string() }),
    )
    .await;
    create_timed_function(&router, case, "esm-hold-lapses-fn", function_timeout).await;
    let created = create_mapping(
        &router,
        &json!({ "FunctionName": "esm-hold-lapses-fn", "EventSourceArn": arn }),
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
    set_visibility_timeout(&router, case, &url, visibility_timeout).await;
    send_message(&router, case, &url, body, None).await;

    // The pass takes the message out of the queue, and the hold it leaves on it is the span
    // the attempt may last and NO MORE: that bound is what the lapse below follows from.
    let before = SystemTime::now();
    driven.poll_once().await;
    let spent = SystemTime::now().duration_since(before).unwrap_or_default();
    let until = *hidden(driven.engine(), &arn, SystemTime::now())
        .get(body)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the pass must take the message out of the queue, and the \
                    queue is hiding {:?}", held_until(driven.engine(), &arn))
        });
    let measured = until.duration_since(before).unwrap_or_default();
    assert!(
        measured <= hold + spent,
        "case {case:?}: the hold on a polled batch is the span one attempt at it may last \
         ({hold:?}) and no more, and the queue hides the message for {measured:?} (the pass \
         itself took {spent:?})"
    );

    let paused = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ "Enabled": false }),
    )
    .await;
    assert_eq!(
        paused.status,
        202,
        "case {case:?}: expected UpdateEventSourceMapping to answer 202, got {} {}",
        paused.status,
        paused.text()
    );
    await_mapping_state(&router, case, &uuid, STATE_DISABLED).await;

    // Once the span the attempt could have lasted is up: the message is the queue's again,
    // and nothing deleted it, because no deployment package ran. The lapse is work the CLOCK
    // owes at the instant the hold above runs out at, so it is looked for again until the
    // queue reports it, inside a deadline taken from that very instant.
    let deadline = Instant::now()
        + until
            .duration_since(SystemTime::now())
            .unwrap_or_default()
        + CLOCK_BUDGET;
    let counts = loop {
        let counts = depths(&router, case, &url).await;
        if (counts.0.as_deref(), counts.1.as_deref()) == (Some("1"), Some("0"))
            || Instant::now() >= deadline
        {
            break counts;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    };
    assert_eq!(
        (counts.0.as_deref(), counts.1.as_deref()),
        (Some("1"), Some("0")),
        "case {case:?}: once no attempt can still be running the message must be visible on \
         its queue again, got {:?} visible and {:?} in flight",
        counts.0,
        counts.1
    );

    // And it stays the queue's: the paused mapping "pauses polling and invocation", so a
    // pass made while it rests takes nothing back.
    driven.poll_once().await;
    let counts = depths(&router, case, &url).await;
    assert_eq!(
        (counts.0.as_deref(), counts.1.as_deref()),
        (Some("1"), Some("0")),
        "case {case:?}: a paused mapping polls nothing, so the message it let go must still \
         be visible on its queue, got {:?} visible and {:?} in flight",
        counts.0,
        counts.1
    );
}
