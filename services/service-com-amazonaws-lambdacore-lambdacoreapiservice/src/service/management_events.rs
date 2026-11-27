// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `lambda.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it.
//!
//! Every control-plane API call of an account is a management event — event history holds
//! the management calls of every service whether or not a trail exists — which makes the
//! recording a property of SERVING a call and not of any one operation. So it is not
//! written per handler: the shared wrapper ([`service_common::audited_operation`]) is handed
//! the deserialized input before the handler runs, the handler's own `Result` after, and the
//! rendered response last, and this module is only what THIS service contributes to each
//! record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the request members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`MANAGEMENT_EVENT_OPERATIONS`].
//!
//! Every modelled operation reaches the table this module's recorder is wired into
//! ([`recording_table`], filled where `EmulatedService::new` lists the whole operation
//! surface and answered from `Service::override_operation`), so no request this service
//! claims can be served by a path that records nothing.
//!
//! The `eventName` is the operation's own modelled name, which is the shared wrapper's
//! default: this model binds every operation under an API-date segment per resource family
//! and CloudTrail publishes a call of it under the operation name alone, unlike the Lambda
//! control plane whose published names carry the API date of each operation's own REST
//! binding.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, PreDispatchProtocol};

/// The `eventSource` every record of this service carries.
///
/// The model's own answer and not a name assembled from the service or crate name:
/// `smithy_query(query="service_info")` reports
/// `awsService.cloudTrailEventSource: lambda.amazonaws.com` — the SAME source the Lambda
/// control plane declares, because both are AWS Lambda: two services audit under one event
/// source and stay apart by their `eventName` and their `eventID`.
pub const EVENT_SOURCE: &str = "lambda.amazonaws.com";

/// The request members this service's records carry the redaction literal for instead of the
/// value the caller sent: none. This model declares no `@sensitive` member
/// (`smithy_query(query="digest")` over its whole surface), so nothing a request carries is
/// withheld from its record.
pub const SENSITIVE_MEMBERS: &[&str] = &[];

/// Every operation of this service and the read/write class of its calls.
///
/// The CLASS is the operation's own semantics, which is what the event's `readOnly` carries
/// and what a trail's `ReadWriteType` narrows a trail's management events by: `Reporting` for
/// the operations the model declares `@readonly` — the `Get…` and `List…` calls, which report
/// state and change none of it — and `Mutating` for every one that creates, updates, deletes
/// or restores a resource.
///
/// Public, so this crate's own tests read the catalog the routing table is built from rather
/// than a second copy of it.
pub const MANAGEMENT_EVENT_OPERATIONS: &[(&str, CallKind)] = &[
    ("CreateNetworkConnector", CallKind::Mutating),
    ("CreateRuntime", CallKind::Mutating),
    ("CreateRuntimeAlias", CallKind::Mutating),
    ("CreateRuntimeVersion", CallKind::Mutating),
    ("DeleteNetworkConnector", CallKind::Mutating),
    ("DeleteRuntime", CallKind::Mutating),
    ("DeleteRuntimeAlias", CallKind::Mutating),
    ("DeleteRuntimeVersion", CallKind::Mutating),
    ("GetNetworkConnector", CallKind::Reporting),
    ("GetRuntime", CallKind::Reporting),
    ("GetRuntimeAlias", CallKind::Reporting),
    ("GetRuntimeVersion", CallKind::Reporting),
    ("ListNetworkConnectorVersions", CallKind::Reporting),
    ("ListNetworkConnectors", CallKind::Reporting),
    ("ListRuntimeAliases", CallKind::Reporting),
    ("ListRuntimeVersions", CallKind::Reporting),
    ("ListRuntimes", CallKind::Reporting),
    ("RestoreRuntime", CallKind::Mutating),
    ("UpdateNetworkConnector", CallKind::Mutating),
    ("UpdateRuntime", CallKind::Mutating),
    ("UpdateRuntimeAlias", CallKind::Mutating),
    ("UpdateRuntimeVersion", CallKind::Mutating),
];

/// The read/write class CloudTrail records the calls of `operation` as a management event
/// with, or `None` for a name this service's catalog does not hold.
#[must_use]
pub fn recorded_class(operation: &str) -> Option<CallKind> {
    MANAGEMENT_EVENT_OPERATIONS
        .iter()
        .find(|(named, _)| *named == operation)
        .map(|(_, kind)| *kind)
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in: this
/// service speaks `restJson1` (`smithy_query(query="service_info")` ->
/// `protocols: ["restJson1"]`), whose errors carry `application/json` and name their shape
/// in `x-amzn-errortype` — the same values the generated per-operation shim passes, so a
/// refusal raised ahead of a handler reads exactly as it did before.
pub const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/json",
    errortype_header: true,
};

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail's `LookupEvents` reads — so it is generic over the engine type exactly as the
/// service is.
pub struct NetworkConnectorAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> NetworkConnectorAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        crate::state::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for NetworkConnectorAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in event history by
            // definition, so it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's. The
        // guard is BOUND rather than used as a temporary: a local is released before the
        // handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read off the request the way every handler of
        // this service reads it — which is also the Region of every ARN a call mints.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in an
    // event's `Resources` are not this change's to invent — a record naming a resource
    // nothing measured would be reported to a lookup by `ResourceName` as fact — so a record
    // of a call to this service carries the call, its parameters and its outcome, and no
    // resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session — and as the role that issued it: the shared store the
        // minting service wrote it to, read the one way this service reads it.
        crate::state::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }
}

/// The empty AUDIT table this service's operations are served through, recording into the
/// account's management-event log.
///
/// The ROWS are filed where the whole operation surface of this service is already listed —
/// `EmulatedService::new`, through its own `wrap_operations!` — because every row carries
/// this service's own per-operation wrapper as well, and one list cannot drift from itself.
/// What lives here is the recorder each of those rows records through and the envelope their
/// pre-dispatch failures render in.
pub fn recording_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> AuditedTable<NetworkConnectorAudit<Eng>> {
    let recorder = Arc::new(NetworkConnectorAudit::new(Arc::clone(engine)));
    AuditedTable::new(recorder, JSON_ENVELOPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a call is recorded as a READ exactly when the model declares the operation
    /// `@readonly`, and as a write otherwise; a name this service does not serve has no
    /// class at all.
    ///
    /// The rows span both halves of the rule over BOTH resource families this service holds
    /// — the connector's reads and writes, and the runtime tree's reads, creates, updates,
    /// deletes and its restore — rather than the one create that motivated it, plus a name
    /// this model does not bind.
    #[test]
    fn a_readonly_operation_is_recorded_as_a_read_and_every_other_as_a_write() {
        let rows = [
            ("the create", "CreateNetworkConnector", Some(CallKind::Mutating)),
            ("the update", "UpdateNetworkConnector", Some(CallKind::Mutating)),
            ("the delete", "DeleteNetworkConnector", Some(CallKind::Mutating)),
            ("the read of one connector", "GetNetworkConnector", Some(CallKind::Reporting)),
            ("the list over the account", "ListNetworkConnectors", Some(CallKind::Reporting)),
            (
                "the list over one connector's versions",
                "ListNetworkConnectorVersions",
                Some(CallKind::Reporting),
            ),
            ("the runtime create", "CreateRuntime", Some(CallKind::Mutating)),
            ("the runtime update", "UpdateRuntime", Some(CallKind::Mutating)),
            ("the runtime delete", "DeleteRuntime", Some(CallKind::Mutating)),
            ("the restore", "RestoreRuntime", Some(CallKind::Mutating)),
            ("the runtime version create", "CreateRuntimeVersion", Some(CallKind::Mutating)),
            ("the runtime version update", "UpdateRuntimeVersion", Some(CallKind::Mutating)),
            ("the runtime version delete", "DeleteRuntimeVersion", Some(CallKind::Mutating)),
            ("the runtime alias create", "CreateRuntimeAlias", Some(CallKind::Mutating)),
            ("the runtime alias update", "UpdateRuntimeAlias", Some(CallKind::Mutating)),
            ("the runtime alias delete", "DeleteRuntimeAlias", Some(CallKind::Mutating)),
            ("the read of one runtime", "GetRuntime", Some(CallKind::Reporting)),
            ("the read of one version", "GetRuntimeVersion", Some(CallKind::Reporting)),
            ("the read of one alias", "GetRuntimeAlias", Some(CallKind::Reporting)),
            ("the list over the account's runtimes", "ListRuntimes", Some(CallKind::Reporting)),
            (
                "the list over one runtime's versions",
                "ListRuntimeVersions",
                Some(CallKind::Reporting),
            ),
            (
                "the list over one runtime's aliases",
                "ListRuntimeAliases",
                Some(CallKind::Reporting),
            ),
            ("a name this service does not serve", "PublishLayerVersion", None),
        ];
        for (case, operation, expected) in rows {
            let actual = recorded_class(operation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the recorded class of {operation} to be {expected:?}, \
                 got {actual:?}"
            );
        }
    }
}
