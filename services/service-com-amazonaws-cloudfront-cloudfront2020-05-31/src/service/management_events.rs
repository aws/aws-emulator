// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `cloudfront.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it — and, while a trail of the account is logging, each MUTATING
//! call published onto the account's default EventBridge bus as `AWS API Call via
//! CloudTrail` (that channel carries the calls that CHANGE an account and not the ones that
//! only report it, which is the shared fan-out's rule:
//! [`service_common::api_call_events`]).
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs and the handler's own `Result` after, and
//! this module is only what THIS service contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region its records are filed in ([`EVENT_REGION`]) and the account they belong to;
//! * the members its records never carry in the clear ([`SENSITIVE_MEMBERS`]);
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with ([`AuditedDefinition`], nested inside this
//! service's own `restXml` error-envelope definition) — so no request can be served by a
//! path that records nothing.
//!
//! The EDGE data plane is not in it, and must not be: a viewer request to a distribution's
//! own domain is not an API call of this account at all (it carries no AWS credential, and
//! names no modelled operation), so it is no management event. It is served by the seam
//! `crate::service` routes it to, outside this table.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable};
use service_common::management_events::{
    ManagementEventLog, RecordedSession, ServedApiCall, SessionAuditDirectory,
};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// This service's model-derived half, routing through the AUDITED operations — the shared
/// wrapper every recording service uses, re-exported under the name this crate holds it by.
pub use service_common::audited_operation::AuditedDefinition;

/// The `eventSource` every record of this service carries, which is the one the model
/// itself states for it (`smithy_query(query="service_info")` ->
/// `awsService.cloudTrailEventSource: cloudfront.amazonaws.com`).
pub const EVENT_SOURCE: &str = "cloudfront.amazonaws.com";

/// The Region this service's records are filed in.
///
/// CloudFront is a GLOBAL service — every ARN it mints carries an empty Region segment
/// (`crate::state::distribution_arn`), and the model gives it one partitional endpoint per
/// partition, in the partition's leader Region
/// (`smithy_query(query="service_info")` -> `standardPartitionalEndpoints`, whose `aws`
/// special case is `region: us-east-1`). So a record of a CloudFront call does not report
/// the Region a client happened to sign for: the account's CloudFront calls are all
/// recorded in that one Region, which is where a lookup over CloudFront's event history is
/// made and where a rule matching its API-call events has to live.
pub const EVENT_REGION: &str = service_common::management_events::PARTITION_LEADER_REGION;

/// The request and response members this service's records carry CloudTrail's redaction
/// literal in place of, rather than the value the call carried.
///
/// Every member this service's model declares `@sensitive`, named as the model spells it:
/// `smithy_query(query="shape", shape="CreateFunctionRequest,UpdateFunctionRequest,CreateConnectionFunctionRequest,UpdateConnectionFunctionRequest,TestFunctionRequest,TestConnectionFunctionRequest,TestResult,ConnectionFunctionTestResult")`
/// reports each of these as `"sensitive": true` (the four sensitive shapes they target
/// being `FunctionBlob`, `FunctionEventObject`, `FunctionExecutionLogList` and
/// `sensitiveStringType`). The set is by member NAME, so it holds for every operation that
/// carries a member of that name — the code on a function create and the same code on its
/// update are both withheld.
///
/// REDACTED rather than omitted: what is withheld here is a caller's own function source,
/// the event object it was tested with and what that run printed — material whose PRESENCE
/// is part of the record of the call, unlike a credential, whose absence from the record is
/// the point.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    // The function source a create or an update carries, for both function kinds.
    "FunctionCode",
    "ConnectionFunctionCode",
    // The event object a test run is driven with, for both function kinds.
    "EventObject",
    "ConnectionObject",
    // What a test run printed, what it answered, and the error it failed with.
    "FunctionExecutionLogs",
    "ConnectionFunctionExecutionLogs",
    "FunctionErrorMessage",
    "ConnectionFunctionErrorMessage",
    "FunctionOutput",
    "ConnectionFunctionOutput",
];

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct CloudFrontAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> CloudFrontAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for CloudFrontAudit<Eng> {
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
        // recorded as that session: the shared store the minting service wrote it to.
        session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // NOT the Region the request was signed for: this service is global, so every
        // record of it is filed in one Region (see [`EVENT_REGION`]).
        let _ = ctx;
        EVENT_REGION.to_string()
    }

    fn account_id(&self) -> String {
        ::emulator_server::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` name a resource by ARN, and a CloudFront resource's ARN
    // carries the id this service minted for it — which a create's request names nothing
    // of — so a record carries the call, its parameters and its outcome rather than a
    // resource built from a name alone.

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
/// this service speaks `restXml` with the wrapped error document, carrying its own
/// `@xmlNamespace` — the same values this service's error envelope states
/// ([`super::envelope::XML_NAMESPACE`]).
const RESTXML_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::RestXml {
    xml_namespace: Some(super::envelope::XML_NAMESPACE),
    no_error_wrapping: false,
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
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Get*` / `List*` /
/// `Describe*` readers, for the two `Test*` operations (each runs a function against an
/// event object the caller supplied and answers what it produced, changing nothing the
/// account then holds — which is why that pair's IAM actions are classified as reads), and
/// for `VerifyDnsConfiguration`, which answers what the account's DNS says; `mutates` for
/// every operation that creates, changes, publishes, associates, tags or removes something
/// the account then holds.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(CloudFrontAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, RESTXML_ENVELOPE);
    audited!(
        table, engine,
        associate_alias::AssociateAlias => mutates,
        associate_distribution_tenant_web_acl::AssociateDistributionTenantWebACL => mutates,
        associate_distribution_web_acl::AssociateDistributionWebACL => mutates,
        copy_distribution::CopyDistribution => mutates,
        create_anycast_ip_list::CreateAnycastIpList => mutates,
        create_cache_policy::CreateCachePolicy => mutates,
        create_cloud_front_origin_access_identity::CreateCloudFrontOriginAccessIdentity => mutates,
        create_connection_function::CreateConnectionFunction => mutates,
        create_connection_group::CreateConnectionGroup => mutates,
        create_continuous_deployment_policy::CreateContinuousDeploymentPolicy => mutates,
        create_distribution::CreateDistribution => mutates,
        create_distribution_tenant::CreateDistributionTenant => mutates,
        create_distribution_with_tags::CreateDistributionWithTags => mutates,
        create_field_level_encryption_config::CreateFieldLevelEncryptionConfig => mutates,
        create_field_level_encryption_profile::CreateFieldLevelEncryptionProfile => mutates,
        create_function::CreateFunction => mutates,
        create_invalidation::CreateInvalidation => mutates,
        create_invalidation_for_distribution_tenant::CreateInvalidationForDistributionTenant
            => mutates,
        create_key_group::CreateKeyGroup => mutates,
        create_key_value_store::CreateKeyValueStore => mutates,
        create_monitoring_subscription::CreateMonitoringSubscription => mutates,
        create_origin_access_control::CreateOriginAccessControl => mutates,
        create_origin_request_policy::CreateOriginRequestPolicy => mutates,
        create_public_key::CreatePublicKey => mutates,
        create_realtime_log_config::CreateRealtimeLogConfig => mutates,
        create_response_headers_policy::CreateResponseHeadersPolicy => mutates,
        create_streaming_distribution::CreateStreamingDistribution => mutates,
        create_streaming_distribution_with_tags::CreateStreamingDistributionWithTags => mutates,
        create_trust_store::CreateTrustStore => mutates,
        create_vpc_origin::CreateVpcOrigin => mutates,
        delete_anycast_ip_list::DeleteAnycastIpList => mutates,
        delete_cache_policy::DeleteCachePolicy => mutates,
        delete_cloud_front_origin_access_identity::DeleteCloudFrontOriginAccessIdentity => mutates,
        delete_connection_function::DeleteConnectionFunction => mutates,
        delete_connection_group::DeleteConnectionGroup => mutates,
        delete_continuous_deployment_policy::DeleteContinuousDeploymentPolicy => mutates,
        delete_distribution::DeleteDistribution => mutates,
        delete_distribution_tenant::DeleteDistributionTenant => mutates,
        delete_field_level_encryption_config::DeleteFieldLevelEncryptionConfig => mutates,
        delete_field_level_encryption_profile::DeleteFieldLevelEncryptionProfile => mutates,
        delete_function::DeleteFunction => mutates,
        delete_key_group::DeleteKeyGroup => mutates,
        delete_key_value_store::DeleteKeyValueStore => mutates,
        delete_monitoring_subscription::DeleteMonitoringSubscription => mutates,
        delete_origin_access_control::DeleteOriginAccessControl => mutates,
        delete_origin_request_policy::DeleteOriginRequestPolicy => mutates,
        delete_public_key::DeletePublicKey => mutates,
        delete_realtime_log_config::DeleteRealtimeLogConfig => mutates,
        delete_resource_policy::DeleteResourcePolicy => mutates,
        delete_response_headers_policy::DeleteResponseHeadersPolicy => mutates,
        delete_streaming_distribution::DeleteStreamingDistribution => mutates,
        delete_trust_store::DeleteTrustStore => mutates,
        delete_vpc_origin::DeleteVpcOrigin => mutates,
        describe_connection_function::DescribeConnectionFunction => reports,
        describe_function::DescribeFunction => reports,
        describe_key_value_store::DescribeKeyValueStore => reports,
        disassociate_distribution_tenant_web_acl::DisassociateDistributionTenantWebACL => mutates,
        disassociate_distribution_web_acl::DisassociateDistributionWebACL => mutates,
        get_anycast_ip_list::GetAnycastIpList => reports,
        get_cache_policy::GetCachePolicy => reports,
        get_cache_policy_config::GetCachePolicyConfig => reports,
        get_cloud_front_origin_access_identity::GetCloudFrontOriginAccessIdentity => reports,
        get_cloud_front_origin_access_identity_config::GetCloudFrontOriginAccessIdentityConfig
            => reports,
        get_connection_function::GetConnectionFunction => reports,
        get_connection_group::GetConnectionGroup => reports,
        get_connection_group_by_routing_endpoint::GetConnectionGroupByRoutingEndpoint => reports,
        get_continuous_deployment_policy::GetContinuousDeploymentPolicy => reports,
        get_continuous_deployment_policy_config::GetContinuousDeploymentPolicyConfig => reports,
        get_distribution::GetDistribution => reports,
        get_distribution_config::GetDistributionConfig => reports,
        get_distribution_tenant::GetDistributionTenant => reports,
        get_distribution_tenant_by_domain::GetDistributionTenantByDomain => reports,
        get_field_level_encryption::GetFieldLevelEncryption => reports,
        get_field_level_encryption_config::GetFieldLevelEncryptionConfig => reports,
        get_field_level_encryption_profile::GetFieldLevelEncryptionProfile => reports,
        get_field_level_encryption_profile_config::GetFieldLevelEncryptionProfileConfig => reports,
        get_function::GetFunction => reports,
        get_invalidation::GetInvalidation => reports,
        get_invalidation_for_distribution_tenant::GetInvalidationForDistributionTenant => reports,
        get_key_group::GetKeyGroup => reports,
        get_key_group_config::GetKeyGroupConfig => reports,
        get_managed_certificate_details::GetManagedCertificateDetails => reports,
        get_monitoring_subscription::GetMonitoringSubscription => reports,
        get_origin_access_control::GetOriginAccessControl => reports,
        get_origin_access_control_config::GetOriginAccessControlConfig => reports,
        get_origin_request_policy::GetOriginRequestPolicy => reports,
        get_origin_request_policy_config::GetOriginRequestPolicyConfig => reports,
        get_public_key::GetPublicKey => reports,
        get_public_key_config::GetPublicKeyConfig => reports,
        get_realtime_log_config::GetRealtimeLogConfig => reports,
        get_resource_policy::GetResourcePolicy => reports,
        get_response_headers_policy::GetResponseHeadersPolicy => reports,
        get_response_headers_policy_config::GetResponseHeadersPolicyConfig => reports,
        get_streaming_distribution::GetStreamingDistribution => reports,
        get_streaming_distribution_config::GetStreamingDistributionConfig => reports,
        get_trust_store::GetTrustStore => reports,
        get_vpc_origin::GetVpcOrigin => reports,
        list_anycast_ip_lists::ListAnycastIpLists => reports,
        list_cache_policies::ListCachePolicies => reports,
        list_cloud_front_origin_access_identities::ListCloudFrontOriginAccessIdentities => reports,
        list_conflicting_aliases::ListConflictingAliases => reports,
        list_connection_functions::ListConnectionFunctions => reports,
        list_connection_groups::ListConnectionGroups => reports,
        list_continuous_deployment_policies::ListContinuousDeploymentPolicies => reports,
        list_distribution_tenants::ListDistributionTenants => reports,
        list_distribution_tenants_by_customization::ListDistributionTenantsByCustomization
            => reports,
        list_distributions::ListDistributions => reports,
        list_distributions_by_anycast_ip_list_id::ListDistributionsByAnycastIpListId => reports,
        list_distributions_by_cache_policy_id::ListDistributionsByCachePolicyId => reports,
        list_distributions_by_connection_function::ListDistributionsByConnectionFunction
            => reports,
        list_distributions_by_connection_mode::ListDistributionsByConnectionMode => reports,
        list_distributions_by_key_group::ListDistributionsByKeyGroup => reports,
        list_distributions_by_origin_request_policy_id::ListDistributionsByOriginRequestPolicyId
            => reports,
        list_distributions_by_owned_resource::ListDistributionsByOwnedResource => reports,
        list_distributions_by_realtime_log_config::ListDistributionsByRealtimeLogConfig
            => reports,
        list_distributions_by_response_headers_policy_id::ListDistributionsByResponseHeadersPolicyId
            => reports,
        list_distributions_by_trust_store::ListDistributionsByTrustStore => reports,
        list_distributions_by_vpc_origin_id::ListDistributionsByVpcOriginId => reports,
        list_distributions_by_web_acl_id::ListDistributionsByWebACLId => reports,
        list_domain_conflicts::ListDomainConflicts => reports,
        list_field_level_encryption_configs::ListFieldLevelEncryptionConfigs => reports,
        list_field_level_encryption_profiles::ListFieldLevelEncryptionProfiles => reports,
        list_functions::ListFunctions => reports,
        list_invalidations::ListInvalidations => reports,
        list_invalidations_for_distribution_tenant::ListInvalidationsForDistributionTenant
            => reports,
        list_key_groups::ListKeyGroups => reports,
        list_key_value_stores::ListKeyValueStores => reports,
        list_origin_access_controls::ListOriginAccessControls => reports,
        list_origin_request_policies::ListOriginRequestPolicies => reports,
        list_public_keys::ListPublicKeys => reports,
        list_realtime_log_configs::ListRealtimeLogConfigs => reports,
        list_response_headers_policies::ListResponseHeadersPolicies => reports,
        list_streaming_distributions::ListStreamingDistributions => reports,
        list_tags_for_resource::ListTagsForResource => reports,
        list_trust_stores::ListTrustStores => reports,
        list_vpc_origins::ListVpcOrigins => reports,
        publish_connection_function::PublishConnectionFunction => mutates,
        publish_function::PublishFunction => mutates,
        put_resource_policy::PutResourcePolicy => mutates,
        tag_resource::TagResource => mutates,
        test_connection_function::TestConnectionFunction => reports,
        test_function::TestFunction => reports,
        untag_resource::UntagResource => mutates,
        update_anycast_ip_list::UpdateAnycastIpList => mutates,
        update_cache_policy::UpdateCachePolicy => mutates,
        update_cloud_front_origin_access_identity::UpdateCloudFrontOriginAccessIdentity => mutates,
        update_connection_function::UpdateConnectionFunction => mutates,
        update_connection_group::UpdateConnectionGroup => mutates,
        update_continuous_deployment_policy::UpdateContinuousDeploymentPolicy => mutates,
        update_distribution::UpdateDistribution => mutates,
        update_distribution_tenant::UpdateDistributionTenant => mutates,
        update_distribution_with_staging_config::UpdateDistributionWithStagingConfig => mutates,
        update_domain_association::UpdateDomainAssociation => mutates,
        update_field_level_encryption_config::UpdateFieldLevelEncryptionConfig => mutates,
        update_field_level_encryption_profile::UpdateFieldLevelEncryptionProfile => mutates,
        update_function::UpdateFunction => mutates,
        update_key_group::UpdateKeyGroup => mutates,
        update_key_value_store::UpdateKeyValueStore => mutates,
        update_origin_access_control::UpdateOriginAccessControl => mutates,
        update_origin_request_policy::UpdateOriginRequestPolicy => mutates,
        update_public_key::UpdatePublicKey => mutates,
        update_realtime_log_config::UpdateRealtimeLogConfig => mutates,
        update_response_headers_policy::UpdateResponseHeadersPolicy => mutates,
        update_streaming_distribution::UpdateStreamingDistribution => mutates,
        update_trust_store::UpdateTrustStore => mutates,
        update_vpc_origin::UpdateVpcOrigin => mutates,
        verify_dns_configuration::VerifyDnsConfiguration => reports,
    );
    table.finish()
}
