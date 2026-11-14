// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssoadmin-swbexternalservice service.
//!
//! The records here are the service's own, wire-independent view of an IAM Identity
//! Center instance, its permission sets and its account assignments. Handlers read and
//! write them through [`service_state`] and project them onto the modeled shapes in
//! `crate::ops::types`.
//!
//! An instance carries its KIND ([`InstanceType`]): an ACCOUNT instance (what
//! `CreateInstance` creates, for a standalone account) serves no permission sets, so the
//! whole permission-set / account-assignment surface is withdrawn on it — see
//! [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn`] and
//! [`ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`], which every operation of that surface asks
//! before looking anything up, through the ONE resolution they all share
//! ([`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface`] and
//! [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_target`]). The account's
//! AMBIENT instance — the one `ListInstances` hands out ([`SEED_INSTANCE_TYPE`]) — is an
//! ACCOUNT instance, so that surface is withdrawn on it until an `UpdateInstance` enables
//! permission sets.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccessControlAttribute, AccountAssignment, AccountAssignmentForPrincipal,
    AccountAssignmentOperationStatus,
    AccountAssignmentOperationStatusMetadata, Application, ApplicationAssignment,
    ApplicationProvider, AttachedManagedPolicy, AuthenticationMethod,
    CustomerManagedPolicyReference, DisplayData, EncryptionConfigurationDetails, Grant,
    InstanceAccessControlAttributeConfiguration, InstanceMetadata, InstanceState,
    PermissionSet, PermissionSetProvisioningStatus, PermissionSetProvisioningStatusMetadata,
    PermissionsBoundary, PortalOptions,
    RegionMetadata, ResourceServerConfig, ResourceServerScopeDetails, ScopeDetails, Tag,
    TrustedTokenIssuerConfiguration, TrustedTokenIssuerMetadata,
};

/// Pagination: the cursor scopes this service issues tokens for, the token codec and the
/// paging every enumeration shares. Re-exported, so `crate::state::page` and friends stay
/// the paths handlers and tests use.
mod pagination;
pub use pagination::*;

/// The window a permission set's `SessionDuration` has to fall in, and the refusal a value
/// outside it earns. Re-exported, so `crate::state::session_duration_refusal` stays the
/// path the two permission-set writes ask it through.
mod session_duration;
pub use session_duration::*;

/// The validation every write that takes caller-supplied `Tags` asks of that list.
/// Re-exported, so `crate::state::tag_list_refusal` stays the path those writes share.
mod tag_requests;
pub use tag_requests::*;

/// The identity-store instance every `InstanceArn`-taking operation resolves against.
///
/// IAM Identity Center allows ONE instance per account, so the emulator seeds exactly
/// one at first touch of this state (see the `Default` impl) rather than minting
/// instances on demand. The id is fixed so repeated `ListInstances` calls — and the
/// permission-set ARNs derived from it — are stable for the life of the process.
///
/// `ssoins-` + 16 characters, the form the model's `InstanceArn` pattern accepts:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
pub const SEED_INSTANCE_ID: &str = "ssoins-7223b1d1ac5e1f0e";

/// The identity store connected to the seeded instance: `d-` + 10 hex digits, which
/// `InstanceMetadata$IdentityStoreId` (`^[a-zA-Z0-9-]*$`, 1..64) accepts and which
/// keeps `IdentityStoreArn` inside its modeled 62..93 length.
pub const SEED_IDENTITY_STORE_ID: &str = "d-9067f3a1c2";

/// The Region an instance is enabled in when the request that enabled it names none —
/// a call that carries no SigV4 credential scope to read a Region off. Matches
/// `RegionName`'s `^([a-z]+-){2,3}\d$` pattern.
///
/// It is a FALLBACK, not the Region of every instance: an instance is enabled in the
/// Region of the request that enabled it ([`request_region`]), which is what
/// `DescribeInstance$PrimaryRegion` ("The primary Region where the IAM Identity Center
/// instance was originally enabled") reports.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The seeded instance's display name (`NameType`, `^[\w+=,.@-]+$`).
pub const SEED_INSTANCE_NAME: &str = "IdentityCenter";

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope.
///
/// This is the only Region signal a request carries, and it is what an instance this
/// account holds is "enabled in": IAM Identity Center is a Regional enablement, so an
/// instance enabled through a call made in one Region reports that Region as its primary
/// one rather than a constant the emulator was built with.
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the function
/// that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in the
/// vendored, read-only framework, and a shared crate cannot be introduced for it without
/// editing the root `Cargo.toml`, which this project does not allow.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `InstanceStatus`/`RegionStatus` value for a usable instance: the model's
/// `InstanceStatus` enum is `CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS |
/// ACTIVE`, and a seeded instance is ready, so it is `ACTIVE` and stays `ACTIVE`.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `InstanceStatus::CREATE_IN_PROGRESS` — the status a freshly created instance carries
/// until the clock makes its activation due (see [`INSTANCE_ACTIVATION_SPAN`]).
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `InstanceStatus::DELETE_IN_PROGRESS` — the status an instance carries between the
/// `DeleteInstance` that started its removal and the instant that removal falls due (see
/// [`INSTANCE_DELETION_SPAN`]). `DescribeInstance`'s own documentation declares it
/// observable: "DELETE_IN_PROGRESS - The instance is being deleted. Returns
/// AccessDeniedException after the delete operation completes."
pub const STATUS_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `RegionStatus::ADDING` (the enum is `ACTIVE | ADDING | REMOVING`) — the status a
/// Region carries while, as `RegionMetadata$Status` documents it, the "Region extension
/// workflow is in progress".
pub const STATUS_ADDING: &str = "ADDING";

/// `RegionStatus::REMOVING` — the status a Region carries while the "Region removal
/// workflow is in progress" (`RegionMetadata$Status`), which is what `RemoveRegion` puts
/// the Region it names into: "The Region status is set to REMOVING and the Region record
/// is deleted when the workflow completes."
pub const STATUS_REMOVING: &str = "REMOVING";

/// How long after `DeleteInstance` the instance stops existing.
///
/// AWS publishes no duration for it, so this span is the emulator's own CHOSEN one, on
/// the same footing as [`INSTANCE_ACTIVATION_SPAN`]: it is never zero, so a caller that
/// describes the instance straight after deleting it still observes the
/// `DELETE_IN_PROGRESS` status the model declares and the operation documents, and it is
/// short enough that a test can wait it out. It is held as the instant the removal falls
/// due and applied by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`] — nothing sleeps.
pub const INSTANCE_DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after `CreateInstance` the new instance's move from `CREATE_IN_PROGRESS` to
/// `ACTIVE` falls due.
///
/// AWS documents no duration for it, so this span is the emulator's own CHOSEN one: it
/// is never zero, so a caller that reads the instance straight after creating it still
/// observes the transient status the model declares, and it is short enough that the
/// instance is usable within one test. It is stored as a due instant and materialised by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`] — nothing sleeps.
pub const INSTANCE_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `RemoveRegion` the Region record it names stops existing — the end of
/// the `REMOVING` the remove put it into ("This operation initiates an asynchronous
/// workflow to clean up IAM Identity Center resources in the specified additional Region.
/// The Region status is set to REMOVING and the Region record is deleted when the workflow
/// completes").
///
/// AWS publishes no duration for that workflow, so this span is the emulator's own CHOSEN
/// one, on the same footing as [`INSTANCE_DELETION_SPAN`]: it is never zero, so a caller
/// that reads the instance straight after the remove still observes the `REMOVING` status
/// the model declares, and it is short enough to be waited out inside one test. It is held
/// as the instant the removal falls due and applied by [`Instance::materialize_due`] —
/// nothing sleeps for it.
pub const REGION_REMOVAL_SPAN: Duration = Duration::from_millis(500);

/// How long after `AddRegion` the Region it names finishes its extension workflow — the
/// end of the `ADDING` the add put it into ("This operation initiates an asynchronous
/// workflow to replicate the IAM Identity Center instance to the target Region. The Region
/// status is set to ADDING at first and changes to ACTIVE when the workflow completes").
///
/// AWS publishes no duration for that workflow, so this span is the emulator's own CHOSEN
/// one, on the same footing as [`REGION_REMOVAL_SPAN`]: it is never zero, so a caller that
/// reads the Region straight after the add still observes the `ADDING` status the model
/// declares, and it is short enough to be waited out inside one test. It is held as the
/// instant the activation falls due and applied by [`Region::materialize_due`] — nothing
/// sleeps for it.
pub const REGION_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// Which KIND of IAM Identity Center instance a record is.
///
/// The distinction is AWS's: `CreateInstance` is documented as creating "an instance of
/// IAM Identity Center for a standalone Amazon Web Services account that is not managed
/// by Organizations or a member Amazon Web Services account in an organization" — an
/// ACCOUNT instance — while the instance of an AWS Organizations organization is enabled
/// through Organizations and not through this API. Multi-account permissions (permission
/// sets, and the account assignments built from them) are a feature of the organization
/// instance only, so the whole permission-set / account-assignment surface is withdrawn
/// on an account instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceType {
    /// An instance of a single standalone account: what `CreateInstance` creates.
    Account,
    /// The instance of an AWS Organizations organization.
    Organization,
}

impl InstanceType {
    /// Whether the permission-set / account-assignment surface is served on an instance
    /// of this kind — which is exactly what `DescribeInstance$PermissionSetsEnabled`
    /// ("Indicates whether permission sets are enabled for this Identity Center
    /// instance") reports.
    pub fn permission_sets_enabled(self) -> bool {
        matches!(self, Self::Organization)
    }
}

/// The kind of instance the account's AMBIENT instance is: the instance a client
/// discovers through `ListInstances` because IAM Identity Center was already enabled on
/// this account before the session started.
///
/// It is an ACCOUNT instance: the instance of a single standalone account, which serves
/// no multi-account permissions, so permission sets, the account assignments built from
/// them and the ABAC attribute configuration are all WITHDRAWN on it
/// (`DescribeInstance$PermissionSetsEnabled` reports `false`) until an `UpdateInstance`
/// turns that enablement on — "Enables permission sets for this Identity Center
/// instance", the one documented way it changes.
///
/// The ORGANIZATION instance of an AWS Organizations organization is enabled through
/// Organizations and not through this API, so no operation here hands one out; a caller
/// that needs that surface on this account's instance enables permission sets on it
/// explicitly.
pub const SEED_INSTANCE_TYPE: InstanceType = InstanceType::Account;

/// The refusal every operation of the permission-set / account-assignment surface
/// answers with when the instance it resolves is an ACCOUNT instance, as the message of
/// the modelled `ValidationException` (HTTP 400).
///
/// The 400/`ValidationException` classification is the requirement — a client that
/// branches on the error must be able to tell "this instance type can never do
/// assignments" from "the permission set is merely missing" (404) — and this is AWS's
/// own wording for the restriction.
pub const ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE: &str =
    "This operation is not supported for account instances of IAM Identity Center.";

/// The refusal `CreateInstance` answers with when the account already holds an
/// instance, as the message of the modelled `ServiceQuotaExceededException`.
///
/// The rule is the operation's own documentation: "You can create only one instance per
/// account and across all Amazon Web Services Regions", and "The CreateInstance request
/// is rejected if … An instance already exists in the same account". AWS reports it as a
/// QUOTA on the account — it names the account the quota was crossed in and points at
/// the published limits — so the message is BUILT from the account the request ran under
/// rather than being a constant about instances in general.
pub fn instance_quota_exceeded_message() -> String {
    format!(
        "You have exceeded IAM Identity Center quotas. Cannot create more than 1 \
         instance for {}. Please refer to \
         https://docs.aws.amazon.com/singlesignon/latest/userguide/limits.html",
        ::engine::emulator_account_id()
    )
}

/// The refusal a repeated `@idempotencyToken` earns when the rest of the request differs
/// from the call that token belongs to, as the message of the modelled
/// `ConflictException` (HTTP 409).
///
/// The rule is the token member's own documentation: "Passing the same value to a later
/// call to an operation requires that you also pass the same value for all other
/// parameters... If you retry the operation with the same ClientToken, but with different
/// parameters, the retry fails". The sentence AWS answers that refusal with is FIXED for
/// the error class — it carries no variable part, naming neither the token nor the
/// resource the first call created — so it is a constant here, and every create of this
/// service that carries an `@idempotencyToken` (`CreateInstance`, `CreateApplication`,
/// `CreateTrustedTokenIssuer`) refuses through this one constant: one condition reads ONE
/// way whichever operation the client called.
pub const CLIENT_TOKEN_MISMATCH_MESSAGE: &str =
    "The request parameters are different from that of a previous request";

/// The refusal an `InstanceArn` this account holds no instance for earns, as the message
/// of whichever not-found the calling operation models — the `ResourceNotFoundException`
/// (HTTP 404) most of this API declares, or the `AccessDeniedException` (HTTP 403) that is
/// the only such channel on `DescribeInstance`/`DeleteInstance`, `AddRegion`,
/// `ListRegions`, `ListApplications` and the trusted-token-issuer create and list
/// ("DELETE_IN_PROGRESS — … Returns AccessDeniedException after the delete operation
/// completes").
///
/// One condition reads ONE way: every operation that resolves an `InstanceArn` reports it
/// through this helper, so a client that branches on the detail text — the only member
/// these error shapes carry — sees the same sentence for one ARN whichever operation it
/// called.
///
/// Built from the request: it names the ARN that resolved to nothing.
pub fn instance_not_found_message(instance_arn: &str) -> String {
    format!("No IAM Identity Center instance {instance_arn} exists in this account")
}

/// The refusal a `PermissionSetArn` that names no permission set of the resolved instance
/// earns, as the message of the modelled `ResourceNotFoundException`.
///
/// One condition reads ONE way, exactly as [`instance_not_found_message`] keeps it for the
/// instance: every operation that names a permission set reports it through
/// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_target`], so a client that
/// branches on the detail text sees the same sentence whichever operation it called.
///
/// Built from the request: it names the ARN that resolved to nothing. Never created,
/// already deleted, gone with its instance, or held under another instance — the same
/// answer either way ([`ComAmazonawsSsoadminSwbexternalserviceState::permission_set`]).
pub fn permission_set_not_found_message(permission_set_arn: &str) -> String {
    format!("No permission set found with the ARN {permission_set_arn}")
}

/// Why a request on the permission-set / account-assignment / ABAC surface never reached
/// the resource its ARNs named, as resolved by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::abac_surface_mut`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_target`] and
/// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_target_mut`].
///
/// Each operation declares its own error enum, so the resolution answers the refusal and
/// the handler maps it onto the variant its model carries — the division [`TagRefusal`]
/// already keeps for the tag surface. The error shape a handler builds from it carries this
/// message and no `reason`: the model's `ValidationExceptionReason` and
/// `ResourceNotFoundExceptionReason` enums carry only KMS key states, none of which these
/// refusals is.
#[derive(Debug)]
pub enum PermissionSetSurfaceRefusal {
    /// The instance the request resolved is an ACCOUNT instance, on which this whole
    /// surface is withdrawn: the operation's modelled `ValidationException` carries this
    /// message ([`ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`]).
    Unsupported(String),
    /// Nothing the request named is held: no instance under the `InstanceArn`, or no
    /// permission set under the `PermissionSetArn` within it. The operation's modelled
    /// `ResourceNotFoundException` carries this message.
    NotFound(String),
}

/// The refusal an operation OTHER than the instance's own describe and delete earns while
/// that instance's creation has not finished, as the message of the modelled
/// `ConflictException`.
///
/// The rule is `DescribeInstance`'s own documentation of the status: "CREATE_IN_PROGRESS -
/// The instance is in the process of being created. … While the instance is in the
/// CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance
/// operations." A `ConflictException` is how the service reports exactly that — "a conflict
/// with a previous successful write … A retry (with appropriate backoff logic) is the
/// recommended response" — since the previous successful write is the `CreateInstance`
/// still in flight and the same call is served once it finishes.
///
/// Built from the request and the stored record: it names the ARN whose instance was
/// resolved and the status that instance is in.
pub fn instance_creation_in_progress_message(instance_arn: &str) -> String {
    format!(
        "Instance {instance_arn} is in status {STATUS_CREATE_IN_PROGRESS}. While the \
         instance is in the {STATUS_CREATE_IN_PROGRESS} state, you can call only \
         DescribeInstance and DeleteInstance operations."
    )
}

/// A refusal of the APPLICATION surface: `what` was not found under the application the
/// request named.
///
/// `No <what> found for application arn: <arn>` is how this service words every not-found
/// of that surface — the application itself, and each of the singleton children a caller
/// configures on it (an access scope, an authentication method, a grant) — so one
/// condition reads ONE way whichever operation the client called, and the caller learns
/// WHAT was missing and the application it was looked for under. Built from the request:
/// both parts are the caller's own values.
fn application_surface_not_found_message(what: &str, application_arn: &str) -> String {
    format!("No {what} found for application arn: {application_arn}")
}

/// The refusal a `Scope` an application does not carry earns, as the message of the
/// modelled `ResourceNotFoundException`.
///
/// Built from the request: it names both the scope and the application it was looked for
/// on, because an unknown scope on a known application is a different failure from an
/// unknown application.
pub fn access_scope_not_found_message(application_arn: &str, scope: &str) -> String {
    application_surface_not_found_message(&format!("access scope {scope}"), application_arn)
}

/// The refusal a `TrustedTokenIssuerArn` nothing holds earns, as the message of the
/// modelled `ResourceNotFoundException`.
///
/// Built from the request: AWS names the missing trusted token issuer by the ARN the
/// caller asked for, which is what tells "this ARN is gone" from "some other resource of
/// this call is missing".
pub fn trusted_token_issuer_not_found_message(trusted_token_issuer_arn: &str) -> String {
    format!("Could not find TrustedTokenIssuer with arn {trusted_token_issuer_arn}.")
}

/// What an application is called in the refusals of its own surface: the application is
/// the RESOURCE the request named, where a scope or a grant is one thing configured on it
/// ([`application_surface_not_found_message`]).
const APPLICATION_SURFACE_RESOURCE: &str = "resource";

/// The refusal an `ApplicationArn` this account holds no application for earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: it names the ARN that resolved to nothing, which is what tells
/// "this application is gone" from "some other resource of this call is missing". Never
/// created, already deleted, or gone with the instance that held it — the same answer
/// either way.
pub fn application_not_found_message(application_arn: &str) -> String {
    application_surface_not_found_message(APPLICATION_SURFACE_RESOURCE, application_arn)
}

/// The detail text a `DescribeApplicationAssignment` or `DeleteApplicationAssignment` of a
/// principal that holds no DIRECT assignment to the application answers with: NONE.
///
/// SOURCE: recorded behavior — this refusal is the modelled `ResourceNotFoundException`
/// with no detail text at all, so the handlers report none rather than wording one of
/// their own. The condition is fully described by the request the caller already holds (an
/// application, a principal type and a principal id), and nothing of the store is
/// disclosed. An `ApplicationArn` this account holds no application for is a DIFFERENT
/// condition and keeps its own message ([`application_not_found_message`]).
pub const APPLICATION_ASSIGNMENT_NOT_FOUND_MESSAGE: Option<&str> = None;

/// The refusal a delete of an account assignment that is not granted earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: an assignment's identity is the whole (account, permission
/// set, principal type, principal id) tuple, so the refusal names every part of it
/// rather than one resource of the call.
pub fn account_assignment_not_found_message(
    account_id: &str,
    permission_set_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> String {
    format!(
        "Could not find an assignment of permission set {permission_set_arn} to \
         {principal_type} {principal_id} on account {account_id}."
    )
}

/// The refusal a `RequestId` this instance holds no assignment-operation status record
/// for earns, as the message of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`trusted_token_issuer_not_found_message`]: it names the
/// request id that resolved to nothing, which is what tells "no operation was ever
/// tracked under this id" from "some other resource of this call is missing". One helper
/// for both halves of that history: a creation status and a deletion status are looked up
/// the same way, by the id the operation answered with.
pub fn assignment_operation_not_found_message(request_id: &str) -> String {
    format!("Could not find the status of an assignment operation with request id {request_id}.")
}

/// The refusal a `RequestId` this instance tracks no permission-set PROVISIONING under
/// earns, as the message of the modelled `ResourceNotFoundException`.
///
/// Built from the request, exactly as [`assignment_operation_not_found_message`] is: it
/// names the request id that resolved to nothing. A separate helper because the two are
/// separate histories — a provisioning request id is not an assignment operation's, and
/// neither answers a read of the other.
pub fn permission_set_provisioning_not_found_message(request_id: &str) -> String {
    format!(
        "Could not find the status of a permission set provisioning with request id \
         {request_id}."
    )
}

/// `ApplicationStatus::ENABLED` (the enum is `ENABLED | DISABLED`) — the status an
/// application carries when the create that made it named none.
///
/// CHOSEN default: the model marks `CreateApplication$Status` optional and declares no
/// `@default` for it, and AWS publishes no value for an omitted one, so the emulator
/// picks the usable state — an application nobody disabled is enabled, which is also
/// what the `PortalOptions$Visibility` member of the same request defaults to.
pub const APPLICATION_STATUS_ENABLED: &str = "ENABLED";

/// `ApplicationVisibility::ENABLED` — the value `PortalOptions$Visibility` takes when
/// the request names none. This one is the MODEL's default, not a choice:
/// `smithy_query(query="shape",shape="ApplicationVisibility")` carries
/// `"default": "ENABLED"`.
pub const APPLICATION_VISIBILITY_ENABLED: &str = "ENABLED";

/// The assignment configuration an application carries before any
/// `PutApplicationAssignmentConfiguration` sets one.
///
/// The MODEL's default, not a choice: `AssignmentRequired` carries `"default": true` on
/// both the put's input and the get's output, and the member's own documentation states
/// it — "If AssignmentsRequired is true (default value), users don't have access to the
/// application unless an assignment is created using the CreateApplicationAssignment
/// API."
pub const APPLICATION_ASSIGNMENT_REQUIRED_DEFAULT: bool = true;

/// `UserBackgroundSessionApplicationStatus::DISABLED` (the enum is `ENABLED | DISABLED`)
/// — the status an application's SESSION configuration carries before any
/// `PutApplicationSessionConfiguration` sets one.
///
/// CHOSEN default, not the model's: `UserBackgroundSessionApplicationStatus` declares no
/// `@default` on either side of the pair and AWS publishes no value for an application
/// nothing has configured, so the emulator reports the resting state — a background
/// session nothing has enabled is not enabled, which this enum spells `DISABLED`.
pub const USER_BACKGROUND_SESSION_STATUS_DEFAULT: &str = "DISABLED";

/// `SignInOrigin::APPLICATION` — the origin that redirects the user to the application's
/// own URL, which is why `SignInOptions$ApplicationUrl` is required with it.
pub const SIGN_IN_ORIGIN_APPLICATION: &str = "APPLICATION";

/// The refusal a `SignInOptions` with `Origin` `APPLICATION` and no `ApplicationUrl`
/// earns, as the message of the modelled `ValidationException`.
///
/// The rule is `SignInOptions$ApplicationUrl`'s own documentation: "The URL that accepts
/// authentication requests for an application. This is a required parameter if the
/// Origin parameter is APPLICATION." The model cannot express a conditionally required
/// member, so the boundary validator passes the request and the handler refuses it.
pub const APPLICATION_URL_REQUIRED_MESSAGE: &str =
    "PortalOptions.SignInOptions.ApplicationUrl is required when \
     PortalOptions.SignInOptions.Origin is APPLICATION.";

/// The refusal `CreateApplication` answers with when `ApplicationProviderArn` names a
/// published provider that cannot back a CUSTOMER MANAGED OAuth 2.0 application, as the
/// message of the modelled `ValidationException` (HTTP 400).
///
/// The rule is the operation's own documentation: it creates "an OAuth 2.0 customer
/// managed application … for the given application provider" and "does not support
/// creating SAML 2.0 customer managed applications or Amazon Web Services managed
/// applications" — a SAML 2.0 application is set up in the console and an Amazon Web
/// Services managed application is created by the service that owns its provider.
///
/// Built from the request: it names the provider ARN that cannot back the create, which
/// is what tells this refusal from the one a provider ARN nothing publishes earns
/// (`ResourceNotFoundException`).
pub fn application_provider_unsupported_message(application_provider_arn: &str) -> String {
    format!(
        "Application provider {application_provider_arn} cannot be used to create an \
         application. This operation creates OAuth 2.0 customer managed applications only."
    )
}

/// The refusal an `ApplicationProviderArn` that names no PUBLISHED application provider
/// earns, as the message of the modelled `ResourceNotFoundException` (HTTP 404).
///
/// An application provider is AWS-owned (the modelled `ApplicationProviderArn` pattern
/// pins the owner segment to `aws`) and no operation creates one, so the providers that
/// exist are exactly the published catalogue
/// ([`BUILT_IN_APPLICATION_PROVIDERS`]) and an ARN outside it resolves to nothing.
///
/// One condition reads ONE way, exactly as [`instance_not_found_message`] keeps it for the
/// instance: every operation that resolves an `ApplicationProviderArn` reports it through
/// this helper, so a client that branches on the detail text — the only member these error
/// shapes carry — sees the same sentence for one provider ARN whichever operation it
/// called. It is distinct from [`application_provider_unsupported_message`], which a
/// provider that IS published but backs no customer managed OAuth 2.0 application earns.
///
/// Built from the request: it names the ARN that resolved to nothing.
pub fn application_provider_not_found_message(application_provider_arn: &str) -> String {
    format!("No application provider with arn '{application_provider_arn}' was found")
}

/// `KmsKeyType::AWS_OWNED_KMS_KEY` — the instance is encrypted with an Amazon Web
/// Services owned key, so no `KmsKeyArn` names it ("Cannot be specified when KeyType is
/// AWS_OWNED_KMS_KEY", `EncryptionConfiguration$KmsKeyArn`).
pub const KEY_TYPE_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";

/// `KmsKeyType::CUSTOMER_MANAGED_KEY` — the instance is encrypted with the caller's own
/// KMS key, which `KmsKeyArn` must name ("Required when KeyType is
/// CUSTOMER_MANAGED_KEY").
pub const KEY_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KEY";

/// `KmsKeyStatus::UPDATING` — an encryption-configuration change that has not taken
/// effect yet.
pub const KMS_KEY_STATUS_UPDATING: &str = "UPDATING";

/// `KmsKeyStatus::ENABLED` — the encryption configuration an instance is actually
/// encrypted under.
pub const KMS_KEY_STATUS_ENABLED: &str = "ENABLED";

/// How long after the `UpdateInstance` that sets an encryption configuration the
/// instance reports that configuration as `ENABLED` rather than `UPDATING`.
///
/// The key change is asynchronous — the model carries `KmsKeyStatus::UPDATING` and an
/// `EncryptionStatusReason` for the failure case precisely because the caller can observe
/// the change in flight — and AWS publishes no duration for it, so this span is the
/// emulator's OWN choice: non-zero, so a caller that reads straight after the update still
/// sees `UPDATING`, and short enough that a test can wait it out.
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// `InstanceAccessControlAttributeConfigurationStatus::ENABLED` — the terminal status of
/// an ABAC configuration whose enabling has finished.
pub const ABAC_STATUS_ENABLED: &str = "ENABLED";

/// `InstanceAccessControlAttributeConfigurationStatus::CREATION_IN_PROGRESS` — the status
/// an ABAC configuration carries between the create that enabled it and the instant the
/// clock makes its enablement due (see [`ABAC_ENABLE_SPAN`]).
pub const ABAC_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// How long after `CreateInstanceAccessControlAttributeConfiguration` the configuration
/// reports `ENABLED` rather than `CREATION_IN_PROGRESS`.
///
/// Enabling ABAC is asynchronous — the model carries the non-terminal
/// `CREATION_IN_PROGRESS` status and the operation's own documentation says "After a
/// successful response, call DescribeInstanceAccessControlAttributeConfiguration to
/// validate that InstanceAccessControlAttributeConfiguration was created", so the caller
/// can observe the enabling in flight — and AWS publishes no duration for it, so this
/// span is the emulator's OWN choice: non-zero, so a caller that describes straight after
/// the create still sees the transient status, and short enough that a test can wait it
/// out. It is held as the instant the change falls due and the status is DERIVED from it
/// (see [`AccessControlAttributeConfigurationRecord::status`]); nothing sleeps for it.
pub const ABAC_ENABLE_SPAN: Duration = Duration::from_millis(500);

/// `StatusValues::SUCCEEDED` — the terminal status of an assignment operation whose
/// completion the clock has already made due.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_PROVISIONED` — the provisioning status of
/// an account whose copy of a permission set is the permission set's CURRENT definition
/// ([`PermissionSetRecord::definition_revision`]).
pub const PROVISIONING_STATUS_LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_NOT_PROVISIONED` — the provisioning status
/// of an account that holds the permission set, but a copy of an EARLIER definition of
/// it: the permission set has changed since it was provisioned there, so the account does
/// not yet have the latest one.
pub const PROVISIONING_STATUS_LATEST_NOT_PROVISIONED: &str =
    "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// The provisioning status a provisioned copy of a permission set is in: the revision that
/// copy was provisioned at, against the permission set's revision now.
///
/// The ONE place that comparison is made, called from the ONE pass over the records that
/// decides which copy an account holds
/// ([`ComAmazonawsSsoadminSwbexternalserviceState::provisioned_copies_held`]), so the two
/// projections of that pass cannot drift apart about which copy is up to date.
fn provisioning_status_of(provisioned_revision: &str, current_revision: &str) -> &'static str {
    if provisioned_revision == current_revision {
        PROVISIONING_STATUS_LATEST_PROVISIONED
    } else {
        PROVISIONING_STATUS_LATEST_NOT_PROVISIONED
    }
}

/// The ordering key of the instant a provisioning CAPTURED the definition it carries: the
/// whole seconds and the sub-second nanoseconds of that instant, which compare exactly as
/// the instant itself does.
fn capture_key(captured_at: &DateTime) -> (i64, u32) {
    (captured_at.secs(), captured_at.subsec_nanos())
}

/// Whether a provisioning that captured its definition at `captured_at` is NOT OLDER than
/// the provisioning whose copy is recorded, which captured its own definition at
/// `recorded_at` — the condition under which the first one's copy is the one in place.
///
/// A provisioning copies the definition it captured when it was ACCEPTED, so which copy an
/// account ends up holding is decided by those capture instants and not by the order the
/// copies happen to land in: a provisioning accepted while an earlier one was still in
/// flight carries the later definition, and the earlier one landing afterwards must not
/// rewind the account to the definition it captured. The two instants being EQUAL answers
/// true, hence "not older": two provisionings that captured at the same instant captured the
/// same definition, so either may stand.
fn is_not_older_than_recorded_copy(captured_at: &DateTime, recorded_at: &DateTime) -> bool {
    capture_key(captured_at) >= capture_key(recorded_at)
}

/// `ProvisionTargetType::AWS_ACCOUNT` — the one of the model's two provisioning targets
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`) that names ONE Amazon Web Services account,
/// which is the account `ProvisionPermissionSet$TargetId` carries. The other names every
/// account the permission set is already provisioned into, so it carries no account.
pub const PROVISION_TARGET_TYPE_AWS_ACCOUNT: &str = "AWS_ACCOUNT";

/// The refusal a provisioning that targets ONE account without naming it earns, as the
/// message of the modelled `ValidationException`.
///
/// BUILT from the request: it names the member the request left out and the target type
/// that requires it ("TargetID is an Amazon Web Services account identifier"), so the
/// caller can fix the request from this text alone.
pub fn target_id_required_message(target_type: &str) -> String {
    format!("TargetId is required when TargetType is {target_type}")
}

/// `StatusValues::IN_PROGRESS` — the NON-TERMINAL status an assignment operation carries
/// from the call that starts it until the instant its completion falls due (see
/// [`ASSIGNMENT_OPERATION_SPAN`]).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// How long after the call that STARTS an account-assignment operation that operation's
/// move from `IN_PROGRESS` to `SUCCEEDED` falls due.
///
/// Creating an account assignment is asynchronous — the model's `StatusValues` enum
/// carries the non-terminal `IN_PROGRESS` and `CreateAccountAssignment`'s own
/// documentation says "After a successful response, call
/// DescribeAccountAssignmentCreationStatus to describe the status of an assignment
/// creation request", which is only meaningful while the operation is still in flight —
/// and AWS publishes no duration for it, so this span is the emulator's OWN choice, on
/// the same footing as [`ABAC_ENABLE_SPAN`]: never zero, so a caller that polls straight
/// after the create still observes the in-flight status, and short enough that a test can
/// wait it out. It is held as the instant the completion falls due and the status is
/// DERIVED from it ([`AssignmentOperationRecord::status`]); nothing sleeps for it.
pub const ASSIGNMENT_OPERATION_SPAN: Duration = Duration::from_millis(500);

/// How long after the call that STARTS a permission-set provisioning that provisioning
/// falls due: the instant its status leaves `IN_PROGRESS` for `SUCCEEDED` and the copy it
/// carries reaches the accounts it targets.
///
/// Provisioning a permission set is asynchronous — `ProvisionPermissionSet` answers with a
/// status object whose `StatusValues` carries the non-terminal `IN_PROGRESS`, and
/// `DescribePermissionSetProvisioningStatus` exists to "retrieve the current status of the
/// provisioning workflow" — and AWS publishes no duration for that workflow, so this span
/// is the emulator's OWN choice, on the same footing as [`ASSIGNMENT_OPERATION_SPAN`]:
/// never zero, so a caller that polls straight after the call still observes the in-flight
/// status, and short enough that a test can wait it out. It is held as the instant the
/// completion falls due and the status is DERIVED from it
/// ([`PermissionSetProvisioningRecord::status`]); nothing sleeps for it.
pub const PERMISSION_SET_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The page size a paginated read uses when the caller sends no `MaxResults`, and the
/// ceiling it clamps a caller's value to: the model's `MaxResults` shape is
/// `range(min: 1, max: 100)`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The ceiling a `ListApplications` page is clamped to, below [`MAX_PAGE_SIZE`].
///
/// The model bounds its OUTPUT list `ApplicationList` at `length(min: 0, max: 50)` while
/// its `MaxResults` still admits up to 100, so a page that honoured a `MaxResults` of 100
/// over 100 stored applications would answer a list the model forbids. The extra items
/// are not dropped: the page carries 50 and the enumeration continues through its
/// `NextToken`, which is what the member documents ("the service might return fewer
/// results than the maximum even when there are more results available").
pub const MAX_APPLICATIONS_PER_PAGE: i32 = 50;

/// The largest page `ListApplicationAccessScopes` can answer: the maximum its own
/// `MaxResults` MEMBER declares, which narrows the `MaxResults` shape's `range(min: 1,
/// max: 100)` and is the same bound its response's `Scopes` list carries
/// (`length(min: 0, max: 10)`), so no larger page is expressible.
///
/// The validating walk enforces the constraints the SCHEMA carries, which are the target
/// shape's, so the handler of an operation whose member narrows them states the narrower
/// bound itself — see [`crate::service::refusal::value_above_maximum`].
pub const MAX_ACCESS_SCOPES_PER_PAGE: i32 = 10;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores ([`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::account_assignments`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_operations`]).
///
/// This is NOT AWS's service quota — IAM Identity Center's published permission-set and
/// assignment limits are per-instance and adjustable, and nothing here claims to
/// reproduce them. It is the emulator's own MEMORY CEILING: every successful create
/// inserts a record that lives until a caller deletes it, so without a ceiling a caller
/// could grow the process until the host runs out of memory. A request that would cross
/// it is refused with the modelled `ServiceQuotaExceededException`, which AWS documents
/// as "the principal has crossed the permitted number of resources that can be
/// created", so the refusal is one a real client already handles.
///
/// One store does not refuse at this ceiling:
/// [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_deletions`] holds the
/// history of deletions that have ALREADY been applied, so it evicts its oldest records
/// instead (see
/// [`ComAmazonawsSsoadminSwbexternalserviceState::record_assignment_deletion`]).
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"permission sets"`.
pub fn quota_exceeded_message(store: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// The maximum number of ACCESS SCOPES this emulator keeps on any ONE application
/// ([`ApplicationRecord::access_scopes`]).
///
/// This is NOT an AWS quota — IAM Identity Center publishes none for it. It is the
/// emulator's own MEMORY CEILING, the per-application counterpart of
/// [`MAX_RECORDS_PER_STORE`]: `PutApplicationAccessScope` inserts one entry per distinct
/// `Scope` name, the model's `Scope` pattern admits unboundedly many of them, and a
/// stored scope lives until a caller deletes it — so without a ceiling a caller could
/// grow the process until the host runs out of memory. A put that would add a scope
/// beyond it is refused.
pub const MAX_SCOPES_PER_APPLICATION: usize = 100;

/// The message a put refuses a new access scope with once an application already holds
/// [`MAX_SCOPES_PER_APPLICATION`] of them; built from the request, which names the
/// application the ceiling was reached on.
pub fn access_scope_ceiling_message(application_arn: &str) -> String {
    format!(
        "Cannot add another access scope: this emulator holds at most \
         {MAX_SCOPES_PER_APPLICATION} access scopes per application, and \
         {application_arn} already holds that many"
    )
}

/// The maximum number of REGIONS this emulator keeps on any ONE instance
/// ([`Instance::regions`]).
///
/// This is NOT an AWS quota — IAM Identity Center publishes none for the number of
/// Regions an instance can be enabled in. It is the emulator's own MEMORY CEILING, the
/// per-instance counterpart of [`MAX_SCOPES_PER_APPLICATION`]: enabling an instance in
/// another Region adds one record per distinct `RegionName`, the model's `RegionName`
/// pattern (`^([a-z]+-){2,3}\d$`) admits unboundedly many of them, and a Region record
/// lives until a caller removes it — so without a ceiling a caller could grow the process
/// until the host runs out of memory. [`Instance::add_region`] is the one place a record
/// is appended, and it refuses an add that would cross this ceiling.
pub const MAX_REGIONS_PER_INSTANCE: usize = 50;

/// The message an add refuses a new Region with once an instance already holds
/// [`MAX_REGIONS_PER_INSTANCE`] of them; built from the instance the ceiling was reached
/// on, and worded as the modelled `ServiceQuotaExceededException` describes itself ("the
/// principal has crossed the permitted number of resources that can be created"), so the
/// refusal is one a real client already handles.
pub fn region_ceiling_message(instance_arn: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: this emulator holds at most {MAX_REGIONS_PER_INSTANCE} Regions per \
         instance, and {instance_arn} already holds that many"
    )
}

/// The maximum number of TAGS a taggable resource of this service (an instance, a
/// permission set, an application or a trusted token issuer) can carry.
///
/// AWS's published tagging limit — a resource carries at most 50 user-created tags, the
/// number IAM Identity Center publishes for its taggable resources — so a write that would
/// push a resource past it is refused with the modelled `ServiceQuotaExceededException`,
/// which AWS documents as "the principal has crossed the permitted number of resources
/// that can be created". It is NOT the model's `TagList` bound, which is wider
/// (`length(min: 0, max: 75)` — the most one REQUEST can carry and one page can report)
/// and therefore admits a request this ceiling refuses.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix AWS reserves for the tags it applies ITSELF — a SYSTEM (service-owned) tag.
///
/// A key under it belongs to Amazon Web Services rather than to the caller, so an end user
/// may neither write one nor remove one ([`mutates_system_tag`]); a key that merely
/// CONTAINS the prefix, or starts with `aws` and no colon, is an ordinary tag.
pub const SYSTEM_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a SYSTEM tag: one in the reserved [`SYSTEM_TAG_KEY_PREFIX`]
/// namespace.
///
/// Matched without regard to case, because the namespace is AWS's however the caller
/// spells it, and `TagKey`'s pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) admits both
/// spellings. A PREFIX test and nothing more: the reservation is on where the segment
/// stands, so `my-aws:thing` is the caller's own key.
pub fn is_system_tag_key(key: &str) -> bool {
    key.get(..SYSTEM_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(SYSTEM_TAG_KEY_PREFIX))
}

/// Whether `keys` asks to mutate a SYSTEM tag — the one question every tag write of this
/// service asks before it changes anything.
///
/// ONE key under the reserved namespace is enough, wherever it stands in the request: the
/// write is refused whole, so a request that names an ordinary key beside a system one
/// stores neither.
///
/// WHERE IT IS ASKED, and the order this service keeps for its pre-mutation refusals: this
/// one reads the REQUEST and nothing else, so what it settles no state can change and every
/// operation asks it FIRST — before the resource the request names is resolved, and so
/// before the refusals that need a resolved record. Those follow, broadest withdrawal
/// first: the instance's own `CREATE_IN_PROGRESS` window, which withdraws every operation
/// but the instance's describe and delete; then the instance's KIND, which withdraws one
/// surface ([`Instance::region_write_refusal`]); then what the request named inside it.
pub fn mutates_system_tag<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    keys.into_iter().any(is_system_tag_key)
}

/// The refusal a request that would mutate a SYSTEM tag earns, as the message of the
/// modelled `ValidationException` (HTTP 400).
///
/// SOURCE: recorded behaviour of `TagResource` and `UntagResource` naming a service-owned
/// key — the model documents only "Associates a set of tags with a specified resource" and
/// states no such rule. The sentence is wholly FIXED for the error class: it interpolates
/// nothing from the request, naming neither the key nor the resource. Every write that
/// takes caller-supplied tags refuses through this one constant — the tagging pair and the
/// four creates that carry `Tags` — so one condition reads ONE way whichever operation the
/// client called, exactly as [`instance_not_found_message`] keeps the missing instance.
pub const SYSTEM_TAG_MUTATION_MESSAGE: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// The message a tagging write refuses with once a resource would carry more than
/// [`MAX_TAGS_PER_RESOURCE`] distinct keys, as the message of the modelled
/// `ServiceQuotaExceededException`.
///
/// The wording for this refusal is FIXED apart from the limit it states twice: it reports
/// the ceiling, and that the tag set the write would LEAVE BEHIND may not cross it, and
/// names neither the resource nor the tags. The limit is [`MAX_TAGS_PER_RESOURCE`], so the
/// sentence always states the ceiling the write was judged against.
pub fn tag_ceiling_message() -> String {
    format!(
        "{MAX_TAGS_PER_RESOURCE} user tags are allowed for a resource at maximum, The \
         resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags."
    )
}

/// The refusal a `ResourceArn` this account holds no TAGGABLE resource for earns, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: it names the resource TYPE the ARN declares and the id the
/// caller sent. An ARN held under a different instance than the `InstanceArn` the request
/// carried resolves to nothing too — see
/// [`ComAmazonawsSsoadminSwbexternalserviceState::tag_target`].
///
/// `Could not find <Type> with id <arn>.` — the tagging surface reaches all four taggable
/// kinds through one `ResourceArn`, so the refusal says WHICH kind the ARN named, which is
/// the only thing known about a resource nothing is held under.
pub fn taggable_resource_not_found_message(resource_arn: &str) -> String {
    match taggable_resource_type(resource_arn) {
        Some(kind) => format!("Could not find {kind} with id {resource_arn}."),
        None => format!("Could not find a taggable resource with arn {resource_arn}."),
    }
}

/// The resource TYPE a `TaggableResourceArn` declares, named as a refusal names it, or
/// `None` for an ARN of none of the four kinds.
///
/// The model's `TaggableResourceArn` puts the kind in the segment that FOLLOWS the
/// account (`^arn:aws(-[a-z]{1,5}){0,3}:sso::` then `:instance/`, `:permissionSet/`, or a
/// 12-digit account and then `:application/` or `:trustedTokenIssuer/`), so the kind is
/// readable from the ARN itself — which is what lets a refusal name it even when nothing
/// is held under the ARN. Matched THERE and nowhere else: a resource id that happens to
/// hold one of those words is still the kind its own segment declares.
fn taggable_resource_type(resource_arn: &str) -> Option<&'static str> {
    let (prefix, rest) = resource_arn.split_once(":sso::")?;
    if !prefix.starts_with("arn:") {
        return None;
    }
    // What stands between `:sso::` and the kind is the owning account, which two of the
    // four kinds carry and two leave empty.
    let (_account, resource) = rest.split_once(':')?;
    [
        ("instance/", "Instance"),
        ("permissionSet/", "PermissionSet"),
        ("application/", TAGGABLE_TYPE_APPLICATION),
        ("trustedTokenIssuer/", "TrustedTokenIssuer"),
    ]
    .into_iter()
    .find_map(|(segment, kind)| resource.starts_with(segment).then_some(kind))
}

/// The resource type [`taggable_resource_type`] answers for an `ApplicationArn`.
const TAGGABLE_TYPE_APPLICATION: &str = "Application";

/// The refusal a tag REPORT answers a `ResourceArn` nothing is held under with, as the
/// message of the modelled `ResourceNotFoundException`.
///
/// SOURCE: recorded behavior for an APPLICATION, which is the one kind the two paths of
/// this surface word differently: `ListTagsForResource` answers the sentence the
/// application's own reads answer ([`application_not_found_message`]), while
/// `TagResource` and `UntagResource` name the resource KIND and the id they were asked
/// for ([`taggable_resource_not_found_message`]). Nothing is recorded for the other three
/// kinds, so they keep ONE wording across the whole surface rather than an asymmetry
/// extended by analogy.
pub fn tag_report_not_found_message(resource_arn: &str) -> String {
    if taggable_resource_type(resource_arn) == Some(TAGGABLE_TYPE_APPLICATION) {
        return application_not_found_message(resource_arn);
    }
    taggable_resource_not_found_message(resource_arn)
}

/// Why a tag call could not reach the resource its `ResourceArn` named, as resolved by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::tag_target`] and
/// [`ComAmazonawsSsoadminSwbexternalserviceState::tag_target_mut`].
///
/// Each operation declares its own error enum, so the resolution answers the refusal and
/// the handler maps it onto the variant its model carries — the division
/// [`ComAmazonawsSsoadminSwbexternalserviceState::add_region`] and [`cursor_after`]
/// already keep. BOTH sides of the tag surface answer the same two refusals, because both
/// resolve the resource the same way and both are withheld while the instance holding it
/// is still being created; only the channel each one lands on differs, and that is the
/// handler's business.
#[derive(Debug)]
pub enum TagRefusal {
    /// Nothing the request could tag is held under the ARNs it named: the operation's
    /// modelled `ResourceNotFoundException` carries this message.
    NotFound(String),
    /// The instance holding the resource is still being created, so this call is not one
    /// that window serves ([`instance_creation_in_progress_message`]): the two writes
    /// carry this message on their modelled `ConflictException`, and the report — whose
    /// model declares no `ConflictException` — on its modelled `AccessDeniedException`,
    /// the channel every instance-scoped read of this service answers an instance it may
    /// not act on with.
    Conflict(String),
}

/// Merge `requested` into the tag set `stored`, which is what a tagging write composes
/// with what a resource already carries: a key the resource already holds takes the
/// requested value in place, a key it does not hold is added, and every other key is
/// left untouched.
///
/// A tag set is keyed by `Tag$Key`, so it holds ONE value per key: the value a key ends up
/// with is the one written LAST, and a key this merge writes is left holding one entry, so
/// the value stored here is the value [`tags_in_key_order`] reports. A request that named
/// one key twice never reaches this merge — [`tag_list_refusal`] refuses it before anything
/// is written — so the repetition this handles is a key the RESOURCE already carries, which
/// is what a merge is for.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) {
    for tag in requested {
        let mut written = false;
        stored.retain_mut(|held| {
            if held.key != tag.key {
                return true;
            }
            if written {
                // A second entry for the same key: the set holds one value per key, so
                // the entry this write just valued is the one that stays.
                return false;
            }
            held.value = tag.value.clone();
            written = true;
            true
        });
        if !written {
            stored.push(tag.clone());
        }
    }
}

/// Remove from the tag set `stored` every entry whose `Tag$Key` is one of `keys`, which
/// is what an untagging write does to what a resource already carries: a key named is
/// gone whatever value it held, a key not named is left untouched.
///
/// A key the resource does not carry removes nothing rather than failing, so repeating
/// the same call answers the same and changes nothing further, and naming the same key
/// twice in one call removes it once. EVERY entry held for a named key is removed, so the
/// read is left with no value for that key at all.
pub fn remove_tags(stored: &mut Vec<Tag>, keys: &[String]) {
    stored.retain(|held| !keys.iter().any(|key| key == &held.key));
}

/// How many DISTINCT keys `stored` would hold once `requested` had been merged into it
/// ([`merge_tags`]).
///
/// What [`MAX_TAGS_PER_RESOURCE`] is judged against, so overwriting the value of a key a
/// resource already carries never crosses the ceiling: it adds no tag.
pub fn merged_tag_count(stored: &[Tag], requested: &[Tag]) -> usize {
    let keys: BTreeSet<&str> = stored
        .iter()
        .chain(requested)
        .map(|tag| tag.key.as_str())
        .collect();
    keys.len()
}

/// The tag set `tags` as the `(page key, item)` pairs [`page`] enumerates: one entry per
/// distinct `Tag$Key`, in key order.
///
/// The KEY is both the page key and the identity of a tag, so the enumeration order is
/// stable across calls however the tags were written, and a pagination cursor resumes
/// after a key rather than after a position a later write would shift.
pub fn tags_in_key_order(tags: &[Tag]) -> Vec<(String, Tag)> {
    let ordered: BTreeMap<&str, &str> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    ordered
        .into_iter()
        .map(|(key, value)| {
            (
                key.to_string(),
                Tag {
                    key: key.to_string(),
                    value: value.to_string(),
                },
            )
        })
        .collect()
}

/// The `AuthenticationMethodType` an [`AuthenticationMethod`] union value carries.
///
/// `AuthenticationMethodItem$AuthenticationMethod` documents the relationship — "The
/// contents of this structure is determined by the `AuthenticationMethodType`" — so the
/// type is a PROPERTY of the union value rather than an independent member, and this
/// function is the single place the mapping lives. The model's `AuthenticationMethod`
/// union carries one variant (`Iam`) and its `AuthenticationMethodType` enum one value
/// (`IAM`), so today the mapping is total in both directions.
pub fn authentication_method_type_of(method: &AuthenticationMethod) -> &'static str {
    match method {
        AuthenticationMethod::Iam(_) => AUTHENTICATION_METHOD_TYPE_IAM,
    }
}

/// `AuthenticationMethodType::IAM` — the one value the model's `AuthenticationMethodType`
/// enum admits (`types::AUTHENTICATION_METHOD_TYPE_VALUES`).
pub const AUTHENTICATION_METHOD_TYPE_IAM: &str = "IAM";

/// The refusal a `GetApplicationAuthenticationMethod` of a type the application carries
/// no authentication method for earns, as the message of the modelled
/// `ResourceNotFoundException`.
///
/// Built from the request, like [`access_scope_not_found_message`]: it names both the
/// type and the application it was looked for on, because an unconfigured type on a known
/// application is a different failure from an unknown application.
pub fn authentication_method_not_found_message(
    application_arn: &str,
    authentication_method_type: &str,
) -> String {
    application_surface_not_found_message(
        &format!("authentication method {authentication_method_type}"),
        application_arn,
    )
}

/// The refusal a put whose `AuthenticationMethodType` disagrees with the variant its
/// `AuthenticationMethod` union carries earns, as the message of the modelled
/// `ValidationException`.
///
/// Built from the request: it names both halves, since the caller has to learn which of
/// the two to change.
pub fn authentication_method_type_mismatch_message(declared: &str, carried: &str) -> String {
    format!(
        "AuthenticationMethod carries a {carried} method, which does not match the \
         AuthenticationMethodType {declared} the request declares"
    )
}

/// The refusal an `IamAuthenticationMethod$ActorPolicy` that is not a JSON OBJECT earns,
/// as the message of the modelled `ValidationException`.
///
/// The rule is the member's own documentation — "An IAM policy document in JSON" — which
/// an `@document` member cannot express: every JSON value deserializes into it, and a
/// scalar, a list or a null is not a policy document. Nothing beyond that shape is
/// checked here: the emulator stores the document the caller sent verbatim rather than
/// judging the policy language inside it.
pub const ACTOR_POLICY_NOT_AN_OBJECT_MESSAGE: &str =
    "IamAuthenticationMethod.ActorPolicy must be a JSON object: an IAM policy document";

/// `GrantType::authorization_code` — the OAuth 2.0 Authorization Code grant
/// (`types::GRANT_TYPE_VALUES`).
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";

/// `GrantType::refresh_token` — the OAuth 2.0 Refresh Token grant.
pub const GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";

/// `GrantType::urn:ietf:params:oauth:grant-type:jwt-bearer` — the JWT Bearer Token
/// Authorization grant.
pub const GRANT_TYPE_JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// `GrantType::urn:ietf:params:oauth:grant-type:token-exchange` — the OAuth 2.0 Token
/// Exchange grant.
pub const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// The `GrantType` a [`Grant`] union value carries.
///
/// The `Grant` union documents the relationship itself — "Exactly one member of the union
/// must be specified, and must match the grant type selected" — so the type is a PROPERTY
/// of the union value rather than an independent member, and this function is the single
/// place the mapping lives. Every variant the model declares maps onto one enum value, so
/// the mapping is total in both directions.
pub fn grant_type_of(grant: &Grant) -> &'static str {
    match grant {
        Grant::AuthorizationCode(_) => GRANT_TYPE_AUTHORIZATION_CODE,
        Grant::RefreshToken(_) => GRANT_TYPE_REFRESH_TOKEN,
        Grant::JwtBearer(_) => GRANT_TYPE_JWT_BEARER,
        Grant::TokenExchange(_) => GRANT_TYPE_TOKEN_EXCHANGE,
    }
}

/// The refusal a `PutApplicationGrant` whose `GrantType` disagrees with the variant its
/// `Grant` union carries earns, as the message of the modelled `ValidationException`.
///
/// The rule is the `Grant` union's own documentation ("must match the grant type
/// selected"), which the model declares as two independent members and cannot tie
/// together. Built from the request: it names both halves, since the caller has to learn
/// which of the two to change.
pub fn grant_type_mismatch_message(declared: &str, carried: &str) -> String {
    format!(
        "Grant carries a {carried} grant, which does not match the GrantType {declared} \
         the request declares"
    )
}

/// The refusal a `Grant` that omits the member its own grant type documents as required
/// earns, as the message of the modelled `ValidationException`; `member` names the
/// missing member and `grant_type` the grant that requires it.
///
/// The rule is each member's own documentation — `AuthorizationCodeGrant$RedirectUris`
/// ("RedirectUris is required when the grant type is authorization_code") and
/// `JwtBearerGrant$AuthorizedTokenIssuers` ("AuthorizedTokenIssuers is required when the
/// grant type is JwtBearerGrant"). The model marks both members OPTIONAL, because a
/// member required only under one union variant is not expressible, so the boundary
/// validator passes the request and the handler refuses it.
pub fn grant_member_required_message(member: &str, grant_type: &str) -> String {
    format!("{member} is required when the grant type is {grant_type}.")
}

/// The refusal a `GrantType` the application carries no grant for earns, as the message
/// of the modelled `ResourceNotFoundException`.
///
/// Built from the request, like [`authentication_method_not_found_message`]: it names both
/// the grant type and the application it was looked for on, because an unconfigured grant
/// type on a known application is a different failure from an unknown application.
pub fn grant_not_found_message(application_arn: &str, grant_type: &str) -> String {
    application_surface_not_found_message(&format!("grant {grant_type}"), application_arn)
}

/// The attribute-based access control (ABAC) configuration of an instance, as the
/// `CreateInstanceAccessControlAttributeConfiguration` that enabled ABAC — and any later
/// `UpdateInstanceAccessControlAttributeConfiguration` — set it.
///
/// The attributes are held EXACTLY as the caller sent them (same order, same
/// `Source` list), because `DescribeInstanceAccessControlAttributeConfiguration` reports
/// "the list of IAM Identity Center identity store attributes that have been added to
/// your ABAC configuration" — a round trip of the caller's own mapping.
///
/// `Status` is DERIVED from [`enabled_at`](Self::enabled_at) rather than stored, so a
/// read never has to write to move the status on.
#[derive(Debug, Clone)]
pub struct AccessControlAttributeConfigurationRecord {
    /// `InstanceAccessControlAttributeConfiguration$AccessControlAttributes`, in the
    /// order the caller sent.
    pub attributes: Vec<AccessControlAttribute>,
    /// When the configuration stops being `CREATION_IN_PROGRESS` and becomes `ENABLED`:
    /// [`ABAC_ENABLE_SPAN`] after the create that enabled ABAC was accepted.
    pub enabled_at: SystemTime,
}

impl AccessControlAttributeConfigurationRecord {
    /// The configuration a create accepted at `accepted_at` writes.
    pub fn new(attributes: Vec<AccessControlAttribute>, accepted_at: SystemTime) -> Self {
        Self {
            attributes,
            enabled_at: accepted_at + ABAC_ENABLE_SPAN,
        }
    }

    /// `InstanceAccessControlAttributeConfigurationStatus` as of `now`:
    /// `CREATION_IN_PROGRESS` until the enablement is due, `ENABLED` after.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.enabled_at {
            ABAC_STATUS_CREATION_IN_PROGRESS
        } else {
            ABAC_STATUS_ENABLED
        }
    }

    /// This configuration as the modeled `InstanceAccessControlAttributeConfiguration`
    /// shape — what a describe reports, attribute for attribute.
    pub fn to_shape(&self) -> InstanceAccessControlAttributeConfiguration {
        InstanceAccessControlAttributeConfiguration {
            access_control_attributes: self.attributes.clone(),
        }
    }
}

/// The first attribute KEY that `attributes` carries twice, if any.
///
/// An ABAC configuration MAPS a key to an identity-source value
/// (`AccessControlAttribute$Key`: "The name of the attribute associated with your
/// identities in your identity source. This is used to map a specified attribute in your
/// identity source with an attribute in IAM Identity Center"), so one key cannot carry
/// two mappings: a request that sends the same key twice is refused rather than stored
/// with one of its two values silently winning. The comparison is case-sensitive, as the
/// key is an opaque identity-source attribute name.
pub fn duplicate_attribute_key(attributes: &[AccessControlAttribute]) -> Option<&str> {
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    attributes
        .iter()
        .find(|attribute| !seen.insert(attribute.key.as_str()))
        .map(|attribute| attribute.key.as_str())
}

/// The refusal a request carrying one attribute key twice earns, as the message of the
/// modelled `ValidationException` (HTTP 400). Built from the request: it names the
/// member and the key that breaks the constraint.
pub fn duplicate_attribute_key_message(key: &str) -> String {
    format!(
        "InstanceAccessControlAttributeConfiguration.AccessControlAttributes carries the \
         attribute key {key} more than once; each key maps exactly one identity source \
         attribute"
    )
}

/// The encryption configuration an instance is encrypted under.
///
/// Every instance HAS one: data at rest is encrypted from the moment the instance
/// exists, under an Amazon Web Services owned key ([`Self::aws_owned_default`]) until a
/// caller opts into a key of its own, so this is a property of the instance rather than
/// something a write brings into being. `UpdateInstance` REPLACES it; nothing removes it.
///
/// `EncryptionStatus` is DERIVED from [`effective_at`](Self::effective_at) rather than
/// stored, so a read never has to write to move the status on: the instant a change
/// becomes effective is fixed when the update is accepted, and every later read computes
/// the status the clock implies at the moment it is asked.
#[derive(Debug, Clone)]
pub struct EncryptionConfigurationRecord {
    /// `KmsKeyType`: [`KEY_TYPE_AWS_OWNED`] or [`KEY_TYPE_CUSTOMER_MANAGED`].
    pub key_type: String,
    /// The customer managed key this instance is encrypted under; `None` for an Amazon
    /// Web Services owned key, which no ARN names.
    pub kms_key_arn: Option<String>,
    /// When the configuration stops being `UPDATING` and becomes `ENABLED`:
    /// [`ENCRYPTION_UPDATE_DELAY`] after the update that set it was accepted, or `None`
    /// for a configuration that is ALREADY in effect — the Amazon Web Services owned
    /// default an instance carries from creation, which no key change is pending on.
    pub effective_at: Option<SystemTime>,
}

impl EncryptionConfigurationRecord {
    /// The configuration an `UpdateInstance` accepted at `accepted_at` writes.
    pub fn new(key_type: String, kms_key_arn: Option<String>, accepted_at: SystemTime) -> Self {
        Self {
            key_type,
            kms_key_arn,
            effective_at: Some(accepted_at + ENCRYPTION_UPDATE_DELAY),
        }
    }

    /// The configuration an instance nobody has reconfigured is encrypted under: an
    /// Amazon Web Services owned key, in effect already — so no `KmsKeyArn` names it
    /// ("Cannot be specified when KeyType is AWS_OWNED_KMS_KEY") and no key change is in
    /// flight.
    pub fn aws_owned_default() -> Self {
        Self {
            key_type: KEY_TYPE_AWS_OWNED.to_string(),
            kms_key_arn: None,
            effective_at: None,
        }
    }

    /// `KmsKeyStatus` as of `now`: `UPDATING` while a key change is not due yet,
    /// `ENABLED` once it is — and for a configuration already in effect.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.effective_at.is_some_and(|effective_at| now < effective_at) {
            KMS_KEY_STATUS_UPDATING
        } else {
            KMS_KEY_STATUS_ENABLED
        }
    }

    /// This configuration as the modeled `EncryptionConfigurationDetails` shape, as of
    /// `now`.
    ///
    /// `EncryptionStatusReason` is left absent: the model documents it as context for a
    /// FAILED update ("particularly useful when the encryption status is UPDATE_FAILED"),
    /// and nothing here fails a key change.
    pub fn to_shape(&self, now: SystemTime) -> EncryptionConfigurationDetails {
        EncryptionConfigurationDetails {
            key_type: Some(self.key_type.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            encryption_status: Some(self.status(now).to_string()),
            encryption_status_reason: None,
        }
    }
}

/// One enabled Region of an IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Region {
    pub region_name: String,
    pub status: String,
    pub added_date: DateTime,
    pub is_primary_region: bool,
    /// When the clock owes this Region record its REMOVAL — the end of the `REMOVING` a
    /// `RemoveRegion` put it into — or `None` for a Region nothing is removing. Stored as
    /// a due INSTANT and applied by [`Instance::materialize_due`], which drops the record;
    /// nothing sleeps for it.
    pub removed_due: Option<SystemTime>,
    /// When the clock owes this Region its move out of `ADDING` — the end of the extension
    /// workflow an `AddRegion` started ([`REGION_ACTIVATION_SPAN`]) — or `None` for a
    /// Region whose own enablement is not in flight. Stored as a due INSTANT and applied
    /// by [`Region::materialize_due`]; nothing sleeps for it.
    ///
    /// A Region of an instance whose WHOLE creation is still in flight carries none: its
    /// enablement finishes with the instance's own activation
    /// ([`Instance::materialize_due`]), not on a span of its own.
    pub active_due: Option<SystemTime>,
}

impl Region {
    /// The PRIMARY Region of an instance: the Region it was enabled in, in its terminal
    /// `ACTIVE` status. It is the one Region of `Regions` that carries `IsPrimaryRegion`,
    /// and `RemoveRegion` refuses to remove it ("The primary Region cannot be removed.").
    pub fn primary(region_name: &str, added_date: DateTime) -> Self {
        Self {
            is_primary_region: true,
            ..Self::active(region_name, added_date)
        }
    }

    /// An additional Region of an instance, in its terminal `ACTIVE` status: a Region
    /// whose extension workflow has finished. NOT the primary one — the primary Region is
    /// the one the instance was enabled in, which [`Instance::active`] holds.
    pub fn active(region_name: &str, added_date: DateTime) -> Self {
        Self {
            region_name: region_name.to_string(),
            status: STATUS_ACTIVE.to_string(),
            added_date,
            is_primary_region: false,
            removed_due: None,
            active_due: None,
        }
    }

    /// An ADDITIONAL Region whose extension workflow has just STARTED at `now`: the record
    /// `AddRegion` appends, `ADDING` until its activation falls due
    /// ([`REGION_ACTIVATION_SPAN`]) and `ACTIVE` from then on — "The Region status is set to
    /// ADDING at first and changes to ACTIVE when the workflow completes."
    ///
    /// `AddedDate` is the instant the Region was added to the instance, which is this one:
    /// `DescribeRegion$AddedDate` is "The timestamp when the Region was added to the IAM
    /// Identity Center instance", not the instant its workflow finished.
    pub fn adding(region_name: &str, now: SystemTime) -> Self {
        Self {
            status: STATUS_ADDING.to_string(),
            active_due: Some(now + REGION_ACTIVATION_SPAN),
            ..Self::active(region_name, DateTime::from(now))
        }
    }

    /// Start this Region's ENABLEMENT at this record: `RegionStatus::ADDING`, the value
    /// `RegionMetadata$Status` documents for a Region whose "extension workflow is in
    /// progress", which is what a Region carries until that workflow finishes and leaves
    /// it `ACTIVE`.
    ///
    /// `RegionStatus` (`ACTIVE | ADDING | REMOVING`) is a DIFFERENT enum from the
    /// instance-level `InstanceStatus` (`CREATE_IN_PROGRESS | CREATE_FAILED |
    /// DELETE_IN_PROGRESS | ACTIVE`), so a Region record never carries an instance-level
    /// value however the enablement was started: this is the one in-flight value it has.
    pub fn start_addition(&mut self) {
        self.status = STATUS_ADDING.to_string();
    }

    /// Whether an add or remove Region workflow is in progress on this Region: the two
    /// non-terminal values of the model's `RegionStatus` enum (`ADDING` — "Region extension
    /// workflow is in progress" — and `REMOVING` — "Region removal workflow is in
    /// progress").
    pub fn workflow_in_progress(&self) -> bool {
        self.status == STATUS_ADDING || self.status == STATUS_REMOVING
    }

    /// Start this Region's removal at `now`: `REMOVING` until the record's deletion falls
    /// due, [`REGION_REMOVAL_SPAN`] later.
    ///
    /// Idempotent on a Region already being removed — the due instant the first
    /// `RemoveRegion` fixed is kept, so a repeated remove cannot push the deletion out.
    pub fn start_removal(&mut self, now: SystemTime) {
        if self.removed_due.is_some() {
            return;
        }
        // The Region is on its way OUT, so no activation is owed it any more.
        self.active_due = None;
        self.status = STATUS_REMOVING.to_string();
        self.removed_due = Some(now + REGION_REMOVAL_SPAN);
    }

    /// Whether this Region's own ENABLEMENT is still in flight: an `AddRegion` started its
    /// extension workflow and the clock has not made its completion due yet.
    pub fn addition_in_flight(&self, now: SystemTime) -> bool {
        self.active_due.is_some_and(|due| due > now)
    }

    /// Apply what the CLOCK already owes this Region: a Region whose extension workflow has
    /// run out is `ACTIVE` — "The Region status is set to ADDING at first and changes to
    /// ACTIVE when the workflow completes" — whether or not anybody called. Idempotent, and
    /// it changes nothing before that instant falls due.
    ///
    /// A Region a `RemoveRegion` has since started removing keeps its `REMOVING`: the
    /// record is on its way out, and [`Self::start_removal`] has already dropped the
    /// activation it was owed.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.removed_due.is_some() {
            return;
        }
        if self.active_due.is_some_and(|due| due <= now) {
            self.active_due = None;
            self.status = STATUS_ACTIVE.to_string();
        }
    }

    /// Whether the clock already owes this Region record its DELETION.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removed_due.is_some_and(|due| due <= now)
    }

    /// This Region as the modeled `RegionMetadata` shape.
    pub fn to_shape(&self) -> RegionMetadata {
        RegionMetadata {
            region_name: Some(self.region_name.clone()),
            status: Some(self.status.clone()),
            added_date: Some(self.added_date),
            is_primary_region: self.is_primary_region,
        }
    }
}

/// An IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Instance {
    pub instance_arn: String,
    pub identity_store_id: String,
    pub identity_store_arn: String,
    pub owner_account_id: String,
    /// `InstanceMetadata$Name` — optional in the model, so an instance created without
    /// one carries none rather than a name the service invented.
    pub name: Option<String>,
    pub created_date: DateTime,
    pub status: String,
    pub primary_region: String,
    pub regions: Vec<Region>,
    /// Account instance or organization instance: what the instance's INITIAL
    /// permission-set enablement comes from (see
    /// [`InstanceType::permission_sets_enabled`]).
    pub instance_type: InstanceType,
    /// `DescribeInstance$PermissionSetsEnabled` — "Indicates whether permission sets are
    /// enabled for this Identity Center instance", and what
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn`]
    /// asks. It starts at what [`InstanceType::permission_sets_enabled`] gives this kind
    /// of instance and `UpdateInstance` can turn it ON — "Enables permission sets for
    /// this Identity Center instance. The only accepted value is true. After permission
    /// sets are enabled, they cannot be disabled" — so it is held per instance rather
    /// than decided per read. `InstanceMetadata` (what `ListInstances` projects) carries
    /// no such member, so only `DescribeInstance` reports it.
    pub permission_sets_enabled: bool,
    /// The encryption configuration this instance's data at rest is encrypted under.
    /// Never absent: an instance is encrypted from the moment it exists, under the
    /// Amazon Web Services owned key default
    /// ([`EncryptionConfigurationRecord::aws_owned_default`]), and `UpdateInstance`
    /// REPLACES that configuration rather than bringing one into being.
    pub encryption: EncryptionConfigurationRecord,
    /// The instance's ABAC configuration, or `None` while ABAC is not enabled on it:
    /// `CreateInstanceAccessControlAttributeConfiguration` is what enables it,
    /// `UpdateInstanceAccessControlAttributeConfiguration` replaces the attributes it
    /// holds and `DeleteInstanceAccessControlAttributeConfiguration` disables it again,
    /// so an instance nobody has enabled ABAC on holds no attribute mapping rather than
    /// an invented one.
    pub access_control_attributes: Option<AccessControlAttributeConfigurationRecord>,
    /// The `ClientToken` of the `CreateInstance` call that minted this instance, when
    /// it sent one. The model marks that member `@idempotencyToken`, so a retry of the
    /// same call must answer with this instance rather than refusing it as a duplicate.
    pub client_token: Option<String>,
    /// The tags `CreateInstance` attached to the instance.
    pub tags: Vec<Tag>,
    /// The [`instance_fingerprint`] of the `CreateInstance` request that minted this
    /// instance, which is what tells a genuine retry of that call from the same
    /// `ClientToken` carrying a DIFFERENT request. It fingerprints the request as it
    /// ARRIVED rather than what the instance holds now, so a `TagResource` or
    /// `UntagResource` in between cannot turn a true retry into a conflict.
    ///
    /// Empty for an instance no `CreateInstance` call minted: such an instance carries no
    /// `ClientToken` either, so no idempotency check ever reaches its fingerprint.
    pub request_fingerprint: String,
    /// When the clock owes this instance its move out of `CREATE_IN_PROGRESS`, or `None`
    /// for an instance that is already in a terminal status. Stored as a due INSTANT and
    /// applied by [`Instance::materialize_due`]; nothing sleeps for it.
    pub active_due: Option<SystemTime>,
    /// When the clock owes this instance its REMOVAL — the end of the
    /// `DELETE_IN_PROGRESS` the `DeleteInstance` that started it put the instance in — or
    /// `None` for an instance nothing is deleting. Stored as a due INSTANT and applied by
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`], which drops the
    /// record; nothing sleeps for it.
    pub deleted_due: Option<SystemTime>,
}

impl Instance {
    /// An instance in its terminal `ACTIVE` status, owned by the emulator's account and
    /// enabled in `region` alone — what an instance looks like once its creation has
    /// finished.
    ///
    /// `region` is the Region the instance was ENABLED in (for a create, the Region of
    /// the request that created it — [`request_region`]), which is what
    /// `PrimaryRegion` reports and the one Region of `Regions` that carries
    /// `IsPrimaryRegion`.
    pub fn active(
        instance_id: &str,
        identity_store_id: &str,
        instance_type: InstanceType,
        name: Option<String>,
        created_date: DateTime,
        region: &str,
    ) -> Self {
        Self {
            instance_arn: instance_arn(instance_id),
            identity_store_id: identity_store_id.to_string(),
            identity_store_arn: identity_store_arn(identity_store_id),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            name,
            created_date,
            status: STATUS_ACTIVE.to_string(),
            primary_region: region.to_string(),
            regions: vec![Region::primary(region, created_date)],
            instance_type,
            // What this KIND of instance starts with; `UpdateInstance` can enable
            // permission sets on an instance that starts without them.
            permission_sets_enabled: instance_type.permission_sets_enabled(),
            // An instance is encrypted at rest from the moment it exists, and
            // `CreateInstance` takes no encryption configuration to say otherwise, so it
            // starts under the Amazon Web Services owned key default; `UpdateInstance`
            // is what opts it into a customer managed key.
            encryption: EncryptionConfigurationRecord::aws_owned_default(),
            // ABAC is off until a caller enables it: `CreateInstance` takes no attribute
            // configuration, so there is none to report until a
            // `CreateInstanceAccessControlAttributeConfiguration` writes one.
            access_control_attributes: None,
            client_token: None,
            tags: Vec::new(),
            request_fingerprint: String::new(),
            active_due: None,
            deleted_due: None,
        }
    }

    /// A just-created instance: `CREATE_IN_PROGRESS`, enabled in `region`, with its
    /// activation owed [`INSTANCE_ACTIVATION_SPAN`] after `now`.
    pub fn creating(
        instance_id: &str,
        identity_store_id: &str,
        instance_type: InstanceType,
        name: Option<String>,
        now: SystemTime,
        region: &str,
    ) -> Self {
        let mut instance = Self::active(
            instance_id,
            identity_store_id,
            instance_type,
            name,
            DateTime::from(now),
            region,
        );
        instance.status = STATUS_CREATE_IN_PROGRESS.to_string();
        // The INSTANCE is `CREATE_IN_PROGRESS` (an `InstanceStatus`), while each Region's
        // own enablement is still in flight, which `RegionStatus` spells `ADDING`: the two
        // members are typed by different enums, and only the instance-level one admits
        // `CREATE_IN_PROGRESS`.
        for region in &mut instance.regions {
            region.start_addition();
        }
        instance.active_due = Some(now + INSTANCE_ACTIVATION_SPAN);
        instance
    }

    /// Start this instance's deletion at `now`: `DELETE_IN_PROGRESS` until the removal
    /// falls due, [`INSTANCE_DELETION_SPAN`] later.
    ///
    /// Idempotent on an instance already being deleted — the due instant the first
    /// `DeleteInstance` fixed is kept, so a repeated delete cannot push the removal out.
    pub fn start_deletion(&mut self, now: SystemTime) {
        if self.deleted_due.is_some() {
            return;
        }
        // The creation is no longer owed: the instance is on its way out, not in. The
        // Regions keep the status they had — `RegionStatus` has no deleting value, and
        // the instance's own `Status` is what says it is going away.
        self.active_due = None;
        self.status = STATUS_DELETE_IN_PROGRESS.to_string();
        self.deleted_due = Some(now + INSTANCE_DELETION_SPAN);
    }

    /// Whether the clock already owes this instance its REMOVAL from the store.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.deleted_due.is_some_and(|due| due <= now)
    }

    /// Whether this instance's CREATION has not finished yet — the window
    /// `DescribeInstance` documents as serving only the instance's own describe and
    /// delete.
    ///
    /// Asked of the STATUS the record carries, which
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::on_request`] has already brought up
    /// to this instant, so it answers exactly what a read at the same instant reports: an
    /// instance whose activation has fallen due is `ACTIVE` and no longer in this window.
    pub fn creation_in_progress(&self) -> bool {
        self.status == STATUS_CREATE_IN_PROGRESS
    }

    /// Whether this is an ACCOUNT instance — the instance of a single standalone account,
    /// which is what `CreateInstance` creates and what the account's ambient instance is
    /// ([`SEED_INSTANCE_TYPE`]).
    ///
    /// Asked of the instance's KIND and not of its permission-set enablement: the kind is
    /// fixed when the instance is enabled and nothing in this API changes it, while
    /// `UpdateInstance` turns permission sets on per instance
    /// ([`Instance::permission_sets_enabled`]), which is a different question and the one
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::permission_set_surface_withdrawn`]
    /// asks.
    pub fn is_account_instance(&self) -> bool {
        matches!(self.instance_type, InstanceType::Account)
    }

    /// The refusal a REGION WRITE (`AddRegion`, `RemoveRegion`) earns on this instance
    /// before anything about the Region it names is judged, or `None` for an instance such
    /// a write can reach at all.
    ///
    /// Replicating an instance across Regions is a feature of the ORGANIZATION instance,
    /// so on an ACCOUNT instance no Region can be added and none removed — which is why the
    /// refusal does not depend on whether the Region the request names is already enabled,
    /// is the primary one, or exists at all: the guard runs BEFORE every one of those
    /// checks. It carries [`ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`], the same sentence the
    /// permission-set surface answers for the same restriction, as the modelled
    /// `ValidationException` (HTTP 400).
    ///
    /// The REPORTING side of the Region surface (`DescribeRegion`, `ListRegions`) is served
    /// on an account instance: an account instance is enabled in its own primary Region and
    /// reports it, so only the WRITES are withdrawn.
    ///
    /// It needs a RESOLVED instance, so it is asked after the instance lookup and after the
    /// `CREATE_IN_PROGRESS` window (which withdraws every operation but the instance's own
    /// describe and delete, and is therefore the broader withdrawal of the two), and before
    /// anything about the Region the request named — the order [`mutates_system_tag`]
    /// states once for every pre-mutation refusal of this service.
    pub fn region_write_refusal(&self) -> Option<&'static str> {
        self.is_account_instance()
            .then_some(ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE)
    }

    /// Enable this instance in one MORE Region, or refuse the add when the instance
    /// already holds [`MAX_REGIONS_PER_INSTANCE`] of them.
    ///
    /// The one place a Region record is appended to an instance, so the emulator's memory
    /// ceiling cannot be bypassed by whatever enables a Region. The `Err` carries the
    /// message the refusal reports ([`region_ceiling_message`]), which a caller sees as
    /// the modelled `ServiceQuotaExceededException`.
    pub fn add_region(&mut self, region: Region) -> Result<(), String> {
        if self.regions.len() >= MAX_REGIONS_PER_INSTANCE {
            return Err(region_ceiling_message(&self.instance_arn));
        }
        self.regions.push(region);
        Ok(())
    }

    /// Apply what the CLOCK already owes this instance: an instance whose activation
    /// instant has passed is `ACTIVE`, in every Region it holds, and a Region whose own
    /// removal instant has passed ([`Region::start_removal`]) is no longer one of its
    /// Regions. Idempotent, and it changes nothing before an instant falls due, so a read
    /// that calls it answers exactly what a read at the same instant would have answered
    /// without it.
    ///
    /// The REMOVAL a delete owes the INSTANCE is applied by the store rather than here,
    /// since it drops the whole record: see
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`].
    pub fn materialize_due(&mut self, now: SystemTime) {
        // A Region whose removal workflow has run out no longer exists, whether or not
        // anybody called: `RemoveRegion` is documented as deleting the Region record "when
        // the workflow completes".
        self.regions.retain(|region| !region.removal_due(now));
        // And a Region whose own EXTENSION workflow has run out is `ACTIVE`, on the span
        // the `AddRegion` that started it fixed — independently of the instance's own
        // lifecycle, which that add did not touch.
        for region in &mut self.regions {
            region.materialize_due(now);
        }
        if self.deleted_due.is_some() {
            return;
        }
        if self.active_due.is_some_and(|due| due <= now) {
            self.active_due = None;
            self.status = STATUS_ACTIVE.to_string();
            for region in &mut self.regions {
                // A Region on its way OUT keeps its `REMOVING`: the instance's activation
                // says nothing about a removal workflow another call started.
                if region.removed_due.is_none() {
                    region.active_due = None;
                    region.status = STATUS_ACTIVE.to_string();
                }
            }
        }
    }

    /// The Region of this instance whose name is `region_name`, or `None` when the
    /// instance is not enabled in it.
    pub fn region(&self, region_name: &str) -> Option<&Region> {
        self.regions
            .iter()
            .find(|region| region.region_name == region_name)
    }

    /// The same Region, for a handler that updates it.
    pub fn region_mut(&mut self, region_name: &str) -> Option<&mut Region> {
        self.regions
            .iter_mut()
            .find(|region| region.region_name == region_name)
    }

    /// This instance as the `InstanceMetadata` shape, `State` included.
    ///
    /// `StatusReason` is left absent: the model documents it as additional context for
    /// an instance in a non-ACTIVE state, and there is none to give — a
    /// `CREATE_IN_PROGRESS` instance is on its way to ACTIVE, which the status already
    /// says.
    ///
    /// `State` is the instance's status again, as a structure: see [`Instance::state`].
    pub fn to_metadata(&self) -> InstanceMetadata {
        InstanceMetadata {
            instance_arn: Some(self.instance_arn.clone()),
            identity_store_id: Some(self.identity_store_id.clone()),
            identity_store_arn: Some(self.identity_store_arn.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            name: self.name.clone(),
            created_date: Some(self.created_date),
            status: Some(self.status.clone()),
            status_reason: None,
            primary_region: Some(self.primary_region.clone()),
            regions: Some(self.regions.iter().map(Region::to_shape).collect()),
            state: Some(self.state()),
        }
    }

    /// This instance's `State` view: the structure served beside its `Status`, whose
    /// `Name` is that same status value.
    ///
    /// Read from the one field the instance's status lives in, so the two views of the
    /// lifecycle cannot disagree — whatever the status is at the instant of the read
    /// (`CREATE_IN_PROGRESS` while the creation is in flight, `ACTIVE` once the
    /// activation has fallen due, `DELETE_IN_PROGRESS` while a deletion is), `State.Name`
    /// is it.
    pub fn state(&self) -> InstanceState {
        InstanceState {
            name: Some(self.status.clone()),
        }
    }
}

/// The `Path` a customer-managed policy reference carries when the request names none.
///
/// `CustomerManagedPolicyReference$Path`'s own documentation: "The path to the IAM policy
/// that you have configured in each account where you want to deploy your permission set.
/// The default is `/`." So the stored reference is normalised to it at write time and
/// every later read reports the same path, whether or not the writer sent one.
pub const DEFAULT_MANAGED_POLICY_PATH: &str = "/";

/// One customer managed policy reference ATTACHED to a permission set.
///
/// `CustomerManagedPolicyReference` is a (`Name`, `Path`) pair naming an IAM policy that
/// the caller has deployed in each member account, so the reference's IDENTITY is that
/// pair — the same name under a different path is a different policy. The stored `Path` is
/// always explicit: a reference attached without one is normalised to
/// [`DEFAULT_MANAGED_POLICY_PATH`], the value the member documents as its default, so
/// every later read and every identity comparison sees one form of the same reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerManagedPolicyReferenceRecord {
    /// `CustomerManagedPolicyReference$Name` (`ManagedPolicyName`, 1..128).
    pub name: String,
    /// `CustomerManagedPolicyReference$Path` (`ManagedPolicyPath`, 1..512), materialised.
    pub path: String,
}

impl CustomerManagedPolicyReferenceRecord {
    /// The record a modeled `CustomerManagedPolicyReference` describes, with its `Path`
    /// materialised to the documented default when the request named none.
    pub fn from_shape(reference: &CustomerManagedPolicyReference) -> Self {
        Self {
            name: reference.name.clone(),
            path: reference
                .path
                .clone()
                .unwrap_or_else(|| DEFAULT_MANAGED_POLICY_PATH.to_string()),
        }
    }

    /// This reference's IDENTITY, and the key an enumeration of a permission set's
    /// references orders and pages by.
    ///
    /// `|` separates the two parts unambiguously: it is in neither member's character
    /// class (`ManagedPolicyName` is `^[\w+=,.@-]+$` and `ManagedPolicyPath` is
    /// `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`), so no two distinct pairs share a key.
    pub fn identity(&self) -> String {
        format!("{}|{}", self.name, self.path)
    }

    /// This record as the modeled `CustomerManagedPolicyReference` shape — what
    /// `ListCustomerManagedPolicyReferencesInPermissionSet` reports, so a read answers
    /// with the pair the attach stored and the `Path` it stored for it.
    pub fn to_shape(&self) -> CustomerManagedPolicyReference {
        CustomerManagedPolicyReference {
            name: self.name.clone(),
            path: Some(self.path.clone()),
        }
    }
}

/// The greatest number of customer managed policy references this emulator holds on ONE
/// permission set.
///
/// NOT a figure the model carries: it is the emulator's own ceiling, the per-permission-set
/// counterpart of [`MAX_RECORDS_PER_STORE`] and of [`MAX_SCOPES_PER_APPLICATION`]. An
/// attach inserts one entry per distinct (`Name`, `Path`) pair, the two members' patterns
/// admit unboundedly many of them, and an attached reference lives until a caller detaches
/// it — so without a ceiling a caller could grow the process until the host runs out of
/// memory. An attach that would add a reference beyond it is refused with the modelled
/// `ServiceQuotaExceededException`, which AWS documents as "the principal has crossed the
/// permitted number of resources that can be created".
pub const MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET: usize = 20;

/// The message an attach refuses a new reference with once a permission set already holds
/// [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`] of them; built from the request,
/// which names the permission set the ceiling was reached on.
pub fn customer_managed_policy_ceiling_message(permission_set_arn: &str) -> String {
    format!(
        "Cannot attach another customer managed policy reference: this emulator holds at \
         most {MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET} customer managed policy \
         references per permission set, and {permission_set_arn} already holds that many"
    )
}

/// The refusal an ATTACH of a reference the permission set ALREADY holds earns, as the
/// message of the modelled `ConflictException` (HTTP 409) — "a conflict with a previous
/// successful write".
///
/// Built from the request: it names the pair that is already attached and the permission
/// set it is attached to, because a client has to tell which of the request's resources
/// the conflict is about.
pub fn customer_managed_policy_already_attached_message(
    permission_set_arn: &str,
    name: &str,
    path: &str,
) -> String {
    format!(
        "A customer managed policy reference with name {name} and path {path} is already \
         attached to permission set {permission_set_arn}."
    )
}

/// The refusal a detach of a reference the permission set does not hold earns, as the
/// message of the modelled `ResourceNotFoundException` (HTTP 404).
///
/// Built from the request: it names the pair that resolved to nothing and the permission
/// set it was looked for on, because an unattached reference on a known permission set is
/// a different failure from an unknown permission set.
pub fn customer_managed_policy_not_attached_message(
    permission_set_arn: &str,
    name: &str,
    path: &str,
) -> String {
    format!(
        "Could not find customer managed policy reference with name {name} and path \
         {path} attached to permission set {permission_set_arn}."
    )
}

/// Cursor scope of a `ListCustomerManagedPolicyReferencesInPermissionSet` enumeration:
/// the operation and the permission set whose references it walks (the instance included,
/// since the same permission-set ARN under another instance is another resource).
pub fn cursor_customer_managed_policies(instance_arn: &str, permission_set_arn: &str) -> String {
    format!("customerManagedPolicies|{instance_arn}|{permission_set_arn}")
}

/// The greatest number of AWS MANAGED POLICY ARNs this emulator holds on ONE permission
/// set.
///
/// NOT a figure the model carries: it is the emulator's own ceiling, the AWS-managed
/// counterpart of [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`]. An attach inserts
/// one entry per distinct `ManagedPolicyArn`, the member's pattern admits unboundedly many
/// of them, and an attached policy lives until a caller detaches it — so without a ceiling
/// a caller could grow the process until the host runs out of memory. An attach that would
/// add a policy beyond it is refused with the modelled `ServiceQuotaExceededException`,
/// which AWS documents as "the principal has crossed the permitted number of resources
/// that can be created".
pub const MAX_MANAGED_POLICIES_PER_PERMISSION_SET: usize = 20;

/// The message an attach refuses a new managed policy with once a permission set already
/// holds [`MAX_MANAGED_POLICIES_PER_PERMISSION_SET`] of them; built from the request,
/// which names the permission set the ceiling was reached on.
pub fn managed_policy_ceiling_message(permission_set_arn: &str) -> String {
    format!(
        "Cannot attach another managed policy: this emulator holds at most \
         {MAX_MANAGED_POLICIES_PER_PERMISSION_SET} managed policies per permission set, \
         and {permission_set_arn} already holds that many"
    )
}

/// The refusal an ATTACH of an AWS managed policy the permission set ALREADY carries
/// earns, as the message of the modelled `ConflictException` (HTTP 409) — "a conflict
/// with a previous successful write".
///
/// Built from the request, like
/// [`customer_managed_policy_already_attached_message`]: it names the ARN that is already
/// attached and the permission set it is attached to.
pub fn managed_policy_already_attached_message(
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> String {
    format!(
        "Managed policy with arn {managed_policy_arn} is already attached to permission \
         set {permission_set_arn}."
    )
}

/// The refusal a detach of an AWS managed policy the permission set does not hold earns,
/// as the message of the modelled `ResourceNotFoundException`.
///
/// Built from the request: it names the ARN that resolved to nothing and the permission set
/// it was looked for on, because an unattached policy on a known permission set is a
/// different failure from an unknown permission set.
pub fn managed_policy_not_attached_message(
    permission_set_arn: &str,
    managed_policy_arn: &str,
) -> String {
    format!(
        "Could not find managed policy with arn {managed_policy_arn} attached to \
         permission set {permission_set_arn}."
    )
}

/// The permissions boundary attached to a permission set: EXACTLY ONE of an AWS managed
/// policy ARN or a customer managed policy reference.
///
/// Modeled as a structure with two optional members, but `PermissionsBoundary`'s own
/// documentation makes it a choice — "Specify either `CustomerManagedPolicyReference` to
/// use the name and path of a customer managed policy, or `ManagedPolicyArn` to use the
/// ARN of an Amazon Web Services managed policy" — so the state holds the CHOICE rather
/// than the pair, and a request that names both or neither never reaches the store
/// ([`PermissionsBoundaryRecord::from_shape`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsBoundaryRecord {
    /// `PermissionsBoundary$ManagedPolicyArn`.
    ManagedPolicy { managed_policy_arn: String },
    /// `PermissionsBoundary$CustomerManagedPolicyReference`, with `Path` normalised to
    /// [`DEFAULT_MANAGED_POLICY_PATH`] when the request named none.
    CustomerManagedPolicy { name: String, path: String },
}

impl PermissionsBoundaryRecord {
    /// The record a modeled `PermissionsBoundary` describes, or the way it broke the
    /// documented choice when it names BOTH policy kinds or NEITHER — the one thing the
    /// model's two optional members cannot express, and which the shape's documented
    /// "specify either … or …" forbids.
    ///
    /// The two malformed forms are kept APART rather than collapsed into one "bad
    /// boundary", because the member a caller has to change differs between them
    /// ([`PermissionsBoundaryChoiceViolation`]).
    pub fn from_shape(
        boundary: &PermissionsBoundary,
    ) -> Result<Self, PermissionsBoundaryChoiceViolation> {
        match (
            &boundary.customer_managed_policy_reference,
            &boundary.managed_policy_arn,
        ) {
            (Some(reference), None) => {
                // The `Path` default is materialised in exactly ONE place —
                // [`CustomerManagedPolicyReferenceRecord::from_shape`] — so a boundary's
                // reference and an attached reference normalise identically.
                let reference = CustomerManagedPolicyReferenceRecord::from_shape(reference);
                Ok(Self::CustomerManagedPolicy {
                    name: reference.name,
                    path: reference.path,
                })
            }
            (None, Some(managed_policy_arn)) => Ok(Self::ManagedPolicy {
                managed_policy_arn: managed_policy_arn.clone(),
            }),
            (Some(_), Some(_)) => Err(PermissionsBoundaryChoiceViolation::BothKinds),
            (None, None) => Err(PermissionsBoundaryChoiceViolation::NeitherKind),
        }
    }

    /// This record as the modeled `PermissionsBoundary` shape — what
    /// `GetPermissionsBoundaryForPermissionSet` answers with, so a read reports the
    /// choice the write stored and nothing else.
    pub fn to_shape(&self) -> PermissionsBoundary {
        match self {
            Self::ManagedPolicy { managed_policy_arn } => PermissionsBoundary {
                customer_managed_policy_reference: None,
                managed_policy_arn: Some(managed_policy_arn.clone()),
            },
            Self::CustomerManagedPolicy { name, path } => PermissionsBoundary {
                // Projected by the same record that projects an attached reference, so
                // the two cannot disagree about the shape they report.
                customer_managed_policy_reference: Some(
                    CustomerManagedPolicyReferenceRecord {
                        name: name.clone(),
                        path: path.clone(),
                    }
                    .to_shape(),
                ),
                managed_policy_arn: None,
            },
        }
    }
}

/// The two halves of the choice `PermissionsBoundary` documents, spelled as the wire
/// spells them (`PermissionsBoundary$CustomerManagedPolicyReference` and
/// `PermissionsBoundary$ManagedPolicyArn`), so the refusal below names the members a
/// caller sent rather than a re-spelling of them.
const PERMISSIONS_BOUNDARY_MEMBERS: (&str, &str) =
    ("CustomerManagedPolicyReference", "ManagedPolicyArn");

/// How a `PermissionsBoundary` broke the choice its own documentation states — the only
/// two ways a request can, since the structure carries exactly these two optional
/// members.
///
/// They are distinct because the FIX differs: a request that named both kinds has to drop
/// one, and a request that named neither has to add one. Telling the second it must "not"
/// name both would be a false statement about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionsBoundaryChoiceViolation {
    /// Both `CustomerManagedPolicyReference` and `ManagedPolicyArn` were named.
    BothKinds,
    /// Neither was named.
    NeitherKind,
}

impl PermissionsBoundaryChoiceViolation {
    /// The refusal this violation earns, as the message of the modelled
    /// `ValidationException` (HTTP 400).
    ///
    /// BUILT from the members the request got wrong, which is what the shape's documented
    /// choice ("Specify either `CustomerManagedPolicyReference` … or `ManagedPolicyArn`")
    /// is about; the caller has to fix the request from this text alone, so only the form
    /// it actually sent is described.
    pub fn message(self) -> String {
        let (customer_managed, managed_policy) = PERMISSIONS_BOUNDARY_MEMBERS;
        match self {
            Self::BothKinds => format!(
                "Specify either {customer_managed} or {managed_policy} in \
                 PermissionsBoundary, but not both."
            ),
            Self::NeitherKind => format!(
                "Specify either {customer_managed} or {managed_policy} in \
                 PermissionsBoundary."
            ),
        }
    }
}

/// A permission set, keyed in [`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`]
/// by the ARN minted for it at creation.
///
/// `managed_policies`, `inline_policy` and `permissions_boundary` are held here so the
/// policy-attachment operations have one record to extend rather than a second map.
#[derive(Debug, Clone)]
pub struct PermissionSetRecord {
    pub permission_set_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub created_date: DateTime,
    pub session_duration: Option<String>,
    pub relay_state: Option<String>,
    pub tags: Vec<Tag>,
    /// The AWS MANAGED POLICY ARNs attached to this permission set.
    ///
    /// An attachment's IDENTITY is its `ManagedPolicyArn` — a permission set holds a given
    /// AWS managed policy at most once, so an attach of an ARN the set already carries is
    /// refused and a detach removes the one entry its ARN names. They live ON the
    /// permission-set record rather than in a second map, for the same reason as
    /// [`Self::customer_managed_policies`]: a permission set that no longer exists holds
    /// none of them, and the attach, detach and list operations all resolve the same
    /// permission set every other read resolves. Holds at most
    /// [`MAX_MANAGED_POLICIES_PER_PERMISSION_SET`] ARNs; an attach that would add one
    /// beyond that ceiling is refused rather than grown into.
    pub managed_policies: Vec<String>,
    /// The CUSTOMER MANAGED POLICY REFERENCES attached to this permission set, by
    /// [`CustomerManagedPolicyReferenceRecord::identity`].
    ///
    /// They live ON the permission-set record rather than in a second map, so a
    /// permission set that no longer exists holds none of them and the attach, detach and
    /// list operations all resolve the same permission set every other read resolves. A
    /// `BTreeMap` so an enumeration walks one stable order and can page by that identity,
    /// and so one identity names at most one stored reference — an attach of a reference
    /// already attached is refused before the insert. Holds at most
    /// [`MAX_CUSTOMER_MANAGED_POLICIES_PER_PERMISSION_SET`] references; an attach that
    /// would add one beyond that ceiling is refused rather than grown into.
    pub customer_managed_policies: BTreeMap<String, CustomerManagedPolicyReferenceRecord>,
    pub inline_policy: Option<String>,
    /// The permissions boundary `PutPermissionsBoundaryToPermissionSet` attached, if one
    /// is attached: a permission set carries AT MOST ONE, so a later put REPLACES it.
    pub permissions_boundary: Option<PermissionsBoundaryRecord>,
}

/// The name of the AWS managed policy `managed_policy_arn` names, as
/// `AttachedManagedPolicy$Name` ("The name of the Amazon Web Services managed policy")
/// reports it.
///
/// DERIVED from the ARN rather than stored beside it: `ManagedPolicyArn`'s pattern
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`)
/// makes the final `/`-separated segment the policy's name and everything before it the
/// path, so the name of an attached policy is a function of the ARN the caller attached
/// and the two can never disagree. An ARN with no `/` at all cannot match that pattern,
/// so the whole string is the only name such a value could have.
pub fn managed_policy_name(managed_policy_arn: &str) -> String {
    match managed_policy_arn.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name.to_string(),
        _ => managed_policy_arn.to_string(),
    }
}

/// One attached AWS managed policy as the modeled `AttachedManagedPolicy` shape — what
/// `ListManagedPoliciesInPermissionSet` enumerates.
pub fn attached_managed_policy(managed_policy_arn: &str) -> AttachedManagedPolicy {
    AttachedManagedPolicy {
        name: Some(managed_policy_name(managed_policy_arn)),
        arn: Some(managed_policy_arn.to_string()),
    }
}

impl PermissionSetRecord {
    /// The AWS managed policies attached to this permission set, in ARN order, as the
    /// `(page key, item)` pairs [`page`] enumerates.
    ///
    /// The ARN is both the page key and the identity of an attachment — a permission set
    /// holds a given managed policy at most once — so the enumeration order is stable
    /// across calls however the attachments were made, and a pagination cursor resumes
    /// after a KEY rather than after a position a later attach would shift.
    pub fn attached_managed_policies(&self) -> Vec<(String, AttachedManagedPolicy)> {
        let mut arns: Vec<String> = self.managed_policies.clone();
        arns.sort();
        arns.dedup();
        arns.into_iter()
            .map(|arn| {
                let policy = attached_managed_policy(&arn);
                (arn, policy)
            })
            .collect()
    }

    /// A fingerprint of the permission set's DEFINITION as it stands now: everything a
    /// provisioning of it copies into an account.
    ///
    /// Two records whose access this permission set grants is identical share a revision,
    /// and any change to that access — its name, its description, its session duration,
    /// its relay state, an attached AWS managed policy, an attached customer managed
    /// policy reference, its inline policy, its permissions boundary — produces a
    /// different one. `tags` are NOT part of it: they label the permission set itself and
    /// change nothing about the access an account's copy of it grants.
    ///
    /// It is DERIVED from the stored fields on every call rather than stamped on a
    /// change, so it cannot drift from the definition it describes however the record was
    /// reached, and the operations that change a permission set need no bookkeeping of
    /// their own. Each list is preceded by its own section marker and its length, and
    /// [`request_fingerprint`] length-prefixes every part, so two different definitions
    /// cannot render alike.
    pub fn definition_revision(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.name.clone(),
            self.description.clone().unwrap_or_default(),
            self.session_duration.clone().unwrap_or_default(),
            self.relay_state.clone().unwrap_or_default(),
        ];
        let mut managed_policies = self.managed_policies.clone();
        managed_policies.sort();
        managed_policies.dedup();
        parts.push("managedPolicies".to_string());
        parts.push(managed_policies.len().to_string());
        parts.extend(managed_policies);
        parts.push("customerManagedPolicies".to_string());
        parts.push(self.customer_managed_policies.len().to_string());
        parts.extend(self.customer_managed_policies.keys().cloned());
        parts.push("inlinePolicy".to_string());
        parts.push(self.inline_policy.clone().unwrap_or_default());
        match &self.permissions_boundary {
            None => parts.push("noBoundary".to_string()),
            Some(PermissionsBoundaryRecord::ManagedPolicy { managed_policy_arn }) => {
                parts.push("boundary/managed".to_string());
                parts.push(managed_policy_arn.clone());
            }
            Some(PermissionsBoundaryRecord::CustomerManagedPolicy { name, path }) => {
                parts.push("boundary/customer".to_string());
                parts.push(name.clone());
                parts.push(path.clone());
            }
        }
        // The crate's one digest helper, so a revision is built exactly as every other
        // fingerprint of this service is.
        request_fingerprint(&parts)
    }

    /// This record as the modeled `PermissionSet` shape — what both
    /// `CreatePermissionSet` and `DescribePermissionSet` return, so a describe answers
    /// byte-identically to the create that stored it.
    pub fn to_shape(&self) -> PermissionSet {
        PermissionSet {
            name: Some(self.name.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            session_duration: self.session_duration.clone(),
            relay_state: self.relay_state.clone(),
        }
    }
}

/// A trusted token issuer connected to one IAM Identity Center instance, keyed in
/// [`ComAmazonawsSsoadminSwbexternalserviceState::trusted_token_issuers`] by the ARN
/// minted for it at creation.
///
/// The `Tags` a create carries are held here (rather than dropped) so the tagging
/// operations have one record to extend rather than a second map, and so the
/// idempotency fingerprint below covers every parameter of the request that made it.
#[derive(Debug, Clone)]
pub struct TrustedTokenIssuerRecord {
    pub trusted_token_issuer_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub trusted_token_issuer_type: String,
    pub configuration: TrustedTokenIssuerConfiguration,
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this issuer, when it carried one.
    ///
    /// Held ON the record, so the idempotency promise lives exactly as long as the
    /// resource it names: a deleted issuer frees its token instead of leaving a record
    /// that would answer a replay with an ARN nothing resolves.
    pub client_token: Option<String>,
    /// [`trusted_token_issuer_fingerprint`] of the request that minted this issuer, so
    /// a retry carrying the SAME token can be told from the same token carrying a
    /// DIFFERENT request.
    pub request_fingerprint: String,
}

impl TrustedTokenIssuerRecord {
    /// This record as the modeled `TrustedTokenIssuerMetadata` shape — the projection an
    /// ENUMERATION of trusted token issuers reports, holding the same values
    /// `DescribeTrustedTokenIssuer` reports for the same record, member for member.
    ///
    /// The shape carries three members and no more, so the stored configuration and the
    /// tags are not part of this view: a caller that wants them describes the issuer this
    /// names.
    pub fn to_metadata(&self) -> TrustedTokenIssuerMetadata {
        TrustedTokenIssuerMetadata {
            trusted_token_issuer_arn: Some(self.trusted_token_issuer_arn.clone()),
            name: Some(self.name.clone()),
            trusted_token_issuer_type: Some(self.trusted_token_issuer_type.clone()),
        }
    }
}

/// An assignment of a permission set to a principal on one account.
///
/// Its identity is the tuple (instance, account, permission set, principal type,
/// principal id); [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_position`]
/// is the single place that tuple is compared.
#[derive(Debug, Clone)]
pub struct AccountAssignmentRecord {
    pub instance_arn: String,
    pub account_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
    /// The `RequestId` of the operation that granted this assignment, so a repeat of
    /// that create answers from the SAME stored status record instead of writing a
    /// second one for work that was already done.
    pub request_id: String,
    /// The permission set's definition revision ([`PermissionSetRecord::definition_revision`])
    /// as it stood when this assignment PROVISIONED the permission set into the account.
    ///
    /// Granting an account assignment provisions the permission set into the target
    /// account, so the account then holds the definition that was current at that
    /// instant. Keeping that revision beside the row is what lets a later read tell an
    /// account holding the permission set's LATEST definition from one whose copy has
    /// since been left behind by a change to the permission set, without having to
    /// re-derive what the account was given.
    pub provisioned_revision: String,
    /// The instant the provisioning that recorded [`Self::provisioned_revision`] CAPTURED
    /// that definition: the instant this assignment was granted while the revision is the
    /// one the grant provisioned, and the instant a later `ProvisionPermissionSet` call was
    /// accepted once that provisioning has refreshed the copy.
    ///
    /// An account holds the copy the LAST provisioning put there, and a provisioning copies
    /// the definition it captured when it was accepted rather than when it lands, so this
    /// instant is what orders the two ([`is_not_older_than_recorded_copy`]): a provisioning that
    /// captured an older definition never replaces this one, however much later it lands.
    pub provisioned_at: DateTime,
}

impl AccountAssignmentRecord {
    /// The record's sort/pagination key within one (account, permission set) filter:
    /// the part of its identity the filter does not already fix.
    pub fn page_key(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }

    /// This record as the modeled `AccountAssignment` shape.
    pub fn to_shape(&self) -> AccountAssignment {
        AccountAssignment {
            account_id: Some(self.account_id.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
        }
    }

    /// The record's sort/pagination key within ONE PRINCIPAL's enumeration: the request
    /// has already fixed the instance and the (type, id) of the principal, so what remains
    /// of the assignment's identity is the account and the permission set granted on it.
    ///
    /// The two parts are joined by `#`, a character neither can hold — `AccountId` is
    /// `^\d{12}$` and `PermissionSetArn`'s pattern admits no `#` — so the key orders by
    /// account first and by permission set within an account, and no two distinct rows
    /// render the same key.
    pub fn principal_page_key(&self) -> String {
        format!("{}#{}", self.account_id, self.permission_set_arn)
    }

    /// This record as the modeled `AccountAssignmentForPrincipal` shape — the same stored
    /// row [`Self::to_shape`] projects, reported with the members that shape declares.
    pub fn to_principal_shape(&self) -> AccountAssignmentForPrincipal {
        AccountAssignmentForPrincipal {
            account_id: Some(self.account_id.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
        }
    }
}

/// A customer managed application `CreateApplication` created under one instance.
///
/// Every member `DescribeApplication` reports is held here, so a read answers from the
/// record the create wrote rather than from anything recomputed on the way out.
#[derive(Debug, Clone)]
pub struct ApplicationRecord {
    /// The server-minted `ApplicationArn` ([`mint_application_arn`]) the application is
    /// stored under.
    pub application_arn: String,
    /// The instance the application belongs to: an application created under an instance
    /// stops existing with it (see
    /// [`ComAmazonawsSsoadminSwbexternalserviceState::remove_instance`]).
    pub instance_arn: String,
    /// `ApplicationProviderArn` — the provider the application runs under, one of
    /// [`BUILT_IN_APPLICATION_PROVIDERS`].
    pub application_provider_arn: String,
    /// `DescribeApplication$ApplicationAccount` — "The account ID" the application lives
    /// in, the account segment of its own ARN.
    pub application_account: String,
    /// The identity store of the owning instance, reported as `IdentityStoreArn`.
    pub identity_store_arn: String,
    /// `Name`, as the caller sent it.
    pub name: String,
    /// `Description`, when the caller sent one.
    pub description: Option<String>,
    /// `ApplicationStatus`: `ENABLED` or `DISABLED`.
    pub status: String,
    /// `AssignmentRequired` — whether a principal needs a direct assignment to this
    /// application to reach it, as the last `PutApplicationAssignmentConfiguration` set
    /// it and as `GetApplicationAssignmentConfiguration` reports it.
    ///
    /// It lives ON the application record rather than in a second map, so an application
    /// that no longer exists holds no configuration and the configuration operations
    /// resolve the same application every other read resolves. An application nothing has
    /// configured carries [`APPLICATION_ASSIGNMENT_REQUIRED_DEFAULT`].
    pub assignment_required: bool,
    /// `UserBackgroundSessionApplicationStatus` — the application's SESSION
    /// configuration, as the last `PutApplicationSessionConfiguration` set it and as
    /// `GetApplicationSessionConfiguration` reports it.
    ///
    /// On the application record for the same reason as [`Self::assignment_required`]: an
    /// application that no longer exists holds no configuration, and the configuration
    /// operations resolve the same application every other read resolves. `None` means
    /// nothing has configured this application, which a read reports as
    /// [`USER_BACKGROUND_SESSION_STATUS_DEFAULT`] — a different statement from a status a
    /// put stored, so a put that names no status can leave it alone.
    pub user_background_session_status: Option<String>,
    /// `PortalOptions`, normalised so the stored visibility is always explicit (see
    /// [`APPLICATION_VISIBILITY_ENABLED`]).
    pub portal_options: PortalOptions,
    /// `CreatedDate` — when this create was accepted.
    pub created_date: DateTime,
    /// `CreatedFrom` — the Region of the request that created it ([`request_region`]).
    pub created_from: String,
    /// `Tags`, as the caller sent them.
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this application, if it carried one.
    pub client_token: Option<String>,
    /// [`application_fingerprint`] of that create, so a retry carrying the SAME token
    /// can be told from the same token carrying a DIFFERENT request.
    pub request_fingerprint: String,
    /// The ACCESS SCOPES of this application, by `Scope` name, as
    /// `PutApplicationAccessScope` stored them.
    ///
    /// They live ON the application record rather than in a second map, so an
    /// application that no longer exists holds no scopes and the scope operations
    /// resolve the same application every other read resolves. The value is the scope's
    /// `AuthorizedTargets`: `None` for a scope stored with no authorized targets, which
    /// is a different statement from an empty list the model's `ScopeTargets`
    /// (`length 1..10`) cannot even express. A `BTreeMap` so an enumeration walks one
    /// stable order and can page by scope name. Holds at most
    /// [`MAX_SCOPES_PER_APPLICATION`] scopes; a put that would add one beyond that
    /// ceiling is refused rather than grown into.
    pub access_scopes: BTreeMap<String, Option<Vec<String>>>,
    /// The AUTHENTICATION METHODS of this application, by `AuthenticationMethodType`, as
    /// `PutApplicationAuthenticationMethod` stored them.
    ///
    /// On the application record for the same reason as [`Self::access_scopes`]: an
    /// application that no longer exists carries no authentication method, and the
    /// method operations resolve the same application every other read resolves. Keyed
    /// by the method's TYPE, which is what identifies it — a put of a type the
    /// application already carries replaces that method and leaves any other type alone.
    /// The key needs no memory ceiling beside [`MAX_SCOPES_PER_APPLICATION`]: the
    /// model's `AuthenticationMethodType` enum admits exactly one value, which the
    /// boundary validator enforces, so this map holds at most one entry per application.
    /// A `BTreeMap` so an enumeration walks one stable order and can page by type.
    pub authentication_methods: BTreeMap<String, AuthenticationMethod>,
    /// The OAuth 2.0 GRANTS of this application, by `GrantType`, as
    /// `PutApplicationGrant` stored them.
    ///
    /// On the application record for the same reason as [`Self::access_scopes`]: an
    /// application that no longer exists carries no grant, and the grant operations
    /// resolve the same application every other read resolves. Keyed by the grant's TYPE,
    /// which is what identifies it — a put of a type the application already carries
    /// replaces that grant and leaves every other type alone. The key needs no memory
    /// ceiling: the model's `GrantType` enum admits exactly four values
    /// (`types::GRANT_TYPE_VALUES`), which the boundary validator enforces, so this map
    /// holds at most four entries per application. A `BTreeMap` so an enumeration walks
    /// one stable order and can page by grant type.
    pub grants: BTreeMap<String, Grant>,
}

impl ApplicationRecord {
    /// This record as the modeled `Application` shape — the projection an ENUMERATION of
    /// applications reports, holding the same values `DescribeApplication` reports for
    /// the same record, member for member.
    ///
    /// `Description` is the one member a create can leave unstored, so it is reported
    /// exactly when the record holds one; everything else is always stored and always
    /// reported. Nothing the create was given beyond the modelled members (its `Tags`,
    /// its `ClientToken`) belongs to this view.
    pub fn to_shape(&self) -> Application {
        Application {
            application_arn: Some(self.application_arn.clone()),
            application_provider_arn: Some(self.application_provider_arn.clone()),
            name: Some(self.name.clone()),
            application_account: Some(self.application_account.clone()),
            instance_arn: Some(self.instance_arn.clone()),
            identity_store_arn: Some(self.identity_store_arn.clone()),
            status: Some(self.status.clone()),
            portal_options: Some(self.portal_options.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            created_from: Some(self.created_from.clone()),
        }
    }

    /// The status `GetApplicationSessionConfiguration` reports for this application: the
    /// one the last `PutApplicationSessionConfiguration` stored, or
    /// [`USER_BACKGROUND_SESSION_STATUS_DEFAULT`] while nothing has configured it.
    pub fn reported_user_background_session_status(&self) -> &str {
        self.user_background_session_status
            .as_deref()
            .unwrap_or(USER_BACKGROUND_SESSION_STATUS_DEFAULT)
    }

    /// The access scopes this application carries, in `Scope` order, as the
    /// `(page key, item)` pairs [`page`] enumerates.
    ///
    /// The scope NAME is both the page key and the identity of a scope — an application
    /// carries a given scope at most once, since [`Self::access_scopes`] is keyed by it —
    /// so the enumeration order is stable across calls however the scopes were put, and a
    /// pagination cursor resumes after a KEY rather than after a position a later put
    /// would shift.
    pub fn access_scope_details(&self) -> Vec<(String, ScopeDetails)> {
        self.access_scopes
            .iter()
            .map(|(scope, authorized_targets)| {
                (
                    scope.clone(),
                    ScopeDetails {
                        scope: scope.clone(),
                        // Reported from the STORED scope: absent for a scope put with no
                        // `AuthorizedTargets`, since `ScopeTargets` is `length 1..10` and
                        // cannot express an empty list.
                        authorized_targets: authorized_targets.clone(),
                    },
                )
            })
            .collect()
    }
}

/// A direct assignment of one principal to one application.
///
/// Its identity is the tuple (application, principal type, principal id) —
/// [`ComAmazonawsSsoadminSwbexternalserviceState::application_assignment_position`] is
/// the single place that tuple is compared — so granting the same assignment twice
/// grants it once.
#[derive(Debug, Clone)]
pub struct ApplicationAssignmentRecord {
    pub application_arn: String,
    pub principal_type: String,
    pub principal_id: String,
}

impl ApplicationAssignmentRecord {
    /// The record's sort/pagination key within ONE application's enumeration: the request
    /// has already fixed the application, so what remains of the assignment's identity is
    /// the (type, id) of the principal it grants access to.
    ///
    /// The pair is UNIQUE within an application — it is exactly the rest of the identity
    /// tuple [`ComAmazonawsSsoadminSwbexternalserviceState::application_assignment_position`]
    /// compares — so no two rows of one application share a key and a cursor resumes after
    /// exactly one of them.
    pub fn page_key(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }

    /// This record as the modeled `ApplicationAssignment` shape.
    pub fn to_shape(&self) -> ApplicationAssignment {
        ApplicationAssignment {
            application_arn: self.application_arn.clone(),
            principal_type: self.principal_type.clone(),
            principal_id: self.principal_id.clone(),
        }
    }
}

/// The status record of one create/delete assignment operation, keyed by its
/// `RequestId` so `DescribeAccountAssignment*Status` and
/// `ListAccountAssignment*Status` read real history rather than a synthesised answer.
///
/// `Status` is DERIVED from [`succeeded_due`](Self::succeeded_due) rather than stored, so
/// a read never has to write to move the status on: the instant the operation completes
/// is fixed when the call that started it is accepted, and every later read computes the
/// status the clock implies at the moment it is asked.
#[derive(Debug, Clone)]
pub struct AssignmentOperationRecord {
    pub request_id: String,
    /// The instance the operation ran under. A status record belongs to one instance,
    /// exactly as the assignment it reports on does, so a `RequestId` minted under
    /// another instance is as absent as one that was never minted.
    pub instance_arn: String,
    pub failure_reason: Option<String>,
    pub target_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
    /// When this operation stops being `IN_PROGRESS` and becomes `SUCCEEDED`:
    /// [`ASSIGNMENT_OPERATION_SPAN`] after the call that started it was accepted.
    pub succeeded_due: SystemTime,
}

impl AssignmentOperationRecord {
    /// `StatusValues` as of `now`: the non-terminal `IN_PROGRESS` until the operation's
    /// completion is due, `SUCCEEDED` after.
    ///
    /// The enum's third value, `FAILED`, is never reported: nothing here fails an
    /// assignment operation, which is also why [`Self::failure_reason`] — "the message
    /// that contains an error or exception in case of an operation failure" — stays
    /// absent.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.succeeded_due {
            STATUS_IN_PROGRESS
        } else {
            STATUS_SUCCEEDED
        }
    }

    /// The record's sort/pagination key within the history of one instance's assignment
    /// operations: the instant the operation was accepted, then its `RequestId`.
    ///
    /// Ordering an enumeration by this key walks the history oldest first, which is the
    /// order it was recorded in, and the `RequestId` tail makes the key UNIQUE — two
    /// operations accepted in the same nanosecond still get distinct keys — so a page can
    /// resume after it. The instant is rendered fixed-width and zero-padded so the
    /// lexicographic comparison the paging helper makes is the chronological one.
    pub fn page_key(&self) -> String {
        format!(
            "{:011}.{:09}#{}",
            self.created_date.secs(),
            self.created_date.subsec_nanos(),
            self.request_id
        )
    }

    /// This record as the modeled `AccountAssignmentOperationStatusMetadata` shape, as of
    /// `now` — the three members an operation-status ENUMERATION reports (the status the
    /// clock implies, the `RequestId` to poll it by and the date it was accepted), which
    /// are exactly the members [`Self::to_tracking_shape`] fills on the fuller status
    /// object, so a list and the call that started the operation cannot disagree.
    pub fn to_metadata_shape(&self, now: SystemTime) -> AccountAssignmentOperationStatusMetadata {
        AccountAssignmentOperationStatusMetadata {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            created_date: Some(self.created_date),
        }
    }

    /// This record as the modeled `AccountAssignmentOperationStatus` shape, as of `now`:
    /// the whole status object, tracking members and the assignment the operation is
    /// about — what a status DESCRIBE reports.
    pub fn to_shape(&self, now: SystemTime) -> AccountAssignmentOperationStatus {
        AccountAssignmentOperationStatus {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            failure_reason: self.failure_reason.clone(),
            target_id: Some(self.target_id.clone()),
            target_type: Some(self.target_type.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
            created_date: Some(self.created_date),
        }
    }

    /// The TRACKING view of this record, as of `now`: the status the operation is in, the
    /// `RequestId` — "the identifier for tracking the request operation" — to poll it by,
    /// and the date it was accepted, and none of the assignment detail.
    ///
    /// Those three members are exactly the ones the model's own
    /// `AccountAssignmentOperationStatusMetadata` carries, which is what the API reports
    /// for an operation a caller has not described yet; it is what the call that STARTS
    /// an operation answers with, since a caller holding the whole request already knows
    /// the assignment it asked for and what it does not yet know is the handle to poll.
    pub fn to_tracking_shape(&self, now: SystemTime) -> AccountAssignmentOperationStatus {
        AccountAssignmentOperationStatus {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            created_date: Some(self.created_date),
            failure_reason: None,
            target_id: None,
            target_type: None,
            permission_set_arn: None,
            principal_type: None,
            principal_id: None,
        }
    }
}

/// One PROVISIONING of a permission set into the accounts a `ProvisionPermissionSet` call
/// targeted, tracked by the `RequestId` that call answered with.
///
/// `Status` is DERIVED from [`succeeded_due`](Self::succeeded_due) rather than stored,
/// exactly as an [`AssignmentOperationRecord`]'s is, so a read never has to write to move
/// the status on. The COPY the provisioning carries — [`revision`](Self::revision), the
/// permission set's definition as it stood when the call was accepted — reaches the
/// accounts in [`accounts`](Self::accounts) at that SAME instant, applied by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::materialize_due`], so an account's copy
/// changes when the workflow completes rather than when it was asked for.
#[derive(Debug, Clone)]
pub struct PermissionSetProvisioningRecord {
    pub request_id: String,
    /// The instance the provisioning ran under. A status record belongs to one instance,
    /// like an assignment operation's, so a `RequestId` minted under another instance is
    /// as absent as one that was never minted.
    pub instance_arn: String,
    pub permission_set_arn: String,
    /// The account the call named as its target, when it named ONE
    /// (`ProvisionTargetType::AWS_ACCOUNT`) — what the status object reports as its
    /// `AccountId`. A provisioning of every account that already holds the permission set
    /// (`ALL_PROVISIONED_ACCOUNTS`) names no single account, so it carries none.
    pub account_id: Option<String>,
    /// Every account this provisioning copies the permission set into, FIXED when the
    /// call was accepted: the one account an `AWS_ACCOUNT` target names, or the accounts
    /// holding a copy at that instant for an `ALL_PROVISIONED_ACCOUNTS` one. An account
    /// that started holding the permission set after the call is not part of the work
    /// this call accepted.
    pub accounts: BTreeSet<String>,
    /// The permission set's definition revision
    /// ([`PermissionSetRecord::definition_revision`]) as it stood when the call was
    /// accepted: the definition this provisioning copies, so a change made to the
    /// permission set while the workflow is in flight is not the one that lands.
    pub revision: String,
    pub failure_reason: Option<String>,
    pub created_date: DateTime,
    /// When this provisioning stops being `IN_PROGRESS` and becomes `SUCCEEDED`:
    /// [`PERMISSION_SET_PROVISIONING_SPAN`] after the call that started it was accepted.
    pub succeeded_due: SystemTime,
    /// Whether the copy has already reached the target accounts. Set when the clock makes
    /// the provisioning due and the copy is applied, so the copy lands exactly once
    /// however often the store is swept.
    pub applied: bool,
}

impl PermissionSetProvisioningRecord {
    /// `StatusValues` as of `now`: the non-terminal `IN_PROGRESS` until the provisioning's
    /// completion is due, `SUCCEEDED` after.
    ///
    /// The enum's third value, `FAILED`, is never reported: nothing here fails a
    /// provisioning, which is also why [`Self::failure_reason`] stays absent.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.succeeded_due {
            STATUS_IN_PROGRESS
        } else {
            STATUS_SUCCEEDED
        }
    }

    /// Whether the clock has made this provisioning's completion due.
    pub fn completion_due(&self, now: SystemTime) -> bool {
        now >= self.succeeded_due
    }

    /// This record as the modeled `PermissionSetProvisioningStatus` shape, as of `now` —
    /// what both `ProvisionPermissionSet` and `DescribePermissionSetProvisioningStatus`
    /// report, so a describe cannot disagree with the call that minted the request id.
    pub fn to_shape(&self, now: SystemTime) -> PermissionSetProvisioningStatus {
        PermissionSetProvisioningStatus {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            account_id: self.account_id.clone(),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            failure_reason: self.failure_reason.clone(),
            created_date: Some(self.created_date),
        }
    }

    /// The TRACKING view of this record, as of `now`: the status the provisioning is in,
    /// the `RequestId` — "the identifier for tracking the request operation" — to poll it
    /// by, and the date it was created, and none of the provisioning's detail.
    ///
    /// Those three members are exactly the ones the model's own
    /// `PermissionSetProvisioningStatusMetadata` carries, the member type of the list an
    /// enumeration of this history answers with: what a caller that has not described a
    /// tracked provisioning yet is told, so it can describe the ones it cares about.
    pub fn to_metadata_shape(&self, now: SystemTime) -> PermissionSetProvisioningStatusMetadata {
        PermissionSetProvisioningStatusMetadata {
            status: Some(self.status(now).to_string()),
            request_id: Some(self.request_id.clone()),
            created_date: Some(self.created_date),
        }
    }

    /// The record's sort/pagination key within one enumeration of this history: the
    /// instant it was created, then the `RequestId`.
    ///
    /// The instant comes first so the enumeration walks the history in the order it
    /// happened, and it is rendered at a FIXED width (`created_date` is the instant a
    /// `ProvisionPermissionSet` call was accepted, so its epoch seconds are positive) so
    /// the text order of the key is the order of the instant. The `RequestId` breaks the
    /// tie between two provisionings accepted at the same instant, and is unique across
    /// the history, so no two records share a key and a cursor resumes after exactly one
    /// of them.
    pub fn page_key(&self) -> String {
        format!(
            "{:020}.{:09}#{}",
            self.created_date.secs(),
            self.created_date.subsec_nanos(),
            self.request_id
        )
    }
}

/// One COPY of a permission set an account holds because a provisioning PUT it there
/// directly, rather than because an assignment of it was granted on the account.
///
/// `ProvisionPermissionSet` provisions the permission set into the accounts its target
/// names, whether or not those accounts hold an assignment of it, so the copy such a
/// provisioning delivers has to be recorded somewhere of its own: an account the
/// provisioning named and no assignment reaches holds the definition the provisioning
/// carried, and every read of where the permission set is provisioned reports it from
/// here.
#[derive(Debug, Clone)]
pub struct ProvisionedCopyRecord {
    /// The instance the copy was provisioned under. A copy belongs to one instance,
    /// like the permission set it is a copy of, so a copy provisioned under another
    /// instance is as absent as one that was never provisioned.
    pub instance_arn: String,
    /// The permission set this is a copy OF.
    pub permission_set_arn: String,
    /// The account that holds the copy.
    pub account_id: String,
    /// The permission set's definition revision
    /// ([`PermissionSetRecord::definition_revision`]) this copy carries: what a read
    /// compares against the permission set's revision now to tell an account holding the
    /// latest definition from one the permission set has changed since.
    pub revision: String,
    /// The instant the provisioning that delivered [`Self::revision`] CAPTURED it — when
    /// that provisioning was accepted — which is what orders this copy against every other
    /// provisioning of the same account, exactly as
    /// [`AccountAssignmentRecord::provisioned_at`] does for the copy a grant delivered.
    pub provisioned_at: DateTime,
}

/// One COPY of a permission set an account holds, as the single pass over the records
/// yields it: which permission set, in which account, and the provisioning status that
/// copy is in.
///
/// Borrows the ARNs and the account id from the records themselves, because the pass is a
/// read over state the caller already holds; a projection clones only the key its own
/// answer is keyed by.
#[derive(Debug, Clone, Copy)]
struct HeldCopy<'a> {
    /// The permission set the copy is a copy OF.
    permission_set_arn: &'a str,
    /// The account that holds it.
    account_id: &'a str,
    /// The status that copy is in ([`provisioning_status_of`]).
    status: &'static str,
}

/// Per-service state for com-amazonaws-ssoadmin-swbexternalservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsoadminSwbexternalserviceState {
    /// Instances by `InstanceArn`. The account's AMBIENT instance is enabled into this
    /// map by the first request that reaches the store (see [`Self::on_request`]); an ARN
    /// absent from it is the `ResourceNotFoundException`/`AccessDeniedException` every
    /// instance-taking operation answers with.
    pub instances: BTreeMap<String, Instance>,
    /// Permission sets by `PermissionSetArn`. A `BTreeMap` so `ListPermissionSets`
    /// enumerates in one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records; a create that would cross that ceiling is
    /// refused rather than grown into.
    pub permission_sets: BTreeMap<String, PermissionSetRecord>,
    /// Account assignments. At most one row per identity tuple, and at most
    /// [`MAX_RECORDS_PER_STORE`] rows in all.
    pub account_assignments: Vec<AccountAssignmentRecord>,
    /// The copies a `ProvisionPermissionSet` DELIVERED to the accounts it targeted, in
    /// the order they first landed: at most one row per (instance, permission set,
    /// account) triple, refreshed in place by a later provisioning of the same triple
    /// (see [`Self::record_provisioned_copy`]).
    ///
    /// A `Vec` in landing order, so the ceiling below evicts the copy provisioned
    /// longest ago. Like [`Self::assignment_deletions`] it EVICTS rather than refusing
    /// at [`MAX_RECORDS_PER_STORE`]: a provisioning of a permission set that exists is
    /// accepted, and `ProvisionPermissionSet` models no quota refusal to answer with.
    pub provisioned_copies: Vec<ProvisionedCopyRecord>,
    /// Trusted token issuers by `TrustedTokenIssuerArn`. A `BTreeMap` so an
    /// enumeration walks one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records.
    pub trusted_token_issuers: BTreeMap<String, TrustedTokenIssuerRecord>,
    /// Applications by `ApplicationArn`, each carrying its own access scopes. A
    /// `BTreeMap` so an enumeration walks one stable order and can page by key. Holds at
    /// most [`MAX_RECORDS_PER_STORE`] records.
    pub applications: BTreeMap<String, ApplicationRecord>,
    /// Direct application assignments. At most one row per (application, principal type,
    /// principal id) tuple, and at most [`MAX_RECORDS_PER_STORE`] rows in all.
    pub application_assignments: Vec<ApplicationAssignmentRecord>,
    /// Assignment operation statuses by `RequestId`: one per assignment currently
    /// granted, so a repeated create reads the record it already wrote instead of
    /// adding another, and a delete that removes an assignment removes its status
    /// record with it. Holds at most [`MAX_RECORDS_PER_STORE`] records.
    pub assignment_operations: BTreeMap<String, AssignmentOperationRecord>,
    /// Assignment DELETION statuses, newest last — the history
    /// `DescribeAccountAssignmentDeletionStatus` reads by `RequestId`, in the same record
    /// type a creation status uses ([`AssignmentOperationRecord`], which carries the
    /// instance it ran under), so the two views cannot disagree about the members of
    /// `AccountAssignmentOperationStatus`.
    ///
    /// Separate from [`Self::assignment_operations`] because the two are read by
    /// different operations: a `RequestId` a deletion carries is not a creation status and
    /// must not answer a describe of one. A deletion status is also history rather than a
    /// resource — by the time it is stored the assignment it reports on is gone, so it
    /// cannot be held beside that assignment. A `Vec` in insertion order, so the history
    /// is chronological; it holds at most [`MAX_RECORDS_PER_STORE`] records and the oldest
    /// is evicted beyond that (see [`Self::record_assignment_deletion`]) rather than a
    /// delete being refused for history nobody can grow. A record whose permission set has
    /// gone is not reported, for the reason given on [`Self::assignment_deletion`], which
    /// is the ONE place a reader of this history resolves an id.
    pub assignment_deletions: Vec<AssignmentOperationRecord>,
    /// Permission-set PROVISIONING statuses, newest last — the history
    /// `DescribePermissionSetProvisioningStatus` reads by `RequestId`, and the record of
    /// the copy each provisioning owes its target accounts until the clock makes it due
    /// (see [`Self::materialize_due`]).
    ///
    /// Separate from [`Self::assignment_operations`] because the two are read by different
    /// operations: a `RequestId` a provisioning carries is not an assignment operation's
    /// and must not answer a describe of one. A `Vec` in insertion order, so the history
    /// is chronological; it holds at most [`MAX_RECORDS_PER_STORE`] records and the oldest
    /// is evicted beyond that (see [`Self::record_permission_set_provisioning`]) rather
    /// than a provisioning being refused for history nobody can grow.
    pub permission_set_provisionings: Vec<PermissionSetProvisioningRecord>,
    /// Whether the account's IAM Identity Center ENABLEMENT has been decided already.
    ///
    /// The emulator's account gets its instance from the first request that needs one: a
    /// `CreateInstance` enables the instance the caller asked for
    /// ([`Self::on_instance_create`]), and any other request finds the account's AMBIENT
    /// instance ([`Self::on_request`]). This flag is what makes that a ONE-TIME decision:
    /// an instance a caller deletes stays deleted instead of being replaced by an ambient
    /// one on the next read, and an instance a caller created is the account's one
    /// instance rather than a second beside an invented one.
    pub ambient_instance_enabled: bool,
}

impl ComAmazonawsSsoadminSwbexternalserviceState {
    /// What every handler that RESOLVES an instance does BEFORE it looks at this store:
    /// enable the account's ambient instance if nothing has decided the account's
    /// enablement yet, then apply every transition the clock already owes
    /// ([`Self::materialize_due`]).
    ///
    /// `region` is the Region of the request ([`request_region`]) — the Region the
    /// ambient instance is enabled in, since the request is the only Region signal the
    /// emulator has. Nothing else about the request reaches the record, and a second
    /// request from another Region finds the instance already enabled and leaves its
    /// Region alone, so this fixes that Region ONCE and every later read of the instance
    /// answers the same.
    pub fn on_request(&mut self, region: &str, now: SystemTime) {
        self.enable_ambient_instance(region, now);
        self.materialize_due(now);
    }

    /// What `CreateInstance` does instead, before it looks at this store.
    ///
    /// It enables NO ambient instance: that caller is enabling the account's instance
    /// ITSELF, and inventing one for it first would refuse — on the account's
    /// one-instance quota — the very call that establishes the account's enablement. It
    /// still applies what the clock already owes, so the quota is judged against the
    /// instances that exist at this instant. Once the create has stored its instance,
    /// [`Self::instance_enablement_decided`] records that the account's enablement is
    /// settled, so no later read enables an ambient instance beside it.
    pub fn on_instance_create(&mut self, now: SystemTime) {
        self.materialize_due(now);
    }

    /// Record that the account's IAM Identity Center enablement is settled — a caller's
    /// `CreateInstance` enabled it — so no later request enables an ambient instance.
    pub fn instance_enablement_decided(&mut self) {
        self.ambient_instance_enabled = true;
    }

    /// Enable the account's ambient instance: the instance the emulator's account already
    /// holds, the stand-in for an account whose IAM Identity Center was enabled before
    /// this session started.
    ///
    /// It is an ACCOUNT instance ([`SEED_INSTANCE_TYPE`]), so the permission-set surface
    /// is withdrawn on it until an `UpdateInstance` enables permission sets. It is
    /// enabled at most once per store, and not at all when the store already holds an
    /// instance (one instance per account), so no request ever adds a second instance or
    /// revives a deleted one.
    fn enable_ambient_instance(&mut self, region: &str, now: SystemTime) {
        let already_decided = self.ambient_instance_enabled || !self.instances.is_empty();
        self.ambient_instance_enabled = true;
        if already_decided {
            return;
        }
        let instance = Instance::active(
            SEED_INSTANCE_ID,
            SEED_IDENTITY_STORE_ID,
            SEED_INSTANCE_TYPE,
            Some(SEED_INSTANCE_NAME.to_string()),
            DateTime::from(now),
            region,
        );
        self.instances
            .insert(instance.instance_arn.clone(), instance);
    }
}

impl ComAmazonawsSsoadminSwbexternalserviceState {
    /// The instance `instance_arn` names, or `None` when no such instance exists.
    pub fn instance(&self, instance_arn: &str) -> Option<&Instance> {
        self.instances.get(instance_arn)
    }

    /// Whether this ACCOUNT holds the instance `instance_arn` names — the question another
    /// emulated service asks before it registers a resource OF that instance (Glue registers
    /// its Identity Center application with one).
    ///
    /// The account holds the instances this store holds, and — while nothing has decided the
    /// account's enablement ([`Self::ambient_instance_enabled`]) — the AMBIENT instance the
    /// next request would be given ([`Self::enable_ambient_instance`]), whose id is
    /// [`SEED_INSTANCE_ID`]. Stated HERE, beside the rule that hands that instance out, so a
    /// service asking the question cannot re-derive it and drift from it.
    ///
    /// A READ: it reports what the account holds at this instant and enables nothing, so
    /// asking it leaves the next request's answer exactly as it was.
    pub fn holds_instance(&self, instance_arn: &str) -> bool {
        self.instance(instance_arn).is_some()
            || (!self.ambient_instance_enabled
                && instance_arn == self::instance_arn(SEED_INSTANCE_ID))
    }

    /// The instance `instance_arn` names, for a handler that updates it.
    pub fn instance_mut(&mut self, instance_arn: &str) -> Option<&mut Instance> {
        self.instances.get_mut(instance_arn)
    }

    /// Whether the instance `instance_arn` names is still being CREATED
    /// ([`Instance::creation_in_progress`]) — what every operation OTHER than that
    /// instance's own describe and delete asks about the instance it resolves, whether
    /// that operation reports or writes: only those two are available in that window, so
    /// every other call on the instance, or on a resource held under it, is refused with
    /// [`instance_creation_in_progress_message`] on the channel its own model carries
    /// (see [`Self::instance_creation_in_progress_gate`]).
    ///
    /// `false` for an ARN this account holds no instance for: that request is a missing
    /// resource, which each operation answers on its own not-found channel — the same
    /// division [`Self::permission_set_surface_withdrawn`] keeps.
    pub fn instance_creation_in_progress(&self, instance_arn: &str) -> bool {
        self.instance(instance_arn)
            .is_some_and(Instance::creation_in_progress)
    }

    /// That window as the REFUSAL it earns, so every operation it withholds states the
    /// rule once: `Err` carrying [`instance_creation_in_progress_message`] while the
    /// instance `instance_arn` names is still being created, `Ok(())` otherwise.
    ///
    /// Asked by every operation but the instance's own describe and delete, once the
    /// instance itself is resolved and before anything finer the request names, because
    /// the window withholds the call whatever else it asks for. `Ok(())` for an ARN this
    /// account holds no instance for, exactly as
    /// [`Self::instance_creation_in_progress`] answers `false` for it: that request is a
    /// missing resource, reported on each operation's own not-found channel.
    ///
    /// The CHANNEL is the caller's, as it is for every refusal this module words: the
    /// operations whose model declares a `ConflictException` answer there, and the
    /// instance-scoped reads, whose models declare none, on their modelled
    /// `AccessDeniedException` — the same channel each of them already answers an
    /// instance this account does not hold on ([`instance_not_found_message`]).
    pub fn instance_creation_in_progress_gate(&self, instance_arn: &str) -> Result<(), String> {
        if self.instance_creation_in_progress(instance_arn) {
            return Err(instance_creation_in_progress_message(instance_arn));
        }
        Ok(())
    }

    /// Apply every transition the CLOCK already owes: see [`Instance::materialize_due`],
    /// plus the REMOVAL a `DeleteInstance` owes — an instance whose deletion instant has
    /// passed no longer exists, whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let removed: Vec<String> = self
            .instances
            .values()
            .filter(|instance| instance.removal_due(now))
            .map(|instance| instance.instance_arn.clone())
            .collect();
        for instance_arn in &removed {
            self.remove_instance(instance_arn);
        }
        for instance in self.instances.values_mut() {
            instance.materialize_due(now);
        }
        self.materialize_due_provisionings(now);
    }

    /// Apply every permission-set provisioning whose completion the clock has made due:
    /// the copy it carries reaches its target accounts at that instant, whether or not
    /// anybody called.
    ///
    /// Each provisioning lands exactly once — the record is marked applied — so a later
    /// sweep neither re-copies a definition the permission set has since moved past nor
    /// undoes a provisioning that followed it.
    fn materialize_due_provisionings(&mut self, now: SystemTime) {
        let due: Vec<usize> = self
            .permission_set_provisionings
            .iter()
            .enumerate()
            .filter(|(_, record)| !record.applied && record.completion_due(now))
            .map(|(index, _)| index)
            .collect();
        for index in due {
            self.apply_provisioning(index);
        }
    }

    /// Copy the definition one stored provisioning carries into the accounts it targeted,
    /// by recording that revision as the one each of those accounts now holds. A
    /// provisioning that has already landed is left alone, so it lands exactly once
    /// however often the store is swept: re-copying it would undo a provisioning that
    /// followed it.
    ///
    /// A target account holds the copy whether or not an assignment of the permission set
    /// was ever granted there — the provisioning is what puts the permission set into the
    /// account, so a target that holds no assignment is provisioned all the same. The copy
    /// is recorded in BOTH places a reader looks: on the assignments granted on the
    /// account, whose revision is what [`Self::provisioned_accounts`] compares for an
    /// assigned account, and as a [`ProvisionedCopyRecord`], which is what a reader has for
    /// an account no assignment reaches.
    ///
    /// Either place can already hold a copy a LATER provisioning recorded — granting an
    /// assignment provisions the permission set into its account at the instant the grant
    /// is accepted, so a grant made while this workflow was in flight carries the later
    /// definition — and such a copy is left alone ([`is_not_older_than_recorded_copy`]): this
    /// provisioning copies what it captured when it was accepted, which does not undo the
    /// provisioning that followed it.
    fn apply_provisioning(&mut self, index: usize) {
        let Some(record) = self.permission_set_provisionings.get_mut(index) else {
            return;
        };
        if record.applied {
            return;
        }
        record.applied = true;
        let instance_arn = record.instance_arn.clone();
        let permission_set_arn = record.permission_set_arn.clone();
        let revision = record.revision.clone();
        let captured_at = record.created_date;
        let accounts = record.accounts.clone();
        for account_id in &accounts {
            self.record_copy_on_assignment_rows(
                &instance_arn,
                &permission_set_arn,
                account_id,
                &revision,
                captured_at,
            );
            self.record_provisioned_copy(
                &instance_arn,
                &permission_set_arn,
                account_id,
                &revision,
                captured_at,
            );
        }
    }

    /// Record the copy a provisioning that captured its definition at `captured_at`
    /// delivered at `revision` on every ASSIGNMENT ROW of `account_id` for
    /// `permission_set_arn` — the rows that record a copy of that permission set in that
    /// account ([`AccountAssignmentRecord::provisioned_revision`]).
    ///
    /// The ONE place a delivered copy is written onto those rows, so every caller applies
    /// the same rule [`is_not_older_than_recorded_copy`] states: a row recording a copy
    /// captured LATER is left alone, because this provisioning copies what it captured when
    /// it was accepted and must not rewind the provisioning that followed it.
    ///
    /// Writes no new row: a row exists because an assignment is granted, which this does not
    /// do.
    fn record_copy_on_assignment_rows(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
        account_id: &str,
        revision: &str,
        captured_at: DateTime,
    ) {
        for row in &mut self.account_assignments {
            if row.instance_arn != instance_arn
                || row.permission_set_arn != permission_set_arn
                || row.account_id != account_id
            {
                continue;
            }
            if !is_not_older_than_recorded_copy(&captured_at, &row.provisioned_at) {
                continue;
            }
            row.provisioned_revision = revision.to_string();
            row.provisioned_at = captured_at;
        }
    }

    /// Record that `account_id` holds the copy of `permission_set_arn` a provisioning that
    /// captured its definition at `captured_at` delivered at `revision`, REPLACING the copy
    /// that account held before: an account holds one copy of a permission set, the one the
    /// last provisioning put there.
    ///
    /// A copy a LATER provisioning recorded is kept instead, under the rule
    /// [`is_not_older_than_recorded_copy`] states: the copy in place is the one whose definition
    /// was captured last, whatever order the provisionings carrying them landed in.
    ///
    /// Evicts the copies that landed longest ago while the store is over
    /// [`MAX_RECORDS_PER_STORE`], for the reason given on [`Self::provisioned_copies`].
    fn record_provisioned_copy(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
        account_id: &str,
        revision: &str,
        captured_at: DateTime,
    ) {
        if let Some(copy) =
            self.provisioned_copy_mut(instance_arn, permission_set_arn, account_id)
        {
            if is_not_older_than_recorded_copy(&captured_at, &copy.provisioned_at) {
                copy.revision = revision.to_string();
                copy.provisioned_at = captured_at;
            }
            return;
        }
        self.provisioned_copies.push(ProvisionedCopyRecord {
            instance_arn: instance_arn.to_string(),
            permission_set_arn: permission_set_arn.to_string(),
            account_id: account_id.to_string(),
            revision: revision.to_string(),
            provisioned_at: captured_at,
        });
        let excess = self
            .provisioned_copies
            .len()
            .saturating_sub(MAX_RECORDS_PER_STORE);
        self.provisioned_copies.drain(..excess);
    }

    /// The copy `account_id` holds of `permission_set_arn` under `instance_arn`, for a
    /// caller that updates it. The ONE place the triple that identifies a copy is
    /// compared.
    fn provisioned_copy_mut(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
        account_id: &str,
    ) -> Option<&mut ProvisionedCopyRecord> {
        self.provisioned_copies.iter_mut().find(|copy| {
            copy.instance_arn == instance_arn
                && copy.permission_set_arn == permission_set_arn
                && copy.account_id == account_id
        })
    }

    /// Carry the copy a WITHDRAWN assignment recorded onto every record that still holds a
    /// copy of that permission set in the account: its remaining assignment rows and the
    /// copy a provisioning delivered there.
    ///
    /// Withdrawing an assignment delivers no copy, so it cannot change which definition the
    /// account holds: an account holds the LAST copy delivered to it, and a grant delivers
    /// one too, so an account whose newest copy came from the assignment being withdrawn
    /// goes on reporting that definition for as long as it holds the permission set at all
    /// — whether what holds it is another assignment or a provisioning that targeted it.
    /// Leaving the withdrawn row's copy behind would rewind the account to an older
    /// definition no later provisioning had undone.
    ///
    /// Only records that already exist are written, so an account that holds no other record
    /// of the permission set — no remaining assignment and no provisioning that targeted it
    /// — is deprovisioned when its last assignment goes, as before.
    ///
    /// `captured_at` is the instant the withdrawn row's copy was captured, so a record
    /// holding a copy a LATER provisioning delivered is kept instead, under the one rule
    /// [`is_not_older_than_recorded_copy`] states.
    pub fn keep_withdrawn_copy(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
        account_id: &str,
        revision: &str,
        captured_at: DateTime,
    ) {
        self.record_copy_on_assignment_rows(
            instance_arn,
            permission_set_arn,
            account_id,
            revision,
            captured_at,
        );
        if let Some(copy) =
            self.provisioned_copy_mut(instance_arn, permission_set_arn, account_id)
        {
            if is_not_older_than_recorded_copy(&captured_at, &copy.provisioned_at) {
                copy.revision = revision.to_string();
                copy.provisioned_at = captured_at;
            }
        }
    }

    /// Forget every copy of `permission_set_arn` under `instance_arn`: a permission set
    /// that no longer exists is provisioned nowhere, and leaving its copies behind would
    /// let a later permission set minted under the same ARN inherit accounts nothing
    /// provisioned it into.
    pub fn forget_provisioned_copies(&mut self, instance_arn: &str, permission_set_arn: &str) {
        self.provisioned_copies.retain(|copy| {
            copy.instance_arn != instance_arn || copy.permission_set_arn != permission_set_arn
        });
    }

    /// Store the status of one permission-set provisioning, evicting the OLDEST records
    /// while the history is over [`MAX_RECORDS_PER_STORE`].
    ///
    /// Like [`Self::record_assignment_deletion`], this store evicts rather than refusing:
    /// AWS accepts a provisioning of a permission set that exists, so the emulator's
    /// memory ceiling is kept by dropping the oldest status instead of refusing the call.
    /// An evicted record's copy lands only if the clock had ALREADY made it due at `now`:
    /// eviction is the memory ceiling and not a second clock, so it never delivers a copy
    /// ahead of the instant the provisioning that owes it reported.
    pub fn record_permission_set_provisioning(
        &mut self,
        record: PermissionSetProvisioningRecord,
        now: SystemTime,
    ) {
        self.permission_set_provisionings.push(record);
        let excess = self
            .permission_set_provisionings
            .len()
            .saturating_sub(MAX_RECORDS_PER_STORE);
        for index in 0..excess {
            if self
                .permission_set_provisionings
                .get(index)
                .is_some_and(|record| record.completion_due(now))
            {
                self.apply_provisioning(index);
            }
        }
        self.permission_set_provisionings.drain(..excess);
    }

    /// The status record of the permission-set provisioning `request_id` names, WITHIN
    /// `instance_arn`.
    ///
    /// Scoped to the instance exactly as [`Self::assignment_operation`] is, and absent
    /// once the permission set it reports on has gone: a status exists to report the
    /// provisioning of one permission set, so an id whose permission set no reader can
    /// see is as absent as one that was never minted.
    pub fn permission_set_provisioning(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&PermissionSetProvisioningRecord> {
        self.permission_set_provisionings
            .iter()
            .find(|record| record.instance_arn == instance_arn && record.request_id == request_id)
            .filter(|record| {
                self.permission_set(instance_arn, &record.permission_set_arn)
                    .is_some()
            })
    }

    /// Every permission-set provisioning this instance TRACKS, in the order the history
    /// happened ([`PermissionSetProvisioningRecord::page_key`] is that same order).
    ///
    /// A record is tracked under exactly the condition
    /// [`Self::permission_set_provisioning`] resolves one under — it is delegated to, so
    /// there is ONE rule — hence an enumeration of this history and a describe of one of
    /// its ids cannot disagree about which ids exist.
    pub fn tracked_permission_set_provisionings(
        &self,
        instance_arn: &str,
    ) -> Vec<&PermissionSetProvisioningRecord> {
        self.permission_set_provisionings
            .iter()
            .filter(|record| {
                self.permission_set_provisioning(instance_arn, &record.request_id)
                    .is_some()
            })
            .collect()
    }

    /// Whether any stored permission-set provisioning already carries `request_id`, so a
    /// freshly minted one is never reused for a second provisioning.
    pub fn permission_set_provisioning_request_id_taken(&self, request_id: &str) -> bool {
        self.permission_set_provisionings
            .iter()
            .any(|record| record.request_id == request_id)
    }

    /// Drop the permission set `permission_set_arn` names under `instance_arn`, and every
    /// row the store holds FOR it: the copies its provisionings delivered, the
    /// provisionings themselves, the assignments made from it and the status records
    /// reporting on those assignments.
    ///
    /// The provisionings go because one of them can still be IN FLIGHT: an unapplied
    /// record is a write the clock owes, and [`Self::materialize_due_provisionings`]
    /// applies what is owed whether or not anybody called, so a record left behind would
    /// deliver a copy of a permission set no reader can see — the very copies
    /// [`Self::forget_provisioned_copies`] had just forgotten. Dropping the owed write
    /// with the resource it reports on is what [`Self::remove_instance`] already does one
    /// level up, and it is the same rule: a resource that no longer exists holds no rows
    /// and owes no work, so nothing can be written for it after it has gone.
    pub fn remove_permission_set(&mut self, instance_arn: &str, permission_set_arn: &str) {
        self.permission_sets.remove(permission_set_arn);
        self.forget_provisioned_copies(instance_arn, permission_set_arn);
        self.permission_set_provisionings.retain(|record| {
            record.instance_arn != instance_arn || record.permission_set_arn != permission_set_arn
        });
        let withdrawn: Vec<String> = self
            .account_assignments
            .iter()
            .filter(|row| {
                row.instance_arn == instance_arn && row.permission_set_arn == permission_set_arn
            })
            .map(|row| row.request_id.clone())
            .collect();
        self.account_assignments.retain(|row| {
            row.instance_arn != instance_arn || row.permission_set_arn != permission_set_arn
        });
        for request_id in &withdrawn {
            self.assignment_operations.remove(request_id);
        }
        self.assignment_deletions.retain(|record| {
            record.instance_arn != instance_arn || record.permission_set_arn != permission_set_arn
        });
    }

    /// Drop the instance `instance_arn` names and everything held INSIDE it.
    ///
    /// A permission set, an account assignment, a trusted token issuer and an
    /// application each belong to one instance — every one of them is created under its
    /// `InstanceArn` — so an instance that no longer exists holds none of them: leaving
    /// them behind would let a later create of an instance inherit resources nobody
    /// created under it. The status record of an assignment operation goes with the
    /// permission set it names, and an application assignment with the application it
    /// grants access to, for the same reason.
    fn remove_instance(&mut self, instance_arn: &str) {
        self.instances.remove(instance_arn);
        self.permission_sets
            .retain(|_, record| record.instance_arn != instance_arn);
        self.account_assignments
            .retain(|record| record.instance_arn != instance_arn);
        self.provisioned_copies
            .retain(|record| record.instance_arn != instance_arn);
        self.trusted_token_issuers
            .retain(|_, record| record.instance_arn != instance_arn);
        self.assignment_operations
            .retain(|_, record| record.instance_arn != instance_arn);
        self.assignment_deletions
            .retain(|record| record.instance_arn != instance_arn);
        self.permission_set_provisionings
            .retain(|record| record.instance_arn != instance_arn);
        let orphaned_applications: Vec<String> = self
            .applications
            .values()
            .filter(|record| record.instance_arn == instance_arn)
            .map(|record| record.application_arn.clone())
            .collect();
        for application_arn in &orphaned_applications {
            self.remove_application(application_arn);
        }
    }

    /// Drop the application `application_arn` names and everything held INSIDE it,
    /// reporting whether such an application was stored at all.
    ///
    /// Its access scopes, its authentication methods and its grants live ON the record
    /// (see [`ApplicationRecord`]), so they go with it. Its DIRECT assignments are stored
    /// beside it and are dropped here, because an assignment names the application it
    /// grants access to: leaving the rows behind would keep a grant of an application no
    /// reader can see, and would let a later application minted under the same ARN
    /// inherit an access nobody granted on it.
    ///
    /// Shared by `DeleteApplication` and by the instance removal above, so an application
    /// a caller deletes and one the clock removes with its instance leave the same store.
    pub fn remove_application(&mut self, application_arn: &str) -> bool {
        let removed = self.applications.remove(application_arn).is_some();
        self.application_assignments
            .retain(|record| record.application_arn != application_arn);
        removed
    }

    /// Whether the permission-set / account-assignment surface is WITHDRAWN on the
    /// instance `instance_arn` names, i.e. it is an account instance.
    ///
    /// `false` for an ARN this account holds no instance for: that request is a missing
    /// resource, which each operation answers with its own
    /// `ResourceNotFoundException`/`AccessDeniedException` — the restriction belongs to a
    /// RESOLVED instance, not to an unknown ARN. Every operation of the surface asks
    /// this BEFORE looking a permission set or an assignment up, because the refusal
    /// does not depend on whether that resource exists: on an account instance none ever
    /// can.
    pub fn permission_set_surface_withdrawn(&self, instance_arn: &str) -> bool {
        self.instance(instance_arn)
            .is_some_and(|instance| !instance.permission_sets_enabled)
    }

    /// The instance a previous `CreateInstance` minted under `client_token`, if that
    /// token was ever used. A retry of the same call answers from it instead of being
    /// refused as a duplicate, which is what `@idempotencyToken` asks of the member.
    pub fn instance_by_client_token(&self, client_token: &str) -> Option<&Instance> {
        self.instances.values().find(|instance| {
            instance
                .client_token
                .as_deref()
                .is_some_and(|stored| stored == client_token)
        })
    }

    /// The permission set `permission_set_arn` names WITHIN `instance_arn`.
    ///
    /// A permission set belongs to exactly one instance, so an ARN that exists under a
    /// different instance is as absent here as one that was never created.
    pub fn permission_set(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Option<&PermissionSetRecord> {
        self.permission_sets
            .get(permission_set_arn)
            .filter(|record| record.instance_arn == instance_arn)
    }

    /// The same permission set, for an operation that CHANGES it (attaching a policy or
    /// a permissions boundary, say).
    ///
    /// Resolves exactly as [`Self::permission_set`] — a record held under another
    /// instance is as absent here as one that was never created — so a write and a read
    /// of the same ARN never disagree about which permission set it names.
    pub fn permission_set_mut(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Option<&mut PermissionSetRecord> {
        self.permission_sets
            .get_mut(permission_set_arn)
            .filter(|record| record.instance_arn == instance_arn)
    }

    /// Whether `name` is already taken by a permission set of `instance_arn`.
    pub fn permission_set_name_taken(&self, instance_arn: &str, name: &str) -> bool {
        self.permission_sets
            .values()
            .any(|record| record.instance_arn == instance_arn && record.name == name)
    }

    /// THE resolution the whole permission-set / account-assignment surface shares, for an
    /// operation whose request names an instance and nothing finer: the instance that
    /// request runs against, or the refusal it earns instead.
    ///
    /// The two steps, in this order and for the same reason every operation of the surface
    /// takes them:
    ///
    /// 1. the ACCOUNT-INSTANCE restriction, asked BEFORE any resource lookup — permission
    ///    sets and the assignments built from them are a feature of an organization
    ///    instance, so on an account instance none can ever exist and the refusal does not
    ///    depend on whether the resource the request names does
    ///    ([`Self::permission_set_surface_withdrawn`], [`ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`]);
    /// 2. the instance itself — a syntactically valid `InstanceArn` this account holds no
    ///    instance for is a missing resource, not a bad request
    ///    ([`instance_not_found_message`]), and it is NOT the restriction above, which
    ///    belongs to a RESOLVED instance.
    ///
    /// Answers the stored instance, so a handler that needs its canonical ARN or its
    /// fields reads them from the record the resolution already resolved.
    pub fn permission_set_surface(
        &self,
        instance_arn: &str,
    ) -> Result<&Instance, PermissionSetSurfaceRefusal> {
        self.permission_set_surface_gate(instance_arn)
            .map_err(PermissionSetSurfaceRefusal::Unsupported)?;
        self.instance(instance_arn).ok_or_else(|| {
            PermissionSetSurfaceRefusal::NotFound(instance_not_found_message(instance_arn))
        })
    }

    /// Step 1 of that resolution ON ITS OWN, as the message of the `ValidationException`
    /// the caller models — for an operation whose own request-consistency check sits
    /// between the two steps (the ABAC writes, which refuse a mapping that gives one key
    /// two values once the surface is known to serve them at all, and before the instance
    /// holding the mapping is resolved).
    ///
    /// This step has ONE refusal, so it answers that message directly and needs no refusal
    /// enum — the division [`TagRefusal`] keeps for the tag surface, whose calls can be
    /// refused two ways. The same rule either way: [`Self::permission_set_surface`] asks
    /// exactly this first, so the gate has one implementation and an operation that takes
    /// both steps together cannot answer it differently from one that takes them apart.
    pub fn permission_set_surface_gate(&self, instance_arn: &str) -> Result<(), String> {
        if self.permission_set_surface_withdrawn(instance_arn) {
            return Err(ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE.to_string());
        }
        Ok(())
    }

    /// The same resolution for an operation that CHANGES the instance it resolves — the
    /// attribute-based access control (ABAC) writes, whose resource lives ON the instance
    /// record.
    ///
    /// ABAC maps identity-store attributes into the session tags that the policies of
    /// multi-account AWS access are evaluated against (`AccessControlAttribute`: "You can
    /// create permissions policies that determine who can access your Amazon Web Services
    /// resources based upon the configured attribute values"), so it is a feature of the
    /// SAME organization-instance surface as permission sets and the account assignments
    /// built from them — which is why its operations take exactly the two steps of
    /// [`Self::permission_set_surface`] rather than a rule of their own, and why the ABAC
    /// read resolves through that function itself. Resolves exactly as it does, so a read
    /// and a write of the same ARN never disagree about which instance it names.
    pub fn abac_surface_mut(
        &mut self,
        instance_arn: &str,
    ) -> Result<&mut Instance, PermissionSetSurfaceRefusal> {
        self.permission_set_surface(instance_arn)?;
        self.instance_mut(instance_arn).ok_or_else(|| {
            PermissionSetSurfaceRefusal::NotFound(instance_not_found_message(instance_arn))
        })
    }

    /// The same resolution for an operation whose request also names a PERMISSION SET: the
    /// stored permission set, having taken the two steps of [`Self::permission_set_surface`]
    /// first, or the refusal the request earns instead.
    ///
    /// The third step is the permission set itself, resolved as [`Self::permission_set`]
    /// resolves it: never created, already deleted, or held under another instance is the
    /// same answer either way ([`permission_set_not_found_message`]).
    pub fn permission_set_target(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Result<&PermissionSetRecord, PermissionSetSurfaceRefusal> {
        self.permission_set_surface(instance_arn)?;
        self.permission_set(instance_arn, permission_set_arn)
            .ok_or_else(|| {
                PermissionSetSurfaceRefusal::NotFound(permission_set_not_found_message(
                    permission_set_arn,
                ))
            })
    }

    /// The same resolution for an operation that CHANGES the permission set (attaching a
    /// policy or a permissions boundary, say). Resolves exactly as
    /// [`Self::permission_set_target`], so a write and a read of the same ARNs never
    /// disagree about which surface and which permission set they name.
    pub fn permission_set_target_mut(
        &mut self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Result<&mut PermissionSetRecord, PermissionSetSurfaceRefusal> {
        self.permission_set_surface(instance_arn)?;
        self.permission_set_mut(instance_arn, permission_set_arn)
            .ok_or_else(|| {
                PermissionSetSurfaceRefusal::NotFound(permission_set_not_found_message(
                    permission_set_arn,
                ))
            })
    }

    /// The trusted token issuer `trusted_token_issuer_arn` names, or `None` when no
    /// such issuer exists.
    pub fn trusted_token_issuer(
        &self,
        trusted_token_issuer_arn: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers.get(trusted_token_issuer_arn)
    }

    /// The trusted token issuer `trusted_token_issuer_arn` names, for a handler that
    /// CHANGES it (an `UpdateTrustedTokenIssuer` writing its name or the attribute paths
    /// of its configuration). Resolves exactly as [`Self::trusted_token_issuer`], so the
    /// read and the write never disagree about which issuer an ARN names.
    pub fn trusted_token_issuer_mut(
        &mut self,
        trusted_token_issuer_arn: &str,
    ) -> Option<&mut TrustedTokenIssuerRecord> {
        self.trusted_token_issuers.get_mut(trusted_token_issuer_arn)
    }

    /// The trusted token issuer a previous `CreateTrustedTokenIssuer` minted under
    /// `client_token`, if one still exists.
    pub fn trusted_token_issuer_for_client_token(
        &self,
        client_token: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The application `application_arn` names, or `None` when no such application
    /// exists — never created, or created under an instance that has since gone.
    pub fn application(&self, application_arn: &str) -> Option<&ApplicationRecord> {
        self.applications.get(application_arn)
    }

    /// The application `application_arn` names, for a handler that updates it (a
    /// `PutApplicationAccessScope` writing one of its access scopes, or a
    /// `DeleteApplicationGrant` removing one of its grants, say).
    pub fn application_mut(&mut self, application_arn: &str) -> Option<&mut ApplicationRecord> {
        self.applications.get_mut(application_arn)
    }

    /// The application a previous `CreateApplication` minted under `client_token`, if
    /// one still exists.
    pub fn application_for_client_token(&self, client_token: &str) -> Option<&ApplicationRecord> {
        self.applications
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The index of the application assignment with this identity tuple, if it exists.
    pub fn application_assignment_position(
        &self,
        application_arn: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> Option<usize> {
        self.application_assignments.iter().position(|row| {
            row.application_arn == application_arn
                && row.principal_type == principal_type
                && row.principal_id == principal_id
        })
    }

    /// The TAGGABLE resource `resource_arn` names: the ARN of the instance it belongs to
    /// (itself, for an instance) and the tag set it carries.
    ///
    /// The model's `TaggableResourceArn` admits four kinds of resource — an instance, a
    /// permission set, an application and a trusted token issuer — and each is stored
    /// under its own ARN, so the ARN alone says which record carries the tags. `None` for
    /// an ARN this account holds no resource under, including one whose instance has
    /// since gone (an instance holds its resources, see [`Self::remove_instance`]).
    fn taggable_resource(&self, resource_arn: &str) -> Option<(&str, &Vec<Tag>)> {
        if let Some(instance) = self.instances.get(resource_arn) {
            return Some((instance.instance_arn.as_str(), &instance.tags));
        }
        if let Some(record) = self.permission_sets.get(resource_arn) {
            return Some((record.instance_arn.as_str(), &record.tags));
        }
        if let Some(record) = self.applications.get(resource_arn) {
            return Some((record.instance_arn.as_str(), &record.tags));
        }
        self.trusted_token_issuers
            .get(resource_arn)
            .map(|record| (record.instance_arn.as_str(), &record.tags))
    }

    /// The INSTANCE that holds the taggable resource `resource_arn` names, or `None` for
    /// an ARN this account holds no resource under.
    ///
    /// What [`Self::tag_holder_ready`] asks to reach the status of the instance the
    /// resource lives under ([`Self::instance_creation_in_progress`]): an instance is
    /// itself taggable, so
    /// this answers the instance's own ARN for it and the holding instance's for a
    /// permission set, an application or a trusted token issuer.
    fn taggable_resource_instance(&self, resource_arn: &str) -> Option<&str> {
        self.taggable_resource(resource_arn).map(|(owner, _)| owner)
    }

    /// The tags the taggable resource `resource_arn` names carries — the resolution step
    /// of [`Self::tag_target`], which is how a handler reaches it.
    ///
    /// `instance_arn` is the request's optional `InstanceArn`: when a caller sends one, a
    /// resource held under a DIFFERENT instance is as absent here as one that was never
    /// created — the same rule [`Self::permission_set`] resolves a permission set by — so
    /// the read and the write below never disagree about which resource an ARN names.
    fn taggable_resource_tags(
        &self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Option<&Vec<Tag>> {
        let (owner, tags) = self.taggable_resource(resource_arn)?;
        instance_arn.is_none_or(|arn| arn == owner).then_some(tags)
    }

    /// The same tag set for a handler that CHANGES it — the resolution step of
    /// [`Self::tag_target_mut`]. Resolves exactly as [`Self::taggable_resource_tags`].
    fn taggable_resource_tags_mut(
        &mut self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Option<&mut Vec<Tag>> {
        let owner = self.taggable_resource(resource_arn)?.0.to_string();
        if instance_arn.is_some_and(|arn| arn != owner) {
            return None;
        }
        if let Some(instance) = self.instances.get_mut(resource_arn) {
            return Some(&mut instance.tags);
        }
        if let Some(record) = self.permission_sets.get_mut(resource_arn) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.applications.get_mut(resource_arn) {
            return Some(&mut record.tags);
        }
        self.trusted_token_issuers
            .get_mut(resource_arn)
            .map(|record| &mut record.tags)
    }

    /// THE resolution the whole tag surface shares, for an operation that REPORTS the tag
    /// set: the tags of the resource a tag request names, or the refusal it earns instead.
    ///
    /// `instance_arn` is the request's optional `InstanceArn`. When a caller sends one it
    /// names the instance the resource is expected to live under, so an ARN this account
    /// holds no instance for can hold no resource either
    /// ([`instance_not_found_message`]), and a resource held under a DIFFERENT instance is
    /// as absent as one that was never created. Never created, already deleted, gone with
    /// its instance, or held elsewhere — the same answer either way
    /// ([`taggable_resource_not_found_message`]). A resource that exists and carries no
    /// tag is an EMPTY set, which is a different answer from that refusal.
    ///
    /// And the instance that HOLDS the resource decides whether this call is available at
    /// all, exactly as it does for the two writes below ([`Self::tag_holder_ready`]): an
    /// instance still being created serves only its own describe and delete, and a tag
    /// report is neither.
    pub fn tag_target(
        &self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Result<&Vec<Tag>, TagRefusal> {
        self.tag_request_instance_scope(instance_arn)
            .map_err(TagRefusal::NotFound)?;
        self.tag_holder_ready(resource_arn)
            .map_err(TagRefusal::Conflict)?;
        self.taggable_resource_tags(instance_arn, resource_arn)
            .ok_or_else(|| TagRefusal::NotFound(tag_report_not_found_message(resource_arn)))
    }

    /// The same resolution for an operation that CHANGES the tag set. Takes exactly the
    /// steps the report takes, in the same order, so a write and a report of the same ARNs
    /// never disagree about which resource they name or about whether the instance holding
    /// it serves them at all.
    pub fn tag_target_mut(
        &mut self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Result<&mut Vec<Tag>, TagRefusal> {
        self.tag_request_instance_scope(instance_arn)
            .map_err(TagRefusal::NotFound)?;
        self.tag_holder_ready(resource_arn)
            .map_err(TagRefusal::Conflict)?;
        self.taggable_resource_tags_mut(instance_arn, resource_arn)
            .ok_or_else(|| {
                TagRefusal::NotFound(taggable_resource_not_found_message(resource_arn))
            })
    }

    /// The optional `InstanceArn` a tag request may carry: it names the instance the
    /// resource is expected to live under, and an ARN this account holds no instance for
    /// can hold no resource either.
    fn tag_request_instance_scope(&self, instance_arn: Option<&str>) -> Result<(), String> {
        match instance_arn.filter(|arn| self.instance(arn).is_none()) {
            Some(arn) => Err(instance_not_found_message(arn)),
            None => Ok(()),
        }
    }

    /// Whether the instance holding the named resource is far enough along to serve a tag
    /// call at all, as the `Err` message of the refusal it earns while it is not.
    ///
    /// The instance's `CREATE_IN_PROGRESS` window, asked through the gate every
    /// instance-scoped operation of this service asks
    /// ([`Self::instance_creation_in_progress_gate`]) and about the HOLDER rather than
    /// about the request's optional `InstanceArn`: a resource is withheld by the instance
    /// it really lives under, whether or not the caller named it. An instance is itself
    /// taggable, so for an instance ARN the holder is the instance itself.
    fn tag_holder_ready(&self, resource_arn: &str) -> Result<(), String> {
        match self.taggable_resource_instance(resource_arn) {
            Some(holder) => self.instance_creation_in_progress_gate(holder),
            None => Ok(()),
        }
    }

    /// Store the status of one assignment deletion, evicting the OLDEST records while
    /// the history is over [`MAX_RECORDS_PER_STORE`].
    ///
    /// A deletion is applied before this is called, so the history cannot refuse it: the
    /// emulator's memory ceiling is kept by dropping the oldest statuses instead, which
    /// is why this store is the one caller-driven store that evicts rather than refusing
    /// (a creation status belongs to a live assignment, so it is bounded by the
    /// assignments themselves).
    pub fn record_assignment_deletion(&mut self, record: AssignmentOperationRecord) {
        self.assignment_deletions.push(record);
        let excess = self
            .assignment_deletions
            .len()
            .saturating_sub(MAX_RECORDS_PER_STORE);
        self.assignment_deletions.drain(..excess);
    }

    /// The stored status of the assignment deletion `request_id` names WITHIN
    /// `instance_arn`.
    ///
    /// Scoped to the instance exactly as [`Self::assignment_operation`] is: a `RequestId`
    /// of another instance's deletion is as absent here as one that never ran. And absent
    /// once the permission set it reports on has gone, exactly as
    /// [`Self::permission_set_provisioning`] is: a status exists to report work on one
    /// permission set, so an id whose permission set no reader can see is as absent as one
    /// that was never minted.
    pub fn assignment_deletion(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&AssignmentOperationRecord> {
        self.assignment_deletions
            .iter()
            .find(|record| record.instance_arn == instance_arn && record.request_id == request_id)
            .filter(|record| {
                self.permission_set(instance_arn, &record.permission_set_arn)
                    .is_some()
            })
    }

    /// Every assignment deletion this instance TRACKS, in the order the history happened
    /// ([`AssignmentOperationRecord::page_key`] is that same order).
    ///
    /// A record is tracked under exactly the condition [`Self::assignment_deletion`]
    /// resolves one under — it is delegated to, so there is ONE rule — hence an
    /// enumeration of this history and a describe of one of its ids cannot disagree about
    /// which ids exist.
    pub fn tracked_assignment_deletions(
        &self,
        instance_arn: &str,
    ) -> Vec<&AssignmentOperationRecord> {
        self.assignment_deletions
            .iter()
            .filter(|record| {
                self.assignment_deletion(instance_arn, &record.request_id)
                    .is_some()
            })
            .collect()
    }

    /// Whether any stored assignment operation — a creation status or a deletion status —
    /// already carries `request_id`, so a freshly minted one is never reused for a second
    /// operation.
    pub fn assignment_request_id_taken(&self, request_id: &str) -> bool {
        self.assignment_operations.contains_key(request_id)
            || self
                .assignment_deletions
                .iter()
                .any(|record| record.request_id == request_id)
    }

    /// The index of the assignment with this identity tuple, if it exists.
    pub fn assignment_position(
        &self,
        instance_arn: &str,
        account_id: &str,
        permission_set_arn: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> Option<usize> {
        self.account_assignments.iter().position(|row| {
            row.instance_arn == instance_arn
                && row.account_id == account_id
                && row.permission_set_arn == permission_set_arn
                && row.principal_type == principal_type
                && row.principal_id == principal_id
        })
    }

    /// Every account the permission set `permission_set_arn` names is PROVISIONED into,
    /// with the provisioning status each of them is in, keyed by account id.
    ///
    /// ONE projection of the relation [`Self::provisioned_copies_held`] expresses — the
    /// accounts axis — so an account appears at most once, whatever reached a copy to it
    /// and however many principals were assigned through it, and the copy reported is the
    /// one the LAST provisioning put there. A permission set no instance holds under
    /// `instance_arn` is provisioned nowhere, so the answer is empty.
    pub fn provisioned_accounts(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> BTreeMap<String, &'static str> {
        self.provisioned_copies_held(instance_arn)
            .into_iter()
            .filter(|held| held.permission_set_arn == permission_set_arn)
            .map(|held| (held.account_id.to_string(), held.status))
            .collect()
    }

    /// Every permission set PROVISIONED into the account `account_id` names under
    /// `instance_arn`, with the provisioning status each of them is in, keyed by the
    /// permission set's ARN.
    ///
    /// The OTHER projection of the relation [`Self::provisioned_copies_held`] expresses,
    /// by the axis this caller asked for: the same records, in the same order, under the
    /// same rule and with the status computed in the same one place, so the two readers
    /// cannot disagree about a copy. A row whose permission set this instance no longer
    /// holds is provisioned nowhere, so it is left out.
    pub fn provisioned_permission_sets(
        &self,
        instance_arn: &str,
        account_id: &str,
    ) -> BTreeMap<String, &'static str> {
        self.provisioned_copies_held(instance_arn)
            .into_iter()
            .filter(|held| held.account_id == account_id)
            .map(|held| (held.permission_set_arn.to_string(), held.status))
            .collect()
    }

    /// The ONE expression of "which copy of a permission set an account holds" under
    /// `instance_arn`: every (permission set, account) pair a provisioning has put a copy
    /// into, with the status that copy is in.
    ///
    /// An account holds a permission set once it has been provisioned there — which both
    /// granting an account assignment on that account and provisioning the permission set
    /// into it do — however many principals were assigned through it. Both ways a copy
    /// reaches an account are walked here, the copies a provisioning delivered to the
    /// accounts it targeted ([`Self::provisioned_copies`]) and the revision each assignment
    /// granted on an account recorded ([`AccountAssignmentRecord::provisioned_revision`]),
    /// and the copy a pair is yielded with is the one whose definition was captured LAST
    /// ([`is_not_older_than_recorded_copy`]) — the copy the last provisioning of that account put
    /// there, whether that provisioning was a grant or a `ProvisionPermissionSet` call. A
    /// pair is therefore yielded exactly once, so either projection of this relation reports
    /// an account, or a permission set, at most once.
    ///
    /// The status is COMPUTED, in the one place that comparison is made
    /// ([`provisioning_status_of`]), against the revision of each permission set as it
    /// stands now: equal means the account holds the latest definition
    /// ([`PROVISIONING_STATUS_LATEST_PROVISIONED`]), and anything else means the
    /// permission set has changed since
    /// ([`PROVISIONING_STATUS_LATEST_NOT_PROVISIONED`]). A copy of a permission set this
    /// instance no longer holds is a copy of nothing, so it is left out — which is what
    /// makes both projections empty for a permission set, or an account, the instance
    /// holds nothing for.
    fn provisioned_copies_held(&self, instance_arn: &str) -> Vec<HeldCopy<'_>> {
        let copies = self
            .provisioned_copies
            .iter()
            .filter(|copy| copy.instance_arn == instance_arn)
            .map(|copy| {
                (
                    (copy.permission_set_arn.as_str(), copy.account_id.as_str()),
                    (&copy.provisioned_at, copy.revision.as_str()),
                )
            });
        let rows = self
            .account_assignments
            .iter()
            .filter(|row| row.instance_arn == instance_arn)
            .map(|row| {
                (
                    (row.permission_set_arn.as_str(), row.account_id.as_str()),
                    (&row.provisioned_at, row.provisioned_revision.as_str()),
                )
            });
        // The copy IN PLACE for each pair: the one the last provisioning of that account
        // recorded, which the capture instant beside each recorded revision decides.
        let mut in_place: BTreeMap<(&str, &str), (&DateTime, &str)> = BTreeMap::new();
        for (pair, recorded) in copies.chain(rows) {
            let keep = in_place
                .get(&pair)
                .is_none_or(|held| is_not_older_than_recorded_copy(recorded.0, held.0));
            if keep {
                in_place.insert(pair, recorded);
            }
        }
        // Each permission set's current revision, derived at most once however many
        // records provisioned it, and absent for one this instance no longer holds.
        let mut revisions: BTreeMap<&str, Option<String>> = BTreeMap::new();
        in_place
            .into_iter()
            .filter_map(
                |((permission_set_arn, account_id), (_, recorded_revision))| {
                    let current = revisions.entry(permission_set_arn).or_insert_with(|| {
                        self.permission_set(instance_arn, permission_set_arn)
                            .map(|permission_set| permission_set.definition_revision())
                    });
                    Some(HeldCopy {
                        permission_set_arn,
                        account_id,
                        status: provisioning_status_of(recorded_revision, current.as_deref()?),
                    })
                },
            )
            .collect()
    }

    /// The status record of the assignment operation `request_id` names, WITHIN
    /// `instance_arn`.
    ///
    /// An assignment operation is performed under one instance, and the record carries
    /// it, so a `RequestId` minted under another instance is as absent here as one
    /// nothing was ever minted for — which is what keeps a `Describe…Status` read of one
    /// instance from reporting another instance's history. A record whose permission set
    /// has gone is absent too, because the delete removes it with the assignments it
    /// reported on ([`Self::remove_instance`], `DeletePermissionSet`).
    pub fn assignment_operation(
        &self,
        instance_arn: &str,
        request_id: &str,
    ) -> Option<&AssignmentOperationRecord> {
        self.assignment_operations
            .get(request_id)
            .filter(|record| record.instance_arn == instance_arn)
    }
}

/// One of the application providers IAM Identity Center publishes ITSELF.
///
/// An application provider is not a caller-created resource: the model's
/// `ApplicationProviderArn` pattern pins the owner segment to `aws`
/// (`^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$`), so every
/// provider belongs to AWS and no operation of this service creates or deletes one.
/// They are therefore a FIXED catalog ([`BUILT_IN_APPLICATION_PROVIDERS`]) rather than a
/// store: `DescribeApplicationProvider` reports an entry of it and refuses every other
/// ARN, and `ListApplicationProviders` enumerates the same entries, through
/// [`BuiltInApplicationProvider::to_shape`], so the two agree member for member.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInApplicationProvider {
    /// Everything after `applicationProvider/` in the provider's ARN: the service slug of
    /// an AWS managed provider (`glue`), or `catalog/<Name>` for one of the pre-integrated
    /// applications of the IAM Identity Center application catalog. The model's
    /// `ApplicationProviderArn` pattern admits `/` inside the id
    /// (`applicationProvider/[a-zA-Z0-9-/]+`), which is what makes the `catalog/` form an
    /// id and not a second path segment.
    pub id: &'static str,
    /// `ApplicationProvider$FederationProtocol` — "The protocol that the application
    /// provider uses to perform federation", one of the model's `FederationProtocol`
    /// enum values `SAML | OAUTH`.
    pub federation_protocol: &'static str,
    /// Whether the applications under this provider are managed by the CUSTOMER — set up
    /// through this API — rather than by Amazon Web Services on the customer's behalf.
    ///
    /// True for the custom application provider alone: an application of an AWS managed
    /// provider is created by the Amazon Web Services service that owns it, and a
    /// pre-integrated catalog application is set up in the console. This is what
    /// [`BuiltInApplicationProvider::accepts_customer_managed_application`] reads, and it
    /// is NOT reported on the wire — no member of the modelled `ApplicationProvider`
    /// carries it.
    pub customer_managed: bool,
    /// `DisplayData$DisplayName` — "The name of the application provider that appears in
    /// the portal" (`Name`, 1..100).
    pub display_name: &'static str,
    /// The access scopes `ApplicationProvider$ResourceServerConfig` reports for this
    /// provider — empty for a provider that exposes no resource server, which is then
    /// reported with no `ResourceServerConfig` at all rather than an empty one.
    pub scopes: &'static [BuiltInAccessScope],
}

/// One IAM Identity Center access scope of a provider's resource server, as
/// `ResourceServerConfig$Scopes` reports it: the scope NAME is the map key and the two
/// `ResourceServerScopeDetails` members (both 1..128) its value.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInAccessScope {
    /// The map key: `<service>:<scope>`, the form IAM Identity Center publishes an access
    /// scope under (the recorded catalog carries `glue:read_write` on the Glue provider).
    pub name: &'static str,
    /// `ResourceServerScopeDetails$DetailedTitle` — "The title of an access scope for a
    /// resource server".
    pub detailed_title: &'static str,
    /// `ResourceServerScopeDetails$LongDescription` — "The description of an access scope
    /// for a resource server".
    pub long_description: &'static str,
}

impl BuiltInAccessScope {
    /// This scope as the modeled `ResourceServerScopeDetails` shape.
    pub fn to_shape(&self) -> ResourceServerScopeDetails {
        ResourceServerScopeDetails {
            detailed_title: Some(self.detailed_title.to_string()),
            long_description: Some(self.long_description.to_string()),
        }
    }
}

/// The host every provider icon of the IAM Identity Center portal is served from.
///
/// SOURCE: recorded behavior — a catalog provider's `DisplayData$IconUrl` is an `https`
/// URL under this static asset host (`https://<host>/app-<digest>/icons/default.png`).
/// The digest in a recorded URL is environment detail, so it is DERIVED from the provider
/// id here (see [`BuiltInApplicationProvider::icon_url`]) rather than copied.
const PROVIDER_ICON_HOST: &str = "static.global.sso.amazonaws.com";

/// The portal description of an application provider: its display name followed by this
/// word.
///
/// SOURCE: recorded behavior — a provider's `DisplayData$Description` is its
/// `DisplayName` as an application, for EVERY provider of the catalog and not only the
/// pre-integrated ones: the entry whose display name is `Statuspage` carries
/// `Statuspage Application`, and the custom provider's own description is its display
/// name followed by the same word. The description is therefore composed from the
/// display name ([`BuiltInApplicationProvider::description`]) rather than carried per
/// entry.
const PROVIDER_DESCRIPTION_SUFFIX: &str = "Application";

/// `FederationProtocol::SAML`, the protocol of a pre-integrated catalog application: the
/// IAM Identity Center application catalog is a catalog of SAML 2.0 applications.
const FEDERATION_SAML: &str = "SAML";

/// `FederationProtocol::OAUTH`, the protocol of an AWS managed application provider that
/// receives a propagated identity (trusted identity propagation).
const FEDERATION_OAUTH: &str = "OAUTH";

impl BuiltInApplicationProvider {
    /// This provider's `DisplayData`, complete: every member the shape declares is
    /// reported, because every provider of the catalog HAS all three — the display name
    /// from the record, the description composed from it, the icon URL derived from the
    /// provider id.
    pub fn display_data(&self) -> DisplayData {
        DisplayData {
            display_name: Some(self.display_name.to_string()),
            icon_url: Some(self.icon_url()),
            description: Some(self.description()),
        }
    }

    /// This provider's `DisplayData$Description`: its display name as an application
    /// ([`PROVIDER_DESCRIPTION_SUFFIX`]) — the one rule every entry of the catalog
    /// follows, whichever of the two id forms it carries. Truncated on a character
    /// boundary to the modelled `Description` ceiling of 128, so no display name the
    /// modelled `Name` (1..100) admits can put an over-long value on the wire.
    pub fn description(&self) -> String {
        let description = format!("{} {PROVIDER_DESCRIPTION_SUFFIX}", self.display_name);
        truncate_chars(&description, 128)
    }

    /// This provider's `DisplayData$IconUrl`:
    /// `https://<icon host>/app-<digest>/icons/default.png`.
    ///
    /// The digest is the first 16 hex digits of the SHA-256 of the provider ID — derived
    /// from the provider, so it is stable across reads and distinct per provider, and
    /// carries nothing from the request. The URL satisfies the modelled `IconUrl`
    /// (`^(http|https)://.*$`, 1..768).
    pub fn icon_url(&self) -> String {
        use ::sha2::Digest as _;
        let digest = ::hex::encode(::sha2::Sha256::digest(self.id.as_bytes()));
        format!(
            "https://{PROVIDER_ICON_HOST}/app-{}/icons/default.png",
            &digest[..16]
        )
    }

    /// This provider's `ResourceServerConfig`, or `None` when it exposes no resource
    /// server: an empty `Scopes` map would claim a resource server with no scopes, which
    /// is a different statement from having none.
    pub fn resource_server_config(&self) -> Option<ResourceServerConfig> {
        if self.scopes.is_empty() {
            return None;
        }
        Some(ResourceServerConfig {
            scopes: Some(
                self.scopes
                    .iter()
                    .map(|scope| (scope.name.to_string(), scope.to_shape()))
                    .collect(),
            ),
        })
    }

    /// This provider's ARN in the `aws` partition.
    ///
    /// `arn:aws:sso::aws:applicationProvider/<id>` — the provider ARN carries no region
    /// and the literal `aws` where a resource of the caller's would carry an account, so
    /// it is not built through `engine::emulator_arn`. The same provider exists in every
    /// partition the pattern admits (which is why
    /// [`built_in_application_provider`] matches on the id alone); an ENUMERATION has to
    /// report one ARN per entry, and it reports it in the partition the emulator serves.
    pub fn arn(&self) -> String {
        format!("arn:aws:sso::aws:applicationProvider/{}", self.id)
    }

    /// Whether an application can be CREATED under this provider through this service's
    /// API.
    ///
    /// `CreateApplication` makes an OAuth 2.0 customer managed application, so a provider
    /// backs one only when the customer manages the applications under it AND it federates
    /// over OAuth 2.0: an application whose provider federates over SAML 2.0 is set up in
    /// the console instead, and an application of an Amazon Web Services managed provider
    /// is created by the service that owns it on the customer's behalf. Every other
    /// operation of the provider surface is unaffected — a provider that backs no create
    /// is still described and enumerated like any other entry of the catalog.
    pub fn accepts_customer_managed_application(&self) -> bool {
        self.customer_managed && self.federation_protocol == FEDERATION_OAUTH
    }

    /// This provider as the modeled `ApplicationProvider` shape — the member-for-member
    /// projection `ListApplicationProviders` reports, holding exactly what
    /// `DescribeApplicationProvider` reports for the same entry, under this provider's ARN
    /// in the `aws` partition.
    pub fn to_shape(&self) -> ApplicationProvider {
        self.to_shape_under_arn(self.arn())
    }

    /// This provider as the modeled `ApplicationProvider` shape, reported under
    /// `application_provider_arn`.
    ///
    /// The same provider exists in every partition the ARN pattern admits
    /// ([`built_in_application_provider`] matches on the id alone), so a read that was
    /// ASKED about one partition reports the entry under the ARN it was asked about; an
    /// enumeration, which is asked about no ARN at all, reports [`Self::arn`].
    pub fn to_shape_under_arn(&self, application_provider_arn: String) -> ApplicationProvider {
        ApplicationProvider {
            application_provider_arn,
            federation_protocol: Some(self.federation_protocol.to_string()),
            display_data: Some(self.display_data()),
            resource_server_config: self.resource_server_config(),
        }
    }
}

/// `text`, cut to at most `max_chars` characters on a character boundary.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// One AWS MANAGED application provider: an Amazon Web Services application that receives
/// a propagated identity from IAM Identity Center, so it federates over OAuth 2.0 and
/// publishes — where it has one — its resource server's access scopes.
///
/// The applications under such a provider are created by the Amazon Web Services service
/// that owns it, not by the customer, so none of them is customer managed.
const fn aws_managed_provider(
    id: &'static str,
    display_name: &'static str,
    scopes: &'static [BuiltInAccessScope],
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_OAUTH,
        customer_managed: false,
        display_name,
        scopes,
    }
}

/// The CUSTOM application provider: the one provider whose applications the CUSTOMER
/// creates and manages, over OAuth 2.0 — the provider `CreateApplication` mints an
/// application under ([`BuiltInApplicationProvider::accepts_customer_managed_application`]).
const fn customer_managed_provider(
    id: &'static str,
    display_name: &'static str,
    scopes: &'static [BuiltInAccessScope],
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_OAUTH,
        customer_managed: true,
        display_name,
        scopes,
    }
}

/// One PRE-INTEGRATED application of the IAM Identity Center application catalog: a SAML
/// 2.0 application whose ARN id is `catalog/<Name>` and which exposes no resource server
/// of its own (the application is the relying party, not an API an identity propagates
/// into).
const fn catalog_application(
    id: &'static str,
    display_name: &'static str,
) -> BuiltInApplicationProvider {
    BuiltInApplicationProvider {
        id,
        federation_protocol: FEDERATION_SAML,
        // A pre-integrated application is set up in the console, not through this API.
        customer_managed: false,
        display_name,
        scopes: &[],
    }
}

/// The built-in application providers of IAM Identity Center.
///
/// SOURCE: the Smithy model carries no list of them — `ApplicationProviderArn` is only a
/// pattern (`smithy_query(query="shape",shape="ApplicationProviderArn")`) and
/// `ApplicationProvider` only the per-entry structure — so the catalog is written out
/// here, from the sets IAM Identity Center publishes:
///
/// * the CUSTOM provider ([`customer_managed_provider`]) — the one provider a customer
///   managed application is created under;
/// * the AWS MANAGED providers ([`aws_managed_provider`]) — the Amazon Web Services
///   applications that receive a propagated identity (trusted identity propagation),
///   whose id is the service slug its `arn:aws:sso::aws:applicationProvider/<slug>` ARN
///   carries, and whose published access scopes are reported as its resource server;
/// * the APPLICATION CATALOG ([`catalog_application`]) — the pre-integrated SAML 2.0
///   applications of the IAM Identity Center application catalog, whose id is
///   `catalog/<Name>` (recorded behavior: the recorded catalog holds entries such as
///   `catalog/Statuspage`, `catalog/DuoNetworkGateway` and `catalog/DruvaInSync`
///   alongside service providers such as `glue`).
///
/// The MEMBERSHIP is behavior, not model: AWS adds applications to its catalog over time,
/// so this is the catalog the emulator publishes rather than a list any rule derives, and
/// the ORDER is the order written here — a service-published catalog's order is its own
/// data, which is why no read of it sorts.
/// What IS a rule — every entry carries a complete `DisplayData` whose `Description` is
/// its display name as an application ([`BuiltInApplicationProvider::description`]), an id
/// of one of the two forms above, and a `ResourceServerConfig` exactly when it publishes
/// access scopes — holds for every entry here, and for any entry added to it.
///
/// A provider exists in every partition the ARN pattern admits, so the lookup
/// ([`built_in_application_provider`]) matches on the id and not on the whole ARN.
pub const BUILT_IN_APPLICATION_PROVIDERS: &[BuiltInApplicationProvider] = &[
    // ---- The custom (customer managed) provider ---------------------------------------
    customer_managed_provider("custom", "SSO Custom Application Provider", &[]),
    // ---- AWS managed providers -------------------------------------------------------
    aws_managed_provider("athena", "Amazon Athena", &[]),
    aws_managed_provider("emr", "Amazon EMR", &[]),
    aws_managed_provider("emr-serverless", "Amazon EMR Serverless", &[]),
    aws_managed_provider(
        "glue",
        "AWS Glue",
        &[BuiltInAccessScope {
            name: "glue:read_write",
            detailed_title: "Read and write AWS Glue data",
            long_description: "Grants access to read and write data through AWS Glue",
        }],
    ),
    aws_managed_provider(
        "lakeformation",
        "AWS Lake Formation",
        &[BuiltInAccessScope {
            name: "lakeformation:query",
            detailed_title: "Query Lake Formation data",
            long_description: "Grants access to query data governed by AWS Lake Formation",
        }],
    ),
    aws_managed_provider("quicksight", "Amazon QuickSight", &[]),
    aws_managed_provider(
        "redshift",
        "Amazon Redshift",
        &[BuiltInAccessScope {
            name: "redshift:connect",
            detailed_title: "Connect to Amazon Redshift",
            long_description: "Grants access to connect to an Amazon Redshift database",
        }],
    ),
    aws_managed_provider(
        "s3",
        "Amazon S3",
        &[BuiltInAccessScope {
            name: "s3:access_grants:read_write",
            detailed_title: "Read and write Amazon S3 data",
            long_description: "Grants access to read and write Amazon S3 data through access grants",
        }],
    ),
    // ---- The application catalog -----------------------------------------------------
    catalog_application("catalog/15Five", "15Five"),
    catalog_application("catalog/AbsorbLMS", "Absorb LMS"),
    catalog_application("catalog/Adobe", "Adobe"),
    catalog_application("catalog/Airtable", "Airtable"),
    catalog_application("catalog/Asana", "Asana"),
    catalog_application("catalog/AtlassianCloud", "Atlassian Cloud"),
    catalog_application("catalog/BambooHR", "BambooHR"),
    catalog_application("catalog/BetterCloud", "BetterCloud"),
    catalog_application("catalog/Bonusly", "Bonusly"),
    catalog_application("catalog/Box", "Box"),
    catalog_application("catalog/Bugsnag", "Bugsnag"),
    catalog_application("catalog/Calendly", "Calendly"),
    catalog_application("catalog/Canva", "Canva"),
    catalog_application("catalog/CircleCI", "CircleCI"),
    catalog_application("catalog/CiscoWebex", "Cisco Webex"),
    catalog_application("catalog/CitrixShareFile", "Citrix ShareFile"),
    catalog_application("catalog/ClickUp", "ClickUp"),
    catalog_application("catalog/Cloudflare", "Cloudflare"),
    catalog_application("catalog/Confluence", "Confluence"),
    catalog_application("catalog/Coupa", "Coupa"),
    catalog_application("catalog/CultureAmp", "Culture Amp"),
    catalog_application("catalog/Databricks", "Databricks"),
    catalog_application("catalog/Datadog", "Datadog"),
    catalog_application("catalog/Deputy", "Deputy"),
    catalog_application("catalog/Dialpad", "Dialpad"),
    catalog_application("catalog/Docebo", "Docebo"),
    catalog_application("catalog/DocuSign", "DocuSign"),
    catalog_application("catalog/Domo", "Domo"),
    catalog_application("catalog/DropboxBusiness", "Dropbox Business"),
    catalog_application("catalog/DruvaInSync", "Druva inSync"),
    catalog_application("catalog/DuoNetworkGateway", "Duo Network Gateway"),
    catalog_application("catalog/DynamicSignal", "Dynamic Signal"),
    catalog_application("catalog/Egnyte", "Egnyte"),
    catalog_application("catalog/Envoy", "Envoy"),
    catalog_application("catalog/Expensify", "Expensify"),
    catalog_application("catalog/Figma", "Figma"),
    catalog_application("catalog/Freshdesk", "Freshdesk"),
    catalog_application("catalog/Freshservice", "Freshservice"),
    catalog_application("catalog/GitHub", "GitHub"),
    catalog_application("catalog/GitLab", "GitLab"),
    catalog_application("catalog/Gong", "Gong"),
    catalog_application("catalog/GoogleWorkspace", "Google Workspace"),
    catalog_application("catalog/Greenhouse", "Greenhouse"),
    catalog_application("catalog/Gusto", "Gusto"),
    catalog_application("catalog/Hootsuite", "Hootsuite"),
    catalog_application("catalog/HubSpot", "HubSpot"),
    catalog_application("catalog/Igloo", "Igloo"),
    catalog_application("catalog/Intercom", "Intercom"),
    catalog_application("catalog/JFrogArtifactory", "JFrog Artifactory"),
    catalog_application("catalog/JamfPro", "Jamf Pro"),
    catalog_application("catalog/JiraCloud", "Jira Cloud"),
    catalog_application("catalog/KeeperSecurity", "Keeper Security"),
    catalog_application("catalog/Kisi", "Kisi"),
    catalog_application("catalog/Lattice", "Lattice"),
    catalog_application("catalog/LaunchDarkly", "LaunchDarkly"),
    catalog_application("catalog/Lessonly", "Lessonly"),
    catalog_application("catalog/Lever", "Lever"),
    catalog_application("catalog/Looker", "Looker"),
    catalog_application("catalog/Lucid", "Lucid"),
    catalog_application("catalog/Mailchimp", "Mailchimp"),
    catalog_application("catalog/Marketo", "Marketo"),
    catalog_application("catalog/Microsoft365", "Microsoft 365"),
    catalog_application("catalog/Miro", "Miro"),
    catalog_application("catalog/Mode", "Mode"),
    catalog_application("catalog/MongoDBAtlas", "MongoDB Atlas"),
    catalog_application("catalog/Namely", "Namely"),
    catalog_application("catalog/Netskope", "Netskope"),
    catalog_application("catalog/NewRelic", "New Relic"),
    catalog_application("catalog/Notion", "Notion"),
    catalog_application("catalog/OneTrust", "OneTrust"),
    catalog_application("catalog/Opsgenie", "Opsgenie"),
    catalog_application("catalog/PagerDuty", "PagerDuty"),
    catalog_application("catalog/Pingboard", "Pingboard"),
    catalog_application("catalog/Pipedrive", "Pipedrive"),
    catalog_application("catalog/Pluralsight", "Pluralsight"),
    catalog_application("catalog/Postman", "Postman"),
    catalog_application("catalog/Proxyclick", "Proxyclick"),
    catalog_application("catalog/Qualtrics", "Qualtrics"),
    catalog_application("catalog/Rollbar", "Rollbar"),
    catalog_application("catalog/SAPConcur", "SAP Concur"),
    catalog_application("catalog/Salesforce", "Salesforce"),
    catalog_application("catalog/Sentry", "Sentry"),
    catalog_application("catalog/ServiceNow", "ServiceNow"),
    catalog_application("catalog/Showpad", "Showpad"),
    catalog_application("catalog/Slack", "Slack"),
    catalog_application("catalog/Smartsheet", "Smartsheet"),
    catalog_application("catalog/Snowflake", "Snowflake"),
    catalog_application("catalog/Splunk", "Splunk"),
    catalog_application("catalog/Statuspage", "Statuspage"),
    catalog_application("catalog/SugarCRM", "SugarCRM"),
    catalog_application("catalog/SumoLogic", "Sumo Logic"),
    catalog_application("catalog/Tableau", "Tableau"),
    catalog_application("catalog/Talkdesk", "Talkdesk"),
    catalog_application("catalog/Tanium", "Tanium"),
    catalog_application("catalog/Teamwork", "Teamwork"),
    catalog_application("catalog/TerraformCloud", "Terraform Cloud"),
    catalog_application("catalog/Tipalti", "Tipalti"),
    catalog_application("catalog/Torii", "Torii"),
    catalog_application("catalog/Trello", "Trello"),
    catalog_application("catalog/Twilio", "Twilio"),
    catalog_application("catalog/Udemy", "Udemy"),
    catalog_application("catalog/UserTesting", "UserTesting"),
    catalog_application("catalog/Vanta", "Vanta"),
    catalog_application("catalog/Vonage", "Vonage"),
    catalog_application("catalog/Webflow", "Webflow"),
    catalog_application("catalog/Workday", "Workday"),
    catalog_application("catalog/WorkplaceByFacebook", "Workplace by Facebook"),
    catalog_application("catalog/Wrike", "Wrike"),
    catalog_application("catalog/Xero", "Xero"),
    catalog_application("catalog/Zendesk", "Zendesk"),
    catalog_application("catalog/Zoho", "Zoho"),
    catalog_application("catalog/Zoom", "Zoom"),
    catalog_application("catalog/Zscaler", "Zscaler"),
];

/// The provider id an `ApplicationProviderArn` names, or `None` when the ARN is not an
/// application-provider ARN at all.
///
/// Everything before `:sso::aws:applicationProvider/` is the `arn:<partition>` prefix,
/// which the model's pattern already constrained, so only the id has to be read out.
pub fn application_provider_id(application_provider_arn: &str) -> Option<&str> {
    let (prefix, id) = application_provider_arn.split_once(":sso::aws:applicationProvider/")?;
    prefix.starts_with("arn:").then_some(id)
}

/// The built-in provider `application_provider_arn` names, or `None` for any other
/// syntactically valid ARN — which `DescribeApplicationProvider` answers with the
/// modelled `ResourceNotFoundException`.
pub fn built_in_application_provider(
    application_provider_arn: &str,
) -> Option<&'static BuiltInApplicationProvider> {
    let id = application_provider_id(application_provider_arn)?;
    BUILT_IN_APPLICATION_PROVIDERS
        .iter()
        .find(|provider| provider.id == id)
}

/// The ARN of the instance whose id is `instance_id`.
///
/// `arn:aws:sso:::instance/<id>` — an IAM Identity Center instance ARN carries NO
/// region and NO account segment (the model's pattern ends the prefix at `sso:::`),
/// which is why it is not built through `engine::emulator_arn`; the owning account is
/// reported separately as `InstanceMetadata$OwnerAccountId`.
pub fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:sso:::instance/{instance_id}")
}

/// The ARN of the identity store whose id is `identity_store_id`, inside the modelled
/// `InstanceIdentityStoreArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:identitystore::\d{12}:identitystore/(d-[0-9a-f]{10}|…)$`.
pub fn identity_store_arn(identity_store_id: &str) -> String {
    format!(
        "arn:aws:identitystore::{}:identitystore/{identity_store_id}",
        ::engine::emulator_account_id()
    )
}

/// Mint a fresh instance id: `ssoins-` + 16 characters, the form the model's
/// `InstanceArn` pattern accepts
/// (`^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`).
pub fn mint_instance_id() -> String {
    format!("ssoins-{}", &mint_digest()[..16])
}

/// Mint a fresh identity-store id: `d-` + 10 hex digits, the form
/// `InstanceIdentityStoreArn` accepts and which keeps that ARN inside its modelled
/// 62..93 length.
pub fn mint_identity_store_id() -> String {
    format!("d-{}", &mint_digest()[..10])
}

/// Mint a fresh `PermissionSetArn` under `instance_arn`.
///
/// `arn:<partition>:sso:::permissionSet/<instance id>/ps-<16 chars>`, per the model's
/// `PermissionSetArn` pattern — the partition and the instance id are carried over
/// from the owning instance's ARN, and only the `ps-` suffix is minted.
pub fn mint_permission_set_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    // 16 hex digits, inside `ps-[a-zA-Z0-9-./]{16}` and unique per call so two creates
    // never collide.
    let suffix = &mint_digest()[..16];
    format!("arn:{partition}:sso:::permissionSet/{instance_id}/ps-{suffix}")
}

/// Mint a fresh `TrustedTokenIssuerArn` under `instance_arn`.
///
/// `arn:<partition>:sso::<account>:trustedTokenIssuer/<instance id>/tti-<uuid>`, per the
/// model's `TrustedTokenIssuerArn` pattern — unlike an instance or permission-set ARN,
/// this one carries the owning ACCOUNT (`:sso::\d{12}:`) and no region, so it is built
/// here rather than through `engine::emulator_arn`. The partition and instance id come
/// from the owning instance's ARN; only the `tti-` UUID is minted.
pub fn mint_trusted_token_issuer_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    format!(
        "arn:{partition}:sso::{}:trustedTokenIssuer/{instance_id}/tti-{}",
        ::engine::emulator_account_id(),
        mint_uuid_v4(),
    )
}

/// Mint a fresh `ApplicationArn` under `instance_arn`.
///
/// `arn:<partition>:sso::<account>:application/<instance id>/apl-<16 chars>`, per the
/// model's `ApplicationArn` pattern — like a `TrustedTokenIssuerArn` it carries the
/// owning ACCOUNT (`:sso::\d{12}:`) and no region. The partition and instance id come
/// from the owning instance's ARN; only the `apl-` suffix is minted, as 16 hex digits,
/// which is inside the pattern's `apl-[a-zA-Z0-9]{16}` and unique per call so two
/// creates never collide.
pub fn mint_application_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    format!(
        "arn:{partition}:sso::{}:application/{instance_id}/apl-{}",
        ::engine::emulator_account_id(),
        &mint_digest()[..16],
    )
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying `CreateInstance` is
/// scoped to — the same rule as [`application_fingerprint`] and
/// [`trusted_token_issuer_fingerprint`], over this operation's members (`Name` and
/// `Tags`; `ClientToken` is the key the fingerprint is compared UNDER, not part of it).
///
/// A `Name` the caller omitted is a different request from one that sent the empty
/// string, so the two are distinguished before the parts are length-prefixed. `Tags` go
/// in through [`push_sorted_tag_parts`], so two requests differing only in the order
/// their tags arrived fingerprint alike, while any difference in the MULTISET of pairs
/// they carry — a missing pair, an extra one, a repeated one — fingerprints differently.
pub fn instance_fingerprint(name: Option<&str>, tags: &[Tag]) -> String {
    let mut parts = match name {
        Some(name) => vec!["name".to_string(), name.to_string()],
        None => vec!["no-name".to_string()],
    };
    push_sorted_tag_parts(&mut parts, tags);
    request_fingerprint(&parts)
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying
/// `CreateApplication` is scoped to — the same rule as
/// [`trusted_token_issuer_fingerprint`], over this operation's members.
///
/// `Tags` go in through [`push_sorted_tag_parts`], because a tag set carries no order the
/// caller chose; everything else is length-prefixed in a fixed order by
/// [`request_fingerprint`].
pub fn application_fingerprint(
    instance_arn: &str,
    application_provider_arn: &str,
    name: &str,
    description: Option<&str>,
    portal_options: &PortalOptions,
    status: &str,
    tags: &[Tag],
) -> String {
    let sign_in = portal_options.sign_in_options.as_ref();
    let mut parts = vec![
        instance_arn.to_string(),
        application_provider_arn.to_string(),
        name.to_string(),
        description.unwrap_or_default().to_string(),
        status.to_string(),
        portal_options.visibility.clone(),
        sign_in
            .map(|options| options.origin.clone())
            .unwrap_or_default(),
        sign_in
            .and_then(|options| options.application_url.clone())
            .unwrap_or_default(),
    ];
    push_sorted_tag_parts(&mut parts, tags);
    request_fingerprint(&parts)
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying
/// `CreateTrustedTokenIssuer` is scoped to.
///
/// It is what tells a genuine retry (the same `ClientToken` AND the same request) from
/// the same token carrying a different request, which is a conflict rather than a
/// replay. `Tags` go in through [`push_sorted_tag_parts`], the same way
/// [`application_fingerprint`] folds them in, because a tag set carries no order the
/// caller chose.
pub fn trusted_token_issuer_fingerprint(
    instance_arn: &str,
    name: &str,
    trusted_token_issuer_type: &str,
    configuration: &TrustedTokenIssuerConfiguration,
    tags: &[Tag],
) -> String {
    let mut parts = vec![
        instance_arn.to_string(),
        name.to_string(),
        trusted_token_issuer_type.to_string(),
    ];
    match configuration {
        TrustedTokenIssuerConfiguration::OidcJwtConfiguration(oidc) => {
            parts.push("OidcJwtConfiguration".to_string());
            parts.push(oidc.issuer_url.clone());
            parts.push(oidc.claim_attribute_path.clone());
            parts.push(oidc.identity_store_attribute_path.clone());
            parts.push(oidc.jwks_retrieval_option.clone());
        }
    }
    push_sorted_tag_parts(&mut parts, tags);
    request_fingerprint(&parts)
}

/// Fold a create's `Tags` into its fingerprint `parts`: the pairs SORTED by key then
/// value, each key and each value as its own part.
///
/// The one place every `@idempotencyToken`-scoped create expresses the rule that a tag
/// set carries no order the caller chose, so two requests differing only in the order
/// their tags arrived fingerprint alike and the second is a replay rather than a
/// conflict. [`instance_fingerprint`], [`application_fingerprint`] and
/// [`trusted_token_issuer_fingerprint`] all call it, so none can drift from the others.
///
/// The pairs are sorted rather than deduplicated, so the parts it pushes represent the
/// MULTISET the caller sent: a list that repeats a pair is not the same request as one
/// that carries a different pair in its place.
fn push_sorted_tag_parts(parts: &mut Vec<String>, tags: &[Tag]) {
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
}

/// A stable digest of a request's parts, each LENGTH-PREFIXED so `["a", "bc"]` and
/// `["ab", "c"]` cannot collide.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// A fresh `RequestId`: the hyphenated lowercase UUID the model's `UUId` shape
/// requires (`length 36..36`, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`),
/// laid out as a version-4 UUID (the `4` in the third group, and the RFC 4122 variant
/// nibble `8`..`b` opening the fourth).
pub fn mint_request_id() -> String {
    mint_uuid_v4()
}

/// A fresh hyphenated lowercase version-4 UUID: `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-
/// [89ab][0-9a-f]{3}-[0-9a-f]{12}` (the `4` names the version and the RFC 4122 variant
/// nibble `8`..`b` opens the fourth group). It is the form the model's `UUId` shape and
/// the `tti-` segment of `TrustedTokenIssuerArn` both require.
fn mint_uuid_v4() -> String {
    let digest = mint_digest();
    let variant = match u8::from_str_radix(&digest[30..31], 16).unwrap_or_default() % 4 {
        0 => '8',
        1 => '9',
        2 => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..15],
        &digest[15..18],
        &digest[18..30],
    )
}

/// Per-process counter behind every minted identifier, so two mints in one process
/// differ whatever the clock reports.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// 64 lowercase hex digits unique to this mint.
///
/// SHA-256 of this process's id, the wall clock in nanoseconds and [`MINT_SEQUENCE`]:
/// the counter separates two mints within one process, the pid and clock separate two
/// processes, so no identifier built from it repeats. Nothing here comes from the
/// request, so a caller can neither predict an id nor steer it into another's.
fn mint_digest() -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(::std::process::id().to_le_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher.update(
        MINT_SEQUENCE
            .fetch_add(1, ::std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    ::hex::encode(hasher.finalize())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsoadminSwbexternalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsoadminSwbexternalserviceState>)
}
