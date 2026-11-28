// SPDX-License-Identifier: CC0-1.0

//! This front door's MANAGEMENT leg: the calls that ADMINISTER a bucket, which CloudTrail
//! records in the account's event history where `LookupEvents` reports them.
//!
//! CloudTrail classes an account's API calls in two and delivers the two to different
//! readers. A call on the CONTENTS of one object is a DATA event, which is never event
//! history ([`super::data_events`]); everything else this service serves administers a
//! BUCKET — creating and deleting it, reading and writing its configuration, listing what
//! it holds, listing the account's buckets — and every one of those is a MANAGEMENT event:
//! recorded in event history, published onto the account's default event bus as
//! `AWS API Call via CloudTrail`, and written into the bucket of every trail that logs
//! management events of its read/write class.
//!
//! Every control-plane API call of an account is a management event, which makes the
//! recording a property of SERVING a call and not of any one operation — so it is not
//! written per handler. The shared wrapper ([`service_common::audited_operation`]) is handed
//! the deserialized input before the handler runs, the handler's own `Result` after, and the
//! rendered response last, and this module is only what THIS service contributes to each
//! record:
//!
//! * its `eventSource` (the modelled [`EVENT_SOURCE`], shared with the data leg);
//! * the Region and the account the call was served for;
//! * the request members whose values its records WITHHOLD ([`SENSITIVE_MEMBERS`]);
//! * the two things about the document that are Amazon S3's own — the member the addressed
//!   bucket is named by, and the sentence a refusal was answered with
//!   ([`super::data_events::this_services_record`], shared with the data leg so one call
//!   cannot be reported two ways); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`MANAGEMENT_EVENT_OPERATIONS`].
//!
//! Every request this service serves reaches the table this module builds, because it is the
//! table `Service::override_operation` resolves an operation out of before the generated one
//! ([`super::front_door`]) — by either address form, and through every seam that resolves a
//! route itself — so no bucket-level request can be served by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served. Neither is a request this service refuses AHEAD of the operation — the bucket NAME
//! it addressed, the DIRECTORY bucket's narrower API, the session or the credential it
//! presented, the expected bucket owner it named ([`super::front_door`] holds those rows):
//! such a request reaches no typed call, so the shared seam has no input, no outcome and no
//! operation to build a record from, exactly as it has none for a request the framework's own
//! boundary refused.

use std::sync::Arc;

use aws_smithy_runtime_api::http::Response as RtResponse;
use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{Operation, OperationContext, OperationRoutingTable};

/// The `eventSource` every record of this service carries, stated once beside the DATA leg
/// that shares it: the model's own `awsService.cloudTrailEventSource`.
pub use super::data_events::EVENT_SOURCE;

/// The request members a record of a bucket-level call carries CloudTrail's redaction
/// literal for instead of the value the caller sent.
///
/// The members this model declares `@sensitive` on the operations recorded here, named as
/// the model spells them: `ServerSideEncryptionByDefault$KMSMasterKeyID`, the KMS key a
/// bucket's default encryption names
/// (`smithy_query(query="shape",shape="ServerSideEncryptionByDefault")`), and `SSEKMS$KeyId`,
/// the KMS key an inventory report's encryption names
/// (`smithy_query(query="shape",shape="SSEKMS")`). The set is by member NAME, so it holds
/// for every operation that carries a member of that name — the key a put declares and the
/// key a read reports are both withheld.
///
/// The object plane's own sensitive members are the DATA leg's
/// ([`super::data_events`]): a member of a request this leg never records needs no row here.
const SENSITIVE_MEMBERS: &[&str] = &["KMSMasterKeyID", "KeyId"];

/// Every operation of this service CloudTrail records as a MANAGEMENT event, with the
/// read/write class of its calls.
///
/// The set is the complement of the data-event catalog
/// ([`super::data_events::DATA_EVENT_OPERATIONS`]): "data events provide visibility into the
/// resource operations performed on or within a resource", so the object-level calls are
/// that catalog's and the BUCKET-level calls — a bucket's own lifecycle, its configuration
/// surfaces, its listings and the account's bucket listings — are management events and are
/// here.
///
/// The CLASS per row is the operation's own documented semantics, which is what the event's
/// `readOnly` carries and what a trail's `ReadWriteType` narrows a trail's management events
/// by: every `Get`/`List`/`Head` reader reports state and changes none of it, and everything
/// that creates, writes, updates or deletes a bucket or one of its configurations changes
/// it.
///
/// Five modelled operations are in NEITHER catalog, and that is deliberate rather than an
/// omission: `CreateSession`, `RenameObject`, `UpdateObjectEncryption`,
/// `WriteGetObjectResponse` and the four object-annotation calls act on one object (or on
/// the session a directory bucket's object plane is reached through), so they are calls on a
/// resource's contents and no event history may hold them — the one class a management
/// record must never carry. Recording them is the data leg's to extend when its catalog
/// grows, not this one's to claim. `SelectObjectContent` implements no
/// `smithy_server_runtime::Operation` at all ([`super::select_object_content`]), so there is
/// no typed call to build a record from.
///
/// Public, so this crate's own tests read the catalog the routing table is built from rather
/// than a second copy of it.
pub const MANAGEMENT_EVENT_OPERATIONS: &[(&str, CallKind)] = &[
    ("CreateBucket", CallKind::Mutating),
    ("CreateBucketMetadataConfiguration", CallKind::Mutating),
    ("CreateBucketMetadataTableConfiguration", CallKind::Mutating),
    ("DeleteBucket", CallKind::Mutating),
    ("DeleteBucketAnalyticsConfiguration", CallKind::Mutating),
    ("DeleteBucketCors", CallKind::Mutating),
    ("DeleteBucketEncryption", CallKind::Mutating),
    (
        "DeleteBucketIntelligentTieringConfiguration",
        CallKind::Mutating,
    ),
    ("DeleteBucketInventoryConfiguration", CallKind::Mutating),
    ("DeleteBucketLifecycle", CallKind::Mutating),
    ("DeleteBucketMetadataConfiguration", CallKind::Mutating),
    ("DeleteBucketMetadataTableConfiguration", CallKind::Mutating),
    ("DeleteBucketMetricsConfiguration", CallKind::Mutating),
    ("DeleteBucketOwnershipControls", CallKind::Mutating),
    ("DeleteBucketPolicy", CallKind::Mutating),
    ("DeleteBucketReplication", CallKind::Mutating),
    ("DeleteBucketTagging", CallKind::Mutating),
    ("DeleteBucketWebsite", CallKind::Mutating),
    ("DeletePublicAccessBlock", CallKind::Mutating),
    ("GetBucketAbac", CallKind::Reporting),
    ("GetBucketAccelerateConfiguration", CallKind::Reporting),
    ("GetBucketAcl", CallKind::Reporting),
    ("GetBucketAnalyticsConfiguration", CallKind::Reporting),
    ("GetBucketCors", CallKind::Reporting),
    ("GetBucketEncryption", CallKind::Reporting),
    (
        "GetBucketIntelligentTieringConfiguration",
        CallKind::Reporting,
    ),
    ("GetBucketInventoryConfiguration", CallKind::Reporting),
    ("GetBucketLifecycleConfiguration", CallKind::Reporting),
    ("GetBucketLocation", CallKind::Reporting),
    ("GetBucketLogging", CallKind::Reporting),
    ("GetBucketMetadataConfiguration", CallKind::Reporting),
    ("GetBucketMetadataTableConfiguration", CallKind::Reporting),
    ("GetBucketMetricsConfiguration", CallKind::Reporting),
    ("GetBucketNotificationConfiguration", CallKind::Reporting),
    ("GetBucketOwnershipControls", CallKind::Reporting),
    ("GetBucketPolicy", CallKind::Reporting),
    ("GetBucketPolicyStatus", CallKind::Reporting),
    ("GetBucketReplication", CallKind::Reporting),
    ("GetBucketRequestPayment", CallKind::Reporting),
    ("GetBucketTagging", CallKind::Reporting),
    ("GetBucketVersioning", CallKind::Reporting),
    ("GetBucketWebsite", CallKind::Reporting),
    ("GetObjectLockConfiguration", CallKind::Reporting),
    ("GetPublicAccessBlock", CallKind::Reporting),
    ("HeadBucket", CallKind::Reporting),
    ("ListBucketAnalyticsConfigurations", CallKind::Reporting),
    (
        "ListBucketIntelligentTieringConfigurations",
        CallKind::Reporting,
    ),
    ("ListBucketInventoryConfigurations", CallKind::Reporting),
    ("ListBucketMetricsConfigurations", CallKind::Reporting),
    ("ListBuckets", CallKind::Reporting),
    ("ListDirectoryBuckets", CallKind::Reporting),
    ("ListMultipartUploads", CallKind::Reporting),
    ("ListObjectVersions", CallKind::Reporting),
    ("ListObjects", CallKind::Reporting),
    ("ListObjectsV2", CallKind::Reporting),
    ("PutBucketAbac", CallKind::Mutating),
    ("PutBucketAccelerateConfiguration", CallKind::Mutating),
    ("PutBucketAcl", CallKind::Mutating),
    ("PutBucketAnalyticsConfiguration", CallKind::Mutating),
    ("PutBucketCors", CallKind::Mutating),
    ("PutBucketEncryption", CallKind::Mutating),
    (
        "PutBucketIntelligentTieringConfiguration",
        CallKind::Mutating,
    ),
    ("PutBucketInventoryConfiguration", CallKind::Mutating),
    ("PutBucketLifecycleConfiguration", CallKind::Mutating),
    ("PutBucketLogging", CallKind::Mutating),
    ("PutBucketMetricsConfiguration", CallKind::Mutating),
    ("PutBucketNotificationConfiguration", CallKind::Mutating),
    ("PutBucketOwnershipControls", CallKind::Mutating),
    ("PutBucketPolicy", CallKind::Mutating),
    ("PutBucketReplication", CallKind::Mutating),
    ("PutBucketRequestPayment", CallKind::Mutating),
    ("PutBucketTagging", CallKind::Mutating),
    ("PutBucketVersioning", CallKind::Mutating),
    ("PutBucketWebsite", CallKind::Mutating),
    ("PutObjectLockConfiguration", CallKind::Mutating),
    ("PutPublicAccessBlock", CallKind::Mutating),
    (
        "UpdateBucketMetadataAnnotationTableConfiguration",
        CallKind::Mutating,
    ),
    (
        "UpdateBucketMetadataInventoryTableConfiguration",
        CallKind::Mutating,
    ),
    (
        "UpdateBucketMetadataJournalTableConfiguration",
        CallKind::Mutating,
    ),
];

/// The modelled operations of this service that are in NEITHER event-class catalog, each for
/// the reason the note on [`MANAGEMENT_EVENT_OPERATIONS`] gives: they act on one object (or on
/// the session a directory bucket's object plane is reached through), so no event history may
/// hold them, and the data leg's catalog does not hold them yet; or they implement no typed
/// `smithy_server_runtime::Operation` for a record to be built from at all.
///
/// Stated as a set so this module's own test can hold the model's WHOLE operation surface
/// against the three sets together: an operation the model declares and no set names is an
/// operation whose calls this service would record as neither class without anybody deciding
/// that.
pub const NEITHER_EVENT_CLASS: &[&str] = &[
    "CreateSession",
    "DeleteObjectAnnotation",
    "GetObjectAnnotation",
    "ListObjectAnnotations",
    "PutObjectAnnotation",
    "RenameObject",
    "SelectObjectContent",
    "UpdateObjectEncryption",
    "WriteGetObjectResponse",
];

/// The read/write class CloudTrail records the calls of `operation` as a management event
/// with, or `None` for an operation whose calls are no management event of this service.
#[must_use]
pub fn recorded_class(operation: &str) -> Option<CallKind> {
    MANAGEMENT_EVENT_OPERATIONS
        .iter()
        .find(|(named, _)| *named == operation)
        .map(|(_, kind)| *kind)
}

/// What this service contributes to the record of each bucket-level call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail's `LookupEvents` reads and the one CloudTrail projects its logging trails
/// into — so it is generic over the engine type exactly as the service is.
pub struct BucketManagementEvents<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> BucketManagementEvents<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        super::data_events::management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for BucketManagementEvents<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        // The two things about the document that are Amazon S3's own, applied by the one
        // function both legs share so a bucket call and an object call cannot be reported
        // two ways.
        let call = super::data_events::this_services_record(call);
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in event history by definition,
            // so it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and what the account's CloudTrail delivers for it while a trail of the
        // Region is logging calls of its kind; making those deliveries is the wrapper's. The
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
        // The Region the call was served in, read off the request the way every other seam
        // of this service reads it ([`crate::state::request_region`]).
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in an
    // event's `Resources` are the DATA leg's own concern here — a data event is SELECTED by
    // the resource it acted on ([`super::data_events`]) — while a management record of this
    // service carries the call, its parameters and its outcome.

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session — and as the role that issued it: the shared store the
        // minting service wrote it to, read the one way this service reads it.
        super::data_events::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn names_the_request(&self, resp: RtResponse) -> (RtResponse, String) {
        // The id the CALLER was answered under, read out of the header S3 states it in —
        // the same reading the data leg joins its records by, stated once
        // ([`super::data_events::stated_request_id`]).
        super::data_events::stated_request_id(resp)
    }
}

/// Every operation of this service whose calls are MANAGEMENT events, each served audited
/// and keyed by the modelled name the protocol resolves for it.
///
/// The rows this service wraps in something of its OWN are served here too, through
/// [`AuditedTable::serve_behind`]: the record reports the operation's own input and outcome
/// from INSIDE that wrapper, so a door that changes how a refusal is rendered on the wire
/// cannot change the document a lookup reports, and one operation is served once rather than
/// twice.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(BucketManagementEvents::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, super::data_events::RESTXML_ENVELOPE);

    /// Serve one operation audited as the management event the catalog says it is.
    ///
    /// The read/write class is the catalog's ([`MANAGEMENT_EVENT_OPERATIONS`]), which is
    /// where it is stated once: an operation the catalog does not hold is left to the
    /// generated table, so a row here and the catalog cannot disagree about which calls are
    /// management events.
    macro_rules! bucket_plane {
        ( $( $module:ident :: $operation:ident ),* $(,)? ) => {{
            $(
                let name = <crate::ops::$module::$operation<Eng> as Operation>::NAME;
                if let Some(kind) = recorded_class(name) {
                    let operation = crate::ops::$module::$operation::new(Arc::clone(engine));
                    // Served behind the wrapper that admits the callers this service
                    // authenticates itself — the ANONYMOUS one for the reads that judge it
                    // ([`super::unsigned_request::served_to_the_anonymous_caller`] is the
                    // catalog), and the one presenting a query credential no registered
                    // authenticator can read
                    // ([`super::legacy_query_credential`]) — INSIDE the recorder, so such a
                    // call is recorded exactly as a header-signed one is and one operation is
                    // still served once.
                    table.serve_behind(operation, kind, |audited, _| {
                        super::unsigned_request::ServedUnsigned::new(name, Box::new(audited))
                    });
                }
            )*
        }};
    }
    bucket_plane!(
        create_bucket::CreateBucket,
        create_bucket_metadata_configuration::CreateBucketMetadataConfiguration,
        create_bucket_metadata_table_configuration::CreateBucketMetadataTableConfiguration,
        delete_bucket::DeleteBucket,
        delete_bucket_analytics_configuration::DeleteBucketAnalyticsConfiguration,
        delete_bucket_cors::DeleteBucketCors,
        delete_bucket_encryption::DeleteBucketEncryption,
        delete_bucket_intelligent_tiering_configuration::DeleteBucketIntelligentTieringConfiguration,
        delete_bucket_inventory_configuration::DeleteBucketInventoryConfiguration,
        delete_bucket_lifecycle::DeleteBucketLifecycle,
        delete_bucket_metadata_configuration::DeleteBucketMetadataConfiguration,
        delete_bucket_metadata_table_configuration::DeleteBucketMetadataTableConfiguration,
        delete_bucket_ownership_controls::DeleteBucketOwnershipControls,
        delete_bucket_policy::DeleteBucketPolicy,
        delete_bucket_replication::DeleteBucketReplication,
        delete_bucket_tagging::DeleteBucketTagging,
        delete_bucket_website::DeleteBucketWebsite,
        delete_public_access_block::DeletePublicAccessBlock,
        get_bucket_abac::GetBucketAbac,
        get_bucket_accelerate_configuration::GetBucketAccelerateConfiguration,
        get_bucket_acl::GetBucketAcl,
        get_bucket_analytics_configuration::GetBucketAnalyticsConfiguration,
        get_bucket_cors::GetBucketCors,
        get_bucket_encryption::GetBucketEncryption,
        get_bucket_intelligent_tiering_configuration::GetBucketIntelligentTieringConfiguration,
        get_bucket_inventory_configuration::GetBucketInventoryConfiguration,
        get_bucket_lifecycle_configuration::GetBucketLifecycleConfiguration,
        get_bucket_location::GetBucketLocation,
        get_bucket_logging::GetBucketLogging,
        get_bucket_metadata_configuration::GetBucketMetadataConfiguration,
        get_bucket_metadata_table_configuration::GetBucketMetadataTableConfiguration,
        get_bucket_metrics_configuration::GetBucketMetricsConfiguration,
        get_bucket_notification_configuration::GetBucketNotificationConfiguration,
        get_bucket_ownership_controls::GetBucketOwnershipControls,
        get_bucket_policy::GetBucketPolicy,
        get_bucket_policy_status::GetBucketPolicyStatus,
        get_bucket_replication::GetBucketReplication,
        get_bucket_request_payment::GetBucketRequestPayment,
        get_bucket_tagging::GetBucketTagging,
        get_bucket_versioning::GetBucketVersioning,
        get_bucket_website::GetBucketWebsite,
        get_object_lock_configuration::GetObjectLockConfiguration,
        get_public_access_block::GetPublicAccessBlock,
        head_bucket::HeadBucket,
        list_bucket_analytics_configurations::ListBucketAnalyticsConfigurations,
        list_bucket_intelligent_tiering_configurations::ListBucketIntelligentTieringConfigurations,
        list_bucket_inventory_configurations::ListBucketInventoryConfigurations,
        list_bucket_metrics_configurations::ListBucketMetricsConfigurations,
        list_buckets::ListBuckets,
        list_directory_buckets::ListDirectoryBuckets,
        put_bucket_abac::PutBucketAbac,
        put_bucket_accelerate_configuration::PutBucketAccelerateConfiguration,
        put_bucket_acl::PutBucketAcl,
        put_bucket_analytics_configuration::PutBucketAnalyticsConfiguration,
        put_bucket_cors::PutBucketCors,
        put_bucket_encryption::PutBucketEncryption,
        put_bucket_intelligent_tiering_configuration::PutBucketIntelligentTieringConfiguration,
        put_bucket_lifecycle_configuration::PutBucketLifecycleConfiguration,
        put_bucket_logging::PutBucketLogging,
        put_bucket_notification_configuration::PutBucketNotificationConfiguration,
        put_bucket_ownership_controls::PutBucketOwnershipControls,
        put_bucket_policy::PutBucketPolicy,
        put_bucket_replication::PutBucketReplication,
        put_bucket_request_payment::PutBucketRequestPayment,
        put_bucket_tagging::PutBucketTagging,
        put_bucket_versioning::PutBucketVersioning,
        put_bucket_website::PutBucketWebsite,
        put_object_lock_configuration::PutObjectLockConfiguration,
        put_public_access_block::PutPublicAccessBlock,
        update_bucket_metadata_annotation_table_configuration::UpdateBucketMetadataAnnotationTableConfiguration,
        update_bucket_metadata_inventory_table_configuration::UpdateBucketMetadataInventoryTableConfiguration,
        update_bucket_metadata_journal_table_configuration::UpdateBucketMetadataJournalTableConfiguration,
    );

    // The three configuration calls this service also serves behind a FRONT DOOR, for the
    // refusal only a door can word ([`super::front_door`] gives the rule for each): the door
    // stays OUTSIDE the recorder, so what it refuses is still answered in this service's own
    // envelope and what it admits is recorded from the operation's own typed call.
    serve_behind_door(
        &mut table,
        crate::ops::put_bucket_metrics_configuration::PutBucketMetricsConfiguration::new(
            Arc::clone(engine),
        ),
    );
    serve_behind_door(
        &mut table,
        crate::ops::delete_bucket_metrics_configuration::DeleteBucketMetricsConfiguration::new(
            Arc::clone(engine),
        ),
    );
    let inventory_kind = recorded_class(
        <crate::ops::put_bucket_inventory_configuration::PutBucketInventoryConfiguration<Eng> as Operation>::NAME,
    )
    .expect("the inventory put is a management event of this service");
    table.serve_behind(
        crate::ops::put_bucket_inventory_configuration::PutBucketInventoryConfiguration::new(
            Arc::clone(engine),
        ),
        inventory_kind,
        |audited, _| {
            super::readable_document::ReadableDocument::<
                crate::ops::put_bucket_inventory_configuration::PutBucketInventoryConfiguration<
                    Eng,
                >,
                _,
            >::new(
                ::service_common::front_door::FrontDoor::worded(
                    audited,
                    super::submitted_value_wording,
                )
                .wording_uri(super::inventory_configuration_id_wording),
            )
        },
    );

    // And the four listings of what a bucket holds, which are COUNTED into the addressed
    // bucket's request metrics as well as served behind a door
    // ([`super::request_metrics::counted_behind_door`]): a listing reports the bucket rather
    // than one object's contents, so it is a management event of this service and is served
    // here — one entry per operation, so the counting, the door and the record are the same
    // served operation rather than three.
    serve_counted_behind_door(
        &mut table,
        engine,
        crate::ops::list_objects_v2::ListObjectsV2::new(Arc::clone(engine)),
    );
    serve_counted_behind_door(
        &mut table,
        engine,
        crate::ops::list_objects::ListObjects::new(Arc::clone(engine)),
    );
    serve_counted_behind_door(
        &mut table,
        engine,
        crate::ops::list_object_versions::ListObjectVersions::new(Arc::clone(engine)),
    );
    serve_counted_behind_door(
        &mut table,
        engine,
        crate::ops::list_multipart_uploads::ListMultipartUploads::new(Arc::clone(engine)),
    );

    table.finish()
}

/// Serve `operation` audited behind this service's front door, worded the way the
/// id-addressed configuration calls are refused ([`super::configuration_id_wording`]).
///
/// The row carries the DOOR as well as the recorder, so a class the catalog does not hold
/// would drop the refusal wording with it: the class is taken with `expect` rather than
/// skipped, so a catalog that no longer holds this operation fails where the table is built
/// instead of serving it with neither.
fn serve_behind_door<Eng, O>(
    table: &mut AuditedTable<BucketManagementEvents<Eng>>,
    operation: O,
) where
    Eng: Send + Sync + 'static,
    O: Operation + smithy_server_runtime::ErasedOperation,
{
    let kind = recorded_class(O::NAME)
        .expect("a configuration call served behind a door is a management event of this service");
    table.serve_behind(operation, kind, |audited, _| {
        ::service_common::front_door::FrontDoor::worded(audited, super::submitted_value_wording)
            .wording_uri(super::configuration_id_wording)
    });
}

/// Serve `operation` audited behind this service's front door with its answers COUNTED into
/// the addressed bucket's request metrics — the pair every listing of a bucket's contents is
/// served as.
///
/// The class is taken with `expect` for the reason [`serve_behind_door`] gives: this row
/// carries the door and the counting as well as the recorder.
fn serve_counted_behind_door<Eng, O>(
    table: &mut AuditedTable<BucketManagementEvents<Eng>>,
    engine: &Arc<Eng>,
    operation: O,
) where
    Eng: Send + Sync + 'static,
    O: Operation + smithy_server_runtime::ErasedOperation,
{
    let kind = recorded_class(O::NAME)
        .expect("a listing of what a bucket holds is a management event of this service");
    let engine = Arc::clone(engine);
    // Served behind the wrapper that admits the callers this service authenticates itself —
    // the ANONYMOUS caller for a listing that judges it, what a bucket's own `READ` grants to
    // everyone ([`super::unsigned_request::served_to_the_anonymous_caller`] is the catalog),
    // and the caller presenting a query credential no registered authenticator can read
    // ([`super::legacy_query_credential`]) — outside the counting and the door and inside the
    // recorder: admitting the caller decides whether the call is served at all, and a call
    // that IS served is counted and recorded like any other.
    table.serve_behind(operation, kind, move |audited, _| {
        super::unsigned_request::ServedUnsigned::new(
            O::NAME,
            Box::new(super::request_metrics::counted_behind_door(
                audited,
                &engine,
                super::request_metrics::RequestKind::List,
            )),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the calls CloudTrail records as management events of this service are the
    /// bucket-level ones, and no call on an object's contents is among them — a data event
    /// is never event history.
    #[test]
    fn the_two_event_classes_of_this_service_are_disjoint() {
        for (operation, kind) in MANAGEMENT_EVENT_OPERATIONS {
            assert_eq!(
                recorded_class(operation),
                Some(*kind),
                "case {operation:?}: a cataloged operation is recorded with its own class"
            );
            assert_eq!(
                super::super::data_events::recorded_class(operation),
                None,
                "case {operation:?}: a management event is no data event"
            );
        }
        for (operation, _, _) in super::super::data_events::DATA_EVENT_OPERATIONS {
            assert_eq!(
                recorded_class(operation),
                None,
                "case {operation:?}: a data event is no management event"
            );
        }
    }

    /// RULE: every operation the catalog names is SERVED audited — the table this service
    /// resolves an operation out of before the generated one holds a row for each, so no
    /// bucket-level request can be served by a path that records nothing.
    #[test]
    fn every_cataloged_operation_is_served_from_the_front_door() {
        let engine = Arc::new(::engine::InMemoryEngine::new());
        let table = super::super::front_door(&engine);
        for (operation, _) in MANAGEMENT_EVENT_OPERATIONS {
            assert!(
                table.get(operation).is_some(),
                "case {operation:?}: expected the front door to serve this operation audited"
            );
        }
    }

    /// RULE: every operation THE MODEL declares is in exactly one of the three sets — a
    /// management event of this service, a data event of it, or deliberately in neither
    /// ([`NEITHER_EVENT_CLASS`]) — and an operation the model declares `@readonly` is
    /// recorded as a REPORTING call.
    ///
    /// Held against the model itself rather than against a second hand-written list, so an
    /// operation added to the model later fails here instead of being served unrecorded, and
    /// a name that leaves the model fails here instead of sitting in a catalog forever.
    #[test]
    fn the_model_s_whole_operation_surface_is_classed() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-s3-amazons3.json"
        );
        let read = ::std::fs::read(path).expect("the crate's own Smithy model is readable");
        let model: ::serde_json::Value =
            ::serde_json::from_slice(&read).expect("the Smithy model is JSON");
        let shapes = model
            .get("shapes")
            .and_then(::serde_json::Value::as_object)
            .expect("a Smithy model holds its shapes");

        let mut modelled: Vec<String> = Vec::new();
        for (shape_id, shape) in shapes {
            if shape.get("type").and_then(::serde_json::Value::as_str) != Some("operation") {
                continue;
            }
            let name = shape_id
                .rsplit('#')
                .next()
                .expect("a shape id names its shape")
                .to_string();
            let read_only = shape
                .get("traits")
                .and_then(|traits| traits.get("smithy.api#readonly"))
                .is_some();
            let management = recorded_class(&name);
            let data = super::super::data_events::recorded_class(&name);
            let neither = NEITHER_EVENT_CLASS.contains(&name.as_str());
            let classed = usize::from(management.is_some())
                + usize::from(data.is_some())
                + usize::from(neither);
            assert_eq!(
                classed, 1,
                "case {name:?}: expected exactly one class (management {management:?}, data \
                 {data:?}, neither {neither})"
            );
            if read_only {
                if let Some(kind) = management {
                    assert_eq!(
                        kind,
                        CallKind::Reporting,
                        "case {name:?}: an operation the model declares @readonly reports state"
                    );
                }
            }
            modelled.push(name);
        }

        // And nothing in a set that the model does not declare.
        for named in MANAGEMENT_EVENT_OPERATIONS
            .iter()
            .map(|(name, _)| *name)
            .chain(
                super::super::data_events::DATA_EVENT_OPERATIONS
                    .iter()
                    .map(|(name, _, _)| *name),
            )
            .chain(NEITHER_EVENT_CLASS.iter().copied())
        {
            assert!(
                modelled.iter().any(|name| name == named),
                "case {named:?}: expected an operation this model declares, got {} modelled \
                 operations",
                modelled.len()
            );
        }
    }
}
