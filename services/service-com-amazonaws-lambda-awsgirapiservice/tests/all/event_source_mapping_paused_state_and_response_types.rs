// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lifecycle state and the response-type list an event source
//! mapping reports: `CreateEventSourceMapping` "creates a mapping between an event source
//! and an Lambda Event Function" and `GetEventSourceMapping` "returns details about an
//! event source mapping".
//!
//! Two rules under test, over the one `EventSourceMappingConfiguration` every mapping
//! operation answers:
//!
//! 1. A create is accepted rather than applied (202), so the mapping it answers is the one
//!    being stood up: it reports the transitional `Creating` whatever polling the request
//!    asked for, and the clock then settles it into the state that setting asks for —
//!    `Enabled`, or the `Disabled` of a mapping asked for with polling paused ("when false,
//!    Lambda pauses polling and invocation"), which is what every later read reports.
//! 2. `FunctionResponseTypes` is "a list of CURRENT response type enums applied to the
//!    event source mapping": the types applied to a mapping are always known, so the member
//!    is reported as the EMPTY list when none were requested rather than left out.
//!
//! Every row drives real HTTP requests through the router — over the ONE mapping harness
//! this test binary has, [`crate::event_source_mappings`] — and reads the mapping back
//! through `GetEventSourceMapping` and `ListEventSourceMappings`, so a state or a list that
//! was answered but never stored fails here. The rows differ along what the rules
//! generalise over: how `Enabled` is stated (omitted, `true`, `false`), WHICH event source
//! the mapping polls (a queue, each of the two shard sources and an MSK cluster) and
//! whether the request states any response type (none at all, the empty list, and the one
//! value the enum has).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::event_source_mappings::{
    await_mapping_state, create_function, create_mapping, create_request, get_mapping,
    list_mappings, router_on, DYNAMODB_SOURCE, KINESIS_SOURCE, MSK_SOURCE, QUEUE_SOURCE,
    STATE_CREATING, STATE_DISABLED,
};

/// The terminal state a mapping asked to be active reaches, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const STATE_ENABLED: &str = "Enabled";

/// The ONLY value `FunctionResponseType` has, from
/// `smithy_query(query="shape", shape="FunctionResponseType")` — a list of at most one of
/// them (`FunctionResponseTypeList` is `@length(min: 0, max: 1)`).
const REPORT_BATCH_ITEM_FAILURES: &str = "ReportBatchItemFailures";

/// One row: the mapping to create, and what its configuration must report.
struct Row {
    name: &'static str,
    function: &'static str,
    event_source: &'static str,
    /// What the create states beside the function and the event source.
    extras: Value,
    /// The `State` the create must answer: the transitional one, whatever polling the
    /// request asked for.
    created_state: &'static str,
    /// The `State` the mapping must be in once the create's settle is due.
    settles_into: &'static str,
    /// The `FunctionResponseTypes` every reader must report.
    response_types: Value,
}

/// The rows.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "a paused queue mapping stating no response type",
            function: "esm-paused-queue",
            event_source: QUEUE_SOURCE,
            extras: json!({ "Enabled": false, "ScalingConfig": { "MaximumConcurrency": 2 } }),
            created_state: STATE_CREATING,
            settles_into: STATE_DISABLED,
            response_types: json!([]),
        },
        Row {
            name: "a paused kinesis mapping stating the one response type the enum has",
            function: "esm-paused-kinesis",
            event_source: KINESIS_SOURCE,
            extras: json!({
                "Enabled": false,
                "StartingPosition": "LATEST",
                "FunctionResponseTypes": [REPORT_BATCH_ITEM_FAILURES],
            }),
            created_state: STATE_CREATING,
            settles_into: STATE_DISABLED,
            response_types: json!([REPORT_BATCH_ITEM_FAILURES]),
        },
        Row {
            name: "a paused msk mapping stating the empty response type list",
            function: "esm-paused-msk",
            event_source: MSK_SOURCE,
            extras: json!({
                "Enabled": false,
                "Topics": ["orders"],
                "FunctionResponseTypes": [],
            }),
            created_state: STATE_CREATING,
            settles_into: STATE_DISABLED,
            response_types: json!([]),
        },
        Row {
            name: "an active queue mapping, enabled by default, stating no response type",
            function: "esm-active-default",
            event_source: QUEUE_SOURCE,
            extras: json!({}),
            created_state: STATE_CREATING,
            settles_into: STATE_ENABLED,
            response_types: json!([]),
        },
        Row {
            name: "an explicitly active dynamodb mapping stating one response type",
            function: "esm-active-dynamodb",
            event_source: DYNAMODB_SOURCE,
            extras: json!({
                "Enabled": true,
                "StartingPosition": "TRIM_HORIZON",
                "FunctionResponseTypes": [REPORT_BATCH_ITEM_FAILURES],
            }),
            created_state: STATE_CREATING,
            settles_into: STATE_ENABLED,
            response_types: json!([REPORT_BATCH_ITEM_FAILURES]),
        },
    ]
}

/// The `FunctionResponseTypes` a configuration reports, as the document carries it —
/// `Value::Null` stands for the member being absent, which is what the rule forbids.
fn response_types(configuration: &Value) -> Value {
    configuration
        .get("FunctionResponseTypes")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Whether `state` is one this row's mapping may report BEFORE the instant its create
/// made a transition due.
///
/// A mapping is `Creating` when its create answers, and the clock may have made the move to
/// its terminal state due before the next read reached the store, so that state is admissible
/// here too (the terminal state is asserted on its own once the move is certainly due).
fn state_before_settle_is_admissible(row: &Row, state: Option<&str>) -> bool {
    state.is_some_and(|reported| reported == row.created_state || reported == row.settles_into)
}

#[tokio::test]
async fn every_mapping_is_created_in_the_transitional_state_and_settles_as_asked() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Every row is created first, over the one store, so the clock-owed transitions of all
    // of them run down together and this suite waits past the due instant ONCE.
    let mut created_rows = Vec::new();
    for row in rows() {
        let label = row.name;
        create_function(&router, row.function).await;

        let created = create_mapping(
            &router,
            &create_request(row.function, Some(row.event_source), &row.extras),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must answer 202 Accepted, got {} {}",
            created.status,
            created.text()
        );
        assert_eq!(
            created.string("State"),
            Some(row.created_state.to_string()),
            "case {label:?}: the create must answer State {}, got {}",
            row.created_state,
            created.text()
        );
        assert_eq!(
            response_types(&created.json()),
            row.response_types,
            "case {label:?}: the create must report FunctionResponseTypes as {}, got {} in {}",
            row.response_types,
            response_types(&created.json()),
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a UUID, got {}", created.text()));
        let created_at = created
            .json()
            .get("LastModified")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: the create reports LastModified"));

        // The round trip: the state and the list come back out of the store, so a value
        // that was answered but never persisted fails here.
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: the read-back must answer 200, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert!(
            state_before_settle_is_admissible(&row, fetched.string("State").as_deref()),
            "case {label:?}: the read-back must report State {} before its settle is due, \
             got {:?} in {}",
            row.created_state,
            fetched.string("State"),
            fetched.text()
        );
        assert_eq!(
            response_types(&fetched.json()),
            row.response_types,
            "case {label:?}: the read-back must report FunctionResponseTypes as {}, got {} in {}",
            row.response_types,
            response_types(&fetched.json()),
            fetched.text()
        );

        // And so does the listing of this mapping's own event source.
        let listed = list_mappings(&router, &format!("EventSourceArn={}", row.event_source)).await;
        let reported = listed
            .mappings()
            .into_iter()
            .find(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the listing must report this mapping, got {}",
                    listed.text()
                )
            });
        assert!(
            state_before_settle_is_admissible(&row, reported.get("State").and_then(Value::as_str)),
            "case {label:?}: the listing must report State {} before the settle is due, got \
             {reported}",
            row.created_state
        );
        assert_eq!(
            response_types(&reported),
            row.response_types,
            "case {label:?}: the listing must report FunctionResponseTypes as {}, got {} in {}",
            row.response_types,
            response_types(&reported),
            listed.text()
        );

        created_rows.push((row, uuid, created_at));
    }

    // Each mapping is read once the transition its create owed is IN FORCE: the transition
    // is the CLOCK's, so this waits for the state it brings — looking again until it holds
    // or the crate's own budget for clock-owed work passes ([`await_mapping_state`]) —
    // rather than for a span of wall clock.
    for (row, uuid, created_at) in created_rows {
        let label = row.name;
        await_mapping_state(&router, label, &uuid, row.settles_into).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State")),
            (200, Some(row.settles_into.to_string())),
            "case {label:?}: once the settle was due the mapping must report {}, got {} {}",
            row.settles_into,
            settled.status,
            settled.text()
        );
        // The transition moves the date to the instant the clock owed it at, which is a
        // FRACTION of a second after the create (the state-settle span this crate chooses),
        // while the member is reported at whole-second resolution (see
        // `crate::event_source_mapping_instant_and_attempt`): so the date the transition
        // stamps is the creation's own second or the one after it, and never any other.
        let settled_at = settled.json().get("LastModified").and_then(Value::as_f64);
        assert!(
            settled_at.is_some_and(|modified| modified == created_at || modified == created_at + 1.0),
            "case {label:?}: LastModified must move with the transition to the second the \
             clock owed it in, {created_at} or the next, got {settled_at:?}"
        );
        assert_eq!(
            response_types(&settled.json()),
            row.response_types,
            "case {label:?}: the settled mapping must still report FunctionResponseTypes as \
             {}, got {} in {}",
            row.response_types,
            response_types(&settled.json()),
            settled.text()
        );
    }
}
