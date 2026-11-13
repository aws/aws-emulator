// SPDX-License-Identifier: CC0-1.0

//! The TRAFFIC SHIFT an Lambda deployment performs: the alias move its revision
//! declares, carried out in AWS Lambda itself, increment by increment.
//!
//! A deployment of the `Lambda` compute platform deploys a revision that IS the
//! AppSpec file — "the content includes a Lambda function name, the alias for its
//! original version, and the alias for its replacement version. The deployment
//! shifts traffic from the original version of the Lambda function to the
//! replacement version"
//! (`smithy_query(query="shape",shape="AppSpecContent")`). So the shift, not the
//! status, is what such a deployment DOES: the terminal `Succeeded`
//! (`smithy_query(query="shape",shape="DeploymentStatus")`) says the shift the
//! revision described has happened in full.
//!
//! Three modules answer three questions, and this one joins them:
//!
//! * WHAT is shifted — the function, the alias and the two versions — is read out
//!   of the revision ONCE when the deployment is recorded ([`super::appspec`]);
//! * IN WHICH INCREMENTS, and when each falls due, comes from the deployment's
//!   resolved `trafficRoutingConfig` ([`super::traffic_shift`]);
//! * the WRITE to the alias is [`super::lambda_alias`]'s, against the Lambda
//!   emulator's own function store.
//!
//! This module is the one that CARRIES a declared shift out: [`apply_due_shifts`]
//! applies whichever increment the clock owes a deployment at this instant, before
//! the clock may give that deployment a successful outcome
//! ([`super::with_state`]), and fails the deployment when the shift cannot be
//! carried out — a deployment that moved no alias never reports success for one.
//! It also holds the rollback's own move, which puts an alias back the way the
//! deployment it rolls back found it.
//!
//! Like [`super::alarm_gate`], it runs with THIS store unlocked while the other
//! service's store is locked: the two are never held at once.

use crate::ops::types::ErrorInformation;

use super::lambda_alias::{AliasMove, Unresolved};
use super::traffic_shift;
use super::{
    deployment_is_terminal, now_ms, service_state, ComAmazonawsCodedeployCodedeploy20141006State,
    Deployment, DEPLOYMENT_STATUS_FAILED, DEPLOYMENT_STATUS_IN_PROGRESS,
};

pub use super::appspec::{
    declared_in_revision, declared_shift, LambdaTrafficShift, COMPUTE_PLATFORM_LAMBDA,
};

/// The `ErrorCode` a deployment that could not carry out its Lambda traffic shift
/// fails with, because the function, the alias or the version its AppSpec names is
/// not one AWS Lambda holds
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it).
pub const ERROR_CODE_INVALID_LAMBDA_FUNCTION: &str = "INVALID_LAMBDA_FUNCTION";

/// How far a deployment's declared traffic shift has got.
///
/// What keeps a deployment from being given a successful outcome before its shift
/// has happened: the clock settles no deployment whose shift is
/// [`TrafficShiftProgress::InFlight`]
/// ([`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`]), so a
/// caller that reads `Succeeded` reads it after the alias moved — including a
/// caller racing the pass that moved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficShiftProgress {
    /// Declared by the revision, and the next increment not yet carried out.
    Declared,
    /// Claimed by a pass that is carrying an increment out in AWS Lambda right
    /// now.
    InFlight,
    /// Carried out in full: the alias invokes the target version.
    Applied,
    /// Attempted and refused, which is what the deployment then fails for.
    Refused,
}

/// One increment claimed by a pass, with everything the write needs.
#[derive(Debug, Clone)]
struct Claim {
    /// The deployment the increment belongs to.
    deployment_id: String,
    /// The function whose alias moves, which is what a refusal names.
    function_name: String,
    /// The alias that moves.
    alias_name: String,
    /// The write this increment is.
    alias_move: AliasMove,
    /// Which increment of the schedule it is, and whether it completes the move.
    index: usize,
    /// See [`Claim::index`].
    completes_the_move: bool,
}

/// Carry out the traffic increment of every deployment the clock owes one at this
/// instant, in AWS Lambda itself.
///
/// Runs before the clock may settle a deployment ([`super::with_state`]), so no
/// caller can read an Lambda deployment that reached `Succeeded` while the alias
/// its revision named still invokes the version it invoked before, and so a
/// caller reading the alias during the shift's window sees the increment that was
/// due. Each increment is a transition the CLOCK owed — it fell due at that
/// instant whether or not anybody called — so carrying one out from a reporting
/// operation shows a caller nothing a later caller would not also see.
///
/// A store holding no deployment that owes an increment costs one read of this
/// store and no Lambda lock at all.
pub fn apply_due_shifts<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = now_ms();
    let (claimed, restores) = {
        let mut store = state.lock();
        (
            claim_due_increments(&mut store, now),
            claim_due_restorations(&mut store),
        )
    };
    // The traffic a rolled-back deployment had already moved goes back first: the
    // alias ends up where that deployment found it, not where a later increment
    // of it would have put it.
    for restore in &restores {
        let _ = super::lambda_alias::move_alias(
            engine,
            &restore.function_name,
            &restore.alias_name,
            &restore.alias_move,
        );
    }
    if claimed.is_empty() {
        return;
    }
    // The alias is moved with THIS store unlocked, and what each attempt came to
    // is written back under it.
    let attempted: Vec<(Claim, Result<(), Unresolved>)> = claimed
        .into_iter()
        .map(|claim| {
            let moved = super::lambda_alias::move_alias(
                engine,
                &claim.function_name,
                &claim.alias_name,
                &claim.alias_move,
            );
            (claim, moved)
        })
        .collect();
    let mut store = state.lock();
    for (claim, outcome) in &attempted {
        match outcome {
            Ok(()) => record_applied(&mut store, claim),
            // WHICH of the function, the version and the alias could not be
            // resolved does not change the outcome CodeDeploy reports (see
            // [`Denial::of`]), so that the move was refused is all that is read
            // here.
            Err(_) => fail_deployment(&mut store, claim, &Denial::of(claim), now),
        }
    }
}

/// Record that `claim`'s increment has been carried out, which is what lets the
/// next one fall due and — once the last of them is applied — lets the clock
/// settle the deployment.
fn record_applied(state: &mut ComAmazonawsCodedeployCodedeploy20141006State, claim: &Claim) {
    let Some(deployment) = state.deployments.get_mut(&claim.deployment_id) else {
        return;
    };
    deployment.traffic_increments_applied = claim.index + 1;
    deployment.traffic_shift = if claim.completes_the_move {
        TrafficShiftProgress::Applied
    } else {
        TrafficShiftProgress::Declared
    };
}

/// What one alias RESTORATION puts back, and where.
#[derive(Debug, Clone)]
struct Restore {
    /// The function whose alias goes back.
    function_name: String,
    /// The alias that goes back.
    alias_name: String,
    /// The write that puts it back: onto the version the deployment was shifting
    /// FROM, with the weighted routing the shift added cleared.
    alias_move: AliasMove,
}

/// The alias restoration every rolled-back deployment that owes one owes, CLAIMED
/// as this pass's to carry out: the flag is cleared under this same lock, so one
/// rollback puts the alias back once however many passes run.
///
/// A deployment whose AppSpec named no `CurrentVersion` still has the weighted
/// routing its increments added cleared, which is the part of the move this
/// service knows it made; the alias then keeps invoking whatever it invokes.
fn claim_due_restorations(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
) -> Vec<Restore> {
    let owing: Vec<String> = state
        .deployments
        .values()
        .filter(|deployment| deployment.awaits_alias_restore)
        .map(|deployment| deployment.deployment_id.clone())
        .collect();
    let mut claimed = Vec::new();
    for deployment_id in owing {
        let Some(deployment) = state.deployments.get_mut(&deployment_id) else {
            continue;
        };
        deployment.awaits_alias_restore = false;
        if let Some(declared) = deployment.lambda_traffic_shift.as_ref() {
            claimed.push(Restore {
                function_name: declared.function_name.clone(),
                alias_name: declared.alias_name.clone(),
                alias_move: AliasMove {
                    function_version: declared.current_version.clone(),
                    additional_version_weights: ::std::collections::BTreeMap::new(),
                },
            });
        }
    }
    claimed
}

/// The increment every deployment that is owed one at `now_ms` owes, CLAIMED as
/// this pass's to carry out: the deployment's shift is marked
/// [`TrafficShiftProgress::InFlight`] before the alias is touched, so two callers
/// racing through here cannot move the same alias twice, and neither can settle
/// the deployment until the pass that claimed it has written back what the move
/// came to.
///
/// A deployment is owed an increment once that increment's own instant has passed
/// and while the deployment has not reached an outcome — one a caller stopped, or
/// the alarms its group polls stopped, is owed nothing further.
fn claim_due_increments(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<Claim> {
    let pending: Vec<String> = state.pending_deployments.clone();
    let mut claimed = Vec::new();
    for deployment_id in pending {
        let Some(deployment) = state.deployments.get(&deployment_id) else {
            continue;
        };
        let Some(claim) = due_claim(&deployment_id, deployment, now_ms) else {
            continue;
        };
        if let Some(deployment) = state.deployments.get_mut(&deployment_id) {
            deployment.traffic_shift = TrafficShiftProgress::InFlight;
        }
        claimed.push(claim);
    }
    claimed
}

/// The claim `deployment` owes at `now_ms`, or `None` when it owes none; see
/// [`claim_due_increments`].
fn due_claim(deployment_id: &str, deployment: &Deployment, now_ms: i64) -> Option<Claim> {
    if deployment.traffic_shift != TrafficShiftProgress::Declared
        || deployment.complete_time_ms.is_some()
        || deployment_is_terminal(&deployment.status)
        // Traffic does not move while a lifecycle event hook that runs BEFORE it
        // has not reported `Succeeded`: that is what such a hook is for — "the
        // function validates lifecycle hooks during a deployment" — so the shift
        // waits for the validation function's own report (see
        // `super::lifecycle_hook`).
        || !super::lifecycle_hook::traffic_is_allowed(deployment)
    {
        return None;
    }
    let declared = deployment.lambda_traffic_shift.as_ref()?;
    let (index, increment) = traffic_shift::due_increment(deployment, now_ms)?;
    Some(Claim {
        deployment_id: deployment_id.to_string(),
        function_name: declared.function_name.clone(),
        alias_name: declared.alias_name.clone(),
        alias_move: increment.alias_move(declared),
        index,
        completes_the_move: index + 1 == deployment.traffic_increments.len(),
    })
}

/// Why a deployment's traffic shift could not be carried out, as CodeDeploy
/// reports it on `DeploymentInfo.errorInformation`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Denial {
    /// An `ErrorCode`; see [`ERROR_CODE_INVALID_LAMBDA_FUNCTION`].
    code: &'static str,
    /// The accompanying message, naming the resource the AppSpec asked for.
    message: String,
}

impl Denial {
    /// The denial for a Lambda resource an attempted move could not resolve.
    ///
    /// ONE denial for all three: WHICH of the function, the version and the alias
    /// was missing is deliberately not disclosed (see
    /// [`invalid_lambda_function_message`]), so the claim's own function name is
    /// all that goes into it.
    fn of(claim: &Claim) -> Self {
        Self {
            code: ERROR_CODE_INVALID_LAMBDA_FUNCTION,
            message: invalid_lambda_function_message(&claim.function_name),
        }
    }
}

/// The message a deployment whose declared traffic shift names a Lambda resource
/// AWS Lambda does not hold fails with.
///
/// A FIXED sentence whose only variable part is the function the AppSpec's
/// `Resources` entry names: CodeDeploy reports the same message whether the
/// function, the version or the alias was the thing it could not find, and names
/// neither the version nor the alias. One sentence for the three cases is the
/// contract — a client triaging a failed release matches on it, and on exactly
/// that ambiguity — so nothing here narrows it down.
#[must_use]
pub fn invalid_lambda_function_message(function_name: &str) -> String {
    format!(
        "The deployment failed because either the target Lambda function {function_name} does \
         not exist or the specified function version or alias cannot be found"
    )
}

/// Fail `claim`'s deployment for a shift that could not be carried out, with the
/// error information CodeDeploy reports for it.
///
/// This store was unlocked while the alias was reached, so what the claim was made
/// from is checked again: a deployment another caller stopped in between keeps the
/// outcome it already has. A deployment the clock had not yet started also gets the
/// start it was owed, so a deployment that failed before anybody read it still
/// reports the instant it began.
fn fail_deployment(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    claim: &Claim,
    denial: &Denial,
    now_ms: i64,
) {
    let deployment_id = claim.deployment_id.as_str();
    let Some(deployment) = state.deployments.get_mut(deployment_id) else {
        return;
    };
    deployment.traffic_shift = TrafficShiftProgress::Refused;
    if deployment.complete_time_ms.is_some() || deployment_is_terminal(&deployment.status) {
        return;
    }
    if deployment.start_time_ms.is_none() {
        deployment.start_time_ms = Some(deployment.starts_at_ms);
        deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
    }
    state.settle_deployment(
        deployment_id,
        DEPLOYMENT_STATUS_FAILED,
        Some(ErrorInformation {
            code: Some(denial.code.to_string()),
            message: Some(denial.message.clone()),
        }),
        now_ms,
    );
    // A deployment that failed is rolled back when its configuration asks for
    // that on a deployment failure, from the one place that is decided.
    super::rollback::roll_back_on_failure(state, deployment_id, now_ms);
}
