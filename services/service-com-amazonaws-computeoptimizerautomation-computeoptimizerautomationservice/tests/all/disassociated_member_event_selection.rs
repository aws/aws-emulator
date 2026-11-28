// SPDX-License-Identifier: CC0-1.0

//! The RETRIEVAL SCOPE of every selection over the automation-EVENT store.
//!
//! The rule under test: a caller selects the automation events of its OWN
//! account and of each member account the organization reaches at the time of
//! the call, and of no other account — the standing is read per call, never
//! inferred from the event being stored. It holds for every operation that
//! SELECTS over the event store, so the table below takes each row through both
//! of them: `ListAutomationEvents` and `ListAutomationEventSummaries` (whose
//! rows count and sum exactly the events the listing reports). An event of an
//! account the caller no longer reaches is still reached BY ID
//! (`GetAutomationEvent`, `ListAutomationEventSteps`), which is what lets
//! `RollbackAutomationEvent` refuse with the caller's standing instead of
//! claiming the event has gone away.
//!
//! The rows differ along the dimension the rule generalises over — which
//! accounts are reachable at the moment of the call — and walk an organization
//! through association, removal of one member, removal of the other and a
//! re-association, with a different number of events per account, so no row is a
//! property of one account, one event count or one order of calls.
//!
//! Every call goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, through the harness
//! `automation_rule_lifecycle` owns and the opted-in router
//! `recommended_action_inventory` owns (opting in over the wire is what records
//! the caller's own analysed inventory, which is what an automation is started
//! for). Nothing reaches into the store, so each row is judged on association
//! state its own calls put there.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{call, call_ok, string_member};
use super::disassociated_member_retrieval::{savings_at, walk, MEMBER_B, SAVINGS_TOLERANCE};
use super::member_account_association_gate::{
    action_ids, assert_access_denied, associate, disassociate, MEMBER_A,
};
use super::recommended_action_inventory::opted_in_emulator;

/// Every event `ListAutomationEvents` reports for `request`.
async fn listed_events(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    walk(
        router,
        "ListAutomationEvents",
        request,
        "automationEvents",
        case,
    )
    .await
}

/// Every row `ListAutomationEventSummaries` reports for `request`.
async fn summary_rows(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    walk(
        router,
        "ListAutomationEventSummaries",
        request,
        "automationEventSummaries",
        case,
    )
    .await
}

/// A listing request narrowed to one account, which is the selection the event
/// store's `AccountId` filter performs
/// (`smithy_query(query="shape",shape="AutomationEventFilterName")`).
fn in_account(account_id: &str) -> Value {
    json!({"filters": [{"name": "AccountId", "values": [account_id]}]})
}

/// The `eventId`s of reported events, as a set: which accounts' events a
/// selection covers is what every row asserts, and the record order the store
/// walks is pinned by the paging suites.
fn event_ids(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|item| string_member(item, "eventId"))
        .collect()
}

/// Start an automation for `how_many` of `account_id`'s analysed actions, and
/// answer the `eventId`s the service minted. Each start is a call a client makes,
/// so the events a row is judged on are the ones the API created.
async fn start_automations(
    router: &MultiServiceRouter,
    account_id: &str,
    how_many: usize,
    case: &str,
) -> BTreeSet<String> {
    let actions = action_ids(account_id);
    assert!(
        actions.len() >= how_many,
        "case {case:?}: the analysis of {account_id} must describe at least {how_many} actions, \
         got {actions:?}"
    );
    let mut started = BTreeSet::new();
    for action_id in actions.into_iter().take(how_many) {
        let answer = call_ok(
            router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": action_id}),
            case,
        )
        .await;
        started.insert(string_member(&answer, "eventId"));
    }
    started
}

/// Assert that `ListAutomationEventSummaries` counts and sums exactly the events
/// `ListAutomationEvents` reports: the rows are the status groups of one
/// selection, so their counts total the reported events and their savings total
/// the reported events' estimates.
async fn assert_summaries_cover(router: &MultiServiceRouter, listed: &[Value], case: &str) {
    let rows = summary_rows(router, &json!({}), case).await;

    let counted: i64 = rows
        .iter()
        .map(|row| {
            row.pointer("/total/automationEventCount")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        })
        .sum();
    let expected_count = i64::try_from(listed.len()).expect("a small fixture set");
    assert_eq!(
        counted, expected_count,
        "case {case:?}: the summary rows must count the {expected_count} events the listing \
         reports, got {rows:?}"
    );

    for pointer in ["beforeDiscountSavings", "afterDiscountSavings"] {
        let reported: f64 = rows
            .iter()
            .map(|row| savings_at(row, &format!("/total/estimatedMonthlySavings/{pointer}")))
            .sum();
        let expected: f64 = listed
            .iter()
            .map(|event| savings_at(event, &format!("/estimatedMonthlySavings/{pointer}")))
            .sum();
        assert!(
            (reported - expected).abs() < SAVINGS_TOLERANCE,
            "case {case:?}: the summary rows must sum the reported events' {pointer} \
             ({expected}), got {reported}"
        );
    }
}

/// One row of the table: the arrangement step it runs first, and the accounts
/// the caller selects events of once that step has been served.
struct Row {
    case: &'static str,
    /// The association step to run: `Some((true, id))` associates `id`,
    /// `Some((false, id))` removes its association, `None` runs no step.
    step: Option<(bool, &'static str)>,
    /// The accounts whose automation events the caller then reaches.
    reachable: Vec<String>,
}

/// Every selection over the event store covers exactly the accounts the caller
/// reaches AT THE TIME OF THE CALL: its own, plus each member account whose
/// association is in place then — through association, a removal per member and
/// a re-association. Each account's events stay reachable by id throughout.
#[tokio::test]
async fn every_event_selection_covers_the_accounts_reachable_at_the_time_of_the_call() {
    let caller = state::caller_account_id();
    let router = opted_in_emulator("event-scope").await;
    associate(&router, MEMBER_A, "arrange").await;
    associate(&router, MEMBER_B, "arrange").await;

    // A different number of events per account, so no assertion below can hold
    // by counting alone.
    let own_events = start_automations(&router, &caller, 2, "arrange").await;
    let member_a_events = start_automations(&router, MEMBER_A, 1, "arrange").await;
    let member_b_events = start_automations(&router, MEMBER_B, 3, "arrange").await;
    let every_event: Vec<(String, BTreeSet<String>)> = vec![
        (caller.clone(), own_events.clone()),
        (MEMBER_A.to_string(), member_a_events.clone()),
        (MEMBER_B.to_string(), member_b_events.clone()),
    ];

    let rows: Vec<Row> = vec![
        Row {
            case: "both-members-associated",
            step: None,
            reachable: vec![caller.clone(), MEMBER_A.to_string(), MEMBER_B.to_string()],
        },
        Row {
            case: "one-member-removed",
            step: Some((false, MEMBER_A)),
            reachable: vec![caller.clone(), MEMBER_B.to_string()],
        },
        Row {
            case: "both-members-removed",
            step: Some((false, MEMBER_B)),
            reachable: vec![caller.clone()],
        },
        Row {
            case: "one-member-associated-again",
            step: Some((true, MEMBER_A)),
            reachable: vec![caller.clone(), MEMBER_A.to_string()],
        },
    ];

    for row in &rows {
        let case = row.case;
        let reachable = &row.reachable;
        if let Some((associate_it, account_id)) = &row.step {
            if *associate_it {
                associate(&router, account_id, case).await;
            } else {
                disassociate(&router, account_id, case).await;
            }
        }
        let expected: BTreeSet<String> = every_event
            .iter()
            .filter(|(account_id, _)| reachable.contains(account_id))
            .flat_map(|(_, events)| events.iter().cloned())
            .collect();

        // The event listing.
        let listed = listed_events(&router, &json!({}), case).await;
        assert_eq!(
            event_ids(&listed),
            expected,
            "case {case:?}: the listing must report the events of {reachable:?} and no others"
        );

        // The summaries, over the very same events.
        assert_summaries_cover(&router, &listed, case).await;

        // And a listing narrowed to ONE account reports that account's events
        // while it is reachable, and nothing of it once it is not.
        for (account_id, events) in &every_event {
            let narrowed = format!("{case}/in-{account_id}");
            let in_one_account = listed_events(&router, &in_account(account_id), &narrowed).await;
            let expected_in_one = if reachable.contains(account_id) {
                events.clone()
            } else {
                BTreeSet::new()
            };
            assert_eq!(
                event_ids(&in_one_account),
                expected_in_one,
                "case {narrowed:?}: a filter selects an account's events only while the caller \
                 reaches that account"
            );
            let narrowed_rows = summary_rows(&router, &in_account(account_id), &narrowed).await;
            let counted: i64 = narrowed_rows
                .iter()
                .map(|summary| {
                    summary
                        .pointer("/total/automationEventCount")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                })
                .sum();
            assert_eq!(
                counted,
                i64::try_from(expected_in_one.len()).expect("a small fixture set"),
                "case {narrowed:?}: the summary of one account counts exactly the events the \
                 listing of that account reports, got {narrowed_rows:?}"
            );

            // Every event stays reachable BY ID, whatever the selections cover:
            // a disassociation leaves the member's events exactly where they
            // were.
            for event_id in events {
                let by_id = format!("{narrowed}/by-id-{event_id}");
                let event = call_ok(
                    &router,
                    "GetAutomationEvent",
                    &json!({"eventId": event_id}),
                    &by_id,
                )
                .await;
                assert_eq!(
                    string_member(&event, "accountId"),
                    *account_id,
                    "case {by_id:?}: an event is reached by its id whatever the caller's \
                     standing in its account, got {event}"
                );
            }
        }
    }
}

/// The selections and the mutation agree about one and the same event: while the
/// member account is reachable the event is listed, summarised, read by id and
/// rolled back; once the association is gone the selections stop reporting it
/// while the by-id readers still serve it and the rollback is refused with the
/// caller's standing; and a re-association brings all of them back.
#[tokio::test]
async fn the_selections_and_the_rollback_agree_about_one_member_event() {
    let router = opted_in_emulator("agreement").await;
    associate(&router, MEMBER_A, "arrange").await;
    let started = start_automations(&router, MEMBER_A, 1, "arrange").await;
    let event_id = started
        .iter()
        .next()
        .expect("one automation was started")
        .clone();

    // Associated: every reader reports the event.
    let listed = listed_events(&router, &json!({}), "associated").await;
    assert_eq!(
        event_ids(&listed),
        started,
        "case \"associated\": the member's event is listed while the association is in place"
    );
    assert_summaries_cover(&router, &listed, "associated").await;

    disassociate(&router, MEMBER_A, "removed").await;

    // Removed: no SELECTION reports it, not even the one narrowed to its own
    // account.
    for request in [json!({}), in_account(MEMBER_A)] {
        let listed = listed_events(&router, &request, "removed").await;
        assert_eq!(
            event_ids(&listed),
            BTreeSet::new(),
            "case \"removed\": no selection reports an event of an account the caller no longer \
             reaches, got {listed:?}"
        );
        let rows = summary_rows(&router, &request, "removed").await;
        assert_eq!(
            rows,
            Vec::<Value>::new(),
            "case \"removed\": a selection that reports no event summarises no row, got {rows:?}"
        );
    }

    // Removed: the by-id readers still serve it, unchanged.
    let by_id = call_ok(
        &router,
        "GetAutomationEvent",
        &json!({"eventId": &event_id}),
        "removed",
    )
    .await;
    assert_eq!(
        string_member(&by_id, "eventId"),
        event_id,
        "case \"removed\": the event is still reached by its id, got {by_id}"
    );
    assert_eq!(
        string_member(&by_id, "accountId"),
        MEMBER_A,
        "case \"removed\": the event still records the account its automation ran in, got {by_id}"
    );
    let steps = walk(
        &router,
        "ListAutomationEventSteps",
        &json!({"eventId": &event_id}),
        "automationEventSteps",
        "removed",
    )
    .await;
    assert!(
        !steps.is_empty(),
        "case \"removed\": the event's steps are still reached by its id, got {steps:?}"
    );

    // Removed: the mutation is refused with the caller's standing, which is the
    // same fact the selections act on.
    assert_access_denied(
        call(
            &router,
            "RollbackAutomationEvent",
            &json!({"eventId": &event_id, "clientToken": "agreement-removed"}),
        )
        .await,
        "removed",
    );

    // Re-associated: the selections report it again, as the same event.
    associate(&router, MEMBER_A, "restored").await;
    let listed = listed_events(&router, &json!({}), "restored").await;
    assert_eq!(
        event_ids(&listed),
        started,
        "case \"restored\": a re-associated member's event is reported again"
    );
    assert_summaries_cover(&router, &listed, "restored").await;
}
