// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `ecr.amazonaws.com`, recorded in the account's event history where CloudTrail's
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
//! * the Region the call was served in and the account it was served for;
//! * the request members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service claims for a modelled operation reaches that table, because it
//! is the routing table this service's definition answers with ([`AuditedDefinition`]) — so
//! no such request can be served by a path that records nothing. The two DATA planes this
//! service also serves (a published layer download URL, and the Docker Registry v2 endpoint)
//! name no modelled operation and are served from `Service::override_routing_error`
//! instead: a call on the CONTENTS of a repository is not a management event, so nothing is
//! recorded in event history for one.
//!
//! The record of a call this service made to ANOTHER service for the caller it was serving —
//! the KMS calls an encrypted repository's create and delete make — belongs to the service
//! that was CALLED and is written where that call is made (`crate::state::key_grant`),
//! through the shared builder for it
//! ([`service_common::on_behalf_of_calls`]); this module records the calls of THIS service's
//! own API, which carry no `invokedBy` because a client made them.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// This service's model-derived half, routing through the AUDITED operations — the shared
/// wrapper every recording service uses, re-exported under the name this crate holds it by.
pub use service_common::audited_operation::AuditedDefinition;

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: ecr`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "ecr.amazonaws.com";

/// The request members this service's records carry the redaction literal for instead of the
/// value the caller sent: NONE. This model declares no `@sensitive` member at all — no shape
/// or member of it carries the trait — so nothing a request carries is withheld from its
/// record.
///
/// The one value of this service that IS a credential — the authorization token
/// `GetAuthorizationToken` answers with — is left out for the other reason the shared wrapper
/// leaves a value out: that call REPORTS state, and the record of a reporting call carries no
/// `responseElements`.
pub const SENSITIVE_MEMBERS: &[&str] = &[];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail's `LookupEvents` reads — so it is generic over the engine type exactly as the
/// service is.
pub struct EcrAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EcrAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        ::service_common::on_behalf_of_calls::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for EcrAudit<Eng> {
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
        // this service reads it: a registry is one account's in one Region, so a record
        // belongs to the Region its call was signed for.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in an
    // event's `Resources` name a resource by ARN, and a record naming a resource nothing
    // resolved would be reported to a lookup by `ResourceName` as fact — so a record of a
    // call to this service carries the call, its parameters and its outcome, and no resource
    // list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session — and as the role that issued it: the shared store the
        // minting service wrote it to.
        ::service_common::on_behalf_of_calls::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in: this
/// service speaks `awsJson1_1` (`smithy_query(query="digest")` reports the one protocol
/// `awsJson1_1`), whose errors carry the fault shape in the body's `__type` rather than in a
/// header — the same values the generated per-operation shim passes, so a refusal raised
/// ahead of a handler reads exactly as it did before.
pub const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.1",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// Serve one operation of this service audited, as a call of the stated kind: `reports` for
/// an operation that only REPORTS state, `mutates` for one that CHANGES what the account
/// holds — which is the event's `readOnly`, and what a lookup can filter on.
///
/// The kind per row names the shared table's own method for it ([`AuditedTable::reports`] /
/// [`AuditedTable::mutates`]), so nothing about what a kind means is restated here.
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
/// EVERY one is here — the operations `smithy_query(query="list_operations")` lists — the
/// ones this emulator has not implemented included: a stub answers 501 and the wrapper
/// records nothing for it, so an operation implemented later is auditable the moment it
/// serves a call rather than when someone remembers to add it.
///
/// This model declares no `@readonly` operation, so the kind per row is the operation's own
/// documented semantics: `reports` for the `Describe*` / `Get*` / `List*` / `BatchGet*`
/// readers, for `BatchCheckLayerAvailability` (which "checks the availability of one or more
/// image layers"), for `GetAuthorizationToken` (which hands back the caller's own
/// credentials as a registry token and changes nothing the account holds) and for
/// `ValidatePullThroughCacheRule` (which answers whether a stored rule works); `mutates` for
/// every operation that creates, deletes, uploads, tags, transitions or otherwise
/// reconfigures something the account then holds — the two that START work whose result the
/// account keeps (`StartImageScan`, `StartLifecyclePolicyPreview`) and the upload legs of a
/// push (`InitiateLayerUpload`, `UploadLayerPart`, `CompleteLayerUpload`, `PutImage`)
/// included.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(EcrAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, JSON_ENVELOPE);
    audited!(
        table, engine,
        batch_check_layer_availability::BatchCheckLayerAvailability => reports,
        batch_delete_image::BatchDeleteImage => mutates,
        batch_get_image::BatchGetImage => reports,
        batch_get_repository_scanning_configuration::BatchGetRepositoryScanningConfiguration
            => reports,
        complete_layer_upload::CompleteLayerUpload => mutates,
        create_pull_through_cache_rule::CreatePullThroughCacheRule => mutates,
        create_repository::CreateRepository => mutates,
        create_repository_creation_template::CreateRepositoryCreationTemplate => mutates,
        delete_lifecycle_policy::DeleteLifecyclePolicy => mutates,
        delete_pull_through_cache_rule::DeletePullThroughCacheRule => mutates,
        delete_registry_policy::DeleteRegistryPolicy => mutates,
        delete_repository::DeleteRepository => mutates,
        delete_repository_creation_template::DeleteRepositoryCreationTemplate => mutates,
        delete_repository_policy::DeleteRepositoryPolicy => mutates,
        delete_signing_configuration::DeleteSigningConfiguration => mutates,
        deregister_pull_time_update_exclusion::DeregisterPullTimeUpdateExclusion => mutates,
        describe_image_replication_status::DescribeImageReplicationStatus => reports,
        describe_image_scan_findings::DescribeImageScanFindings => reports,
        describe_image_signing_status::DescribeImageSigningStatus => reports,
        describe_images::DescribeImages => reports,
        describe_pull_through_cache_rules::DescribePullThroughCacheRules => reports,
        describe_registry::DescribeRegistry => reports,
        describe_replication_backfill_results::DescribeReplicationBackfillResults => reports,
        describe_repositories::DescribeRepositories => reports,
        describe_repository_creation_templates::DescribeRepositoryCreationTemplates => reports,
        get_account_setting::GetAccountSetting => reports,
        get_authorization_token::GetAuthorizationToken => reports,
        get_download_url_for_layer::GetDownloadUrlForLayer => reports,
        get_lifecycle_policy::GetLifecyclePolicy => reports,
        get_lifecycle_policy_preview::GetLifecyclePolicyPreview => reports,
        get_registry_policy::GetRegistryPolicy => reports,
        get_registry_scanning_configuration::GetRegistryScanningConfiguration => reports,
        get_repository_policy::GetRepositoryPolicy => reports,
        get_signing_configuration::GetSigningConfiguration => reports,
        initiate_layer_upload::InitiateLayerUpload => mutates,
        list_image_referrers::ListImageReferrers => reports,
        list_images::ListImages => reports,
        list_pull_time_update_exclusions::ListPullTimeUpdateExclusions => reports,
        list_replication_backfills::ListReplicationBackfills => reports,
        list_tags_for_resource::ListTagsForResource => reports,
        put_account_setting::PutAccountSetting => mutates,
        put_image::PutImage => mutates,
        put_image_scanning_configuration::PutImageScanningConfiguration => mutates,
        put_image_tag_mutability::PutImageTagMutability => mutates,
        put_lifecycle_policy::PutLifecyclePolicy => mutates,
        put_registry_policy::PutRegistryPolicy => mutates,
        put_registry_scanning_configuration::PutRegistryScanningConfiguration => mutates,
        put_replication_configuration::PutReplicationConfiguration => mutates,
        put_signing_configuration::PutSigningConfiguration => mutates,
        register_pull_time_update_exclusion::RegisterPullTimeUpdateExclusion => mutates,
        set_repository_policy::SetRepositoryPolicy => mutates,
        start_image_scan::StartImageScan => mutates,
        start_lifecycle_policy_preview::StartLifecyclePolicyPreview => mutates,
        start_replication_backfill::StartReplicationBackfill => mutates,
        stop_replication_backfill::StopReplicationBackfill => mutates,
        tag_resource::TagResource => mutates,
        untag_resource::UntagResource => mutates,
        update_image_storage_class::UpdateImageStorageClass => mutates,
        update_pull_through_cache_rule::UpdatePullThroughCacheRule => mutates,
        update_repository_creation_template::UpdateRepositoryCreationTemplate => mutates,
        upload_layer_part::UploadLayerPart => mutates,
        validate_pull_through_cache_rule::ValidatePullThroughCacheRule => reports,
    );
    table.finish()
}
