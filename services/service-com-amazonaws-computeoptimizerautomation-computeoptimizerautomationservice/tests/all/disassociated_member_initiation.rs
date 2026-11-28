// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED initiation leaves behind, and what a removed association does
//! NOT take away.
//!
//! The gate `member_account_initiation_gate` pins — automation work is initiated
//! in another account only while that account is currently associated — is about
//! INITIATING work. This suite pins its two consequences:
//!
//! * a removal takes nothing out of the STORE: the member account's analysed
//!   actions are still held, by the same ids, so the association alone decides
//!   what the organization retrieves (they come back, unchanged, the moment the
//!   account is associated again) and a start of one of them is refused for the
//!   caller's standing rather than answered "no such resource"; the automation
//!   events the account already ran stay readable by their ids throughout,
//!   because an event is a resource of its own, and the listing reports them
//!   again the moment the account is associated again;
//! * a refused start consumes nothing, its `clientToken` included, so the same
//!   request is served once the association is back and a retry of it answers
//!   that one automation instead of starting a second.
//!
//! Every call goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the
//! emulator binary mounts, through the harness `automation_rule_lifecycle` owns
//! and the opted-in router `recommended_action_inventory` owns. The helpers are
//! the ones `member_account_association_gate` owns, so the two suites drive the
//! service the same way.

use serde_json::json;

use super::automation_rule_lifecycle::{call_ok, string_member};
use super::member_account_association_gate::{
    action_ids, assert_access_denied, associate, disassociate, event_ids_in_account,
    listed_action_ids, start, MEMBER_A,
};
use super::recommended_action_inventory::opted_in_emulator;

/// A removal of an association leaves the member account's inventory and its
/// past automations exactly where they were IN THE STORE: an event the member
/// account already ran is still readable, the action ids still resolve (a start
/// of one is refused for the caller's standing, not answered "no such
/// resource"), and associating the account again reports the very same actions —
/// nothing was deleted, the organization's retrieval scope is simply no longer
/// over them.
#[tokio::test]
async fn a_removed_association_takes_nothing_out_of_the_store() {
    let router = opted_in_emulator("state").await;
    associate(&router, MEMBER_A, "associated").await;

    let listed_while_associated = listed_action_ids(&router, MEMBER_A, "associated").await;
    assert_eq!(
        listed_while_associated,
        action_ids(MEMBER_A),
        "case \"associated\": the member account's analysed actions must be listed first"
    );

    let action_id = listed_while_associated
        .first()
        .cloned()
        .expect("the analysis describes at least one volume");
    let (status, started) = start(&router, &action_id, None).await;
    assert_eq!(
        status, 200,
        "case \"associated\": the automation must run before the association is removed, got \
         {status} and {started}"
    );
    let event_id = string_member(&started, "eventId");

    disassociate(&router, MEMBER_A, "disassociated").await;

    // The event store is left alone, but the organization's reach over that
    // account is not: the event SELECTIONS stop covering it (the scope every
    // selection is narrowed by — `crate::disassociated_member_event_selection`)
    // while the event itself stays exactly where it was, reachable by its id.
    assert_eq!(
        event_ids_in_account(&router, MEMBER_A, "disassociated").await,
        Vec::<String>::new(),
        "case \"disassociated\": the listing must stop selecting an account the organization no \
         longer reaches"
    );
    let event = call_ok(
        &router,
        "GetAutomationEvent",
        &json!({"eventId": &event_id}),
        "disassociated",
    )
    .await;
    assert_eq!(
        string_member(&event, "accountId"),
        MEMBER_A,
        "case \"disassociated\": the event still reports the account it ran in, got {event}"
    );

    // The action is no longer automatable, which is the gate — and the refusal
    // is about the caller's standing, so the id still resolves.
    assert_access_denied(start(&router, &action_id, None).await, "disassociated");

    // Nothing was deleted: the association alone decided what was retrieved, so
    // the same actions come back the moment it is in place again.
    associate(&router, MEMBER_A, "re-associated").await;
    assert_eq!(
        listed_action_ids(&router, MEMBER_A, "re-associated").await,
        listed_while_associated,
        "case \"re-associated\": the removal kept the inventory the analysis produced"
    );
    assert_eq!(
        event_ids_in_account(&router, MEMBER_A, "re-associated").await,
        vec![event_id],
        "case \"re-associated\": the removal kept the automation the member account ran"
    );
}

/// A start the gate refuses consumes nothing: the `clientToken` it carried is
/// still fresh, so once the account is associated again the SAME request is
/// served, and a replay of it answers that one automation instead of starting a
/// second.
#[tokio::test]
async fn a_refused_start_consumes_no_client_token() {
    let router = opted_in_emulator("token").await;
    let action_id = action_ids(MEMBER_A)
        .into_iter()
        .next()
        .expect("the analysis describes at least one volume");
    // The model's `ClientToken` pattern is `^[a-zA-Z0-9_-]{1,64}$`.
    let token = "start-gate-token-1";

    // Associated, then removed: the action is stored, the standing is gone.
    associate(&router, MEMBER_A, "token").await;
    disassociate(&router, MEMBER_A, "token").await;
    assert_access_denied(start(&router, &action_id, Some(token)).await, "refused");

    associate(&router, MEMBER_A, "token").await;
    let (status, served) = start(&router, &action_id, Some(token)).await;
    assert_eq!(
        status, 200,
        "case \"re-associated\": the token the refused call carried must still be fresh, got \
         {status} and {served}"
    );
    let event_id = string_member(&served, "eventId");

    let (status, replayed) = start(&router, &action_id, Some(token)).await;
    assert_eq!(
        status, 200,
        "case \"replay\": the retry must be served, got {status} and {replayed}"
    );
    assert_eq!(
        string_member(&replayed, "eventId"),
        event_id,
        "case \"replay\": the retry must answer the automation the first call started, got \
         {replayed}"
    );
    assert_eq!(
        event_ids_in_account(&router, MEMBER_A, "replay").await,
        vec![event_id],
        "case \"replay\": the account must hold exactly the one automation that was started"
    );
}
