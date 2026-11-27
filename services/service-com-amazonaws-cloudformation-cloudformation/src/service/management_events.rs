// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `cloudformation.amazonaws.com`, recorded in the account's event history where
//! CloudTrail's `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs and the handler's own `Result` after, and
//! this module is only what THIS service contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region the call was served in and the account it was served for;
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with ([`AuditedDefinition`], nested inside this
//! service's own `awsQuery` error-envelope definition) — so no request can be served by a
//! path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable};
use service_common::management_events::{ManagementEventLog, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// This service's model-derived half, routing through the AUDITED operations — the shared
/// wrapper every recording service uses, re-exported under the name this crate holds it by.
pub use service_common::audited_operation::AuditedDefinition;

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: cloudformation`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "cloudformation.amazonaws.com";

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct CloudFormationAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> CloudFormationAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for CloudFormationAudit<Eng> {
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

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read off the request the way every handler of
        // this service reads it: CloudFormation is Regional, and a stack name is a
        // per-Region namespace, so a record belongs to the Region its call was signed for.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::emulator_server::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` name a resource by ARN, and a stack's ARN carries the id the
    // store minted for it — which a request names nothing of — so a record carries the
    // call, its parameters and its outcome rather than a resource built from a name alone.

    fn sensitive_members(&self) -> &'static [&'static str] {
        // Nothing is withheld: this service's model declares no `@sensitive` member, so
        // there is no member whose value a record of one of its calls replaces with the
        // redaction literal (checked over the whole model — no shape or member of it
        // carries the trait).
        &[]
    }
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type ([`service_common::management_events::ManagementEventLog`]) and not a
/// field of this service's own state: CloudTrail reports the management calls of every
/// service, and the service that served a call is the only one that knows it happened, so
/// recorder and reporter reach one log rather than two copies of it.
///
/// The ACCESSOR is stated per service (the same five lines live in the IAM, STS, Amazon RDS,
/// ELBv2, CloudTrail and Amazon Cognito Identity recorders) because it names a concrete
/// engine type, and `services/common` — where the log type itself lives — does not depend on
/// the engine crate at all. Hoisting it there is a change to that manifest, and belongs to
/// the change that makes it rather than to this one.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsQuery` (not the EC2 dialect), which is the same value the
/// generated per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

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
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Describe*` /
/// `Get*` / `List*` readers, for `BatchDescribeTypeConfigurations`, and for the two
/// operations that answer a question about a submitted template without touching the
/// account (`ValidateTemplate`, `EstimateTemplateCost`); `mutates` for every operation that
/// creates, changes, moves or removes something the account then holds — the drift and
/// scan detections included, since each one runs a check whose result the account keeps.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(CloudFormationAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, QUERY_ENVELOPE);
    audited!(
        table, engine,
        activate_organizations_access::ActivateOrganizationsAccess => mutates,
        activate_type::ActivateType => mutates,
        batch_describe_type_configurations::BatchDescribeTypeConfigurations => reports,
        cancel_update_stack::CancelUpdateStack => mutates,
        continue_update_rollback::ContinueUpdateRollback => mutates,
        create_change_set::CreateChangeSet => mutates,
        create_generated_template::CreateGeneratedTemplate => mutates,
        create_stack::CreateStack => mutates,
        create_stack_instances::CreateStackInstances => mutates,
        create_stack_refactor::CreateStackRefactor => mutates,
        create_stack_set::CreateStackSet => mutates,
        deactivate_organizations_access::DeactivateOrganizationsAccess => mutates,
        deactivate_type::DeactivateType => mutates,
        delete_change_set::DeleteChangeSet => mutates,
        delete_generated_template::DeleteGeneratedTemplate => mutates,
        delete_stack::DeleteStack => mutates,
        delete_stack_instances::DeleteStackInstances => mutates,
        delete_stack_set::DeleteStackSet => mutates,
        deregister_type::DeregisterType => mutates,
        describe_account_limits::DescribeAccountLimits => reports,
        describe_change_set::DescribeChangeSet => reports,
        describe_change_set_hooks::DescribeChangeSetHooks => reports,
        describe_events::DescribeEvents => reports,
        describe_generated_template::DescribeGeneratedTemplate => reports,
        describe_organizations_access::DescribeOrganizationsAccess => reports,
        describe_publisher::DescribePublisher => reports,
        describe_resource_scan::DescribeResourceScan => reports,
        describe_stack_drift_detection_status::DescribeStackDriftDetectionStatus => reports,
        describe_stack_events::DescribeStackEvents => reports,
        describe_stack_instance::DescribeStackInstance => reports,
        describe_stack_refactor::DescribeStackRefactor => reports,
        describe_stack_resource::DescribeStackResource => reports,
        describe_stack_resource_drifts::DescribeStackResourceDrifts => reports,
        describe_stack_resources::DescribeStackResources => reports,
        describe_stack_set::DescribeStackSet => reports,
        describe_stack_set_operation::DescribeStackSetOperation => reports,
        describe_stacks::DescribeStacks => reports,
        describe_type::DescribeType => reports,
        describe_type_registration::DescribeTypeRegistration => reports,
        detect_stack_drift::DetectStackDrift => mutates,
        detect_stack_resource_drift::DetectStackResourceDrift => mutates,
        detect_stack_set_drift::DetectStackSetDrift => mutates,
        estimate_template_cost::EstimateTemplateCost => reports,
        execute_change_set::ExecuteChangeSet => mutates,
        execute_stack_refactor::ExecuteStackRefactor => mutates,
        get_generated_template::GetGeneratedTemplate => reports,
        get_hook_result::GetHookResult => reports,
        get_stack_policy::GetStackPolicy => reports,
        get_template::GetTemplate => reports,
        get_template_summary::GetTemplateSummary => reports,
        import_stacks_to_stack_set::ImportStacksToStackSet => mutates,
        list_change_sets::ListChangeSets => reports,
        list_exports::ListExports => reports,
        list_generated_templates::ListGeneratedTemplates => reports,
        list_hook_results::ListHookResults => reports,
        list_imports::ListImports => reports,
        list_resource_scan_related_resources::ListResourceScanRelatedResources => reports,
        list_resource_scan_resources::ListResourceScanResources => reports,
        list_resource_scans::ListResourceScans => reports,
        list_stack_instance_resource_drifts::ListStackInstanceResourceDrifts => reports,
        list_stack_instances::ListStackInstances => reports,
        list_stack_refactor_actions::ListStackRefactorActions => reports,
        list_stack_refactors::ListStackRefactors => reports,
        list_stack_resources::ListStackResources => reports,
        list_stack_set_auto_deployment_targets::ListStackSetAutoDeploymentTargets => reports,
        list_stack_set_operation_results::ListStackSetOperationResults => reports,
        list_stack_set_operations::ListStackSetOperations => reports,
        list_stack_sets::ListStackSets => reports,
        list_stacks::ListStacks => reports,
        list_type_registrations::ListTypeRegistrations => reports,
        list_type_versions::ListTypeVersions => reports,
        list_types::ListTypes => reports,
        publish_type::PublishType => mutates,
        record_handler_progress::RecordHandlerProgress => mutates,
        register_publisher::RegisterPublisher => mutates,
        register_type::RegisterType => mutates,
        remove_alias::RemoveAlias => mutates,
        rollback_stack::RollbackStack => mutates,
        set_alias::SetAlias => mutates,
        set_stack_policy::SetStackPolicy => mutates,
        set_type_configuration::SetTypeConfiguration => mutates,
        set_type_default_version::SetTypeDefaultVersion => mutates,
        signal_resource::SignalResource => mutates,
        start_resource_scan::StartResourceScan => mutates,
        stop_stack_set_operation::StopStackSetOperation => mutates,
        test_type::TestType => mutates,
        update_generated_template::UpdateGeneratedTemplate => mutates,
        update_stack::UpdateStack => mutates,
        update_stack_instances::UpdateStackInstances => mutates,
        update_stack_set::UpdateStackSet => mutates,
        update_termination_protection::UpdateTerminationProtection => mutates,
        validate_template::ValidateTemplate => reports,
    );
    table.finish()
}
