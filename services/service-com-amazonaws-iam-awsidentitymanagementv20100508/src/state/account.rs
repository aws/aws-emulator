// SPDX-License-Identifier: CC0-1.0

//! The account's own IAM state: the records it holds and the reads and writes over them.
//!
//! [`super::ComAmazonawsIamAwsidentitymanagementv20100508State`] is the one store every
//! handler reaches through [`super::service_state`]; this module holds that struct, the
//! records it keeps (report jobs, server certificates, the organization it belongs to) and
//! the queries and writes its handlers make over them. The constants, identifier helpers,
//! ARN builders and published catalogs those reads resolve against stay in the module
//! root beside it.

#[allow(unused_imports)]
use super::*;

/// One Organizations service-last-accessed report job the account has asked for, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::organizations_access_reports`] by
/// the `JobId` the generate answered with.
///
/// `GenerateOrganizationsAccessReport` "generates a report for service last accessed data
/// for Organizations" and answers only a job id: the report itself is what
/// `GetOrganizationsAccessReport` reports, which is why the job is a stored record rather
/// than an answer computed on the spot.
#[derive(Debug, Clone)]
pub struct OrganizationsAccessReportJob {
    /// "The path of the Organizations entity (root, OU, or account)"
    /// (`GenerateOrganizationsAccessReport$EntityPath`) the report was asked for, which
    /// the report's own entries name.
    pub entity_path: String,
    /// "The identifier of the Organizations service control policy (SCP)" the request
    /// narrowed the report to, when it named one.
    pub organizations_policy_id: Option<String>,
    /// "The date and time, in ISO 8601 date-time format, when the report job was created"
    /// (`GetOrganizationsAccessReport$JobCreationDate`).
    pub created: ::aws_smithy_types::DateTime,
    /// The instant the generation is DUE: until it, the read reports the job
    /// `IN_PROGRESS`; from it, `COMPLETED` with this instant as its
    /// `JobCompletionDate`. Stored rather than slept for, so a caller that polls
    /// straight after the generate still sees the job in flight.
    pub completes: ::aws_smithy_types::DateTime,
}

/// One service-last-accessed report job the account has asked for, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::service_last_accessed_reports`]
/// by the `JobId` the generate answered with.
///
/// `GenerateServiceLastAccessedDetails` "generates a report that includes details about
/// when an IAM resource (user, group, role, or policy) was last used in an attempt to
/// access Amazon Web Services services" and answers only a job id: the report itself is
/// what `GetServiceLastAccessedDetails` and `GetServiceLastAccessedDetailsWithEntities`
/// report, so the job records WHICH resource and WHICH level of detail were asked for
/// rather than an answer computed on the spot.
#[derive(Debug, Clone)]
pub struct ServiceLastAccessedJob {
    /// "The ARN of the IAM resource (user, group, role, or managed policy) used to
    /// generate information about when the resource was last used in an attempt to
    /// access an Amazon Web Services service"
    /// (`GenerateServiceLastAccessedDetails$Arn`), as the request supplied it: the
    /// resource whose permissions policies the report is about.
    pub arn: String,
    /// "The level of detail that you want to generate"
    /// (`GenerateServiceLastAccessedDetails$Granularity`), reported back by the read as
    /// its `JobType`. Held RESOLVED: a request that omits the parameter asks for
    /// service data ("if you don't include this optional parameter, the operation
    /// generates service data"), which is [`USAGE_GRANULARITY_SERVICE_LEVEL`].
    pub granularity: String,
    /// "The date and time, in ISO 8601 date-time format, when the report job was
    /// created" (`GetServiceLastAccessedDetails$JobCreationDate`).
    pub created: ::aws_smithy_types::DateTime,
    /// The instant the generation is DUE, read through [`report_job_status`] exactly as
    /// an Organizations report job's is ([`OrganizationsAccessReportJob::completes`]):
    /// stored rather than slept for, so a caller polling straight after the generate
    /// still sees the job in flight.
    pub completes: ::aws_smithy_types::DateTime,
}

/// One service-linked role deletion the account has submitted, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::service_linked_role_deletions`]
/// by the `DeletionTaskId` the submit answered with.
///
/// `DeleteServiceLinkedRole` "submits a service-linked role deletion request and returns
/// a DeletionTaskId, which you can use to check the status of the deletion", so the
/// submit records the task and `GetServiceLinkedRoleDeletionStatus` is what reports it.
/// The record says WHICH role the deletion was submitted for and WHEN it is due, so the
/// status a read reports is read off the clock rather than invented per read.
#[derive(Debug, Clone)]
pub struct ServiceLinkedRoleDeletionTask {
    /// The key the deleted role was held under
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::roles`]), which is what
    /// lets a repeated submit for one role find the task already in flight for it.
    pub role_key: String,
    /// The instant the deletion was submitted, which orders the tasks by age for the
    /// bounded retention [`MAX_RETAINED_SERVICE_LINKED_ROLE_DELETION_TASKS`] states.
    pub created: ::aws_smithy_types::DateTime,
    /// The instant the deletion is DUE: until it, a read reports the task
    /// `IN_PROGRESS`; from it, `SUCCEEDED`. Stored rather than slept for, so a caller
    /// that polls straight after the submit still sees the deletion in flight.
    pub completes: ::aws_smithy_types::DateTime,
}

/// The account's credential-report generation, as
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report`] holds it.
///
/// `GenerateCredentialReport` "generates a credential report for the Amazon Web Services
/// account" and answers only the `State` of that generation, so what the account has to
/// remember between two calls is WHEN the generation was asked for, WHEN it is due, and
/// whether a caller has already been told it finished — the three facts the state a call
/// reports is read off.
#[derive(Debug, Clone)]
pub struct CredentialReportGeneration {
    /// The instant the generation was asked for, which is what the once-every-four-hours
    /// rate ([`CREDENTIAL_REPORT_REGENERATION_SECONDS`]) is measured from.
    ///
    /// It is also the instant the report this generation produces reports as its
    /// `GeneratedTime`: the report is the snapshot of the account taken when the
    /// generation was asked for ([`Self::content`]), so the artifact and the instant it
    /// names are one, and the window the report stays readable for
    /// ([`CREDENTIAL_REPORT_VALIDITY_SECONDS`]) runs from that same instant — which is
    /// why it ends exactly where this record stops holding a new generation back.
    pub requested: ::aws_smithy_types::DateTime,
    /// The instant the generation is DUE: until it, a call reports the generation
    /// `INPROGRESS`; from it, the report exists. Stored rather than slept for, so a caller
    /// that calls again straight away still sees the generation in flight — the same clock
    /// rule every asynchronous job of this service is read through
    /// ([`report_job_status`]).
    pub completes: ::aws_smithy_types::DateTime,
    /// Whether a call has already reported this generation `COMPLETE`.
    ///
    /// The terminal state of a generation is reported ONCE, to the call that first finds it
    /// due; from then on the account HAS that report and a further call is asking for a new
    /// one, which the published rate is what decides. That ordering is what was measured: a
    /// repeated request inside the window is refused for exceeding the rate rather than
    /// answered `COMPLETE` for the rest of it, so the completion cannot be the window's
    /// standing answer.
    pub reported_complete: bool,
    /// The report this generation produces: the `text/csv` artifact
    /// `GetCredentialReport$Content` carries, rendered from the account's credentials by
    /// the call that STARTED the generation
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report_csv`]).
    ///
    /// Held here, rather than rendered per read, because a report is a SNAPSHOT: it is
    /// made once, and from the instant it is due every read of it answers the same bytes
    /// and the same [`Self::requested`] as its `GeneratedTime` — so reading one changes
    /// nothing a later caller sees, and two reads cannot disagree about one report.
    pub content: String,
}

/// One persisted server certificate: the metadata every operation reports about it,
/// beside the PEM material a read may hand back.
///
/// The uploaded PRIVATE KEY is deliberately not a field. `ServerCertificate` — the only
/// shape any operation reports a server certificate through — carries just the metadata,
/// the certificate body and the chain
/// (`smithy_query(query="shape", shape="ServerCertificate")`), and the model marks
/// `UploadServerCertificate$PrivateKey` sensitive, so nothing can ever read the key back:
/// holding it would be keeping a secret no caller may be given. It is read once, by the
/// upload, to check that it belongs to the certificate's public key.
#[derive(Debug, Clone)]
pub struct ServerCertificateRecord {
    /// "The meta information of the uploaded server certificate without its certificate
    /// body, certificate chain, and private key"
    /// (`UploadServerCertificate$ServerCertificateMetadata`): the path, the name, the
    /// minted id, the ARN, the instant it was uploaded and the instant the certificate
    /// itself expires. Stored whole, so the upload response and every later read report
    /// one record.
    pub metadata: ServerCertificateMetadata,
    /// "The contents of the public key certificate" (`ServerCertificate$CertificateBody`),
    /// verbatim as the upload carried it.
    pub certificate_body: String,
    /// "The contents of the public key certificate chain"
    /// (`ServerCertificate$CertificateChain`), when the upload carried one.
    pub certificate_chain: Option<String>,
    /// The tags attached to the certificate, sorted by tag key, because "the returned list
    /// of tags is sorted by tag key" (`UploadServerCertificate$Tags`). `None` when the
    /// upload attached none.
    pub tags: Option<Vec<Tag>>,
}

/// A FIDO security key the account holds as an MFA device, as `GetMFADevice` reports it.
///
/// `GetMFADevice` is the read that serves this kind of device and only this kind — "for
/// this API, we only accept FIDO security key ARNs"
/// (`smithy_query(query="doc",shape="GetMFADeviceRequest",member="SerialNumber")`) — so
/// the record carries exactly what that response reports: the serial the device is
/// addressed by, the user it is enabled for, when it was first enabled and the
/// certification levels it was published with.
#[derive(Debug, Clone)]
pub struct FidoSecurityKey {
    /// "Serial number that uniquely identifies the MFA device"
    /// (`GetMFADeviceResponse$SerialNumber`) — the key this device is held under, and the
    /// value a request addresses it by.
    pub serial_number: String,
    /// [`name_key`] of the IAM user the device is enabled for: the key
    /// [`ComAmazonawsIamAwsidentitymanagementv20100508State::users`] holds that user
    /// under, so the device is reported for the user the account still holds under that
    /// name however the name was spelled.
    pub user_key: String,
    /// "The date that a specified user's MFA device was first enabled"
    /// (`GetMFADeviceResponse$EnableDate`).
    pub enable_date: ::aws_smithy_types::DateTime,
    /// "The certifications of a specified user's MFA device … obtained from FIDO Alliance
    /// Metadata Service (MDS)" (`GetMFADeviceResponse$Certifications`), keyed by
    /// certification name. A `BTreeMap` because the modelled member is a map, so what a
    /// read reports is the SET of pairs the device carries rather than an order.
    pub certifications: BTreeMap<String, String>,
}

/// The organization the account belongs to, as IAM sees it: which organization it is,
/// which account manages it, and what it carries for centralized root access.
///
/// IAM reads this where an operation of its own is served only to an organization's
/// management account — `GenerateOrganizationsAccessReport` ("to call this operation, you
/// must be signed in using your Organizations management account credentials") — so the
/// two facts such a decision needs are held in ONE place: the organization's id, which an
/// Organizations entity path names, and the account that manages it. The centralized root
/// access facts are held beside them, because they are facts about that SAME organization:
/// its feature set, whether IAM has trusted access to it, which accounts administer that
/// access, and which of the modelled features are enabled for it.
#[derive(Debug, Clone)]
pub struct Organization {
    /// "The unique identifier (ID) of an organization" (`OrganizationIdType`, pattern
    /// `^o-[a-z0-9]{10,32}$`) — the value an entity path's first segment carries.
    pub organization_id: String,
    /// The account that manages the organization: the one whose credentials IAM serves
    /// the organization-wide operations to, and the account every other member is
    /// refused as.
    pub management_account_id: String,
    /// Whether the organization has "All features" enabled, the Organizations feature set
    /// the centralized root access operations require: a request against an organization
    /// without it "was rejected because your organization does not have All features
    /// enabled" (`OrganizationNotInAllFeaturesModeException`).
    pub all_features_enabled: bool,
    /// Whether trusted access is enabled for IAM in Organizations, the other standing
    /// requirement of the centralized root access operations: without it a request "was
    /// rejected because trusted access is not enabled for IAM in Organizations"
    /// (`ServiceAccessNotEnabledException`).
    pub iam_trusted_access_enabled: bool,
    /// The member accounts registered as a delegated administrator for centralized root
    /// access, which — beside [`Self::management_account_id`] — are the accounts those
    /// operations are served to ("the account making the request is not the management
    /// account or delegated administrator account for centralized root access",
    /// `AccountNotManagementOrDelegatedAdministratorException`). A `BTreeSet`, because what
    /// it holds is a SET of accounts rather than an order.
    pub root_access_delegated_administrator_ids: BTreeSet<String>,
    /// The centralized root access features currently enabled for the organization, held as
    /// the SET of `FeatureType` values enabled (`ops::types::FEATURE_TYPE_VALUES`): what
    /// `EnableOrganizationsRootCredentialsManagement` and `EnableOrganizationsRootSessions`
    /// add to, what their `Disable` counterparts take away, and what
    /// `ListOrganizationsFeatures` reports through [`Self::reported_root_access_features`].
    pub enabled_root_access_features: BTreeSet<String>,
}

impl Organization {
    /// Whether the centralized root access operations are served to `account_id`: the
    /// organization's management account and the accounts registered as a delegated
    /// administrator for that access are, and every other account — a member account of
    /// this organization, or an account outside it — is not.
    pub fn serves_root_access_to(&self, account_id: &str) -> bool {
        self.management_account_id == account_id
            || self
                .root_access_delegated_administrator_ids
                .contains(account_id)
    }

    /// The organization's enabled centralized root access features as a reported
    /// `FeaturesListType` value: the `FeatureType` values the model declares
    /// ([`crate::ops::types::FEATURE_TYPE_VALUES`]) that [`Self::enabled_root_access_features`]
    /// holds, in the order the model declares them, so the list a read reports is the
    /// modelled catalog narrowed to what is enabled rather than whatever spelling or order
    /// a write happened to leave.
    pub fn reported_root_access_features(&self) -> Vec<String> {
        crate::ops::types::FEATURE_TYPE_VALUES
            .iter()
            .filter(|feature| self.enabled_root_access_features.contains(**feature))
            .map(|feature| (*feature).to_string())
            .collect()
    }
}

/// Why the centralized root access operations are not served to a caller: the standing
/// requirement of theirs the account does not meet, each the modelled error one of those
/// operations answers with.
///
/// The requirements are the organization's, not one operation's, so they are decided in ONE
/// place ([`ComAmazonawsIamAwsidentitymanagementv20100508State::centralized_root_access_organization`])
/// and every operation that has them maps the answer onto its own error channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootAccessPrecondition {
    /// The account belongs to no organization at all.
    OrganizationNotFound,
    /// Its organization does not have "All features" enabled.
    OrganizationNotInAllFeaturesMode,
    /// Trusted access is not enabled for IAM in Organizations.
    ServiceAccessNotEnabled,
    /// The account is neither the management account nor a delegated administrator for
    /// centralized root access.
    AccountNotManagementOrDelegatedAdministrator,
}

impl RootAccessPrecondition {
    /// The message the refusal carries: the reason AWS states for this error, as the
    /// modelled exception itself words it.
    pub fn message(self) -> &'static str {
        match self {
            Self::OrganizationNotFound => {
                "The request was rejected because no organization is associated with your \
                 account."
            }
            Self::OrganizationNotInAllFeaturesMode => {
                "The request was rejected because your organization does not have All features \
                 enabled."
            }
            Self::ServiceAccessNotEnabled => {
                "The request was rejected because trusted access is not enabled for IAM in \
                 Organizations."
            }
            Self::AccountNotManagementOrDelegatedAdministrator => {
                "The request was rejected because the account making the request is not the \
                 management account or delegated administrator account for centralized root \
                 access."
            }
        }
    }
}

/// The organization an Organizations entity path names: the `o-…` segment it begins with.
///
/// "You can build an entity path using the known structure of your organization … your
/// entity path is `o-a1b2c3d4e5/r-f6g7h8i9j0example/ou-rge0-awsabcde/123456789012`"
/// (`GenerateOrganizationsAccessReport$EntityPath`), so the organization is the first
/// segment and the root, the organizational units and the account follow it. `None` for a
/// value that carries no such segment — which the member's own pattern
/// (`^o-[0-9a-z]{10,32}\/r-…`) already refuses before a handler sees it.
pub fn organizations_entity_path_organization(entity_path: &str) -> Option<&str> {
    let organization = entity_path.split('/').next()?;
    organization
        .strip_prefix("o-")
        .is_some_and(|id| !id.is_empty())
        .then_some(organization)
}

/// Per-service state for com-amazonaws-iam-awsidentitymanagementv20100508. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` so enumeration order is total and stable, which is
/// what lets a list operation hand out a resumable marker: the next page starts
/// strictly after the key the marker names.
///
/// `Default` is NOT derived: the account does not start out empty. It starts out holding
/// the IAM user this emulator's own long-term credential belongs to
/// ([`Self::default`]), which is why [`Self::empty`] — the account with nothing at all in
/// it — is a separate constructor.
#[derive(Debug)]
pub struct ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The persisted IAM users, keyed by [`name_key`] of the user name. IAM names
    /// "are not distinguished by case" (`CreateUser$UserName` documentation), so
    /// the key is the case-folded name while `User::user_name` keeps the case the
    /// creator chose.
    pub users: BTreeMap<String, User>,
    /// The persisted IAM roles, keyed by [`name_key`] of the role name, for the
    /// same reason users are: IAM entity names are unique within the account and
    /// "are not distinguished by case", while `Role::role_name` keeps the case the
    /// name was minted with.
    pub roles: BTreeMap<String, Role>,
    /// The persisted IAM groups, keyed by [`name_key`] of the group name, for the
    /// same reason users and roles are: IAM entity names are unique within the
    /// account and "are not distinguished by case" (`CreateGroup$GroupName`), while
    /// `Group::group_name` keeps the case the creator chose. `CreateGroup` fills
    /// this map and `ListGroups` pages over it.
    pub groups: BTreeMap<String, Group>,
    /// The users each group holds: [`name_key`] of the group name -> [`name_key`] of
    /// every member user's name. A `BTreeSet` because a user belongs to a group at
    /// most once (adding a member it already holds is the same membership) and
    /// because the total key order is what lets `GetGroup` hand out a resumable
    /// marker over the membership.
    ///
    /// The membership is held HERE rather than on either entity, so the group and the
    /// user records stay what their own reads report and one place answers which
    /// users a group holds. `AddUserToGroup` is what fills it and
    /// `RemoveUserFromGroup` what empties it.
    pub group_members: BTreeMap<String, BTreeSet<String>>,
    /// Inline user policies: [`name_key`] of the user name -> [`name_key`] of the
    /// policy name -> the policy. Policy names are case-folded for the same reason
    /// user names are: the model's `CreateUser$UserName` documentation says "user,
    /// group, role, and policy names must be unique within the account. Names are
    /// not distinguished by case."
    pub user_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Inline role policies: [`name_key`] of the role name -> [`name_key`] of the
    /// policy name -> the policy. Keyed exactly as
    /// [`user_inline_policies`](Self::user_inline_policies) is, and for the same
    /// reason: "user, group, role, and policy names must be unique within the
    /// account. Names are not distinguished by case."
    pub role_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Inline group policies: [`name_key`] of the group name -> [`name_key`] of the
    /// policy name -> the policy. Keyed exactly as
    /// [`user_inline_policies`](Self::user_inline_policies) is, and for the same
    /// reason: "user, group, role, and policy names must be unique within the
    /// account. Names are not distinguished by case." `PutGroupPolicy` fills this
    /// map, `GetGroupPolicy` reads one entry and `ListGroupPolicies` pages over
    /// the names.
    pub group_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Access key id -> the key's record, including [`name_key`] of the user that
    /// owns it. `GetUser` with no `UserName` resolves the caller through this map,
    /// and `CreateAccessKey` is what populates it.
    pub access_keys: BTreeMap<String, AccessKeyRecord>,
    /// Access key id -> the role session that credential speaks for, for the temporary
    /// credentials the account's roles are being used under. See [`RoleSessionRecord`]:
    /// the authorizer resolves a signing principal through this map when the key is not
    /// a long-term one, and `sts:AssumeRole` (and every other operation that vends an
    /// assumed-role credential) is what fills it through
    /// [`Self::record_role_session`]. At most [`MAX_ROLE_SESSIONS`] entries, which the
    /// same write keeps by pruning the records the clock has already expired.
    pub role_sessions: BTreeMap<String, RoleSessionRecord>,
    /// `CertificateId` -> the signing certificate's record, including [`name_key`] of
    /// the user that holds it. `UploadSigningCertificate` is what fills this map,
    /// `ListSigningCertificates` pages over it, `UpdateSigningCertificate` moves one
    /// record's status and `DeleteSigningCertificate` empties it. Keyed by the id
    /// (rather than held on the user) so enumeration order is total and stable, which
    /// is what lets the list hand out a resumable marker.
    pub signing_certificates: BTreeMap<String, SigningCertificateRecord>,
    /// `SSHPublicKeyId` -> the SSH public key's record, including [`name_key`] of the user
    /// that holds it. `UploadSSHPublicKey` is what fills this map and `ListSSHPublicKeys`
    /// pages over it. Keyed by the id (rather than held on the user) so enumeration order
    /// is total and stable, which is what lets the list hand out a resumable marker — the
    /// same shape [`Self::signing_certificates`] takes, for the same reason.
    pub ssh_public_keys: BTreeMap<String, SshPublicKeyRecord>,
    /// The persisted login profiles, keyed by [`name_key`] of the user name — the
    /// same key [`Self::users`] uses, because a user has AT MOST ONE login profile
    /// ("a login profile is created when you create a password for the user",
    /// `GetLoginProfile`) and that profile is reached only through its user.
    /// `CreateLoginProfile` is what fills this map and `GetLoginProfile` reads it.
    pub login_profiles: BTreeMap<String, LoginProfileRecord>,
    /// The persisted service-specific credentials, keyed by their
    /// `ServiceSpecificCredentialId` — the handle every other operation on one takes
    /// (`ListServiceSpecificCredentials` reports it,
    /// `DeleteServiceSpecificCredential`, `ResetServiceSpecificCredential` and
    /// `UpdateServiceSpecificCredential` name it). Keying by the id keeps
    /// enumeration order total and stable, which is what lets the list operation
    /// hand out a resumable marker.
    ///
    /// The stored value is the `ServiceSpecificCredentialMetadata` shape, so one
    /// record serves both the create response and the list; the generated PASSWORD
    /// is deliberately not among its members, because the create response "is the
    /// only time that the password for this credential set is available. It cannot
    /// be recovered later"
    /// (`CreateServiceSpecificCredential$ServiceSpecificCredential`).
    pub service_specific_credentials: BTreeMap<String, ServiceSpecificCredentialMetadata>,
    /// The persisted customer managed policies, keyed by [`name_key`] of the
    /// policy name, for the same reason users and roles are: the model's
    /// `CreatePolicy$PolicyName` documentation says "IAM user, group, role, and
    /// policy names must be unique within the account. Names are not
    /// distinguished by case."
    ///
    /// This is also the ONE record of which customer-managed policies the account
    /// holds, which is what makes such a policy attachable: an ARN naming a policy
    /// this map does not carry refers to no entity (see [`Self::policy_by_arn`]).
    /// AWS-managed policies (`arn:aws:iam::aws:policy/...`) are NOT in here — they
    /// are owned by AWS, not by the account ([`AWS_MANAGED_POLICY_ACCOUNT`]), and
    /// are served from [`AWS_MANAGED_POLICIES`]; a read addressed by ARN resolves
    /// against both ([`Self::resolve_policy`]).
    pub policies: BTreeMap<String, ManagedPolicy>,
    /// The persisted IAM instance profiles, keyed by [`name_key`] of the instance
    /// profile name, for the same reason users and roles are: IAM entity names are
    /// unique within the account and "are not distinguished by case", while
    /// `InstanceProfile::instance_profile_name` keeps the case the creator chose.
    ///
    /// The stored value is an [`InstanceProfileRecord`]: the modelled profile beside the
    /// names of the roles bound to it — "The role associated with the instance profile"
    /// (`InstanceProfile$Roles`) — so `AddRoleToInstanceProfile` has one place to bind a
    /// role and `GetInstanceProfile` one place to read the binding from, while the role
    /// itself stays in [`Self::roles`].
    pub instance_profiles: BTreeMap<String, InstanceProfileRecord>,
    /// The managed policies attached to each role: [`name_key`] of the role name
    /// -> the attached policies' ARNs. A `BTreeSet` because a policy is attached
    /// to a role at most once (a repeat attach is the same attachment) and
    /// because the total ARN order is what lets
    /// `ListAttachedRolePolicies` hand out a resumable marker.
    pub role_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// The managed policies attached to each user: [`name_key`] of the user name
    /// -> the attached policies' ARNs. Keyed and ordered exactly as
    /// [`Self::role_attached_policies`] is, and for the same reasons: a policy is
    /// attached to a user at most once, and the total ARN order is what lets a
    /// reader of the set page over it.
    pub user_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// The managed policies attached to each group: [`name_key`] of the group name
    /// -> the attached policies' ARNs. Keyed and ordered exactly as
    /// [`Self::role_attached_policies`] is, and for the same reasons: a policy is
    /// attached to a group at most once, and the total ARN order is what lets a
    /// reader of the set page over it. `AttachGroupPolicy` is what fills it, and
    /// `ListAttachedGroupPolicies` is what pages over it.
    pub group_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// The persisted IAM OpenID Connect (OIDC) identity providers, keyed by their
    /// ARN — the handle every operation on one takes, and the value
    /// [`open_id_connect_provider_arn`] derives from the provider's URL. Keying by
    /// the ARN keeps enumeration order total and stable, and makes the URL the
    /// provider's identity, which is what refuses a second create for one the
    /// account already holds.
    pub open_id_connect_providers: BTreeMap<String, OpenIdConnectProviderRecord>,
    /// The persisted IAM SAML 2.0 identity providers, keyed by their ARN — the handle
    /// every operation on one takes, and the value [`saml_provider_arn`] derives from
    /// the provider's name. Keying by the ARN keeps enumeration order total and stable,
    /// and makes the name the provider's identity, which is what refuses a second
    /// create for one the account already holds.
    pub saml_providers: BTreeMap<String, SamlProviderRecord>,
    /// The persisted server certificates, keyed by [`name_key`] of the server certificate
    /// name — the handle every operation on one takes
    /// (`GetServerCertificate$ServerCertificateName`) — and case-folded for the same
    /// reason user and role names are: IAM entity names are unique within the account and
    /// "are not distinguished by case", while
    /// `ServerCertificateMetadata::server_certificate_name` keeps the case the uploader
    /// chose. `UploadServerCertificate` is what fills this map and `GetServerCertificate`
    /// reads it.
    pub server_certificates: BTreeMap<String, ServerCertificateRecord>,
    /// The FIDO security keys the account holds as MFA devices, keyed by the serial
    /// number each is addressed by — the handle `GetMFADevice` takes
    /// (`GetMFADeviceRequest$SerialNumber`), which for this kind of device is an ARN.
    ///
    /// EMPTY until something registers one, and no operation of THIS service does: a FIDO
    /// security key is enrolled by the key itself, through the browser's WebAuthn
    /// ceremony, and IAM's API binds no operation that registers one — the same position
    /// [`Self::organization`] is in. A serial this map does not carry therefore names no
    /// FIDO security key of the account, which is what `GetMFADevice` refuses a request
    /// for.
    pub fido_security_keys: BTreeMap<String, FidoSecurityKey>,
    /// The virtual MFA devices the account holds, keyed by the serial number each is
    /// addressed by — "the serial number associated with VirtualMFADevice" is its ARN
    /// (`VirtualMFADevice$SerialNumber`), the handle `EnableMFADevice`,
    /// `DeleteVirtualMFADevice` and the tag operations take.
    ///
    /// `CreateVirtualMFADevice` is what fills this map — "creates a new virtual MFA device
    /// for the Amazon Web Services account" — and `ListVirtualMFADevices` is what pages
    /// over it. Keyed by the serial (rather than by the device name) because that is what
    /// every later operation names the device by, and because the total key order is what
    /// lets the list hand out a resumable marker; the device's NAME, which the create
    /// requires to be unique, is the last segment of that serial.
    pub virtual_mfa_devices: BTreeMap<String, VirtualMfaDeviceRecord>,
    /// The account's alias, when one has been created. An `Option` rather than a
    /// collection because an account holds AT MOST ONE: "Lists the account alias
    /// associated with the Amazon Web Services account (Note: you can have only
    /// one)" (`ListAccountAliases`). `CreateAccountAlias` is what fills it —
    /// "your AWS account can have only one alias. If you create a new alias for
    /// your AWS account, the new alias replaces the previous alias" (the AWS
    /// Sign-In User Guide page "Creating, deleting, and listing an AWS account
    /// alias", which `CreateAccountAlias`'s own documentation links to) — and
    /// `DeleteAccountAlias` is what clears it.
    ///
    /// Stored verbatim: `accountAliasType`'s pattern
    /// (`^[a-z0-9]([a-z0-9]|-(?!-)){1,61}[a-z0-9]$`) admits only lowercase, so
    /// there is no case to fold and no [`name_key`] to build.
    pub account_alias: Option<String>,
    /// The account-level properties the account has persisted, keyed by the
    /// property key itself — "keys are in the format `Namespace/PropertyName`"
    /// (`GetAccountPropertiesResponse$Properties`). `PutAccountProperties` is what
    /// fills this map and `GetAccountProperties` is what reports it; it starts at the
    /// values an untouched account holds ([`default_account_properties`]).
    ///
    /// A `BTreeMap` so the store's own enumeration order is total and stable; the
    /// modelled response member is a map, so what a read reports is the SET of
    /// pairs the account holds rather than an order. Keys are stored VERBATIM: `accountPropertyKeyType`'s pattern
    /// (`^[A-Za-z][A-Za-z0-9/_-]*$`) admits both cases and the namespace part of a
    /// key is a name the caller chose, so there is no case to fold and no
    /// [`name_key`] to build.
    pub account_properties: BTreeMap<String, String>,
    /// The account's password policy, or `None` while the account has none.
    ///
    /// `UpdateAccountPasswordPolicy` is what fills this field — "updates the password
    /// policy settings for the Amazon Web Services account" — and every operation that
    /// SETS a user's console password reads it, because a password the policy does not
    /// admit is refused rather than held
    /// ([`Self::password_policy_violation`]).
    ///
    /// An `Option` rather than a record with defaults, and `None` rather than a policy
    /// holding every documented default: an account starts with NO custom password policy
    /// at all, which is a different fact from holding one that happens to be lenient — it
    /// is what the read of the policy reports as a missing entity.
    pub account_password_policy: Option<AccountPasswordPolicy>,
    /// The persisted delegation requests, keyed by their `DelegationRequestId` — the
    /// handle every operation on one takes (`GetDelegationRequest$DelegationRequestId`,
    /// "the unique identifier of the delegation request"), and the last segment of the
    /// ARN that names one ([`delegation_request_arn`]). `CreateDelegationRequest`
    /// ("creates an IAM delegation request") is what fills this map.
    ///
    /// The identifier is held VERBATIM: `delegationRequestIdType`'s pattern is
    /// `^[\w-]+$` and the identifier is minted rather than named by a caller, so there
    /// is no case to fold and no [`name_key`] to build.
    pub delegation_requests: BTreeMap<String, DelegationRequest>,
    /// Which delegation request each `RequestorWorkflowId` was filed under:
    /// `RequestorWorkflowId` -> the `DelegationRequestId` of the request that carried
    /// it.
    ///
    /// `CreateDelegationRequest$RequestorWorkflowId` is "the unique identifier on the
    /// partner side that can be used to track the progress of the request. IAM maintains
    /// a uniqueness check on this workflow id for each request - if a workflow id for an
    /// existing request is passed, this API call will fail", so the account has to
    /// remember which workflow ids it has already filed a request under. It is an INDEX
    /// beside [`Self::delegation_requests`] and not a second copy of the resource:
    /// `DelegationRequest` declares no member carrying the workflow id
    /// (`smithy_query(query="shape", shape="DelegationRequest")`), so there is nothing in
    /// the stored record to scan for it.
    ///
    /// Held VERBATIM: `requestorWorkflowIdType`'s pattern admits every printable
    /// character, the value is the partner's own handle rather than an IAM entity name,
    /// and AWS distinguishes such a handle by case, so there is no [`name_key`] to build.
    pub delegation_request_workflow_ids: BTreeMap<String, String>,
    /// The role templates the account can create roles from, by the `TemplateArn` that
    /// names one, each holding its minor versions by `MinorVersion`.
    ///
    /// A role template is published by a SERVICE — `managedByTypeType` declares the single
    /// value `Service` (`smithy_query(query="shape", shape="managedByTypeType")`) — and
    /// this API surface has no operation that creates one: `AcquireRole` creates a role
    /// FROM a template and `GetRoleTemplateVersion` reports one, so this map holds a
    /// template only where something recorded it. The templates AWS services publish are
    /// NOT in it — they are the same in every account, so they live in
    /// [`PUBLISHED_ROLE_TEMPLATES`] and are resolved after this map
    /// ([`Self::role_template_version`]), which is what lets an account record a template
    /// of its own under an ARN without the catalog's entry superseding it.
    ///
    /// The ARN is held VERBATIM: it is the handle the publishing service minted rather than
    /// an IAM entity name, so there is no case to fold and no [`name_key`] to build. The
    /// inner map is keyed by the minor version, so the version a request names is resolved
    /// directly and the versions enumerate in numeric order.
    pub role_templates: BTreeMap<String, BTreeMap<i32, RoleTemplateVersion>>,
    /// The account's credential-report generation, or `None` while none has ever been
    /// asked for.
    ///
    /// `GenerateCredentialReport` is what fills this field and what reports the state of
    /// the generation it holds. An `Option` rather than a collection because an account has
    /// ONE credential report: "generates a credential report for the Amazon Web Services
    /// account", so a second request is a request to generate THAT report again rather than
    /// another report beside it.
    pub credential_report: Option<CredentialReportGeneration>,
    /// The account's outbound web identity federation, or `None` while the account has
    /// never enabled it.
    ///
    /// `EnableOutboundWebIdentityFederation` fills this field and
    /// `DisableOutboundWebIdentityFederation` clears its flag;
    /// `GetOutboundWebIdentityFederationInfo` reports it. An `Option` rather than a
    /// collection because the feature is an account-level flag: "enables the outbound
    /// identity federation feature for your Amazon Web Services account", so there is one
    /// such record per account. What each of the three calls does with it is
    /// [`super::outbound_web_identity_federation`].
    pub outbound_web_identity_federation: Option<OutboundWebIdentityFederation>,
    /// The organization this account belongs to, or `None` while it belongs to none.
    ///
    /// `None` is the standalone account this emulator presents until something records
    /// otherwise: an account is put in an organization by AWS Organizations, not by IAM,
    /// so no operation of THIS service fills this field — the organization-wide operations
    /// IAM serves only consult it ([`Self::manages_organization`]) and refuse a caller it
    /// does not make the organization's management account. An `Option` rather than a
    /// collection because an account belongs to at most one organization.
    pub organization: Option<Organization>,
    /// The Organizations service-last-accessed report jobs the account has asked for,
    /// keyed by the `JobId` [`GenerateOrganizationsAccessReport`] answered with — "the job
    /// identifier … use this parameter in the GetOrganizationsAccessReport operation" —
    /// which is the handle the read takes.
    ///
    /// `GenerateOrganizationsAccessReport` is what fills this map and
    /// `GetOrganizationsAccessReport` what reports one entry.
    pub organizations_access_reports: BTreeMap<String, OrganizationsAccessReportJob>,
    /// The service-last-accessed report jobs the account has asked for, keyed by the
    /// `JobId` [`GenerateServiceLastAccessedDetails`] answered with — "the JobId that
    /// you can use in the GetServiceLastAccessedDetails or
    /// GetServiceLastAccessedDetailsWithEntities operations" — which is the handle both
    /// reads take.
    ///
    /// `GenerateServiceLastAccessedDetails` is what fills this map;
    /// `GetServiceLastAccessedDetails` and `GetServiceLastAccessedDetailsWithEntities`
    /// are what report one entry. Held apart from
    /// [`Self::organizations_access_reports`] because the two are different reports of
    /// different resources, read back by different operations: a job id minted for one
    /// names no job of the other.
    pub service_last_accessed_reports: BTreeMap<String, ServiceLastAccessedJob>,
    /// The service-linked role deletions the account has submitted, keyed by the
    /// `DeletionTaskId` `DeleteServiceLinkedRole` answered with — the handle
    /// `GetServiceLinkedRoleDeletionStatus` takes.
    ///
    /// `DeleteServiceLinkedRole` is what fills this map and
    /// `GetServiceLinkedRoleDeletionStatus` is what reports one entry. A finished task
    /// stays readable, up to [`MAX_RETAINED_SERVICE_LINKED_ROLE_DELETION_TASKS`] of them.
    pub service_linked_role_deletions: BTreeMap<String, ServiceLinkedRoleDeletionTask>,
    /// The service-linked role names the account has asked IAM to DELETE, by
    /// [`name_key`] of the role name.
    ///
    /// A `DeleteServiceLinkedRole` submit is the account saying that service-linked role
    /// is to be gone, so from then on nothing reports a role under that name: the
    /// account's own record is removed, and the record the OWNING SERVICE keeps
    /// provisioned under a published name ([`service_provisioned_role`]) is withdrawn
    /// here — otherwise the derived record would step straight back in and a client
    /// waiting for the role to disappear (the documented pattern, since the name is
    /// fixed per service principal) would wait forever.
    ///
    /// A name is taken out of this set by a role being CREATED under it again
    /// ([`insert_service_linked_role`]), which is what makes the set a record of what the
    /// account has given up rather than a permanent ban. Only a name some service
    /// publishes is held, so the set is bounded by
    /// [`PUBLISHED_SERVICE_LINKED_ROLE_NAMES`]
    /// ([`Self::withdraw_provisioned_role`]).
    pub withdrawn_service_provisioned_roles: BTreeSet<String>,
    /// Monotonic counter feeding every report job's `JobId`
    /// ([`next_access_report_job_id`](Self::next_access_report_job_id) and
    /// [`next_service_last_accessed_job_id`](Self::next_service_last_accessed_job_id)).
    ///
    /// ONE counter for both kinds of job, so no two jobs the account ever records can
    /// share an id whichever generate minted them.
    pub report_job_seq: u64,
    /// Monotonic counter feeding the task-uuid of every service-linked role deletion's
    /// `DeletionTaskId`
    /// ([`next_service_linked_role_deletion_task_id`](Self::next_service_linked_role_deletion_task_id)).
    pub service_linked_role_deletion_seq: u64,
    /// Monotonic counter feeding
    /// [`next_delegation_request_id`](Self::next_delegation_request_id).
    pub delegation_request_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_saml_provider_uuid`](Self::next_saml_provider_uuid).
    pub saml_provider_uuid_seq: u64,
    /// Monotonic counter feeding
    /// [`next_saml_private_key_id`](Self::next_saml_private_key_id).
    pub saml_private_key_id_seq: u64,
    /// Monotonic counter feeding [`next_user_id`](Self::next_user_id).
    pub user_id_seq: u64,
    /// Monotonic counter feeding [`next_role_id`](Self::next_role_id).
    pub role_id_seq: u64,
    /// Monotonic counter feeding [`next_group_id`](Self::next_group_id).
    pub group_id_seq: u64,
    /// Monotonic counter feeding [`next_policy_id`](Self::next_policy_id).
    pub policy_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_instance_profile_id`](Self::next_instance_profile_id).
    pub instance_profile_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_server_certificate_id`](Self::next_server_certificate_id).
    pub server_certificate_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_signing_certificate_id`](Self::next_signing_certificate_id).
    pub signing_certificate_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_ssh_public_key_id`](Self::next_ssh_public_key_id).
    pub ssh_public_key_id_seq: u64,
    /// Monotonic counter feeding [`hold_password`](Self::hold_password), so every
    /// password the account holds a verifier for carries its own salt.
    pub password_salt_seq: u64,
    /// The version of the global endpoint token the account is set to, as
    /// `GetAccountSummary` reports it, or `None` while nothing has set a preference.
    ///
    /// `SetSecurityTokenServicePreferences` is what fills this field — it "sets the
    /// specified version of the global endpoint token as the token version used for the
    /// Amazon Web Services account" — and "to view the current session token version, see
    /// the GlobalEndpointTokenVersion entry in the response of the GetAccountSummary
    /// operation", which is the one reader of it
    /// ([`Self::account_summary`]).
    ///
    /// `None` rather than the default value itself, so an account that has never set a
    /// preference is a different record from one set to the version it already had; both
    /// report [`super::account_summary::DEFAULT_GLOBAL_ENDPOINT_TOKEN_VERSION`].
    pub global_endpoint_token_version: Option<i32>,
}

impl Default for ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The account's IAM as it stands before any request: holding the one user this
    /// emulator's own long-term credential belongs to, and nothing else.
    ///
    /// A long-term access key belongs to an IAM USER, so the principal STS reports for a
    /// caller presenting the key the emulator publishes
    /// ([`::service_common::caller_principal`]) is a user of this account — present from
    /// the moment the account exists, exactly as the user behind an AWS access key is. It
    /// is therefore a stored record and not a derived one: a caller that reads its own
    /// identity and then manages that identity (embeds an inline policy, attaches a
    /// managed one, tags it, reads it back) is managing the ACCOUNT's own user, which the
    /// account's own collections have to carry for those writes to land and for a later
    /// read to report them.
    ///
    /// Its access key is deliberately NOT seeded beside it: the key pair the emulator
    /// publishes is held by the credential store, not by `iam:CreateAccessKey`, and
    /// [`super::authorization`] resolves a caller's identity-based policies through
    /// [`Self::access_keys`] — so seeding one there would start evaluating that caller
    /// against an empty policy set instead of leaving it in the position this emulator's
    /// published credential is in.
    ///
    /// The account-level properties start at the values an account holds before anything
    /// has been written to them ([`default_account_properties`]), because that is the
    /// configuration an account IS in: each account-wide feature these properties gate is
    /// in its default state from the moment the account exists, so a read of one before a
    /// write reports that state rather than nothing.
    fn default() -> Self {
        let mut account = Self::empty();
        let caller = ambient_user();
        account
            .users
            .insert(name_key(&caller.user_name), caller);
        account.account_properties = default_account_properties();
        account
    }
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// An account holding NOTHING: every collection empty and every id counter at its
    /// start.
    ///
    /// The base [`Self::default`] builds the account's starting records on top of. Written
    /// out field by field rather than derived, so a field added later has to say here what
    /// an account with nothing in it holds for it.
    fn empty() -> Self {
        Self {
            users: BTreeMap::new(),
            roles: BTreeMap::new(),
            groups: BTreeMap::new(),
            group_members: BTreeMap::new(),
            user_inline_policies: BTreeMap::new(),
            role_inline_policies: BTreeMap::new(),
            group_inline_policies: BTreeMap::new(),
            access_keys: BTreeMap::new(),
            role_sessions: BTreeMap::new(),
            signing_certificates: BTreeMap::new(),
            ssh_public_keys: BTreeMap::new(),
            login_profiles: BTreeMap::new(),
            service_specific_credentials: BTreeMap::new(),
            policies: BTreeMap::new(),
            instance_profiles: BTreeMap::new(),
            role_attached_policies: BTreeMap::new(),
            user_attached_policies: BTreeMap::new(),
            group_attached_policies: BTreeMap::new(),
            open_id_connect_providers: BTreeMap::new(),
            saml_providers: BTreeMap::new(),
            server_certificates: BTreeMap::new(),
            fido_security_keys: BTreeMap::new(),
            virtual_mfa_devices: BTreeMap::new(),
            account_alias: None,
            account_properties: BTreeMap::new(),
            account_password_policy: None,
            delegation_requests: BTreeMap::new(),
            delegation_request_workflow_ids: BTreeMap::new(),
            role_templates: BTreeMap::new(),
            credential_report: None,
            outbound_web_identity_federation: None,
            organization: None,
            organizations_access_reports: BTreeMap::new(),
            service_last_accessed_reports: BTreeMap::new(),
            service_linked_role_deletions: BTreeMap::new(),
            withdrawn_service_provisioned_roles: BTreeSet::new(),
            report_job_seq: 0,
            service_linked_role_deletion_seq: 0,
            delegation_request_id_seq: 0,
            saml_provider_uuid_seq: 0,
            saml_private_key_id_seq: 0,
            user_id_seq: 0,
            role_id_seq: 0,
            group_id_seq: 0,
            policy_id_seq: 0,
            instance_profile_id_seq: 0,
            server_certificate_id_seq: 0,
            signing_certificate_id_seq: 0,
            ssh_public_key_id_seq: 0,
            password_salt_seq: 0,
            global_endpoint_token_version: None,
        }
    }
}

/// The IAM user this emulator's own long-term credential belongs to, as the account holds
/// it.
///
/// Every member is a pure function of the account contract, stated once by
/// [`::service_common::caller_principal`] so this record and the one STS reports as the
/// calling principal cannot differ: the name, the account-root path, the ARN of that path
/// and name, and the `UserId` derived from that ARN. The `UserId` is NOT drawn from
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::user_id_seq`] for that reason —
/// the counter is what mints the id of a user a CALLER creates, and this user predates
/// every request — and it cannot collide with one of those: the two are 17 characters
/// drawn independently from the same 32-character alphabet.
///
/// Nothing in this account created the user — the key pair it holds was published before
/// any request was served — so its `CreateDate` is the fixed instant a record the account
/// starts out with reports ([`ACCOUNT_BOOTSTRAP_CREATE_MILLIS`]) rather than the clock at
/// first touch, which would make the record differ between two emulators serving the same
/// account. It carries no password (nothing here serves a console sign-in), no permissions
/// boundary and no tags, because nothing has attached any.
fn ambient_user() -> User {
    User {
        path: ::service_common::caller_principal::AMBIENT_USER_PATH.to_string(),
        user_name: ::service_common::caller_principal::AMBIENT_USER_NAME.to_string(),
        user_id: ::service_common::caller_principal::ambient_user_id(),
        arn: ::service_common::caller_principal::ambient_user_arn(),
        create_date: ::aws_smithy_types::DateTime::from_millis(ACCOUNT_BOOTSTRAP_CREATE_MILLIS),
        password_last_used: None,
        permissions_boundary: None,
        tags: None,
    }
}

/// The `CreateDate` a record the account STARTS OUT with reports: the epoch.
///
/// Nothing in this account created such a record, so there is no creation instant to
/// report, and reading the clock would answer a different `CreateDate` on every fresh
/// process. A FIXED instant keeps it stable, and the epoch is the value this repository
/// already gives a record nothing created — the same choice
/// [`SERVICE_PROVISIONED_ROLE_CREATE_MILLIS`] states for a service-linked role.
///
/// Shared with [`super::credential_report`], which reports the account's root user: that
/// user is the account itself, which nothing here created either, so its reported creation
/// instant is this same one rather than a second fixed instant beside it.
pub(crate) const ACCOUNT_BOOTSTRAP_CREATE_MILLIS: i64 = 0;

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// Mint the next user's `UserId`.
    ///
    /// Unique by construction: the counter is bijectively mixed and the whole
    /// 64-bit result is spelled out in the suffix, so two distinct counter values
    /// can never produce one id. Stable by construction too — the id is minted
    /// once, stored with the user, and every later read returns the stored value.
    pub fn next_user_id(&mut self) -> String {
        self.user_id_seq += 1;
        unique_id(USER_ID_PREFIX, self.user_id_seq)
    }

    /// Mint the next role's `RoleId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the role.
    pub fn next_role_id(&mut self) -> String {
        self.role_id_seq += 1;
        unique_id(ROLE_ID_PREFIX, self.role_id_seq)
    }

    /// Mint the next group's `GroupId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the group.
    pub fn next_group_id(&mut self) -> String {
        self.group_id_seq += 1;
        unique_id(GROUP_ID_PREFIX, self.group_id_seq)
    }

    /// Mint the next Organizations access-report job's `JobId`.
    ///
    /// Spelled by [`uuid_from_counter`] — unique for the same reason
    /// [`next_user_id`](Self::next_user_id) is, and minted once, then stored with the job.
    pub fn next_access_report_job_id(&mut self) -> String {
        self.report_job_seq += 1;
        uuid_from_counter(self.report_job_seq)
    }

    /// Mint the next service-last-accessed report job's `JobId`.
    ///
    /// Drawn from the same counter and the same spelling as
    /// [`next_access_report_job_id`](Self::next_access_report_job_id), so an id is
    /// unique across BOTH kinds of report job the account records, and minted once,
    /// then stored with the job.
    pub fn next_service_last_accessed_job_id(&mut self) -> String {
        self.report_job_seq += 1;
        uuid_from_counter(self.report_job_seq)
    }

    /// Mint the `DeletionTaskId` of a deletion submitted for the service-linked role
    /// `role_name`, which is linked to `service_principal`.
    ///
    /// `DeleteServiceLinkedRole$DeletionTaskId` documents the identifier "is returned in
    /// the format `task/aws-service-role/<service-principal-name>/<role-name>/<task-uuid>`",
    /// so the id names the role the task was submitted for. The task-uuid is spelled by
    /// [`uuid_from_counter`] off this account's own counter, so it is unique for the same
    /// reason [`next_access_report_job_id`](Self::next_access_report_job_id) is and two
    /// deletions submitted for one role name never share an id. Minted once, then stored
    /// with the task.
    pub fn next_service_linked_role_deletion_task_id(
        &mut self,
        service_principal: &str,
        role_name: &str,
    ) -> String {
        self.service_linked_role_deletion_seq += 1;
        format!(
            "{DELETION_TASK_ID_PREFIX}{service_principal}/{role_name}/{}",
            uuid_from_counter(self.service_linked_role_deletion_seq)
        )
    }

    /// The `DeletionTaskId` of the deletion already in flight for the role keyed by
    /// `role_key` at `now`, or `None` when the account has no incomplete deletion for it.
    ///
    /// `DeleteServiceLinkedRole`: "if you call this operation more than once for the same
    /// service-linked role and an earlier deletion task is not complete, then the
    /// DeletionTaskId of the earlier request is returned" — this is the lookup that
    /// answers which task that is, so a repeated submit records no second task.
    ///
    /// One role has at most one deletion in flight, so there is normally one task to find.
    /// Where a NAME has carried more than one role — a role created again after an earlier
    /// one's deletion was submitted — the most recently submitted of the incomplete tasks
    /// is the one the name's current deletion is.
    pub fn in_flight_service_linked_role_deletion(
        &self,
        role_key: &str,
        now: ::aws_smithy_types::DateTime,
    ) -> Option<&str> {
        self.service_linked_role_deletions
            .iter()
            .filter(|(_, task)| {
                task.role_key == role_key
                    && service_linked_role_deletion_status(task, now)
                        == DELETION_TASK_STATUS_IN_PROGRESS
            })
            .max_by(|(left_id, left), (right_id, right)| {
                (left.created.secs(), left_id.as_str())
                    .cmp(&(right.created.secs(), right_id.as_str()))
            })
            .map(|(deletion_task_id, _)| deletion_task_id.as_str())
    }

    /// Mint the next delegation request's `DelegationRequestId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is: the
    /// counter is bijectively mixed into the id, and the id is minted once and then
    /// stored with the request, so "the unique identifier for the created delegation
    /// request" (`CreateDelegationRequestResponse$DelegationRequestId`) is the handle
    /// every later read of that request takes. It carries no entity-type prefix — AWS
    /// publishes none for a delegation request — so the whole id is
    /// [`DELEGATION_REQUEST_ID_CHARACTERS`] base32 characters.
    pub fn next_delegation_request_id(&mut self) -> String {
        self.delegation_request_id_seq += 1;
        unique_id_of_length(
            "",
            self.delegation_request_id_seq,
            DELEGATION_REQUEST_ID_CHARACTERS,
        )
    }

    /// The minor version of the role template `template_arn` that a request naming
    /// `minor_version` addresses, or `None` when the account holds no such template or no
    /// such version of it.
    ///
    /// "The minor version of the role template to use. If you do not specify a minor
    /// version, the service uses the template's default minor version"
    /// (`AcquireRole$TemplateMinorVersion`), which each held version records as
    /// `RoleTemplateVersion$DefaultMinorVersion` — "the minor version that the service uses
    /// by default when you create a role from this template without specifying a minor
    /// version". A template whose versions declare no default has its HIGHEST held minor
    /// version addressed, because that is the one version a template with a single minor
    /// version has and the latest of several.
    ///
    /// The ONE resolution of that rule, so `AcquireRole` and `GetRoleTemplateVersion`
    /// cannot address two different versions of one template.
    ///
    /// A template the ACCOUNT holds takes precedence: an ARN this account recorded a
    /// template under is resolved against the versions it recorded and nothing else, and
    /// every other ARN resolves against the templates AWS services publish
    /// ([`published_role_template_version`]), which every account can create roles from
    /// without having recorded anything.
    pub fn role_template_version(
        &self,
        template_arn: &str,
        minor_version: Option<i32>,
    ) -> Option<RoleTemplateVersion> {
        let Some(versions) = self.role_templates.get(template_arn) else {
            return published_role_template_version(template_arn, minor_version);
        };
        match minor_version {
            Some(minor_version) => versions.get(&minor_version),
            None => versions
                .values()
                .find_map(|version| version.default_minor_version)
                .and_then(|default| versions.get(&default))
                .or_else(|| versions.values().next_back()),
        }
        .cloned()
    }

    /// Mint the next SAML provider's `SAMLProviderUUID`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is:
    /// the counter is bijectively mixed into the suffix, and the id is minted once and
    /// then stored with the provider.
    pub fn next_saml_provider_uuid(&mut self) -> String {
        self.saml_provider_uuid_seq += 1;
        unique_id(SAML_PROVIDER_UUID_PREFIX, self.saml_provider_uuid_seq)
    }

    /// Mint the next uploaded SAML private key's `KeyId`.
    ///
    /// Unique and stable for the same reason
    /// [`next_saml_provider_uuid`](Self::next_saml_provider_uuid) is, and drawn from its
    /// own counter and prefix so a key id is never a provider uuid.
    pub fn next_saml_private_key_id(&mut self) -> String {
        self.saml_private_key_id_seq += 1;
        unique_id(SAML_PRIVATE_KEY_ID_PREFIX, self.saml_private_key_id_seq)
    }

    /// Mint the next server certificate's `ServerCertificateId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is: the
    /// counter is bijectively mixed into the suffix, and the id is minted once and then
    /// stored with the certificate, so "the stable and unique string identifying the
    /// server certificate" (`ServerCertificateMetadata$ServerCertificateId`) is what every
    /// later read reports.
    pub fn next_server_certificate_id(&mut self) -> String {
        self.server_certificate_id_seq += 1;
        unique_id(
            SERVER_CERTIFICATE_ID_PREFIX,
            self.server_certificate_id_seq,
        )
    }

    /// Mint the next signing certificate's `CertificateId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is:
    /// the counter is bijectively mixed into the id, and the id is minted once and then
    /// stored with the certificate, so "the ID for the signing certificate"
    /// (`SigningCertificate$CertificateId`) is what every later read reports. It
    /// carries no entity-type prefix — AWS publishes none for a signing certificate —
    /// so the whole id is [`SIGNING_CERTIFICATE_ID_CHARACTERS`] base32 characters,
    /// inside `certificateIdType`'s len 24..128 and `^[\w]+$`.
    pub fn next_signing_certificate_id(&mut self) -> String {
        self.signing_certificate_id_seq += 1;
        unique_id_of_length(
            "",
            self.signing_certificate_id_seq,
            SIGNING_CERTIFICATE_ID_CHARACTERS,
        )
    }

    /// How the account holds `password`: the [`PasswordVerifier`] for it, under a salt
    /// no other held password shares.
    ///
    /// Every write that sets a console password goes through here — the create, the
    /// administrative update and the user's own change — so there is one definition of
    /// what the account keeps of a password, and the check `ChangePassword` makes is a
    /// check against whatever the last of those writes left.
    pub fn hold_password(&mut self, password: &str) -> PasswordVerifier {
        self.password_salt_seq += 1;
        PasswordVerifier::new(self.password_salt_seq, password)
    }

    /// Which rule of the account's password policy `password` breaks, or `None` when the
    /// account admits it.
    ///
    /// `None` for an account holding NO policy, which is the account this emulator starts
    /// out as: there is no rule to break until [`Self::account_password_policy`] holds one.
    /// Every operation that sets a console password asks THIS question — the create, the
    /// administrative update and the user's own change — so the policy in force is
    /// evaluated in one place and the three cannot disagree about which passwords the
    /// account accepts. A READ: asking changes nothing a later caller sees.
    pub fn password_policy_violation(&self, password: &str) -> Option<String> {
        self.account_password_policy
            .as_ref()
            .and_then(|policy| policy.violation(password))
    }

    /// Whether `account_id` manages the organization `organization_id` — the question an
    /// operation served only to an organization's management account is decided by.
    ///
    /// False for an account that belongs to no organization at all (a standalone account,
    /// which is what [`Self::organization`] holds `None` for), for a member account of an
    /// organization it does not manage, and for the management account of ANOTHER
    /// organization than the one asked about.
    pub fn manages_organization(&self, account_id: &str, organization_id: &str) -> bool {
        self.organization.as_ref().is_some_and(|organization| {
            organization.organization_id == organization_id
                && organization.management_account_id == account_id
        })
    }

    /// The organization `account_id` may read and change centralized root access for, or
    /// the standing requirement of that access the account does not meet.
    ///
    /// The requirements are the ones the centralized root access operations all model, taken
    /// in the order they narrow the caller down: there is an organization at all, it has "All
    /// features" enabled, trusted access is enabled for IAM in it, and the account is the one
    /// that administers that access. Decided here so each of those operations answers the
    /// same question the same way and refuses the same account.
    ///
    /// A READ: it only consults the store, so asking changes nothing a later caller sees.
    pub fn centralized_root_access_organization(
        &self,
        account_id: &str,
    ) -> Result<&Organization, RootAccessPrecondition> {
        let Some(organization) = self.organization.as_ref() else {
            return Err(RootAccessPrecondition::OrganizationNotFound);
        };
        if !organization.all_features_enabled {
            return Err(RootAccessPrecondition::OrganizationNotInAllFeaturesMode);
        }
        if !organization.iam_trusted_access_enabled {
            return Err(RootAccessPrecondition::ServiceAccessNotEnabled);
        }
        if !organization.serves_root_access_to(account_id) {
            return Err(RootAccessPrecondition::AccountNotManagementOrDelegatedAdministrator);
        }
        Ok(organization)
    }

    /// How many signing certificates the user keyed by `user_key` holds.
    ///
    /// This is the measure IAM's signing-certificates-per-user quota
    /// ([`MAX_SIGNING_CERTIFICATES_PER_USER`]) is taken against, and what tells a user
    /// delete that certificates still depend on the user.
    pub fn signing_certificates_held_by(&self, user_key: &str) -> usize {
        self.signing_certificates
            .values()
            .filter(|record| record.user_key == user_key)
            .count()
    }

    /// How many access keys the user keyed by `user_key` holds.
    ///
    /// This is the measure IAM's access-keys-per-user quota
    /// ([`MAX_ACCESS_KEYS_PER_USER`]) is taken against, and what tells a delete
    /// that credentials still depend on the user.
    pub fn access_keys_held_by(&self, user_key: &str) -> usize {
        self.access_keys
            .values()
            .filter(|record| record.user_key == user_key)
            .count()
    }

    /// Whether a signed request presenting `access_key_id` carries a credential this
    /// account will accept.
    ///
    /// `UpdateAccessKey$Status` documents what the stored value means: "Active means
    /// that the key can be used for programmatic calls to Amazon Web Services, while
    /// Inactive means that the key cannot be used" — so a key the account holds
    /// authenticates a request only while its status is
    /// [`ACCESS_KEY_ACTIVE_STATUS`], and deactivating one makes it unusable for API
    /// calls while leaving it listed. Any other `statusType` value the member admits
    /// is likewise not `Active`, so it is not usable either.
    ///
    /// A key id the account holds NO record of is usable: such a credential is not an
    /// IAM user's at all — the key pair the emulator publishes at start, or a session
    /// another service minted — and this account's `AccessKey$Status` says nothing
    /// about it, so there is nothing here to refuse it on.
    ///
    /// A READ: the store is only consulted, so asking changes nothing a later caller
    /// sees.
    pub fn access_key_is_usable(&self, access_key_id: &str) -> bool {
        self.access_keys
            .get(access_key_id)
            .is_none_or(|record| record.status == ACCESS_KEY_ACTIVE_STATUS)
    }

    /// Record the role session the temporary credential `access_key_id` speaks for, so
    /// a request signed with it is judged on that role's policies
    /// ([`Self::role_sessions`]).
    ///
    /// Called by the operation that MINTS such a credential, which is the only thing
    /// that knows the session it belongs to. Recording it again under the same key
    /// replaces the record, because the credential store never hands out an id it is
    /// already holding one for.
    ///
    /// Every record the clock has already expired is pruned first: that credential is
    /// withdrawn at its expiry whether or not anybody called, so the record speaks for
    /// nobody past it, and the pruning is what keeps this map's size tied to the sessions
    /// still in play. At [`MAX_ROLE_SESSIONS`] live records the record given up to make
    /// room is the one CLOSEST TO EXPIRING, and the session being recorded is always
    /// recorded: a session this store holds no record of is one the authorizer does not
    /// bound, so dropping the new record would leave the freshest credential unbounded
    /// for as long as it lives.
    pub fn record_role_session(
        &mut self,
        access_key_id: &str,
        session: RoleSessionRecord,
        now: ::aws_smithy_types::DateTime,
    ) {
        self.role_sessions
            .retain(|_, held| held.expires.secs() > now.secs());
        while !self.role_sessions.contains_key(access_key_id)
            && self.role_sessions.len() >= MAX_ROLE_SESSIONS
        {
            let earliest = self
                .role_sessions
                .iter()
                .min_by_key(|(key, held)| (held.expires.secs(), (*key).clone()))
                .map(|(key, _)| key.clone());
            match earliest {
                Some(key) => {
                    self.role_sessions.remove(&key);
                }
                None => break,
            }
        }
        self.role_sessions
            .insert(access_key_id.to_string(), session);
    }

    /// The role session the temporary credential `access_key_id` speaks for at the
    /// instant `now`, or `None` when this account holds no live record for that key.
    ///
    /// A record whose expiry has passed answers `None`: the credential was withdrawn at
    /// that instant, so it speaks for nobody. A READ — the record is left where it is,
    /// and the next [`Self::record_role_session`] prunes it.
    pub fn role_session(
        &self,
        access_key_id: &str,
        now: ::aws_smithy_types::DateTime,
    ) -> Option<&RoleSessionRecord> {
        self.role_sessions
            .get(access_key_id)
            .filter(|session| session.expires.secs() > now.secs())
    }

    /// The PATH of the role `role_name` names, as every operation that PROTECTS a
    /// service-linked role resolves it: the account's own record first, and failing that
    /// the record the owning service keeps provisioned under a published service-linked
    /// role name ([`service_provisioned_role`]). `None` when neither holds a role under
    /// that name — "a resource entity that does not exist".
    ///
    /// The ONE reading of "which role does this request address, and is it a role only
    /// AWS may change", so the protection cannot drift per handler: `DeleteRole`,
    /// `UpdateAssumeRolePolicy`, `Put`/`DeleteRolePolicy`, `Attach`/`DetachRolePolicy`,
    /// `Add`/`RemoveRoleFromInstanceProfile` and the two boundary writes all read it
    /// here, and pass the answer to [`is_service_linked_role_path`].
    ///
    /// A READ: the store is only consulted.
    pub fn addressed_role_path(&self, role_name: &str) -> Option<String> {
        self.roles
            .get(&name_key(role_name))
            .map(|role| role.path.clone())
            .or_else(|| self.provisioned_role(role_name).map(|role| role.path))
    }

    /// The role the OWNING SERVICE keeps provisioned under the published name
    /// `role_name` ([`service_provisioned_role`]), unless this ACCOUNT has asked for that
    /// service-linked role to be deleted ([`Self::withdrawn_service_provisioned_roles`]),
    /// in which case the account holds no role under that name and nothing reports one.
    ///
    /// The ONE place the derived record is resolved against the account's state, so every
    /// operation that resolves a role beyond the account's own collection — the role
    /// report, the description write and the protections, all of which reach it through
    /// [`Self::addressed_role_path`] — agrees about whether a role exists under that name.
    ///
    /// A READ: the store is only consulted.
    pub fn provisioned_role(&self, role_name: &str) -> Option<Role> {
        if self
            .withdrawn_service_provisioned_roles
            .contains(&name_key(role_name))
        {
            return None;
        }
        service_provisioned_role(role_name)
    }

    /// Record that the account has given up the service-linked role keyed by `role_key`:
    /// the record the owning service keeps provisioned under that name stops resolving
    /// ([`Self::provisioned_role`]).
    ///
    /// Called by `DeleteServiceLinkedRole`, the operation through which the account asks
    /// for such a role to be gone, and undone by a role being created under that name
    /// again ([`insert_service_linked_role`]).
    ///
    /// Only a name some service PUBLISHES is recorded, because only such a name resolves
    /// a provisioned record at all — which is also what bounds the set: it can never hold
    /// more entries than [`PUBLISHED_SERVICE_LINKED_ROLE_NAMES`] has rows, however many
    /// names submits are made for.
    pub fn withdraw_provisioned_role(&mut self, role_key: &str) {
        if published_service_linked_role(role_key).is_none() {
            return;
        }
        self.withdrawn_service_provisioned_roles
            .insert(name_key(role_key));
    }

    /// The groups the user keyed by `user_key` belongs to, by the key
    /// [`Self::groups`] holds each of them under, in that map's stable key order.
    ///
    /// The one place the membership is read from the group-keyed map
    /// ([`Self::group_members`]) the other way round, so every caller that asks what
    /// a USER belongs to agrees: the measure IAM's groups-per-user quota
    /// ([`MAX_GROUPS_PER_USER`]) is taken against, what tells a delete that the user
    /// still belongs to a group, what a report of the user's groups lists, and where
    /// the group policies in force on the user are collected from.
    pub fn group_keys_held_by<'a>(&'a self, user_key: &'a str) -> impl Iterator<Item = &'a String> {
        self.group_members
            .iter()
            .filter(move |(_, members)| members.contains(user_key))
            .map(|(group_key, _)| group_key)
    }

    /// Materialise the status changes the clock has already made due: a
    /// service-specific credential whose `ExpirationDate` has passed is `Expired`,
    /// whether or not anybody called.
    ///
    /// This is the only write a reporting operation on these credentials performs,
    /// and it is owed at the same instant regardless of the call that observes it.
    /// Every other status change is a caller's
    /// (`UpdateServiceSpecificCredential` moves one between `Active` and
    /// `Inactive`).
    pub fn expire_service_specific_credentials(&mut self, now: ::aws_smithy_types::DateTime) {
        for credential in self.service_specific_credentials.values_mut() {
            let due = credential
                .expiration_date
                .is_some_and(|expires_at| expires_at.secs() <= now.secs());
            if due {
                credential.status = CREDENTIAL_STATUS_EXPIRED.to_string();
            }
        }
    }

    /// How many sets of service-specific credentials the user keyed by `user_key`
    /// holds for `service_name` — the count
    /// [`MAX_SERVICE_SPECIFIC_CREDENTIALS_PER_SERVICE`] is taken against.
    ///
    /// Both the user name and the service name are compared case-insensitively, for
    /// the same reasons the stores key on them that way: IAM entity names "are not
    /// distinguished by case", and a service principal is a DNS name.
    pub fn service_specific_credential_count(&self, user_key: &str, service_name: &str) -> usize {
        self.service_specific_credentials
            .values()
            .filter(|credential| {
                name_key(&credential.user_name) == user_key
                    && credential.service_name.eq_ignore_ascii_case(service_name)
            })
            .count()
    }

    /// What the (user, credential id) PAIR an operation on a service-specific credential
    /// was addressed by resolves to.
    ///
    /// The three answers are deliberately distinct, because the refusals are: a credential
    /// is reachable only through the user that holds it, so the pair is what the operation
    /// resolves, and an id the account holds under ANOTHER user is a different failure of
    /// that pair from an id the account holds no credential under at all. The one place
    /// that decision is made, so the status, the code and the wording every operation
    /// reaching a credential by id answers cannot drift apart.
    ///
    /// A READ: the store is only consulted. Names compare case-folded, because IAM entity
    /// names "are not distinguished by case".
    pub fn service_specific_credential_lookup(
        &self,
        user_key: &str,
        credential_id: &str,
    ) -> ServiceSpecificCredentialLookup {
        match self.service_specific_credentials.get(credential_id) {
            None => ServiceSpecificCredentialLookup::Absent,
            Some(credential) if name_key(&credential.user_name) == user_key => {
                ServiceSpecificCredentialLookup::Held
            }
            Some(_) => ServiceSpecificCredentialLookup::HeldByAnotherUser,
        }
    }

    /// Mint the next managed policy's `PolicyId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the policy. An id already carried by an entry of AWS's
    /// catalog ([`AWS_MANAGED_POLICIES`], whose ids are published data this account did
    /// not mint) is passed over, because a `PolicyId` identifies ONE policy.
    pub fn next_policy_id(&mut self) -> String {
        loop {
            self.policy_id_seq += 1;
            let minted = unique_id(POLICY_ID_PREFIX, self.policy_id_seq);
            if !AWS_MANAGED_POLICIES
                .iter()
                .any(|entry| entry.policy_id == minted)
            {
                return minted;
            }
        }
    }

    /// Mint the next instance profile's `InstanceProfileId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is: the
    /// counter is bijectively mixed into the suffix, and the id is minted once and then
    /// stored with the profile.
    pub fn next_instance_profile_id(&mut self) -> String {
        self.instance_profile_id_seq += 1;
        unique_id(INSTANCE_PROFILE_ID_PREFIX, self.instance_profile_id_seq)
    }

    /// The managed policy `arn` names, if the account holds it.
    ///
    /// Every modelled read of a managed policy addresses it by ARN while the map
    /// is keyed by the name (the identity IAM enforces uniqueness on), so the ARN
    /// is parsed ([`parse_policy_arn`]) for the name to look up, and the held
    /// policy answers only when its own ARN is the one asked for — so an ARN that
    /// carries another path, another account or another casing of the name names no
    /// policy.
    pub fn policy_by_arn(&self, arn: &str) -> Option<&ManagedPolicy> {
        let parsed = parse_policy_arn(arn)?;
        let held = self.policies.get(&name_key(&parsed.policy_name))?;
        (held.policy.arn.as_deref() == Some(arn)).then_some(held)
    }

    /// The managed policy `arn` names, if the account holds it, for a caller that
    /// is going to CHANGE it.
    ///
    /// The same lookup and the same identity test as
    /// [`policy_by_arn`](Self::policy_by_arn) — an ARN carrying another path,
    /// another account or another casing of the name names no policy — so a write
    /// addressed by ARN reaches exactly the record a read of that ARN reports, and
    /// the account's policies are never found under two different rules.
    pub fn policy_by_arn_mut(&mut self, arn: &str) -> Option<&mut ManagedPolicy> {
        let parsed = parse_policy_arn(arn)?;
        let held = self.policies.get_mut(&name_key(&parsed.policy_name))?;
        (held.policy.arn.as_deref() == Some(arn)).then_some(held)
    }

    /// How many entities the managed policy `arn` names is attached to —
    /// `Policy$AttachmentCount`, "the number of entities (users, groups, and roles)
    /// that the policy is attached to".
    ///
    /// Counted from the attachments themselves on every read, so the number a
    /// reader reports is the number of attachments that exist at that instant: an
    /// attach or a detach changes it without any reader having to be told, and the
    /// count can never drift from the attachment sets it describes. The same
    /// measure answers for a policy the account created and for one of AWS's
    /// ([`AWS_MANAGED_POLICIES`]) — what is counted is the attachments, not the
    /// policy's owner.
    ///
    /// All three identity kinds the measure names can hold an attachment, so all
    /// three attachment sets are counted — which is why the policy's own record does
    /// not cache the number.
    pub fn policy_attachment_count(&self, arn: &str) -> i32 {
        let attachments = self
            .role_attached_policies
            .values()
            .chain(self.user_attached_policies.values())
            .chain(self.group_attached_policies.values())
            .filter(|attached| attached.contains(arn))
            .count();
        i32::try_from(attachments).unwrap_or(i32::MAX)
    }

    /// For how many entities the managed policy `arn` names is used to set the
    /// permissions boundary — `Policy$PermissionsBoundaryUsageCount`, "the number
    /// of entities (users and roles) for which the policy is used to set the
    /// permissions boundary".
    ///
    /// Derived on every read for the same reason
    /// [`policy_attachment_count`](Self::policy_attachment_count) is: the boundary
    /// a role or user carries is the one record of that use.
    pub fn permissions_boundary_usage_count(&self, arn: &str) -> i32 {
        // One definition of "the boundary ARN an entity carries"
        // ([`permissions_boundary_arn`]), the same one `ListEntitiesForPolicy`
        // compares against, so the count and the listing cannot disagree about
        // which entities a boundary policy relates to.
        let boundary_is = |boundary: Option<&crate::ops::types::AttachedPermissionsBoundary>| {
            permissions_boundary_arn(boundary) == Some(arn)
        };
        let roles = self
            .roles
            .values()
            .filter(|role| boundary_is(role.permissions_boundary.as_ref()))
            .count();
        let users = self
            .users
            .values()
            .filter(|user| boundary_is(user.permissions_boundary.as_ref()))
            .count();
        i32::try_from(roles + users).unwrap_or(i32::MAX)
    }

    /// The managed policy `arn` names — the account's own ([`policy_by_arn`](Self::policy_by_arn))
    /// or AWS's ([`aws_managed_policy`]) — as every reader of a managed policy
    /// reports it, or `None` when neither holds that ARN.
    ///
    /// ONE resolution shared by every read addressed by `PolicyArn`, because the
    /// `arn:aws:iam::aws:policy/` namespace is part of every account's view of IAM:
    /// a policy AWS owns can be attached, so it must also be readable, and a reader
    /// that consulted only the account's store would call a policy AWS serves
    /// absent. The two counts are recomputed here rather than read out of the
    /// stored record, so a reader reports the attachments and boundary uses that
    /// exist now; nothing else about the stored policy is touched, and the
    /// resolution writes nothing.
    pub fn resolve_policy(&self, arn: &str) -> Option<ManagedPolicy> {
        // The counts are derived only for an ARN that resolves: a miss — an ARN of
        // another entity, or one no policy is held under — pays no scan of the
        // attachment sets, the roles or the users.
        if let Some(held) = self.policy_by_arn(arn) {
            let mut resolved = held.clone();
            resolved.policy.attachment_count = Some(self.policy_attachment_count(arn));
            resolved.policy.permissions_boundary_usage_count =
                Some(self.permissions_boundary_usage_count(arn));
            return Some(resolved);
        }
        aws_managed_policy(arn).map(|entry| {
            entry.record(
                self.policy_attachment_count(arn),
                self.permissions_boundary_usage_count(arn),
            )
        })
    }

    /// Whether the managed policy `arn` names may be attached to an identity of this
    /// account, or used to set one's permissions boundary — the decision behind
    /// `PolicyNotAttachableException`, "Amazon Web Services service role policies can
    /// only be attached to the service-linked role for that service".
    ///
    /// ONE decision, consulted by the attach calls and by the permissions-boundary
    /// puts, so a policy one of them accepts is accepted by all of them. For a policy
    /// THIS account created it is the policy's own `IsAttachable`. For a policy AWS
    /// owns it is AWS's reservation: a policy filed under the path reserved for
    /// service-linked roles belongs to that role alone
    /// ([`AwsManagedPolicy::is_reserved_for_service_linked_role`]), whether or not the
    /// catalog carries that entry. The reservation is NOT what a reader reports:
    /// `Policy$IsAttachable` is the flag AWS publishes for the policy, and AWS
    /// publishes every policy of its catalog as attachable.
    ///
    /// A value that is not a managed-policy ARN at all is not this question's business
    /// and is taken as attachable: an operation taking a `PolicyArn` refuses such a
    /// value as an invalid input before it asks.
    pub fn policy_is_attachable(&self, arn: &str) -> bool {
        if let Some(held) = self.policy_by_arn(arn) {
            return held.policy.is_attachable;
        }
        parse_policy_arn(arn).is_none_or(|parsed| !is_service_linked_role_path(&parsed.path))
    }

    /// Whether the managed policy `arn` names EXISTS — the question every operation
    /// addressed by a `PolicyArn` asks before it does anything with that policy.
    ///
    /// Which store answers is decided by the ARN's account segment, the one thing that
    /// says who owns the policy. A customer-managed policy exists exactly when THIS
    /// account holds it under that ARN ([`policy_by_arn`](Self::policy_by_arn)); a
    /// policy in AWS's own namespace is not account state, so AWS's catalog answers for
    /// it ([`is_published_aws_managed_policy_arn`]), where a name the catalog carries is
    /// published under exactly one path and the same name under any other path names
    /// nothing. A value that is not a managed-policy ARN at all names no policy either —
    /// an operation taking a `PolicyArn` refuses such a value as an invalid input before
    /// it asks.
    ///
    /// ONE resolution, shared by the attach calls, the permissions-boundary puts and the
    /// readers that resolve a policy through its attachability, so none of them can
    /// accept an ARN another calls missing — and nothing an identity carries can be an
    /// ARN that names no policy.
    pub fn managed_policy_exists(&self, arn: &str) -> bool {
        let Some(parsed) = parse_policy_arn(arn) else {
            return false;
        };
        if parsed.account == AWS_MANAGED_POLICY_ACCOUNT {
            return is_published_aws_managed_policy_arn(arn);
        }
        self.policy_by_arn(arn).is_some()
    }

    /// The delegation request `arn` names, if the account holds it.
    ///
    /// Every read of a delegation request addressed by ARN resolves it here, and the
    /// held request answers only when its OWN ARN is the one asked for — the same
    /// identity test [`Self::policy_by_arn`] applies to a managed policy — so an ARN
    /// carrying another account's id names no request THIS account holds.
    pub fn delegation_request_by_arn(&self, arn: &str) -> Option<&DelegationRequest> {
        let parsed = parse_delegation_request_arn(arn)?;
        let held = self.delegation_requests.get(&parsed.delegation_request_id)?;
        (delegation_request_arn(&parsed.delegation_request_id) == arn).then_some(held)
    }

    /// The identity-based policies the IAM identity whose ARN is `arn` carries, or
    /// `None` when `arn` names no identity the account holds.
    ///
    /// The documents alone, which is what a caller evaluating or reading them needs;
    /// [`principal_sourced_policies`](Self::principal_sourced_policies) is the same
    /// collection with each document's provenance kept, and is where the collection
    /// rule itself lives.
    pub fn principal_policies(&self, arn: &str) -> Option<PrincipalPolicies> {
        let mut collected = PrincipalPolicies::default();
        for policy in self.principal_sourced_policies(arn)? {
            match (policy.document, policy.source) {
                (Some(document), _) => collected.documents.push(document),
                // Only an attached managed policy can resolve to no document (see
                // [`PrincipalPolicies::uncarried`]).
                (None, PolicySource::Managed { policy_arn, .. }) => {
                    collected.uncarried.push(policy_arn)
                }
                (None, PolicySource::Inline { .. }) => {}
            }
        }
        Some(collected)
    }

    /// The document of the managed policy `policy_arn` names that is IN FORCE: its
    /// DEFAULT version's, "the one that is in effect for the IAM users, groups, and
    /// roles to which the policy is attached" (`SetDefaultPolicyVersion`). `None` when
    /// this emulator holds no document for that ARN — an entry of AWS's catalog it does
    /// not carry — which is the one attachment that resolves to nothing. A record whose
    /// default version is not recorded is read at the version every policy is created
    /// with ([`FIRST_POLICY_VERSION_ID`]), the one it would be in force at.
    ///
    /// ONE resolution, shared by every reader of what an attached policy grants: the
    /// collection of an identity's own policies ([`Self::principal_sourced_policies`])
    /// and the ceiling attached to it ([`Self::permissions_boundary_document`]), so the
    /// two cannot read one policy differently.
    pub fn default_version_document(&self, policy_arn: &str) -> Option<String> {
        let policy = self.resolve_policy(policy_arn)?;
        // The one reading of which version is in force
        // ([`ManagedPolicy::default_version_id`]), so what an attachment grants is the
        // version a reader of the policy flags as its default.
        policy
            .versions
            .get(policy.default_version_id())
            .and_then(|version| version.document.clone())
    }

    /// The permissions boundary document in force on the IAM identity whose ARN is
    /// `arn`, or `None` when it carries none — or carries one whose document this
    /// emulator does not hold, a ceiling it cannot read and so cannot apply.
    ///
    /// A boundary is attached as a MANAGED policy — "the ARN of the managed policy
    /// that is used to set the permissions boundary"
    /// (`AttachedPermissionsBoundary$PermissionsBoundaryArn`) — so the document in
    /// force is that policy's DEFAULT version, read through the resolution every
    /// attached policy's document is read through ([`Self::default_version_document`]).
    /// Only a user and a role can carry one, so a group's ARN names no boundary.
    ///
    /// ONE definition of "the ceiling in force on this identity", consulted by this
    /// service's own authorizer, by its policy simulator and by every other service
    /// that decides a call against this store's principals, so none of them caps a
    /// principal differently from the others. A READ: it only consults the store.
    ///
    /// The identity is resolved by NAME, the key the account holds it under, because an
    /// entity carries exactly one boundary whatever path an ARN spells for it — the
    /// question here is which ceiling that entity carries, not whether the ARN is a
    /// well-formed spelling of the entity.
    pub fn permissions_boundary_document(&self, arn: &str) -> Option<String> {
        self.permissions_boundary_in_force(arn)
            .map(|(_, document)| document)
    }

    /// [`Self::permissions_boundary_document`] with the POLICY it came from named: the ARN
    /// of the managed policy that is the identity's permissions boundary, beside the
    /// document in force on it.
    ///
    /// A refusal explained by an explicit `Deny` in the boundary names the boundary that
    /// carried it, which is the ARN here — the same value the identity's own read reports
    /// under `PermissionsBoundary.PermissionsBoundaryArn` — so the decision and the
    /// refusal that reports it read ONE record.
    pub fn permissions_boundary_in_force(&self, arn: &str) -> Option<(String, String)> {
        let entity = parse_entity_arn(arn)?;
        let key = name_key(&entity.name);
        let attached = match entity.kind {
            EntityKind::User => self
                .users
                .get(&key)
                .and_then(|user| permissions_boundary_arn(user.permissions_boundary.as_ref())),
            EntityKind::Role => self
                .roles
                .get(&key)
                .and_then(|role| permissions_boundary_arn(role.permissions_boundary.as_ref())),
            EntityKind::Group => None,
        }?
        .to_string();
        let document = self.default_version_document(&attached)?;
        Some((attached, document))
    }

    /// The FIDO security key the account holds under the serial number `serial_number`, or
    /// `None` when it holds none — which is every serial until a key is enrolled
    /// ([`Self::fido_security_keys`]).
    ///
    /// A READ: it only consults the store.
    pub fn fido_security_key(&self, serial_number: &str) -> Option<&FidoSecurityKey> {
        self.fido_security_keys.get(serial_number)
    }

    /// The tags the IAM resource whose ARN is `arn` carries AT THIS INSTANT, or `None`
    /// when `arn` names no resource this account holds.
    ///
    /// The distinction the `Option` draws is the one an authorization decision turns on:
    /// a resource the account HOLDS has a tag set that is known in full, so a question
    /// about one of its tags is answerable — including answering that it carries no value
    /// for a key — while an ARN naming nothing this store holds leaves such a question
    /// unanswerable rather than answered "untagged". An entity that holds no tags answers
    /// with the EMPTY set, which is a known tag set and not an unknown one.
    ///
    /// Every resource type this store persists tags for is read here, found the way each
    /// of them is already addressed: a user, group or role through
    /// [`parse_entity_arn`] and a managed policy through
    /// [`policy_by_arn`](Self::policy_by_arn), both of which answer only for a record
    /// whose OWN ARN is the one asked for; the identity providers by the ARN their maps
    /// are keyed on; the instance profiles and server certificates — keyed by name — by
    /// the ARN each record carries. A group is held but carries no tags, because IAM's
    /// tagging operations cover no group resource.
    ///
    /// A READ: it only consults the store.
    pub fn resource_tags(&self, arn: &str) -> Option<&[Tag]> {
        /// A resource whose tag set is known to be empty, which is what a record holding
        /// no tags and an untaggable resource type both answer with.
        const UNTAGGED: &[Tag] = &[];

        if let Some(entity) = parse_entity_arn(arn) {
            let key = name_key(&entity.name);
            return match entity.kind {
                EntityKind::User => self
                    .users
                    .get(&key)
                    .filter(|user| user.arn == arn)
                    .map(|user| held_tags(user.tags.as_ref())),
                EntityKind::Role => self
                    .roles
                    .get(&key)
                    .filter(|role| role.arn == arn)
                    .map(|role| held_tags(role.tags.as_ref())),
                EntityKind::Group => self
                    .groups
                    .get(&key)
                    .filter(|group| group.arn == arn)
                    .map(|_| UNTAGGED),
            };
        }
        if let Some(policy) = self.policy_by_arn(arn) {
            return Some(held_tags(policy.policy.tags.as_ref()));
        }
        if let Some(provider) = self.open_id_connect_providers.get(arn) {
            return Some(held_tags(provider.tags.as_ref()));
        }
        if let Some(provider) = self.saml_providers.get(arn) {
            return Some(held_tags(provider.tags.as_ref()));
        }
        if let Some(profile) = self
            .instance_profiles
            .values()
            .find(|record| record.profile.arn == arn)
        {
            return Some(held_tags(profile.profile.tags.as_ref()));
        }
        if let Some(certificate) = self
            .server_certificates
            .values()
            .find(|record| record.metadata.arn == arn)
        {
            return Some(held_tags(certificate.tags.as_ref()));
        }
        None
    }

    /// The identity-based policies the IAM identity whose ARN is `arn` carries, each
    /// with WHERE it comes from, or `None` when `arn` names no identity the account
    /// holds.
    ///
    /// An identity's own permissions come from the policies attached to it: the
    /// inline policies embedded in it, plus the DEFAULT version of each managed
    /// policy attached to it — "the default version is the operative version …
    /// the one that is in effect for the IAM users, groups, and roles to which the
    /// policy is attached" (`SetDefaultPolicyVersion`). A resource-based policy is
    /// not one of them, so a role's trust policy — which says who may assume the
    /// role rather than what the role may do — is not collected. For a USER, the
    /// policies in force on the groups it belongs to are collected too, each keeping
    /// the group it is attached to as its provenance.
    ///
    /// The provenance is kept because a caller can be asked about the policies as
    /// INDIVIDUAL policies rather than as one set: `SimulatePrincipalPolicy`'s
    /// `PolicyExclusionList` names policies to leave out of a simulation by type, by
    /// ARN or by inline name and the entity they are attached to, which is decidable
    /// only from where each document came from.
    ///
    /// The stored identity answers only when its OWN ARN is the one asked for, the
    /// identity test [`policy_by_arn`](Self::policy_by_arn) applies to a policy: an
    /// ARN carrying another path, another account or another resource type names no
    /// identity of this account's. The collection is read from the STORE alone, the
    /// way every other report of the account's record of an identity's attachments
    /// answers (see [`service_provisioned_role`]), and it writes nothing.
    pub fn principal_sourced_policies(&self, arn: &str) -> Option<Vec<SourcedPolicy>> {
        let source = parse_entity_arn(arn)?;
        let key = name_key(&source.name);
        let mut collected: Vec<SourcedPolicy> = Vec::new();
        let embedded = match source.kind {
            EntityKind::User => &self.user_inline_policies,
            EntityKind::Role => &self.role_inline_policies,
            EntityKind::Group => &self.group_inline_policies,
        };
        let attached: Vec<String> = match source.kind {
            EntityKind::User => {
                let user = self.users.get(&key)?;
                if user.arn != arn {
                    return None;
                }
                self.user_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
            EntityKind::Role => {
                let role = self.roles.get(&key)?;
                if role.arn != arn {
                    return None;
                }
                self.role_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
            EntityKind::Group => {
                let group = self.groups.get(&key)?;
                if group.arn != arn {
                    return None;
                }
                self.group_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
        };

        // The inline policies the identity embeds, ordered by policy name — the
        // order the embedding map holds them in, so two reads collect them alike.
        for (_, policy) in embedded.get(&key).into_iter().flatten() {
            collected.push(SourcedPolicy {
                source: PolicySource::Inline {
                    policy_name: policy.policy_name.clone(),
                    attachment_type: source.kind.resource_type(),
                    attachment_name: source.name.clone(),
                },
                document: Some(policy.policy_document.clone()),
            });
        }
        for policy_arn in attached {
            // The DEFAULT version's document, the operative one (see
            // [`Self::default_version_document`]).
            let document = self.default_version_document(&policy_arn);
            collected.push(SourcedPolicy {
                source: PolicySource::Managed {
                    aws_owned: is_aws_managed_policy_arn(&policy_arn),
                    policy_arn,
                },
                document,
            });
        }

        // A user's permissions include the policies in force on every group it belongs
        // to: "if you specify a user, the simulation also includes all policies that
        // are attached to any groups the user belongs to"
        // (`SimulatePrincipalPolicy$PolicySourceArn`). Each inherited policy keeps the
        // GROUP as its provenance, because the group is the entity it is attached to;
        // collecting it through the group's OWN ARN is what makes one rule answer for
        // both, so a policy a group carries is in force on its members however the
        // group came to carry it.
        if source.kind == EntityKind::User {
            let group_arns: Vec<String> = self
                .group_keys_held_by(&key)
                .filter_map(|group_key| self.groups.get(group_key))
                .map(|group| group.arn.clone())
                .collect();
            for group_arn in group_arns {
                collected.extend(
                    self.principal_sourced_policies(&group_arn)
                        .into_iter()
                        .flatten(),
                );
            }
        }
        Some(collected)
    }

    /// The permissions policies the resource a service-last-accessed report is about
    /// carries, or `None` when `arn` names no such resource of this account.
    ///
    /// "The ARN of the IAM resource (user, group, role, or managed policy) used to
    /// generate information about when the resource was last used in an attempt to access
    /// an Amazon Web Services service" (`GenerateServiceLastAccessedDetails$Arn`): the
    /// report is about what that resource's permissions policies ALLOW, so for an
    /// IDENTITY the collection is the one every reader of an identity's grants takes
    /// ([`Self::principal_sourced_policies`], which for a user also collects the policies
    /// the groups it belongs to carry), and for a managed POLICY it is that policy's own
    /// operative document ([`Self::default_version_document`]).
    ///
    /// Only permissions policies: "service last accessed data does not use other policy
    /// types when determining whether a resource could access a service. These other
    /// policy types include resource-based policies, access control lists, Organizations
    /// policies, IAM permissions boundaries, and STS assume role policies. It only
    /// applies permissions policy logic" (`GenerateServiceLastAccessedDetails`) — which
    /// is exactly the collection those readers make, so a role's trust policy and the
    /// ceiling an entity carries are not in it.
    ///
    /// A READ: it only consults the store.
    pub fn report_resource_policies(&self, arn: &str) -> Option<Vec<SourcedPolicy>> {
        if parse_entity_arn(arn).is_some() {
            return self.principal_sourced_policies(arn);
        }
        // A managed policy answers only through the ARN grammar that names one, and only
        // when this emulator holds that policy at all — the account's own or an entry of
        // AWS's catalog ([`Self::resolve_policy`]).
        parse_policy_arn(arn)?;
        self.resolve_policy(arn)?;
        Some(vec![SourcedPolicy {
            source: PolicySource::Managed {
                aws_owned: is_aws_managed_policy_arn(arn),
                policy_arn: arn.to_string(),
            },
            document: self.default_version_document(arn),
        }])
    }

    /// What the inline policies embedded in the user keyed by `user_key` take up,
    /// counting every policy except the one keyed by `replacing` (a put of an
    /// existing name replaces that document rather than adding to it).
    ///
    /// [`InlinePolicySize::measured`] is what IAM's user policy size quota
    /// ([`MAX_USER_INLINE_POLICY_CHARACTERS`]) is taken against.
    pub fn user_inline_policy_size(&self, user_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.user_inline_policies, user_key, replacing)
    }

    /// The same, for the role keyed by `role_key`, whose measured size is what IAM's
    /// role policy size quota ([`MAX_ROLE_INLINE_POLICY_CHARACTERS`]) is taken
    /// against.
    pub fn role_inline_policy_size(&self, role_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.role_inline_policies, role_key, replacing)
    }

    /// The same, for the group keyed by `group_key`, whose measured size is what
    /// IAM's group policy size quota ([`MAX_GROUP_INLINE_POLICY_CHARACTERS`]) is
    /// taken against.
    pub fn group_inline_policy_size(&self, group_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.group_inline_policies, group_key, replacing)
    }

    /// The instance profile keyed by `profile_key` as a reader must report it: the
    /// profile's own members, with `Roles` resolved from [`Self::roles`].
    ///
    /// `InstanceProfile$Roles` reports the ROLE, not a name, so the report has to carry
    /// the role as the account holds it at the moment of the read: the binding
    /// `AddRoleToInstanceProfile` wrote names the role, and a later change to that role
    /// (a tag, a description, a permissions boundary) is part of what the next read of
    /// the profile reports. A binding whose role the account no longer holds reports no
    /// role rather than a resurrected copy of it.
    pub fn instance_profile_report(&self, profile_key: &str) -> Option<InstanceProfile> {
        let held = self.instance_profiles.get(profile_key)?;
        Some(InstanceProfile {
            roles: held
                .role_keys
                .iter()
                .filter_map(|role_key| self.roles.get(role_key).cloned())
                .collect(),
            ..held.profile.clone()
        })
    }

    /// The names of the instance profiles the role keyed by `role_key` is bound to, in
    /// the profiles' stable key order.
    ///
    /// This is what tells a delete that an instance profile still depends on the role:
    /// "before you delete a role, remove the following attached items: inline policies,
    /// attached managed policies, instance profile" (`DeleteRole`). The names are the
    /// ones the profiles were created with, so a refusal can say which they are.
    pub fn instance_profiles_holding_role(&self, role_key: &str) -> Vec<String> {
        self.instance_profiles
            .values()
            .filter(|record| record.role_keys.iter().any(|held| held == role_key))
            .map(|record| record.profile.instance_profile_name.clone())
            .collect()
    }

    /// The modelled `LoginProfile` of the user keyed by `user_key`, or `None` when
    /// that user has no password — or no longer exists.
    ///
    /// `LoginProfile$UserName` reports the name as the ACCOUNT holds it, not the
    /// case a request happened to spell, because IAM names "are not distinguished
    /// by case"; the name is therefore read from the user rather than copied into
    /// the login profile record. One definition, shared by the create that returns
    /// the new profile and the read that reports it, so both answer with the same
    /// values from the same state.
    pub fn login_profile_report(&self, user_key: &str) -> Option<LoginProfile> {
        let user = self.users.get(user_key)?;
        let held = self.login_profiles.get(user_key)?;
        Some(LoginProfile {
            user_name: user.user_name.clone(),
            create_date: held.create_date,
            password_reset_required: held.password_reset_required,
        })
    }

    /// Move the user keyed by `from_key` to `user_name` and `path`, carrying everything
    /// the account holds ABOUT that user with it.
    ///
    /// `UpdateUser` "updates the name and/or the path of the specified IAM user", and
    /// "the user's unique identifier remains the same" — which is the whole point of the
    /// move: a user that kept its `UserId` keeps the credentials, policies, memberships
    /// and console password it held, so every collection keyed by the user's name or
    /// carrying its key is re-pointed rather than left behind. The user's ARN is REBUILT,
    /// because it is derived from the path and the name ("if you change the user's name or
    /// path, you must update the policies that refer to it" — the ARN moves with them).
    ///
    /// One definition, so the rename cannot leave a collection behind: a caller adds to
    /// [`Self`] and this method is where the new collection joins the move.
    ///
    /// Answers `false` when `from_key` names no user, and does nothing.
    pub fn rename_user(&mut self, from_key: &str, user_name: &str, path: &str) -> bool {
        let Some(mut user) = self.users.remove(from_key) else {
            return false;
        };
        let to_key = name_key(user_name);
        user.arn = user_arn(path, user_name);
        user.user_name = user_name.to_string();
        user.path = path.to_string();
        self.users.insert(to_key.clone(), user);
        if let Some(policies) = self.user_inline_policies.remove(from_key) {
            self.user_inline_policies.insert(to_key.clone(), policies);
        }
        if let Some(attached) = self.user_attached_policies.remove(from_key) {
            self.user_attached_policies.insert(to_key.clone(), attached);
        }
        if let Some(profile) = self.login_profiles.remove(from_key) {
            self.login_profiles.insert(to_key.clone(), profile);
        }
        for members in self.group_members.values_mut() {
            if members.remove(from_key) {
                members.insert(to_key.clone());
            }
        }
        for record in self.access_keys.values_mut() {
            if record.user_key == from_key {
                record.user_key = to_key.clone();
            }
        }
        for record in self.signing_certificates.values_mut() {
            if record.user_key == from_key {
                record.user_key = to_key.clone();
            }
        }
        for credential in self.service_specific_credentials.values_mut() {
            if name_key(&credential.user_name) == from_key {
                credential.user_name = user_name.to_string();
            }
        }
        true
    }
}
