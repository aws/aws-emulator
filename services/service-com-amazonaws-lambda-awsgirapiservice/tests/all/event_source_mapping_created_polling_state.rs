// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `State` a mapping is CREATED in, over every operation that
//! answers an `EventSourceMappingConfiguration`.
//!
//! THE RULE: a create is ACCEPTED rather than applied — it answers 202 — so the mapping it
//! answers is the one being stood up: `CreateEventSourceMapping` reports the transitional
//! `Creating` whatever polling the request asked for, and the CLOCK then settles the mapping
//! into the state its `Enabled` setting asks for — "when true, the event source mapping is
//! active. When false, Lambda pauses polling and invocation. Default: True"
//! (`smithy_query(query="doc", shape="CreateEventSourceMappingRequest", member="Enabled")`) —
//! so an active mapping ends `Enabled` and a paused one ends `Disabled`, on every later read
//! of the record. A client that polls a mapping it has just created therefore watches it leave
//! `Creating` for the state it asked for.
//!
//! The rows differ along what the rule generalises over: how `Enabled` is stated (omitted,
//! `true`, `false`) and WHICH event source the mapping polls — a queue, each of the two
//! shard sources, and an MSK cluster — including sources beyond the queue the wire fixture
//! this suite was written for used.
//!
//! Every row drives real HTTP requests through the router, over the ONE mapping harness this
//! test binary has ([`crate::event_source_mappings`]), and reads each mapping back through
//! `GetEventSourceMapping` and `ListEventSourceMappings`, so a state that was answered but
//! never stored fails here. Nothing waits for a span of wall clock: the one transition any
//! row owes is waited for by looking again until the mapping REPORTS it
//! ([`crate::event_source_mappings::await_mapping_state`]).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, create_function, create_mapping, create_request, get_mapping,
    list_mappings, router_on, DYNAMODB_SOURCE, KINESIS_SOURCE, MSK_SOURCE, QUEUE_SOURCE,
    STATE_CREATING, STATE_DISABLED, STATE_ENABLED,
};

/// One row: the mapping to create, the `State` its create must answer, and the terminal
/// `State` it must report once every transition the create owes is due.
struct Row {
    case: &'static str,
    function: &'static str,
    event_source: &'static str,
    /// What the create states beside the function and the event source.
    extras: Value,
    /// The `State` the create itself must answer.
    created_as: &'static str,
    /// The terminal `State` the mapping must report.
    settles_into: &'static str,
}

/// The rows.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a queue mapping stating no polling setting at all",
            function: "created-state-queue-default",
            event_source: QUEUE_SOURCE,
            extras: json!({}),
            created_as: STATE_CREATING,
            settles_into: STATE_ENABLED,
        },
        Row {
            case: "a queue mapping asked to be active",
            function: "created-state-queue-active",
            event_source: QUEUE_SOURCE,
            extras: json!({ "Enabled": true, "BatchSize": 4 }),
            created_as: STATE_CREATING,
            settles_into: STATE_ENABLED,
        },
        Row {
            case: "a queue mapping asked for with polling paused",
            function: "created-state-queue-paused",
            event_source: QUEUE_SOURCE,
            extras: json!({ "Enabled": false }),
            created_as: STATE_CREATING,
            settles_into: STATE_DISABLED,
        },
        Row {
            case: "a kinesis mapping asked for with polling paused",
            function: "created-state-kinesis-paused",
            event_source: KINESIS_SOURCE,
            extras: json!({ "Enabled": false, "StartingPosition": "LATEST" }),
            created_as: STATE_CREATING,
            settles_into: STATE_DISABLED,
        },
        Row {
            case: "a dynamodb stream mapping asked to be active",
            function: "created-state-dynamodb-active",
            event_source: DYNAMODB_SOURCE,
            extras: json!({ "Enabled": true, "StartingPosition": "TRIM_HORIZON" }),
            created_as: STATE_CREATING,
            settles_into: STATE_ENABLED,
        },
        Row {
            case: "an msk mapping asked for with polling paused",
            function: "created-state-msk-paused",
            event_source: MSK_SOURCE,
            extras: json!({ "Enabled": false, "Topics": ["orders"] }),
            created_as: STATE_CREATING,
            settles_into: STATE_DISABLED,
        },
    ]
}

/// The `State` the listing of `event_source` reports for the mapping `uuid` names.
async fn listed_state(
    router: &MultiServiceRouter,
    case: &str,
    event_source: &str,
    uuid: &str,
) -> Option<String> {
    let listed = list_mappings(router, &format!("EventSourceArn={event_source}")).await;
    listed
        .mappings()
        .into_iter()
        .find(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the listing must report this mapping, got {}",
                listed.text()
            )
        })
        .get("State")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_mapping_is_created_in_creating_and_settles_into_what_its_polling_asks_for() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let mut created_rows = Vec::new();
    for row in rows() {
        let case = row.case;
        create_function(&router, row.function).await;
        let created = create_mapping(
            &router,
            &create_request(row.function, Some(row.event_source), &row.extras),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: the create must answer 202 Accepted, got {} {}",
            created.status,
            created.text()
        );
        assert_eq!(
            created.string("State").as_deref(),
            Some(row.created_as),
            "case {case:?}: the create must answer State {}, got {}",
            row.created_as,
            created.text()
        );
        let uuid = created.string("UUID").unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create answers a UUID, got {}",
                created.text()
            )
        });

        // Every mapping is still being stood up when the create answers, and the clock may
        // have settled it before a read-back arrives, so the STORED state is asserted below
        // once the settle is certainly due — which is what makes the state a reader answers
        // one the record carries rather than a value an answer made up.
        created_rows.push((row, uuid));
    }

    // Every row ends in the state its polling setting asks for: the transition the CLOCK owes
    // brings it there — waited for by looking again until the mapping reports it.
    for (row, uuid) in created_rows {
        let case = row.case;
        await_mapping_state(&router, case, &uuid, row.settles_into).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State").as_deref()),
            (200, Some(row.settles_into)),
            "case {case:?}: the mapping must report {} once every transition it owes is due, \
             got {} {}",
            row.settles_into,
            settled.status,
            settled.text()
        );
        assert_eq!(
            listed_state(&router, case, row.event_source, &uuid)
                .await
                .as_deref(),
            Some(row.settles_into),
            "case {case:?}: the listing must report {} too",
            row.settles_into
        );
    }
}
