// SPDX-License-Identifier: CC0-1.0

//! The rollback deployment CodeDeploy creates for a deployment that was stopped,
//! and the `RollbackInfo` that links the two records.
//!
//! ONE place, because a rollback is the same record whatever stopped the
//! deployment: a caller's `StopDeployment` asking for one, or an alarm the
//! deployment group polls ([`super::alarm_gate`]). Both go through
//! [`record_rollback`], so the two can never report a rollback differently.
//!
//! What the rollback record is: a deployment of the revision this deployment
//! group was on BEFORE the stopped deployment — "the ID of the deployment
//! rollback" (`smithy_query(query="shape",shape="RollbackInfo")`) — created by the
//! service rather than by a client, so it carries the `codeDeployRollback`
//! creator (`smithy_query(query="shape",shape="DeploymentCreator")`) and is then
//! run by the clock like any other deployment. Both records carry the same
//! `RollbackInfo`, whose `rollbackTriggeringDeploymentId` is "the deployment ID of
//! the deployment that was underway and triggered a rollback deployment because it
//! failed or was stopped".
//!
//! A deployment group whose FIRST deployment is the one being stopped has no
//! revision to go back to, so no rollback deployment is created for it; the
//! `RollbackInfo` then carries no `rollbackDeploymentId` and says why.
//!
//! What the rollback DOES, for a deployment that had already shifted traffic: it
//! re-points the Lambda alias at the revision it redeploys — the version the
//! stopped deployment started from — in ONE shift rather than in the increments
//! that deployment was walking. A rollback is CodeDeploy's own remediation for a
//! deployment that was stopped, so it puts the alias back rather than walking a
//! window again (which is also why [`super::alarm_gate`] leaves it ungated).

use crate::ops::types::RollbackInfo;

use super::{
    deployment_completion_due, deployment_start_due, lambda_traffic_shift, lifecycle_hook,
    revision_key, termination_wait, traffic_shift,
    ComAmazonawsCodedeployCodedeploy20141006State, Deployment, DeploymentGroup,
    DEPLOYMENT_CREATOR_ROLLBACK, DEPLOYMENT_STATUS_CREATED, DEPLOYMENT_STATUS_FAILED,
};
use crate::ops::types::AutoRollbackConfiguration;

/// The `AutoRollbackEvent` that asks for a rollback of a deployment that FAILED
/// (`smithy_query(query="shape",shape="AutoRollbackEvent")`: DEPLOYMENT_FAILURE |
/// DEPLOYMENT_STOP_ON_ALARM | DEPLOYMENT_STOP_ON_REQUEST).
pub const AUTO_ROLLBACK_EVENT_DEPLOYMENT_FAILURE: &str = "DEPLOYMENT_FAILURE";

/// The deployment group `deployment` was created against, when the store still
/// holds it.
///
/// The one lookup from a deployment to its group, so every question asked of a
/// deployment's group — its alarm configuration, its automatic rollback — is asked
/// of the same record.
pub fn group_of<'a>(
    state: &'a ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> Option<&'a DeploymentGroup> {
    let group_name = deployment.deployment_group_name.as_deref()?;
    state
        .applications
        .get(&deployment.application_name)?
        .deployment_groups
        .get(group_name)
}

/// The automatic rollback configuration `deployment` runs under: the deployment's
/// own, else its deployment group's — the same precedence the alarm configuration
/// is read with ("the alarm configuration that you specify here will override the
/// alarm configuration at the deployment group level",
/// `smithy_query(query="doc",shape="CreateDeploymentInput",member="overrideAlarmConfiguration")`).
pub fn effective_auto_rollback_configuration<'a>(
    state: &'a ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &'a Deployment,
) -> Option<&'a AutoRollbackConfiguration> {
    deployment.auto_rollback_configuration.as_ref().or_else(|| {
        group_of(state, deployment).and_then(|group| group.auto_rollback_configuration.as_ref())
    })
}

/// Whether `deployment` is rolled back when `event` happens to it: only an
/// ENABLED configuration that names that `AutoRollbackEvent` asks for it.
pub fn rolls_back_on(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
    event: &str,
) -> bool {
    effective_auto_rollback_configuration(state, deployment).is_some_and(|configuration| {
        configuration.enabled
            && configuration
                .events
                .iter()
                .flatten()
                .any(|named| named == event)
    })
}

/// Roll `deployment_id` back because it FAILED, when its configuration asks for a
/// rollback on that event — the one place a deployment failure is turned into a
/// rollback, so every way a deployment can fail rolls back the same way.
///
/// Nothing happens for a deployment that did not fail, one that already carries
/// rollback information, or one the SERVICE created to roll another one back: a
/// rollback is CodeDeploy's own remediation, so a rollback never rolls back a
/// rollback.
pub fn roll_back_on_failure(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    now_ms: i64,
) {
    let rolls_back = state
        .deployments
        .get(deployment_id)
        .is_some_and(|deployment| {
            deployment.status == DEPLOYMENT_STATUS_FAILED
                && deployment.creator != DEPLOYMENT_CREATOR_ROLLBACK
                && deployment.rollback_info.is_none()
                && rolls_back_on(state, deployment, AUTO_ROLLBACK_EVENT_DEPLOYMENT_FAILURE)
        });
    if rolls_back {
        record_rollback(state, deployment_id, now_ms);
    }
}

/// Roll `deployment_id` back: mint the rollback deployment this group has a
/// revision for, write the `RollbackInfo` onto BOTH records, and answer that
/// information so the caller can report it.
///
/// Call it for a deployment that is being stopped — the status and the completion
/// of the stopped deployment itself belong to whoever stops it, so nothing here
/// touches them. `None` means the store holds no such deployment.
pub fn record_rollback(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    now_ms: i64,
) -> Option<RollbackInfo> {
    // What a rollback of this deployment redeploys, and what the rollback
    // deployment inherits from it: a rollback goes to the same deployment group
    // with the same configuration, so all of it comes from the stored record.
    let deployment = state.deployments.get(deployment_id)?;
    let rollback_revision = deployment.previous_revision.clone();
    let rolled_back_revision = deployment.revision.clone();
    let application_name = deployment.application_name.clone();
    let deployment_group_name = deployment.deployment_group_name.clone();
    let deployment_config_name = deployment.deployment_config_name.clone();
    let compute_platform = deployment.compute_platform.clone();
    let deployment_style = deployment.deployment_style.clone();
    let auto_rollback_configuration = deployment.auto_rollback_configuration.clone();
    // A rollback runs in the Region the deployment it rolls back ran in.
    let region = deployment.region.clone();
    // A rollback goes to the SAME deployment group, so it resolves the same
    // deployment targets as the deployment it rolls back, and its terminal status
    // follows from them the same way.
    let mut targets = deployment.targets.clone();

    // The id of the rollback deployment, minted only when this deployment group
    // has a revision to go back to. A group whose first deployment is being
    // stopped has none, which is what the rollback message then reports.
    let rollback_deployment_id = rollback_revision
        .is_some()
        .then(|| state.mint_deployment_id());
    let rollback_info = match &rollback_deployment_id {
        Some(rollback_id) => RollbackInfo {
            rollback_deployment_id: Some(rollback_id.clone()),
            rollback_triggering_deployment_id: Some(deployment_id.to_string()),
            rollback_message: Some(format!(
                "Deployment {rollback_id} was created to roll deployment {deployment_id} back \
                 to the previously deployed revision"
            )),
        },
        None => RollbackInfo {
            rollback_deployment_id: None,
            rollback_triggering_deployment_id: Some(deployment_id.to_string()),
            rollback_message: Some(format!(
                "Deployment {deployment_id} cannot be rolled back: no revision was deployed to \
                 this deployment group before it"
            )),
        },
    };
    if let Some(deployment) = state.deployments.get_mut(deployment_id) {
        deployment.rollback_info = Some(rollback_info.clone());
        // A rollback of a deployment that had already moved some of the traffic
        // puts the alias back the way that deployment found it. That is owed by
        // the ROLLBACK itself, so it is owed whether or not this group also had a
        // revision to create a rollback deployment of: "if you stop a deployment,
        // the traffic is restored to the original version" is what a rollback of
        // a traffic shift means, and the version it is restored to is the one the
        // stopped deployment's own AppSpec was shifting from.
        deployment.awaits_alias_restore = deployment.lambda_traffic_shift.is_some()
            && deployment.traffic_increments_applied > 0;
    }

    // The rollback is a DEPLOYMENT of its own, created by the service rather than
    // by a client, which the clock then runs like any other.
    if let (Some(rollback_id), Some(revision)) = (rollback_deployment_id, rollback_revision) {
        // A rollback is a deployment to the SAME deployment group, so it owes the
        // original environment whatever that group configured, exactly as a
        // client's own deployment to it does.
        let (terminates_original_instances, termination_wait_millis) = deployment_group_name
            .as_deref()
            .and_then(|group_name| {
                state
                    .applications
                    .get(&application_name)
                    .and_then(|application| application.deployment_groups.get(group_name))
            })
            .map_or((false, 0), termination_wait);
        // The traffic shift the rollback performs is the one the revision it
        // redeploys describes, read exactly as a client's own deployment of that
        // revision reads it: a rollback moves the alias back by re-pointing it,
        // rather than by undoing anything.
        let declared_shift = lambda_traffic_shift::declared_shift(&compute_platform, &revision);
        // The lifecycle event hooks the revision it REDEPLOYS declares, read
        // exactly as a client's own deployment of that revision reads them: a
        // rollback runs the lifecycle events of the revision it puts back.
        let hook_executions = lifecycle_hook::declared_executions(&compute_platform, &revision);
        // A rollback redeploys a revision of its own, so it reaches for that
        // revision's object exactly as a client's own deployment of it would.
        let awaits_revision_fetch = super::s3_revision::awaits_fetch(&revision);
        // The rollback deploys the revision it redeploys, so its Lambda target is
        // the function THAT revision names.
        super::name_lambda_targets(&mut targets, declared_shift.as_ref());
        state.record_deployment(Deployment {
            deployment_id: rollback_id,
            application_name,
            deployment_group_name,
            deployment_config_name,
            // The revision's identity, computed once here so no later request
            // re-derives it from the revision's content.
            revision_key: revision_key(&revision),
            revision: Some(revision),
            // The revision this group was on when the rollback started: the one
            // the stopped deployment was deploying.
            previous_revision: rolled_back_revision,
            status: DEPLOYMENT_STATUS_CREATED.to_string(),
            creator: DEPLOYMENT_CREATOR_ROLLBACK.to_string(),
            description: None,
            ignore_application_stop_failures: false,
            update_outdated_instances_only: false,
            file_exists_behavior: None,
            deployment_mode: None,
            auto_rollback_configuration,
            // Nothing a client sent to the stopped deployment carries over: a
            // rollback targets the group's own instances with the group's own
            // settings.
            target_instances: None,
            override_alarm_configuration: None,
            ec2_deployment_options: None,
            deployment_style,
            compute_platform,
            external_id: None,
            rollback_info: Some(rollback_info.clone()),
            create_time_ms: now_ms,
            start_time_ms: None,
            complete_time_ms: None,
            starts_at_ms: deployment_start_due(now_ms),
            completes_at_ms: deployment_completion_due(now_ms) + termination_wait_millis,
            // Nothing has failed, and no lifecycle event hook has reported
            // against the rollback yet.
            error_information: None,
            lifecycle_hook_executions: Vec::new(),
            hook_executions,
            targets,
            terminates_original_instances,
            termination_wait_millis,
            instance_termination_wait_time_started: false,
            lambda_traffic_shift: declared_shift,
            awaits_revision_fetch,
            // The rollback performs its own shift; it has nothing of its own to
            // put back.
            awaits_alias_restore: false,
            traffic_shift: lambda_traffic_shift::TrafficShiftProgress::Declared,
            // In ONE shift: a rollback undoes, so it re-points the alias at the
            // revision it redeploys instead of walking the increments the
            // deployment it rolls back was walking.
            traffic_increments: traffic_shift::single_shift(),
            traffic_increments_applied: 0,
            region,
            // The rollback is a deployment of its own, so its own lifecycle
            // events are published as it reaches them.
            published_trigger_events: Vec::new(),
        });
    }

    Some(rollback_info)
}
