// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is recorded as an event of
//! `states.amazonaws.com`, where CloudTrail reports it — a management event in the
//! account's event history for the administration of its resources, and a DATA event of the
//! machine a call ran for this service's synchronous data plane ([`super::data_events`]).
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler here. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs, the handler's own `Result` after, and the
//! rendered response last, and this module is only what THIS service contributes to each
//! record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the session a call's temporary credential belongs to, so a call made with a role
//!   session is attributed to that session and to the role that issued it;
//! * the members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with
//! ([`service_common::audited_operation::AuditedDefinition`]) — the same table the
//! identity gate in front of the operations ([`super::identity_authorization`]) resolves
//! each admitted request through, so no request can be served by a path that records
//! nothing.
//!
//! Two kinds of answer are NOT records, both for the shared wrapper's own reason — event
//! history is the record of the calls an account's APIs actually SERVED:
//!
//! * a call the scaffold refused as not implemented (answered 501), which reached no
//!   handler;
//! * a request refused before any handler ran — a body that does not deserialize, a member
//!   outside a modelled constraint, and the identity-based refusal this service takes in
//!   front of the routing table ([`super::identity_authorization`]), which is decided
//!   without the typed call a record is built from ever happening.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{
    AuditedResource, ManagementEventLog, RecordedSession, ServedApiCall, SessionAuditDirectory,
};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// The `eventSource` every record of this service carries.
///
/// The model's own answer and not a name assembled here:
/// `smithy_query(query="service_info")` reports
/// `awsService.cloudTrailEventSource: states.amazonaws.com` (the same
/// `<endpointPrefix>.amazonaws.com` form CloudTrail stamps on every event).
pub const EVENT_SOURCE: &str = "states.amazonaws.com";

/// The members this service's records carry CloudTrail's redaction literal for instead of
/// the value the call carried.
///
/// The withholding is by member NAME — that is how the shared recorder states it, so one
/// entry answers for every operation that carries a member of that name, in a request and
/// in a response alike. The set is therefore exactly the names this model declares
/// `@sensitive` WHEREVER they appear: a name the model also uses for a plain member cannot
/// be withheld without hiding a value the API reports (`result`, the mocked result of a
/// tested state and the `OK`/`FAIL` result code of a validated definition, is the one such
/// name), and a name it marks everywhere must be, down to the members nested inside a
/// tested state's inspection report.
///
/// Derived from the model and not transcribed judgement:
/// `smithy_query(query="shape",shape="<Shape>",model_path="services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json")`
/// reports `sensitive: true` on each of them, and this crate's own test re-derives the whole
/// set from the model (`tests/all/management_event_recording.rs`). They hold a caller's own
/// workflow data, the text of a definition, and the token that lets a task be settled, so
/// the value is replaced before anything is stored and the plaintext is retrievable from
/// event history nowhere.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    // `SensitiveData` / `SensitiveDataJobInput`: a workflow's own payloads.
    "input",
    "output",
    "variables",
    "variableReferences",
    "context",
    "mapItemReaderData",
    "redriveStatusReason",
    // `SensitiveError` / `SensitiveCause`: the failure a caller states or is told.
    "error",
    "cause",
    // `Definition`: the Amazon States Language text of a state machine or a tested state.
    "definition",
    // `VersionDescription` / `AliasDescription`.
    "description",
    "versionDescription",
    // `TestStateStateName`: the state a test, or a mocked error, names.
    "stateName",
    "errorCausedByState",
    // The inspection report a tested state answers, and the members nested inside it: the
    // data-flow stages of the state under test, the HTTP exchange of an HTTP Task, the
    // handled-exception detail, and the diagnostics of a validated definition.
    "inspectionData",
    "afterArguments",
    "afterInputPath",
    "afterParameters",
    "afterResultSelector",
    "afterResultPath",
    "afterItemsPath",
    "afterItemSelector",
    "afterItemBatcher",
    "afterItemsPointer",
    "catchIndex",
    "retryIndex",
    "retryBackoffIntervalSeconds",
    "code",
    "location",
    "parameters",
    // `TaskToken`: holding one is what lets a task be settled.
    "taskToken",
];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct StepFunctionsAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> StepFunctionsAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for StepFunctionsAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session — and as the role that issued it, never as the role the
        // state machine the call acted on runs under: the shared store the minting service
        // wrote it to.
        session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it — which is also the Region of every ARN the call mints.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // The pairs CloudTrail reports in an event's `Resources` are not invented here: a
    // record naming a resource nothing resolved would be reported to a lookup by
    // `ResourceName` as fact, so a record of a MANAGEMENT call of this service carries the
    // call, its parameters and its outcome, and no resource list. A DATA event is the one
    // record that must name one, because a trail's selector is what SELECTS it by resource
    // type and ARN ([`super::data_events`]).
    fn resources(
        &self,
        operation: &str,
        request_parameters: &serde_json::Value,
        region: &str,
    ) -> Vec<AuditedResource> {
        let _ = region;
        super::data_events::data_event_resources(operation, request_parameters)
    }

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type ([`service_common::management_events::ManagementEventLog`]) and not a
/// field of this service's own state: CloudTrail reports the management calls of every
/// service, and the service that served a call is the only one that knows it happened, so
/// recorder and reporter reach one log rather than two copies of it.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The account's SESSION directory in that same shared engine: the sessions the token
/// service minted, each keyed by the access key id it was minted under, which is what lets
/// a record of a call made with one name the session it was made by.
///
/// A SHARED type for the same reason the log is: the service that MINTS a session and the
/// services that record the calls made WITH it are not the same service.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<SessionAuditDirectory>)
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsJson1_0` (`smithy_query(query="digest")` reports the one
/// protocol `awsJson1_0`), which is the same value the generated per-operation shim passes.
const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.0",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// The kind per row names the shared table's own method for it
/// ([`AuditedTable::reports`] / [`AuditedTable::mutates`]), so nothing about what a kind
/// means is restated here.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.$kind(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
            );
        )*
    }};
}

/// Every modelled operation of this service, each constructed from the shared engine and
/// served behind the audit recorder.
///
/// EVERY one is here — the 37 operations `smithy_query(query="digest")` lists — so an
/// operation is auditable because this service serves it and not because someone
/// remembered to add it.
///
/// The kind per row is the operation's own documented semantics: `reports` for the
/// `Describe*` / `List*` readers and for `GetExecutionHistory`, which "returns the history
/// of the specified execution" and changes none of it, and for
/// `ValidateStateMachineDefinition`, which "validates the syntax of a state machine
/// definition" without creating one; `mutates` for every operation that creates, starts,
/// settles, changes or removes something the account then holds — `GetActivityTask`
/// included, since it is "used by workers to retrieve a task" and hands that task out,
/// which no later poll can see again, and `TestState`, which "accepts the definition of a
/// single state and executes it".
///
/// The CLASS per row is CloudTrail's: every row below is a management event except this
/// service's synchronous data plane, which is served as a DATA event of the state machine it
/// runs ([`super::data_events`]) — recorded in no event history and delivered only into the
/// bucket of a trail whose selectors admit it.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(StepFunctionsAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, JSON_ENVELOPE);
    audited!(
        table, engine,
        create_activity::CreateActivity => mutates,
        create_state_machine::CreateStateMachine => mutates,
        create_state_machine_alias::CreateStateMachineAlias => mutates,
        delete_activity::DeleteActivity => mutates,
        delete_state_machine::DeleteStateMachine => mutates,
        delete_state_machine_alias::DeleteStateMachineAlias => mutates,
        delete_state_machine_version::DeleteStateMachineVersion => mutates,
        describe_activity::DescribeActivity => reports,
        describe_execution::DescribeExecution => reports,
        describe_map_run::DescribeMapRun => reports,
        describe_state_machine::DescribeStateMachine => reports,
        describe_state_machine_alias::DescribeStateMachineAlias => reports,
        describe_state_machine_for_execution::DescribeStateMachineForExecution => reports,
        get_activity_task::GetActivityTask => mutates,
        get_execution_history::GetExecutionHistory => reports,
        list_activities::ListActivities => reports,
        list_executions::ListExecutions => reports,
        list_map_runs::ListMapRuns => reports,
        list_state_machine_aliases::ListStateMachineAliases => reports,
        list_state_machine_versions::ListStateMachineVersions => reports,
        list_state_machines::ListStateMachines => reports,
        list_tags_for_resource::ListTagsForResource => reports,
        publish_state_machine_version::PublishStateMachineVersion => mutates,
        redrive_execution::RedriveExecution => mutates,
        send_task_failure::SendTaskFailure => mutates,
        send_task_heartbeat::SendTaskHeartbeat => mutates,
        send_task_success::SendTaskSuccess => mutates,
        start_execution::StartExecution => mutates,
        stop_execution::StopExecution => mutates,
        tag_resource::TagResource => mutates,
        test_state::TestState => mutates,
        untag_resource::UntagResource => mutates,
        update_map_run::UpdateMapRun => mutates,
        update_state_machine::UpdateStateMachine => mutates,
        update_state_machine_alias::UpdateStateMachineAlias => mutates,
        validate_state_machine_definition::ValidateStateMachineDefinition => reports,
    );
    // This service's synchronous DATA plane: the call that runs a state machine and answers
    // the run's own data, which CloudTrail records as a data event of that machine rather
    // than as a management event of the account. It is a WRITE, so its record carries
    // `readOnly` false exactly as the asynchronous start's does; what differs is where the
    // record goes (see [`super::data_events`]).
    let data_events = Arc::new(super::data_events::StateMachineDataEvents::new(Arc::clone(
        engine,
    )));
    table.serves_data_event(
        crate::ops::start_sync_execution::StartSyncExecution::new(Arc::clone(engine)),
        CallKind::Mutating,
        &data_events,
    );
    table.finish()
}
