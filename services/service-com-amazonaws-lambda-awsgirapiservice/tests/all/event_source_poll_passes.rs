// SPDX-License-Identifier: CC0-1.0

//! The POLL PASS of an event source mapping on an Amazon SQS queue as a caller's own step:
//! `crate::state::claim_poll_passes` and `crate::state::poll_passes_once`.
//!
//! A mapping's data plane is background work — `CreateEventSourceMapping` "creates a mapping
//! between an event source and an Lambda Event Function. Lambda reads items from the event
//! source and invokes the function", with no client call asking for any one read — and nothing
//! a client can read says whether a pass has happened yet. So the emulator lets the host that
//! owns the store TAKE the one poll claim of that store and make the passes itself: while the
//! claim is held no background loop is started for the store, and each pass is one awaited
//! call that does the whole of one wake-up's work.
//!
//! The RULE under test, in one sentence: while a caller holds a store's poll claim, that
//! store's queue mappings advance only on the caller's passes — nothing is taken out of a
//! queue, handed over or reported in between — and ONE pass is the whole of what one wake-up
//! of the poller does for every mapping that is polling: at most `BatchSize` messages taken
//! out of the queue, the mapping's `FilterCriteria` applied to them, the admitted ones handed
//! to the function, exactly the messages the attempt consumed deleted, and what that attempt
//! came to recorded as the mapping's `LastProcessingResult` before the call answers.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with
//! Lambda and Amazon SQS on ONE engine, as `core/router` assembles them: the queue is created
//! and written through Amazon SQS's own surface and the mapping through Lambda's, and what a
//! row asserts is the queue's own message counts and the mapping's own report. The rows differ
//! along what the rule generalises over: how many messages are waiting (none, one, several),
//! how many of them one batch may hold (above and below what is waiting), whether the
//! mapping's filter admits them or rejects them all, and whether the mapping is polling at all
//! ("when false, Lambda pauses polling and invocation").
//!
//! No row sleeps and no row waits out a span: the only waits are for state moves the clock
//! owes and every read of the resource materializes.

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    claim_poll_passes, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_OK,
};

use crate::event_source_mappings::{
    await_mapping_state, create_mapping, create_queue, create_timed_function,
    driven_router_with_queues, get_mapping, messages_held, queue_message_counts, send_message,
    STATE_DISABLED, STATE_ENABLED,
};

/// The mapped function's `Timeout`, in seconds, and the queue's own `VisibilityTimeout`: the
/// same value, which is the pairing `CreateEventSourceMapping` accepts (a queue must outlast
/// the function it feeds). Several seconds, so a message one pass took is still out of the
/// queue while the row reads the counts the pass left.
const TIMEOUT_SECONDS: i64 = 30;

/// How many times a row re-reads its queue to show that NOTHING advances between passes.
///
/// A count of looks and not a span of time: with the claim held there is no loop to wait for,
/// so a row that would see a background poll see it on the next read rather than after some
/// number of seconds.
const LOOKS_WITH_NO_PASS: usize = 5;

/// One row: what the mapping asks for, what is waiting on its queue, and what one pass of its
/// poller must then do.
struct Row {
    case: &'static str,
    /// Whether the mapping is created polling: "When false, Lambda pauses polling and
    /// invocation."
    enabled: bool,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from your
    /// stream or queue".
    batch_size: usize,
    /// The `Pattern` of the mapping's one `FilterCriteria` filter, if it carries one.
    pattern: Option<&'static str>,
    /// The bodies waiting on the queue when the first pass is made.
    bodies: &'static [&'static str],
    /// How many messages the FIRST pass must take out of the queue.
    first_pass_takes: u32,
    /// How many messages the queue still holds once that pass has answered — a message
    /// leaves only because the filter rejected it or because the attempt processed it.
    held_after_first_pass: u32,
    /// What the mapping must report as `LastProcessingResult` after that pass: `None` for a
    /// pass that handed no batch over, and so has no attempt to report.
    reported: Option<&'static str>,
}

/// The catalog: nothing waiting and something waiting, a batch ceiling above and below what
/// is waiting, a filter that admits everything and one that rejects everything, and a mapping
/// that is not polling at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one message waiting, a ceiling above it",
            enabled: true,
            batch_size: 10,
            pattern: None,
            bodies: &[r#"{"kind":"keep"}"#],
            first_pass_takes: 1,
            held_after_first_pass: 1,
            reported: Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
        },
        Row {
            case: "three messages waiting, two per batch",
            enabled: true,
            batch_size: 2,
            pattern: None,
            bodies: &[r#"{"kind":"a"}"#, r#"{"kind":"b"}"#, r#"{"kind":"c"}"#],
            first_pass_takes: 2,
            held_after_first_pass: 3,
            reported: Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
        },
        Row {
            case: "a filter that rejects every message waiting",
            enabled: true,
            batch_size: 10,
            pattern: Some(r#"{"body":{"kind":["keep"]}}"#),
            bodies: &[r#"{"kind":"drop"}"#, r#"{"kind":"other"}"#],
            first_pass_takes: 2,
            // A message the filter rejected is removed from the queue: nothing else would,
            // and the mapping would be handed it again at every pass.
            held_after_first_pass: 0,
            // The pass handled the whole batch and had no invocation to make for it, which is
            // a processing attempt that succeeded.
            reported: Some(LAST_PROCESSING_RESULT_OK),
        },
        Row {
            case: "a filter that admits one of the two waiting",
            enabled: true,
            batch_size: 10,
            pattern: Some(r#"{"body":{"kind":["keep"]}}"#),
            bodies: &[r#"{"kind":"keep"}"#, r#"{"kind":"drop"}"#],
            first_pass_takes: 2,
            held_after_first_pass: 1,
            reported: Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
        },
        Row {
            case: "nothing waiting at all",
            enabled: true,
            batch_size: 10,
            pattern: None,
            bodies: &[],
            first_pass_takes: 0,
            held_after_first_pass: 0,
            reported: None,
        },
        Row {
            case: "a mapping that is not polling",
            enabled: false,
            batch_size: 10,
            pattern: None,
            bodies: &[r#"{"kind":"keep"}"#],
            first_pass_takes: 0,
            held_after_first_pass: 1,
            reported: None,
        },
    ]
}

#[tokio::test]
async fn one_poll_pass_is_the_whole_of_a_wake_up_and_the_only_thing_that_advances_a_mapping() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, mut driven) = driven_router_with_queues();
        // The claim is EXCLUSIVE: it is what keeps a background loop from being started for
        // this store, so a second caller cannot have it while this suite holds it.
        assert!(
            !claim_poll_passes(driven.engine()),
            "case {case:?}: the poll claim this suite holds must not be handed out twice"
        );

        let function = format!("poll-pass-fn-{index}");
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("poll-pass-queue-{index}"),
            &json!({ "VisibilityTimeout": TIMEOUT_SECONDS.to_string() }),
        )
        .await;
        create_timed_function(&router, case, &function, TIMEOUT_SECONDS).await;
        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": arn,
            "BatchSize": row.batch_size,
            "Enabled": row.enabled,
        });
        if let Some(pattern) = row.pattern {
            request["FilterCriteria"] = json!({ "Filters": [{ "Pattern": pattern }] });
        }
        let created = create_mapping(&router, &request).await;
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
        let settled = if row.enabled {
            STATE_ENABLED
        } else {
            STATE_DISABLED
        };
        await_mapping_state(&router, case, &uuid, settled).await;

        for body in row.bodies {
            send_message(&router, case, &url, body, None).await;
        }

        // Nothing advances without a pass: with the claim held there is no loop, so however
        // often the row looks, every message it sent is still the queue's own and the mapping
        // has no attempt to report.
        let waiting = u32::try_from(row.bodies.len()).unwrap_or(u32::MAX);
        for look in 0..LOOKS_WITH_NO_PASS {
            let counts = queue_message_counts(&router, case, &url).await;
            assert_eq!(
                counts,
                (waiting, 0, 0),
                "case {case:?}: look {look} before any pass must find all {waiting} \
                 message(s) visible on the queue, got {counts:?} as (visible, in flight, \
                 delayed)"
            );
            let reading = get_mapping(&router, &uuid).await;
            assert_eq!(
                reading.json().get("LastProcessingResult"),
                None,
                "case {case:?}: look {look} before any pass must find no processing attempt \
                 reported, got {}",
                reading.text()
            );
        }

        // ONE pass: it takes at most the mapping's `BatchSize` out of the queue, applies the
        // filter, hands the admitted messages over, deletes exactly what left the queue for
        // good and records the attempt — all before it answers.
        driven.poll_once().await;
        let counts = queue_message_counts(&router, case, &url).await;
        // A message the pass took is either in flight with the poller or gone from the queue,
        // so what is still VISIBLE is what the pass left behind.
        assert_eq!(
            (waiting - counts.0, messages_held(counts)),
            (row.first_pass_takes, row.held_after_first_pass),
            "case {case:?}: one pass must take {} message(s) out of the queue and leave it \
             holding {}, got {counts:?} as (visible, in flight, delayed)",
            row.first_pass_takes,
            row.held_after_first_pass
        );
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.string("LastProcessingResult").as_deref(),
            row.reported,
            "case {case:?}: the pass must report {:?} as the attempt it made, got {}",
            row.reported,
            reading.text()
        );

        // And the passes that follow get through whatever the ceiling left behind: a mapping
        // that is polling clears its queue of everything the filter rejected or an attempt
        // processed, and one that is not polling takes nothing however many passes are made.
        for _ in 0..row.bodies.len() {
            driven.poll_once().await;
        }
        let counts = queue_message_counts(&router, case, &url).await;
        let (expected_visible, expected_held) = if row.enabled {
            // Every message was taken out of the queue by one pass or another: what is left
            // is what no attempt consumed, in flight with the poller.
            (0, row.held_after_first_pass)
        } else {
            (waiting, waiting)
        };
        assert_eq!(
            (counts.0, messages_held(counts)),
            (expected_visible, expected_held),
            "case {case:?}: after a pass per waiting message the queue must report \
             {expected_visible} visible of {expected_held} held, got {counts:?} as (visible, \
             in flight, delayed)"
        );
    }
}

/// The claim is per STORE, so a caller driving one engine's passes says nothing about
/// another's: the second engine's claim is free, and its own passes are its caller's.
#[tokio::test]
async fn the_poll_claim_belongs_to_the_store_it_was_taken_on() {
    let case = "two engines, each with its own claim";
    let (first_router, mut first) = driven_router_with_queues();
    let (second_router, mut second) = driven_router_with_queues();
    assert!(
        !claim_poll_passes(first.engine()) && !claim_poll_passes(second.engine()),
        "case {case:?}: each engine's claim is held by the caller that took it"
    );

    // One mapping per engine, over a queue of its own, and one message waiting on each.
    let mut queues: Vec<(String, String)> = Vec::new();
    for (index, router) in [&first_router, &second_router].into_iter().enumerate() {
        let (url, arn) = create_queue(
            router,
            case,
            &format!("poll-claim-queue-{index}"),
            &json!({ "VisibilityTimeout": TIMEOUT_SECONDS.to_string() }),
        )
        .await;
        let function = format!("poll-claim-fn-{index}");
        create_timed_function(router, case, &function, TIMEOUT_SECONDS).await;
        let created = create_mapping(
            router,
            &json!({ "FunctionName": function, "EventSourceArn": arn }),
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
        await_mapping_state(router, case, &uuid, STATE_ENABLED).await;
        send_message(router, case, &url, r#"{"kind":"keep"}"#, None).await;
        queues.push((url, uuid));
    }

    // A pass on the FIRST engine advances the first mapping and nothing else.
    first.poll_once().await;
    let first_counts = queue_message_counts(&first_router, case, &queues[0].0).await;
    let second_counts = queue_message_counts(&second_router, case, &queues[1].0).await;
    assert_eq!(
        (first_counts, second_counts),
        ((0, 1, 0), (1, 0, 0)),
        "case {case:?}: the pass made on the first engine must take the first queue's message \
         and leave the second engine's where it is, got {first_counts:?} and {second_counts:?} \
         as (visible, in flight, delayed)"
    );
    let untouched = get_mapping(&second_router, &queues[1].1).await;
    assert_eq!(
        untouched.json().get("LastProcessingResult"),
        None,
        "case {case:?}: the mapping of the engine nobody polled has no attempt to report, got \
         {}",
        untouched.text()
    );

    // And then the second's own pass advances it, on its own store.
    second.poll_once().await;
    let second_counts = queue_message_counts(&second_router, case, &queues[1].0).await;
    assert_eq!(
        second_counts,
        (0, 1, 0),
        "case {case:?}: the pass made on the second engine must take its queue's message, got \
         {second_counts:?} as (visible, in flight, delayed)"
    );
    let reported = get_mapping(&second_router, &queues[1].1).await;
    assert_eq!(
        reported.string("LastProcessingResult").as_deref(),
        Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
        "case {case:?}: the second engine's pass must report the attempt it made, got {}",
        reported.text()
    );
}
