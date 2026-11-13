// SPDX-License-Identifier: CC0-1.0

//! This front door's DATA leg: the calls on the CONTENTS of a state machine, which
//! CloudTrail records as events of the `Data` category rather than as management events.
//!
//! CloudTrail classes the calls of an account in two, and the two reach different readers.
//! The ADMINISTRATION of this service's resources — creating, reading, changing and
//! removing a state machine, an alias, a version or an activity, and starting an
//! asynchronous execution the account then holds — is a management event, recorded in event
//! history where `LookupEvents` reports it ([`super::management_events`]). A call that RUNS
//! a state machine and hands the run's own data straight back to its caller acts on the
//! machine's contents instead, so it is a DATA event:
//!
//! * it is NOT event history ("data events are not logged by default when you create a
//!   trail", and a lookup serves management and Insights events), so no such call appears in
//!   a lookup at all;
//! * it is NOT published onto the default event bus, which carries management events; and
//! * it is delivered to a trail's BUCKET only while one of that trail's advanced event
//!   selectors admits it — which such a selector states by naming the `Data` category and
//!   the TYPE of the resource the call acted on ([`STATE_MACHINE_RESOURCE_TYPE`]), and may
//!   narrow further by that resource's ARN.
//!
//! Which class a call falls in is stated once, where this service's routing table is built
//! ([`super::management_events::audited_table`]): the record itself is built from the typed
//! call by the shared recorder either way ([`service_common::audited_operation`]), so a data
//! event reports the same parameters, identity and outcome a management event of this
//! service would, with the members this model declares `@sensitive` — a run's `input` among
//! them — carrying the redaction literal rather than the caller's own workflow data.
//!
//! This module is the two things that are THIS service's: which of its operations are data
//! events and which resource each one acts on ([`data_event_resources`]), and the delivery
//! of one such record into the account's trails ([`StateMachineDataEvents`]).

use std::sync::Arc;
use std::time::SystemTime;

use service_common::data_events::DataEventDelivery;
use service_common::management_events::{recorded_member_name, AuditedResource, ServedApiCall};
use service_common::trail_log_files::DeliveredLogFile;

/// The type a state machine is named by in a delivered record's `resources` and in an
/// advanced event selector's `resources.type` field: the resource type CloudTrail publishes
/// for the data events of this service.
pub const STATE_MACHINE_RESOURCE_TYPE: &str = "AWS::StepFunctions::StateMachine";

/// The operations of this service CloudTrail records as DATA events, each with the input
/// member naming the state machine the call acts on.
///
/// One row, and the reason it is one: this API has a single SYNCHRONOUS data plane. "Starts
/// a Synchronous Express state machine execution" (`StartSyncExecution`) runs the machine
/// and answers the run's own `output`, `error` and `cause` to the caller, which is a call on
/// that machine's contents; every other operation that starts work —
/// `StartExecution`, `RedriveExecution`, `TestState` — answers a resource identifier or a
/// report the account then holds, and is administration of the account's resources. The
/// member is the one the request names the machine in
/// (`smithy_query(query="operation",operation="StartSyncExecution",model_path="services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json")`
/// -> `StartSyncExecutionInput$stateMachineArn`, required).
///
/// Public, so this crate's own tests read the catalog the routing table is built from rather
/// than a second copy of it.
pub const DATA_EVENT_OPERATIONS: &[(&str, &str)] = &[("StartSyncExecution", "stateMachineArn")];

/// Whether CloudTrail records the calls of `operation` as DATA events.
#[must_use]
pub fn is_data_event(operation: &str) -> bool {
    DATA_EVENT_OPERATIONS
        .iter()
        .any(|(named, _)| *named == operation)
}

/// The resources a call of `operation` acted on, read off the parameters its record carries
/// — the state machine the request addressed, by the ARN it addressed it with.
///
/// A record names the resource a data event acted on because that is what a trail's selector
/// SELECTS the event by: `resources.type` names the class of data events a trail logs, and
/// `resources.ARN` narrows it to one machine, so a record naming no resource is admitted by
/// no such selector. A management call of this service names none (see
/// [`super::management_events`]), so this answers a resource only for the operations above.
///
/// An absent, non-text or empty member names no resource rather than one built from a value
/// the request did not carry.
#[must_use]
pub fn data_event_resources(
    operation: &str,
    request_parameters: &serde_json::Value,
) -> Vec<AuditedResource> {
    DATA_EVENT_OPERATIONS
        .iter()
        .filter(|(named, _)| *named == operation)
        .filter_map(|(_, member)| {
            request_parameters
                .get(recorded_member_name(member))
                .and_then(serde_json::Value::as_str)
                .filter(|arn| !arn.is_empty())
                .map(|arn| AuditedResource::new(STATE_MACHINE_RESOURCE_TYPE, arn.to_string()))
        })
        .collect()
}

/// What this service contributes to the delivery of one of its data events: the log files
/// the account's trails deliver it as.
///
/// Holds the shared engine — the trail projection the delivery is decided by lives in the
/// log that engine carries, the one CloudTrail recomputes whenever a trail of the account is
/// created, reconfigured, started or stopped — so it is generic over the engine type exactly
/// as the service is.
pub struct StateMachineDataEvents<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> StateMachineDataEvents<Eng> {
    /// Build the deliverer from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> DataEventDelivery for StateMachineDataEvents<Eng> {
    fn delivered(
        &self,
        call: ServedApiCall,
        served_at: SystemTime,
        answered_as: &str,
    ) -> Vec<DeliveredLogFile> {
        let Some(log) = super::management_events::management_event_log(&*self.engine) else {
            // Not silent: an undelivered data event leaves no trace in the account's trail
            // by definition, so it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the data-plane call was served \
                 with no trail able to deliver it"
            );
            return Vec::new();
        };
        // Nothing is RECORDED: a data event is not event history, so the log is only asked
        // which of the account's trails deliver this record. The guard is BOUND rather than
        // used as a temporary: a local is released before the handle it borrows, which a
        // temporary at the end of this block would not be.
        let delivering = log.lock();
        delivering.data_event_log_files(call, served_at, answered_as)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN of a state machine of this account, as a request addresses one.
    fn machine(name: &str) -> String {
        crate::state::state_machine_arn("us-east-1", name)
    }

    /// RULE: a data event's record names the state machine the request addressed, by its
    /// ARN and under the data-event resource type; a call of any other operation names no
    /// resource.
    #[test]
    fn a_data_event_names_the_state_machine_the_request_addressed() {
        let subject = machine("subject");
        // The rows differ along the operation, along whether the request carried the member
        // naming the machine, and along the form of the value it carried.
        let rows: Vec<(&str, &str, serde_json::Value, Vec<AuditedResource>)> = vec![
            (
                "the synchronous data plane, addressing a machine",
                "StartSyncExecution",
                serde_json::json!({"stateMachineArn": subject, "input": "{}"}),
                vec![AuditedResource::new(
                    STATE_MACHINE_RESOURCE_TYPE,
                    subject.clone(),
                )],
            ),
            (
                "the synchronous data plane, addressing a qualified machine",
                "StartSyncExecution",
                serde_json::json!({"stateMachineArn": format!("{subject}:PROD")}),
                vec![AuditedResource::new(
                    STATE_MACHINE_RESOURCE_TYPE,
                    format!("{subject}:PROD"),
                )],
            ),
            (
                "the asynchronous start, which is administration",
                "StartExecution",
                serde_json::json!({"stateMachineArn": subject, "input": "{}"}),
                Vec::new(),
            ),
            (
                "a read of the same machine",
                "DescribeStateMachine",
                serde_json::json!({"stateMachineArn": subject}),
                Vec::new(),
            ),
            (
                "a data call whose member carries no text at all",
                "StartSyncExecution",
                serde_json::json!({"stateMachineArn": ""}),
                Vec::new(),
            ),
            (
                "a data call whose member carries a value of another type",
                "StartSyncExecution",
                serde_json::json!({"stateMachineArn": 7}),
                Vec::new(),
            ),
            (
                "a data call carrying no such member",
                "StartSyncExecution",
                serde_json::json!({"input": "{}"}),
                Vec::new(),
            ),
        ];
        for (case, operation, request_parameters, expected) in rows {
            let actual = data_event_resources(operation, &request_parameters);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the record to name {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the operations recorded as data events are exactly the catalog, and every one
    /// of them is an operation this service serves.
    #[test]
    fn only_the_cataloged_operations_are_data_events() {
        for (operation, _) in DATA_EVENT_OPERATIONS {
            assert!(
                is_data_event(operation),
                "case {operation:?}: a cataloged operation is a data event"
            );
            assert!(
                super::super::identity_authorization::served_operation(operation).is_some(),
                "case {operation:?}: a cataloged operation must be one this service serves"
            );
        }
        for operation in ["StartExecution", "DescribeStateMachine", "SendTaskSuccess"] {
            assert!(
                !is_data_event(operation),
                "case {operation:?}: a control-plane call is no data event"
            );
        }
    }
}
