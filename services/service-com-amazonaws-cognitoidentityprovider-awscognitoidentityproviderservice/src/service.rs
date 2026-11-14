// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Six opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name, which this
//!   service widens to the two names ONE awsJson endpoint answers on (see the method).
//!   Edit it for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — EDITED HERE: the key and the handler stay the generated
//!   table's, but each operation is served through [`WithSubmittedValues`], so a request
//!   refused before the handler runs states the COLLECTION the caller submitted, which
//!   the framework's walk can only summarise as `<n> items` — and through
//!   [`BehindWebAcl`], so a request to a pool with a WAFv2 web ACL associated is decided
//!   by that web ACL before the operation serves it — and through
//!   [`iam_authorization::IamAuthorized`], so an operation the model requires a signature
//!   for is served only to a caller IAM allows to perform it — and, outermost, through
//!   [`RequestIdReportingOperation::header`], so every response an operation produced names
//!   the request it answers (see "Naming the request a response answers").
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired. This
//!   service answers three such requests itself: a GET of a pool's
//!   `/<UserPoolId>/.well-known/jwks.json`, the public key set its tokens are
//!   verified against, a GET of its
//!   `/<UserPoolId>/.well-known/openid-configuration`, the OpenID Connect
//!   discovery document naming its endpoints, and a GET of a level's
//!   `/<UserPoolId>/<ClientId>/assets/login_logo.png`, the classic hosted UI logo
//!   image `SetUICustomization` stored — neither of which the Smithy model
//!   describes.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//! - `override_protocol` — EDITED HERE: the model's own `awsJson1_1`, wrapped so that an
//!   ERROR is answered the way this endpoint is recorded answering one — the status every
//!   refusal of the caller carries, and the `x-amzn-errortype` naming its error shape.
//!   See the `error_wire` module for the rule.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success exactly as on a
//! failure, which is what an SDK surfaces for the call and what a client correlating a
//! call with a record of it joins on. It is a property of the RESPONSE rather than of any
//! modelled shape: no output and no error shape of this model declares it, so no handler
//! can set it, and the framework's `awsJson1_1` renderers do not write it.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report this protocol takes (the header
//! alone — `awsJson1_1` has no envelope element for the id) and wires it at the two layers
//! every response of this service passes through:
//!
//! * [`RequestIdReportingOperation::header`], the outermost wrapper of each row of the
//!   table `override_operation` serves from, which covers a success, a modelled error, the
//!   web ACL's and the IAM gate's own refusals, and a constraint refusal raised while that
//!   operation was being served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, which covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use aws_smithy_schema_server::ProtocolError;
// The shared naming of the two documents every OIDC issuer this emulator serves publishes,
// which this service's own well-known routes answer with.
use service_common::openid_discovery::WellKnownDocument;
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdReportingOperation,
};
use service_common::validation::{
    refusing_violations_with_declared_echoes_quoting_up_to, unanchored, SubmittedEcho,
};
use service_common::validation_message::{
    in_hashed_enum_order, quoting_the_submitted_collection, quoting_the_whole_submitted_value,
};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, OperationError, OperationRoutingTable, Refusal, Refused, ServerHttpProtocol,
    Service,
};
use std::sync::Arc;

/// How this service answers an ERROR on the wire: the status a refusal of the caller
/// carries, and the header naming the error shape it refused with. Private: it is reached
/// through `override_protocol` and the refusal tail below, not by a caller.
mod error_wire;

/// The WAFv2 web ACL in front of a user pool, which decides the pool's API operations
/// before the operation serves them ([`BehindWebAcl`]).
mod web_acl;

/// Whether IAM lets a request perform the action it names, which is decided in front of
/// every operation the model requires a SigV4 signature for
/// ([`iam_authorization::IamAuthorized`]).
pub mod iam_authorization;

/// The error code Amazon Cognito answers a bad parameter with, and the code this service
/// gives a refusal the framework made before any handler ran. Restated beside the message
/// it travels with, because a chosen [`Refused`] replaces the code and the message
/// TOGETHER and a client branches on the code.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The member POSITIONS the echo-collecting walk may visit for one request of this
/// service, stated from this model rather than taken from the shared default (256).
///
/// A COLLECTION is quoted in a clause only when every one of its elements was walked, so
/// the ceiling has to cover this model's largest collection one element PAST its bound.
/// The largest collection of STRUCTURES is `SchemaAttributesListType` (`@length` max 50)
/// over `SchemaAttributeType`, whose members bottom out in nine leaf positions (five
/// scalars plus the two of each attribute-constraints structure): 51 x 9 = 459. The
/// largest collection of SCALARS is a risk configuration's IP ranges
/// (`BlockedIPRangeListType` / `SkippedIPRangeListType`, max 200): 201. This covers both,
/// with room for the other members of the request carrying it. Past it the framework's own
/// count summary stands, which is the fail-safe direction.
const MAX_ECHO_POSITIONS: usize = 512;

/// The largest `@length` maximum any member of this model declares: `CSSType` and
/// `ImageFileType` (`smithy_query(query="shape",shape="CSSType")`).
const LARGEST_DECLARED_LENGTH: usize = 131_072;

/// The largest single value one clause of this service quotes.
///
/// An Amazon Cognito constraint clause quotes the value the request carried, and a value
/// that BREAKS a declared maximum is longer than that maximum — so a ceiling able to quote
/// the refusal of this model's largest member has to sit above
/// [`LARGEST_DECLARED_LENGTH`]. Twice it, so a value a little past the largest member's
/// bound is quoted whole while one clause can never carry request-sized content: a MODEL
/// constant rather than a request-sized one. Past it the framework's own elided rendering
/// stands, which is the fail-safe direction.
const MAX_QUOTED_VALUE: usize = 2 * LARGEST_DECLARED_LENGTH;

/// The validation path of the `Domain` member every operation that names a user pool's
/// hosted domain binds (`smithy_query(query="operation",operation="CreateUserPoolDomain")`),
/// as the framework spells a member path.
const DOMAIN_MEMBER_PATH: &str = "domain";

/// The operations that ask a user pool FOR a hosted domain name, as against the ones that
/// address a domain already asked for: a create of the pool's domain, and the update that
/// re-points it. Their request carries a name the pool is to TAKE, which is the name Amazon
/// Cognito validates with its own prose ([`domain_name_prose`]).
const DOMAIN_NAMING_OPERATIONS: [&str; 2] = ["CreateUserPoolDomain", "UpdateUserPoolDomain"];

/// The operation that reports the domain a name holds, which Amazon Cognito answers for ANY
/// name (`smithy_query(query="operation",operation="DescribeUserPoolDomain")`): see
/// [`served_despite`].
const DESCRIBE_USER_POOL_DOMAIN: &str = "DescribeUserPoolDomain";

/// The machine-to-machine grant, whose request PRESENTS a credential to be verified
/// (`smithy_query(query="operation",operation="GetClientToken")`): see [`served_despite`].
const GET_CLIENT_TOKEN: &str = "GetClientToken";

/// The validation path of the `Secret` member that grant presents its app client's secret
/// in, as the framework spells a member path.
const SECRET_MEMBER_PATH: &str = "secret";

/// The app-client listing, whose page size Amazon Cognito does not hold to the minimum its
/// own model declares (`smithy_query(query="operation",operation="ListUserPoolClients")`
/// puts `MaxResults` in `@range` 1..60): see [`served_despite`].
const LIST_USER_POOL_CLIENTS: &str = "ListUserPoolClients";

/// The validation path of the `MaxResults` member a listing's page size is asked for in, as
/// the framework spells a member path.
const MAX_RESULTS_MEMBER_PATH: &str = "maxResults";

/// The validation path of the `UserPoolId` member every operation that works ON a stored
/// user pool binds (`smithy_query(query="operation",operation="DescribeUserPoolClient")`
/// puts it in `UserPoolIdType`, `@required`), as the framework spells a member path.
const USER_POOL_ID_MEMBER_PATH: &str = "userPoolId";

/// That same member as the MODEL names it, which is how the sentence answered for a request
/// carrying no pool to address names it ([`absent_user_pool_id_sentence`]). Held beside the
/// path it is cased from, and checked against the model by this file's own test.
const USER_POOL_ID_MEMBER: &str = "UserPoolId";

/// The `com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers (see the module
    /// docs). Identity, the protocol and the operations are the generated entry's
    /// untouched.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// The shared engine, so the non-API routes a user-pool endpoint serves
    /// (the pool's JSON Web Key Set and a level's hosted logo image, see
    /// [`EmulatedService::override_routing_error`]) can be answered from this
    /// service's stored state. Operations reach the engine through the entry
    /// that constructed them, not through this field.
    engine: Arc<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`WithSubmittedValues`]. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    with_submitted_values: OperationRoutingTable,
    /// This service's wire protocol, built ONCE here because `override_protocol` hands
    /// out a borrow of it rather than a new one per request. It is the `awsJson1_1` the
    /// model declares, answering an error the way this endpoint is recorded answering one
    /// (see [`error_wire`]).
    protocol: error_wire::ErrorWireProtocol,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`WithSubmittedValues`], in [`iam_authorization::IamAuthorized`], in
/// [`BehindWebAcl`] and — outermost, so every response an operation produced names the
/// request it answers — in [`RequestIdReportingOperation::header`], and keyed by its
/// operation shape name — the routing key `ServerHttpProtocol::routing_key` resolves and
/// the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(RequestIdReportingOperation::header(BehindWebAcl::new(
                    stringify!($operation),
                    ::std::sync::Arc::clone(&$engine),
                    ::std::boxed::Box::new(iam_authorization::IamAuthorized::new(
                        stringify!($operation),
                        ::std::sync::Arc::clone(&$engine),
                        ::std::boxed::Box::new(WithSubmittedValues::new(
                            crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                        )) as ::std::boxed::Box<dyn ErasedOperation>,
                    )) as ::std::boxed::Box<dyn ErasedOperation>,
                ))) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut with_submitted_values = OperationRoutingTable::new();
        wrap_operations!(
            with_submitted_values,
            engine,
            add_custom_attributes::AddCustomAttributes,
            add_user_pool_client_secret::AddUserPoolClientSecret,
            admin_add_user_to_group::AdminAddUserToGroup,
            admin_confirm_sign_up::AdminConfirmSignUp,
            admin_create_user::AdminCreateUser,
            admin_delete_software_token::AdminDeleteSoftwareToken,
            admin_delete_user::AdminDeleteUser,
            admin_delete_user_attributes::AdminDeleteUserAttributes,
            admin_disable_provider_for_user::AdminDisableProviderForUser,
            admin_disable_user::AdminDisableUser,
            admin_enable_user::AdminEnableUser,
            admin_forget_device::AdminForgetDevice,
            admin_get_device::AdminGetDevice,
            admin_get_user::AdminGetUser,
            admin_get_user_auth_factors::AdminGetUserAuthFactors,
            admin_initiate_auth::AdminInitiateAuth,
            admin_link_provider_for_user::AdminLinkProviderForUser,
            admin_list_devices::AdminListDevices,
            admin_list_groups_for_user::AdminListGroupsForUser,
            admin_list_user_auth_events::AdminListUserAuthEvents,
            admin_remove_user_from_group::AdminRemoveUserFromGroup,
            admin_reset_user_password::AdminResetUserPassword,
            admin_respond_to_auth_challenge::AdminRespondToAuthChallenge,
            admin_set_user_mfa_preference::AdminSetUserMFAPreference,
            admin_set_user_password::AdminSetUserPassword,
            admin_set_user_settings::AdminSetUserSettings,
            admin_update_auth_event_feedback::AdminUpdateAuthEventFeedback,
            admin_update_device_status::AdminUpdateDeviceStatus,
            admin_update_user_attributes::AdminUpdateUserAttributes,
            admin_user_global_sign_out::AdminUserGlobalSignOut,
            associate_software_token::AssociateSoftwareToken,
            change_password::ChangePassword,
            complete_web_authn_registration::CompleteWebAuthnRegistration,
            confirm_device::ConfirmDevice,
            confirm_forgot_password::ConfirmForgotPassword,
            confirm_sign_up::ConfirmSignUp,
            create_group::CreateGroup,
            create_identity_provider::CreateIdentityProvider,
            create_managed_login_branding::CreateManagedLoginBranding,
            create_resource_server::CreateResourceServer,
            create_terms::CreateTerms,
            create_user_import_job::CreateUserImportJob,
            create_user_pool::CreateUserPool,
            create_user_pool_client::CreateUserPoolClient,
            create_user_pool_domain::CreateUserPoolDomain,
            create_user_pool_replica::CreateUserPoolReplica,
            delete_group::DeleteGroup,
            delete_identity_provider::DeleteIdentityProvider,
            delete_managed_login_branding::DeleteManagedLoginBranding,
            delete_resource_server::DeleteResourceServer,
            delete_terms::DeleteTerms,
            delete_user::DeleteUser,
            delete_user_attributes::DeleteUserAttributes,
            delete_user_pool::DeleteUserPool,
            delete_user_pool_client::DeleteUserPoolClient,
            delete_user_pool_client_secret::DeleteUserPoolClientSecret,
            delete_user_pool_domain::DeleteUserPoolDomain,
            delete_user_pool_replica::DeleteUserPoolReplica,
            delete_web_authn_credential::DeleteWebAuthnCredential,
            describe_identity_provider::DescribeIdentityProvider,
            describe_managed_login_branding::DescribeManagedLoginBranding,
            describe_managed_login_branding_by_client::DescribeManagedLoginBrandingByClient,
            describe_resource_server::DescribeResourceServer,
            describe_risk_configuration::DescribeRiskConfiguration,
            describe_terms::DescribeTerms,
            describe_terms_by_client::DescribeTermsByClient,
            describe_user_import_job::DescribeUserImportJob,
            describe_user_pool::DescribeUserPool,
            describe_user_pool_client::DescribeUserPoolClient,
            describe_user_pool_domain::DescribeUserPoolDomain,
            forget_device::ForgetDevice,
            forgot_password::ForgotPassword,
            get_client_token::GetClientToken,
            get_csv_header::GetCSVHeader,
            get_device::GetDevice,
            get_group::GetGroup,
            get_identity_provider_by_identifier::GetIdentityProviderByIdentifier,
            get_log_delivery_configuration::GetLogDeliveryConfiguration,
            get_provisioned_limit::GetProvisionedLimit,
            get_signing_certificate::GetSigningCertificate,
            get_tokens_from_refresh_token::GetTokensFromRefreshToken,
            get_ui_customization::GetUICustomization,
            get_user::GetUser,
            get_user_attribute_verification_code::GetUserAttributeVerificationCode,
            get_user_auth_factors::GetUserAuthFactors,
            get_user_pool_mfa_config::GetUserPoolMfaConfig,
            global_sign_out::GlobalSignOut,
            initiate_auth::InitiateAuth,
            list_devices::ListDevices,
            list_groups::ListGroups,
            list_identity_providers::ListIdentityProviders,
            list_resource_servers::ListResourceServers,
            list_tags_for_resource::ListTagsForResource,
            list_terms::ListTerms,
            list_user_import_jobs::ListUserImportJobs,
            list_user_pool_client_secrets::ListUserPoolClientSecrets,
            list_user_pool_clients::ListUserPoolClients,
            list_user_pool_replicas::ListUserPoolReplicas,
            list_user_pools::ListUserPools,
            list_users::ListUsers,
            list_users_in_group::ListUsersInGroup,
            list_web_authn_credentials::ListWebAuthnCredentials,
            resend_confirmation_code::ResendConfirmationCode,
            respond_to_auth_challenge::RespondToAuthChallenge,
            revoke_token::RevokeToken,
            set_log_delivery_configuration::SetLogDeliveryConfiguration,
            set_risk_configuration::SetRiskConfiguration,
            set_ui_customization::SetUICustomization,
            set_user_mfa_preference::SetUserMFAPreference,
            set_user_pool_mfa_config::SetUserPoolMfaConfig,
            set_user_settings::SetUserSettings,
            sign_up::SignUp,
            start_user_import_job::StartUserImportJob,
            start_web_authn_registration::StartWebAuthnRegistration,
            stop_user_import_job::StopUserImportJob,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_auth_event_feedback::UpdateAuthEventFeedback,
            update_device_status::UpdateDeviceStatus,
            update_group::UpdateGroup,
            update_identity_provider::UpdateIdentityProvider,
            update_managed_login_branding::UpdateManagedLoginBranding,
            update_provisioned_limit::UpdateProvisionedLimit,
            update_resource_server::UpdateResourceServer,
            update_terms::UpdateTerms,
            update_user_attributes::UpdateUserAttributes,
            update_user_pool::UpdateUserPool,
            update_user_pool_client::UpdateUserPoolClient,
            update_user_pool_domain::UpdateUserPoolDomain,
            update_user_pool_replica::UpdateUserPoolReplica,
            verify_software_token::VerifySoftwareToken,
            verify_user_attribute::VerifyUserAttribute,
        );
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(crate::ServiceDefinition::new(
                Arc::clone(&engine),
            )),
            engine,
            with_submitted_values,
            protocol: error_wire::ErrorWireProtocol::new(),
        }
    }
}

/// One modelled operation, served BEHIND the WAFv2 web ACL associated with the user pool
/// the request addresses.
///
/// A web ACL is "a set of rules that defines the strategy that WAF uses to inspect and
/// handle web requests" and it sits in FRONT of the resource it is associated with, so it
/// is asked ahead of everything the operation itself decides about the request — the
/// parameters, the app client, the credential and the pool's own records, none of which a
/// request it blocks ever reaches. A pool with no web ACL associated, and an operation no
/// web ACL decides, are served exactly as they were before web ACLs were read at all; the
/// whole decision and the answer it renders are [`web_acl`]'s.
///
/// A `CustomRequestHandling`'s inserted headers are deliberately not carried onto the
/// request the operation goes on to serve: WAF inserts them "into the web request" for the
/// ORIGIN to read, and an operation of this service is addressed entirely by its modelled
/// input rather than by a header of its request, so there is nothing here that would read
/// one.
struct BehindWebAcl<Eng> {
    /// The operation shape name, which is both this entry's routing key and the name
    /// [`web_acl::governs`] asks about.
    name: &'static str,
    /// The shared engine, so the web ACL the wafv2 control plane stored for this pool is
    /// read from the one store every service of this emulator shares.
    engine: Arc<Eng>,
    /// The operation this one wraps, served for every request the web ACL let through.
    operation: Box<dyn ErasedOperation>,
}

impl<Eng> BehindWebAcl<Eng> {
    /// Put `operation`, already constructed from the shared engine, behind the web ACL.
    fn new(name: &'static str, engine: Arc<Eng>, operation: Box<dyn ErasedOperation>) -> Self {
        Self {
            name,
            engine,
            operation,
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for BehindWebAcl<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let (refused, logged) = web_acl::refusal(&*self.engine, self.name, req);
            // What the web ACL LOGS about the request it has just decided, delivered to
            // the destination its own `LoggingConfiguration` names: WAF records the
            // request it inspected, so the record is delivered for a request it refused
            // at the edge as much as for one it let through to the operation.
            if let Some(logged) = logged.as_ref() {
                ::service_com_amazonaws_wafv2_awswaf_20190729::service::logs::deliver(ctx, logged)
                    .await;
            }
            if let Some(refused) = refused {
                return refused;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One modelled operation, served so that a refusal of a modelled `@length` on a LIST
/// states the list the caller submitted.
///
/// An Amazon Cognito constraint refusal is built from the request — `1 validation error
/// detected: Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>` — so the value between the quotes is part of the rule rather than prose.
/// The framework's constraint walk stores a RENDERING of that value on each violation, and
/// for a COLLECTION it has only the count (`<n> items`), so a clause on a collection past
/// its `@length` reports `Value '3 items'` where Amazon Cognito reports the entries that
/// were submitted. The `override_refusal` seam cannot repair that — it is handed the
/// violations alone, never the request they came from — so the restoring step happens
/// here, per operation, where the input can be re-read.
///
/// Nothing else is re-implemented, and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), the sentence from the framework's own
/// `constraints::envelope`, and the status from the framework's refusal channel. A request
/// this wrapper does not refuse is handed to the generated [`ErasedOperation::run`], which
/// owns deserialize -> validate -> handle -> serialize, telemetry and every modelled
/// error. The cost is DELIBERATE: a valid request's body is read twice, once here and once
/// in the operation's own sequence, because the framework hands no already-built input to
/// an operation.
struct WithSubmittedValues<O> {
    operation: O,
}

impl<O> WithSubmittedValues<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// `violation` with the clause's value half stating the value the caller submitted, and its
/// `@pattern` requirement quoted the way Amazon Cognito quotes one.
///
/// Three renderings of one clause, each delegated to the one place that owns it:
///
/// * the framework's constraint walk has no rendering for a COLLECTION, so a clause on a
///   list or a map over or under its `@length` carries the walk's `<n> items` / `<n> entries`
///   summary, which is not a value any caller sent: it is replaced by the collection as
///   `echo` renders it ([`quoting_the_submitted_collection`]);
/// * that walk ELIDES a long value into the clause at a ceiling of its own, while Amazon
///   Cognito quotes the whole value the request carried: the elided half is replaced by that
///   value ([`quoting_the_whole_submitted_value`], within [`MAX_QUOTED_VALUE`]);
/// * a `@pattern` requirement is quoted as the expression Amazon Cognito publishes for the
///   violated member's target shape, which for most of this model's shapes is the modelled
///   text WITHOUT the `^`/`$` a whole-value pattern is written with ([`unanchored`]) and for
///   a terms document's own members is that text as the model writes it
///   ([`pattern_as_stated`]). The CHECK is still the model's pattern as written: only this
///   rendering decides the pair.
///
/// The last of those is one half of [`requirement_as_stated`], which owns BOTH of this
/// endpoint's requirement renderings: the `@pattern` expression and the order an
/// enumerated type's value set is listed in.
///
/// Every other clause keeps the framework's own rendering, which already states the bytes the
/// caller submitted, and a value the echo bounds declined to render keeps the framework's
/// text rather than losing the clause.
///
/// A member the model marks `@sensitive` is quoted too: the validating walk withholds
/// such a value, and Amazon Cognito's recorded constraint clause states it — the refusal
/// of a `ClientId` outside its `@pattern` quotes the id the request carried
/// (`tests/all/input_validation_refusal_wording.rs`, and the recorded
/// `DescribeTermsByClient` refusal behind it). The value comes from the request this walk
/// re-read, which is the only place it survives.
fn quoting_the_submitted_value(violation: Violation, echo: SubmittedEcho) -> Violation {
    let quoted = quoting_the_whole_submitted_value(
        quoting_the_submitted_collection(withheld_value_quoted(violation, &echo), echo.clone()),
        &echo,
    );
    requirement_as_stated(&quoted)
}

/// `violation` with the REQUIREMENT half of its clause worded the way this endpoint words
/// it, and every other half of the clause left exactly as the framework built it.
///
/// Two requirement classes are stated by the SHAPE the violated member targets rather than
/// by the member, the operation or the depth the value sat at — so both are applied here, in
/// the one place every refusal of this service passes through, rather than per operation:
///
/// * a `@pattern` expression is published as the expression the violated member's TARGET
///   SHAPE advertises, which is the modelled text with the whole-value `^`/`$` pair dropped
///   for most of this model's shapes and the modelled text as written for a terms document's
///   ([`pattern_as_stated`]);
/// * an enumerated type's admissible set is enumerated in the order the set's own HASH
///   TABLE is walked — ascending bucket of each value's string hash, two values of one
///   bucket read from the head each was pushed onto — rather than in the order the model
///   declares them or a sort of them
///   ([`in_hashed_enum_order`]). `FeedbackValueType`, declared `Valid`, `Invalid`
///   (`smithy_query(query="shape",shape="FeedbackValueType")`), is stated `[Invalid,
///   Valid]`; `UpdateReplicaStatusType`'s two values fall in ascending buckets, so its set
///   reads in declaration order. The order is computed from the declared values
///   themselves, so a type the model gains needs no row anywhere.
///
/// The CHECK is untouched by either: the model's pattern as written is still what refused
/// the value, and the set that refused it still holds exactly the model's values.
fn requirement_as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            kind: ViolationKind::Pattern(pattern_as_stated(&violation.path, pattern)),
            ..violation.clone()
        },
        ViolationKind::EnumValue(_) => in_hashed_enum_order(violation),
        _ => violation.clone(),
    }
}

/// The members of this model whose clause quotes the modelled `@pattern` CHARACTER FOR
/// CHARACTER, the `^`/`$` pair included: the two that address a set of terms documents,
/// named as a clause names a member (lowerCamel) — `TermsId`, targeting `TermsIdType`
/// (`^[0-9a-fA-F]{8}-…-[0-9a-fA-F]{12}$`), and `TermsName`, targeting `TermsNameType`
/// (`^(terms-of-use|privacy-policy)$`); `smithy_query(query="shape",shape="TermsIdType")`
/// and `…shape="TermsNameType"`.
///
/// The set is keyed by the MEMBER NAME because that is the only thing a violation carries
/// about which shape declared the expression — the framework's member schema names the
/// member, never its target — and these two names are each the name of exactly one member
/// position of this model, whichever operation carries it (`TermsId` on `DeleteTerms`,
/// `DescribeTerms` and `UpdateTerms`; `TermsName` on `CreateTerms`, `DescribeTermsByClient`
/// and `UpdateTerms`). So the rendering follows the shape, not the operation or the request:
/// every operation that addresses a terms document states its expression the same way.
const PATTERN_QUOTED_AS_WRITTEN: &[&str] = &["termsId", "termsName"];

/// The `@pattern` expression a clause about the member at `path` quotes, over the modelled
/// `pattern` that refused the value.
///
/// A clause states the expression Amazon Cognito PUBLISHES for that member, and this endpoint
/// publishes two renderings of the one modelled text:
///
/// * for a terms document's members ([`PATTERN_QUOTED_AS_WRITTEN`]) the modelled text itself,
///   anchors and all;
/// * for every other patterned member the modelled text without the whole-value `^`/`$` pair
///   ([`unanchored`]), which is the expression a value has to satisfy once the pair that
///   turns a find into a whole-string match is taken off.
///
/// The CHECK is the model's pattern as written in BOTH cases: a value is refused by the
/// anchored expression whatever the clause goes on to quote, so the two renderings differ
/// only in what the message advertises.
///
/// The LAST segment of the path names the member, so a member at any depth is rendered by
/// the shape it targets rather than by the position it sat at; a map ENTRY's path ends in the
/// collection step rather than in a member name, so a key's clause keeps the common
/// rendering.
fn pattern_as_stated(path: &str, pattern: &str) -> String {
    let member = path.split('.').next_back().unwrap_or(path);
    if PATTERN_QUOTED_AS_WRITTEN.contains(&member) {
        return pattern.to_string();
    }
    unanchored(pattern)
}

/// `violation` with the clause's value half restored where the framework WITHHELD it.
///
/// The framework's constraint walk drops the value of a `@sensitive` member from the
/// violation and says so on it, so its clause names the member alone. Amazon Cognito quotes
/// the value a request carried in that clause anyway — `ClientIdType` is `@sensitive` and
/// its recorded refusal states `Value '<the client id>' at 'clientId' …` — so the value this
/// service's clause states is restored from the echo, the one bounded rendering of the
/// submitted value, and the withholding is cleared with it so the renderer states what was
/// restored.
///
/// A violation the walk did not withhold, and one whose echo the bounds declined to render,
/// are handed back untouched.
fn withheld_value_quoted(violation: Violation, echo: &SubmittedEcho) -> Violation {
    let SubmittedEcho::Rendered(submitted) = echo else {
        return violation;
    };
    if !violation.sensitive {
        return violation;
    }
    Violation {
        value: Some(submitted.clone()),
        sensitive: false,
        ..violation
    }
}

/// The prose Amazon Cognito answers a `Domain` that is not a name a user pool's hosted
/// domain may take, in place of the constraint report the framework would render \u2014 or `None`
/// where this refusal is not that one.
///
/// Amazon Cognito validates the domain name a request ASKS FOR itself and reports a character
/// a domain name
/// cannot carry in its own sentence, naming the characters a domain may carry and the format
/// a name has to follow, rather than as a clause about a member. The format it quotes is the
/// expression the model declares for a domain name, taken from the violation rather than
/// restated here, so the sentence cannot drift from the pattern that refused the name \u2014 and
/// quoted as the MODEL writes it, anchors included, because the prose states the expression a
/// WHOLE name must match rather than the one a clause quotes.
///
/// Only an `operation` that asks for a name answers it ([`DOMAIN_NAMING_OPERATIONS`]): one
/// that merely ADDRESSES a domain is asking about a name rather than for one, so it keeps the
/// modelled constraint report. And only for a request refused for the FORM of its domain name
/// and nothing else: a
/// request that breaks another member's constraint too is a refusal of more than this
/// sentence reports, so the framework's envelope stands.
fn domain_name_prose(operation: &str, violations: &[(Violation, SubmittedEcho)]) -> Option<String> {
    if !DOMAIN_NAMING_OPERATIONS.contains(&operation) {
        return None;
    }
    if violations
        .iter()
        .any(|(violation, _)| violation.path != DOMAIN_MEMBER_PATH)
    {
        return None;
    }
    violations
        .iter()
        .find_map(|(violation, _)| match &violation.kind {
            ViolationKind::Pattern(pattern) => Some(format!(
                "The domain name contains an invalid character. Domain names can only contain \
                 lower-case letters, numbers, and hyphens. Please enter a different name that \
                 follows this format: {pattern}"
            )),
            _ => None,
        })
}

/// The sentence Amazon Cognito answers a request that names no user pool to work on, in
/// place of the constraint report the framework would render — or `None` where this refusal
/// is not that one.
///
/// Amazon Cognito resolves the pool a request addresses before it reads anything else about
/// that request, so a request carrying no pool id at all is refused by the null check that
/// resolution opens with: the member as the MODEL names it ([`USER_POOL_ID_MEMBER`]), then
/// that it must not be null, and nothing about a value or a constraint. A pool id that WAS
/// supplied keeps the constraint report, whatever its shape — that report quotes the value
/// the request carried, which is what it is for — and so does every other member, present or
/// absent.
///
/// The sentence is the WHOLE refusal, because the check it comes from is the first thing the
/// request meets: a request that also breaks another member's rule is still refused for the
/// pool it never named, and the other member is not reported beside it.
fn absent_user_pool_id_sentence<'v>(
    violations: impl IntoIterator<Item = &'v Violation>,
) -> Option<String> {
    violations
        .into_iter()
        .any(|violation| {
            violation.path == USER_POOL_ID_MEMBER_PATH
                && matches!(violation.kind, ViolationKind::MissingRequiredMember)
        })
        .then(|| format!("{USER_POOL_ID_MEMBER} must not be null"))
}

/// Whether `violation` is one this service does NOT refuse on, because Amazon Cognito
/// SERVES the request that carries it — the handler then answers it from stored state.
///
/// Three kinds of member reach this:
///
/// * A lookup addressed BY a hosted domain name reports the domain that name holds, and a
///   name no create could ever take holds none — so Amazon Cognito answers such a lookup with
///   the domain it found (none) rather than refusing the name's form, even though the model
///   declares that form on the member. The operations that ask a pool FOR a name do refuse it
///   ([`DOMAIN_NAMING_OPERATIONS`], [`domain_name_prose`]), which is where the name has to be
///   one the pool can take.
/// * A member carrying a CREDENTIAL the request presents to be verified is checked against
///   the stored secret rather than against its declared form: the machine-to-machine grant
///   "provides its client ID and secret"
///   (`smithy_query(query="operation",operation="GetClientToken")`) and either authenticates
///   that pair or does not, so a presented secret of any shape is answered by the grant's
///   own refusal, which says neither which half was wrong nor what was sent. Refusing its
///   FORM instead would both name the wrong class and quote the submitted secret back —
///   this service's constraint clause states the value a request carried — so a credential
///   no caller may read off a refusal would travel in one.
///
/// Only the form of the presented value is served this way: a required member the request
/// omitted carries no value to verify, so it keeps the framework's refusal.
///
/// A third kind is a PAGE SIZE under the minimum the model declares for it: the app-client
/// listing is recorded answering such a request with a page — HTTP 200 carrying its
/// `UserPoolClients` member — rather than refusing it, so the listing serves it and reports
/// the items a page of that size holds. Only the floor is served: a page size over the
/// declared maximum asks for more than the listing serves at once, which is not a page it
/// has, and keeps the framework's refusal.
fn served_despite(operation: &str, violation: &Violation) -> bool {
    let presented_value = !matches!(violation.kind, ViolationKind::MissingRequiredMember);
    (operation == DESCRIBE_USER_POOL_DOMAIN
        && violation.path == DOMAIN_MEMBER_PATH
        && matches!(violation.kind, ViolationKind::Pattern(_)))
        || (operation == GET_CLIENT_TOKEN
            && violation.path == SECRET_MEMBER_PATH
            && presented_value)
        || (operation == LIST_USER_POOL_CLIENTS
            && violation.path == MAX_RESULTS_MEMBER_PATH
            && matches!(violation.kind, ViolationKind::MinValue(_)))
}

impl<O> WithSubmittedValues<O>
where
    O: Operation + ErasedOperation,
{
    /// Serve a request the model's own constraints would refuse and Amazon Cognito answers
    /// ([`served_despite`]), or `None` when its body is not one this protocol can read at
    /// all — which the framework's own sequence then answers in its own envelope.
    ///
    /// The framework's centralized sequence validates EVERY modelled constraint between
    /// deserializing and handling, with no seam to leave one of them out, so the steps
    /// around that check are taken here instead: the protocol's own deserializer, the
    /// operation's own handler and the protocol's own serializer, each called rather than
    /// reimplemented, so the body, the status and the error wording stay the ones every
    /// other request of this service gets.
    ///
    /// ONE thing that sequence owns is NOT taken: its telemetry — the recording timer, the
    /// in-flight guard and the failure classification
    /// (`vendor/smithy-server-runtime/src/operation.rs`) — because none of it is reachable
    /// from here. A request served this way is therefore invisible to a collecting client,
    /// which is the cost of having no seam to skip one violation; the structural fix is such
    /// a seam in that frozen crate, and until it exists this is the narrower loss (one
    /// member of one operation) than answering a request Amazon Cognito serves with a
    /// refusal.
    async fn served(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Option<RtResponse> {
        // Each protocol borrow is taken, used and dropped inside one statement:
        // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across the await
        // below would make this future non-`Send`.
        let input = {
            let protocol = wire_protocol(svc);
            let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
            <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?
        };
        let answered = self.operation.handle(input, ctx).await;
        let protocol = wire_protocol(svc);
        let rendered = match &answered {
            Ok(output) => protocol.serialize_output(
                output,
                O::OUTPUT_SCHEMA,
                O::NAME,
                O::SUCCESS_STATUS,
            ),
            Err(error) => protocol.serialize_error(
                error.as_serializable(),
                error.variant_schema(),
                error.shape_name(),
                error.http_status(),
            ),
        };
        Some(
            rendered
                .unwrap_or_else(|failed| svc.definition().framework_error_response(failed, req)),
        )
    }
}

/// The wire protocol a response of this service is rendered with: the one this service
/// overrides the model's with ([`EmulatedService::override_protocol`]), as the framework's
/// own sequence resolves it.
fn wire_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

impl<O> ErasedOperation for WithSubmittedValues<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            let collected = refusing_violations_with_declared_echoes_quoting_up_to::<O>(
                svc.definition().protocol(),
                req,
                MAX_ECHO_POSITIONS,
                MAX_QUOTED_VALUE,
            );
            // Read off the violations as the MODEL stated them, before the clause
            // renderings below requote the expression: the prose quotes the pattern a
            // whole domain name must match, anchors included. A request that names no
            // pool to work on is answered by the null check that pool resolution opens
            // with, which is ahead of every clause about a member of the request.
            let prose = absent_user_pool_id_sentence(
                collected.iter().map(|(violation, _)| violation),
            )
            .or_else(|| domain_name_prose(O::NAME, &collected));
            // A violation this service does not refuse on is left out of the refusal and
            // noted, because the framework's own sequence WOULD refuse it.
            let served = collected
                .iter()
                .any(|(violation, _)| served_despite(O::NAME, violation));
            let refused: Vec<Violation> = collected
                .into_iter()
                .filter(|(violation, _)| !served_despite(O::NAME, violation))
                .map(|(violation, echo)| quoting_the_submitted_value(violation, echo))
                .collect();
            if let Some(message) = prose.or_else(|| constraints::envelope(&refused)) {
                // The framework renders the refusal in this service's own envelope; the
                // one thing it does not state on an awsJson error is the error shape in
                // `x-amzn-errortype`, which this service answers on EVERY error, so the
                // code chosen here names it there too (see [`error_wire`]).
                return error_wire::naming_the_error_shape(
                    svc.definition().refusal_response(
                        ProtocolError::Invalid(refused),
                        Some(Refused {
                            code: INVALID_PARAMETER_EXCEPTION.to_string(),
                            message,
                        }),
                        req,
                    ),
                    INVALID_PARAMETER_EXCEPTION,
                );
            }
            // Nothing left to refuse. A request carrying a violation this service serves
            // is served HERE, because the framework's own sequence would refuse it; every
            // other request goes through that sequence unchanged.
            if served {
                if let Some(response) = self.served(svc, req, ctx).await {
                    return response;
                }
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition`, which reads identity,
        // protocol and the routing table straight off that entry and adds the request-id
        // header to the three error renderers (see the module docs).
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service speaks `awsJson1_1` and several of its operations are called
        // with NO SigV4 credentials — `InitiateAuth`, `SignUp`, `ConfirmSignUp`,
        // `ForgotPassword`, `RespondToAuthChallenge` and the rest of the sign-in
        // surface carry `@optionalAuth`, since a client has no credentials until it
        // has signed in. The framework resolves a routing key from the SigV4
        // credential scope FIRST and from `X-Amz-Target` only when there is no scope,
        // and those two carry different names: the scope carries the signing name
        // (`cognito-idp`), the target carries the model's service shape name
        // (`AWSCognitoIdentityProviderService.<Operation>`). Claiming the signing name
        // alone therefore serves every SIGNED call and leaves every unsigned one
        // unclaimed, which the router answers `UnknownService` — the service is
        // mounted, but nothing claims the request.
        //
        // Both names address this one endpoint, so both are its routing keys, and the
        // framework's own resolver still decides which one a given request resolves
        // to: a request signed for ANOTHER service resolves to that service's signing
        // name and is not claimed here however it is addressed. This service's signing
        // name is its own, so the claim needs no further discriminator.
        //
        // Both halves are read with the framework's OWN readers
        // (`emulator_server`), which is all this service needs: its published target
        // prefix is a single Smithy identifier, so the segment before the first dot
        // that `service_from_target` reads IS the service shape's name. A service whose
        // published prefix is a fully qualified Coral name carries dots of its own and
        // needs `service_common::rpc_target`'s reading of the LAST prefix segment
        // instead — that is what the CloudTrail and CodeConnections crates claim
        // through, and where this rule belongs for them.
        routes_to_signing_name(parts, self.entry.identity().signing_name())
            || unsigned_target_names_shape(parts, self.entry.identity().shape_id())
            // The hosted UI logo image this service hands out a locator for
            // (`UICustomizationType.ImageUrl`) is fetched by an anonymous
            // browser, with no credential scope and no `X-Amz-Target` to name
            // this service by — only the path this service itself published. A
            // GET of exactly that path is therefore claimed here, so the image
            // is reachable at the URL this service reported; every other path
            // still needs one of the two routing keys above.
            || (parts.method == http::Method::GET
                && hosted_logo_route(&parts.uri.to_string()).is_some())
            // A pool's well-known documents are fetched by whatever verifier or
            // relying party holds a token this service minted, or bootstraps
            // from the pool's discovery document, which follows the token's
            // `iss` claim to `<iss>/.well-known/...` — anonymously, with no
            // credential scope and no `X-Amz-Target`, and at the endpoint the
            // issuer names, which is the endpoint the token's holder reached
            // this service on. A GET of exactly those paths is therefore claimed
            // here too, so each document resolves wherever the issuer announced
            // it rather than only at this service's published AWS host.
            || (parts.method == http::Method::GET
                && well_known_route(&parts.uri.to_string()).is_some())
            // The CSV upload target of a user import job
            // (`UserImportJobType.PreSignedUrl`) is written by whatever client
            // holds the locator this service handed out, with no credential
            // scope and no `X-Amz-Target` either — exactly as a pre-signed S3
            // URL is written. A PUT of exactly that path is therefore claimed
            // here, so the file can really be uploaded to the URL this service
            // reported.
            || (parts.method == http::Method::PUT
                && user_import_csv_route(&parts.uri.to_string()).is_some())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in
        // [`WithSubmittedValues`] so a constraint refusal states the value the caller
        // submitted rather than the framework's summary of it, in
        // [`iam_authorization::IamAuthorized`] so a caller IAM does not allow is refused
        // before the operation reads or writes anything, in [`BehindWebAcl`] so
        // the web ACL in front of the pool decides the request first, and in
        // [`RequestIdReportingOperation::header`] so whatever of those layers answers, the
        // response names the request it answered. A key the model does not
        // name resolves to `None` here exactly as it does in the generated table (the two
        // non-modelled GET routes below are among them), so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.with_submitted_values.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Amazon Cognito states a request that breaks a modelled rule — an
        // absent required member, a value outside an enumerated set, a length or
        // range violation — as `InvalidParameterException`, the 400 every
        // operation of this service declares for a bad parameter (OPERATIONS.md
        // lists `InvalidParameterException (HTTP 400)` on each one). The
        // framework's generic `ValidationException` is not a code this service
        // models, and a client branches on the code, so only the code changes
        // here: the message stays the framework's own rendering of the
        // violations, which already names the member and the constraint it
        // missed.
        //
        // [`WithSubmittedValues`] answers a refusal it could collect before the walk
        // reaches here, which is every refusal of a request this protocol could read;
        // this seam remains for the rest (a body the codec cannot open at all), so one
        // request that breaks a modelled rule is never worded two ways.
        match refusal {
            Refusal::Invalid(violations) => {
                // The requirement rendering is the same one the wrapper states
                // ([`requirement_as_stated`]), so one request's expression cannot be
                // quoted two ways and one enum's set cannot be listed two ways — and the
                // sentence a request naming no pool earns is the same one
                // ([`absent_user_pool_id_sentence`]), so that refusal is not worded one way
                // here and another there.
                let message = absent_user_pool_id_sentence(violations.iter()).unwrap_or_else(|| {
                    let violations: Vec<Violation> =
                        violations.iter().map(requirement_as_stated).collect();
                    ::aws_smithy_schema_server::constraints::envelope(&violations)
                        .unwrap_or_else(|| "invalid request".to_string())
                });
                Some(Refused {
                    code: INVALID_PARAMETER_EXCEPTION.to_string(),
                    message,
                })
            }
            // An unauthenticated caller is the emulator's own refusal, not this
            // service's, so it keeps the framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. A user-pool endpoint serves routes this service's
        // Smithy model does not describe. The first two are the WELL-KNOWN
        // documents a pool publishes under its token issuer: its JSON Web Key
        // Set at `/<UserPoolId>/.well-known/jwks.json`, which every verifier of
        // a Cognito token fetches to check the token's signature under the
        // algorithm the key set itself names, and its OpenID Connect discovery
        // document at `/<UserPoolId>/.well-known/openid-configuration`, which
        // every relying party reads the pool's endpoints, its signing algorithm
        // and the location of that key set from (Amazon Cognito Developer Guide,
        // "Verifying a JSON web token"). Both carry only PUBLIC values and AWS
        // serves them unauthenticated, which is what this seam gives: the router
        // consults it before it authenticates anything, so an anonymous GET
        // reaches each document the same way it does on AWS. The third is the
        // classic hosted UI logo image of one level of a pool, which
        // `UICustomizationType.ImageUrl` locates for an anonymous browser to
        // fetch. Every other unrouted request keeps the framework's
        // protocol-correct refusal.
        // The CSV upload target of a user import job is written with PUT, so it
        // is answered before the GET routes below: the locator this service
        // published describes an upload, and the job records the file that
        // arrived so `StartUserImportJob` has something to import.
        if req.method() == "PUT" {
            let (user_pool_id, job_id) = user_import_csv_route(req.uri())?;
            let state = crate::state::service_state(&*self.engine)?;
            // The router materialises a request before it dispatches one, so the
            // body is already in memory here; only its LENGTH is recorded, so
            // nothing proportional to the upload is retained.
            let body = req.body().bytes()?;
            let now = ::std::time::SystemTime::now();
            {
                let mut guard = state.lock();
                // A path naming no stored job is not this route: it falls
                // through to the framework's refusal rather than accepting an
                // upload nothing will import.
                let job = guard
                    .pool_mut(user_pool_id)?
                    .user_import_jobs
                    .get_mut(job_id)?;
                job.record_csv_upload(body.len(), now);
            }
            // The answer a pre-signed S3 PUT gives once the object is written:
            // 200 with no body.
            let mut response = aws_smithy_runtime_api::http::Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(200).ok()?,
                ::aws_smithy_types::body::SdkBody::empty(),
            );
            response.headers_mut().insert("content-length", "0");
            return Some(response);
        }
        if req.method() != "GET" {
            return None;
        }
        let state = crate::state::service_state(&*self.engine)?;
        if let Some((user_pool_id, client_id)) = hosted_logo_route(req.uri()) {
            // A path naming no stored image is not this route: it falls through
            // to the framework's refusal rather than answering an empty body.
            // Rendering the STORED bytes is a pure read.
            let image = {
                let guard = state.lock();
                guard.pool(user_pool_id)?.hosted_logo(client_id)?.to_vec()
            };
            let media_type = logo_media_type(&image);
            let mut response = aws_smithy_runtime_api::http::Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(200).ok()?,
                ::aws_smithy_types::body::SdkBody::from(image),
            );
            response.headers_mut().insert("content-type", media_type);
            return Some(response);
        }
        let (user_pool_id, document) = well_known_route(req.uri())?;
        let rendered = {
            let guard = state.lock();
            // A path naming no stored pool IS this route — the pool id has the
            // form this service mints one in, so the request addressed a pool of
            // this emulator — and the endpoint answers for the pool that does
            // not exist rather than letting the request fall through to the
            // router's "no service claimed this request" refusal. Rendering a
            // stored document is a pure read: no key is generated here, and
            // nothing is written.
            let Some(pool) = guard.pool(user_pool_id) else {
                let refusal = ::serde_json::json!({
                    "message": crate::state::user_pool_not_found_message(user_pool_id),
                });
                return json_response(404, &refusal.to_string());
            };
            match document {
                WellKnownDocument::KeySet => pool.jwks(),
                // The discovery document names its issuer and every endpoint
                // under it, so it is rendered for the endpoint THIS request
                // reached — the same endpoint the `iss` claim of a token served
                // here names.
                WellKnownDocument::OpenIdConfiguration => {
                    pool.openid_configuration(&crate::state::served_endpoint_of(
                        req.uri(),
                        req.headers().get("host"),
                        &pool.region,
                    ))
                }
            }
        };
        json_response(200, &rendered.to_string())
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model-derived protocol is `awsJson1_1`, and so is this one: it delegates
        // routing, request deserialization and the success body to the framework's own
        // `awsJson1_1`, and differs only in how an ERROR is answered — the status every
        // refusal of the caller carries and the `x-amzn-errortype` naming its error
        // shape, both of this endpoint's recorded wire. See [`error_wire`] for the rule.
        Some(&self.protocol)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

/// A JSON document answered on one of this endpoint's non-modelled routes:
/// `status` with `body` and the media type the body is in.
///
/// The shared rendering ([`::service_common::openid_discovery::json_response`]), which every
/// endpoint of this emulator that publishes such a document answers through.
///
/// `None` only for a status that is not a valid HTTP status code, which the two
/// callers' literals are not.
fn json_response(status: u16, body: &str) -> Option<aws_smithy_runtime_api::http::Response> {
    ::service_common::openid_discovery::json_response(status, body)
}

/// The `UserPoolId` and the document a request path of the form
/// `/<UserPoolId>/.well-known/<document>` addresses, and `None` for every other
/// path.
///
/// These are the well-known documents a user-pool endpoint publishes under a
/// pool's token issuer, which is `<endpoint>/<UserPoolId>` — exactly as the `iss`
/// claim of a token this service issued states it. The reading is the shared one
/// ([`::service_common::openid_discovery::well_known_route`]), over the same paths the
/// pool's own published locators are built from, so a document this service announces and
/// the route that serves it cannot drift. The pool id has to have the FORM this service
/// mints one in, which is what this service answers for, so a path of the same shape
/// addressed to another mounted service is not claimed here.
fn well_known_route(uri: &str) -> Option<(&str, WellKnownDocument)> {
    ::service_common::openid_discovery::well_known_route(uri, &|user_pool_id| {
        is_user_pool_id_shaped(user_pool_id)
    })
}

/// The pool and the level whose hosted UI logo image `uri` addresses, for a
/// request path of the form `/<UserPoolId>/<ClientId>/assets/login_logo.png`,
/// and `None` for every other path.
///
/// This is the path [`crate::state::hosted_logo_url`] builds — the two read the
/// same [`crate::state::HOSTED_LOGO_FILE`], so a locator this service hands out
/// and the route that serves it cannot drift. Both segments have to have the
/// FORM this service mints them in, so a path of the same shape addressed to
/// another mounted service (a path-style object GET, say) is not mistaken for a
/// locator this service published.
fn hosted_logo_route(uri: &str) -> Option<(&str, &str)> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let (user_pool_id, tail) = path.strip_prefix('/')?.split_once('/')?;
    let (client_id, file) = tail.split_once('/')?;
    (file == crate::state::HOSTED_LOGO_FILE
        && is_user_pool_id_shaped(user_pool_id)
        && is_client_id_shaped(client_id))
    .then_some((user_pool_id, client_id))
}

/// The pool and the job whose user-import CSV upload target `uri` addresses, for
/// a request path of the form `/user-import-jobs/<UserPoolId>/<JobId>`, and
/// `None` for every other path.
///
/// This is the path [`crate::state::user_import_csv_upload_url`] builds — the two
/// read the same [`crate::state::USER_IMPORT_CSV_ROUTE`], so a locator this
/// service hands out and the route that accepts the upload cannot drift. Both
/// ids have to have the FORM this service mints them in, so a path of the same
/// shape addressed to another mounted service is not mistaken for a locator this
/// service published.
fn user_import_csv_route(uri: &str) -> Option<(&str, &str)> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let (prefix, tail) = path.strip_prefix('/')?.split_once('/')?;
    if prefix != crate::state::USER_IMPORT_CSV_ROUTE {
        return None;
    }
    let (user_pool_id, job_id) = tail.split_once('/')?;
    (is_user_pool_id_shaped(user_pool_id) && crate::state::is_user_import_job_id_shaped(job_id))
        .then_some((user_pool_id, job_id))
}

/// Whether `candidate` has the form of a `UserPoolIdType`: the modelled pattern
/// `^[\w-]+_[0-9a-zA-Z]+$` and length 1..55 (this service's model). The suffix
/// carries no underscore, so the LAST one separates the two halves.
fn is_user_pool_id_shaped(candidate: &str) -> bool {
    let Some((prefix, suffix)) = candidate.rsplit_once('_') else {
        return false;
    };
    candidate.len() <= 55
        && !prefix.is_empty()
        && !suffix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `candidate` has the form of a `ClientIdType`: the modelled pattern
/// `^[\w+]+$` and length 1..128 (this service's model), which the pool-level
/// default [`crate::state::UI_CUSTOMIZATION_ALL_CLIENTS`] also satisfies.
fn is_client_id_shaped(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 128
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+'))
}

/// The media type a stored logo image is served as, read from the image's own
/// leading bytes: the file formats a browser renders a login logo in each
/// announce themselves there (PNG, RFC 2083 §3.1; JPEG, ISO/IEC 10918-1;
/// GIF87a and GIF89a; the RIFF container WebP rides in). Bytes that announce
/// none of them are served as unnamed binary rather than as an image they are
/// not.
fn logo_media_type(image: &[u8]) -> &'static str {
    if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if image.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if image.starts_with(b"GIF87a") || image.starts_with(b"GIF89a") {
        "image/gif"
    } else if image.starts_with(b"RIFF") && image.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Whether the routing key the framework resolves for `parts` is `signing_name`.
///
/// This is `emulator_server::can_handle_based_on_route_key`'s rule applied over
/// `Parts`: that helper takes a whole buffered `http::Request`, which the
/// `Service::can_handle` seam does not receive. Comparison is case-insensitive on a
/// trimmed name, as there. It answers for every SIGNED request (the credential scope
/// carries the signing name) and for one addressed by this service's endpoint host.
fn routes_to_signing_name(parts: &http::request::Parts, signing_name: &str) -> bool {
    smithy_server_runtime::resolve_route_key(parts)
        .is_some_and(|key| key.eq_ignore_ascii_case(signing_name.trim()))
}

/// Whether `parts` carries NO SigV4 credential scope, names the service whose shape id
/// is `service_shape_id` in its `X-Amz-Target`, and arrives on the path this service's
/// protocol uses.
///
/// The three conjuncts, each for its own reason:
///
/// * NO credential scope — neither an `Authorization` header nor a presigned
///   `X-Amz-Credential` query parameter. The framework resolves a routing key from the
///   scope FIRST (`emulator_server::resolve_route_key`), so a request signed for another
///   service resolves to that service and must not be claimed here however it is
///   targeted; reading the target only in the scope's absence keeps the framework's
///   precedence instead of competing with it.
/// * the target NAMES this service — read with the framework's own
///   `service_from_target`, which takes the segment before the first dot and normalises
///   it the way a routing key is normalised. This service's published target prefix is a
///   single Smithy identifier (`AWSCognitoIdentityProviderService`), so that segment IS
///   the shape's local name; the comparison is therefore against the shape id the
///   generated identity states, with its namespace dropped, and nothing is written out
///   here that the model does not state.
/// * the ROOT path, which is the only path an `awsJson` operation posts to. A request
///   that names this service out of band while addressing some other model's REST path
///   contradicts itself, so it must reach NOBODY rather than a handler picked off the
///   target alone.
fn unsigned_target_names_shape(parts: &http::request::Parts, service_shape_id: &str) -> bool {
    if emulator_server::service_from_sigv4(&parts.headers).is_some()
        || emulator_server::service_from_sigv4_query(&parts.uri).is_some()
    {
        return false;
    }
    let shape_name = service_shape_id
        .split('#')
        .next_back()
        .unwrap_or(service_shape_id)
        .trim();
    emulator_server::service_from_target(&parts.headers)
        .is_some_and(|named| named.eq_ignore_ascii_case(shape_name))
        && parts.uri.path().trim_matches('/').is_empty()
}

#[cfg(test)]
mod tests {
    //! The two names this ONE `awsJson` endpoint answers on, over the signals a request
    //! can carry, beside the two non-modelled paths this endpoint serves a document and
    //! an image on. `tests/all/initiate_auth.rs` asserts the routing rule on the wire,
    //! through the router, `tests/all/token_signing.rs` fetches the key set over it and
    //! `tests/all/ui_customization.rs` fetches a stored logo over it; these tables are
    //! the rules themselves, row by row.
    //!
    //! One more thing a wire test cannot see lives here: that the hand-maintained
    //! `wrap_operations!` list is the WHOLE surface.

    use super::{
        hosted_logo_route, logo_media_type, routes_to_signing_name, unsigned_target_names_shape,
        well_known_route, EmulatedService, ErasedServiceDefinition, WellKnownDocument,
    };
    use std::sync::Arc;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`super::WithSubmittedValues`], under the same key the generated routing
    /// table is keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// added to the model and not to `wrap_operations!` fails here instead of silently
    /// stating a summarised value in a refusal — and a key spelled differently from the
    /// generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_its_submitted_values() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            operations.len() > 100,
            "case the model's surface: expected this service's operations, got {} of them",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.with_submitted_values.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 WithSubmittedValues; this one is missing from wrap_operations!, so a \
                 constraint refusal of it states the framework's summary of a value instead \
                 of the value the caller submitted"
            );
        }
    }

    /// [`hosted_logo_route`] over the paths that reach this service: the locator
    /// this service publishes, and the paths that are not it — including one of
    /// the same SHAPE whose segments are not the form this service mints, which
    /// another mounted service may own.
    #[test]
    fn only_a_published_logo_locator_names_a_pool_and_a_level() {
        /// One row: a path that reaches this service, and the pool and level it
        /// addresses a logo of, if any.
        struct Row {
            case: &'static str,
            path: &'static str,
            expected: Option<(&'static str, &'static str)>,
        }

        let rows = [
            Row {
                case: "the pool-level default's logo",
                path: "/us-east-1_abc123def/ALL/assets/login_logo.png",
                expected: Some(("us-east-1_abc123def", "ALL")),
            },
            Row {
                case: "an app client's own logo",
                path: "/us-west-2_0a9b8c7d6/1h57kf5cpq17m0eml12eu8b4cg/assets/login_logo.png",
                expected: Some(("us-west-2_0a9b8c7d6", "1h57kf5cpq17m0eml12eu8b4cg")),
            },
            Row {
                case: "a logo with a cache-busting query",
                path: "/us-east-1_abc123def/ALL/assets/login_logo.png?v=2",
                expected: Some(("us-east-1_abc123def", "ALL")),
            },
            Row {
                case: "a path-style object of the same shape, whose first segment is no pool id",
                path: "/my-bucket/branding/assets/login_logo.png",
                expected: None,
            },
            Row {
                case: "a level whose id is outside ClientIdType's pattern",
                path: "/us-east-1_abc123def/not a client/assets/login_logo.png",
                expected: None,
            },
            Row {
                case: "another asset of the same level",
                path: "/us-east-1_abc123def/ALL/assets/background.png",
                expected: None,
            },
            Row {
                case: "the pool's key set, which is the other non-modelled route",
                path: "/us-east-1_abc123def/.well-known/jwks.json",
                expected: None,
            },
            Row {
                case: "the root path, where every modelled call goes",
                path: "/",
                expected: None,
            },
            Row {
                case: "a logo path with no leading slash",
                path: "us-east-1_abc123def/ALL/assets/login_logo.png",
                expected: None,
            },
        ];

        for row in rows {
            let actual = hosted_logo_route(row.path);
            assert_eq!(
                actual, row.expected,
                "case {:?} ({}): expected {:?}, got {actual:?}",
                row.case, row.path, row.expected
            );
        }
    }

    /// [`logo_media_type`] over the leading bytes each image format announces
    /// itself with, and over bytes that announce none.
    #[test]
    fn a_logo_is_served_as_the_format_its_own_bytes_announce() {
        // (case, leading bytes, expected media type)
        let rows: &[(&str, &[u8], &str)] = &[
            ("a PNG", b"\x89PNG\r\n\x1a\n rest", "image/png"),
            ("a JPEG", b"\xff\xd8\xff\xe0 rest", "image/jpeg"),
            ("a GIF87a", b"GIF87a rest", "image/gif"),
            ("a GIF89a", b"GIF89a rest", "image/gif"),
            ("a WebP", b"RIFF\x10\x00\x00\x00WEBPVP8 ", "image/webp"),
            (
                "a RIFF container that is not WebP",
                b"RIFF\x10\x00\x00\x00WAVEfmt ",
                "application/octet-stream",
            ),
            (
                "markup, which is never served as markup",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                "application/octet-stream",
            ),
            ("bytes too short to announce anything", b"\x89P", "application/octet-stream"),
        ];

        for (case, image, expected) in rows {
            let actual = logo_media_type(image);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// [`well_known_route`] over the paths that reach this service: each
    /// well-known document a pool publishes, and the paths that are neither.
    #[test]
    fn only_a_pools_well_known_document_paths_name_a_pool_and_a_document() {
        /// One row: a path that reaches this service, and the pool and the
        /// document it addresses, if any.
        struct Row {
            case: &'static str,
            path: &'static str,
            expected: Option<(&'static str, WellKnownDocument)>,
        }

        let rows = [
            Row {
                case: "a pool's key set",
                path: "/us-east-1_abc123def/.well-known/jwks.json",
                expected: Some(("us-east-1_abc123def", WellKnownDocument::KeySet)),
            },
            Row {
                case: "a pool's key set with a query string",
                path: "/us-west-2_0a9b8c7d6/.well-known/jwks.json?cachebust=1",
                expected: Some(("us-west-2_0a9b8c7d6", WellKnownDocument::KeySet)),
            },
            Row {
                case: "a pool's discovery document",
                path: "/us-east-1_abc123def/.well-known/openid-configuration",
                expected: Some((
                    "us-east-1_abc123def",
                    WellKnownDocument::OpenIdConfiguration,
                )),
            },
            Row {
                case: "a pool's discovery document with a query string",
                path: "/us-west-2_0a9b8c7d6/.well-known/openid-configuration?v=1",
                expected: Some((
                    "us-west-2_0a9b8c7d6",
                    WellKnownDocument::OpenIdConfiguration,
                )),
            },
            Row {
                case: "the root path, where every modelled call goes",
                path: "/",
                expected: None,
            },
            Row {
                case: "a well-known document no pool publishes",
                path: "/us-east-1_abc123def/.well-known/oauth-authorization-server",
                expected: None,
            },
            Row {
                case: "a key set with no pool id",
                path: "//.well-known/jwks.json",
                expected: None,
            },
            Row {
                case: "a first segment that is no pool id",
                path: "/my-bucket/.well-known/jwks.json",
                expected: None,
            },
            Row {
                case: "a deeper path under a pool",
                path: "/us-east-1_abc123def/oauth2/token",
                expected: None,
            },
            Row {
                case: "a path with no leading slash",
                path: "jwks.json",
                expected: None,
            },
        ];

        for row in rows {
            let actual = well_known_route(row.path);
            assert_eq!(
                actual, row.expected,
                "case {:?} ({}): expected {:?}, got {actual:?}",
                row.case, row.path, row.expected
            );
        }
    }

    /// This service's SigV4 signing name and Smithy shape id, as the generated identity
    /// states them (`smithy_query(query="service_info")`: `sigv4Name` `cognito-idp`,
    /// service shape `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`).
    const SIGNING_NAME: &str = "cognito-idp";
    const SHAPE_ID: &str = "com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService";

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// One row: how the request is addressed — its credential scope, its `X-Amz-Target`,
    /// its `Host` and its request URI, each absent as `None` — and whether this service
    /// claims it.
    struct Row {
        case: &'static str,
        scope: Option<&'static str>,
        target: Option<&'static str>,
        host: Option<&'static str>,
        uri: &'static str,
        expected: bool,
    }

    /// The path every `awsJson` operation posts to.
    const ROOT: &str = "/";

    #[test]
    fn this_endpoint_answers_on_its_signing_name_and_on_its_shape_name() {
        let rows = [
            Row {
                case: "signed for it, targeted at it",
                scope: Some(SIGNING_NAME),
                target: Some("AWSCognitoIdentityProviderService.AdminGetUser"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                // The unsigned sign-in call this rule exists for: no credential scope,
                // so the target is all that names the service.
                case: "unsigned, targeted at it",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at it, on the emulator's own host",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: Some("127.0.0.1:9297"),
                uri: ROOT,
                expected: true,
            },
            Row {
                // The target tier outranks the `Host` tier in the framework's own
                // resolver, so the peer that host names resolves the same
                // target-derived key and does not claim this request either.
                case: "unsigned, targeted at it, on another service's endpoint host",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: Some("lambda.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, lower-cased target prefix",
                scope: None,
                target: Some("awscognitoidentityproviderservice.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at the separate Cognito Identity service",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetId"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // The credential scope outranks the target, so a request signed for
                // another service does not reach this one.
                case: "signed for another service, targeted at it",
                scope: Some("lambda"),
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // A request that names this service out of band while addressing
                // another model's REST path contradicts itself: it reaches nobody.
                case: "unsigned, targeted at it, on a REST path",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: "/identitypools/pool-1",
                expected: false,
            },
            Row {
                case: "unsigned, addressed by its endpoint host",
                scope: None,
                target: None,
                host: Some("cognito-idp.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "no routing signal at all",
                scope: None,
                target: None,
                host: None,
                uri: ROOT,
                expected: false,
            },
        ];

        for row in rows {
            let mut builder = super::http::Request::builder()
                .method("POST")
                .uri(row.uri);
            if let Some(scope) = row.scope {
                builder = builder.header("authorization", authorization(scope));
            }
            if let Some(target) = row.target {
                builder = builder.header("x-amz-target", target);
            }
            if let Some(host) = row.host {
                builder = builder.header("host", host);
            }
            let parts = builder
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0;
            let actual = routes_to_signing_name(&parts, SIGNING_NAME)
                || unsigned_target_names_shape(&parts, SHAPE_ID);
            assert_eq!(
                actual, row.expected,
                "case {:?}: expected this service to claim it == {}, got {actual}",
                row.case, row.expected
            );
        }
    }

    /// RULE: every member name [`super::PATTERN_QUOTED_AS_WRITTEN`] holds is a member THIS
    /// MODEL declares, at a position that carries a `@pattern` whose text is anchored at
    /// both ends — so the set cannot name a member the model does not have, or one whose
    /// expression the rendering would not change.
    ///
    /// Read off the model rather than restated, so a member renamed or a pattern rewritten
    /// in the model fails here instead of leaving a stale name behind in the set.
    #[test]
    fn every_member_quoted_as_written_is_a_patterned_member_of_this_model() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");

        /// The `@pattern` the shape `target` declares, if any. A member whose target is a
        /// prelude shape this model does not declare has none.
        fn declared_pattern<'m>(
            shapes: &'m serde_json::Map<String, serde_json::Value>,
            target: &str,
        ) -> Option<&'m str> {
            shapes
                .get(target)?
                .get("traits")?
                .get("smithy.api#pattern")?
                .as_str()
        }

        for member in super::PATTERN_QUOTED_AS_WRITTEN {
            // Every structure member of the model whose lowerCamel name is this one, with
            // the pattern its target declares.
            let patterns: Vec<&str> = shapes
                .values()
                .filter(|shape| shape["type"] == "structure")
                .filter_map(|shape| shape["members"].as_object())
                .flat_map(|members| members.iter())
                .filter(|(name, _)| super::constraints::path::member(name) == *member)
                .filter_map(|(_, declaration)| {
                    declared_pattern(shapes, declaration["target"].as_str()?)
                })
                .collect();
            assert!(
                !patterns.is_empty(),
                "case {member:?}: the set names a member this model declares with a declared \
                 @pattern; the model has no such patterned member"
            );
            for pattern in patterns {
                assert!(
                    pattern.starts_with('^') && pattern.ends_with('$'),
                    "case {member:?}: its modelled expression is anchored at both ends, so \
                     quoting it as written is what distinguishes this member; got {pattern:?}"
                );
                assert_eq!(
                    super::pattern_as_stated(member, pattern),
                    pattern,
                    "case {member:?}: its clause quotes the modelled expression as written, got \
                     {:?}",
                    super::pattern_as_stated(member, pattern)
                );
                assert_eq!(
                    super::pattern_as_stated("userPoolId", pattern),
                    super::unanchored(pattern),
                    "case {member:?}: the same expression on a member outside the set loses the \
                     whole-value pair, got {:?}",
                    super::pattern_as_stated("userPoolId", pattern)
                );
            }
        }
    }

    /// RULE: the member the null-check sentence names
    /// ([`super::absent_user_pool_id_sentence`]) is a member THIS MODEL declares, spelled as
    /// the model spells it, and the path the sentence keys on is the validation path the
    /// framework spells that same member as.
    ///
    /// Read off the model rather than restated, so the member renamed in the model fails here
    /// instead of leaving a sentence naming a member no request can carry.
    #[test]
    fn the_pool_id_the_null_check_names_is_this_models_member() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");

        // Every operation input of this model that REQUIRES the member the sentence names.
        let requiring: Vec<&String> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "structure")
            .filter(|(_, shape)| {
                shape["members"][super::USER_POOL_ID_MEMBER]["traits"]
                    .get("smithy.api#required")
                    .is_some()
            })
            .map(|(name, _)| name)
            .collect();
        assert!(
            !requiring.is_empty(),
            "the sentence names {:?}, which this model declares as a required member of no \
             request at all",
            super::USER_POOL_ID_MEMBER
        );
        assert_eq!(
            super::constraints::path::member(super::USER_POOL_ID_MEMBER),
            super::USER_POOL_ID_MEMBER_PATH,
            "the path the sentence keys on must be the one the framework spells {:?} as, got {:?}",
            super::USER_POOL_ID_MEMBER,
            super::constraints::path::member(super::USER_POOL_ID_MEMBER)
        );
    }
}
