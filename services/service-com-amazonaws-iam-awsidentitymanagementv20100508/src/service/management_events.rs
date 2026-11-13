// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `iam.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler here. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs and the handler's own `Result` after, and
//! this module is only what THIS service contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region its records are filed in ([`EVENT_REGION`]) and the account the call was
//!   served for;
//! * the members whose values its records WITHHOLD, and how ([`OMITTED_MEMBERS`]);
//! * which of its refusals are decided BEFORE the operation runs, whose records carry no
//!   request parameters at all ([`AuditedService::stopped_at_authorization`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with ([`AuditedDefinition`]) — so no request can be
//! served by a path that records nothing.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::collections::BTreeMap;
use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{ManagementEventLog, RecordedSession, ServedApiCall};
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// This service's model-derived half, routing through the AUDITED operations — the shared
/// wrapper every recording service uses, re-exported under the name this service's
/// `EmulatedService` holds it by.
pub use service_common::audited_operation::AuditedDefinition;

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: iam`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "iam.amazonaws.com";

/// The Region this service's records are filed in.
///
/// IAM is a GLOBAL service — every ARN it mints carries an empty Region segment
/// (`crate::state::user_arn`) and every endpoint of it is served out of one Region — so a
/// record of an IAM call does not report the Region a client happened to sign for: the
/// account's IAM calls are all recorded in the partition's leader Region, which is where
/// a lookup over IAM's event history is made.
pub const EVENT_REGION: &str = "us-east-1";

/// The request and response members this service's records LEAVE OUT — no key for them in
/// `requestParameters` / `responseElements` at all, as against a key carrying CloudTrail's
/// redaction literal.
///
/// Every member this service's model declares `@sensitive`, named as the model spells it:
/// `smithy_query(query="shape", shape="AccessKey,CreateLoginProfileRequest,UpdateLoginProfileRequest,ChangePasswordRequest,ServiceSpecificCredential,VirtualMFADevice,UploadServerCertificateRequest,CreateSAMLProviderRequest,UpdateSAMLProviderRequest")`
/// reports each of these as `"sensitive": true`. The set is by member NAME, so it holds
/// for every operation that carries a member of that name — a console password on a create
/// and the same password on an update are both left out.
///
/// WHY omitted rather than redacted: the record of an IAM call carries no trace of the
/// credential material the call minted or was given, not even a placeholder saying a value
/// was there. That is this service's own recorded form, and it is what lets a client prove
/// the NEGATIVE — that a long-term secret or a console password was never persisted into
/// event history — which a placeholder could not.
pub const OMITTED_MEMBERS: &[&str] = &[
    // The long-term secret `CreateAccessKey` answers with, the one value a signed request
    // is computed from.
    "SecretAccessKey",
    // The console password on a login-profile create and update.
    "Password",
    // Both passwords a `ChangePassword` carries.
    "OldPassword",
    "NewPassword",
    // The generated password and secret of a service-specific credential.
    "ServicePassword",
    "ServiceCredentialSecret",
    // The virtual MFA device's shared seed, and the QR code that encodes it.
    "Base32StringSeed",
    "QRCodePNG",
    // The private key uploaded with a server certificate, and the one a SAML provider
    // decrypts assertions with.
    "PrivateKey",
    "AddPrivateKey",
];

/// The error code this service answers an authorization refusal with, as the record names
/// it: the fault shape's own name, which for this refusal is the code IAM uses on the wire
/// too (`crate::scaffolded::Scaffolded::shape_name`).
const ACCESS_DENIED_SHAPE_NAME: &str = "AccessDenied";

/// The status that refusal carries (`crate::scaffolded::Scaffolded::http_status`): the one
/// AWS answers an authorization refusal with.
const ACCESS_DENIED_STATUS: u16 = 403;

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct IamAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> IamAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for IamAudit<Eng> {
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
        // recorded as that session: the shared store the minting service wrote it to.
        session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // Nothing of the request decides it: this service is global, so every record of it
        // is filed in the one Region its event history lives in (see [`EVENT_REGION`]).
        let _ = ctx;
        EVENT_REGION.to_string()
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` are not this fix's to invent — a record naming a resource
    // nothing measured would be reported to a lookup by `ResourceName` as fact — so an IAM
    // record carries the call, its parameters and its outcome, and no resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        // Nothing is redacted to the placeholder literal: what this service withholds, it
        // withholds by OMISSION (see [`OMITTED_MEMBERS`]).
        &[]
    }

    fn omitted_members(&self) -> &'static [&'static str] {
        OMITTED_MEMBERS
    }

    fn stopped_at_authorization(
        &self,
        operation: &str,
        shape_name: &str,
        http_status: u16,
    ) -> bool {
        // This service authorizes its OWN API: every call it serves is decided against the
        // calling principal's identity-based policies by one gate
        // ([`crate::state::authorization`]) BEFORE the operation reads or writes anything,
        // and the refusal that gate answers is this code with this status. So a call refused
        // this way reached no operation, and its record carries neither the parameters it
        // would have been dispatched with nor any response elements — while every refusal an
        // operation reaches of its own keeps them, because that call WAS dispatched:
        // the refusals carrying another code (an entity that is not there, a name the
        // account already holds, a quota) and the operations below, which answer this one
        // themselves.
        shape_name == ACCESS_DENIED_SHAPE_NAME
            && http_status == ACCESS_DENIED_STATUS
            && !REFUSED_AFTER_DISPATCH.contains(&operation)
    }
}

/// The operations whose HANDLER answers an authorization refusal of its own, after the gate
/// has already admitted the call — so the call was dispatched, and its record carries the
/// parameters it was dispatched with like any other dispatched call's.
///
/// SOURCE, for adding an entry: a handler that builds the refusal itself
/// (`crate::scaffolded::Scaffolded::access_denied`) instead of letting the authorization
/// gate raise it.
const REFUSED_AFTER_DISPATCH: &[&str] = &[
    // The old password the change presented is not the one in force: a check against the
    // login profile the handler had already read, not a decision about what the caller may
    // do.
    "ChangePassword",
    // The account is not the management account of the organization the entity path names:
    // the requirement this operation's own documentation states, decided against the
    // organization the store holds after the gate admitted the call.
    "GenerateOrganizationsAccessReport",
];

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

/// The suffix CloudTrail's `eventSource` carries after the service's own namespace: a
/// record names the service that served a call as `<endpoint-prefix>.amazonaws.com`
/// ([`ServedApiCall::event_source`]), while the namespace a reader of a key's usage reports
/// is that service's name alone (`iam`).
const EVENT_SOURCE_SUFFIX: &str = ".amazonaws.com";

/// The service namespace `event_source` names — the `<prefix>` of its
/// `<prefix>.amazonaws.com` form, and the whole value for a source not in that form, since
/// no namespace is invented for one here.
fn service_namespace(event_source: &str) -> &str {
    event_source
        .strip_suffix(EVENT_SOURCE_SUFFIX)
        .unwrap_or(event_source)
}

/// One recorded USE of an access key: when the call was served, by which service, in which
/// Region.
///
/// What `GetAccessKeyLastUsed` reports about one key and what a credential report's usage
/// columns carry for every key, so the shape of that fact is declared once here beside the
/// log it is read off.
#[derive(Debug, Clone)]
pub struct AccessKeyUse {
    /// The instant the call was served, as the record stamped it.
    pub at: ::std::time::SystemTime,
    /// The service namespace the call was made to ([`service_namespace`]).
    pub service_name: String,
    /// The Region the call was served in, as the record carries it.
    pub region: String,
}

/// The most recent recorded use of EVERY access key the account's calls were signed with,
/// keyed by access key id.
///
/// `None` for a log this process cannot reach at all, which says nothing about any key
/// either way and is a server failure rather than an answer about usage; a key the log was
/// READ and holds no call for is simply absent from the map — it has not been used.
///
/// "Most recent" is by the instant the record was stamped with, with the log's own ordinal
/// breaking a tie between two calls served in the same millisecond — so the use reported is
/// the latest one whatever order the records were written in. That is why the whole log is
/// walked rather than scanned backwards to the first match: the log's order is the order
/// calls were RECORDED, which a recorder stamping an instant of its own (a call it served
/// earlier, replayed state) need not have in the order of their instants, and the rule is
/// about the instants.
///
/// ONE reading of that rule, for the two readers of it: the operation that reports one
/// key's last use and the credential report that carries every key's. A READ of the shared
/// log: nothing is written.
pub fn last_use_by_access_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<BTreeMap<String, AccessKeyUse>> {
    let log = management_event_log(engine)?;
    let held = log.lock();
    let mut latest: BTreeMap<String, (::std::time::SystemTime, u64)> = BTreeMap::new();
    let mut uses: BTreeMap<String, AccessKeyUse> = BTreeMap::new();
    for event in held.iter() {
        let Some(access_key_id) = event.call.access_key_id.as_deref() else {
            continue;
        };
        let stamp = (event.event_time, event.seq);
        if latest
            .get(access_key_id)
            .is_some_and(|held| *held >= stamp)
        {
            continue;
        }
        latest.insert(access_key_id.to_string(), stamp);
        uses.insert(
            access_key_id.to_string(),
            AccessKeyUse {
                at: event.event_time,
                service_name: service_namespace(&event.call.event_source).to_string(),
                region: event.call.aws_region.clone(),
            },
        );
    }
    Some(uses)
}

/// The account's SESSION directory in that same shared engine: the sessions STS minted,
/// each keyed by the access key id it was minted under, which is what lets a record of a
/// call made with one name the session it was made by.
///
/// A SHARED type, for the same reason as the log above: the service that MINTS a session
/// and the services that record the calls made WITH it are not the same service.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::SessionAuditDirectory,
            >,
        )
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsQuery` (not the EC2 dialect), which is the same value the
/// generated per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

/// A call that only REPORTS state, as the table's rows name that kind.
const fn reports() -> CallKind {
    CallKind::Reporting
}

/// A call that CHANGES the account's resources.
const fn mutates() -> CallKind {
    CallKind::Mutating
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                $kind(),
            );
        )*
    }};
}

/// Every operation this service serves, each constructed from the shared engine and served
/// behind the audit recorder.
///
/// EVERY one is here, the ones this emulator has not implemented included: a stub answers
/// 501 and the wrapper records nothing for it, so an operation implemented later is
/// auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Get*` / `List*`
/// readers and for the two `Simulate*` evaluations, which answer a decision over the
/// account's policies and change none of it, and `mutates` for every operation that
/// creates, changes, moves or removes a resource — the `Generate*` report jobs included,
/// since each one submits a job the account then holds.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(IamAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, QUERY_ENVELOPE);
    audited!(
        table, engine,
        accept_delegation_request::AcceptDelegationRequest => mutates,
        acquire_role::AcquireRole => mutates,
        add_client_id_to_open_id_connect_provider::AddClientIDToOpenIDConnectProvider => mutates,
        add_role_to_instance_profile::AddRoleToInstanceProfile => mutates,
        add_user_to_group::AddUserToGroup => mutates,
        associate_delegation_request::AssociateDelegationRequest => mutates,
        attach_group_policy::AttachGroupPolicy => mutates,
        attach_role_policy::AttachRolePolicy => mutates,
        attach_user_policy::AttachUserPolicy => mutates,
        change_password::ChangePassword => mutates,
        create_access_key::CreateAccessKey => mutates,
        create_account_alias::CreateAccountAlias => mutates,
        create_delegation_request::CreateDelegationRequest => mutates,
        create_group::CreateGroup => mutates,
        create_instance_profile::CreateInstanceProfile => mutates,
        create_login_profile::CreateLoginProfile => mutates,
        create_open_id_connect_provider::CreateOpenIDConnectProvider => mutates,
        create_policy::CreatePolicy => mutates,
        create_policy_version::CreatePolicyVersion => mutates,
        create_role::CreateRole => mutates,
        create_saml_provider::CreateSAMLProvider => mutates,
        create_service_linked_role::CreateServiceLinkedRole => mutates,
        create_service_specific_credential::CreateServiceSpecificCredential => mutates,
        create_user::CreateUser => mutates,
        create_virtual_mfa_device::CreateVirtualMFADevice => mutates,
        deactivate_mfa_device::DeactivateMFADevice => mutates,
        delete_access_key::DeleteAccessKey => mutates,
        delete_account_alias::DeleteAccountAlias => mutates,
        delete_account_password_policy::DeleteAccountPasswordPolicy => mutates,
        delete_group::DeleteGroup => mutates,
        delete_group_policy::DeleteGroupPolicy => mutates,
        delete_instance_profile::DeleteInstanceProfile => mutates,
        delete_login_profile::DeleteLoginProfile => mutates,
        delete_open_id_connect_provider::DeleteOpenIDConnectProvider => mutates,
        delete_policy::DeletePolicy => mutates,
        delete_policy_version::DeletePolicyVersion => mutates,
        delete_role::DeleteRole => mutates,
        delete_role_permissions_boundary::DeleteRolePermissionsBoundary => mutates,
        delete_role_policy::DeleteRolePolicy => mutates,
        delete_saml_provider::DeleteSAMLProvider => mutates,
        delete_server_certificate::DeleteServerCertificate => mutates,
        delete_service_linked_role::DeleteServiceLinkedRole => mutates,
        delete_service_specific_credential::DeleteServiceSpecificCredential => mutates,
        delete_signing_certificate::DeleteSigningCertificate => mutates,
        delete_ssh_public_key::DeleteSSHPublicKey => mutates,
        delete_user::DeleteUser => mutates,
        delete_user_permissions_boundary::DeleteUserPermissionsBoundary => mutates,
        delete_user_policy::DeleteUserPolicy => mutates,
        delete_virtual_mfa_device::DeleteVirtualMFADevice => mutates,
        detach_group_policy::DetachGroupPolicy => mutates,
        detach_role_policy::DetachRolePolicy => mutates,
        detach_user_policy::DetachUserPolicy => mutates,
        disable_organizations_root_credentials_management::DisableOrganizationsRootCredentialsManagement => mutates,
        disable_organizations_root_sessions::DisableOrganizationsRootSessions => mutates,
        disable_outbound_web_identity_federation::DisableOutboundWebIdentityFederation => mutates,
        enable_mfa_device::EnableMFADevice => mutates,
        enable_organizations_root_credentials_management::EnableOrganizationsRootCredentialsManagement => mutates,
        enable_organizations_root_sessions::EnableOrganizationsRootSessions => mutates,
        enable_outbound_web_identity_federation::EnableOutboundWebIdentityFederation => mutates,
        generate_credential_report::GenerateCredentialReport => mutates,
        generate_organizations_access_report::GenerateOrganizationsAccessReport => mutates,
        generate_service_last_accessed_details::GenerateServiceLastAccessedDetails => mutates,
        get_access_key_last_used::GetAccessKeyLastUsed => reports,
        get_account_authorization_details::GetAccountAuthorizationDetails => reports,
        get_account_password_policy::GetAccountPasswordPolicy => reports,
        get_account_properties::GetAccountProperties => reports,
        get_context_keys_for_custom_policy::GetContextKeysForCustomPolicy => reports,
        get_context_keys_for_principal_policy::GetContextKeysForPrincipalPolicy => reports,
        get_credential_report::GetCredentialReport => reports,
        get_delegation_request::GetDelegationRequest => reports,
        get_group::GetGroup => reports,
        get_group_policy::GetGroupPolicy => reports,
        get_human_readable_summary::GetHumanReadableSummary => reports,
        get_instance_profile::GetInstanceProfile => reports,
        get_login_profile::GetLoginProfile => reports,
        get_mfa_device::GetMFADevice => reports,
        get_open_id_connect_provider::GetOpenIDConnectProvider => reports,
        get_organizations_access_report::GetOrganizationsAccessReport => reports,
        get_outbound_web_identity_federation_info::GetOutboundWebIdentityFederationInfo => reports,
        get_policy::GetPolicy => reports,
        get_policy_version::GetPolicyVersion => reports,
        get_role::GetRole => reports,
        get_role_policy::GetRolePolicy => reports,
        get_role_template_version::GetRoleTemplateVersion => reports,
        get_saml_provider::GetSAMLProvider => reports,
        get_server_certificate::GetServerCertificate => reports,
        get_service_last_accessed_details::GetServiceLastAccessedDetails => reports,
        get_service_last_accessed_details_with_entities::GetServiceLastAccessedDetailsWithEntities => reports,
        get_service_linked_role_deletion_status::GetServiceLinkedRoleDeletionStatus => reports,
        get_ssh_public_key::GetSSHPublicKey => reports,
        get_user::GetUser => reports,
        get_user_policy::GetUserPolicy => reports,
        list_access_keys::ListAccessKeys => reports,
        list_account_aliases::ListAccountAliases => reports,
        list_attached_group_policies::ListAttachedGroupPolicies => reports,
        list_attached_role_policies::ListAttachedRolePolicies => reports,
        list_attached_user_policies::ListAttachedUserPolicies => reports,
        list_delegation_requests::ListDelegationRequests => reports,
        list_entities_for_policy::ListEntitiesForPolicy => reports,
        list_group_policies::ListGroupPolicies => reports,
        list_groups::ListGroups => reports,
        list_groups_for_user::ListGroupsForUser => reports,
        list_instance_profile_tags::ListInstanceProfileTags => reports,
        list_instance_profiles::ListInstanceProfiles => reports,
        list_instance_profiles_for_role::ListInstanceProfilesForRole => reports,
        list_mfa_device_tags::ListMFADeviceTags => reports,
        list_mfa_devices::ListMFADevices => reports,
        list_open_id_connect_provider_tags::ListOpenIDConnectProviderTags => reports,
        list_open_id_connect_providers::ListOpenIDConnectProviders => reports,
        list_organizations_features::ListOrganizationsFeatures => reports,
        list_policies::ListPolicies => reports,
        list_policies_granting_service_access::ListPoliciesGrantingServiceAccess => reports,
        list_policy_tags::ListPolicyTags => reports,
        list_policy_versions::ListPolicyVersions => reports,
        list_role_policies::ListRolePolicies => reports,
        list_role_tags::ListRoleTags => reports,
        list_roles::ListRoles => reports,
        list_saml_provider_tags::ListSAMLProviderTags => reports,
        list_saml_providers::ListSAMLProviders => reports,
        list_server_certificate_tags::ListServerCertificateTags => reports,
        list_server_certificates::ListServerCertificates => reports,
        list_service_specific_credentials::ListServiceSpecificCredentials => reports,
        list_signing_certificates::ListSigningCertificates => reports,
        list_ssh_public_keys::ListSSHPublicKeys => reports,
        list_user_policies::ListUserPolicies => reports,
        list_user_tags::ListUserTags => reports,
        list_users::ListUsers => reports,
        list_virtual_mfa_devices::ListVirtualMFADevices => reports,
        put_account_properties::PutAccountProperties => mutates,
        put_group_policy::PutGroupPolicy => mutates,
        put_role_permissions_boundary::PutRolePermissionsBoundary => mutates,
        put_role_policy::PutRolePolicy => mutates,
        put_user_permissions_boundary::PutUserPermissionsBoundary => mutates,
        put_user_policy::PutUserPolicy => mutates,
        reject_delegation_request::RejectDelegationRequest => mutates,
        remove_client_id_from_open_id_connect_provider::RemoveClientIDFromOpenIDConnectProvider => mutates,
        remove_role_from_instance_profile::RemoveRoleFromInstanceProfile => mutates,
        remove_user_from_group::RemoveUserFromGroup => mutates,
        reset_service_specific_credential::ResetServiceSpecificCredential => mutates,
        resync_mfa_device::ResyncMFADevice => mutates,
        send_delegation_token::SendDelegationToken => mutates,
        set_default_policy_version::SetDefaultPolicyVersion => mutates,
        set_security_token_service_preferences::SetSecurityTokenServicePreferences => mutates,
        simulate_custom_policy::SimulateCustomPolicy => reports,
        simulate_principal_policy::SimulatePrincipalPolicy => reports,
        tag_instance_profile::TagInstanceProfile => mutates,
        tag_mfa_device::TagMFADevice => mutates,
        tag_open_id_connect_provider::TagOpenIDConnectProvider => mutates,
        tag_policy::TagPolicy => mutates,
        tag_role::TagRole => mutates,
        tag_saml_provider::TagSAMLProvider => mutates,
        tag_server_certificate::TagServerCertificate => mutates,
        tag_user::TagUser => mutates,
        untag_instance_profile::UntagInstanceProfile => mutates,
        untag_mfa_device::UntagMFADevice => mutates,
        untag_open_id_connect_provider::UntagOpenIDConnectProvider => mutates,
        untag_policy::UntagPolicy => mutates,
        untag_role::UntagRole => mutates,
        untag_saml_provider::UntagSAMLProvider => mutates,
        untag_server_certificate::UntagServerCertificate => mutates,
        untag_user::UntagUser => mutates,
        update_access_key::UpdateAccessKey => mutates,
        update_account_password_policy::UpdateAccountPasswordPolicy => mutates,
        update_assume_role_policy::UpdateAssumeRolePolicy => mutates,
        update_delegation_request::UpdateDelegationRequest => mutates,
        update_group::UpdateGroup => mutates,
        update_login_profile::UpdateLoginProfile => mutates,
        update_open_id_connect_provider_thumbprint::UpdateOpenIDConnectProviderThumbprint => mutates,
        update_role::UpdateRole => mutates,
        update_role_description::UpdateRoleDescription => mutates,
        update_saml_provider::UpdateSAMLProvider => mutates,
        update_server_certificate::UpdateServerCertificate => mutates,
        update_service_specific_credential::UpdateServiceSpecificCredential => mutates,
        update_signing_certificate::UpdateSigningCertificate => mutates,
        update_ssh_public_key::UpdateSSHPublicKey => mutates,
        update_user::UpdateUser => mutates,
        upload_server_certificate::UploadServerCertificate => mutates,
        upload_signing_certificate::UploadSigningCertificate => mutates,
        upload_ssh_public_key::UploadSSHPublicKey => mutates,
    );
    table.finish()
}
