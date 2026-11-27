// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHOSE precondition the manual Amazon Web Services Support Center
//! console step is, in `RegisterSlackWorkspaceForOrganization`.
//!
//! The rule under test: the manual console authorization is the precondition of the
//! account that registers a Slack workspace FOR THE ORGANIZATION, and a member account is
//! exempt from it — "After the management account authorizes the Slack workspace, member
//! accounts can call this API to authorize the same Slack workspace for their individual
//! accounts. Member accounts don't need to authorize the Slack workspace manually through
//! the Amazon Web Services Support Center"
//! (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — so a
//! member account's call is decided only by whether the management account has registered
//! that workspace for the organization: served when it has, and otherwise the modelled
//! `ConflictException` (HTTP 409) that enumerates exactly that condition, "Call the
//! RegisterSlackWorkspaceForOrganization API from a member account, but the management
//! account hasn't registered that workspace yet for the organization"
//! (`smithy_query(query="doc",shape="ConflictException")`) — never the
//! `ResourceNotFoundException` that asks the caller for the console step.
//!
//! The same workspace, in the same account, is therefore answered differently depending on
//! the role the organization gives the caller: the console step's miss is the 404
//! [`crate::console_authorized_workspace`] covers for the account that has to take that
//! step, and the registry conflict here for the one the model exempts from it.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]; the only things
//! not driven over the wire are the console authorization itself and which account manages
//! the organization, neither of which any mounted operation can reach (see
//! `Emulator::authorize_workspace_in_console` and `Emulator::hand_organization_to`), and
//! each row reads the effect of its calls back through `ListSlackWorkspaceConfigurations`.
//!
//! The tests are tables. Their rows differ along what the rule generalises over: the
//! `teamId` form and length at both published `len 1..256` bounds, which account manages
//! the organization, whether the console authorization is held at all, how many other
//! workspaces the organization already has registered, and how many times the call is
//! repeated.

use serde_json::json;
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, ACCOUNT_TYPE_MEMBER, CONSOLE_AUTHORIZED_SLACK_WORKSPACES,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::{
    list_workspaces, reported, reported_team_ids, team_ids,
};

/// The operation under test, as its refusals name it.
const OPERATION: &str = "RegisterSlackWorkspaceForOrganization";

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation",operation="RegisterSlackWorkspaceForOrganization")`).
const TEAM_ID_MIN_LEN: usize = 1;
const TEAM_ID_MAX_LEN: usize = 256;

/// The channel a row configures when it reads the registry through the create path; the
/// rule under test is about the workspace, so one channel id is enough.
const CHANNEL_ID: &str = "C01234A5BCD";

/// The management accounts the tables hand the organization to, so the calling account is
/// a MEMBER of its organization: 12-digit account ids other than the one the emulator
/// presents.
fn other_management_accounts() -> Vec<String> {
    let caller = engine::emulator_account_id();
    let accounts = vec![
        "111122223333".to_string(),
        "999988887777".to_string(),
        "000000000001".to_string(),
    ];
    for account in &accounts {
        assert_ne!(
            account,
            caller,
            "a management account row must be an account other than the caller {caller}"
        );
        assert!(
            account.len() == 12 && account.chars().all(char::is_numeric),
            "a management account row must be a 12-digit account id, got {account:?}"
        );
    }
    accounts
}

/// The team ids the refusal table names: forms and lengths the rule generalises over,
/// including the shape the issue cited and both published `teamId` bounds. None is a
/// workspace the console authorized, which is the whole point — a member account's answer
/// must not turn on that.
fn team_ids_outside_the_console_authorizations() -> Vec<String> {
    let ids = vec![
        "T3790D96EA9".to_string(),
        "Y".repeat(TEAM_ID_MIN_LEN),
        "t-member.Case_7".to_string(),
        "Y".repeat(TEAM_ID_MAX_LEN),
    ];
    for id in &ids {
        assert!(
            !CONSOLE_AUTHORIZED_SLACK_WORKSPACES.contains(&id.as_str()),
            "a row must not be a console-authorized workspace, got {id:?}"
        );
        assert!(
            !team_ids().contains(id),
            "a row must not be a workspace the neighbour registrations use, got {id:?}"
        );
        assert!(
            (TEAM_ID_MIN_LEN..=TEAM_ID_MAX_LEN).contains(&id.chars().count())
                && !id.chars().any(char::is_whitespace),
            "a row must satisfy teamId's own constraints, got {id:?}"
        );
    }
    ids
}

/// Register `team_id` with whatever role the organization currently gives this account,
/// and with nothing arranged about the workspace — the call under test.
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .call(
            "register-slack-workspace-for-organization",
            json!({ "teamId": team_id }),
        )
        .await
}

/// Withdraw the Amazon Web Services Support Center console's authorization of `team_id`.
///
/// The console's authorization flow is not an API call, so neither granting nor
/// withdrawing one has a wire path: the set this service's handlers answer from is edited
/// through its state module, exactly as `Emulator::authorize_workspace_in_console` grants
/// one. Every row that does this reads the effect back over the wire.
fn revoke_console_authorization(emulator: &Emulator, team_id: &str) {
    let engine = emulator.engine();
    let state = service_com_amazonaws_supportapp_supportapp::state::service_state(&*engine)
        .expect("the Support App store must be reachable on the shared engine");
    state
        .lock()
        .console_authorized_slack_workspaces
        .remove(team_id);
}

/// Hand the organization to `master_account_id` and read the hand-over back over the wire
/// with `DescribeOrganization`, so the role every later call computes is the one the
/// Organizations store reports.
async fn organization_managed_by(emulator: &Emulator, label: &str, master_account_id: &str) {
    emulator.hand_organization_to(master_account_id);
    let described = emulator
        .organizations("DescribeOrganization", json!({}))
        .await;
    assert_eq!(
        described.body["Organization"]["MasterAccountId"].as_str(),
        Some(master_account_id),
        "case {label:?}: expected the organization to be managed by {master_account_id}, got {}",
        described.body
    );
}

/// The message template a caller the console step IS the precondition of is refused with:
/// fixed text, the request's `teamId` and the account the emulator presents.
fn console_step_message(team_id: &str) -> String {
    format!(
        "Slack workspace {team_id} not found for account {account}. Sign in to the Amazon Web \
         Services Support Center and authorize the Slack workspace before you register it.",
        account = engine::emulator_account_id()
    )
}

/// The message template the documented member-account conflict carries: fixed text, the
/// request's `teamId`, the account the emulator presents and the operation that was called.
fn member_conflict_message(team_id: &str) -> String {
    format!(
        "Account {account} is a member account of its organization, and the management \
         account hasn't registered Slack workspace {team_id} yet for the organization. The \
         management account must call the {OPERATION} API for a Slack workspace before a \
         member account can.",
        account = engine::emulator_account_id()
    )
}

/// Assert `reply` is the documented member-account conflict for `team_id`, and NOT the
/// refusal that asks for the manual console step.
fn assert_member_conflict(label: &str, team_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 409,
        "case {label:?}: expected HTTP 409, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ConflictException",
        "case {label:?}: expected x-amzn-errortype ConflictException, got {:?} / {}",
        reply.error_type, reply.body
    );
    let message = reply.string("message").unwrap_or_default();
    assert_eq!(
        message,
        member_conflict_message(team_id),
        "case {label:?}: expected the member-account conflict template, got {message:?}"
    );
    assert_ne!(
        message,
        console_step_message(team_id),
        "case {label:?}: a member account must not be asked for the manual console step"
    );
}

/// A create in the workspace `team_id` identifies, which is only possible once that
/// workspace is registered — the independent read of "is this workspace in the registry".
async fn create_channel(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .create(json!({
            "teamId": team_id,
            "channelId": CHANNEL_ID,
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await
}

#[tokio::test]
async fn a_member_account_registers_a_workspace_the_organization_registered_without_the_console_step(
) {
    let masters = other_management_accounts();
    for (index, team_id) in team_ids().into_iter().enumerate() {
        let master = masters[index % masters.len()].clone();
        let label = format!(
            "managed by {master}, team {}",
            &team_id[..team_id.len().min(16)]
        );
        let emulator = Emulator::new();

        // The management account takes the console step and registers the workspace for
        // the organization — the first of the two steps the operation documents.
        let registered = emulator.register_workspace(&team_id).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: the management account's registration must answer HTTP 200, \
             got {} / {}",
            registered.status, registered.body
        );

        // Now the console authorization is gone, so nothing about the console can be what
        // lets the member account's call through. That it really is gone is read back over
        // the wire: the account that has to take the console step is refused for the very
        // same workspace.
        revoke_console_authorization(&emulator, &team_id);
        let refused = register(&emulator, &team_id).await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: with no console authorization the management account's own \
             call must answer HTTP 404, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.string("message").unwrap_or_default(),
            console_step_message(&team_id),
            "case {label:?}: expected the console-step template, got {}",
            refused.body
        );

        organization_managed_by(&emulator, &label, &master).await;
        let before = list_workspaces(&emulator, None).await;

        // Twice: the member account's call is served every time, it authorizes the
        // workspace for that account alone, so it leaves the stored record alone.
        for attempt in 1..=2 {
            let served = register(&emulator, &team_id).await;
            assert_eq!(
                served.status, 200,
                "case {label:?} attempt {attempt}: a member account needs no console \
                 authorization for a workspace the management account registered, got {} / {}",
                served.status, served.body
            );
            assert_eq!(
                served.string("accountType").as_deref(),
                Some(ACCOUNT_TYPE_MEMBER),
                "case {label:?} attempt {attempt}: expected accountType \
                 {ACCOUNT_TYPE_MEMBER:?}, got {}",
                served.body
            );
            assert_eq!(
                served.string("teamId").as_deref(),
                Some(team_id.as_str()),
                "case {label:?} attempt {attempt}: expected the registered teamId, got {}",
                served.body
            );
            assert_eq!(
                served.string("teamName").as_deref(),
                Some(derived_slack_workspace_name(&team_id).as_str()),
                "case {label:?} attempt {attempt}: expected the recorded teamName, got {}",
                served.body
            );
        }

        let after = list_workspaces(&emulator, None).await;
        assert_eq!(
            after.body, before.body,
            "case {label:?}: a member account's registration must leave the stored record \
             as the management account's left it; the listing reported {} after having \
             reported {}",
            after.body, before.body
        );
    }
}

#[tokio::test]
async fn a_member_account_is_refused_with_the_documented_conflict_not_the_console_step() {
    let neighbours_available = team_ids();
    for master in other_management_accounts() {
        for team_id in team_ids_outside_the_console_authorizations() {
            // Rows differ in whether the console authorization is held at all — a member
            // account's answer must be the same either way — and in how many other
            // workspaces the organization already has registered, so no refusal is about
            // an empty registry.
            for (authorized_in_console, neighbours) in
                [(false, 0usize), (true, 0usize), (false, 2usize), (true, 2usize)]
            {
                let label = format!(
                    "managed by {master}, console authorization {authorized_in_console}, \
                     {neighbours} already registered, team {}",
                    &team_id[..team_id.len().min(16)]
                );
                let emulator = Emulator::new();
                for neighbour in neighbours_available.iter().take(neighbours) {
                    assert_eq!(
                        emulator.register_workspace(neighbour).await.status,
                        200,
                        "case {label:?}: the neighbour workspace {} must be registered by \
                         the management account",
                        &neighbour[..neighbour.len().min(16)]
                    );
                }
                // The organization has to exist before it can be handed over, so a row
                // that registers nothing still creates it.
                emulator.ensure_organization().await;
                if authorized_in_console {
                    emulator.authorize_workspace_in_console(&team_id);
                }
                organization_managed_by(&emulator, &label, &master).await;

                let before = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));

                // Twice: a refusal is not a state transition, so the second call is
                // refused exactly as the first.
                for attempt in 1..=2 {
                    let refused = register(&emulator, &team_id).await;
                    assert_member_conflict(
                        &format!("{label} attempt {attempt}"),
                        &team_id,
                        &refused,
                    );
                }

                // Nothing was registered: the registry is exactly what the management
                // account's registrations left.
                let after = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
                assert_eq!(
                    after, before,
                    "case {label:?}: a refused registration must register nothing, the \
                     listing reported {after:?} after having reported {before:?}"
                );
                let mut expected: Vec<String> =
                    neighbours_available.iter().take(neighbours).cloned().collect();
                expected.sort();
                assert_eq!(
                    after, expected,
                    "case {label:?}: expected only the workspaces the management account \
                     registered, got {after:?}"
                );

                // And the workspace really is unregistered: a channel configuration in it
                // still hits its own missing-workspace precondition.
                let channel = create_channel(&emulator, &team_id).await;
                assert_eq!(
                    channel.status, 409,
                    "case {label:?}: expected the channel create to hit the \
                     missing-workspace precondition, got {} / {}",
                    channel.status, channel.body
                );
            }
        }
    }
}

#[tokio::test]
async fn the_same_request_is_answered_by_the_role_the_organization_gives_the_caller() {
    // One workspace no console authorization recorded, one account, one request: the
    // account that has to take the console step is told to take it (404), and the account
    // the model exempts from it is told what its own answer depends on instead (409).
    let caller = engine::emulator_account_id();
    for team_id in team_ids_outside_the_console_authorizations() {
        for master in other_management_accounts() {
            let label = format!(
                "managed by {master}, team {}",
                &team_id[..team_id.len().min(16)]
            );
            let emulator = Emulator::new();
            emulator.ensure_organization().await;

            // The caller manages the organization: the console step is its own.
            let refused = register(&emulator, &team_id).await;
            assert_eq!(
                refused.status, 404,
                "case {label:?}: the management account must be asked for the console \
                 step, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ResourceNotFoundException",
                "case {label:?}: expected x-amzn-errortype ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            assert_eq!(
                refused.string("message").unwrap_or_default(),
                console_step_message(&team_id),
                "case {label:?}: expected the console-step template, got {}",
                refused.body
            );

            // The same request from a member account of the same organization: the model
            // exempts it from that step, so it is the registry that decides.
            organization_managed_by(&emulator, &label, &master).await;
            assert_member_conflict(&label, &team_id, &register(&emulator, &team_id).await);

            // Handed back: the console step is the caller's own again.
            organization_managed_by(&emulator, &label, caller).await;
            let refused = register(&emulator, &team_id).await;
            assert_eq!(
                refused.status, 404,
                "case {label:?}: the management account must be asked for the console step \
                 again once it manages the organization, got {} / {}",
                refused.status, refused.body
            );

            // Nothing any of those calls did registered a workspace.
            let listed = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
            assert!(
                listed.is_empty(),
                "case {label:?}: no refused registration may register a workspace, the \
                 listing reported {listed:?}"
            );
        }
    }
}
