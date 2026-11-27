// SPDX-License-Identifier: CC0-1.0

//! ONE remediation execution record: the run of a Config rule's remediation action
//! against one resource, and the `RemediationExecutionState` a status read reports it at.
//!
//! The record holds what Config DECIDED — the resource the execution is for, the target it
//! runs, the instant it was invoked and the instants its moves fall due at — and the run it
//! belongs to (`super::remediation`); the state is READ OFF those rather than written by
//! whoever reports it, which is what keeps a status read from moving an execution on.

use super::*;

/// One remediation execution: the run of a rule's remediation action against ONE
/// resource, as `DescribeRemediationExecutionStatus` reports it.
///
/// The execution is asynchronous, so what is stored is when it was invoked, the run it
/// belongs to ([`Self::automation`]) and the instants its two moves are due at; the state
/// it is in is READ OFF that run, or off the clock where there is none
/// ([`Self::state_at`]), rather than written by whoever reads it.
#[derive(Debug, Clone)]
pub struct RemediationExecutionEntry {
    /// The type of the remediated resource (`ResourceKey$resourceType`).
    pub resource_type: String,
    /// The id of the remediated resource (`ResourceKey$resourceId`).
    pub resource_id: String,
    /// The remediation target this execution runs — the automation document the rule's
    /// remediation configuration names (`RemediationConfiguration$TargetId`) — which is
    /// the one step the execution reports (`RemediationExecutionStep$Name`).
    pub step_name: String,
    /// When Config invoked the execution
    /// (`RemediationExecutionStatus$InvocationTime`), which is the instant of the call
    /// that started it.
    pub invoked_at: SystemTime,
    /// When the execution is due to leave `QUEUED`; see
    /// [`REMEDIATION_EXECUTION_STARTS_AFTER`].
    pub starts_at: SystemTime,
    /// When the execution is due to have finished; see
    /// [`REMEDIATION_EXECUTION_COMPLETES_AFTER`].
    pub completes_at: SystemTime,
    /// The RUN of this execution: the Systems Manager automation execution of the document
    /// the rule's remediation configuration names, and what that service has said about it
    /// (`crate::state::remediation`).
    ///
    /// The run is what the execution reports the progress of, so where there is one THAT
    /// is the state reported. It stays absent for an execution whose run Systems Manager
    /// could not be asked for at all — nothing is then known about any run — and the
    /// reported state follows the clock's own spans.
    pub automation: Option<RemediationAutomation>,
}

impl RemediationExecutionEntry {
    /// The execution `StartRemediationExecution` invoked at `at` for the resource
    /// `resource_type`/`resource_id`, running the target `step_name`.
    pub fn invoked(
        resource_type: &str,
        resource_id: &str,
        step_name: &str,
        at: SystemTime,
    ) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            step_name: step_name.to_string(),
            invoked_at: at,
            starts_at: at + REMEDIATION_EXECUTION_STARTS_AFTER,
            completes_at: at + REMEDIATION_EXECUTION_COMPLETES_AFTER,
            automation: None,
        }
    }

    /// The key this execution is stored under within the rule `config_rule_name`.
    pub fn key(&self, config_rule_name: &str) -> RemediationExecutionKey {
        (
            config_rule_name.to_string(),
            self.resource_type.clone(),
            self.resource_id.clone(),
        )
    }

    /// The `RemediationExecutionState` this execution is in at `now`.
    ///
    /// Where the execution HAS a run ([`Self::automation`]), that run is what the state
    /// reports — the remediation is the automation Systems Manager runs, so this execution
    /// has succeeded exactly when that automation has. Everything else follows the clock:
    /// queued until the instant it is due to start, running until the instant it is due to
    /// have finished, and succeeded from then on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if let Some(automation) = self.automation.as_ref() {
            return automation.state;
        }
        if now < self.starts_at {
            REMEDIATION_EXECUTION_STATE_QUEUED
        } else if now < self.completes_at {
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS
        } else {
            REMEDIATION_EXECUTION_STATE_SUCCEEDED
        }
    }

    /// Whether this execution has NOT finished yet, which is the precondition
    /// `StartRemediationExecution` states for a repeat call for the same resource key:
    /// "An existing StartRemediationExecution call for the specified resource keys must
    /// complete before you can call the API again."
    pub fn in_flight(&self, now: SystemTime) -> bool {
        matches!(
            self.state_at(now),
            REMEDIATION_EXECUTION_STATE_QUEUED | REMEDIATION_EXECUTION_STATE_IN_PROGRESS
        )
    }

    /// When this execution's run began: the instant Config asked for the run where it
    /// has one, and the instant the clock made the move due otherwise.
    pub(super) fn started_running_at(&self) -> SystemTime {
        self.automation
            .as_ref()
            .map(|automation| automation.started_at)
            .unwrap_or(self.starts_at)
    }

    /// When this execution's run ended; see [`Self::started_running_at`].
    pub(super) fn stopped_running_at(&self) -> SystemTime {
        self.automation
            .as_ref()
            .and_then(|automation| automation.ended_at)
            .unwrap_or(self.completes_at)
    }

    /// When this execution last moved (`RemediationExecutionStatus$LastUpdatedTime`):
    /// the instant of the most recent transition — the instant its own run moved it where
    /// this emulator ran it, and the instant the clock made the move due otherwise, which
    /// is the invocation itself while the execution is still queued.
    pub fn last_updated_at(&self, now: SystemTime) -> SystemTime {
        match self.state_at(now) {
            REMEDIATION_EXECUTION_STATE_QUEUED => self.invoked_at,
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS => self.started_running_at(),
            _ => self.stopped_running_at(),
        }
    }

    /// The modelled `RemediationExecutionStatus`, read back out of state at `now`.
    ///
    /// The one step is the target the remediation configuration names: it is pending
    /// while the execution is queued, running while the execution is, and succeeded
    /// once it has finished — and it carries an `ErrorMessage` only where the run it
    /// reports failed, which is the one thing that interrupted it.
    pub fn to_shape(&self, now: SystemTime) -> RemediationExecutionStatus {
        let state = self.state_at(now);
        let step_state = match state {
            REMEDIATION_EXECUTION_STATE_QUEUED => REMEDIATION_EXECUTION_STEP_STATE_PENDING,
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS => {
                REMEDIATION_EXECUTION_STEP_STATE_IN_PROGRESS
            }
            REMEDIATION_EXECUTION_STATE_FAILED => REMEDIATION_EXECUTION_STEP_STATE_FAILED,
            _ => REMEDIATION_EXECUTION_STEP_STATE_SUCCEEDED,
        };
        RemediationExecutionStatus {
            resource_key: Some(ResourceKey {
                resource_type: self.resource_type.clone(),
                resource_id: self.resource_id.clone(),
            }),
            state: Some(state.to_string()),
            step_details: Some(vec![RemediationExecutionStep {
                name: Some(self.step_name.clone()),
                state: Some(step_state.to_string()),
                error_message: self
                    .automation
                    .as_ref()
                    .and_then(|automation| automation.failure_message.clone()),
                // A step that has not started has no start instant to report, and one
                // that has not finished no stop instant.
                start_time: (state != REMEDIATION_EXECUTION_STATE_QUEUED)
                    .then(|| timestamp(self.started_running_at())),
                stop_time: (state == REMEDIATION_EXECUTION_STATE_SUCCEEDED
                    || state == REMEDIATION_EXECUTION_STATE_FAILED)
                    .then(|| timestamp(self.stopped_running_at())),
            }]),
            invocation_time: Some(timestamp(self.invoked_at)),
            last_updated_time: Some(timestamp(self.last_updated_at(now))),
        }
    }
}
