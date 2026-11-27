// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Slack workspace `RegisterSlackWorkspaceForOrganization` acts
//! on having to EXIST first.
//!
//! The rule under test: an operation that acts on the resource its request identifies
//! settles that resource's existence in this account's state before any precondition on
//! the CALLING ACCOUNT, and answers a miss with the modelled `ResourceNotFoundException`
//! (HTTP 404) — "The specified resource is missing or doesn't exist, such as an account
//! alias, Slack channel configuration, or Slack workspace configuration"
//! (`smithy_query(query="operation",operation="RegisterSlackWorkspaceForOrganization",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`).
//! For this operation the resource is the Slack workspace itself: a workspace reaches
//! this API through the interactive flow in the Amazon Web Services Support Center
//! console — "Sign in to the Amazon Web Services Support Center and authorize the Slack
//! workspaces where you want your organization to have access to", after which the API
//! "authorize[s] each Slack workspace for the organization"
//! (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — so for a
//! caller the model asks to take that console step a `teamId` no console authorization
//! recorded is that 404, never the `ConflictException` (409) of an account topology, and
//! nothing is registered.
//!
//! The caller the model EXEMPTS from the console step — a member account, which "[doesn't]
//! need to authorize the Slack workspace manually through the Amazon Web Services Support
//! Center" — is therefore outside this rule, and is covered in
//! [`crate::member_account_console_step_exemption`].
//!
//! So the tables below run the rule over what it generalises over, with the recorded
//! `T<10 uppercase hex>` case as a single row:
//!
//! * the `teamId` form and length, at both published `len 1..256` bounds;
//! * the ACCOUNT TOPOLOGY the lookup happens in, among those the console step is the
//!   caller's own — the default account, which belongs to no organization, and an
//!   organization the caller manages — because the resource miss is settled ahead of
//!   every one of them;
//! * what the account holds BESIDES the named workspace: nothing, or other workspaces
//!   authorized and registered;
//! * how many times the call is repeated, since a refusal is not a state transition.
//!
//! The served side of the rule is here too: the workspace the API's own documentation
//! names (the one entry of
//! `service_com_amazonaws_supportapp_supportapp::state::CONSOLE_AUTHORIZED_SLACK_WORKSPACES`,
//! which the issue did not cite) registers over the wire with nothing arranged beyond the
//! organization, and a console authorization outlives the registration it let through, so
//! a deleted workspace configuration can be registered again.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]; the only thing
//! not driven over the wire is the console authorization itself, which is not an API call
//! (see `Emulator::authorize_workspace_in_console`), and every row reads its effect back
//! through `ListSlackWorkspaceConfigurations`.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, CONSOLE_AUTHORIZED_SLACK_WORKSPACES,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::{
    list_workspaces, reported, reported_team_ids, team_ids,
};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation",operation="RegisterSlackWorkspaceForOrganization")`).
const TEAM_ID_MIN_LEN: usize = 1;
const TEAM_ID_MAX_LEN: usize = 256;

/// Which organization, if any, holds the calling account when the lookup happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topology {
    /// The default emulator account: it belongs to no organization at all — the recorded
    /// case, and the one whose membership `ConflictException` used to mask this miss.
    NoOrganization,
    /// An organization the calling account manages.
    ManagedByTheCaller,
}

/// Put the account in the topology `topology` names, with `neighbours` other workspaces
/// authorized in the console beside the one the row names — and registered too wherever
/// the topology allows a registration at all, so no row's answer can come from an account
/// that holds nothing.
///
/// The organization itself is created by driving the Organizations emulator's own
/// `CreateOrganization`.
async fn arrange(emulator: &Emulator, label: &str, topology: Topology, neighbours: &[String]) {
    for neighbour in neighbours {
        emulator.authorize_workspace_in_console(neighbour);
    }
    if topology == Topology::NoOrganization {
        // An account in no organization cannot register anything, so its neighbours are
        // the console authorizations alone.
        return;
    }
    emulator.ensure_organization().await;
    for neighbour in neighbours {
        assert_eq!(
            register(emulator, neighbour).await.status,
            200,
            "case {label:?}: the neighbour workspace {} must register, so the row's answer \
             is not about an empty registry",
            &neighbour[..neighbour.len().min(16)]
        );
    }
}

/// Register `team_id` with nothing arranged about the workspace — the call under test.
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .call(
            "register-slack-workspace-for-organization",
            json!({ "teamId": team_id }),
        )
        .await
}

/// A create in the workspace `team_id` identifies, which is only possible once that
/// workspace is registered — the independent read of "is this workspace in the registry".
async fn create_channel(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .create(json!({
            "teamId": team_id,
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await
}

/// The template a miss on the Slack workspace a registration names is reported with:
/// fixed text, the request's `teamId` and the account the emulator presents.
fn not_found(team_id: &str) -> String {
    format!(
        "Slack workspace {team_id} not found for account {account}. Sign in to the Amazon Web \
         Services Support Center and authorize the Slack workspace before you register it.",
        account = engine::emulator_account_id()
    )
}

/// Assert `reply` is the modelled `ResourceNotFoundException` for a workspace no console
/// authorization recorded: HTTP 404, that error type on the wire, and the one message
/// template.
fn assert_not_found(label: &str, team_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {label:?}: expected HTTP 404, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {label:?}: expected x-amzn-errortype ResourceNotFoundException, got {:?} / {}",
        reply.error_type, reply.body
    );
    let message = reply.string("message").unwrap_or_default();
    assert_eq!(
        message,
        not_found(team_id),
        "case {label:?}: expected the workspace-miss template, got {message:?}"
    );
}

/// The team ids the miss table names: forms and lengths the rule generalises over,
/// including the recorded `T<10 uppercase hex>` shape and both published `teamId` bounds.
/// None is an entry of the console-authorized set, which is what makes each a miss.
fn never_authorized_team_ids() -> Vec<String> {
    let ids = vec![
        // The recorded case: one row of many.
        "T3790D96EA9".to_string(),
        "Z".repeat(TEAM_ID_MIN_LEN),
        "t012abcdefg".to_string(),
        "T-other.Case_8".to_string(),
        "T012ABCDEFG-2".to_string(),
        "Z".repeat(TEAM_ID_MAX_LEN),
    ];
    for id in &ids {
        assert!(
            !CONSOLE_AUTHORIZED_SLACK_WORKSPACES.contains(&id.as_str()),
            "a miss row must not be a console-authorized workspace, got {id:?}"
        );
        assert!(
            !team_ids().contains(id),
            "a miss row must not be a workspace the neighbour rows authorize, got {id:?}"
        );
        assert!(
            (TEAM_ID_MIN_LEN..=TEAM_ID_MAX_LEN).contains(&id.chars().count())
                && !id.chars().any(char::is_whitespace),
            "a miss row must satisfy teamId's own constraints, got {id:?}"
        );
    }
    ids
}

#[tokio::test]
async fn a_workspace_no_console_authorization_recorded_is_not_found() {
    for topology in [Topology::NoOrganization, Topology::ManagedByTheCaller] {
        for team_id in never_authorized_team_ids() {
            // Rows differ in how much the account holds besides the named workspace: an
            // empty account, and one already holding other authorized, registered
            // workspaces, so no answer can come from an empty registry.
            for neighbours in [0usize, 2] {
                let label = format!(
                    "{topology:?}, {neighbours} other workspaces, team {}",
                    &team_id[..team_id.len().min(16)]
                );
                let emulator = Emulator::new();
                let others: Vec<String> = team_ids().into_iter().take(neighbours).collect();
                arrange(&emulator, &label, topology, &others).await;

                let before = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));

                // Twice: a refusal is not a state transition, so the second call is
                // refused exactly as the first.
                for attempt in 1..=2 {
                    let refused = register(&emulator, &team_id).await;
                    assert_not_found(
                        &format!("{label} attempt {attempt}"),
                        &team_id,
                        &refused,
                    );
                }

                // Nothing was registered: the registry is exactly what it was.
                let after = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
                assert_eq!(
                    after, before,
                    "case {label:?}: a refused registration must register nothing, the \
                     listing reported {after:?} after having reported {before:?}"
                );

                // And the workspace really is unregistered: a channel configuration in it
                // still hits its own missing-workspace precondition, which is the
                // CONFLICT that is not the answer to this request.
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
async fn the_documented_authorized_workspace_registers_over_the_wire() {
    // The console authorizations a fresh account starts from, named by the state module
    // that seeds them from the API's own documentation ("This ID uniquely identifies a
    // Slack workspace, such as T012ABCDEFG"). Each registers with nothing arranged beyond
    // the organization the operation requires, which is the one wire-reachable served
    // path — the console's interactive flow is not an API call.
    assert!(
        !CONSOLE_AUTHORIZED_SLACK_WORKSPACES.is_empty(),
        "a fresh account must start from at least one console authorization"
    );
    for team_id in CONSOLE_AUTHORIZED_SLACK_WORKSPACES {
        let label = format!("documented workspace {team_id}");
        let emulator = Emulator::new();

        // Before the organization exists the SAME request is refused for the caller's
        // topology, not for the workspace: the resource is there, the account is not in an
        // organization.
        let refused = register(&emulator, team_id).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: expected the membership conflict for an authorized workspace, \
             got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case {label:?}: expected x-amzn-errortype ConflictException, got {:?}",
            refused.error_type
        );

        emulator.ensure_organization().await;
        let registered = register(&emulator, team_id).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.string("teamId").as_deref(),
            Some(team_id),
            "case {label:?}: expected the registered teamId, got {}",
            registered.body
        );
        assert_eq!(
            registered.string("teamName").as_deref(),
            Some(derived_slack_workspace_name(team_id).as_str()),
            "case {label:?}: expected the recorded teamName, got {}",
            registered.body
        );

        // Read back through the listing: the registration is persisted, not echoed.
        let listed = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
        assert_eq!(
            listed,
            vec![team_id.to_string()],
            "case {label:?}: expected the registry to hold exactly the registered \
             workspace, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_console_authorization_outlives_the_registration_it_let_through() {
    // Deleting a Slack workspace CONFIGURATION removes the account's registration; it
    // revokes no console authorization, so the same workspace registers again — while a
    // workspace that was never authorized is still not found, before and after.
    for team_id in team_ids() {
        let label = format!("team {}", &team_id[..team_id.len().min(16)]);
        let emulator = Emulator::new();

        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the authorized workspace must register"
        );
        let deleted = emulator.delete_workspace(&team_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: the workspace configuration must delete, got {} / {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            reported(&list_workspaces(&emulator, None).await),
            Vec::<Value>::new(),
            "case {label:?}: the delete must leave the registry empty"
        );

        let again = register(&emulator, &team_id).await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: a workspace the console authorized registers again after its \
             configuration was deleted, got {} / {}",
            again.status, again.body
        );

        // The authorization is per workspace, so another workspace is still a miss.
        let never = never_authorized_team_ids()
            .into_iter()
            .next_back()
            .expect("the miss table must not be empty");
        assert_not_found(&label, &never, &register(&emulator, &never).await);
    }
}
