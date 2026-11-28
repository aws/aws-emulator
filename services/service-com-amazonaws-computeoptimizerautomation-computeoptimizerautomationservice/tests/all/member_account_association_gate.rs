// SPDX-License-Identifier: CC0-1.0

//! The ASSOCIATION gate on initiating automation work, across a member
//! account's WHOLE INVENTORY and across SEVERAL accounts at once.
//!
//! The rule under test is the one `member_account_initiation_gate` pins for one
//! action per association state: the two operations that initiate automation
//! work in an account — the on-demand automation of a recommended action, and
//! the rollback of an automation event — act in an account OTHER than the
//! caller's only while that account is currently associated. This suite takes
//! that rule across the dimensions one row of that table cannot show: EVERY
//! recommended action of a member account rather than its first, so the gate is
//! the account's standing and not a property of one action; and several member
//! accounts in one organization, so a removal is judged per account and leaves
//! another member's actions, and the caller's own, exactly as reachable as they
//! were.
//!
//! Every call goes over the wire: an awsJson1_0 `POST /` through the same
//! [`MultiServiceRouter`] the emulator binary mounts, through the harness
//! `automation_rule_lifecycle` owns and the opted-in router
//! `recommended_action_inventory` owns (opting in over the wire is what records
//! the caller's own analysed inventory).

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{assert_refused, call, call_ok, member, string_member};
use super::recommended_action_inventory::opted_in_emulator;

/// Member account ids in the model's `AccountId` form (`^[0-9]{12}$`), none of
/// them the calling account.
pub(crate) const MEMBER_A: &str = "444455556666";
const MEMBER_B: &str = "777788889999";

/// Associate `account_id` over the wire.
pub(crate) async fn associate(router: &MultiServiceRouter, account_id: &str, case: &str) {
    call_ok(
        router,
        "AssociateAccounts",
        &json!({"accountIds": [account_id]}),
        case,
    )
    .await;
}

/// Remove `account_id`'s association over the wire.
pub(crate) async fn disassociate(router: &MultiServiceRouter, account_id: &str, case: &str) {
    call_ok(
        router,
        "DisassociateAccounts",
        &json!({"accountIds": [account_id]}),
        case,
    )
    .await;
}

/// The `recommendedActionId`s the analysis of `account_id`'s resources produces.
/// The inventory is a pure function of the account id, so a case can name every
/// action of an account without listing it.
pub(crate) fn action_ids(account_id: &str) -> Vec<String> {
    state::recommended_action_inventory(account_id)
        .into_iter()
        .map(|(action_id, _)| action_id)
        .collect()
}

/// The `recommendedActionId`s `ListRecommendedActions` reports for one account,
/// walked page by page so the whole set is seen however it is paged.
pub(crate) async fn listed_action_ids(
    router: &MultiServiceRouter,
    account_id: &str,
    case: &str,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"filters": [{"name": "AccountId", "values": [account_id]}]});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListRecommendedActions", &request, case).await;
        walked.extend(
            member(&page, "recommendedActions")
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: recommendedActions is a list, got {page}"))
                .iter()
                .map(|action| string_member(action, "recommendedActionId")),
        );
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return walked;
        }
        assert!(
            walked.len() <= 1_000,
            "case {case:?}: the walk must terminate, walked {} actions",
            walked.len()
        );
    }
}

/// The `eventId`s `ListAutomationEvents` reports for one account.
pub(crate) async fn event_ids_in_account(
    router: &MultiServiceRouter,
    account_id: &str,
    case: &str,
) -> Vec<String> {
    let listed = call_ok(
        router,
        "ListAutomationEvents",
        &json!({"filters": [{"name": "AccountId", "values": [account_id]}]}),
        case,
    )
    .await;
    member(&listed, "automationEvents")
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| string_member(event, "eventId"))
                .collect()
        })
        .unwrap_or_default()
}

/// `StartAutomationEvent` for one action, with an optional client token.
pub(crate) async fn start(
    router: &MultiServiceRouter,
    action_id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"recommendedActionId": action_id});
    if let Some(token) = client_token {
        request["clientToken"] = json!(token);
    }
    call(router, "StartAutomationEvent", &request).await
}

/// Assert a call was refused with the access-denied answer the gate gives a
/// caller that may not initiate work in the account.
pub(crate) fn assert_access_denied((status, body): (u16, Value), case: &str) {
    assert_refused((status, body.clone()), 403, "AccessDeniedException", case);
    assert_eq!(
        string_member(&body, "message"),
        state::ACCESS_DENIED_MESSAGE,
        "case {case:?}: the refusal must carry this service's access-denied wording, got {body}"
    );
}

/// EVERY recommended action of a member account is automatable while the account
/// is associated, and every one of them is refused once it is not — the gate is
/// the account's standing, not a property of the one action a request names.
/// The caller's own actions are reachable throughout.
#[tokio::test]
async fn the_gate_covers_every_action_of_a_member_account() {
    let router = opted_in_emulator("inventory").await;
    associate(&router, MEMBER_A, "associated").await;

    let inventory = action_ids(MEMBER_A);
    assert!(
        inventory.len() > 1,
        "case \"associated\": the inventory must span more than one action for this case to \
         cover the whole of it, got {inventory:?}"
    );

    for action_id in &inventory {
        let case = format!("associated-{action_id}");
        let (status, body) = start(&router, action_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an associated member's action is automated, got {status} and {body}"
        );
    }
    let events_while_associated = event_ids_in_account(&router, MEMBER_A, "associated").await;
    assert_eq!(
        events_while_associated.len(),
        inventory.len(),
        "case \"associated\": each action started one automation in that account, got \
         {events_while_associated:?}"
    );

    disassociate(&router, MEMBER_A, "disassociated").await;

    for action_id in &inventory {
        let case = format!("disassociated-{action_id}");
        assert_access_denied(start(&router, action_id, None).await, &case);
    }
    // The refused starts minted nothing: the event listing no longer selects
    // that account at all while the association is gone
    // (`crate::disassociated_member_event_selection`), so the events it holds
    // are counted again once the association is back — and they are exactly the
    // ones the served starts created.
    associate(&router, MEMBER_A, "re-associated").await;
    assert_eq!(
        event_ids_in_account(&router, MEMBER_A, "re-associated").await,
        events_while_associated,
        "case \"re-associated\": the refused starts minted no further automation event"
    );
    disassociate(&router, MEMBER_A, "disassociated-again").await;

    for action_id in action_ids(&state::caller_account_id()) {
        let case = format!("own-{action_id}");
        let (status, body) = start(&router, &action_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the caller's own action is automated whatever a member account's \
             association does, got {status} and {body}"
        );
    }
}

/// The gate is judged PER ACCOUNT: in an organization with two associated member
/// accounts, removing one account's association refuses only that account's
/// actions, while the other member's actions and the caller's own stay
/// automatable — and each served automation runs in the account its action is
/// about.
#[tokio::test]
async fn a_removal_refuses_only_the_account_it_names() {
    let router = opted_in_emulator("two-members").await;
    associate(&router, MEMBER_A, "two-members").await;
    associate(&router, MEMBER_B, "two-members").await;

    disassociate(&router, MEMBER_A, "two-members").await;

    // (case, the account whose actions are named, whether a start is served)
    let rows: [(&str, String, bool); 3] = [
        ("removed-member", MEMBER_A.to_string(), false),
        ("kept-member", MEMBER_B.to_string(), true),
        ("caller", state::caller_account_id(), true),
    ];

    for (case, account_id, served) in rows {
        for action_id in action_ids(&account_id) {
            let row = format!("{case}-{action_id}");
            let answer = start(&router, &action_id, None).await;
            if served {
                let (status, body) = answer;
                assert_eq!(
                    status, 200,
                    "case {row:?}: the removal of another account's association must not reach \
                     this one, got {status} and {body}"
                );
                let event = call_ok(
                    &router,
                    "GetAutomationEvent",
                    &json!({"eventId": string_member(&body, "eventId")}),
                    &row,
                )
                .await;
                assert_eq!(
                    string_member(&event, "accountId"),
                    account_id,
                    "case {row:?}: the automation runs in the account the action is about, got \
                     {event}"
                );
            } else {
                assert_access_denied(answer, &row);
            }
        }
        if !served {
            assert_eq!(
                event_ids_in_account(&router, &account_id, case).await,
                Vec::<String>::new(),
                "case {case:?}: nothing was started in the account the removal names"
            );
        }
    }
}
