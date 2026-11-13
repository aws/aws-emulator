// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice service.
//!
//! One [`UserPool`] per created pool, each owning its app clients and its users,
//! beside the auth sessions and the tokens the sign-in operations issue. Every
//! operation computes its response from this state — a create stores a resource
//! and a later read renders the STORED resource, never the request that made it.

/// The threat-protection event log of a pool's users: the authentication events a
/// pool with its add-ons switched on records, and the feedback left on one.
pub mod auth_events;
/// The CloudWatch series a pool's own authentication and registration traffic publishes:
/// what one terminated attempt observes, and where that observation is stored.
pub mod auth_metrics;
/// The user-pool Lambda-trigger dispatch: a pool's [`LambdaConfigType`] member becomes an
/// invocation of the function it names, with the event for the stage the invoking
/// operation reached.
pub mod lambda_triggers;
/// The stored user pool itself: its record and the configuration it holds.
mod user_pool;
/// The resources a pool holds beside its users: hosted domains, replicas and app
/// clients.
mod pool_resources;
/// The cross-member preconditions a user-pool write has to satisfy before the
/// pool is stored.
mod pool_preconditions;
/// The stored user of a pool, and the import jobs that create users in bulk.
mod users;
/// A pool's third-party identity providers and its managed login branding.
mod identity_providers;
/// A user's attributes, and the pool schema they are judged against.
mod attributes;
/// The column order a pool's user-import CSV template carries.
mod csv_header;
/// The tokens a pool issues, and the key its JWTs are signed with.
mod tokens;
/// The X.509 certificate a pool signs its SAML 2.0 federation traffic with, and
/// the SAML name it is issued to.
mod saml_signing_certificate;
/// The form a refresh token travels in: the JWE compact serialization it is vended as,
/// and the key this service seals one under.
mod refresh_token_jwe;
/// What a pool publishes about itself under its token issuer: its OpenID Connect
/// discovery document and the paths it publishes its documents at.
mod openid_discovery;
/// What a pool says about a token another service was presented: the server-side
/// validation an integrated identity pool asks for.
mod presented_tokens;
/// The token lifetimes an app client may be configured with, and the range each
/// one's duration has to lie in.
mod token_validity;
/// The software token a user sets up as a TOTP authenticator: the private key an
/// association mints for it and the one-time password a registration presents.
mod software_tokens;
pub use attributes::*;
pub use csv_header::*;
pub use presented_tokens::*;
pub use identity_providers::*;
pub use openid_discovery::*;
pub use pool_preconditions::*;
pub use pool_resources::*;
pub use refresh_token_jwe::*;
pub use saml_signing_certificate::*;
pub use software_tokens::*;
pub use token_validity::*;
pub use tokens::*;
pub use user_pool::*;
pub use users::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccountRecoverySettingType, AccountTakeoverRiskConfigurationType, AcrLevelConfigType,
    AdminCreateUserConfigType,
    AnalyticsConfigurationType, AttributeType, AuthenticationResultType,
    ClientAuthenticationResultType, ClientSecretDescriptorType,
    CodeDeliveryDetailsType, CompromisedCredentialsRiskConfigurationType, CustomDomainConfigType,
    DeviceConfigurationType, DeviceSecretVerifierConfigType, DeviceType, DomainDescriptionType,
    EmailConfigurationType, EmailMfaConfigType, GroupType, IdentityProviderType,
    IssuerConfigurationType,
    KeyConfigurationType, LambdaConfigType, LimitDefinitionType, LimitType,
    LogConfigurationType, LogDeliveryConfigurationType,
    MFAOptionType, NewDeviceMetadataType,
    NumberAttributeConstraintsType, PasswordPolicyType, ProviderUserIdentifierType,
    RecoveryOptionType,
    RefreshTokenRotationType,
    ResourceServerScopeType, ResourceServerType, RiskConfigurationType,
    RiskExceptionConfigurationType, RoutingType,
    SchemaAttributeType, SignInPolicyType, SmsConfigurationType, SmsMfaConfigType,
    SoftwareTokenMfaConfigType, StringAttributeConstraintsType,
    TermsDescriptionType, TermsType, TokenValidityUnitsType, UserAttributeUpdateSettingsType,
    UserImportJobType,
    UserPoolAddOnsType,
    UserPoolClientType, UserPoolDescriptionType, UserPoolPolicyType, UserPoolReplicaType,
    UserPoolType, UserType,
    UsernameConfigurationType, VerificationMessageTemplateType, WebAuthnConfigurationType,
    WebAuthnCredentialDescription,
};

/// The region a request that carries no SigV4 credential scope falls back to, so
/// a minted pool id and ARN always name one region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service's SigV4 / endpoint / ARN namespace (OPERATIONS.md "Wire protocol").
pub const SIGNING_NAME: &str = "cognito-idp";

/// The resource type a user pool's ARN names: a pool is
/// `arn:<partition>:cognito-idp:<region>:<account>:userpool/<UserPoolId>`, the ARN
/// form [`UserPool::arn`] carries and the one a tagging operation's `ResourceArn`
/// addresses a pool by.
pub const USER_POOL_RESOURCE_TYPE: &str = "userpool";

/// The most tags one user pool holds: "A user pool can have as many as 50 tags"
/// (`TagResource` in this service's model).
pub const MAX_TAGS_PER_USER_POOL: usize = 50;

/// The longest a `UserPoolId` is: `UserPoolIdType` is modelled with a maximum
/// length of 55 (`smithy_query(query="shape",shape="UserPoolIdType")`).
const USER_POOL_ID_MAX_LENGTH: usize = 55;

/// Amazon Cognito's default password policy for a new user pool: at least 8
/// characters, with an uppercase letter, a lowercase letter, a number and a
/// symbol required (Amazon Cognito Developer Guide, "Password policies"). The
/// model's `PasswordPolicyType.MinimumLength` range starts at 6, so 8 is a
/// default, not a bound.
const DEFAULT_PASSWORD_MIN_LENGTH: i32 = 8;

/// `PasswordPolicyType.TemporaryPasswordValidityDays`: "Defaults to `7`. If you
/// submit a value of `0`, Amazon Cognito treats it as a null value and sets
/// `TemporaryPasswordValidityDays` to its default value." (this service's model).
const DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS: i32 = 7;

/// `AdminCreateUserConfigType.UnusedAccountValidityDays`: "The default value for
/// this parameter is 7." (this service's model).
const DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS: i32 = 7;

/// `CreateUserPoolClient.AccessTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your access tokens are valid for one
/// hour." (this service's model). `IdTokenValidity` is documented the same way.
const DEFAULT_ACCESS_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// `CreateUserPoolClient.RefreshTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your refresh tokens are valid for 30
/// days", and a submitted `0` is overridden "with the default value of 30 days"
/// (this service's model).
const DEFAULT_REFRESH_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// `CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes, of
/// that session token" and its modelled range is 3..15, so an app client that
/// sets no value gets the range's minimum.
const DEFAULT_AUTH_SESSION_VALIDITY_MINUTES: i32 = 3;

/// `CreateUserPoolClient.RefreshTokenValidity`: "You can't set
/// `RefreshTokenValidity` to 0. If you do, Amazon Cognito overrides the value
/// with the default value of 30 days", and "the default time unit for
/// `RefreshTokenValidity` in an API request is days" (this service's model), so
/// the value an app client holds for a request that asked for none — or asked
/// for the modelled default `0` — is 30.
const DEFAULT_REFRESH_TOKEN_VALIDITY: i32 = 30;

/// The `RefreshTokenValidity` an app client holds when a create or an update
/// carried `requested`.
///
/// The member is modelled with a default of `0`, so the deserializer hands a
/// request that named no value the same `0` a request that named one does, and
/// `0` is the one value an app client cannot hold:
/// [`DEFAULT_REFRESH_TOKEN_VALIDITY`] replaces it.
pub fn effective_refresh_token_validity(requested: i32) -> i32 {
    if requested == 0 {
        DEFAULT_REFRESH_TOKEN_VALIDITY
    } else {
        requested
    }
}

/// The `AuthSessionValidity` an app client holds when a create or an update
/// carried `requested`: the value asked for, or the service default every app
/// client gets.
pub fn effective_auth_session_validity(requested: Option<i32>) -> i32 {
    requested.unwrap_or(DEFAULT_AUTH_SESSION_VALIDITY_MINUTES)
}

/// The `TokenValidityUnits` an app client holds when a create or an update
/// carried `requested`.
///
/// Every token-validity member has a documented default time unit of its own, so
/// an app client always has a unit block; one that named no unit holds a block
/// with no unit overridden in it.
pub fn effective_token_validity_units(
    requested: Option<TokenValidityUnitsType>,
) -> TokenValidityUnitsType {
    requested.unwrap_or_default()
}

/// `CreateUserPoolClient.ExplicitAuthFlows`: "If you don't specify a value for
/// `ExplicitAuthFlows`, your app client supports `ALLOW_REFRESH_TOKEN_AUTH`,
/// `ALLOW_USER_SRP_AUTH`, and `ALLOW_CUSTOM_AUTH`." (this service's model).
pub const DEFAULT_EXPLICIT_AUTH_FLOWS: [&str; 3] = [
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_USER_SRP_AUTH",
    "ALLOW_CUSTOM_AUTH",
];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=ADMIN_USER_PASSWORD_AUTH`:
/// the current one and the legacy one it "replaces"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const ADMIN_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ADMIN_NO_SRP_AUTH"];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=USER_PASSWORD_AUTH`:
/// "`ALLOW_USER_PASSWORD_AUTH`: Enable user password-based authentication", beside
/// the legacy `USER_PASSWORD_AUTH` the same member documents
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_USER_PASSWORD_AUTH", "USER_PASSWORD_AUTH"];

/// The `ExplicitAuthFlows` value that enables `AuthFlow=REFRESH_TOKEN_AUTH` and its
/// `REFRESH_TOKEN` alias: "`ALLOW_REFRESH_TOKEN_AUTH`: Enable authflow to refresh
/// tokens" (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model), which
/// is also one of the three a client that sets no `ExplicitAuthFlows` gets.
pub const REFRESH_TOKEN_AUTH_FLOWS: [&str; 1] = ["ALLOW_REFRESH_TOKEN_AUTH"];

/// The `ExplicitAuthFlows` value that enables `AuthFlow=USER_AUTH`:
/// "`ALLOW_USER_AUTH`: Enable selection-based sign-in with `USER_AUTH`"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_AUTH_FLOWS: [&str; 1] = ["ALLOW_USER_AUTH"];

/// The `ExplicitAuthFlows` value that configures an app client for
/// machine-to-machine authorization: "you must configure the app client with a
/// client secret and enable the `ALLOW_CLIENT_TOKEN_AUTH` authentication flow"
/// (`GetClientToken` in this service's model).
pub const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// The message an app-client configuration owes when its `ExplicitAuthFlows`
/// breaks the exclusivity rule [`CLIENT_TOKEN_AUTH_FLOW`] carries: the rule as
/// `GetClientToken` states it in this service's model, word for word. It names no
/// value from the request, so every such refusal carries this one sentence.
pub const EXCLUSIVE_AUTH_FLOW_MESSAGE: &str =
    "The ALLOW_CLIENT_TOKEN_AUTH flow is mutually exclusive with user authentication \
     flows. It must be the only authentication flow that you configure for the app \
     client.";

/// [`EXCLUSIVE_AUTH_FLOW_MESSAGE`] when `flows` breaks that rule, and `None` when
/// it configures an app client the service accepts.
///
/// "The `ALLOW_CLIENT_TOKEN_AUTH` flow is mutually exclusive with user
/// authentication flows. It must be the only authentication flow that you
/// configure for the app client" (`GetClientToken` in this service's model). So a
/// list naming it beside ANOTHER flow describes an app client that cannot exist,
/// while a list naming it alone — or not at all — is a configuration the service
/// accepts. A list that merely repeats it names no other flow, so it is accepted:
/// the rule is about which flows are configured, not how many times each is sent.
/// Both the create and the update path ask this one function, so the two cannot
/// disagree about what an app client may hold.
pub fn exclusive_auth_flow_violation(flows: &[String]) -> Option<&'static str> {
    let names_exclusive_flow = flows.iter().any(|flow| flow == CLIENT_TOKEN_AUTH_FLOW);
    let names_another_flow = flows.iter().any(|flow| flow != CLIENT_TOKEN_AUTH_FLOW);
    (names_exclusive_flow && names_another_flow).then_some(EXCLUSIVE_AUTH_FLOW_MESSAGE)
}

/// The message the ADMINISTRATIVE sign-in refuses with when the app client's
/// stored `ExplicitAuthFlows` do not enable the requested flow.
///
/// `ExplicitAuthFlows` is "the authentication flows that you want your user pool
/// client to support" (`CreateUserPoolClient.ExplicitAuthFlows` in this
/// service's model), so a request whose `AuthFlow` the stored list does not
/// enable asks for a flow this app client is not configured for. The
/// administrative API reports that in ONE fixed sentence: it interpolates
/// neither the flow asked for nor the app client it was asked of, so the refusal
/// is the same for every flow and every client id. The client-side sign-in and
/// the machine-to-machine grant word the same condition with the flow the
/// request asked for ([`auth_flow_not_enabled_message`]) — Amazon Cognito does
/// not word this refusal identically in every API, so each path carries the
/// sentence ITS OWN API answers.
pub const ADMIN_AUTH_FLOW_NOT_ENABLED_MESSAGE: &str = "Auth flow not enabled for this client";

/// The message the CLIENT-SIDE sign-in and the machine-to-machine grant refuse
/// with when the app client's stored `ExplicitAuthFlows` do not enable the
/// requested flow.
///
/// One sentence whose only variable part is the flow the request asked for —
/// `auth_flow`, the `AuthFlowType` member as SUBMITTED, so an alias is echoed as
/// the caller wrote it — and which says nothing else about the app client: the
/// sentence is the same for every client id, so this refusal, which an
/// unauthenticated caller can provoke, carries no identifier of the app client
/// it was sent to. The administrative sign-in's own wording is
/// [`ADMIN_AUTH_FLOW_NOT_ENABLED_MESSAGE`].
pub fn auth_flow_not_enabled_message(auth_flow: &str) -> String {
    format!("{auth_flow} flow not enabled for this client")
}

/// The message an operation refuses with when the app client the request names
/// does not have the FEATURE that operation needs switched on in its stored
/// configuration.
///
/// `UnsupportedOperationException` is "thrown when you attempt to perform an
/// operation that isn't enabled for the user pool client" (this service's model),
/// and Amazon Cognito reports it in ONE fixed sentence with no variable part at
/// all: it interpolates neither the feature asked for, nor the app client, nor the
/// pool, so the refusal reads the same for every client id and tells a caller
/// nothing about the client it addressed. A request such a gate refuses changes
/// nothing, so the sentence is also the whole of what the caller learns.
pub const CLIENT_FEATURE_NOT_ENABLED_MESSAGE: &str = "This feature is not enabled for this client";

/// The `ChallengeParameters` of a successful authentication response that leaves
/// NO challenge outstanding: the empty map.
///
/// `InitiateAuth`, `AdminInitiateAuth`, `RespondToAuthChallenge` and
/// `AdminRespondToAuthChallenge` all model `ChallengeParameters` as a
/// `ChallengeParametersType` map (this service's model), and a successful
/// response carries it whether or not a challenge is outstanding: the parameters
/// of the challenge the caller must answer, or no parameters at all once
/// authentication is complete. A caller therefore reads the map unconditionally
/// and finds it empty rather than absent, so every one of those operations asks
/// this one function for the completed case and none of them can drop the member.
#[must_use]
pub fn completed_challenge_parameters() -> HashMap<String, String> {
    HashMap::new()
}

/// The `ChallengeParameters` of an `MFA_SETUP` challenge for `username`: the user
/// it is for, and the MFA methods that user can set up to satisfy the pool's MFA
/// requirement.
///
/// `MFA_SETUP` is the challenge for a user that "must set up" a second factor
/// before a sign-in completes, and its parameters report the methods available to
/// set up under `MFAS_CAN_SETUP` as a JSON array of the `UserMFASettingList`
/// values they are reported under — "the possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model), which
/// is the order they are listed in — beside the user the challenge belongs to,
/// named exactly as every other challenge of these operations names it
/// ([`new_password_required_challenge_parameters`]).
///
/// Both `InitiateAuth` and `AdminInitiateAuth` start this challenge, and both read
/// its parameters here, so the two cannot report it differently.
#[must_use]
pub fn mfa_setup_challenge_parameters(
    username: &str,
    settings_can_setup: &[String],
) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    parameters.insert(USER_ID_FOR_SRP.to_string(), username.to_string());
    parameters.insert(
        MFAS_CAN_SETUP.to_string(),
        ::serde_json::Value::Array(
            settings_can_setup
                .iter()
                .map(|setting| ::serde_json::Value::String(setting.clone()))
                .collect(),
        )
        .to_string(),
    );
    parameters
}

/// the user it is for, the attributes the challenge response must supply, and the
/// values the user already has.
///
/// `ChallengeResponses` documents the response this challenge takes as
/// `{"NEW_PASSWORD": "[new_password]", "USERNAME": "[username]"}` plus
/// `"userAttributes.[attribute_name]": "[attribute_value]"` (this service's model),
/// so the parameters report the names under that same `userAttributes.` prefix and
/// the current values as a JSON object.
///
/// `userAttributes` reports the user's OWN attributes: `sub` is minted by the pool
/// for every user, is immutable, and is not an attribute any challenge response can
/// carry — the same reason it is never among the required attributes a challenge asks
/// for ([`UserPool::unset_required_attributes`]) — so it is not reported among them,
/// and a user with no other attribute is reported with an empty object.
///
/// Both `InitiateAuth` and `AdminInitiateAuth` start this challenge, and both read
/// its parameters here, so the two cannot report it differently.
#[must_use]
pub fn new_password_required_challenge_parameters(
    username: &str,
    required_attributes: &[String],
    user_attributes: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    parameters.insert(USER_ID_FOR_SRP.to_string(), username.to_string());
    parameters.insert(
        REQUIRED_ATTRIBUTES.to_string(),
        ::serde_json::Value::Array(
            required_attributes
                .iter()
                .map(|name| {
                    ::serde_json::Value::String(format!("{USER_ATTRIBUTES_PREFIX}{name}"))
                })
                .collect(),
        )
        .to_string(),
    );
    parameters.insert(
        USER_ATTRIBUTES.to_string(),
        ::serde_json::Value::Object(
            user_attributes
                .iter()
                .filter(|(name, _)| name.as_str() != SUB_ATTRIBUTE)
                .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
                .collect(),
        )
        .to_string(),
    );
    parameters
}

/// The `ChallengeParameters` entry naming the user a challenge is outstanding for.
const USER_ID_FOR_SRP: &str = "USER_ID_FOR_SRP";

/// The `ChallengeParameters` entry listing the attributes a challenge response must
/// supply.
const REQUIRED_ATTRIBUTES: &str = "requiredAttributes";

/// The `ChallengeParameters` entry carrying the values the user already has.
const USER_ATTRIBUTES: &str = "userAttributes";

/// The `ChallengeParameters` entry of an `MFA_SETUP` challenge that lists the MFA
/// methods the user can set up to satisfy the pool's MFA requirement.
const MFAS_CAN_SETUP: &str = "MFAS_CAN_SETUP";

/// The `ChallengeResponses` key prefix an attribute value is submitted under:
/// `"userAttributes.[attribute_name]": "[attribute_value]"` (this service's model),
/// which is how `requiredAttributes` names the attributes it asks for — and how
/// `AdminRespondToAuthChallenge` reads them back off a response, so the asking and
/// the answering side cannot disagree.
pub const USER_ATTRIBUTES_PREFIX: &str = "userAttributes.";

/// The attribute every user pool mints for its users and no caller sets: "the user's
/// unique identifier" the pool assigns (`sub` in this service's standard schema).
pub const SUB_ATTRIBUTE: &str = "sub";

/// The scope an access token has to carry to authorize the user self-service
/// operations: each one is authorized "with a signed-in user's access token. It
/// must include the scope `aws.cognito.signin.user.admin`" (`GetUser`,
/// `UpdateUserAttributes` and the rest of the self-service surface in this
/// service's model), and `CreateUserPoolClient.AllowedOAuthScopes` names it as
/// "the `aws.cognito.signin.user.admin` scope" that "authorizes user
/// self-service operations".
pub const USER_ADMIN_SCOPE: &str = "aws.cognito.signin.user.admin";

/// The message a self-service operation owes a live access token whose scopes do
/// not authorize it ([`TokenRejection::MissingRequiredScopes`]). It names no
/// value from the request, so every such refusal carries this one sentence, and
/// every operation that checks a token's scopes reads it here so the two cannot
/// disagree.
pub const MISSING_REQUIRED_SCOPES_MESSAGE: &str = "Access Token does not have required scopes";

/// The message a self-service operation owes an access token this emulator issued
/// and has since REVOKED ([`TokenRejection::Revoked`]).
///
/// A sign-out "invalidates the identity, access, and refresh tokens that Amazon
/// Cognito issued to a user", after which "Amazon Cognito returns an `Access Token
/// has been revoked` error when your app attempts to authorize a user pools API
/// request with a revoked access token" (`GlobalSignOut` in this service's model).
/// It names no value from the request, so every such refusal carries this one
/// sentence, and it is told apart from the refusal of a value that resolves to no
/// access token at all: the caller holds a token this service did issue and has
/// since invalidated.
pub const ACCESS_TOKEN_REVOKED_MESSAGE: &str = "Access Token has been revoked";

/// The message a challenge response owes a `Session` that does not belong to the request
/// it arrived on: one started for another pool or app client, one naming another user than
/// the response does, or one whose challenge the stored user no longer owes.
///
/// It reports that the session resolves no challenge THIS request can answer and names
/// nothing of the request, so the refusal is the same whichever of those it was — a value
/// this emulator never handed out among them — and it is told apart from the replay of a
/// spent session ([`SESSION_ALREADY_USED_MESSAGE`]) and from a closed validity window
/// ([`SESSION_EXPIRED_MESSAGE`]).
pub const SESSION_NOT_FOR_THIS_REQUEST_MESSAGE: &str = "Invalid session for the user.";

/// The message a challenge response owes a `Session` this emulator handed out
/// and an earlier response ALREADY answered.
///
/// A `Session` is "the session identifier that maintains the state of
/// authentication requests and challenge responses" (`RespondToAuthChallenge` in
/// this service's model): answering its challenge is what spends it, so the
/// state it maintained is gone and the same session cannot carry a second
/// sign-in. The refusal says exactly that, which is a different answer from the
/// one a session that resolves no outstanding challenge gets
/// ([`SESSION_NOT_FOR_THIS_REQUEST_MESSAGE`]) and from the one a session whose
/// validity window closed gets ([`SESSION_EXPIRED_MESSAGE`]). It names no value
/// from the request, so every such refusal carries this one sentence.
pub const SESSION_ALREADY_USED_MESSAGE: &str =
    "Invalid session for the user, session can only be used once.";

/// The message a challenge response owes a `Session` whose validity window has
/// closed. `CreateUserPoolClient.AuthSessionValidity` is "the duration, in
/// minutes, of that session token" (this service's model), so a response past it
/// resumes a challenge that is no longer outstanding.
pub const SESSION_EXPIRED_MESSAGE: &str = "Invalid session for the user, session is expired.";

/// The message a challenge response owes a `ChallengeName` that is not the
/// challenge its own stored session is waiting on.
///
/// `ChallengeName` is "the name of the challenge that you are responding to"
/// (`smithy_query(query="operation",operation="AdminRespondToAuthChallenge")`),
/// so a response that names another challenge answers none: the state it carries
/// — the session beside the challenge it claims to answer — is not the state the
/// server is holding, which is what `CodeMismatchException` reports ("thrown if
/// the provided code doesn't match what the server was expecting"). The sentence
/// names neither the challenge the response gave nor the one the session awaits,
/// so every such refusal carries this one sentence.
pub const INVALID_CHALLENGE_STATE_MESSAGE: &str = "Invalid code or auth state for the user.";

/// `UserPoolTierType` defaults to `ESSENTIALS` (`CreateUserPool.UserPoolTier`).
const DEFAULT_USER_POOL_TIER: &str = ESSENTIALS_USER_POOL_TIER;

/// The feature plan threat-protection (`userAuthEvents`) activity logs need:
/// `LogConfigurationType.EventSource` is "info-level logs about threat-protection
/// user activity in user pools with the Plus feature plan" (this service's
/// model); one of the modelled
/// [`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`].
pub const PLUS_USER_POOL_TIER: &str = "PLUS";

/// `FeatureType`'s value for a setting that is switched on, which
/// `RefreshTokenRotationType.Feature` carries for an app client that rotates its
/// refresh tokens; its only other value is the one for a setting that is not
/// (`crate::ops::types::FEATURE_TYPE_VALUES`).
const FEATURE_ENABLED: &str = "ENABLED";

/// `UserPoolMfaType` is `OFF` for a pool that configures no MFA
/// (`MfaConfiguration`: "Sets multi-factor authentication (MFA) to be on, off, or
/// optional" in this service's model). It is also the mode in which no MFA factor
/// has to be deliverable, which a handler that configures factors asks about.
pub const MFA_OFF: &str = "OFF";

/// `UserPoolMfaType`'s value for a pool in which "MFA tokens are required for all
/// user sign-ins" (`MfaConfiguration` in this service's model), which is what
/// makes a second factor a precondition of completing a sign-in
/// ([`UserPool::requires_mfa`]).
pub const MFA_REQUIRED: &str = "ON";

/// Deletion protection is opt-in: "When active, `DeletionProtection` prevents
/// accidental deletion of your user pool" (this service's model), so a pool that
/// asks for none can be deleted.
const DEFAULT_DELETION_PROTECTION: &str = "INACTIVE";

/// `PreventUserExistenceErrors` "Defaults to `LEGACY`"
/// (`CreateUserPoolClient.PreventUserExistenceErrors` in this service's model),
/// which is the mode that answers `UserNotFoundException` for an absent user.
const DEFAULT_PREVENT_USER_EXISTENCE_ERRORS: &str = "LEGACY";

/// The message a `UserNotFoundException` carries for an operation that resolves
/// the user against a pool it names, or against the access token it is given.
///
/// `UserNotFoundException` is "the message returned when a user isn't found"
/// (this service's model) and the answer is the same sentence whatever was asked
/// for: it names neither the username nor the pool, so the error reports that
/// the lookup found nothing without telling the caller which identity it looked
/// for. A message built from the requested username would answer a different
/// sentence per username, which is what makes the refusal an existence oracle.
///
/// The public sign-up and password-recovery operations resolve the user from a
/// different pair and answer their own sentence —
/// [`USERNAME_CLIENT_NOT_FOUND_MESSAGE`].
pub const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// The message a `UserNotFoundException` carries for an operation that resolves
/// the user from the `Username` and the `ClientId` of the request TOGETHER.
///
/// The public sign-up and password-recovery operations — the ones whose input
/// models a required `Username` beside a required `ClientId` and no
/// `UserPoolId`, so the app client is what names the pool to look in — report an
/// unresolved user as the combination of the two not being found, rather than as
/// a statement about the username alone. Like [`USER_NOT_FOUND_MESSAGE`] it is
/// one fixed sentence that quotes neither the username nor the client id.
pub const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// The message a device operation's `ResourceNotFoundException` carries for a
/// `DeviceKey` the signed-in user holds no confirmed device for. Like
/// [`USER_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so the
/// refusal is the same sentence for every key.
pub const DEVICE_NOT_FOUND_MESSAGE: &str = "Device does not exist.";

/// The message `ConfirmDevice`'s `InvalidParameterException` carries for a
/// `DeviceKey` the signed-in user holds no unconfirmed device for. Like
/// [`DEVICE_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so a key
/// another user or pool holds is reported exactly as one that was never minted.
pub const INVALID_DEVICE_KEY_MESSAGE: &str = "Invalid device key given.";

/// The message the refusal of a sign-in by a user Amazon Cognito is waiting on a
/// new password from carries ([`UserStatus::ResetRequired`]).
///
/// One fixed sentence, naming no username: every operation that reports that
/// state — the sign-in operations, the refresh-token exchange and the
/// self-service operations a signed-in user's access token authorizes — reports
/// it the same way.
pub const PASSWORD_RESET_REQUIRED_MESSAGE: &str = "Password reset required for the user";

/// The message the refusal of a password reset for a user whose registration is
/// not complete carries: the account has no password for a reset to apply to,
/// so the state the user is in is what the refusal reports, naming no username.
pub const PASSWORD_NOT_RESETTABLE_MESSAGE: &str =
    "User password cannot be reset in the current state.";

/// The message a passkey operation's `ResourceNotFoundException` carries for a
/// `CredentialId` the signed-in user holds no registered passkey for. Like
/// [`DEVICE_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so the
/// refusal is the same sentence for every credential id — and so a credential
/// REGISTERED TO ANOTHER user is reported exactly as one that was never
/// registered at all, which is what keeps the refusal from telling a caller
/// whether somebody else holds that credential.
pub const WEB_AUTHN_CREDENTIAL_NOT_FOUND_MESSAGE: &str = "The credential does not exist";

/// The message a `WebAuthnNotEnabledException` carries on a pool that does not
/// admit the passkey factor. One fixed sentence, naming neither the pool nor the
/// operation that asked: every operation that gates on the feature — the
/// registration operations — reports it the same way.
pub const WEB_AUTHN_NOT_ENABLED_MESSAGE: &str = "WebAuthn not enabled for this pool.";

/// The message a device operation's `InvalidUserPoolConfigurationException`
/// carries on a pool that stores no `DeviceConfiguration`: "When you provide a
/// value for any property of `DeviceConfiguration`, you activate the device
/// remembering for the user pool" (this service's model), so a pool that
/// provided none remembers no device and has no device surface to serve.
pub const DEVICE_REMEMBERING_NOT_CONFIGURED_MESSAGE: &str =
    "Device tracking not currently enabled for this pool.";

/// The message a `ResourceNotFoundException` carries when the group an operation
/// names is not one the named pool holds.
///
/// Amazon Cognito answers the same sentence for every group name: the group
/// lookup reports that nothing was found and names no group, so the refusal
/// cannot be read as a statement about one particular group name. Every
/// operation that resolves a `GroupName` against a pool reports it the same way,
/// exactly as [`USER_NOT_FOUND_MESSAGE`] is reported for a username.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// The message a `ResourceNotFoundException` carries when the identity provider
/// an operation RESOLVES is not one the named pool holds.
///
/// Amazon Cognito reports an unresolved user pool IdP by naming what the request
/// asked for — `asked_for` is the `ProviderName` an operation looked the IdP up
/// by, or the `IdpIdentifier` when it looked it up by identifier — beside the
/// pool it was looked up in, which this sentence calls the tenant. Every operation
/// that RESOLVES an IdP against a pool reports it the same way, so a read and a
/// delete of the same absent provider answer the same sentence.
///
/// The WRITE path words the same class differently and has its own sentence
/// ([`identity_provider_not_found_in_pool_message`]).
pub fn identity_provider_not_found_message(asked_for: &str, user_pool_id: &str) -> String {
    format!("Identity provider {asked_for} for tenantId {user_pool_id} does not exist.")
}

/// The message a `ResourceNotFoundException` carries when the identity provider an UPDATE
/// names is not one the named pool holds.
///
/// The same class as [`identity_provider_not_found_message`] and the same two variable
/// parts — the provider the request named and the pool it was named in — worded as the
/// update path words it: a recorded `UpdateIdentityProvider` refusal of a provider its pool
/// does not hold names the pool as the user pool it is rather than as a tenant, which is the
/// sentence that path answers whatever the provider and the pool are.
pub fn identity_provider_not_found_in_pool_message(asked_for: &str, user_pool_id: &str) -> String {
    format!("Identity provider {asked_for} in User Pool {user_pool_id} does not exist.")
}

/// The `ProviderName` that names a user pool's OWN users rather than one of its
/// third-party identity providers: "To deactivate a local user, set
/// `ProviderName` to `Cognito`" (`AdminDisableProviderForUser`), and "the
/// `ProviderName` should be set to `Cognito` for users in Cognito user pools"
/// (`AdminLinkProviderForUser.DestinationUser`), both in this service's model.
pub const NATIVE_PROVIDER_NAME: &str = "Cognito";

/// The message an `InvalidParameterException` carries when a request names a
/// provider identity the pool holds no link for.
///
/// Deactivating a provider for a user acts on a LINK — the external identity's
/// link to a local profile, or, for the native `Cognito` provider, the links a
/// local profile is the destination of — so an account that is linked to
/// nothing has no link to act on, and the refusal is the same sentence whatever
/// the provider or the identity named.
pub const ACCOUNT_NOT_LINKED_MESSAGE: &str =
    "Account isn't linked to anyone else, verify that the account has been linked";

/// How many federated identities may link to one local user: "The maximum
/// number of federated identities linked to a user is five."
/// (`AdminLinkProviderForUser` in this service's model).
pub const MAX_LINKED_PROVIDER_IDENTITIES: usize = 5;

/// The message `AdminLinkProviderForUser`'s `InvalidParameterException` carries
/// for a `SourceUser` that already links to the very `DestinationUser` the
/// request names.
///
/// A federated identity links to ONE local profile — the operation "links an
/// existing user account ... to an identity from an external IdP" and the link
/// is what lets that identity sign in as the profile — so a call that asks for
/// the link the pool already holds is told so: the caller deactivates that link
/// before linking the identity again. One fixed sentence that names the two
/// MEMBERS of the request rather than the provider, the identity or the profile
/// they carried, so the refusal echoes nothing the caller submitted. A request
/// that points the identity at ANOTHER profile is refused for what that
/// identity resolves to instead: [`INVALID_SOURCE_USER_MESSAGE`] for a profile
/// that signs in with a username and a password, and
/// [`SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE`] for one that does not.
pub const SOURCE_USER_ALREADY_LINKED_MESSAGE: &str =
    "SourceUser is already linked to DestinationUser";

/// The message `AdminLinkProviderForUser`'s `AliasExistsException` carries for a
/// `SourceUser` identity another local profile already answers for.
///
/// A federated identity links to ONE local profile, so an identity that already
/// links to another profile is one this pool has given out: handing it to a
/// second profile is the alias collision `AliasExistsException` reports, "an
/// ... alias for a different user profile" (that error in this service's
/// model). It is the refusal left when the identity resolves to a profile that
/// is NOT a user pool user with a username and a password — one that holds no
/// password — because such a profile is not what
/// [`INVALID_SOURCE_USER_MESSAGE`] reports. One fixed sentence that names the
/// MEMBER whose value was already linked rather than the provider, the identity
/// or either profile, so the refusal echoes nothing the caller submitted.
pub const SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE: &str =
    "SourceUser with the specified attribute value is already linked to another user";

/// The message `AdminLinkProviderForUser`'s `InvalidParameterException` carries
/// for a `SourceUser` that resolves to one of the pool's OWN local profiles
/// rather than to an external identity.
///
/// `SourceUser` is "an external IdP account for a user who doesn't exist yet in
/// the user pool. This user must be a federated user (for example, a SAML or
/// Facebook user), not another native user"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`),
/// so an identifier that resolves to a user pool user who signs in with a
/// username and password identifies no external account to link — whether it
/// names the native provider outright, or names a federated identity a live
/// link has already made such a profile's. An identity linked to a profile that
/// holds no password resolves to no such user, and carries
/// [`SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE`] instead. One fixed sentence,
/// which names neither the identifier nor the user it resolved to.
pub const INVALID_SOURCE_USER_MESSAGE: &str =
    "Invalid SourceUser: Cognito users with a username/password may not be passed in as a \
     SourceUser, only as a DestinationUser";

/// The message a `TermsExistsException` carries for a `TermsName` the app client
/// already holds terms documents under.
///
/// Terms document names "must be unique to the app client" (this service's
/// model), so the refusal names the name that collided and the app client it
/// collided within — the same sentence wherever that uniqueness is enforced, on
/// a create of a second document of that name and on a rename onto it alike.
pub fn terms_exists_message(terms_name: &str, client_id: &str) -> String {
    format!("Terms with name {terms_name} already exists for Client {client_id}.")
}

/// The message a `ResourceNotFoundException` carries when a request resolves no
/// set of terms documents — by its `TermsId` in the named pool, or by the app
/// client and `TermsName` that name it together.
///
/// Amazon Cognito answers the same sentence however the documents were looked
/// up: it reports that the lookup found no terms documents and quotes neither
/// the id, the name, the app client nor the pool, exactly as
/// [`USER_NOT_FOUND_MESSAGE`] and [`GROUP_NOT_FOUND_MESSAGE`] report their own
/// unresolved lookups. A message built from the request would word the refusal
/// differently per id or per name, so anything that groups refusals by message
/// would see one error per request rather than one not-found class — and the
/// lookup by app client would put a `@sensitive` `ClientId` in it.
///
/// The app-client lookup THIS pool fails to resolve at all is a different class
/// and keeps its own wording (the `describe_terms_by_client` handler): there the
/// app client, not the terms documents, is what does not exist.
pub const TERMS_NOT_FOUND_MESSAGE: &str = "Terms does not exist.";

// ---------------------------------------------------------------------------
// Password policy rejection wording
//
// A rejection says that the password missed the pool's policy and which
// requirement it missed, in a fixed clause per requirement: the clause is the
// same whatever the pool configured, so the refusal does not report the pool's
// policy values to a caller that has not read them.
// ---------------------------------------------------------------------------

/// Which wording a password-policy rejection carries, ahead of the clause for
/// the requirement the password missed.
///
/// Amazon Cognito does not word this refusal identically in every API that
/// judges a password: the administrative password set, the administrative create
/// and the paths where the user supplies its own password each answer their own
/// sentence. The clause for the requirement is the same in all of them, so only
/// the prefix is chosen here and every operation names the wording ITS OWN API
/// answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordPolicyWording {
    /// The wording `AdminSetUserPassword` answers.
    AdminSetsPassword,
    /// The wording `AdminCreateUser` answers for a `TemporaryPassword` the
    /// policy refuses.
    AdminCreatesUser,
    /// The wording the paths where the user itself supplies the password answer
    /// — a sign-up, a forgotten-password confirmation, a self-service password
    /// change.
    UserSuppliesPassword,
    /// The wording the operations that answer an outstanding authentication
    /// challenge answer: a `NEW_PASSWORD_REQUIRED` response carries the chosen
    /// password in `ChallengeResponses`, and a policy the password misses is
    /// reported there in the same sentence the password-setting APIs use, not in
    /// the self-service one.
    AnswersChallenge,
}

impl PasswordPolicyWording {
    /// The prefix this wording carries ahead of the requirement's clause.
    fn prefix(self) -> &'static str {
        match self {
            Self::AdminSetsPassword | Self::AnswersChallenge => {
                "Password does not conform to policy: "
            }
            Self::AdminCreatesUser => "Password did not conform with password policy: ",
            Self::UserSuppliesPassword => "Password did not conform with policy: ",
        }
    }
}

/// The clause for a password shorter than `PasswordPolicyType.MinimumLength`.
const PASSWORD_TOO_SHORT: &str = "Password not long enough";

/// The clause for `PasswordPolicyType.RequireUppercase`.
const PASSWORD_NEEDS_UPPERCASE: &str = "Password must have uppercase characters";

/// The clause for `PasswordPolicyType.RequireLowercase`.
const PASSWORD_NEEDS_LOWERCASE: &str = "Password must have lowercase characters";

/// The clause for `PasswordPolicyType.RequireNumbers`.
const PASSWORD_NEEDS_NUMBERS: &str = "Password must have numeric characters";

/// The clause for `PasswordPolicyType.RequireSymbols`.
const PASSWORD_NEEDS_SYMBOLS: &str = "Password must have symbol characters";

/// The message a password-policy rejection worded as `wording` asks for
/// carries for the requirement `requirement` names.
fn password_policy_message(wording: PasswordPolicyWording, requirement: &str) -> String {
    format!("{}{requirement}", wording.prefix())
}

/// The message the refusal of a password the pool's history policy restricts
/// carries: a `PasswordHistoryPolicyViolationException` is "the message returned
/// when a user's new password matches a previous password and doesn't comply with
/// the password-history policy" (this service's model), so the refusal reports
/// that the submitted password is one the user has already had and names neither
/// the password nor how many the policy keeps. It carries no policy prefix: this
/// is not a `PasswordPolicyType` requirement the password missed
/// ([`PasswordPolicyWording`]), it is the one password-history refusal.
pub const PASSWORD_REUSED_MESSAGE: &str = "Password has previously been used";

// ---------------------------------------------------------------------------
// Threat protection (risk configuration)
// ---------------------------------------------------------------------------

/// The `AdvancedSecurityMode` that leaves threat protection switched off: one of
/// the modelled [`crate::ops::types::ADVANCED_SECURITY_MODE_TYPE_VALUES`], and
/// the value a pool that carries no `UserPoolAddOns` behaves as.
pub const ADVANCED_SECURITY_MODE_OFF: &str = "OFF";

/// The `AdvancedSecurityMode` under which threat protection ACTS on traffic: "to
/// configure automatic security responses to potentially unwanted traffic to
/// your user pool, set to `ENFORCED`" (`UserPoolAddOnsType` in this service's
/// model), as against `AUDIT`, which logs "user security information but take[s]
/// no action". One of the modelled
/// [`crate::ops::types::ADVANCED_SECURITY_MODE_TYPE_VALUES`].
pub const ADVANCED_SECURITY_MODE_ENFORCED: &str = "ENFORCED";

/// The `AdvancedSecurityEnabledModeType` that logs "user security information
/// but take[s] no action" (`UserPoolAddOnsType` in this service's model), and the
/// mode threat protection runs the additional authentication types in where the
/// request configures none for them: that enum admits no "off" value
/// ([`crate::ops::types::ADVANCED_SECURITY_ENABLED_MODE_TYPE_VALUES`]), so a pool
/// whose add-ons are configured reports the auditing mode for them.
pub const ADVANCED_SECURITY_MODE_AUDIT: &str = "AUDIT";

/// The message a `UserPoolAddOnNotEnabledException` carries.
///
/// `UserPoolAddOnNotEnabledException` is thrown "when user pool add-ons aren't
/// enabled" (this service's model), which is the state of a pool that carries no
/// threat-protection surface: "to change threat protection to audit-only or off,
/// update the value of `UserPoolAddOns` in an `UpdateUserPool` request"
/// (`SetRiskConfiguration`). The refusal reports that state of the pool, names the
/// member to change to leave it, and names nothing the request asked for, so it is
/// the same sentence for every pool and app client.
pub const USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE: &str = "This is an add on feature. Please \
     update AdvancedSecurityMode for your user pool to access this API.";

// ---------------------------------------------------------------------------
// User pool domains
// ---------------------------------------------------------------------------

/// The managed login branding version for hosted UI (classic): "A value of `1`
/// indicates hosted UI (classic) and a version of `2` indicates managed login"
/// (`CreateUserPoolDomain.ManagedLoginVersion` in this service's model).
pub const HOSTED_UI_CLASSIC_BRANDING: i32 = 1;

/// The managed login branding version, which the same member documents as
/// needing "your user pool be configured for any feature plan other than
/// `Lite`".
pub const MANAGED_LOGIN_BRANDING: i32 = 2;

/// The only branding versions `ManagedLoginVersion` names, so a request asking
/// for any other value asks for a version that does not exist.
pub const MANAGED_LOGIN_VERSIONS: [i32; 2] = [HOSTED_UI_CLASSIC_BRANDING, MANAGED_LOGIN_BRANDING];

/// The branding version a domain created without one carries: the model names
/// only the two above, and hosted UI (classic) is the one that needs no feature
/// plan.
pub const DEFAULT_MANAGED_LOGIN_VERSION: i32 = HOSTED_UI_CLASSIC_BRANDING;

/// The feature plan managed login is NOT included in
/// (`CreateUserPoolDomain.ManagedLoginVersion`); one of the modelled
/// `UserPoolTierType` values.
pub const LITE_USER_POOL_TIER: &str = "LITE";

/// The feature plan email message MFA needs: `EmailMfaConfigType` is "To
/// activate this setting, your user pool must be in the Essentials tier or
/// higher" (this service's model); one of the modelled
/// [`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`].
pub const ESSENTIALS_USER_POOL_TIER: &str = "ESSENTIALS";

/// The modelled feature plans ([`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`])
/// in INCLUSION order, cheapest first: a plan includes every feature the plans
/// before it include, which is what "the Essentials tier or higher"
/// (`EmailMfaConfigType`) and "the Plus feature plan"
/// (`LogConfigurationType.EventSource`) each name a floor of.
const USER_POOL_TIER_ORDER: [&str; 3] = [
    LITE_USER_POOL_TIER,
    ESSENTIALS_USER_POOL_TIER,
    PLUS_USER_POOL_TIER,
];

/// Whether a pool on `tier` includes a feature whose floor is the plan
/// `minimum`: the plans are ordered by inclusion, so every plan from `minimum`
/// upwards includes it and every plan below it does not. An unmodelled `tier`
/// includes nothing beyond the cheapest plan, so a gated feature is refused
/// rather than granted.
#[must_use]
pub fn user_pool_tier_includes(tier: &str, minimum: &str) -> bool {
    let rank = |plan: &str| {
        USER_POOL_TIER_ORDER
            .iter()
            .position(|known| *known == plan)
            .unwrap_or(0)
    };
    rank(tier) >= rank(minimum)
}

/// The refusal a feature whose floor is the plan `minimum` owes a pool on
/// `tier`, or `None` when that pool's plan includes the feature.
///
/// Every operation that configures a plan-gated feature asks this one question,
/// so the boundary and the sentence it is reported with cannot disagree between
/// them. `feature` is the feature's display name, as
/// [`feature_unavailable_in_tier_message`] renders it.
#[must_use]
pub fn feature_plan_refusal(tier: &str, minimum: &str, feature: &str) -> Option<String> {
    (!user_pool_tier_includes(tier, minimum))
        .then(|| feature_unavailable_in_tier_message(tier, feature))
}

/// The message a `FeatureUnavailableInTierException` carries: a tier refusal
/// reports the pool's configured feature plan and the feature that plan does not
/// include, in one template, so a caller can parse the feature out of it and
/// tell its operator what to upgrade.
///
/// `tier` is the pool's stored `UserPoolTier` (a `UserPoolTierType` value) and
/// `feature` names the feature the request asked for.
pub fn feature_unavailable_in_tier_message(tier: &str, feature: &str) -> String {
    format!("The following feature is not available for the {tier} pricing tier configured: {feature}")
}

/// The message a `FeatureUnavailableInTierException` carries for a pool whose
/// OWN configuration asks for a feature its feature plan excludes: the request
/// sets the plan and the feature TOGETHER, so the refusal reports the features
/// that have to be off for the plan it configured rather than a feature that is
/// unavailable to configure.
///
/// `tier` is the plan the pool is left on and `feature` the feature's display
/// name, as [`feature_unavailable_in_tier_message`] renders it for the
/// operations that configure a feature on a pool whose plan is already set.
pub fn features_need_disabling_message(tier: &str, feature: &str) -> String {
    format!(
        "The following features need to be disabled for the {tier} pricing tier configured: \
         {feature}"
    )
}

/// The feature a tier refusal names for user pool email message MFA: the
/// `EmailMfaConfiguration` member of `SetUserPoolMfaConfig` configures "user
/// pool email message MFA and sign-in with one-time passwords (OTPs)"
/// (`EmailMfaConfigType`), reported through
/// [`feature_unavailable_in_tier_message`] the way every other tier refusal is.
pub const EMAIL_MFA_FEATURE: &str = "Email MFA";

/// The feature name a tier refusal gives for a `ManagedLoginVersion` of
/// `version`: the branding version is part of the feature asked for, since only
/// managed login (version 2) needs a feature plan other than
/// [`LITE_USER_POOL_TIER`].
pub fn managed_login_feature_name(version: i32) -> String {
    format!("Managed Login Version {version}")
}

/// The feature a tier refusal names for choice-based (selection-based) sign-in:
/// the pool's `SignInPolicy` and the app client's `ALLOW_USER_AUTH` flow are the
/// two halves of ONE feature, and each documents the same floor — "To activate
/// this setting, your user pool must be in the Essentials tier or higher"
/// (`SignInPolicyType`, and `ALLOW_USER_AUTH` under
/// `CreateUserPoolClient.ExplicitAuthFlows`) — so both halves are refused under
/// this one name.
pub const CHOICE_BASED_SIGN_IN_FEATURE: &str = "Choice Based Authentication";

/// The feature a tier refusal names for threat protection: `UserPoolAddOnsType`
/// carries "settings for activation of threat protection" and "to activate this
/// setting, your user pool must be on the Plus tier" (this service's model).
pub const THREAT_PROTECTION_FEATURE: &str = "Threat Protection";

/// Whether `policies` configures choice-based sign-in — a first authentication
/// factor BEYOND username and password.
///
/// `SignInPolicyType.AllowedFirstAuthFactors` is "the sign-in methods that a
/// user pool supports as the first factor. You can permit users to start
/// authentication with a standard username and password, or with other one-time
/// password and hardware factors" (this service's model), and a pool that
/// configures no policy admits [`PASSWORD_AUTH_FACTOR`] alone
/// ([`effective_policies`]) — so a policy naming only that factor asks for
/// nothing beyond what every plan serves, while any other factor is the gated
/// feature. Both the create and the update path ask this one function, so the
/// two cannot disagree about which configuration needs which plan.
#[must_use]
pub fn configures_choice_based_sign_in(policies: Option<&UserPoolPolicyType>) -> bool {
    policies
        .and_then(|policies| policies.sign_in_policy.as_ref())
        .and_then(|policy| policy.allowed_first_auth_factors.as_ref())
        .is_some_and(|factors| factors.iter().any(|factor| factor != PASSWORD_AUTH_FACTOR))
}

/// Whether `flows` configures choice-based sign-in on an app client: the
/// `ALLOW_USER_AUTH` flow "covers username-password, secure remote password
/// (SRP), passwordless, and passkey authentication" and names the same floor
/// ([`CHOICE_BASED_SIGN_IN_FEATURE`]). Asked by the create and the update path
/// alike.
#[must_use]
pub fn configures_user_auth_flow(flows: &[String]) -> bool {
    flows
        .iter()
        .any(|flow| USER_AUTH_FLOWS.contains(&flow.as_str()))
}

/// The refusal an APP CLIENT configuration owes the feature plan of the pool it
/// belongs to, or `None` when that plan includes every feature the
/// configuration asks for.
///
/// An app-client member whose own documentation names a feature plan is gated on
/// it: `ALLOW_USER_AUTH` documents "the Essentials tier or higher"
/// ([`CHOICE_BASED_SIGN_IN_FEATURE`]), which is the only such member of an app
/// client's configuration (this service's model). The plan is the POOL's stored
/// one, which the client request does not set, so the refusal reports the
/// feature as unavailable ([`feature_unavailable_in_tier_message`]) rather than
/// as one to disable. Both the create and the update path ask this one function.
#[must_use]
pub fn client_feature_plan_refusal(tier: &str, explicit_auth_flows: Option<&[String]>) -> Option<String> {
    if explicit_auth_flows.is_some_and(configures_user_auth_flow) {
        if let Some(message) = feature_plan_refusal(
            tier,
            ESSENTIALS_USER_POOL_TIER,
            CHOICE_BASED_SIGN_IN_FEATURE,
        ) {
            return Some(message);
        }
    }
    None
}

/// The message an app client that asks to propagate additional user context data
/// without holding a client secret is refused with: "You can only activate
/// `EnablePropagateAdditionalUserContextData` in an app client that has a client
/// secret" (`CreateUserPoolClient` and `UpdateUserPoolClient` in this service's
/// model). It names no value from the request, so every such refusal carries
/// this one sentence, and both paths read it here.
pub const CLIENT_SECRET_REQUIRED_FOR_CONTEXT_DATA_MESSAGE: &str =
    "Client Secret is required to set EnablePropagateAdditionalUserContextData as true";

/// The refusal a USER POOL configuration owes the feature plan it is on, or
/// `None` when that plan includes every feature the configuration asks for.
///
/// A member whose own documentation names a feature plan is gated on it, and
/// each one states its own floor: `SignInPolicyType` ("the Essentials tier or
/// higher") and `UserPoolAddOnsType` ("the Plus tier") are the two such members
/// of a pool's configuration (this service's model). What each member asks FOR
/// is what the gate reads, not whether the member is present: `UserPoolAddOns`
/// asks for the gated feature in the mode that acts on traffic
/// ([`ADVANCED_SECURITY_MODE_ENFORCED`]) and for neither logging-only threat
/// protection ([`ADVANCED_SECURITY_MODE_OFF`] and `AUDIT`, which "take[s] no
/// action") nor a `SignInPolicy` naming only [`PASSWORD_AUTH_FACTOR`], which
/// configures no further factor.
///
/// The plan the pool is CONFIGURED on is what decides: an update is judged
/// against the plan it leaves the pool on, and a create against the plan its own
/// request names — a create that names none configures no plan and is put on one
/// that includes its features instead
/// ([`effective_user_pool_tier_for_configuration`]). Both paths ask this one
/// function, so neither can gate a feature the other serves.
#[must_use]
pub fn pool_feature_plan_refusal(
    tier: &str,
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> Option<String> {
    pool_configuration_feature_floors(policies, add_ons)
        .into_iter()
        .find(|(floor, _)| !user_pool_tier_includes(tier, floor))
        .map(|(_, feature)| features_need_disabling_message(tier, feature))
}

/// Every plan-gated feature a pool configuration ASKS FOR, each with the plan
/// floor its own documentation names and the name a refusal reports it under.
///
/// The two gated members of a pool's configuration are `SignInPolicyType` ("the
/// Essentials tier or higher") and `UserPoolAddOnsType` ("the Plus tier") (this
/// service's model), and what each asks FOR is what counts — not whether the
/// member is present. Both the gate ([`pool_feature_plan_refusal`]) and the plan
/// a create that names none is put on
/// ([`effective_user_pool_tier_for_configuration`]) read this one table, so the
/// plan a pool is given can never exclude a feature the same configuration is
/// refused for.
#[must_use]
fn pool_configuration_feature_floors(
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> Vec<(&'static str, &'static str)> {
    let mut floors = Vec::new();
    if configures_choice_based_sign_in(policies) {
        floors.push((ESSENTIALS_USER_POOL_TIER, CHOICE_BASED_SIGN_IN_FEATURE));
    }
    if add_ons.is_some_and(|add_ons| add_ons.advanced_security_mode == ADVANCED_SECURITY_MODE_ENFORCED)
    {
        floors.push((PLUS_USER_POOL_TIER, THREAT_PROTECTION_FEATURE));
    }
    floors
}

/// The feature plan a pool whose configuration is `policies` and `add_ons` is
/// created on when the request names `requested`.
///
/// A request that names a plan CONFIGURES it, so that plan is the pool's and
/// [`pool_feature_plan_refusal`] judges the configuration against it. A request
/// that names none configures no plan: "`UserPoolTier` ... determines the
/// eligibility of the user pool for features like managed login, access-token
/// customization, and threat protection" (`CreateUserPool.UserPoolTier` in this
/// service's model), so the pool is created on the cheapest plan that includes
/// every feature its configuration asks for — the documented
/// [`DEFAULT_USER_POOL_TIER`] when it asks for none, and the highest floor among
/// the features it does ask for otherwise.
#[must_use]
pub fn effective_user_pool_tier_for_configuration(
    requested: Option<String>,
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> String {
    if let Some(requested) = requested {
        return requested;
    }
    pool_configuration_feature_floors(policies, add_ons)
        .into_iter()
        .fold(DEFAULT_USER_POOL_TIER, |plan, (floor, _)| {
            if user_pool_tier_includes(plan, floor) {
                plan
            } else {
                floor
            }
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// App client OAuth configuration
// ---------------------------------------------------------------------------

/// The `OAuthFlowType` value of the machine-to-machine grant: "`client_credentials`
/// Issue the access token from the /oauth2/token endpoint directly to a non-person
/// user" (`CreateUserPoolClient.AllowedOAuthFlows` in this service's model).
pub const CLIENT_CREDENTIALS_OAUTH_FLOW: &str = "client_credentials";

/// The message an app client whose `AllowedOAuthFlows` combine the
/// machine-to-machine grant with a user-facing one is refused with
/// (`InvalidOAuthFlowException`). It names no value from the request, so every
/// such refusal carries this one sentence.
pub const CLIENT_CREDENTIALS_FLOW_EXCLUSIVE_MESSAGE: &str =
    "client_credentials flow can not be selected along with code flow or implicit flow.";

/// The refusal an `AllowedOAuthFlows` list owes when it asks for a combination
/// no app client can hold, and `None` when it describes one the service accepts.
///
/// "To create an app client that generates client credentials grants, you must
/// add `client_credentials` as the only allowed OAuth flow"
/// (`CreateUserPoolClient.AllowedOAuthFlows` in this service's model): the
/// machine-to-machine grant is issued to a non-person user, while `code` and
/// `implicit` are grants a signed-in human is redirected through, so a list
/// naming `client_credentials` beside either of them describes an app client
/// that cannot exist. A list naming it alone — or not at all, or repeating it —
/// is accepted: the rule is about which grants are configured, not how many
/// times each is sent. Both the create and the update path ask this one
/// function, so the two cannot disagree about what an app client may hold.
#[must_use]
pub fn invalid_oauth_flow_violation(flows: &[String]) -> Option<&'static str> {
    let names_client_credentials = flows
        .iter()
        .any(|flow| flow == CLIENT_CREDENTIALS_OAUTH_FLOW);
    let names_a_user_grant = flows
        .iter()
        .any(|flow| flow != CLIENT_CREDENTIALS_OAUTH_FLOW);
    (names_client_credentials && names_a_user_grant).then_some(CLIENT_CREDENTIALS_FLOW_EXCLUSIVE_MESSAGE)
}

/// The scopes every user pool authorizes without a resource server of its own:
/// "Scope values include `phone`, `email`, `openid`, and `profile`. The
/// `aws.cognito.signin.user.admin` scope authorizes user self-service
/// operations" (`CreateUserPoolClient.AllowedOAuthScopes` in this service's
/// model). Every other scope value is a custom one, which only a resource server
/// of the pool can define ([`UserPool::defines_scope`]).
pub const STANDARD_OAUTH_SCOPES: [&str; 5] = [
    "phone",
    "email",
    "openid",
    "profile",
    USER_ADMIN_SCOPE,
];

/// The message a `ScopeDoesNotExistException` carries for `scope`: the scope the
/// request asked for is the one value such a refusal reports, echoed only as far
/// as this service echoes any submitted value.
pub fn scope_does_not_exist_message(scope: &str) -> String {
    format!("Invalid scope requested: {}", echoed_value(scope))
}

/// The `DomainStatusType` a freshly created domain is in, until the clock makes
/// its activation due: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";

/// The `DomainStatusType` a domain reaches once [`DOMAIN_ACTIVATION_SPAN`] has
/// elapsed: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// The `DomainStatusType` a domain whose configuration has just changed is in,
/// until the clock makes it [`DOMAIN_STATUS_ACTIVE`] again: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_UPDATING: &str = "UPDATING";

/// How long a created domain stays `CREATING` before the clock owes it `ACTIVE`.
///
/// A chosen span, not a documented duration: `CreateUserPoolDomain` only says
/// "Your prefix domain might take up to one minute to take effect" and "Your
/// custom domain is online within five minutes", so a domain is NOT ready when
/// the call returns, and this emulator makes the transition due shortly after
/// instead of waiting minutes. Never zero, so a caller reading the domain right
/// after creating it still sees `CREATING`.
pub const DOMAIN_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The sentence a `ResourceNotFoundException` carries for a `UserPoolId` no
/// stored pool answers to, in the operations that word that lookup `UserPool
/// with ID: <UserPoolId>does not exist.` — `SetLogDeliveryConfiguration`,
/// `CreateUserPoolDomain` and `CreateUserPoolReplica`.
///
/// The fixed text is `UserPool with ID: ` and `does not exist.`: Amazon Cognito
/// joins the two halves with no separator, so the absent space before `does` is
/// the service's own wording and not a typo to correct. The one variable part is
/// the `UserPoolId` the REQUEST carried. A not-found sentence belongs to the
/// operation that answers it rather than to the resource kind, which is why the
/// operations that word the same lookup `User pool <UserPoolId> does not exist.`
/// keep their own sentence instead of calling this.
pub fn user_pool_with_id_not_found_message(user_pool_id: &str) -> String {
    format!("UserPool with ID: {user_pool_id}does not exist.")
}

/// The message an operation that resolves a domain by the PAIR it is addressed
/// by — a `UserPoolId` and a `Domain` — carries when that pair holds no domain.
///
/// The pair is resolved as a unit, so the refusal reports the pair and names
/// neither half: one fixed sentence for a pool that holds no domain of that
/// name, whichever of the two the caller got wrong.
pub const NO_SUCH_DOMAIN_MESSAGE: &str = "No such domain or user pool exists.";

/// The message an operation that resolves a domain by the PAIR `UserPoolId` +
/// `Domain` carries when the `Domain` half resolves to a domain of ANOTHER pool.
///
/// A domain name is unique across pools, so this pair does not name a resource
/// that is missing: it names one the pool it was asked for is not configured
/// with. The sentence is fixed text in full and names neither half of the pair,
/// so it discloses nothing about the pool that does hold the domain.
pub const DOMAIN_OF_OTHER_POOL_MESSAGE: &str = "The domain is not configured for this user pool.";

/// The message `CreateUserPoolDomain` carries when the `Domain` the request asks
/// for is one some pool already holds.
///
/// Fixed text in full, with no variable part: the refusal reports that the name
/// is taken and names neither the domain nor the pool holding it, so it
/// discloses nothing about a pool the caller may not own.
pub const DOMAIN_EXISTS_MESSAGE: &str = "Domain already exists.";

// ---------------------------------------------------------------------------
// User pool replicas
//
// A replica's `Status` is COMPUTED from what is stored plus the clock, so a
// read renders the status the replica is in at that instant and writes
// nothing. Each status below is one of the modelled
// [`crate::ops::types::REPLICA_STATUS_TYPE_VALUES`] and each role one of the
// modelled [`crate::ops::types::REPLICA_ROLE_TYPE_VALUES`].
// ---------------------------------------------------------------------------

/// "`CREATING`: The replica is being created" (`UserPoolReplicaType.Status`):
/// what a created secondary replica reports until the clock makes its
/// activation due.
pub const REPLICA_STATUS_CREATING: &str = "CREATING";

/// "`ACTIVE`: The replica is available for both end-user and administrator
/// operations" (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_ACTIVE: &str = "ACTIVE";

/// "`INACTIVE`: The replica has been created, but is not accepting requests for
/// end-users. Administrator configuration operations are supported"
/// (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_INACTIVE: &str = "INACTIVE";

/// "`DELETING`: The replica is being deleted" (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_DELETING: &str = "DELETING";

/// "`PRIMARY`: The primary replica supports all end user and administrator
/// operations" (`UserPoolReplicaType.Role`): the role the pool holds in the
/// Region it was created in.
pub const REPLICA_ROLE_PRIMARY: &str = "PRIMARY";

/// "`SECONDARY`: The secondary replica supports a limited set of end user and
/// administrator operations" (`UserPoolReplicaType.Role`): the role of every
/// replica a `CreateUserPoolReplica` call adds.
pub const REPLICA_ROLE_SECONDARY: &str = "SECONDARY";

/// How long a created secondary replica stays `CREATING` before the clock owes
/// it the status it settles into.
///
/// A chosen span, not a documented duration: the model gives a replica that
/// "is being created" its own status without saying how long creating takes, so
/// a replica is NOT ready when the call returns and this emulator makes the
/// transition due shortly after instead of waiting minutes. Never zero, so a
/// caller listing the replicas straight after creating one still sees
/// `CREATING`.
pub const REPLICA_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long a deleted replica stays [`REPLICA_STATUS_DELETING`] before the
/// clock owes its removal. A chosen span, on the same grounds as
/// [`REPLICA_ACTIVATION_SPAN`]: `DELETING` means "The replica is being
/// deleted", so the replica is still reported, as `DELETING`, right after the
/// call.
pub const REPLICA_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The feature a tier refusal names for a replica: replicating a user pool to
/// another Region needs a feature plan other than [`LITE_USER_POOL_TIER`], and
/// a refusal reports it through [`feature_unavailable_in_tier_message`] the way
/// every other tier refusal does.
pub const REPLICATION_FEATURE: &str = "Multi-Region Replication";

/// The Regions a `RegionName` may name: the emulator's SHARED Region catalog,
/// not a copy of it.
///
/// A `RegionNameType` is "the Region in which the replica user pool will be
/// created" (`CreateUserPoolReplica.RegionName`), and the model bounds only its
/// length (5..32), so the set of values that name a Region at all is not on the
/// wire. That set is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS
/// region the wire may carry, sorted for binary search", maintained centrally
/// and already a dependency of this crate), so the Regions this service accepts
/// cannot drift from the ones the rest of the emulator knows.
pub const AWS_REGIONS: &[&str] = ::emulator_telemetry::policy::KNOWN_REGIONS;

/// Whether `region` names an AWS Region ([`AWS_REGIONS`]). Every operation that
/// takes a `RegionName` asks this one question, so none of them can accept a
/// Region the others refuse. The lookup is the binary search the shared
/// catalog's ordering is maintained for.
#[must_use]
pub fn is_aws_region(region: &str) -> bool {
    AWS_REGIONS.binary_search(&region).is_ok()
}

/// The message an operation refusing a `RegionName` that names no AWS Region
/// carries. It reports that the submitted Region name is not one and quotes
/// nothing from the request, so every such refusal carries this one sentence.
pub const INVALID_REGION_MESSAGE: &str = "Invalid region name";

/// How much of a caller-supplied value a refusal message echoes before it
/// elides: the bound the framework's own constraint messages use, so a long
/// submitted value cannot amplify a small request into a large response.
const MAX_ECHOED_VALUE: usize = 256;

/// `value` as a refusal message echoes it: itself when short, else a
/// character-boundary-safe prefix of [`MAX_ECHOED_VALUE`] bytes with the
/// elision the framework's constraint messages render — `<prefix>... [<n>
/// bytes]` — so a refusal this service states by hand reads exactly like one
/// the framework stated for the same value. Rendering only: the caller checks
/// the ORIGINAL value, because eliding before a check would compare text the
/// caller never sent.
#[must_use]
pub fn echoed_value(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}... [{} bytes]",
        value.get(..end).unwrap_or_default(),
        value.len()
    )
}

/// `submitted` as this service STORES an optional text member: a value the
/// request left out and one it sent empty are stored the same way — as no value
/// at all.
///
/// Amazon Cognito materialises no empty string: a member whose modelled length
/// starts at 0 accepts `""` on input (`DescriptionType` is length 0..2048,
/// `smithy_query(query="shape",shape="DescriptionType")`), and the stored
/// resource then carries no such member, so a later read omits it rather than
/// rendering an empty value. The ONE place that normalization is decided, so
/// every writer of an optional text member agrees.
#[must_use]
pub fn stored_text(submitted: Option<String>) -> Option<String> {
    submitted.filter(|value| !value.is_empty())
}

/// The attribute values `requested` writes onto a user, as this service stores
/// them: one entry per named attribute, with every empty value dropped.
///
/// "To delete an attribute from your user, submit the attribute in your API
/// request with a blank value" (`AdminUpdateUserAttributes.UserAttributes` in
/// this service's model) — an empty value is not a value of the attribute, so an
/// attribute submitted with one is stored for no user and a read lists it for
/// none. The ONE place a submitted attribute list becomes stored attribute
/// values, so a create normalizes exactly as an update does
/// ([`plan_attribute_update`]).
#[must_use]
pub fn stored_attribute_values(requested: &[AttributeType]) -> BTreeMap<String, String> {
    requested
        .iter()
        .filter_map(|attribute| {
            stored_text(attribute.value.clone()).map(|value| (attribute.name.clone(), value))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// User import jobs
//
// A job's `Status` is COMPUTED from what is stored plus the clock, so a read
// renders the status the job is in at that instant and writes nothing.
// Each value below is one of the modelled
// [`crate::ops::types::USER_IMPORT_JOB_STATUS_TYPE_VALUES`].
// ---------------------------------------------------------------------------

/// "`Created` - The job was created but not started"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_CREATED: &str = "Created";

/// "`Pending` - A transition state. You have started the job, but it has not
/// begun importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_PENDING: &str = "Pending";

/// "`InProgress` - The job has started, and users are being imported"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// "`Stopping` - You have stopped the job, but the job has not stopped
/// importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPING: &str = "Stopping";

/// "`Stopped` - You have stopped the job, and the job has stopped importing
/// users" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPED: &str = "Stopped";

/// "`Expired` - You created a job, but did not start the job within 24-48
/// hours" (`UserImportJobType.Status`).
pub const JOB_STATUS_EXPIRED: &str = "Expired";

/// How long a started job stays `Pending` before the clock owes it
/// `InProgress`.
///
/// A chosen span, not a documented duration: `UserImportJobType.Status`
/// documents `Pending` as "A transition state. You have started the job, but it
/// has not begun importing users yet", so a job is NOT importing when
/// `StartUserImportJob` returns. Never zero, so a caller reading the job right
/// after starting it still sees the transient status.
pub const JOB_START_SPAN: Duration = Duration::from_millis(500);

/// The sentence a start refused because the job's CSV file was never uploaded
/// carries: a job "imports users from a CSV file" that the client writes to the
/// job's `PreSignedUrl` (`StartUserImportJob` and
/// `UserImportJobType.PreSignedUrl` in this service's model), so the refusal
/// reports that no file was uploaded for the job it names.
///
/// The fixed text is `No csv file was uploaded for `, which carries no trailing
/// period, and the one variable part is the `JobId` the REQUEST carried.
pub fn no_csv_uploaded_message(job_id: &str) -> String {
    format!("No csv file was uploaded for {job_id}")
}

/// How long a stopped job stays `Stopping` before the clock owes it `Stopped`.
///
/// A chosen span, not a documented duration: `Stopping` is "You have stopped the
/// job, but the job has not stopped importing users yet"
/// (`UserImportJobType.Status`), so the stop is not complete when
/// `StopUserImportJob` returns.
pub const JOB_STOP_SPAN: Duration = Duration::from_millis(500);

/// How long a created job can go unstarted before the clock owes it `Expired`:
/// "You created a job, but did not start the job within 24-48 hours ... the job
/// can't be started" (`UserImportJobType.Status`), whose earliest documented
/// bound is 24 hours.
pub const JOB_EXPIRY_SPAN: Duration = Duration::from_secs(24 * 60 * 60);

/// The sentence the operations addressed by the pair `UserPoolId` + `JobId` —
/// `DescribeUserImportJob`, `StartUserImportJob` and `StopUserImportJob` —
/// answer for a `JobId` the named pool holds no user import job for.
///
/// The fixed text is `The import job ` and ` does not exist`, which carries NO
/// trailing period, and the one variable part is the `JobId` the REQUEST
/// carried. The three resolve the same job by the same pair, so they report an
/// unresolved job with the same sentence.
pub fn import_job_not_found_message(job_id: &str) -> String {
    format!("The import job {job_id} does not exist")
}

/// The sentence `StopUserImportJob` answers for a job that is not running:
/// "the ID of a running user import job" (`StopUserImportJob.JobId`), so a job
/// in any other status has no import to stop.
///
/// The fixed text is `Job is not running. The specified job has a status of `,
/// with no trailing period and no job id; the one variable part is the status
/// the job is in as the STORE and the clock put it, never a value the request
/// carried.
pub fn job_not_running_message(status: &str) -> String {
    format!("Job is not running. The specified job has a status of {status}")
}

/// How long a passkey registration challenge stays answerable, which is also the
/// `timeout` its credential creation options advertise, so the window the caller
/// was told about is the window the emulator holds: W3C Web Authentication Level
/// 2 §5.4 recommends a `PublicKeyCredentialCreationOptions.timeout` "between
/// 300000 milliseconds (5 minutes) and 600000 milliseconds (10 minutes)" for a
/// registration ceremony.
pub const WEB_AUTHN_REGISTRATION_TIMEOUT: Duration = Duration::from_millis(300_000);

/// `UserVerificationType`'s value for a pool that encourages but does not
/// require user verification, which is what a pool configuring none asks for.
pub const USER_VERIFICATION_PREFERRED: &str = "preferred";

// ---------------------------------------------------------------------------
// Store bounds
//
// This state lives in the emulator's memory, so every store a caller can grow
// is bounded: a repeated request must not grow the process without limit.
// These caps are the EMULATOR's, not an AWS quota, and are deliberately far
// above anything a test fixture builds. The stores whose entries expire are
// swept of what the clock has already expired before the cap is applied, so the
// cap is reached only by live state.
// ---------------------------------------------------------------------------

/// The most user pools this emulator holds.
pub const MAX_USER_POOLS: usize = 1_000;

/// The most app clients one user pool holds.
pub const MAX_CLIENTS_PER_POOL: usize = 1_000;

/// The most users one user pool holds.
pub const MAX_USERS_PER_POOL: usize = 10_000;

/// The most groups one user pool holds.
pub const MAX_GROUPS_PER_POOL: usize = 1_000;

/// The most devices one user holds. A sign-in from a device the user pool does
/// not recognise stores one, so the device authenticated longest ago is evicted
/// past this cap: repeated sign-ins cannot grow one user without limit.
pub const MAX_DEVICES_PER_USER: usize = 200;

/// The most superseded passwords one user holds, beside the one it holds now:
/// `PasswordHistorySizeType` is modelled with a maximum of 24
/// (`smithy_query(query="shape",shape="PasswordHistorySizeType")`), so no pool
/// policy can restrict reuse of more passwords than this and the password set
/// longest ago is dropped past the cap.
pub const MAX_PASSWORD_HISTORY_PER_USER: usize = 24;

/// The most resource servers one user pool holds.
pub const MAX_RESOURCE_SERVERS_PER_POOL: usize = 1_000;

/// The most identity providers one user pool holds.
pub const MAX_IDENTITY_PROVIDERS_PER_POOL: usize = 1_000;

/// The most secondary replicas one user pool holds.
pub const MAX_REPLICAS_PER_POOL: usize = 100;

/// The most user import jobs one user pool holds: Amazon Cognito's quota for
/// user import jobs per user pool, which its refusal states in full
/// ([`USER_IMPORT_JOB_LIMIT_MESSAGE`]). `ListUserImportJobs` reports them a page
/// at a time, so the store is not bounded by what one listing can carry.
pub const MAX_USER_IMPORT_JOBS_PER_POOL: usize = 1_000;

/// The message `CreateUserImportJob`'s `LimitExceededException` carries once the
/// pool holds [`MAX_USER_IMPORT_JOBS_PER_POOL`] jobs.
///
/// The fixed text is `The limit for user pool import jobs is: ` and the trailing
/// `.`; the one variable part is the limit itself, so the sentence and the
/// ceiling the handler enforces can never drift apart.
pub fn user_import_job_limit_message() -> String {
    format!("The limit for user pool import jobs is: {MAX_USER_IMPORT_JOBS_PER_POOL}.")
}

/// The most custom (and developer-only) attributes one user pool's schema
/// holds: Amazon Cognito's quota for custom attributes per user pool is 50 and
/// cannot be raised (Amazon Cognito Developer Guide, "Quotas in Amazon
/// Cognito"). The standard attributes every pool carries are not counted
/// against it.
pub const MAX_CUSTOM_ATTRIBUTES_PER_POOL: usize = 50;

/// The most passkey credentials one user holds: `ListWebAuthnCredentials`'s
/// `MaxResults` is modelled with a maximum of 20
/// (`smithy_query(query="operation",operation="ListWebAuthnCredentials")`), so a
/// user holds no more passkeys than ONE listing can report.
pub const MAX_WEB_AUTHN_CREDENTIALS_PER_USER: usize = 20;

/// The most unexpired passkey registration challenges this emulator holds; the
/// oldest are evicted past it.
const MAX_WEB_AUTHN_REGISTRATIONS: usize = 10_000;

/// The most unexpired challenge sessions this emulator holds; the oldest are
/// evicted past it.
const MAX_AUTH_SESSIONS: usize = 10_000;

/// The most unexpired issued tokens this emulator holds; the oldest are evicted
/// past it. Three are recorded per sign-in, two per refresh.
const MAX_ISSUED_TOKENS: usize = 30_000;

/// Per-service state. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// Every created user pool, keyed by its minted `UserPoolId`.
    pub user_pools: HashMap<String, UserPool>,
    /// Challenge sessions, keyed by the opaque `Session` token handed to the caller.
    pub auth_sessions: HashMap<String, AuthSession>,
    /// Every token this emulator issued, keyed by the token string, so a later
    /// operation can resolve a token back to the stored user.
    pub issued_tokens: HashMap<String, TokenRecord>,
    /// The key every refresh token this emulator vends is sealed under
    /// ([`RefreshTokenKey`]), minted by the first `CreateUserPool` of this
    /// emulator.
    ///
    /// It belongs to the SERVICE rather than to any pool — a pool publishes the
    /// keys its identity and access tokens are signed with and publishes no key a
    /// refresh token is sealed under — so it is held here beside the tokens it
    /// seals, once per emulator, and nothing publishes it.
    pub refresh_token_key: ::std::sync::OnceLock<RefreshTokenKey>,
    /// The passkey registrations in flight, keyed by the challenge value the
    /// credential creation options carried to the caller.
    pub web_authn_registrations: HashMap<String, WebAuthnRegistration>,
    /// The provisioned request rate this account has set per API category, in
    /// requests per second, keyed by the category name. A category no
    /// adjustment has moved holds no entry and reports the rate it includes, so
    /// the map records the adjustments rather than the whole catalogue.
    pub provisioned_api_rates: BTreeMap<String, i32>,
    /// The account's Service Quotas ceiling per API category, in requests per
    /// second, keyed by the category name: the highest rate the account may
    /// provision for that category's limit
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::api_category_quota_ceiling`]).
    ///
    /// A category whose quota no increase has raised holds no entry, and its
    /// ceiling is the value Service Quotas starts it at — the category's own
    /// published request rate. Raising one is a Service Quotas increase
    /// request, which belongs to Service Quotas rather than to this API, so no
    /// operation of this service writes here
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::raise_api_category_quota_ceiling`]
    /// is what a granted increase does to this account).
    pub api_category_quota_ceilings: BTreeMap<String, i32>,
    /// The managed login branding styles a modification is IN FLIGHT for right
    /// now, keyed by the (`UserPoolId`, `ManagedLoginBrandingId`) pair every
    /// handler reaches a style by, which is what makes concurrent modification
    /// of one style observable: `ConcurrentModificationException` "is thrown if
    /// two or more modifications are happening concurrently", so a modification
    /// is pending for the span it takes and a second modification of the SAME
    /// style inside that span is refused instead of silently overwriting the
    /// first one's read-modify-write. Modifications of DIFFERENT styles never
    /// collide.
    ///
    /// Entered and left by [`Self::begin_branding_modification`] /
    /// [`Self::end_branding_modification`]: a span, never a stored status, so
    /// nothing is left for a later caller to clean up.
    pub modifying_branding: BTreeSet<(String, String)>,
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The stored pool, or `None` when no pool carries that id.
    pub fn pool(&self, user_pool_id: &str) -> Option<&UserPool> {
        self.user_pools.get(user_pool_id)
    }

    /// The stored pool, mutably.
    pub fn pool_mut(&mut self, user_pool_id: &str) -> Option<&mut UserPool> {
        self.user_pools.get_mut(user_pool_id)
    }

    /// The tag set of the user pool `user_pool_id` AS IT STANDS IN `region` as of
    /// `now`, or `None` when this service holds no such pool there.
    ///
    /// A pool keeps ONE id across the Regions it is replicated to, and "You can
    /// maintain tags independently on replica user pools"
    /// (`CreateUserPoolReplica.UserPoolTags`), so the Region selects WHICH tag set
    /// is addressed: the pool's own in [`UserPool::region`], and a secondary
    /// replica's own in the Region it was created in. A replica whose deletion the
    /// clock has already finished is gone, which is computed here rather than
    /// written.
    pub fn pool_tags(
        &self,
        region: &str,
        user_pool_id: &str,
        now: SystemTime,
    ) -> Option<&HashMap<String, String>> {
        let pool = self.pool(user_pool_id)?;
        if region == pool.region {
            return Some(&pool.tags);
        }
        pool.live_replica(region, now).map(|replica| &replica.tags)
    }

    /// The same tag set as [`Self::pool_tags`], mutably: what a call that assigns
    /// or removes tags writes.
    pub fn pool_tags_mut(
        &mut self,
        region: &str,
        user_pool_id: &str,
        now: SystemTime,
    ) -> Option<&mut HashMap<String, String>> {
        let pool = self.pool_mut(user_pool_id)?;
        if region == pool.region {
            return Some(&mut pool.tags);
        }
        pool.replicas
            .get_mut(region)
            .filter(|replica| !replica.removed(now))
            .map(|replica| &mut replica.tags)
    }

    /// Mark a modification of the branding style `branding_id` of the pool
    /// `user_pool_id` as in flight, answering `false` when another modification
    /// of the SAME style already is — the caller then refuses its request with
    /// the modelled `ConcurrentModificationException`.
    pub fn begin_branding_modification(&mut self, user_pool_id: &str, branding_id: &str) -> bool {
        self.modifying_branding
            .insert((user_pool_id.to_string(), branding_id.to_string()))
    }

    /// Release the in-flight marker [`Self::begin_branding_modification`] took.
    pub fn end_branding_modification(&mut self, user_pool_id: &str, branding_id: &str) {
        self.modifying_branding
            .remove(&(user_pool_id.to_string(), branding_id.to_string()));
    }

    /// The stored pool that holds the app client `client_id`, or `None` when no pool
    /// does.
    ///
    /// The unauthenticated half of this API — `InitiateAuth`, `SignUp`,
    /// `ForgotPassword`, `RespondToAuthChallenge` — identifies its user pool by
    /// `ClientId` ALONE and models no `UserPoolId` member, so the stored app client is
    /// what names the pool. An id is minted per client and lives in exactly one pool,
    /// so at most one pool can answer.
    pub fn pool_of_client(&self, client_id: &str) -> Option<&UserPool> {
        self.user_pools
            .values()
            .find(|pool| pool.clients.contains_key(client_id))
    }

    /// How an operation reports an app-client lookup it made inside the pool
    /// `user_pool_id` that resolved nothing.
    ///
    /// A `ClientId` is minted per app client and lives in exactly one pool
    /// ([`Self::pool_of_client`]), so such a lookup fails in three different
    /// ways and Amazon Cognito words them in two. An id that names an app client
    /// of ANOTHER pool, and an id asked for in a pool this emulator does not
    /// hold at all, are both reported with both halves of what was asked for
    /// ([`app_client_not_found_in_pool_message`]) — in each case the pool the
    /// request named is not one holding that app client. An id that names no app
    /// client at all, asked for in a pool that IS stored, carries `unresolved`:
    /// the sentence the ASKING operation answers for that case, which is
    /// [`APP_CLIENT_NOT_FOUND_MESSAGE`] for the operations that address the app
    /// client itself and [`app_client_not_found_message`] for the ones that
    /// quote the id.
    ///
    /// Call it only where the lookup in `user_pool_id` has already failed.
    pub fn app_client_lookup_refusal(
        &self,
        user_pool_id: &str,
        client_id: &str,
        unresolved: &str,
    ) -> String {
        if self.pool(user_pool_id).is_none()
            || self
                .pool_of_client(client_id)
                .is_some_and(|pool| pool.id != user_pool_id)
        {
            return app_client_not_found_in_pool_message(client_id, user_pool_id);
        }
        unresolved.to_string()
    }

    /// Whether the pool `user_pool_id` is stored and the domain `domain` is a
    /// stored domain of a DIFFERENT pool.
    ///
    /// A domain string names at most one pool ([`Self::domain`]), so an
    /// operation addressed by the PAIR `UserPoolId` + `Domain` can tell a pair
    /// that resolves nothing from a stored pool that is simply not the one
    /// configured with a domain some other pool holds, and the two are refused
    /// differently. A `UserPoolId` nothing answers to is the first case,
    /// whichever domain it is paired with.
    pub fn domain_of_other_pool(&self, user_pool_id: &str, domain: &str) -> bool {
        self.pool(user_pool_id).is_some()
            && self
                .domain(domain)
                .is_some_and(|stored| stored.user_pool_id != user_pool_id)
    }

    /// The stored user pool domain `domain` names, prefix or custom, or `None`
    /// when no pool in this emulator holds it.
    ///
    /// `DescribeUserPoolDomain` identifies a domain by `Domain` ALONE and models
    /// no `UserPoolId` member, so one domain string names at most one pool: the
    /// lookup spans every pool rather than one.
    pub fn domain(&self, domain: &str) -> Option<&UserPoolDomain> {
        self.user_pools
            .values()
            .find_map(|pool| pool.domain_named(domain))
    }

    /// Store a challenge session and return its token.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — and the oldest are
    /// evicted if the store is still at [`MAX_AUTH_SESSIONS`], so repeating the
    /// challenge cannot grow this map without limit.
    pub fn start_session(&mut self, now: SystemTime, session: AuthSession) -> String {
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        evict_oldest_until(
            &mut self.auth_sessions,
            MAX_AUTH_SESSIONS.saturating_sub(1),
            |stored| stored.expires_at,
        );
        let token = new_session_token();
        self.auth_sessions.insert(token.clone(), session);
        token
    }

    /// Resolve the `Session` token a challenge response carries.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — which is what tells
    /// an expired session apart from one this emulator never handed out. A
    /// session a response already answered stays stored as SPENT until the
    /// clock expires it ([`Self::end_session`]), which is what tells a replay
    /// apart from a value this emulator never handed out. A session that is
    /// neither is still outstanding: a caller whose response was refused can
    /// send a corrected one while the session lives.
    pub fn session(&mut self, token: &str, now: SystemTime) -> SessionLookup {
        let expired = self
            .auth_sessions
            .get(token)
            .filter(|stored| stored.expires_at <= now)
            .cloned();
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        match self.auth_sessions.get(token) {
            Some(stored) if stored.spent => SessionLookup::Spent(stored.clone()),
            Some(stored) => SessionLookup::Found(stored.clone()),
            None => match expired {
                Some(stored) => SessionLookup::Expired(stored),
                None => SessionLookup::Unknown,
            },
        }
    }

    /// Spend the challenge session `token` names: a session answers its
    /// challenge once.
    ///
    /// The record STAYS stored, marked spent, until the clock expires it — the
    /// sweeps in [`Self::session`] and [`Self::start_session`] are what drop it
    /// — so a caller replaying the session is answered as the replay it is
    /// rather than as a session this emulator never issued.
    pub fn end_session(&mut self, token: &str) {
        if let Some(stored) = self.auth_sessions.get_mut(token) {
            stored.spent = true;
        }
    }

    /// Store a passkey registration and return the CHALLENGE value its creation
    /// options carry, which is also the key the completing call resolves it by.
    ///
    /// Registrations the clock has already expired are swept first — the expiry
    /// was owed at this instant whether or not anybody called — and the oldest
    /// are evicted if the store is still at [`MAX_WEB_AUTHN_REGISTRATIONS`], so
    /// repeating the registration cannot grow this map without limit. A started
    /// registration leaves every registration started before it answerable: each
    /// one carries its own challenge.
    pub fn start_web_authn_registration(
        &mut self,
        now: SystemTime,
        registration: WebAuthnRegistration,
    ) -> String {
        self.web_authn_registrations
            .retain(|_, stored| stored.expires_at > now);
        evict_oldest_until(
            &mut self.web_authn_registrations,
            MAX_WEB_AUTHN_REGISTRATIONS.saturating_sub(1),
            |stored| stored.expires_at,
        );
        let challenge = new_web_authn_challenge();
        self.web_authn_registrations
            .insert(challenge.clone(), registration);
        challenge
    }

    /// The passkey registration `challenge` resolves as of `now`, or `None` when
    /// this emulator handed out no such challenge or the clock has already taken
    /// it past [`WebAuthnRegistration::expires_at`].
    ///
    /// A PURE read: an expired registration is refused where it stands rather
    /// than swept, so asking changes nothing a later caller sees.
    pub fn web_authn_registration(
        &self,
        challenge: &str,
        now: SystemTime,
    ) -> Option<&WebAuthnRegistration> {
        self.web_authn_registrations
            .get(challenge)
            .filter(|registration| registration.expires_at > now)
    }

    /// Forget the passkey registration `challenge` names: a challenge is answered
    /// once, so the call that completes a registration consumes it and a second
    /// response to the same challenge resolves nothing.
    pub fn end_web_authn_registration(&mut self, challenge: &str) {
        self.web_authn_registrations.remove(challenge);
    }

    /// The token record for `token`, or `None` when this emulator never issued it.
    pub fn token(&self, token: &str) -> Option<&TokenRecord> {
        self.issued_tokens.get(token)
    }

    /// The key every refresh token this emulator vends is sealed under.
    ///
    /// Minted on the blocking pool by the first `CreateUserPool` of this emulator,
    /// which is the create every sign-in follows, so this resolves to the stored
    /// key. It is generated HERE only if that never happened — a key is needed
    /// before a credential can be sealed, and a sign-in refusing for want of one
    /// would report no token at all.
    pub fn refresh_token_key(&self) -> &RefreshTokenKey {
        self.refresh_token_key
            .get_or_init(RefreshTokenKey::generate)
    }

    /// Record that the refresh token `token` stops authorizing a token refresh at
    /// `invalid_from`, because a refresh with rotation enabled replaced it.
    ///
    /// `RefreshTokenRotationType.RetryGracePeriodSeconds`: "the original refresh
    /// token that you're rotating out can remain valid for a period of time of up
    /// to 60 seconds. This allows for client-side retries. When
    /// `RetryGracePeriodSeconds` is `0`, the grace period is disabled and a
    /// successful request immediately invalidates the submitted refresh token"
    /// (this service's model). A token already rotated out keeps the EARLIEST
    /// instant it was invalidated from, so retrying inside the grace period
    /// cannot push that window further out.
    pub fn rotate_out_refresh_token(&mut self, token: &str, invalid_from: SystemTime) {
        if let Some(record) = self.issued_tokens.get_mut(token) {
            record.rotated_out_at = Some(match record.rotated_out_at {
                Some(already) => already.min(invalid_from),
                None => invalid_from,
            });
        }
    }

    /// The stored pool and user that an ACCESS token identifies as the currently
    /// signed-in user, as of `now`.
    ///
    /// The self-service half of this API identifies its user by the `AccessToken`
    /// the sign-in returned and models no `UserPoolId` or `Username` member at
    /// all, so the stored token record is what names the pool and the user:
    /// `ListDevices.AccessToken` is "a valid access token that Amazon Cognito
    /// issued to the currently signed-in user" (this service's model). Only an
    /// access token answers — an id or refresh token names its user too, but it
    /// is not the token the member asks for.
    ///
    /// An access token the pool issued is then checked against what it
    /// AUTHORIZES: each of these operations "must include the scope
    /// `aws.cognito.signin.user.admin`" (this service's model), so a live access
    /// token whose scopes do not carry it — the machine-to-machine token of an
    /// app client, whose scopes authorize resource servers — is told apart from a
    /// value that resolves to no access token at all
    /// ([`TokenRecord::authorizes_user_self_service`]).
    ///
    /// A PURE read: a token the clock has already expired is refused where it
    /// stands rather than swept, so asking changes nothing a later caller sees.
    pub fn signed_in_user(
        &self,
        access_token: &str,
        now: SystemTime,
    ) -> Result<(&UserPool, &User), TokenRejection> {
        let record = self
            .issued_tokens
            .get(access_token)
            .ok_or(TokenRejection::NotAnAccessToken)?;
        if !record.is_access_token() {
            return Err(TokenRejection::NotAnAccessToken);
        }
        // A record this emulator issued and has since REVOKED — a sign-out of its
        // user, or a revocation of its sign-in lineage — is refused AS revoked,
        // which is a different answer from the one a value this service cannot
        // resolve to an access token at all gets.
        if record.revoked {
            return Err(TokenRejection::Revoked);
        }
        if record.expires_at <= now {
            return Err(TokenRejection::Expired);
        }
        // A live access token still has to AUTHORIZE this half of the API: one
        // whose scopes do not include the self-service scope identifies no
        // signed-in user here, whoever it was issued to.
        if !record.authorizes_user_self_service() {
            return Err(TokenRejection::MissingRequiredScopes);
        }
        let pool = self
            .pool(&record.user_pool_id)
            .ok_or(TokenRejection::PoolGone)?;
        let user = pool
            .user(&record.username)
            .ok_or(TokenRejection::UserGone)?;
        // The record points at the user by name, and a name can be taken again:
        // the stored user it finds is the one this token was issued to only when
        // it still carries the same id, so a token outliving its user resolves to
        // no user rather than to a later namesake.
        if user.sub != record.sub {
            return Err(TokenRejection::UserGone);
        }
        // A user Amazon Cognito is waiting on a new password from authorizes no
        // self-service call until that password is set: a live access token
        // issued before the reset was asked for still resolves to this user, and
        // the operations that resolve one all declare
        // `PasswordResetRequiredException` — "thrown when a password reset is
        // required" (this service's model) — so the stored status is read here,
        // once, rather than by each of them.
        if user.status == UserStatus::ResetRequired {
            return Err(TokenRejection::PasswordResetRequired);
        }
        Ok((pool, user))
    }

    /// Issue the access / id / refresh token triple for `username` on
    /// `client_id`, record all three, and return the modelled result.
    ///
    /// Every claim comes from stored state: the `sub` and `username` from the
    /// stored user, its `cognito:groups` from the groups the stored user belongs
    /// to, `client_id` from the stored app client, the issuer from the endpoint
    /// this request is served on and the stored pool's id
    /// ([`UserPool::issuer`]), and each lifetime from the app client's validity
    /// settings — each of them as the pool's pre-token-generation function left it
    /// (`overrides`, which is what that function answered and
    /// [`lambda_triggers::TokenOverrides::none`] for a generation that invoked none). The
    /// access and id tokens are signed with the POOL's key pair, so a verifier
    /// can check them against the key set that pool publishes; the refresh token
    /// is an OPAQUE credential ([`mint_refresh_token`]), sealed under this
    /// service's own key and vended in JWE compact serialization, that carries no
    /// claim its holder can read. `None` when the pool, the client or the user is
    /// gone, when the signature could not be computed, or when no refresh
    /// credential was minted — never a token nothing signed.
    ///
    /// Tokens the clock has already expired are swept first, and the oldest are
    /// evicted if the store is still at [`MAX_ISSUED_TOKENS`], so repeated
    /// sign-ins cannot grow this map without limit.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        credentials: &::emulator_server::CredentialStore,
        parts: &::http::request::Parts,
        overrides: &lambda_triggers::TokenOverrides,
    ) -> Option<AuthenticationResultType> {
        let refresh_token = mint_refresh_token(credentials, self.refresh_token_key())?;
        self.mint_tokens(
            user_pool_id,
            client_id,
            username,
            now,
            Some(refresh_token),
            None,
            parts,
            overrides,
        )
    }

    /// Issue a fresh triple that CONTINUES the sign-in lineage of
    /// `presented_refresh_token`, for a refresh that rotates the submitted
    /// refresh token out: the new tokens carry the submitted token's
    /// `origin_jti`, so they were "generated by" it (`RevokeToken` in this
    /// service's model) and a revocation naming either one reaches them all.
    /// Otherwise identical to [`issue_tokens`].
    #[allow(clippy::too_many_arguments)]
    pub fn issue_rotated_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        presented_refresh_token: &str,
        credentials: &::emulator_server::CredentialStore,
        parts: &::http::request::Parts,
        overrides: &lambda_triggers::TokenOverrides,
    ) -> Option<AuthenticationResultType> {
        let lineage = self.token_lineage(presented_refresh_token);
        let refresh_token = mint_refresh_token(credentials, self.refresh_token_key())?;
        self.mint_tokens(
            user_pool_id,
            client_id,
            username,
            now,
            Some(refresh_token),
            lineage,
            parts,
            overrides,
        )
    }

    /// Issue the access / id token pair for a sign-in that REUSES the refresh token the
    /// caller presented, record both, and return the modelled result.
    ///
    /// `InitiateAuth`'s `AuthFlow`: "REFRESH_TOKEN_AUTH and REFRESH_TOKEN — Receive new
    /// ID and access tokens when you pass a `REFRESH_TOKEN` parameter with a valid
    /// refresh token as the value" (this service's model). The refresh token in force
    /// stays the one the caller already holds, so none is minted and the result carries
    /// no `RefreshToken` member. Otherwise identical to [`issue_tokens`], whose claim
    /// building it shares.
    #[allow(clippy::too_many_arguments)]
    pub fn refresh_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        presented_refresh_token: &str,
        parts: &::http::request::Parts,
        overrides: &lambda_triggers::TokenOverrides,
    ) -> Option<AuthenticationResultType> {
        let lineage = self.token_lineage(presented_refresh_token);
        self.mint_tokens(
            user_pool_id,
            client_id,
            username,
            now,
            None,
            lineage,
            parts,
            overrides,
        )
    }

    /// The sign-in lineage ([`TokenRecord::origin`]) of the token `token`, or
    /// `None` when this emulator issued no such token — in which case the tokens
    /// minted next start a lineage of their own.
    fn token_lineage(&self, token: &str) -> Option<String> {
        self.issued_tokens
            .get(token)
            .map(|record| record.origin.clone())
    }

    /// Issue the machine-to-machine access token of the app client `client_id`
    /// in `user_pool_id`, carrying `scopes`, record it, and return the modelled
    /// result.
    ///
    /// "Issues an access token for machine-to-machine (M2M) authorization. Your
    /// app client provides its client ID and secret, and receives an access
    /// token that authorizes requests to your resource servers"
    /// (`GetClientToken` in this service's model): the token is issued to the
    /// APP CLIENT, so its subject is that client's id, it carries no user claim
    /// and no id or refresh token is minted beside it. Its lifetime is the app
    /// client's own configured access-token validity — the same setting that
    /// bounds every other access token this client issues — and it is signed
    /// with the POOL's key pair and names that key by `kid`, so a resource
    /// server verifies it against the key set that pool publishes.
    ///
    /// `None` when the pool or the app client is gone, or when the signature
    /// could not be computed — never a token nothing signed.
    ///
    /// Tokens the clock has already expired are swept first, and the oldest are
    /// evicted if the store is still at [`MAX_ISSUED_TOKENS`], exactly as a
    /// sign-in's are.
    pub fn issue_client_token(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        scopes: &[String],
        now: SystemTime,
        parts: &::http::request::Parts,
    ) -> Option<ClientAuthenticationResultType> {
        let (issuer, signing_keys, access_ttl) = {
            let pool = self.user_pools.get(user_pool_id)?;
            let client = pool.clients.get(client_id)?;
            (
                pool.issuer(&served_endpoint(parts, &pool.region)),
                pool.signing_keys.clone(),
                client.access_token_ttl(),
            )
        };

        let issued_at = unix_seconds(now);
        let jti = new_uuid();
        // The expiry is derived from the app client's configured validity, which
        // a caller chose, so both sums are CHECKED and computed before anything
        // is stored: a validity whose expiry cannot be represented issues no
        // token at all rather than storing half a record.
        let expires_at = now.checked_add(access_ttl)?;
        let exp = expiry_claim(issued_at, access_ttl)?;
        let claims = serde_json::json!({
            "sub": client_id,
            "iss": issuer,
            "client_id": client_id,
            "token_use": "access",
            // An access token's granted scopes are carried as "a list of
            // space-delimited, case-sensitive strings" (RFC 6749 §3.3), in the
            // order the grant authorized them.
            "scope": scopes.join(" "),
            "iat": issued_at,
            "exp": exp,
            "jti": jti.clone(),
        });
        let access_token = signed_jws(&claims, signing_keys.for_use(SigningKeyUse::AccessToken))?;

        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(1),
            |record| record.expires_at,
        );
        self.issued_tokens.insert(
            access_token.clone(),
            TokenRecord {
                user_pool_id: user_pool_id.to_string(),
                client_id: client_id.to_string(),
                // Issued to the app client, not to a user: the record names the
                // client as its subject and no user at all.
                username: String::new(),
                sub: client_id.to_string(),
                kind: TokenKind::ClientCredentials,
                // The scopes this grant authorized, which are what the token's
                // `scope` claim above carries.
                scopes: scopes.to_vec(),
                // Issued alone, with no `origin_jti` claim to share: its own
                // `jti` is its lineage, so no revocation of a sign-in's tokens
                // reaches it.
                origin: jti,
                issued_at: now,
                expires_at,
                revoked: false,
                rotated_out_at: None,
            },
        );

        Some(ClientAuthenticationResultType {
            access_token: Some(access_token),
            // "The number of seconds until the access token expires"
            // (`ClientAuthenticationResultType.ExpiresIn` in this service's
            // model).
            expires_in: i32::try_from(access_ttl.as_secs()).unwrap_or(i32::MAX),
            // "The type of the token. For example, `Bearer`"
            // (`ClientAuthenticationResultType.TokenType`), which is the type a
            // bearer access token of this API is presented as.
            token_type: Some("Bearer".to_string()),
        })
    }

    /// The one token-minting path behind [`issue_tokens`] and [`refresh_tokens`]:
    /// `refresh_token` is the opaque refresh credential to record and report beside the
    /// access and id tokens, and `None` for a refresh that reuses the one the caller
    /// already holds, so the two entry points cannot disagree about a claim.
    ///
    /// Every token it SIGNS is signed with the POOL's key pair and names that key by
    /// `kid`, so a verifier can check it against the key set the pool publishes; a token
    /// whose signature could not be computed is not issued at all ([`signed_jws`]). The
    /// refresh credential is signed by nothing and carries no claims: it is opaque to
    /// its holder, and the record this writes for it is the only place its sign-in is
    /// stated.
    ///
    /// `lineage` is the `origin_jti` the minted tokens join — the sign-in of the
    /// refresh token a refresh presented — and `None` starts a new one, which is
    /// what a first sign-in does.
    ///
    /// `parts` is the request being served, which is what the `iss` claim names:
    /// the user-pool endpoint this call reached ([`served_endpoint`]) with the
    /// pool's id appended, so the issuer a token announces is the address its
    /// holder is already talking to and the one a verifier resolves the pool's
    /// key set from.
    #[allow(clippy::too_many_arguments)]
    fn mint_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        refresh_token: Option<String>,
        lineage: Option<String>,
        parts: &::http::request::Parts,
        overrides: &lambda_triggers::TokenOverrides,
    ) -> Option<AuthenticationResultType> {
        let TokenInputs {
            issuer,
            sub,
            stored_username,
            email,
            groups,
            role_claims,
            signing_keys,
            access_ttl,
            id_ttl,
            refresh_ttl,
        } = {
            let pool = self.user_pools.get(user_pool_id)?;
            let client = pool.clients.get(client_id)?;
            let user = pool.user(username)?;
            TokenInputs {
                issuer: pool.issuer(&served_endpoint(parts, &pool.region)),
                sub: user.sub.clone(),
                stored_username: user.username.clone(),
                email: user.attributes.get("email").cloned(),
                groups: pool.user_group_names(user),
                role_claims: pool.user_group_role_claims(user),
                signing_keys: pool.signing_keys.clone(),
                access_ttl: client.access_token_ttl(),
                id_ttl: client.id_token_ttl(),
                refresh_ttl: client.refresh_token_ttl(),
            }
        };

        // What the pool's pre-token-generation function asked these tokens to carry, where
        // one ran and answered: the group configuration it replaced the user's stored
        // memberships with, the scopes it added to or took out of the access token's grant,
        // and (below, once every computed claim is in place) the claims it gave values for
        // or suppressed. A generation that invoked no function overrides none of it.
        let (groups, role_claims) = overrides.group_claims(groups, role_claims);

        let issued_at = unix_seconds(now);
        let origin_jti = lineage.unwrap_or_else(new_uuid);

        // Every expiry is derived from the app client's configured validity, which
        // the request chose, so each sum is computed with CHECKED arithmetic and
        // before anything is stored: a validity whose expiry cannot be represented
        // mints no token at all, the same answer this function already gives for a
        // token it could not sign, rather than panicking part-way through. The
        // refresh token's own expiry is computed only where that token is minted.
        let access_expires_at = now.checked_add(access_ttl)?;
        let id_expires_at = now.checked_add(id_ttl)?;
        let access_exp = expiry_claim(issued_at, access_ttl)?;
        let id_exp = expiry_claim(issued_at, id_ttl)?;

        // An access token of a sign-in authorizes the user self-service
        // operations, so it carries the one scope those operations require, and
        // the claim and the record read it from the same place.
        let access_scopes = overrides.access_scopes(vec![USER_ADMIN_SCOPE.to_string()]);
        let mut access_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "client_id": client_id,
            "username": stored_username,
            "token_use": "access",
            "scope": access_scopes.join(" "),
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": access_exp,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        let mut id_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "aud": client_id,
            "cognito:username": stored_username,
            "token_use": "id",
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": id_exp,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        if let (Some(email), Some(members)) = (email, id_claims.as_object_mut()) {
            members.insert("email".to_string(), serde_json::Value::String(email));
        }
        // An access token a pre-token-generation function suppressed every scope of is
        // authorized for none, and the claim states the scopes a token IS authorized for
        // ("a list of space-delimited, case-sensitive strings", RFC 6749 §3.3), so it
        // carries no `scope` claim rather than an empty one — the same answer the group
        // claim below gives for a generation left with no group.
        if access_scopes.is_empty() {
            if let Some(members) = access_claims.as_object_mut() {
                members.remove("scope");
            }
        }

        // `AdminAddUserToGroup`: "A user who is in a group ... populates a
        // `cognito:groups` claim to their access and identity tokens" (this
        // service's model), so the claim is carried by both of those two tokens
        // and by neither the refresh token. The groups are the ones this generation
        // carries — the user's stored memberships, or the configuration a
        // pre-token-generation function replaced them with. A generation with no group
        // at all carries no claim rather than an empty list. The model documents no order
        // for the list, so it is rendered in the stored order, which is stable across
        // reads.
        if !groups.is_empty() {
            let claim = serde_json::Value::Array(
                groups
                    .iter()
                    .map(|name| serde_json::Value::String(name.clone()))
                    .collect(),
            );
            for claims in [&mut access_claims, &mut id_claims] {
                if let Some(members) = claims.as_object_mut() {
                    members.insert("cognito:groups".to_string(), claim.clone());
                }
            }
        }

        // The ROLE a group grants is the identity token's alone: "Amazon Cognito ID tokens
        // have a `cognito:preferred_role` claim that presents the highest-precedence group
        // that a user belongs to. Both ID and access tokens also contain a `cognito:groups`
        // claim" (`CreateGroup`'s `RoleArn`, this service's model). Which roles those are,
        // and whether the groups name one of them unambiguously, is the stored precedence's
        // decision (`UserPool::user_group_role_claims`); a user whose groups grant no role
        // carries neither claim rather than an empty list, and one whose best precedence is
        // tied between different roles carries the list without a preference.
        if let Some(members) = id_claims.as_object_mut() {
            if !role_claims.roles.is_empty() {
                members.insert(
                    "cognito:roles".to_string(),
                    serde_json::Value::Array(
                        role_claims
                            .roles
                            .iter()
                            .map(|role_arn| serde_json::Value::String(role_arn.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(preferred_role) = role_claims.preferred_role {
                members.insert(
                    "cognito:preferred_role".to_string(),
                    serde_json::Value::String(preferred_role),
                );
            }
        }

        // Last of all, what the pre-token-generation function asked each token's own claims
        // to be: it customises what the state computed above, so it is applied once every
        // computed claim is in place and before anything is signed.
        overrides.apply_to_id_token(&mut id_claims);
        overrides.apply_to_access_token(&mut access_claims);

        // Each token is signed with the pool key of ITS OWN kind, so the access
        // token and the identity token of one sign-in name different keys of the
        // pool's published key set.
        let access_token =
            signed_jws(&access_claims, signing_keys.for_use(SigningKeyUse::AccessToken))?;
        let id_token = signed_jws(&id_claims, signing_keys.for_use(SigningKeyUse::IdToken))?;

        let minted = 2 + usize::from(refresh_token.is_some());
        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(minted),
            |record| record.expires_at,
        );

        // A token's lifetime comes from the app client's stored validity
        // settings, which a caller sets, so every instant a token expires at is
        // the CHECKED sum computed above, before anything is stored: an
        // unrepresentable one answers no token at all rather than unwinding out
        // of this state mutation or leaving half a triple behind.
        let refresh_expires_at = match refresh_token.as_ref() {
            Some(_) => Some(now.checked_add(refresh_ttl)?),
            None => None,
        };
        let mut records = Vec::with_capacity(minted);
        for (token, kind, expires_at) in [
            Some((&access_token, TokenKind::Access, access_expires_at)),
            Some((&id_token, TokenKind::Id, id_expires_at)),
            refresh_token
                .as_ref()
                .zip(refresh_expires_at)
                .map(|(token, expires_at)| (token, TokenKind::Refresh, expires_at)),
        ]
        .into_iter()
        .flatten()
        {
            records.push((
                token.clone(),
                TokenRecord {
                    user_pool_id: user_pool_id.to_string(),
                    client_id: client_id.to_string(),
                    username: stored_username.clone(),
                    sub: sub.clone(),
                    kind,
                    // Only the access token of the triple carries a `scope`
                    // claim, so only its record holds scopes.
                    scopes: match kind {
                        TokenKind::Access => access_scopes.clone(),
                        _ => Vec::new(),
                    },
                    origin: origin_jti.clone(),
                    issued_at: now,
                    expires_at,
                    revoked: false,
                    rotated_out_at: None,
                },
            ));
        }
        for (token, record) in records {
            self.issued_tokens.insert(token, record);
        }

        Some(AuthenticationResultType {
            access_token: Some(access_token),
            // `ExpiresIn` is the access token's lifetime, in seconds.
            expires_in: i32::try_from(access_ttl.as_secs()).unwrap_or(i32::MAX),
            token_type: Some("Bearer".to_string()),
            refresh_token,
            id_token: Some(id_token),
            new_device_metadata: None,
        })
    }

    /// Record the device a completed sign-in presented and answer the
    /// `AuthenticationResult.NewDeviceMetadata` that sign-in owes.
    ///
    /// `DeviceConfigurationType`: "When you provide a value for any property of
    /// `DeviceConfiguration`, you activate the device remembering for the user
    /// pool", and `NewDeviceMetadataType` is "Information that your user pool
    /// responds with in `AuthenticationResult` when you configure it to remember
    /// devices and a user signs in with an unrecognized device" (this service's
    /// model). So a pool that stores NO `DeviceConfiguration` answers `None`,
    /// whatever the sign-in presented, and so does a sign-in that presented a
    /// `DEVICE_KEY` this user already holds — that device is recognized, and only
    /// its last-authenticated instant moves. Anything else is an unrecognized
    /// device: a device is stored for the user and its key, beside the pool's
    /// device group key, is what the caller gets back. A key the user does not
    /// hold — another user's, or one nothing minted — is unrecognized by this
    /// user pool for this user, so it is answered the same way as none at all
    /// rather than refusing the sign-in the first factor already passed.
    ///
    /// The stored device is NOT confirmed yet: `ConfirmDevice` is what confirms
    /// it and decides whether it is remembered.
    pub fn track_sign_in_device(
        &mut self,
        user_pool_id: &str,
        username: &str,
        presented_device_key: Option<&str>,
        now: SystemTime,
    ) -> Option<NewDeviceMetadataType> {
        let pool = self.user_pools.get_mut(user_pool_id)?;
        // A pool that provided no `DeviceConfiguration` activated no device
        // remembering, so it answers nothing whatever the sign-in presented.
        pool.device_configuration.as_ref()?;
        let region = pool.region.clone();
        let device_group_key = pool.device_group_key.clone();
        let user = pool.user_mut(username)?;
        if let Some(device) = presented_device_key.and_then(|key| user.devices.get_mut(key)) {
            device.last_authenticated_date = Some(now);
            return None;
        }
        let device_key = new_device_key(&region);
        user.store_device(Device::signed_in(device_key.clone(), now));
        Some(NewDeviceMetadataType {
            device_key: Some(device_key),
            device_group_key: Some(device_group_key),
        })
    }

    /// Forget the outstanding sign-in state of `username` in `user_pool_id`:
    /// the challenge sessions and the passkey registrations that could still
    /// carry a sign-in to completion. Called when the user they belong to is
    /// deleted, which is why a deleted user profile "can no longer be used to
    /// sign in" (`DeleteUser` in this service's model).
    ///
    /// The tokens already ISSUED to that user stay stored and keep naming it:
    /// they are self-contained bearer tokens their holder still has, so one
    /// presented after the profile is gone resolves the user it names and finds
    /// nothing. That is what makes a call made with such a token a refusal for
    /// the user that is gone ([`TokenRejection::UserGone`]) rather than a
    /// rejection of the token itself — and the user it names being gone, the
    /// token authorizes nothing. What bounds the record map is the same thing
    /// that bounds it for a live user: [`Self::mint_tokens`] sweeps every record
    /// the clock has expired and evicts the oldest at [`MAX_ISSUED_TOKENS`].
    pub fn forget_user_sign_in_state(&mut self, user_pool_id: &str, username: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.username == username));
        self.web_authn_registrations
            .retain(|_, r| !(r.user_pool_id == user_pool_id && r.username == username));
    }

    /// Revoke every token this emulator issued to `username` in `user_pool_id`,
    /// and forget its outstanding challenge sessions.
    ///
    /// A revoked record stays stored and stays refused: the token checks reject a
    /// revoked token wherever one is presented, so a token a caller still holds
    /// authorizes nothing after the user it names is deactivated.
    pub fn revoke_user_tokens(&mut self, user_pool_id: &str, username: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.username == username));
        for record in self.issued_tokens.values_mut() {
            if record.user_pool_id == user_pool_id && record.username == username {
                record.revoked = true;
            }
        }
    }

    /// Revoke every token of the sign-in lineage `origin` in `user_pool_id`:
    /// the refresh token that lineage was minted with, the access and id tokens
    /// minted beside it, and every token a refresh with it issued.
    ///
    /// "Revokes all of the access tokens generated by, and at the same time as,
    /// the specified refresh token. After a token is revoked, you can't use the
    /// revoked token to access Amazon Cognito user APIs, or to authorize access
    /// to your resource server" (`RevokeToken` in this service's model), so a
    /// revoked record stays stored and stays refused wherever the token it
    /// records is later presented, exactly as a signed-out user's does.
    pub fn revoke_token_lineage(&mut self, user_pool_id: &str, origin: &str) {
        for record in self.issued_tokens.values_mut() {
            if record.user_pool_id == user_pool_id && record.origin == origin {
                record.revoked = true;
            }
        }
    }

    /// Forget every session, token and passkey registration issued in
    /// `user_pool_id`. Called when the pool they belong to is deleted.
    pub fn forget_pool_credentials(&mut self, user_pool_id: &str) {
        self.auth_sessions
            .retain(|_, s| s.user_pool_id != user_pool_id);
        self.issued_tokens
            .retain(|_, t| t.user_pool_id != user_pool_id);
        self.web_authn_registrations
            .retain(|_, r| r.user_pool_id != user_pool_id);
    }

    /// Forget every outstanding challenge session started for the app client
    /// `client_id` of `user_pool_id`. Called when that app client is deleted: a
    /// response to such a challenge carries the `ClientId` the pool no longer
    /// holds, so the session can never be answered.
    ///
    /// The tokens the client issued stay stored and keep resolving the user they
    /// name until the clock expires them, exactly as an already-issued token
    /// does; a refresh is refused where it stands, because the app client the
    /// request has to name is gone.
    pub fn forget_client_sessions(&mut self, user_pool_id: &str, client_id: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.client_id == client_id));
    }
}

/// AWS's refusal message for a managed login branding style this service holds
/// none of: fixed text that names no style, because the request already carries
/// the id that found nothing. Shared by every operation that reaches a style by
/// its `ManagedLoginBrandingId`, so the readers and the writers cannot disagree
/// about what a missing style is.
pub const BRANDING_NOT_FOUND_MESSAGE: &str = "ManagedLoginBranding does not exist.";

/// AWS's refusal message for an app client that has no managed login branding
/// style assigned, naming the app client from the request.
pub fn branding_for_client_not_found_message(client_id: &str) -> String {
    format!("ManagedLoginBranding for client {client_id} does not exist.")
}

/// The message a modification of a branding style that another modification is
/// already applying is refused with: "This exception is thrown if two or more
/// modifications are happening concurrently"
/// (`ConcurrentModificationException` in this service's model). It names no
/// style, so every loser of a race carries this one sentence.
pub const CONCURRENT_BRANDING_MODIFICATION_MESSAGE: &str =
    "Only one request to modify a ManagedLoginBranding can be processed at a time";

/// The `ClientId` that names a user pool's POOL-LEVEL classic hosted UI
/// branding: "Set the default configuration for all clients with a ClientId of
/// ALL" (`SetUICustomization`), which is also the level an app client without
/// branding of its own is served from.
pub const UI_CUSTOMIZATION_ALL_CLIENTS: &str = "ALL";

/// One stored classic hosted UI branding configuration of a user pool: what
/// `SetUICustomization` persisted for one level, and what every later read of
/// that level renders.
///
/// The level is the app client the branding applies to, or
/// [`UI_CUSTOMIZATION_ALL_CLIENTS`] for the pool-level default: "When the
/// ClientId value is an app client ID, the settings you pass in this request
/// apply to that app client and override the default ALL configuration"
/// (`SetUICustomization`).
#[derive(Debug, Clone)]
pub struct UiCustomization {
    /// The level this branding applies to, which is the key it is stored under.
    pub client_id: String,
    /// The stored style sheet: "A plaintext CSS file that contains the custom
    /// fields that you want to apply to your user pool or app client"
    /// (`SetUICustomization.CSS`), kept exactly as submitted.
    pub css: Option<String>,
    /// The stored logo image: "The image that you want to set as your login in
    /// the classic hosted UI" (`SetUICustomization.ImageFile`), kept byte for
    /// byte as submitted.
    pub image_file: Option<Vec<u8>>,
    /// The version this branding is at, reported as
    /// `UICustomizationType.CSSVersion`, "The CSS version number": the stamp of
    /// the write that produced what is stored, so a read reports the version of
    /// the configuration it renders.
    pub css_version: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

/// The version a write at `at` leaves the configuration it wrote at: that
/// instant in UTC as the digits of the modelled timestamp rendering
/// (`yyyymmddhhmmss`), so a later write is at a later version and the version is
/// a number as `UICustomizationType.CSSVersion` ("The CSS version number") and
/// `DomainDescriptionType.Version` ("The app version") each report one. The
/// stamp is 14 digits, inside `DomainVersionType`'s modelled length 1..20.
fn version_stamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// The most a managed login branding style's settings and image files may
/// occupy together: "This operation has a 2-megabyte request-size limit and
/// include the CSS settings and image assets for your app client"
/// (`CreateManagedLoginBranding` in this service's model), so a style past it is
/// a style Amazon Cognito would not have accepted in one request.
pub const MAX_BRANDING_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// The bytes a branding style's payload takes on the wire: its settings
/// document plus the image files it carries, which is what
/// [`MAX_BRANDING_PAYLOAD_BYTES`] bounds. An approximation of the request's own
/// size — it counts the members that carry the style and not the surrounding
/// protocol framing — so a style measured inside the limit is one a single
/// request can carry.
pub fn branding_payload_bytes(
    settings: Option<&::aws_smithy_types::Document>,
    assets: &[crate::ops::types::AssetType],
) -> usize {
    let settings = settings.map(document_bytes).unwrap_or_default();
    assets.iter().fold(settings, |total, asset| {
        total
            .saturating_add(asset.category.len())
            .saturating_add(asset.color_mode.len())
            .saturating_add(asset.extension.len())
            .saturating_add(asset.resource_id.as_ref().map_or(0, String::len))
            // A blob travels base64-encoded (`AssetType.Bytes` is "the image
            // file, in Base64-encoded binary"), which is four characters per
            // three bytes, rounded up to the next quantum.
            .saturating_add(base64_len(asset.bytes.as_ref().map_or(0, Vec::len)))
    })
}

/// The length base64 encodes `bytes` bytes to: four characters per three-byte
/// group, the last group padded.
fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3).saturating_mul(4)
}

/// The bytes a settings document takes on the wire: every string and member name
/// counts its own UTF-8 length, a number counts eight bytes and every other
/// scalar one byte, so a document cannot be grown without the count growing with
/// it.
///
/// Walked ITERATIVELY over an explicit worklist: the document's nesting depth is
/// the caller's to choose, and a recursive walk of it would put that depth on
/// the stack.
fn document_bytes(document: &::aws_smithy_types::Document) -> usize {
    let mut total: usize = 0;
    let mut pending: Vec<&::aws_smithy_types::Document> = vec![document];
    while let Some(node) = pending.pop() {
        let node_bytes = match node {
            ::aws_smithy_types::Document::Null | ::aws_smithy_types::Document::Bool(_) => 1,
            ::aws_smithy_types::Document::Number(_) => ::std::mem::size_of::<f64>(),
            ::aws_smithy_types::Document::String(text) => text.len(),
            ::aws_smithy_types::Document::Array(items) => {
                pending.extend(items.iter());
                1
            }
            ::aws_smithy_types::Document::Object(members) => {
                members.iter().fold(1_usize, |names, (name, member)| {
                    pending.push(member);
                    names.saturating_add(name.len())
                })
            }
        };
        total = total.saturating_add(node_bytes);
    }
    total
}

/// A challenge session: what a `RespondToAuthChallenge` for this session resumes.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub user_pool_id: String,
    pub client_id: String,
    pub username: String,
    pub challenge_name: String,
    pub expires_at: SystemTime,
    /// The one-time code this challenge is answered with, for a challenge whose
    /// start DELIVERED one (the email one-time password), and `None` for a
    /// challenge that is answered with something else.
    ///
    /// The code lives on the session because that is what bounds it: the
    /// challenge is answerable only while its session lasts
    /// (`CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes,
    /// of that session token"), so the code's window is the session's window.
    pub code: Option<VerificationCode>,
    /// Whether a response has already ANSWERED this session's challenge
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::end_session`]).
    ///
    /// A spent session stays stored until the clock expires it, which is what
    /// tells a session a caller is replaying apart from a value this emulator
    /// never handed out: the two owe different refusals
    /// ([`SESSION_ALREADY_USED_MESSAGE`] and
    /// [`SESSION_NOT_FOR_THIS_REQUEST_MESSAGE`]).
    pub spent: bool,
}

/// What resolving a `Session` token found
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::session`]).
/// An expired session is told apart from an unknown one because a challenge
/// response owes a different refusal for each.
#[derive(Debug, Clone)]
pub enum SessionLookup {
    /// The stored session, still inside its validity window and not yet
    /// answered.
    Found(AuthSession),
    /// A stored session whose validity window had already closed, as it was
    /// stored: which challenge it was waiting on decides the refusal it owes.
    Expired(AuthSession),
    /// A stored session an earlier response already ANSWERED: the challenge it
    /// maintained the state of is over, so this one is a replay.
    Spent(AuthSession),
    /// No session of that token: this emulator never handed it out.
    Unknown,
}

/// Which token a [`TokenRecord`] records: one of a sign-in's triple, or the
/// machine-to-machine token an app client gets for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Id,
    Refresh,
    /// An access token issued to an APP CLIENT rather than to a user, "for
    /// machine-to-machine (M2M) authorization" (`GetClientToken` in this
    /// service's model). It "authorizes requests to your resource servers", so
    /// it is not the access token "that Amazon Cognito issued to the currently
    /// signed-in user" the self-service operations ask for, and the record of
    /// it names no user.
    ClientCredentials,
}

/// One issued token, so a later operation can resolve it back to its user.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub user_pool_id: String,
    pub client_id: String,
    /// The user this token was issued to, and EMPTY for a token that was issued
    /// to the app client itself ([`TokenKind::ClientCredentials`]), which names
    /// no user: a username is at least one character long, so no user-scoped
    /// lookup or revocation matches such a record.
    pub username: String,
    /// The `sub` claim this token carries: the immutable id of the user it was
    /// issued to. A user created later under the same username is a different
    /// user with a different id, so recording the id beside the name is what
    /// keeps a token of a deleted user from resolving to its namesake. A
    /// [`TokenKind::ClientCredentials`] token is issued to the app client
    /// itself, so its subject is that client's id.
    pub sub: String,
    pub kind: TokenKind,
    /// The scopes this token's `scope` claim carries, in the order it carries
    /// them: the self-service scope for an access token of a sign-in, and the
    /// scopes the grant authorized for a client-credentials one. An id or
    /// refresh token carries no `scope` claim, so it holds none.
    pub scopes: Vec<String>,
    /// The `origin_jti` claim this token carries: the id of the SIGN-IN it came
    /// from, shared by the triple minted together and by every access or id
    /// token a refresh with that sign-in's refresh token minted. It is what
    /// makes the tokens "generated by, and at the same time as, the specified
    /// refresh token" (`RevokeToken` in this service's model) one unit a
    /// revocation can name. A [`TokenKind::ClientCredentials`] token is issued
    /// alone and carries no `origin_jti` claim, so its record holds its own
    /// `jti` and shares a lineage with nothing.
    pub origin: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub revoked: bool,
    /// When refresh token rotation stops this REFRESH token authorizing a token
    /// refresh, once a rotation has replaced it; `None` for a token no rotation
    /// has replaced. Recorded rather than deleted, so a refresh with the
    /// replaced token is answered as the reuse it is
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::rotate_out_refresh_token`]).
    pub rotated_out_at: Option<SystemTime>,
}

impl TokenRecord {
    /// Whether this record is of an ACCESS token: the one a sign-in issued to a
    /// user, or the one a client-credentials grant issued to an app client. Both
    /// state `token_use` `access`; an id or refresh token does not.
    pub fn is_access_token(&self) -> bool {
        matches!(self.kind, TokenKind::Access | TokenKind::ClientCredentials)
    }

    /// Whether this token authorizes the user self-service operations: it has to
    /// be an access token a USER's sign-in issued, carrying the
    /// [`USER_ADMIN_SCOPE`] those operations require.
    ///
    /// A client-credentials token authorizes "requests to your resource servers"
    /// (`GetClientToken` in this service's model) and identifies an app client
    /// rather than a signed-in user, so it authorizes none of them whatever its
    /// granted scopes are.
    pub fn authorizes_user_self_service(&self) -> bool {
        self.kind == TokenKind::Access
            && self.scopes.iter().any(|scope| scope == USER_ADMIN_SCOPE)
    }

    /// Whether refresh token rotation has already invalidated this token as of
    /// `now`: a rotation recorded the instant it stops being accepted, and up to
    /// that instant it still authorizes a retry.
    pub fn rotated_out_by(&self, now: SystemTime) -> bool {
        self.rotated_out_at.is_some_and(|invalid_from| now >= invalid_from)
    }
}

/// A passkey registration in flight: what `StartWebAuthnRegistration` handed out
/// credential creation options for, and what the call that completes the
/// registration resolves by the challenge it answers.
#[derive(Debug, Clone)]
pub struct WebAuthnRegistration {
    pub user_pool_id: String,
    /// The app client the access token that started this registration was issued
    /// to: a credential presented with an access token "for a different client
    /// than the one in the original StartWebAuthnRegistration request" is
    /// refused (`WebAuthnClientMismatchException` in this service's model), so
    /// the client is recorded with the challenge.
    pub client_id: String,
    pub username: String,
    /// The relying party the credential is registered under, as it was in force
    /// when the registration started.
    pub relying_party_id: String,
    /// The user-verification requirement the creation options asked for.
    pub user_verification: String,
    pub created_at: SystemTime,
    /// When the challenge stops being answerable, one
    /// [`WEB_AUTHN_REGISTRATION_TIMEOUT`] after it was handed out.
    pub expires_at: SystemTime,
}

/// One passkey credential registered to a user, as the completed registration
/// stored it and every later read of it renders.
#[derive(Debug, Clone)]
pub struct WebAuthnCredential {
    /// "The unique identifier of the passkey credential"
    /// (`WebAuthnCredentialDescription.CredentialId`), which the authenticator
    /// minted and the registration presented.
    pub credential_id: String,
    /// "An automatically-generated friendly name for the passkey credential"
    /// (`WebAuthnCredentialDescription.FriendlyCredentialName`).
    pub friendly_credential_name: String,
    /// The relying party the credential was registered under.
    pub relying_party_id: String,
    /// "The general category of the passkey authenticator"
    /// (`WebAuthnCredentialDescription.AuthenticatorAttachment`), when the
    /// registration reported one.
    pub authenticator_attachment: Option<String>,
    /// "The transport methods of the passkey credential, for example USB or
    /// Bluetooth Low Energy"
    /// (`WebAuthnCredentialDescription.AuthenticatorTransports`).
    pub authenticator_transports: Vec<String>,
    pub created_at: SystemTime,
}

impl WebAuthnCredential {
    /// The modelled view of this passkey credential, rendered from what the
    /// completed registration STORED, so a later read reports the registered
    /// credential and not the request that registered it.
    pub fn to_credential_description(&self) -> WebAuthnCredentialDescription {
        WebAuthnCredentialDescription {
            credential_id: self.credential_id.clone(),
            friendly_credential_name: self.friendly_credential_name.clone(),
            relying_party_id: self.relying_party_id.clone(),
            authenticator_attachment: self.authenticator_attachment.clone(),
            authenticator_transports: self.authenticator_transports.clone(),
            created_at: to_date_time(self.created_at),
        }
    }
}

/// The time unit a token-validity number is counted in
/// (`TokenValidityUnitsType`: `seconds`, `minutes`, `hours`, `days`).
#[derive(Debug, Clone, Copy)]
enum TokenValidityUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TokenValidityUnit {
    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "seconds" => Some(Self::Seconds),
            "minutes" => Some(Self::Minutes),
            "hours" => Some(Self::Hours),
            "days" => Some(Self::Days),
            _ => None,
        }
    }

    fn seconds(self) -> u64 {
        match self {
            Self::Seconds => 1,
            Self::Minutes => 60,
            Self::Hours => 60 * 60,
            Self::Days => 24 * 60 * 60,
        }
    }
}

/// The `exp` claim of a token issued `issued_at` seconds after the epoch and
/// living for `ttl`, and `None` when that sum is not representable as the whole
/// seconds a claim states: a token whose expiry cannot be stated is not issued.
fn expiry_claim(issued_at: i64, ttl: Duration) -> Option<i64> {
    issued_at.checked_add(i64::try_from(ttl.as_secs()).ok()?)
}

/// `validity` counted in `unit` (or `default_unit` when the client set none), or
/// `default_ttl` when the client set no validity at all.
fn token_ttl(
    validity: Option<i32>,
    unit: Option<&str>,
    default_unit: TokenValidityUnit,
    default_ttl: Duration,
) -> Duration {
    let Some(validity) = validity.and_then(|validity| u64::try_from(validity).ok()) else {
        return default_ttl;
    };
    let unit = unit
        .and_then(TokenValidityUnit::parse)
        .unwrap_or(default_unit);
    Duration::from_secs(validity.saturating_mul(unit.seconds()))
}

/// Drop entries from `map`, the one `expiry` puts first soonest, until `map`
/// holds at most `keep`.
fn evict_oldest_until<V, F: Fn(&V) -> SystemTime>(
    map: &mut HashMap<String, V>,
    keep: usize,
    expiry: F,
) {
    while map.len() > keep {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| expiry(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// Amazon Cognito's default password policy for a new pool.
pub fn default_password_policy() -> PasswordPolicyType {
    PasswordPolicyType {
        minimum_length: Some(DEFAULT_PASSWORD_MIN_LENGTH),
        require_uppercase: true,
        require_lowercase: true,
        require_numbers: true,
        require_symbols: true,
        password_history_size: None,
        temporary_password_validity_days: DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS,
    }
}

/// The pool policies a request leaves unset, filled in with the documented
/// defaults. A policy the request DOES carry is stored as sent.
pub fn effective_policies(requested: Option<UserPoolPolicyType>) -> UserPoolPolicyType {
    let mut policies = requested.unwrap_or_default();
    let mut password_policy = policies
        .password_policy
        .unwrap_or_else(default_password_policy);
    if password_policy.minimum_length.is_none() {
        password_policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
    }
    if password_policy.temporary_password_validity_days == 0 {
        password_policy.temporary_password_validity_days =
            DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
    }
    policies.password_policy = Some(password_policy);
    if policies.sign_in_policy.is_none() {
        policies.sign_in_policy = Some(SignInPolicyType {
            // Password is the first auth factor of a pool that configures none.
            allowed_first_auth_factors: Some(vec!["PASSWORD".to_string()]),
        });
    }
    policies
}

/// The `AdminCreateUserConfig` a request leaves unset, with the model's
/// documented defaults filled in.
pub fn effective_admin_create_user_config(
    requested: Option<AdminCreateUserConfigType>,
) -> AdminCreateUserConfigType {
    let mut config = requested.unwrap_or_default();
    if config.unused_account_validity_days == 0 {
        config.unused_account_validity_days = DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS;
    }
    config
}

/// The `EmailSendingAccount` a pool that asks for no email sending method gets:
/// "When Amazon Cognito emails your users, it uses its built-in email
/// functionality. When you use the default option ..."
/// (`EmailConfigurationType.EmailSendingAccount` in this service's model), as
/// against `DEVELOPER`, which sends through the caller's own Amazon SES
/// configuration. One of
/// [`crate::ops::types::EMAIL_SENDING_ACCOUNT_TYPE_VALUES`].
pub const DEFAULT_EMAIL_SENDING_ACCOUNT: &str = "COGNITO_DEFAULT";

/// The `IssuerConfiguration.Type` a pool that asks for none gets: "the ORIGINAL
/// issuer configuration for user pools. The issuer URL is hosted in the user
/// pool's region" (`IssuerConfigurationType.Type` in this service's model),
/// which is the issuer every pool's tokens carry until one is configured
/// otherwise. One of [`crate::ops::types::ISSUER_TYPE_VALUES`].
pub const DEFAULT_ISSUER_TYPE: &str = "ORIGINAL";

/// The `KeyConfiguration.KeyType` a pool that names no key gets: "a key owned by
/// Amazon Web Services in Key Management Service"
/// (`KeyConfigurationType.KeyType` in this service's model), the key a pool that
/// carries no `KmsKeyArn` is encrypted under. One of
/// [`crate::ops::types::ENCRYPTION_KEY_TYPE_VALUES`].
pub const DEFAULT_ENCRYPTION_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// The `KeyConfiguration.KeyType` of a pool encrypted under a key its own
/// account manages, which is the key type multi-Region replication needs
/// ([`encrypted_under_customer_managed_key`]). One of
/// [`crate::ops::types::ENCRYPTION_KEY_TYPE_VALUES`].
pub const CUSTOMER_MANAGED_KEY_TYPE: &str = "CUSTOMER_MANAGED_KEY";

/// What a pool that cannot be replicated because of the key it is encrypted
/// under is refused with.
///
/// `KeyConfigurationType.KeyType` documents the requirement on the key type that
/// admits replication — "CUSTOMER_MANAGED_KEY: a key managed by the customer in
/// Key Management Service. You must use a multi-region key to enable
/// multi-region replication for a user pool" (this service's model) — and the
/// sentence states the key the pool needs and the feature it needs it for,
/// naming nothing from the request or the pool.
pub const CUSTOMER_MANAGED_KEY_REQUIRED_MESSAGE: &str =
    "User pool needs a customer-managed KMS key for multi-region replication";

/// Whether `configuration` encrypts a pool under a customer-managed key, which
/// is the key type a multi-Region pool's data has to live under
/// ([`CUSTOMER_MANAGED_KEY_REQUIRED_MESSAGE`]).
///
/// Read off the STORED configuration, which always names a key type
/// ([`effective_key_configuration`]), so a pool that configured none is the
/// Amazon Web Services owned key it holds by default. A configuration naming a
/// `KmsKeyArn` is encrypted under that key whatever key type it names beside it:
/// "the Amazon Resource Name (ARN) of the KMS key used for encryption. If not
/// specified, Amazon Web Services managed keys are used"
/// (`KeyConfigurationType.KmsKeyArn` in this service's model), so the pool that
/// specifies one is the pool whose own key is in use.
#[must_use]
pub fn encrypted_under_customer_managed_key(configuration: &KeyConfigurationType) -> bool {
    configuration.key_type.as_deref() == Some(CUSTOMER_MANAGED_KEY_TYPE)
        || configuration
            .kms_key_arn
            .as_deref()
            .is_some_and(|arn| !arn.is_empty())
}

/// The `DefaultEmailOption` a pool that configures no verification message gets:
/// a verification email carries a code rather than a clickable link, which is
/// the option the code-bearing `{####}` template of
/// `VerificationMessageTemplateType.EmailMessage` is written for. One of
/// [`crate::ops::types::DEFAULT_EMAIL_OPTION_TYPE_VALUES`].
pub const DEFAULT_EMAIL_OPTION: &str = "CONFIRM_WITH_CODE";

/// The `LambdaConfig` a pool is STORED with, which is the one the request
/// carried or no trigger at all.
///
/// A pool always HAS a trigger configuration — every stage runs the function it
/// names, or none ([`crate::state::lambda_triggers`]) — so a request that
/// configures none is stored with an empty collection rather than with nothing,
/// and every read reports the configuration the pool actually applies.
pub fn effective_lambda_config(requested: Option<LambdaConfigType>) -> LambdaConfigType {
    requested.unwrap_or_default()
}

/// The `EmailConfiguration` a pool is STORED with: the request's, with the
/// sending method the pool applies filled in.
///
/// A pool always sends its messages one of the two ways
/// `EmailConfigurationType.EmailSendingAccount` names, so a configuration that
/// names neither is stored with [`DEFAULT_EMAIL_SENDING_ACCOUNT`], the built-in
/// one; a request that carries no configuration at all gets that same method
/// with nothing else set.
pub fn effective_email_configuration(
    requested: Option<EmailConfigurationType>,
) -> EmailConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.email_sending_account.is_none() {
        configuration.email_sending_account = Some(DEFAULT_EMAIL_SENDING_ACCOUNT.to_string());
    }
    configuration
}

/// The `IssuerConfiguration` a pool is STORED with: the request's, with
/// [`DEFAULT_ISSUER_TYPE`] filled in where it names no issuer type, since a
/// pool's tokens always carry one issuer.
pub fn effective_issuer_configuration(
    requested: Option<IssuerConfigurationType>,
) -> IssuerConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.r#type.is_none() {
        configuration.r#type = Some(DEFAULT_ISSUER_TYPE.to_string());
    }
    configuration
}

/// The `KeyConfiguration` a pool is STORED with: the request's, with
/// [`DEFAULT_ENCRYPTION_KEY_TYPE`] filled in where it names no key type, since a
/// pool's data is always encrypted under one key.
pub fn effective_key_configuration(
    requested: Option<KeyConfigurationType>,
) -> KeyConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.key_type.is_none() {
        configuration.key_type = Some(DEFAULT_ENCRYPTION_KEY_TYPE.to_string());
    }
    configuration
}

/// The `UserAttributeUpdateSettings` a pool is STORED with: the request's, with
/// an EMPTY list of attributes filled in where it names none.
///
/// The setting decides, for every attribute, whether a change waits on
/// verification ([`UserPool::requires_verification_before_update`]), so a pool
/// that configures none holds the empty collection — no attribute requires it —
/// rather than nothing.
pub fn effective_user_attribute_update_settings(
    requested: Option<UserAttributeUpdateSettingsType>,
) -> UserAttributeUpdateSettingsType {
    let mut settings = requested.unwrap_or_default();
    if settings
        .attributes_require_verification_before_update
        .is_none()
    {
        settings.attributes_require_verification_before_update = Some(Vec::new());
    }
    settings
}

/// The `VerificationMessageTemplate` a pool is STORED with: the request's, with
/// [`DEFAULT_EMAIL_OPTION`] filled in where it chooses no email option, since a
/// pool's verification emails always carry either a code or a link.
pub fn effective_verification_message_template(
    requested: Option<VerificationMessageTemplateType>,
) -> VerificationMessageTemplateType {
    let mut template = requested.unwrap_or_default();
    if template.default_email_option.is_none() {
        template.default_email_option = Some(DEFAULT_EMAIL_OPTION.to_string());
    }
    template
}

/// The `UserPoolAddOns` a pool is STORED with: `None` for a pool whose request
/// configures no threat protection at all, and otherwise the requested add-ons
/// with the mode for the additional authentication types filled in.
///
/// `AdvancedSecurityAdditionalFlows` holds the mode threat protection runs
/// custom authentication in, and that mode's enum admits no "off" value, so a
/// pool whose add-ons are configured runs them in
/// [`ADVANCED_SECURITY_MODE_AUDIT`] unless the request chose otherwise.
pub fn effective_user_pool_add_ons(
    requested: Option<UserPoolAddOnsType>,
) -> Option<UserPoolAddOnsType> {
    let mut add_ons = requested?;
    let mut flows = add_ons.advanced_security_additional_flows.unwrap_or_default();
    if flows.custom_auth_mode.is_none() {
        flows.custom_auth_mode = Some(ADVANCED_SECURITY_MODE_AUDIT.to_string());
    }
    add_ons.advanced_security_additional_flows = Some(flows);
    Some(add_ons)
}

/// The feature plan a request leaves unset: "Defaults to `ESSENTIALS`".
pub fn effective_user_pool_tier(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_USER_POOL_TIER.to_string())
}

/// The MFA mode a request leaves unset: MFA is `OFF`.
pub fn effective_mfa_configuration(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| MFA_OFF.to_string())
}

/// The deletion-protection mode a request leaves unset: `INACTIVE`.
pub fn effective_deletion_protection(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_DELETION_PROTECTION.to_string())
}

/// `PreventUserExistenceErrors` a request leaves unset: "Defaults to `LEGACY`".
pub fn effective_prevent_user_existence_errors(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_PREVENT_USER_EXISTENCE_ERRORS.to_string())
}

/// One of Amazon Cognito's standard (OIDC) user-pool attributes, as the default
/// schema declares it. `min`/`max` are the bounds the service stamps on that
/// attribute's values: `MinLength`/`MaxLength` for a `String` attribute and
/// `MinValue`/`MaxValue` for a `Number` one
/// (`smithy_query(query="shape",shape="SchemaAttributeType")`); `None` leaves
/// that bound out of the attribute's constraints.
#[derive(Debug, Clone, Copy)]
struct StandardAttribute {
    name: &'static str,
    data_type: &'static str,
    mutable: bool,
    required: bool,
    min: Option<&'static str>,
    max: Option<&'static str>,
}

/// Amazon Cognito's standard (OIDC) user-pool attributes, the schema every new
/// pool starts with. The names are the standard attributes of the Amazon Cognito
/// Developer Guide's "User pool attributes"; the fields are
/// `SchemaAttributeType` (`smithy_query(query="shape",shape="SchemaAttributeType")`).
///
/// Each attribute carries the service's OWN bounds rather than one generic
/// template: the minted identifier is at least one character long, the
/// date-of-birth attribute holds exactly a ten-character date, the number
/// attribute starts at zero, and every other string attribute takes up to the
/// 2048 characters an attribute value may be.
const STANDARD_SCHEMA: [StandardAttribute; 20] = [
    StandardAttribute { name: "sub", data_type: "String", mutable: false, required: true, min: Some("1"), max: Some("2048") },
    StandardAttribute { name: "name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "given_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "family_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "middle_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "nickname", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "preferred_username", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "profile", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "picture", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "website", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email_verified", data_type: "Boolean", mutable: true, required: false, min: None, max: None },
    StandardAttribute { name: "gender", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "birthdate", data_type: "String", mutable: true, required: false, min: Some("10"), max: Some("10") },
    StandardAttribute { name: "zoneinfo", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "locale", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number_verified", data_type: "Boolean", mutable: true, required: false, min: None, max: None },
    StandardAttribute { name: "address", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "updated_at", data_type: "Number", mutable: true, required: false, min: Some("0"), max: None },
];

/// The schema attributes the SERVICE itself maintains on every pool, beyond the
/// ones a create configures: the attribute that renders the external identities
/// a profile is linked to ([`IDENTITIES_ATTRIBUTE`], whose value this service
/// writes when a link is made, never a caller). Unbounded, so it reports the
/// empty string constraints of a string attribute with no bounds.
///
/// They belong to the pool from the moment it exists, so every READ of a pool
/// reports them ([`UserPool::reported_schema`]); the create that configured the
/// pool answers with the configured attributes alone
/// ([`UserPool::to_configured_user_pool_type`]).
const SERVICE_MAINTAINED_SCHEMA: [StandardAttribute; 1] = [StandardAttribute {
    name: IDENTITIES_ATTRIBUTE,
    data_type: "String",
    mutable: true,
    required: false,
    min: None,
    max: None,
}];

/// The schema entry `standard` declares.
fn standard_schema_attribute(standard: &StandardAttribute) -> SchemaAttributeType {
    let (number_attribute_constraints, string_attribute_constraints) =
        schema_attribute_constraints(standard.data_type, standard.min, standard.max);
    SchemaAttributeType {
        name: Some(standard.name.to_string()),
        attribute_data_type: Some(standard.data_type.to_string()),
        developer_only_attribute: Some(false),
        mutable: Some(standard.mutable),
        required: Some(standard.required),
        number_attribute_constraints,
        string_attribute_constraints,
    }
}

/// Whether `name` is one of the standard attributes a pool's schema starts
/// from, which a request names rather than adds.
pub fn is_standard_attribute(name: &str) -> bool {
    STANDARD_SCHEMA
        .iter()
        .any(|standard| standard.name == name)
}

/// The `AttributeDataType` values whose values a `String` bound describes, and
/// the one whose values a `Number` bound describes
/// (`smithy_query(query="shape",shape="AttributeDataType")`): an attribute of
/// any OTHER type carries neither constraints structure.
const STRING_ATTRIBUTE_DATA_TYPE: &str = "String";
const NUMBER_ATTRIBUTE_DATA_TYPE: &str = "Number";

/// The constraints structures a schema attribute of `data_type` reports, as
/// `(NumberAttributeConstraints, StringAttributeConstraints)`.
///
/// An attribute reports the structure that describes ITS data type and nothing
/// else: "specifies the constraints for an attribute of the number type" /
/// "... of the string type" (`SchemaAttributeType` in this service's model). The
/// structure is reported even where the attribute is unbounded — then as the
/// EMPTY structure — because it is the attribute's data type, not the presence
/// of a bound, that decides which constraints an attribute has; an attribute of
/// any other type (a boolean, a date) reports neither.
fn schema_attribute_constraints(
    data_type: &str,
    min: Option<&str>,
    max: Option<&str>,
) -> (
    Option<NumberAttributeConstraintsType>,
    Option<StringAttributeConstraintsType>,
) {
    match data_type {
        NUMBER_ATTRIBUTE_DATA_TYPE => (
            Some(NumberAttributeConstraintsType {
                min_value: min.map(str::to_string),
                max_value: max.map(str::to_string),
            }),
            None,
        ),
        STRING_ATTRIBUTE_DATA_TYPE => (
            None,
            Some(StringAttributeConstraintsType {
                min_length: min.map(str::to_string),
                max_length: max.map(str::to_string),
            }),
        ),
        _ => (None, None),
    }
}

/// The columns Amazon Cognito's user-import CSV carries BEYOND the pool's own
/// attributes, in the order the import format puts them: the MFA state of the
/// imported user, the username to create them under (Amazon Cognito Developer
/// Guide, "Importing users from a CSV file") and that user's password as a hash
/// — "the password hashing algorithm used to generate the hashes in the CSV file
/// for this import job" (`CreateUserImportJob.PasswordHashingAlgorithm` in this
/// service's model) is what an import job reads that last column with. They are
/// not attributes of the pool's schema, so they are not reported by any other
/// read of it.
const CSV_HEADER_TRAILING_COLUMNS: [&str; 3] = [
    "cognito:mfa_enabled",
    "cognito:username",
    "password_hash",
];

/// How many digits a minted verification code carries: Amazon Cognito's
/// verification messages carry a six-digit code.
const VERIFICATION_CODE_LENGTH: usize = 6;

/// How many digits a minted email one-time password carries: Amazon Cognito's
/// `EMAIL_OTP` sign-in challenge delivers an eight-digit one-time password.
const EMAIL_OTP_CODE_LENGTH: usize = 8;

/// A fresh verification code: [`VERIFICATION_CODE_LENGTH`] decimal digits.
pub fn new_verification_code() -> String {
    new_numeric_code(VERIFICATION_CODE_LENGTH)
}

/// A fresh email one-time password: [`EMAIL_OTP_CODE_LENGTH`] decimal digits.
pub fn new_email_otp_code() -> String {
    new_numeric_code(EMAIL_OTP_CODE_LENGTH)
}

/// A fresh code of `digits` decimal digits, which satisfies
/// `ConfirmationCodeType`'s modelled length 1..2048 and pattern `^[\S]+$` — the
/// shape the member that carries it back accepts.
///
/// The randomness is the same v4-UUID source every other minted value in this
/// file draws on ([`new_uuid`]), so nothing here is a hand-rolled generator: a
/// v4 UUID renders as 32 hex characters, each reduced to one decimal digit, so
/// one draw always yields the whole code and this makes no loop.
fn new_numeric_code(digits: usize) -> String {
    new_uuid()
        .chars()
        .filter_map(|character| character.to_digit(16))
        .filter_map(|value| char::from_digit(value % 10, 10))
        .take(digits)
        .collect()
}

/// The attribute that records whether `attribute`'s stored value was confirmed,
/// for the two attributes this API verifies (`VerifiedAttributeType`: `email`
/// and `phone_number`, whose flags are the standard `email_verified` and
/// `phone_number_verified`), or `None` for an attribute that carries no such
/// flag.
pub fn verified_flag(attribute: &str) -> Option<&'static str> {
    match attribute {
        "email" => Some("email_verified"),
        "phone_number" => Some("phone_number_verified"),
        _ => None,
    }
}

/// The attribute `value` has the SHAPE of a value of: an `email` holds an
/// address, a `phone_number` holds a number in E.164 form ("A phone number must
/// be in E.164 format", `AdminCreateUser.UserAttributes` in this service's
/// model, which is a `+` and digits), and a value that is neither belongs to no
/// addressing attribute.
///
/// Used to tell which sign-in attribute a submitted sign-in name is a value of
/// ([`UserPool::sign_in_attribute`]); it judges the shape only, never whether a
/// value is deliverable.
pub fn attribute_shape_of(value: &str) -> Option<&'static str> {
    if value.starts_with('+') {
        return Some("phone_number");
    }
    if value.contains('@') {
        return Some("email");
    }
    None
}

/// The value a verified flag carries once the attribute's value is confirmed.
/// `email_verified` and `phone_number_verified` are `Boolean` schema
/// attributes, whose values are carried as the strings every attribute value
/// is.
pub const FLAG_CONFIRMED: &str = "true";

/// The value a verified flag carries while the attribute's value is not
/// confirmed.
pub const FLAG_UNCONFIRMED: &str = "false";

/// Each `DeliveryMediumType` value beside the user attribute whose stored value
/// addresses a message sent over it: an email address is mailed and a phone
/// number is texted, so `EMAIL` and `SMS` — the values
/// [`crate::ops::types::DELIVERY_MEDIUM_TYPE_VALUES`] carries — name the two
/// attributes a message is ever addressed to.
///
/// The ONE place the two value sets are mapped to each other, so the medium a
/// delivery REPORTS and the attribute a delivery NEEDS cannot disagree.
pub const DELIVERY_MEDIUM_ATTRIBUTES: [(&str, &str); 2] =
    [("EMAIL", "email"), ("SMS", "phone_number")];

/// The `DeliveryMediumType` a verification message for `attribute` travels by —
/// an email address is mailed and a phone number is texted — or `None` for an
/// attribute no message is sent for.
pub fn delivery_medium(attribute: &str) -> Option<&'static str> {
    DELIVERY_MEDIUM_ATTRIBUTES
        .iter()
        .find(|(_, addressed_by)| *addressed_by == attribute)
        .map(|(medium, _)| *medium)
}

/// The message the refusal of a request for a verification code of an attribute
/// outside the verifiable set carries.
///
/// Only an attribute whose stored value ADDRESSES a message can have a code sent
/// to it ([`DELIVERY_MEDIUM_ATTRIBUTES`]), and the refusal states that permitted
/// set rather than the name the request asked for, so it is one fixed sentence
/// for every other attribute name.
pub const UNVERIFIABLE_ATTRIBUTE_MESSAGE: &str =
    "Invalid attribute name. Only phone_number and email can be verified.";

/// The user attribute whose stored value addresses a message delivered over
/// `medium` — the inverse of [`delivery_medium`] — or `None` for a value no
/// `DeliveryMediumType` names.
pub fn medium_destination_attribute(medium: &str) -> Option<&'static str> {
    DELIVERY_MEDIUM_ATTRIBUTES
        .iter()
        .find(|(named, _)| *named == medium)
        .map(|(_, addressed_by)| *addressed_by)
}

/// The first medium among `requested` that no address is held for, beside the
/// attribute that would have addressed it, or `None` when every requested
/// medium can be delivered over.
///
/// A message needs an address to go to, and the medium it travels over is what
/// names the attribute holding one ([`DELIVERY_MEDIUM_ATTRIBUTES`]): a request
/// that asks for delivery over a medium the recipient holds no value for asks
/// for a message that can never be addressed, which is a bad parameter rather
/// than a delivery that was attempted. `holds_destination` answers whether a
/// value for the attribute it is given is held.
///
/// A PURE function of the request and the lookup it is given — it reads and
/// writes no stored state itself — so a request refused by it leaves the store
/// exactly as it was. Each requested medium costs ONE call of
/// `holds_destination` and nothing is stored per entry, so a caller that
/// decides the addressing attributes once, ahead of this walk, keeps the whole
/// check linear in the request's size: the submitted medium list bounds nothing
/// that is stored and nothing that is re-scanned. A medium no
/// `DeliveryMediumType` names addresses no attribute and is passed over here:
/// the request walk has already refused a value outside the modelled set.
pub fn undeliverable_medium<F>(
    requested: &[String],
    holds_destination: F,
) -> Option<(&str, &'static str)>
where
    F: Fn(&str) -> bool,
{
    requested.iter().find_map(|medium| {
        let attribute = medium_destination_attribute(medium)?;
        (!holds_destination(attribute)).then_some((medium.as_str(), attribute))
    })
}

/// The message an `InvalidParameterException` carries for a request that asks
/// for delivery over `medium` where no `attribute` addresses it
/// ([`undeliverable_medium`]).
///
/// The refusal reports the medium the request asked for and the attribute that
/// medium needs — both of them values the caller submitted or the model names —
/// so it says what to send instead and discloses nothing stored.
///
/// Amazon Cognito states this condition as two clauses, the message it could not
/// send and the registration it has no record of, and closes neither with a
/// period: a resend with nothing to send to answers "Cannot resend codes. No
/// registered/verified email or phone_number" (the sentence
/// `ResendConfirmationCode` already carries in this crate). This is that
/// sentence for ONE medium and the ONE attribute addressing it, so a caller
/// matching the family matches both.
pub fn undeliverable_medium_message(medium: &str, attribute: &str) -> String {
    format!("Cannot send the message by {medium}. No registered {attribute}")
}

/// What one submitted attribute does to a stored user
/// ([`plan_attribute_update`]).
#[derive(Debug)]
pub struct AttributeUpdate {
    /// The attribute the request named.
    pub name: String,
    /// The value to store, or `None` to delete the attribute (a blank value).
    pub value: Option<String>,
    /// Whether the value is held pending the user's confirmation rather than
    /// stored over the confirmed one.
    pub deferred: bool,
    /// The verified flag this change leaves vouching for a value the user no
    /// longer holds, when the request does not set that flag itself: it goes
    /// unconfirmed beside a value that changed, and away with a value that was
    /// deleted, since a deleted attribute has nothing to vouch for.
    pub stale_verified_flag: Option<&'static str>,
}

/// The whole update: which stored user it applies to, what it does to that
/// user's attributes and the verification messages it sends.
#[derive(Debug)]
pub struct AttributeUpdatePlan {
    /// The pool holding the user this plan applies to.
    pub user_pool_id: String,
    /// The STORED username of that user, however the request named it.
    pub username: String,
    /// What the update does to that user's attributes, one entry per attribute
    /// it changes.
    pub updates: Vec<AttributeUpdate>,
    /// One `(attribute, value)` per verification message this update sends: the
    /// attribute the message is addressed to and the value it goes to, which is
    /// what the code issued for it is recorded against.
    pub verifications: Vec<(String, String)>,
    /// The OTHER users of the pool this update takes an alias value away from,
    /// as `(storage key, verified flag)`: the alias each held is now this
    /// user's, so the flag that made it theirs no longer vouches for anything
    /// ([`UserPool::apply_attribute_update`]).
    pub demotions: Vec<(String, &'static str)>,
}

/// Why a submitted attribute list is no update to make
/// ([`plan_attribute_update`]). The operation that asked maps each one to a
/// modelled error IT declares.
#[derive(Debug)]
pub enum AttributeUpdateRefusal {
    /// The list names no single storable update: the message says which rule it
    /// breaks, for a client to fix the request from, and names no stored value.
    Invalid(String),
    /// The submitted value of the named attribute is one another user of the
    /// pool already holds as its alias ([`UserPool::alias_taken`]) and which no
    /// verified flag can take away from that user, so the two would both answer
    /// to it.
    AliasTaken(String),
}

/// What a verified flag a request submits ITSELF says about the value beside
/// it.
///
/// `AdminUpdateUserAttributes.UserAttributes` documents that "to skip the
/// verification message and update the value of an attribute that requires
/// verification in the same API request, include the `email_verified` or
/// `phone_number_verified` attribute, with a value of `true`" and that Amazon
/// Cognito then "doesn't send a verification message to your user" (this
/// service's model), while the self-service `UpdateUserAttributes` documents no
/// such clause: the caller of that one is the user whose value would be
/// verified, so the flag it submits is not an assertion that it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedFlagAuthority {
    /// A submitted flag of [`FLAG_CONFIRMED`] asserts the value beside it is
    /// verified: that value is stored at once and no message is sent for it.
    Asserts,
    /// A submitted flag is stored as the value of a `Boolean` attribute and
    /// asserts nothing: the pool's own settings alone decide whether a new
    /// value waits for a confirmation.
    Records,
}

/// The message a request refused because it would change an attribute the
/// pool's schema does not let change carries.
///
/// `SchemaAttributeType.Mutable` is what "specifies whether the value of the
/// attribute can be changed" (this service's model), so an attribute it freezes
/// is one no request may write — storing a new value or taking the stored one
/// away alike, since both change it. Amazon Cognito reports that in one
/// sentence whose only variable part is the attribute the request named, and
/// names neither the pool nor the user: the schema, not this user's state, is
/// what refused it.
///
/// This is the sentence a DELETION of such an attribute carries. A write that
/// submits a VALUE for it is validated against the schema value by value, so it
/// is refused with that report's own line for the finding
/// ([`schema_validation_report`]) rather than with this sentence.
pub fn non_mutable_attribute_message(attribute_name: &str) -> String {
    format!("Cannot modify the non-mutable attribute {attribute_name}")
}

/// What Amazon Cognito's schema validation of a user's submitted attributes reports
/// about ONE attribute it will not write.
///
/// A write that submits name/value pairs is validated against the pool's schema
/// attribute by attribute, and each finding is reported as its own line: the
/// attribute, under the `user.` prefix the schema gives a user's own attributes,
/// then what is wrong with it, then the newline every line of that report ends
/// with. [`schema_validation_report`] is the report those lines make up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaValidationFinding {
    /// The pool's schema declares no such attribute, so there is nothing to write.
    NotInSchema,
    /// The pool's schema declares the attribute immutable
    /// (`SchemaAttributeType.Mutable`), so its stored value cannot change.
    NotUpdatable,
}

impl SchemaValidationFinding {
    /// The sentence this finding is reported with.
    fn sentence(self) -> &'static str {
        match self {
            Self::NotInSchema => "Attribute does not exist in the schema.",
            Self::NotUpdatable => "Attribute cannot be updated.",
        }
    }
}

/// The report Amazon Cognito's schema validation states for `attributes`, each with the
/// finding it earned: one line per attribute, in the order they are handed over, each
/// ending in its own newline.
///
/// Empty when `attributes` is empty, which is a request that broke nothing.
pub fn schema_validation_report<'a>(
    attributes: impl IntoIterator<Item = (&'a str, SchemaValidationFinding)>,
) -> String {
    attributes
        .into_iter()
        .map(|(name, finding)| format!("user.{name}: {}\n", finding.sentence()))
        .collect()
}

/// The message a request refused for naming an attribute whose type the pool's schema
/// does not declare carries.
///
/// An attribute DELETION names attributes without values
/// (`AttributeNameListType`), so each name's declared type is what says what is being
/// taken away, and a name the schema declares nothing for has no type to resolve at all.
/// Amazon Cognito reports that rather than a schema-validation line, since no value was
/// validated: the attribute is named in braces, as the report names a name it could not
/// resolve.
pub fn undetermined_attribute_type_message(attribute_name: &str) -> String {
    format!(
        "Invalid user attributes: Type for attribute {{{attribute_name}}} could not be determined"
    )
}

/// A new user pool with `name` in `region`, with every configuration default
/// filled in and `signing_keys` as the key pairs its tokens are signed with. The
/// caller stores it under [`UserPool::id`].
///
/// The keys are passed IN rather than minted here: generating them is
/// milliseconds of CPU per key ([`PoolSigningKeys::generate`]), so the caller
/// runs that on the blocking pool and only after the store's own bound has
/// admitted the pool.
///
/// The pool's SAML signing certificate IS issued here, once its id is settled:
/// the certificate is issued to the SAML entity id that id implies, and the key
/// pair it is signed with is an elliptic-curve one, generated in microseconds
/// rather than in the milliseconds an RSA key pair costs.
pub fn new_user_pool(
    name: String,
    region: &str,
    now: SystemTime,
    signing_keys: PoolSigningKeys,
) -> UserPool {
    let region = sanitize_region(region);
    let id = new_pool_id(&region);
    let arn = ::engine::emulator_arn(SIGNING_NAME, &region, &format!("userpool/{id}"));
    let mut pool = UserPool {
        id,
        name,
        arn,
        region,
        creation_date: now,
        last_modified_date: now,
        policies: effective_policies(None),
        lambda_config: effective_lambda_config(None),
        mfa_configuration: effective_mfa_configuration(None),
        deletion_protection: effective_deletion_protection(None),
        user_pool_tier: DEFAULT_USER_POOL_TIER.to_string(),
        username_attributes: Vec::new(),
        alias_attributes: Vec::new(),
        auto_verified_attributes: Vec::new(),
        schema: effective_schema(None),
        admin_create_user_config: effective_admin_create_user_config(None),
        username_configuration: None,
        device_configuration: None,
        email_configuration: effective_email_configuration(None),
        sms_configuration: None,
        verification_message_template: effective_verification_message_template(None),
        account_recovery_setting: None,
        user_attribute_update_settings: effective_user_attribute_update_settings(None),
        user_pool_add_ons: None,
        key_configuration: effective_key_configuration(None),
        issuer_configuration: effective_issuer_configuration(None),
        acr_configuration: None,
        sms_authentication_message: None,
        software_token_mfa_configuration: None,
        email_mfa_configuration: None,
        web_authn_configuration: None,
        log_configurations: Vec::new(),
        tags: HashMap::new(),
        domain: None,
        custom_domain: None,
        replicas: BTreeMap::new(),
        clients: HashMap::new(),
        users: HashMap::new(),
        groups: BTreeMap::new(),
        resource_servers: BTreeMap::new(),
        identity_providers: BTreeMap::new(),
        terms: BTreeMap::new(),
        managed_login_branding: BTreeMap::new(),
        ui_customizations: BTreeMap::new(),
        user_import_jobs: BTreeMap::new(),
        // A pool's token-signing keys exist from the moment the pool does, as on
        // AWS, so its key set is publishable before anything signs in: one key
        // pair per token type, minted by the caller before the pool is built and
        // never again.
        signing_keys,
        // Issued below, once the pool's id — the name the certificate is issued
        // to — is settled.
        saml_signing_certificate: None,
        device_group_key: new_device_group_key(),
        risk_configuration: None,
    };
    // A pool's SAML signing certificate exists from the moment the pool does, as
    // on AWS: `GetSigningCertificate` answers one for a pool nothing has
    // federated yet, and the pool keeps the certificate it was issued.
    pool.issue_saml_signing_certificate();
    pool
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. A pool's id and ARN name the region it was created
/// in, so they are derived from the request's scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| sanitize_region(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `region`, reduced to the characters a `UserPoolId` may carry
/// (`^[\w-]+_[0-9a-zA-Z]+$`), falling back to [`DEFAULT_REGION`].
fn sanitize_region(region: &str) -> String {
    let cleaned: String = region
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        cleaned
    }
}

/// A fresh `UserPoolId`: `<region>_<9 random characters>`, which satisfies the
/// modelled pattern `^[\w-]+_[0-9a-zA-Z]+$` and length 1..55.
fn new_pool_id(region: &str) -> String {
    format!("{region}_{}", random_token(9))
}

/// Whether `region` can stand in the Region field of an ARN, and in the `^[\w-]+`
/// half of a `UserPoolId`.
fn is_region_like(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `user_pool_id` could be a `UserPoolId` at all: the modelled pattern
/// `^[\w-]+_[0-9a-zA-Z]+$` and length 1..[`USER_POOL_ID_MAX_LENGTH`]
/// (`smithy_query(query="shape",shape="UserPoolIdType")`). The separator is the
/// LAST underscore, because `\w` admits one in the first half too.
pub fn is_user_pool_id_shaped(user_pool_id: &str) -> bool {
    if user_pool_id.is_empty() || user_pool_id.len() > USER_POOL_ID_MAX_LENGTH {
        return false;
    }
    let Some((region, suffix)) = user_pool_id.rsplit_once('_') else {
        return false;
    };
    is_region_like(region)
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The Region and the `UserPoolId` the user-pool ARN `arn` names, or `None` when
/// `arn` names no user pool of this service at all — another ARN namespace or
/// resource type, a malformed partition, Region or account, or a resource id no
/// `UserPoolId` could be.
///
/// Only the ARN's FORM is read here. Whether this service holds that pool in that
/// Region is the caller's lookup, which is what separates a `ResourceArn` that
/// names no user pool (a bad parameter) from one naming a pool that is not stored
/// (a missing resource).
pub fn user_pool_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(namespace),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    let partition_is_named = !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    // An AWS account id is twelve digits.
    let account_is_an_id = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    if namespace != SIGNING_NAME
        || !partition_is_named
        || !is_region_like(region)
        || !account_is_an_id
    {
        return None;
    }
    let user_pool_id = resource
        .strip_prefix(USER_POOL_RESOURCE_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|user_pool_id| is_user_pool_id_shaped(user_pool_id))?;
    Some((region, user_pool_id))
}

/// A fresh CloudFront endpoint for a custom domain: `<distribution>.cloudfront.net`,
/// the form AWS reports as "The fully-qualified domain name (FQDN) of the Amazon
/// CloudFront distribution" (`CreateUserPoolDomain.CloudFrontDomain`) and the
/// value a caller's alias record has to point at. A CloudFront distribution
/// domain is `d` followed by 13 lowercase alphanumerics.
fn new_cloud_front_domain() -> String {
    format!("d{}.cloudfront.net", random_token(13))
}

/// A fresh `JobId` for a user import job: `import-` followed by a fixed-width
/// token of [`USER_IMPORT_JOB_TOKEN_LEN`] mixed-case alphanumerics, the shape
/// Amazon Cognito's own job ids carry (`"JobId": "import-TZqNQvDRnW"` in the API
/// reference's examples for this service's import-job operations), which
/// satisfies the modelled pattern `^import-[0-9a-zA-Z-]+$` and length 1..55.
pub fn new_user_import_job_id() -> String {
    format!(
        "import-{}",
        random_alphanumeric_token(USER_IMPORT_JOB_TOKEN_LEN)
    )
}

/// How many characters the token of a minted [`new_user_import_job_id`] carries.
const USER_IMPORT_JOB_TOKEN_LEN: usize = 10;

/// Whether `job_id` could be a `JobId` at all: the modelled pattern
/// `^import-[0-9a-zA-Z-]+$` and length 1..55
/// (`smithy_query(query="shape",shape="UserImportJobIdType")`). Asked by the
/// non-modelled upload route, so a path of some other shape is not mistaken for
/// one of this service's CSV upload targets.
#[must_use]
pub fn is_user_import_job_id_shaped(job_id: &str) -> bool {
    let Some(suffix) = job_id.strip_prefix("import-") else {
        return false;
    };
    job_id.len() <= 55
        && !suffix.is_empty()
        && suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The first path segment of the locator a user import job's CSV is uploaded to.
/// Shared by the locator [`user_import_csv_upload_url`] hands out and by the
/// route that accepts the upload (`crate::service`), so the two cannot drift.
pub const USER_IMPORT_CSV_ROUTE: &str = "user-import-jobs";

/// Where a client uploads the CSV file of the import job `job_id`:
/// `UserImportJobType.PreSignedUrl`, "The pre-signed URL target for uploading
/// the CSV file".
///
/// AWS answers an Amazon S3 pre-signed URL here. This emulator provisions no
/// object store, so the URL names THIS job's CSV on the endpoint THIS REQUEST
/// ARRIVED AT, and this service accepts the upload on that path itself (see
/// `crate::service`) — a URL a client can really write the file to, rather than
/// one pointing at the real service. A request that names no host of its own gets
/// no locator at all rather than a baked-in one, which would point a following
/// client's upload at the real service.
///
/// `host` is taken only when it LOOKS like a `host[:port]`, so a caller cannot
/// get a string of its own into the value this service reports.
pub fn user_import_csv_upload_url(host: &str, user_pool_id: &str, job_id: &str) -> Option<String> {
    is_host_shaped(host)
        .then(|| format!("https://{host}/{USER_IMPORT_CSV_ROUTE}/{user_pool_id}/{job_id}"))
}

/// The path segment under a user pool's level that the classic hosted UI serves
/// its logo image on. Shared by the locator [`hosted_logo_url`] hands out and by
/// the route that serves it (`crate::service`), so the two cannot drift.
pub const HOSTED_LOGO_FILE: &str = "assets/login_logo.png";

/// Where a client fetches the logo image stored for `client_id` of
/// `user_pool_id`: `UICustomizationType.ImageUrl`, "A URL path to the hosted
/// logo image of your UI customization".
///
/// AWS serves it from the content delivery network that fronts the classic
/// hosted UI. This emulator provisions none, so the locator names the image on
/// the endpoint THIS REQUEST ARRIVED AT, which this service serves itself (see
/// `crate::service`) — a URL a caller can fetch the stored bytes from, rather
/// than one pointing at the real service. A request that names no host of its
/// own gets no locator at all.
///
/// `host` is taken only when it LOOKS like a `host[:port]`, so a caller cannot
/// get a string of its own into the value this service reports.
pub fn hosted_logo_url(host: &str, user_pool_id: &str, client_id: &str) -> Option<String> {
    is_host_shaped(host)
        .then(|| format!("https://{host}/{user_pool_id}/{client_id}/{HOSTED_LOGO_FILE}"))
}

/// The endpoint host this request addressed — its `Host` header, or the
/// authority of the request URI when it carries one — which is the endpoint a
/// locator an operation hands out has to name. `None` when the request names no
/// endpoint of its own.
pub fn request_host(ctx: &smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    ctx.parts()
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            ctx.parts()
                .uri
                .authority()
                .map(::http::uri::Authority::as_str)
        })
        .map(ToString::to_string)
}

/// Whether `host` is a non-empty `host[:port]` of the characters a hostname
/// carries.
fn is_host_shaped(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':'))
}

/// A fresh `ClientId`: 26 characters, satisfying the modelled `^[\w+]+$`.
pub fn new_client_id() -> String {
    random_token(26)
}

/// A fresh `ClientSecret`: 51 characters, inside the modelled length 24..64 and
/// satisfying `^[\w+]+$`.
pub fn new_client_secret() -> String {
    random_token(51)
}

/// The message a `SecretHash` authenticates: "username plus the client ID in
/// the message" (`SignUp.SecretHash` in this service's model), in that order, so
/// the hash of one username is not the hash of another and the hash for one app
/// client is not the hash for another.
fn secret_hash_message(username: &str, client_id: &str) -> String {
    format!("{username}{client_id}")
}

/// A fresh user `sub`: a v4 UUID, as Amazon Cognito's `sub` attribute is.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()`
/// rendered canonically — `vendor/emulator-telemetry/src/types.rs`), which this
/// crate already depends on: nothing here is a hand-rolled generator and this
/// crate declares no second path to the same crate.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A fresh `Session` token: 64 characters, inside the modelled length 20..4096.
fn new_session_token() -> String {
    random_token(64)
}

/// A fresh passkey registration challenge: the value the authenticator signs
/// over, carried base64url as the creation options encode one (W3C Web
/// Authentication Level 2 §5.4, `PublicKeyCredentialCreationOptions.challenge`,
/// a `BufferSource` the JSON form base64url-encodes).
///
/// 32 characters of the same v4-UUID randomness every other minted value in this
/// file draws on ([`random_token`]), so nothing here is a hand-rolled generator.
fn new_web_authn_challenge() -> String {
    base64_url(random_token(32).as_bytes())
}

/// A fresh `DeviceKey` for a device of a pool in `region`: `<region>_<v4 UUID>`,
/// which satisfies the modelled pattern `^[\w-]+_[0-9a-f-]+$` — `region` carries
/// only the characters [`sanitize_region`] admits, and a v4 UUID is lowercase
/// hex and hyphens.
///
/// The prefix is clamped to [`MAX_DEVICE_KEY_PREFIX`] so the key stays inside
/// the modelled length 1..55 whatever region the pool was created in: the
/// region comes from the caller's credential scope, so its length is not this
/// service's to assume.
fn new_device_key(region: &str) -> String {
    let prefix = region
        .char_indices()
        .take_while(|(at, _)| *at < MAX_DEVICE_KEY_PREFIX)
        .map(|(_, c)| c)
        .collect::<String>();
    format!("{prefix}_{}", new_uuid())
}

/// How many characters of the region a minted `DeviceKey` carries ahead of its
/// separator: `DeviceKeyType`'s modelled length is 1..55 and the UUID after the
/// separator is 36 characters long.
const MAX_DEVICE_KEY_PREFIX: usize = 55 - 1 - 36;

/// A fresh `DeviceGroupKey` for a user pool: a v4 UUID, the shape Amazon Cognito
/// issues this identifier in, inside the modelled length 0..131072.
fn new_device_group_key() -> String {
    new_uuid()
}

/// The bytes `text` carries, read as base64url (RFC 4648 §5) — the encoding the
/// JSON form of a WebAuthn `BufferSource` member uses — or `None` when `text` is
/// not a base64 encoding of anything.
///
/// The inverse of [`base64_url`]: the URL alphabet's `-` and `_` are mapped back
/// to the standard alphabet's `+` and `/`, and the padding [`base64_url`] trims
/// is restored, so a value encoded with padding and one encoded without both
/// decode. Decoding itself is the codec this workspace already uses for every
/// Smithy blob (`aws_smithy_types::base64`).
pub fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut standard = text.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(standard).ok()
}

/// `bytes` in base64url without padding, as a JWS segment is encoded
/// (RFC 7515 §2): the standard-alphabet codec this workspace already uses for
/// every Smithy blob (`aws_smithy_types::base64`), with `+` and `/` replaced by
/// `-` and `_` and the padding removed.
pub fn base64_url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// `time` as the modelled timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        time.duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default(),
    )
}

/// `time` as whole seconds since the epoch, for a token claim.
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `time` as whole milliseconds since the epoch, the unit a rendered linked
/// identity reports its creation instant in.
fn unix_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `Some(values)` when `values` is non-empty, else `None`: an absent list is
/// absent on the wire rather than an empty one.
pub fn non_empty<T: Clone>(values: &[T]) -> Option<Vec<T>> {
    (!values.is_empty()).then(|| values.to_vec())
}

// ---------------------------------------------------------------------------
// Provisioned API rate limits
//
// An account holds one request-rate limit per API category: the rate the
// category includes at no additional cost, and the provisioned rate in force,
// which the account may set anywhere from that included rate up to its
// account-level maximum. These limits belong to the ACCOUNT rather than to a
// user pool, so they live beside the pools rather than inside one.
// ---------------------------------------------------------------------------

/// The one `LimitClass` this service models, which is the class of the
/// request-rate limits: "For API rate limits, this is `API_CATEGORY`"
/// (`LimitDefinitionType.LimitClass` in this service's model, whose only
/// modelled value is [`crate::ops::types::LIMIT_CLASS_VALUES`]).
pub const API_CATEGORY_LIMIT_CLASS: &str = "API_CATEGORY";

/// The attribute that identifies WHICH API rate limit a definition means:
/// "For API rate limits, specify the `Category` key with a value like
/// `UserAuthentication` or `UserCreation`"
/// (`LimitDefinitionType.Attributes` in this service's model).
pub const API_CATEGORY_ATTRIBUTE: &str = "Category";

/// One API category an account holds a request-rate limit for.
#[derive(Debug)]
pub struct ApiCategory {
    /// The `Category` attribute value that names it.
    pub name: &'static str,
    /// The rate this category includes at no additional cost, in requests per
    /// second: `LimitType.FreeLimitValue` is "the default (free) limit value,
    /// in requests per second (RPS) … the rate included at no additional cost"
    /// (this service's model), which is the category's published request-rate
    /// quota (Amazon Cognito Developer Guide, "Quotas in Amazon Cognito"). It
    /// is the rate in force until an adjustment provisions more, and the lowest
    /// rate the limit can be provisioned at.
    pub free_rate: i32,
}

/// The API categories an account holds a request-rate limit for, with the rate
/// each one includes at no additional cost. A definition naming no category
/// here identifies no limit this account holds.
///
/// The names are the ones this service's model documents for the `Category`
/// attribute (`LimitDefinitionType.Attributes` names `UserAuthentication` and
/// `UserCreation`, `UpdateProvisionedLimit` also `UserFederation`), beside the
/// remaining request-rate categories of user pool operations, whose published
/// per-category rates are the included rates here (Amazon Cognito Developer
/// Guide, "Quotas in Amazon Cognito").
///
/// Every limit is provisionable from the rate it includes up to the account's
/// Service Quotas ceiling for it
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::api_category_quota_ceiling`]).
/// The upper bound is the account's rather than the category's: a quota
/// increase moves it, and nothing in this API does.
pub const API_CATEGORIES: &[ApiCategory] = &[
    ApiCategory {
        name: "UserAuthentication",
        free_rate: 120,
    },
    ApiCategory {
        name: "UserFederation",
        free_rate: 25,
    },
    ApiCategory {
        name: "UserCreation",
        free_rate: 50,
    },
    ApiCategory {
        name: "UserRead",
        free_rate: 120,
    },
    ApiCategory {
        name: "UserUpdate",
        free_rate: 25,
    },
    ApiCategory {
        name: "UserAccountRecovery",
        free_rate: 30,
    },
];

/// Why a limit definition identifies no limit of this service.
///
/// Every one of these is a defect of the DEFINITION the request carried — a
/// class, an attribute or a value that identifies no limit this service
/// publishes — rather than a limit the account is missing, so each is reported
/// as a bad parameter by both operations that resolve a definition. Both
/// operations resolve one the same way, so both render these; a rate refusal is
/// [`RateRefusal`], which only the operation that requests a rate can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitLookupFailure {
    /// The definition names a limit class this service publishes no limits of.
    UnsupportedClass(String),
    /// The definition carries none of the attributes that identify which limit
    /// of its class is meant; the attribute it has to carry is named.
    MissingAttribute(&'static str),
    /// The definition's attributes name no limit of its class.
    UnknownCategory(String),
}

impl LimitLookupFailure {
    /// What a refusal tells the caller, which is the same sentence whichever
    /// operation was refused: the caller has to correct the same definition
    /// from it either way.
    ///
    /// A value the definition carried that names nothing is reported as
    /// UNKNOWN, with the class it was looked up under and the value itself —
    /// the two things the caller has to change — and nothing about the account.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedClass(class) => {
                format!("Unknown limit class: {class}")
            }
            Self::MissingAttribute(attribute) => format!(
                "The limit definition must include the attribute {attribute} to identify \
                 the {API_CATEGORY_LIMIT_CLASS} limit it means."
            ),
            Self::UnknownCategory(category) => {
                format!("Unknown {API_CATEGORY_LIMIT_CLASS} category: {category}")
            }
        }
    }
}

/// Why a requested rate is not one this account can provision for a limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateRefusal {
    /// The requested rate is below the rate the limit includes, which is the
    /// lowest rate it can hold.
    BelowFreeRate { requested: i32, free_rate: i32 },
    /// The requested rate is above the account's Service Quotas ceiling for
    /// this limit, so provisioning it would need a quota increase first.
    AboveQuotaCeiling { requested: i32, ceiling: i32 },
    /// The requested rate is the one a previous provisioning stored for this
    /// limit, so there is no adjustment to make.
    AlreadyProvisioned,
}

impl RateRefusal {
    /// What a refusal tells the caller.
    ///
    /// A rate outside the account's range names the requested rate and the
    /// bound it missed, because `UpdateProvisionedLimit` documents the range as
    /// "the value must be between the default limit and your account-level
    /// maximum limit in Service Quotas" (this service's model) and neither
    /// bound is in the request.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::BelowFreeRate {
                requested,
                free_rate,
            } => format!(
                "The requested limit value {requested} is below the default limit of \
                 {free_rate} requests per second for this limit."
            ),
            // The quota excess states the rate that was asked for and the
            // ceiling it passed, and nothing else: the caller raises the
            // ceiling in Service Quotas rather than in this API, so there is no
            // step for this sentence to name.
            Self::AboveQuotaCeiling { requested, ceiling } => {
                format!("Requested value {requested} exceeds Service Quota ceiling of {ceiling}")
            }
            // A rate a previous provisioning already stored names no
            // adjustment, so the sentence reports that the request asks for the
            // limit in force and states no rate: both halves — what was asked
            // for and what is provisioned — are the same value the caller sent.
            Self::AlreadyProvisioned => {
                "Requested value is the same as the current provisioned limit".to_string()
            }
        }
    }
}

/// The API category `definition` identifies.
///
/// A definition identifies a limit by its class and the attributes of that
/// class: the request-rate limits are the `API_CATEGORY` class, and the
/// `Category` attribute is what names one of them. Nothing about the account's
/// state takes part, so both operations resolve a definition the same way.
pub fn api_category(
    definition: &LimitDefinitionType,
) -> Result<&'static ApiCategory, LimitLookupFailure> {
    if definition.limit_class != API_CATEGORY_LIMIT_CLASS {
        return Err(LimitLookupFailure::UnsupportedClass(
            definition.limit_class.clone(),
        ));
    }
    let named = definition
        .attributes
        .get(API_CATEGORY_ATTRIBUTE)
        .ok_or(LimitLookupFailure::MissingAttribute(API_CATEGORY_ATTRIBUTE))?;
    API_CATEGORIES
        .iter()
        .find(|candidate| candidate.name == named)
        .ok_or_else(|| LimitLookupFailure::UnknownCategory(named.clone()))
}

/// The definition that identifies `category`'s limit, as every reader of that
/// limit renders it: the class of the request-rate limits and the one attribute
/// that names the category. Built from the RESOLVED category rather than echoed
/// from a request, so one limit is identified the same way whatever a caller
/// sent.
#[must_use]
pub fn api_category_definition(category: &ApiCategory) -> LimitDefinitionType {
    LimitDefinitionType {
        limit_class: API_CATEGORY_LIMIT_CLASS.to_string(),
        attributes: [(
            API_CATEGORY_ATTRIBUTE.to_string(),
            category.name.to_string(),
        )]
        .into_iter()
        .collect(),
    }
}

/// The account's Service Quotas ceiling for `category`'s request-rate limit,
/// read off the account's quota state
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::api_category_quota_ceiling`]).
///
/// `UpdateProvisionedLimit`: "The value must be between the default limit and
/// your account-level maximum limit in Service Quotas" (this service's model).
/// That maximum is a Service Quotas value of the ACCOUNT, raised only by a
/// quota increase request — "to increase these limits, submit a Service Quotas
/// increase request" (the same doc) — and no operation of this service submits
/// or grants one. So a category whose quota nothing has raised holds the value
/// Service Quotas starts it at, the category's own published request rate,
/// which is also the rate the limit reports as its default
/// (`LimitType.FreeLimitValue`). A rate above the ceiling needs that increase
/// before it can be provisioned, which is what `ServiceQuotaExceededException`
/// reports.
#[must_use]
pub fn default_api_category_quota_ceiling(category: &ApiCategory) -> i32 {
    category.free_rate
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The limit this account holds for `category`.
    ///
    /// A PURE read: the provisioned rate is the one a previous update stored,
    /// and a limit nothing has adjusted reports the rate the category includes
    /// at no additional cost — the rate Amazon Cognito enforces until the
    /// account provisions more.
    #[must_use]
    pub fn api_category_limit(&self, category: &ApiCategory) -> LimitType {
        LimitType {
            limit_definition: api_category_definition(category),
            provisioned_limit_value: self
                .provisioned_api_rates
                .get(category.name)
                .copied()
                .unwrap_or(category.free_rate),
            free_limit_value: category.free_rate,
        }
    }

    /// The account's Service Quotas ceiling for `category`'s request-rate
    /// limit, in requests per second: the highest rate this account may
    /// provision for it.
    ///
    /// Read off the account's quota state
    /// ([`Self::api_category_quota_ceilings`](ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState)),
    /// which holds an entry only for a category whose quota an increase has
    /// raised; every other category stands at
    /// [`default_api_category_quota_ceiling`].
    #[must_use]
    pub fn api_category_quota_ceiling(&self, category: &ApiCategory) -> i32 {
        self.api_category_quota_ceilings
            .get(category.name)
            .copied()
            .unwrap_or_else(|| default_api_category_quota_ceiling(category))
    }

    /// Record a GRANTED Service Quotas increase for `category`'s request-rate
    /// limit, raising the highest rate this account may provision for it to
    /// `ceiling`.
    ///
    /// What moves this ceiling is a Service Quotas increase request — "to
    /// increase these limits, submit a Service Quotas increase request"
    /// (`UpdateProvisionedLimit` in this service's model) — which belongs to
    /// Service Quotas rather than to this API, so no operation of this service
    /// calls this: the account's quota is state this service READS when it
    /// decides a requested rate. It never LOWERS a ceiling, and never below
    /// the rate the category includes, which is the floor of the range
    /// whatever the account's quota says.
    pub fn raise_api_category_quota_ceiling(&mut self, category: &ApiCategory, ceiling: i32) {
        let raised = ceiling
            .max(self.api_category_quota_ceiling(category))
            .max(category.free_rate);
        self.api_category_quota_ceilings
            .insert(category.name.to_string(), raised);
    }

    /// Provision `requested` requests per second for `category` and return the
    /// limit as it now stands.
    ///
    /// Every rate in the account's range is stored but the rate this account has
    /// already PROVISIONED: the range runs from the rate the category includes
    /// up to the account's Service Quotas ceiling for that limit
    /// ([`Self::api_category_quota_ceiling`]), a rate outside it is refused, and
    /// so is the rate a previous provisioning stored — "use this operation to
    /// adjust" a limit (`UpdateProvisionedLimit` in this service's model), and a
    /// rate equal to the one provisioned adjusts nothing. A limit NOTHING has
    /// provisioned holds no such rate, so the first provisioning of any rate in
    /// the range is an adjustment, the rate the limit reports until then being
    /// the one the category includes rather than one this account provisioned.
    /// A refused call stores nothing, so the limit a later read reports is the
    /// one that was already in force.
    pub fn set_api_category_limit(
        &mut self,
        category: &ApiCategory,
        requested: i32,
    ) -> Result<LimitType, RateRefusal> {
        if requested < category.free_rate {
            return Err(RateRefusal::BelowFreeRate {
                requested,
                free_rate: category.free_rate,
            });
        }
        let ceiling = self.api_category_quota_ceiling(category);
        if requested > ceiling {
            return Err(RateRefusal::AboveQuotaCeiling { requested, ceiling });
        }
        if self.provisioned_api_rates.get(category.name) == Some(&requested) {
            return Err(RateRefusal::AlreadyProvisioned);
        }
        self.provisioned_api_rates
            .insert(category.name.to_string(), requested);
        Ok(self.api_category_limit(category))
    }
}

// ---------------------------------------------------------------------------
// Pagination
//
// Every paginated listing of this service pages the same way: over a stable
// ascending order of keys, with an OPAQUE `NextToken` naming the key the next
// page resumes strictly after.
// ---------------------------------------------------------------------------

/// The most items one page of a paginated listing holds, and what a request
/// that asks for no page size gets: `QueryLimitType`'s modelled range is 0..60
/// (this service's model, e.g. `AdminListGroupsForUser.Limit`), so 60 is the
/// most a caller can ask for.
pub const MAX_QUERY_LIMIT: i32 = 60;

/// Marks a `NextToken` as one THIS service minted, so a token it never minted
/// is refused on its prefix rather than on whether its bytes happen to decode.
const PAGE_TOKEN_PREFIX: &str = "cognito-idp-page-1:";

/// Separates a page token's listing scope from the key it resumes after. ASCII
/// unit separator: `PaginationKey`'s pattern is `^[\S]+$`, but the token is
/// base64-encoded before it reaches the wire, so the separator only has to be a
/// byte no scope carries.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// A `NextToken` that resumes `scope`'s listing strictly after `last_key`.
///
/// `scope` names the listing the token belongs to — the operation plus the
/// resource whose items it pages — so a token one listing issued names no page
/// of another. The token is base64-encoded with `aws_smithy_types::base64`, the
/// codec the wire protocols already carry, which keeps it opaque to the caller
/// and free of the whitespace `PaginationKey`'s pattern `^[\S]+$` excludes.
#[must_use]
pub fn encode_page_token(scope: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{PAGE_TOKEN_PREFIX}{scope}{PAGE_TOKEN_SEPARATOR}{last_key}"
    ))
}

/// The key `token` resumes `scope`'s listing after, or `None` when `token` is
/// not a token this service minted for that listing.
#[must_use]
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let (token_scope, last_key) = body.split_once(PAGE_TOKEN_SEPARATOR)?;
    (token_scope == scope).then(|| last_key.to_string())
}

/// One page of a paginated listing: the keys it serves and the `NextToken` the
/// page after it resumes at, absent when this page is the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub keys: Vec<String>,
    pub next_token: Option<String>,
}

/// The message an `InvalidParameterException` carries for a pagination token that
/// names no page of the listing it was presented to.
///
/// Amazon Cognito reports a token it cannot resume a listing from in FIXED text
/// that names neither the token nor the resource the listing was over: a token is
/// a value the caller holds already and the pool it belonged to is not something
/// the refusal discloses. It does not word that sentence identically in every list
/// API — the group listing answers
/// [`PAGINATION_TOKEN_INVALID_RETRY_MESSAGE`] — so an operation names the sentence
/// ITS OWN API answers, and this is the one the rest of them answer.
pub const PAGINATION_TOKEN_INVALID_MESSAGE: &str = "Invalid pagination token given.";

/// The same refusal as the group listing (`ListGroups`) words it: the sentence that
/// also tells the caller what to do about it. See
/// [`PAGINATION_TOKEN_INVALID_MESSAGE`].
pub const PAGINATION_TOKEN_INVALID_RETRY_MESSAGE: &str =
    "Provided pagination token is invalid. Check the token and try your request again.";

/// How many items a listing serves for `limit`.
///
/// `QueryLimitType`'s modelled range (0..60) is refused at the boundary before
/// a handler runs, so the cap here never changes a request the framework
/// accepted. A request that asks for no limit takes the modelled maximum, and
/// so does one that asks for `0`: a page that may hold no item terminates no
/// pagination loop, and this service's model already reads a submitted `0` as
/// "no value" elsewhere (`PasswordPolicyType.TemporaryPasswordValidityDays`).
#[must_use]
pub fn page_size(limit: Option<i32>) -> usize {
    let maximum = MAX_QUERY_LIMIT as usize;
    match limit {
        Some(limit) if limit > 0 => (limit as usize).min(maximum),
        _ => maximum,
    }
}

/// The page of `keys` that `limit` and `next_token` ask for in `scope`'s
/// listing, or `None` when `next_token` is not a token this listing minted —
/// which the calling operation refuses with its own modelled error.
///
/// `keys` must be in the listing's stable ASCENDING order; the page resumes at
/// the first key strictly after the one the token names, so a key deleted
/// between two pages neither repeats nor skips the keys around it.
#[must_use]
pub fn page_of(
    scope: &str,
    keys: &[String],
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let after = decode_page_token(scope, token)?;
            keys.partition_point(|key| key <= &after)
        }
    };
    let end = start.saturating_add(page_size(limit)).min(keys.len());
    let served = keys.get(start..end).unwrap_or_default().to_vec();
    let next_token = match served.last() {
        Some(last) if end < keys.len() => Some(encode_page_token(scope, last)),
        _ => None,
    };
    Some(Page {
        keys: served,
        next_token,
    })
}

/// Whether `c` counts as a symbol for a password policy: anything that is
/// neither a letter nor a digit.
fn is_symbol(c: char) -> bool {
    !c.is_alphanumeric()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>)
}
