// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `codepipeline.amazonaws.com`, recorded in the account's event history where
//! CloudTrail's `LookupEvents` reads it.
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
//! * the session a call's temporary credential belongs to, so a decision submitted by a
//!   role session is attributed to that session and to the role that issued it;
//! * the members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with
//! ([`service_common::audited_operation::AuditedDefinition`]) — so no request can be served
//! by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};

use crate::service::action_type_wire_form::DiscriminatedOperation;
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// The `eventSource` every record of this service carries.
///
/// The model's own answer and not a name assembled here:
/// `smithy_query(query="service_info")` reports
/// `awsService.cloudTrailEventSource: codepipeline.amazonaws.com` (the same
/// `<endpointPrefix>.amazonaws.com` form CloudTrail stamps on every event).
pub const EVENT_SOURCE: &str = "codepipeline.amazonaws.com";

/// The members this service's records carry CloudTrail's redaction literal for instead of
/// the value the call carried.
///
/// Two classes, both credential-bearing, so a record of the call that carried one shows
/// that it did without event history becoming a place to read the secret out of:
///
/// * every member this model declares `@sensitive` — the job worker's artifact credentials
///   (`smithy_query(query="shape", shape="AWSSessionCredentials")` reports the structure and
///   each of its three members as sensitive, and `JobData$artifactCredentials` /
///   `ThirdPartyJobData$artifactCredentials` target it). They are the temporary credentials
///   a job is handed to read and write the artifact store;
/// * the members the SERVING API itself withholds or authenticates a caller with, which
///   this model marks with no trait: the webhook's `SecretToken`, which
///   `WebhookAuthConfiguration$SecretToken` documents as "If a secret token was provided,
///   it will be redacted in the response" and which this service already keeps out of its
///   own answers ([`crate::state::REDACTED_SECRET_TOKEN`]); the `clientToken` a partner proves
///   its right to a third-party job with (`ClientToken` — the pair `clientId` /
///   `clientToken`, whose mismatch is `InvalidClientTokenException`); and the `nonce` a
///   worker claims a dispatched job with (`InvalidNonceException` is the refusal of a
///   wrong one), so holding a job id and its nonce is what lets a claim be made.
///
/// The set is by member NAME, so it holds for every operation that carries a member of
/// that name — the secret inside a `PutWebhook` definition and the same secret inside the
/// webhook a listing reports are both withheld, in a request and in a response alike.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    "artifactCredentials",
    "accessKeyId",
    "secretAccessKey",
    "sessionToken",
    "SecretToken",
    "clientToken",
    "nonce",
];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct CodePipelineAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> CodePipelineAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        crate::state::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for CodePipelineAudit<Eng> {
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
        // resource the call acted on runs under: the shared store the minting service wrote
        // it to.
        crate::state::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it — which is also the Region of every ARN the call mints.
        crate::state::request_region(&ctx.parts().headers)
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` are not this change's to invent — a record naming a resource
    // nothing resolved would be reported to a lookup by `ResourceName` as fact — so a
    // record of a call to this service carries the call, its parameters and its outcome,
    // and no resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }
}

/// The envelope the audited table hands every row for a pre-dispatch failure: the one this
/// service's `awsJson1_1` protocol gives it.
///
/// What a row actually answers such a failure in is the envelope of the protocol the REQUEST
/// is served in, read per request by the wrapper each row is served behind
/// ([`crate::service::action_type_wire_form`]) — this model declares two protocols, so one
/// value cannot be right for both calls. This is the value the table carries for a row
/// served through no such wrapper.
const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.1",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// The kind per row names the shared table's own value for it
/// ([`CallKind::Reporting`] / [`CallKind::Mutating`]), so nothing about what a kind means
/// is restated here.
macro_rules! call_kind {
    (reports) => {
        CallKind::Reporting
    };
    (mutates) => {
        CallKind::Mutating
    };
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// Every row is served behind the one wrapper that renders this service's responses in the
/// wire form the protocol gives them ([`crate::service::action_type_wire_form`]), with the
/// audited call INSIDE it: the record a lookup reports is the operation's own document,
/// while the response on the wire carries the shape discriminator wherever a structure of
/// this service has one.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve_behind(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                call_kind!($kind),
                DiscriminatedOperation::new,
            );
        )*
    }};
}

/// Every modelled operation of this service, each constructed from the shared engine and
/// served behind the audit recorder.
///
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Get*` / `List*`
/// readers, which report state and change none of it, and `mutates` for every operation
/// that creates, changes, triggers, answers or removes something the account then holds —
/// the two job polls included, since each one hands out the jobs it reports and records the
/// nonce a worker must acknowledge, and the job and approval results, since each one
/// settles the action it answers.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(CodePipelineAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, JSON_ENVELOPE);
    audited!(
        table, engine,
        acknowledge_job::AcknowledgeJob => mutates,
        acknowledge_third_party_job::AcknowledgeThirdPartyJob => mutates,
        create_custom_action_type::CreateCustomActionType => mutates,
        create_pipeline::CreatePipeline => mutates,
        delete_custom_action_type::DeleteCustomActionType => mutates,
        delete_pipeline::DeletePipeline => mutates,
        delete_webhook::DeleteWebhook => mutates,
        deregister_webhook_with_third_party::DeregisterWebhookWithThirdParty => mutates,
        disable_stage_transition::DisableStageTransition => mutates,
        enable_stage_transition::EnableStageTransition => mutates,
        get_action_type::GetActionType => reports,
        get_job_details::GetJobDetails => reports,
        get_pipeline::GetPipeline => reports,
        get_pipeline_execution::GetPipelineExecution => reports,
        get_pipeline_state::GetPipelineState => reports,
        get_third_party_job_details::GetThirdPartyJobDetails => reports,
        list_action_executions::ListActionExecutions => reports,
        list_action_types::ListActionTypes => reports,
        list_deploy_action_execution_targets::ListDeployActionExecutionTargets => reports,
        list_pipeline_executions::ListPipelineExecutions => reports,
        list_pipelines::ListPipelines => reports,
        list_rule_executions::ListRuleExecutions => reports,
        list_rule_types::ListRuleTypes => reports,
        list_tags_for_resource::ListTagsForResource => reports,
        list_webhooks::ListWebhooks => reports,
        override_stage_condition::OverrideStageCondition => mutates,
        poll_for_jobs::PollForJobs => mutates,
        poll_for_third_party_jobs::PollForThirdPartyJobs => mutates,
        put_action_revision::PutActionRevision => mutates,
        put_approval_result::PutApprovalResult => mutates,
        put_job_failure_result::PutJobFailureResult => mutates,
        put_job_success_result::PutJobSuccessResult => mutates,
        put_third_party_job_failure_result::PutThirdPartyJobFailureResult => mutates,
        put_third_party_job_success_result::PutThirdPartyJobSuccessResult => mutates,
        put_webhook::PutWebhook => mutates,
        register_webhook_with_third_party::RegisterWebhookWithThirdParty => mutates,
        retry_stage_execution::RetryStageExecution => mutates,
        rollback_stage::RollbackStage => mutates,
        start_pipeline_execution::StartPipelineExecution => mutates,
        stop_pipeline_execution::StopPipelineExecution => mutates,
        tag_resource::TagResource => mutates,
        untag_resource::UntagResource => mutates,
        update_action_type::UpdateActionType => mutates,
        update_pipeline::UpdatePipeline => mutates,
    );
    table.finish()
}
