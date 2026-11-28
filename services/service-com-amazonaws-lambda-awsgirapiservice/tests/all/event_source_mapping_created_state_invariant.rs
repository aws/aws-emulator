// SPDX-License-Identifier: CC0-1.0

//! What `CreateEventSourceMapping` answers does not depend on the POLLING the request asked
//! for.
//!
//! THE RULE: the operation is accepted rather than applied — it answers 202 — so the mapping
//! it answers is the one being stood up: `State` is the transitional `Creating` whatever
//! `Enabled` said, and the CLOCK then settles the mapping into the state that setting asks for
//! ("When true, the event source mapping is active. When false, Lambda pauses polling and
//! invocation. Default: True"), which is `Enabled` for a mapping asked to poll and `Disabled`
//! for one asked to be paused. A client that creates a mapping and polls it therefore watches
//! it LEAVE the transitional state, whichever state it asked for.
//!
//! Where the suites beside this one assert the rule row by row, this one asserts the
//! invariant: the set of states the creates answered has exactly ONE member, so a create's
//! answer cannot be keyed on the polling it was asked for — and every mapping still ends in
//! the state its own request asked for, read back through `GetEventSourceMapping`.
//!
//! The rows differ along what the rule generalises over: how `Enabled` is stated (omitted,
//! `true`, `false`) and which event source the mapping polls (a queue and a shard source,
//! whose pollers differ).
//!
//! Every row drives real HTTP requests through the router, over the ONE mapping harness this
//! test binary has ([`crate::event_source_mappings`]). Nothing waits for a span of wall clock:
//! the transition each row owes is waited for by looking again until the mapping REPORTS it
//! ([`crate::event_source_mappings::await_mapping_state`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::event_source_mappings::{
    await_mapping_state, create_function, create_mapping, create_request, get_mapping, router_on,
    settled_state, KINESIS_SOURCE, QUEUE_SOURCE, STATE_CREATING,
};

/// One row: the function and event source to map, and what the request states beside them.
struct Row {
    case: &'static str,
    function: &'static str,
    event_source: &'static str,
    extras: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue mapping stating no polling setting",
            function: "create-answer-queue-default",
            event_source: QUEUE_SOURCE,
            extras: json!({}),
        },
        Row {
            case: "a queue mapping asked to poll",
            function: "create-answer-queue-active",
            event_source: QUEUE_SOURCE,
            extras: json!({ "Enabled": true }),
        },
        Row {
            case: "a queue mapping asked to be paused",
            function: "create-answer-queue-paused",
            event_source: QUEUE_SOURCE,
            extras: json!({ "Enabled": false }),
        },
        Row {
            case: "a kinesis mapping asked to be paused",
            function: "create-answer-kinesis-paused",
            event_source: KINESIS_SOURCE,
            extras: json!({ "Enabled": false, "StartingPosition": "LATEST" }),
        },
        Row {
            case: "a kinesis mapping asked to poll",
            function: "create-answer-kinesis-active",
            event_source: KINESIS_SOURCE,
            extras: json!({ "Enabled": true, "StartingPosition": "TRIM_HORIZON" }),
        },
    ]
}

#[tokio::test]
async fn every_create_answers_one_state_whatever_polling_it_was_asked_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let mut answered_states: BTreeSet<String> = BTreeSet::new();
    let mut created = Vec::new();
    for row in rows() {
        let case = row.case;
        create_function(&router, row.function).await;
        let answer = create_mapping(
            &router,
            &create_request(row.function, Some(row.event_source), &row.extras),
        )
        .await;
        assert_eq!(
            answer.status,
            202,
            "case {case:?}: the create must answer 202 Accepted, got {} {}",
            answer.status,
            answer.text()
        );
        let state = answer.string("State").unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create answers a State, got {}",
                answer.text()
            )
        });
        answered_states.insert(state);
        let uuid = answer.string("UUID").unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create answers a UUID, got {}",
                answer.text()
            )
        });
        created.push((case, uuid, settled_state(&row.extras)));
    }

    // The invariant: one state for every create, and it is the transitional one.
    assert_eq!(
        answered_states.iter().map(String::as_str).collect::<Vec<_>>(),
        vec![STATE_CREATING],
        "the state a create answers may not be keyed on the polling it was asked for, got \
         {answered_states:?}"
    );

    // And every mapping still ends in the state its own request asked for, out of the store.
    for (case, uuid, settles_into) in created {
        await_mapping_state(&router, case, &uuid, settles_into).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State")),
            (200, Some(settles_into.to_string())),
            "case {case:?}: expected the stored {settles_into} once the transition was due, got \
             {} {}",
            settled.status,
            settled.text()
        );
    }
}
