// SPDX-License-Identifier: CC0-1.0

//! @op GetCredentialsForIdentity
//! Issues the credentials of an `sts:AssumeRoleWithWebIdentity` session of the pool's
//! resolved role, a FRESH session on every call.
//!
//! ## One session per call
//!
//! Every served vend mints its own session: the `Credentials` answered carry an
//! `AccessKeyId`, `SecretKey` and `SessionToken` triple this call alone was handed, and no
//! call is answered with a session an earlier one was given. The exchange is the assume it
//! is passed through as — "If the token is for cognito-identity.amazonaws.com, it will be
//! passed through to Security Token Service with the appropriate role for the token" (this
//! operation's own doc) — and an assume mints a session, so a caller that asks again gets
//! another one, which is what lets it pick up a change in what the role may do and what
//! makes each session separately attributable where a call it signs is recorded. A session
//! handed out earlier is untouched by the next vend: it is the token service's, valid for
//! its own window, so this service keeps no per-identity record of it and revokes nothing
//! by vending again. Minting per call is bounded where the sessions live: the token
//! service's registry holds at most its own stated number of sessions that have not expired
//! and gives up the expired ones to make room, so a vend it cannot record is refused rather
//! than answered with a credential nothing resolves.
//!
//! ## Which role the session stands for
//!
//! The credentials a real identity pool vends are a session of the ROLE the pool's own
//! configuration resolves for the request, written through `SetIdentityPoolRoles`, and
//! `CustomRoleArn` picks one of the roles the exchange's identity provider offered instead.
//! Where the pool carries a `RoleMappings` entry for the identity provider the presented
//! login was made under, that mapping is what resolves the role — for a rules mapping, the
//! role the first rule to match the presented token's claims names, and for a token mapping
//! the role the token's own group claims name — and a mapping that resolves none states, in
//! its `AmbiguousRoleResolution`, what becomes of that exchange: the pool's authenticated
//! role, or a refusal that vends nothing at all. The pool-wide authenticated or
//! unauthenticated role is the fallback for an exchange no mapping of the pool's decides.
//! This handler resolves that role and assumes it: the session it mints is
//! recorded in the STS session registry in the same engine, so
//! `sts:GetCallerIdentity` under the vended credential answers
//! `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`, and the
//! assume itself is recorded in the account's management-event log as STS's own
//! `AssumeRoleWithWebIdentity` event.
//!
//! The assume happens only where the role admits it. A pool that names no role for the
//! request, a role the account does not hold, and a role whose trust policy does not admit
//! an assume by THIS pool's users — the `cognito-identity.amazonaws.com:aud` condition that
//! keeps one identity pool out of another pool's role — are each answered with the modelled
//! `InvalidIdentityPoolConfigurationException` (HTTP 400), the error the real service
//! raises when it has no role it can assume for the request, rather than with a session
//! that stands for nothing. A TAGGED assume — one for a pool that maps principal tags for
//! the provider the login was made under — needs one grant more: "To pass session tags, the
//! principal must have the `sts:TagSession` permission" (`AssumeRole$Tags`, the STS model),
//! so a role whose trust policy admits only the assume cannot be assumed with tags and the
//! vend is answered the same configuration refusal. A role mapping that DENIES the exchange
//! is a different refusal: the pool's configuration is not at fault, the token presented is
//! simply not authorized to receive a credential, so that one is the modelled
//! `NotAuthorizedException` (HTTP 403). A `CustomRoleArn` naming a role the exchange's own
//! identity provider did not offer — a role absent from the presented token's role claims,
//! the pool's own default roles among them — is that same authorization refusal, and it
//! names the role the request asked for.
//!
//! `Logins` selects WHICH flow the exchange is: a request that presents a login is an
//! authenticated one, and the pool's `authenticated` role is the one assumed where no
//! mapping of the pool's decides the login it presents.
//!
//! ## What the session is scoped to
//!
//! This flow makes the assume itself, so it is this flow that passes the SESSION POLICIES the
//! session it hands back carries: the credentials a GUEST exchange — one presenting no login
//! at all — vends an anonymous caller reach only the services an identity pool limits a guest
//! to, so an action outside that scope is refused by the service that receives the call even
//! where the unauthenticated role's own documents allow it
//! ([`crate::state::vended_session_scope`]). The basic flow passes none of this service's
//! session policies, the assume being its caller's own
//! (`sts:AssumeRoleWithWebIdentity`), so a session minted that way is capped by the role's
//! documents alone.
//!
//! The authentication methods the trust-policy decision is made with — the
//! `cognito-identity.amazonaws.com:amr` condition values — come from the LOGIN rather than
//! from the name it is presented under: a token this service minted names the providers
//! that authenticated the identity, as its own issuance recorded them, so a
//! developer-authenticated login is admitted by a role conditioned on the pool's developer
//! provider and a login of another provider is not.
//!
//! ## The login the exchange is made with
//!
//! An entry presented under this service's own provider name carries a token THIS service
//! minted FOR one identity, so both its validity window and the identity it stands for are
//! ones the service recorded and can read: such a token is a time-bounded credential of
//! that identity, and it is honoured only inside that window and only for that identity.
//! The window it is read against is the recorded one extended by the clock-skew grace the
//! `TokenDuration` documentation asks a caller to allow for
//! (`crate::state::PRESENTED_TOKEN_CLOCK_SKEW_GRACE`), so a token just minted for the
//! shortest window the model admits is still spendable and a caller whose clock runs ahead
//! of this service's is not refused for that.
//!
//! Both are read before any role is resolved and before any session is minted or handed
//! back, and a request presenting a token whose window has closed, one this service cannot
//! resolve at all, or one minted for another identity — the token of another identity pool
//! among them, each identity belonging to the pool that minted it — is answered with the
//! modelled `NotAuthorizedException` (HTTP 403) instead of a credential. A token of any
//! other provider is decided by the provider that issued it, which this emulator does not
//! reach.
//!
//! An entry presented under this service's own provider name is honoured only as a token
//! THIS service minted, so a value no issuance of this service resolves buys no credential,
//! and WHICH refusal it carries follows from what the value itself claims to be: a value of
//! this service's own token form is the unverifiable token it claims to be, while a value
//! that claims this service as its issuer in no way — a developer user identifier, an opaque
//! credential of some other provider, a token of another issuer's form — is no Cognito token
//! at all, the refusal a token another identity pool minted carries as well.
//!
//! Presenting such an issuance READS it and never consumes it: its window is the whole of
//! what bounds it, so a caller that exchanges the same token again while that window is open
//! is served again, and the token is refused only once the window has closed — as the expired
//! token it then is. The mint hands out a credential its caller may keep for exactly that
//! long: "You can specify a custom expiration time for the token so that you can cache it"
//! (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentityInput",member="TokenDuration")`),
//! and a cached token is a token presented more than once.
//!
//! A DEVELOPER provider is no provider a login can be redeemed against here: it is "the
//! 'domain' by which Cognito will refer to your users", this account's own back end rather
//! than an identity provider, and it issues no token — a developer-authenticated identity
//! reaches this exchange with the token `GetOpenIdTokenForDeveloperIdentity` minted for it,
//! presented under THIS service's own provider name. So an entry keyed by such a name carries
//! a developer user identifier where a token has to be, which is no Cognito token either,
//! whichever identity the pool has registered the identifier under. WHICH names those are is
//! the classification every reading of a presented `Logins` map makes
//! ([`crate::state::developer_provider_logins`]): the pool's own `DeveloperProviderName` and
//! every other name this service cannot say is an identity PROVIDER's.
//!
//! Every presented
//! login is decided BEFORE the request's role-selection parameters are looked at, so a
//! request whose login does not stand up is answered for the login, whatever else it asked
//! for.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetCredentialsForIdentity::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetCredentialsForIdentity`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json")
//! ```
//!
//! Returns credentials for the provided identity ID. Any provided logins will be validated
//! against supported login providers. If the token is for cognito-identity.amazonaws.com,
//! it will be passed through to Security Token Service with the appropriate role for the
//! token. This is a public API. You do not need any credentials to call this API.
//!
//! ## Input
//!
//! * `CustomRoleArn` (ARNString, len 20..2048) — The Amazon Resource Name (ARN) of the role
//!   to be assumed when multiple roles were received in the token from the identity provider.
//!   For example, a SAML-based identity provider. This parameter is optional for identity
//!   providers that do not support role customization.
//! * `IdentityId` (IdentityId, len 1..55, pattern ^[\w-]+:[0-9a-f-]+$) **required** — A
//!   unique identifier in the format REGION:GUID.
//! * `Logins` (LoginsMap, len 0..10) — A set of optional name-value pairs that map provider
//!   names to provider tokens. The name-value pair will follow the syntax "provider_name":
//!   "provider_user_identifier". Logins should not be specified when trying to get
//!   credentials for an unauthenticated identity. The Logins parameter is required when using
//!   identities associated with external identity providers such as Facebook. For examples of
//!   Logins maps, see the code examples in the External Identity Providers section of the
//!   Amazon Cognito Developer Guide.
//!
//! ## Output
//!
//! * `Credentials` (Credentials) — Credentials for the provided identity ID.
//! * `IdentityId` (IdentityId, len 1..55, pattern ^[\w-]+:[0-9a-f-]+$) — A unique identifier
//!   in the format REGION:GUID.
//!
//! ## Errors
//!
//! * `ExternalServiceException` (HTTP 400) — An exception thrown when a dependent service
//!   such as Facebook or Twitter is not responding
//! * `InternalErrorException` (HTTP 500) — Thrown when the service encounters an error during
//!   processing the request.
//! * `InvalidIdentityPoolConfigurationException` (HTTP 400) — If you provided authentication
//!   information in the request, the identity pool has no authenticated role configured, or
//!   STS returned an error response to the request to assume the authenticated role from the
//!   identity pool. If you provided no authentication information in the request, the
//!   identity pool has no unauthenticated role configured, or STS returned an error response
//!   to the request to assume the unauthenticated role from the identity pool. Your role
//!   trust policy must grant AssumeRoleWithWebIdentity permissions to
//!   cognito-identity.amazonaws.com.
//! * `InvalidParameterException` (HTTP 400) — Thrown for missing or bad input parameter(s).
//! * `NotAuthorizedException` (HTTP 403) — Thrown when a user is not authorized to access the
//!   requested resource.
//! * `ResourceConflictException` (HTTP 409) — Thrown when a user tries to use a login which
//!   is already linked to another account.
//! * `ResourceNotFoundException` (HTTP 404) — Thrown when the requested resource (for
//!   example, a dataset or record) does not exist.
//! * `TooManyRequestsException` (HTTP 429) — Thrown when a request is throttled.

#[allow(unused_imports)]
use super::types::*;

/// How long the credentials this operation issues stay valid. The Amazon Cognito
/// Developer Guide's "Authentication flow" section states the credentials an identity
/// pool vends "are valid for one hour"
/// (<https://docs.aws.amazon.com/cognito/latest/developerguide/authentication-flow.html>).
const CREDENTIAL_VALIDITY: ::std::time::Duration = ::std::time::Duration::from_secs(3600);

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetCredentialsForIdentityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetCredentialsForIdentityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cognitoidentity#GetCredentialsForIdentity")]
#[derive(Debug)]
pub enum GetCredentialsForIdentityErrorModeled {
    ExternalServiceException(ExternalServiceException),
    InternalErrorException(InternalErrorException),
    InvalidIdentityPoolConfigurationException(InvalidIdentityPoolConfigurationException),
    InvalidParameterException(InvalidParameterException),
    NotAuthorizedException(NotAuthorizedException),
    ResourceConflictException(ResourceConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_credentials_for_identity::GetCredentialsForIdentityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetCredentialsForIdentityError = crate::scaffolded::Scaffolded<GetCredentialsForIdentityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetCredentialsForIdentityErrorModeled> for GetCredentialsForIdentityError {
    fn from(modeled: GetCredentialsForIdentityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCredentialsForIdentity` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetCredentialsForIdentity<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCredentialsForIdentity<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCredentialsForIdentity<Eng>
{
    type Input = GetCredentialsForIdentityInput;
    type Output = GetCredentialsForIdentityOutput;
    type Error = GetCredentialsForIdentityError;

    const NAME: &'static str = "GetCredentialsForIdentity";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CREDENTIALS_FOR_IDENTITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CREDENTIALS_FOR_IDENTITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // This call writes NO management event of this service
        // ([`crate::state::record_served_call`]): it is one of the identity data-plane
        // operations, which CloudTrail delivers as a DATA event of the pool it acts on
        // instead ([`crate::state::data_events`]). The assume the served vend makes IS a
        // management event — STS's own — and is recorded where it happens.
        //
        // The session the served call answers is a credential, so the record names the
        // credential — its access key id, its session token and its expiry, which is what
        // lets a reader tell which session acted later in the trail — and withholds the one
        // member no record carries, the SECRET KEY
        // ([`crate::state::data_events::recorded_credentials`]).
        let region = crate::state::request_region(ctx.parts());
        let request_parameters = ::service_common::management_events::recorded_members([
            ("identityId", ::serde_json::json!(input.identity_id)),
            (
                "logins",
                ::serde_json::json!(crate::state::data_events::recorded_login_values(
                    input.logins.as_ref()
                )),
            ),
            ("customRoleArn", ::serde_json::json!(input.custom_role_arn)),
        ]);
        let outcome = self.vend(&input, ctx).await;
        crate::state::data_events::hold_data_event(
            &*self.engine,
            ctx.parts(),
            &crate::state::data_events::ServedDataCall {
                operation: Self::NAME,
                region: &region,
                // The call MINTS a session for the identity, which is not a report of state.
                read_only: false,
                identity_pool_id: crate::state::identity_pool_of(
                    &*self.engine,
                    &input.identity_id,
                ),
                request_parameters,
                response_elements: outcome.as_ref().ok().map(|output| {
                    ::service_common::management_events::recorded_members([
                        ("identityId", ::serde_json::json!(output.identity_id)),
                        (
                            "credentials",
                            ::serde_json::json!(crate::state::data_events::recorded_credentials(
                                output.credentials.as_ref().map(|credentials| {
                                    crate::state::data_events::VendedCredential {
                                        access_key_id: credentials.access_key_id.as_deref(),
                                        session_token: credentials.session_token.as_deref(),
                                        expiration: credentials.expiration.as_ref(),
                                    }
                                })
                            )),
                        ),
                    ])
                }),
            },
            &outcome,
        );
        outcome
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> GetCredentialsForIdentity<Eng> {
    /// The vend itself: resolve the pool's role, assume it, and answer the session.
    async fn vend(
        &self,
        input: &GetCredentialsForIdentityInput,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<GetCredentialsForIdentityOutput, GetCredentialsForIdentityError> {
        use crate::state::{MappedRole, VendedAssume, INVALID_POOL_CONFIGURATION_MESSAGE};
        use ::std::collections::HashMap;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetCredentialsForIdentityErrorModeled::InternalErrorException(
                InternalErrorException {
                    message: Some("the identity-pool store is unavailable".to_string()),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        // Which issuers the identity's pool trusts is partly IAM's record — an
        // `OpenIdConnectProviderARNs` entry names an IAM OpenID Connect provider — so the
        // account's providers are read BEFORE this service's own store is locked, and only
        // for a vend that presents a login at all.
        let registered = if input.logins.as_ref().is_none_or(HashMap::is_empty) {
            crate::state::RegisteredOidcProviders::default()
        } else {
            crate::state::registered_oidc_providers(&*self.engine)
        };
        // What an integrated user pool says about a user-pool token this vend presents is
        // that service's record, so it too is read before this service's store is locked
        // ([`crate::state::user_pool_logins`]).
        let integrated = match input.logins.as_ref() {
            Some(logins) => crate::state::user_pool_logins(&*self.engine, logins),
            None => crate::state::IntegratedUserPools::default(),
        };
        // The pool's own configuration is read FIRST, and only the facts this vend needs
        // are copied out, so the identity-pool store is not held while the IAM and STS
        // stores in the same engine are read.
        let resolved = {
            let store = state.lock();

            let Some(identity) = store.identities.get(&input.identity_id) else {
                return Err(GetCredentialsForIdentityErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::identity_not_found_message(
                            &input.identity_id,
                        )),
                    },
                )
                .into());
            };
            let identity_pool_id = identity.identity_pool_id.clone();
            let Some(pool) = store.identity_pools.get(&identity_pool_id) else {
                return Err(GetCredentialsForIdentityErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!("IdentityPool '{identity_pool_id}' not found.")),
                    },
                )
                .into());
            };
            let pool_admits_unauthenticated = pool.allow_unauthenticated_identities;

            // "Any provided logins will be validated against supported login providers"
            // (this operation's own doc): a `Logins` entry carries its issuer in the
            // provider name, so the stored trust the identity's OWN pool states for that
            // issuer decides it — a `CognitoIdentityProviders` entry, or an
            // `OpenIdConnectProviderARNs` entry that still resolves to an IAM OIDC
            // provider — together with the audience that trust accepts, and the token the
            // entry presents is decided before either.
            let logins = input.logins.clone().unwrap_or_default();
            if !logins.is_empty() {
                if let Some(refusal) = pool.refuse_logins(&registered, &integrated, &logins) {
                    return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                        NotAuthorizedException {
                            message: Some(refusal.message(&identity_pool_id).into_owned()),
                        },
                    )
                    .into());
                }

                // A `Logins` entry keyed by the pool's own `DeveloperProviderName` carries a
                // developer user identifier where this exchange needs a TOKEN, so it is no
                // Cognito token at all and buys no credential — the token a
                // developer-authenticated identity reaches this exchange with is the one its
                // own mint handed back, presented under THIS service's provider name
                // ([`crate::state::IdentityPool::refuse_developer_provider_login`]).
                if let Some(refusal) = pool.refuse_developer_provider_login(&logins) {
                    return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                        NotAuthorizedException {
                            message: Some(refusal.message().to_string()),
                        },
                    )
                    .into());
                }

                // A `Logins` entry made under THIS service's own provider name carries a
                // token this service minted — "If the token is for
                // cognito-identity.amazonaws.com, it will be passed through to Security
                // Token Service with the appropriate role for the token" (this operation's
                // own doc) — and such a token is a time-bounded credential minted FOR one
                // identity: it buys an exchange only inside the validity window its
                // issuance recorded, and only for the identity that issuance names. Both
                // are read HERE, before any role is resolved and before any session is
                // minted or handed back, so a token whose window the clock has closed, a
                // token this service cannot resolve at all, and a token minted for another
                // identity — the token of another identity pool among them, each identity
                // belonging to the pool that minted it — are refused rather than vended
                // for. WHICH refusal such a value carries is decided by what the value
                // itself claims to be, not by this call
                // ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::refuse_presented_login_tokens`]).
                if let Some(refusal) =
                    store.refuse_presented_login_tokens(&input.identity_id, &logins, now)
                {
                    return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                        NotAuthorizedException {
                            message: Some(refusal.message().to_string()),
                        },
                    )
                    .into());
                }
            }

            // An identity a login has been linked to is a federated user's, and the
            // credentials this operation vends are that user's: what authorizes the vend is
            // therefore a LOGIN of the identity it names — one the pool resolves to it, or a
            // token this service minted for it, which is what still reaches an identity an
            // unlink or a merge left linked to no provider. A vend presenting none at all,
            // and one presenting only some other user's login, have authorized the vend for
            // that identity in no way: the first is refused access to the identity it named,
            // the second the login mismatch it is, and both before any role is resolved.
            //
            // A vend presenting no login at all IS the unauthenticated exchange, and only a
            // pool whose `AllowUnauthenticatedIdentities` admits one serves it — "identities
            // from this identity pool and the associated IAM roles can only be used with
            // authenticated identity providers" otherwise
            // (`CreateIdentityPoolInput$AllowUnauthenticatedIdentities`, this service's
            // model) — so for a pool that does not, no identity of it is reached that way,
            // and the refusal says so in the words that case has rather than in the words of
            // an identity the caller may not use.
            if let Some(refusal) = store.refuse_identity_access(identity, &logins, now) {
                return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                    NotAuthorizedException {
                        message: Some(refusal.message(&input.identity_id)),
                    },
                )
                .into());
            }
            if logins.is_empty() && !pool_admits_unauthenticated {
                return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                    NotAuthorizedException {
                        message: Some(crate::state::UNAUTHENTICATED_ACCESS_MESSAGE.to_string()),
                    },
                )
                .into());
            }
            let providers: Vec<String> = logins.keys().cloned().collect();
            // The methods the exchange is made under, which is what the trust-policy
            // decision below publishes: a token this service minted carries the providers
            // that authenticated the identity in the issuance it was recorded with, so a
            // developer-authenticated login and a user-pool login presented under the same
            // provider name are told apart by the policy that conditions on them.
            let methods = store.exchange_authentication_methods(&logins, now);

            // The session tags this exchange asks its assume to attach: the principal tags
            // the pool maps for the providers the presented logins were made under, taken
            // from those tokens' own claims
            // ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::requested_session_tags`]).
            // They are what the assumed role's trust policy decides with
            // `aws:RequestTag/<key>`, and what the minted session then carries.
            let requested_tags = store.requested_session_tags(&identity_pool_id, &logins);

            // Which role this vend assumes: the `CustomRoleArn` the request names, which
            // can only be honoured among the roles this exchange's own identity provider
            // OFFERED it — the roles a presented token states, and, where no token states
            // any, the roles the pool has stored — or, for a request that names none, the
            // role the pool's own configuration resolves for this exchange. A `RoleMappings`
            // entry for the identity provider the login was made under is what resolves it
            // there, and the pool-wide role for the flow the request is in is the fallback
            // when no mapping of the pool's resolves one.
            // A mapping that DECIDES the exchange without resolving a role is the case its
            // `AmbiguousRoleResolution` states the outcome of: `AuthenticatedRole` names the
            // pool's own authenticated role, while `Deny` is a decision to vend NOTHING —
            // an authorization decision about the token presented, so the exchange is
            // refused with `NotAuthorizedException` rather than falling back to a role the
            // mapping refused.
            let configured = store.identity_pool_roles.get(&identity_pool_id);
            // Naming a `CustomRoleArn` at all is something only an exchange whose identity
            // provider offered SEVERAL roles may do — "Only SAML providers and providers
            // with RoleMappings support custom role ARN"
            // ([`crate::state::custom_role_arn_is_supported`]) — so an exchange of any other
            // provider is refused the bad parameter it sent, before the roles that exchange
            // could have resolved are considered at all. The logins the exchange presents
            // are decided BEFORE this: whether the request may name a role at all is a
            // question about a request whose login stands up, so a login that does not is
            // answered for the login rather than for the parameter.
            if input.custom_role_arn.is_some()
                && !crate::state::custom_role_arn_is_supported(pool, configured, &logins)
            {
                return Err(GetCredentialsForIdentityErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(
                            crate::state::CUSTOM_ROLE_ARN_UNSUPPORTED_MESSAGE.to_string(),
                        ),
                    },
                )
                .into());
            }
            let role_arn = match (&input.custom_role_arn, configured) {
                // A role the exchange's own identity provider did not offer it is a role
                // this caller may not assume, which is an authorization decision about the
                // token presented rather than a fault of the pool's configuration: the
                // refusal is `NotAuthorizedException` (HTTP 403) and it NAMES the role the
                // request asked for, so a caller holding several candidates is told which
                // one was refused
                // ([`crate::state::IdentityPoolRoles::admits_custom_role_arn`]). It is
                // decided before the pool's `RoleMappings` are consulted: a request that
                // names a role asks for THAT role, so what a mapping would have resolved —
                // and what its `AmbiguousRoleResolution` states for resolving none — is not
                // what such a request is answered about.
                // A pool that stores NO role configuration at all is the other case
                // entirely: it has no role for any exchange, which is what
                // `InvalidIdentityPoolConfigurationException` states, so such a request
                // falls to that refusal below rather than being told about this one's
                // claim.
                (Some(_), None) => None,
                (Some(custom_role_arn), Some(offer)) => {
                    if !offer.admits_custom_role_arn(&logins, custom_role_arn) {
                        return Err(GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                            NotAuthorizedException {
                                message: Some(
                                    crate::state::custom_role_arn_not_in_token_message(
                                        custom_role_arn,
                                    ),
                                ),
                            },
                        )
                        .into());
                    }
                    Some(custom_role_arn.clone())
                }
                (None, _) => {
                    let mapped = configured
                        .zip(store.identity_pools.get(&identity_pool_id))
                        .map(|(configured, pool)| configured.mapped_role(pool, &logins))
                        .unwrap_or(MappedRole::Unmapped);
                    match mapped {
                        MappedRole::Denied(provider) => {
                            return Err(
                                GetCredentialsForIdentityErrorModeled::NotAuthorizedException(
                                    NotAuthorizedException {
                                        message: Some(
                                            crate::state::denied_role_resolution_message(
                                                &provider,
                                            ),
                                        ),
                                    },
                                )
                                .into(),
                            )
                        }
                        MappedRole::Resolved(role_arn) => Some(role_arn),
                        MappedRole::AuthenticatedRole => configured
                            .and_then(|configured| configured.role_for(true))
                            .map(ToString::to_string),
                        MappedRole::Unmapped => configured
                            .and_then(|configured| configured.role_for(!providers.is_empty()))
                            .map(ToString::to_string),
                    }
                }
            };
            let Some(role_arn) = role_arn else {
                return Err(
                    GetCredentialsForIdentityErrorModeled::InvalidIdentityPoolConfigurationException(
                        InvalidIdentityPoolConfigurationException {
                            message: Some(INVALID_POOL_CONFIGURATION_MESSAGE.to_string()),
                        },
                    )
                    .into(),
                );
            };

            ResolvedVend {
                identity_pool_id,
                methods,
                role_arn,
                requested_tags,
                unauthenticated: logins.is_empty(),
            }
        };

        // Nothing is assumed that the account does not hold and no trust policy admits:
        // the role the pool names, and its trust policy's grant to the Cognito Identity
        // principal for THIS pool, are both read before a credential exists.
        let refused = || -> GetCredentialsForIdentityError {
            GetCredentialsForIdentityErrorModeled::InvalidIdentityPoolConfigurationException(
                InvalidIdentityPoolConfigurationException {
                    message: Some(INVALID_POOL_CONFIGURATION_MESSAGE.to_string()),
                },
            )
            .into()
        };
        let Some(role) = crate::state::resolve_role(&*self.engine, &resolved.role_arn) else {
            return Err(refused());
        };
        // The web identity this vend exchanges: the condition keys the assume publishes
        // about it, which the trust-policy decision below is taken over, and the federation
        // a record of a call made with the vended credential names — one value, so the two
        // cannot name two different identities.
        let web_identity = crate::state::vended_web_identity(
            &resolved.identity_pool_id,
            &input.identity_id,
            &resolved.methods,
        );
        // The assume this vend makes is a TAGGED one where the pool maps principal tags for
        // the exchange's provider, so the trust-policy decision is taken over the keys the
        // tag request publishes beside the exchange's own
        // ([`crate::state::session_tags`]): a trust policy gated on
        // `aws:RequestTag/<key>` admits the identities whose mapped value satisfies it and
        // refuses the others, which is the pool-configuration refusal below.
        let mut assume_context = web_identity.claims.clone();
        assume_context.extend(crate::state::session_tags::requested_session_tag_conditions(
            &resolved.requested_tags,
        ));
        if !crate::state::role_admits_web_identity(&role, &assume_context) {
            return Err(refused());
        }
        // Attaching those tags is a SEPARATE grant from the assume: "To pass session tags,
        // the principal must have the `sts:TagSession` permission" (`AssumeRole$Tags`, the
        // STS model), so a role whose trust policy admits only the web-identity assume
        // cannot be assumed WITH tags — the assume STS is asked for is refused, which is
        // the "STS returned an error response to the request to assume the … role from the
        // identity pool" case `InvalidIdentityPoolConfigurationException` states. Asked
        // only of an exchange that really asks for a tag: a vend whose pool maps none
        // attaches none and needs no grant to pass one.
        if !resolved.requested_tags.is_empty()
            && !crate::state::role_admits_session_tags(&role, &assume_context)
        {
            return Err(refused());
        }

        // The session itself: minted HERE, for THIS call, so the triple the caller is
        // handed is one no earlier vend was given. Nothing of an earlier session is read or
        // given back — it stays valid in the token service's registry for its own window.
        //
        // It is minted SCOPED: this flow performs the assume, so it is this flow that passes
        // the session policies the session carries, and the credentials a GUEST exchange
        // hands an anonymous caller reach only what an identity pool limits a guest to
        // ([`crate::state::vended_session_scope`]). The basic flow
        // (`GetOpenIdToken` and then `sts:AssumeRoleWithWebIdentity`) passes none of this
        // service's, so a session minted that way stays capped by the role's own documents
        // alone.
        let vended = crate::state::mint_vended_role_session(
            &*self.engine,
            ctx.credential_store(),
            &role,
            now,
            CREDENTIAL_VALIDITY,
            &crate::state::VendedSessionRequest {
                web_identity: &web_identity,
                tags: &resolved.requested_tags,
                policies: crate::state::vended_session_scope::vended_session_policies(
                    resolved.unauthenticated,
                ),
            },
        );
        // A vend that could not be recorded is refused rather than answered with a
        // credential no later call can resolve.
        let vended = match vended {
            Ok(vended) => vended,
            Err(why) => {
                return Err(GetCredentialsForIdentityErrorModeled::InternalErrorException(
                    InternalErrorException { message: Some(why) },
                )
                .into())
            }
        };
        let duration_seconds = i32::try_from(CREDENTIAL_VALIDITY.as_secs())
            .unwrap_or(i32::MAX)
            .min(role.max_session_duration);
        crate::state::record_vended_assume(
            &*self.engine,
            ctx.parts(),
            &VendedAssume {
                role: &role,
                session: &vended,
                identity_pool_id: &resolved.identity_pool_id,
                identity_id: &input.identity_id,
                // The Region the vend was served in is the POOL's own, which its
                // `REGION:GUID` id carries: `GetCredentialsForIdentity` is modelled
                // `@optionalAuth` and a client calls it with no credential scope at
                // all, so the request itself names no Region.
                region: crate::state::region_of(&resolved.identity_pool_id),
                duration_seconds,
            },
        );

        Ok(GetCredentialsForIdentityOutput {
            identity_id: Some(input.identity_id.clone()),
            credentials: Some(Credentials {
                access_key_id: Some(vended.access_key_id),
                secret_key: Some(vended.secret_key),
                session_token: Some(vended.session_token),
                expiration: Some(::aws_smithy_types::DateTime::from(vended.expiration)),
            }),
        })
    }
}

/// What the pool's stored configuration decides about one vend, read out of the
/// identity-pool store before the role it names is resolved against IAM.
struct ResolvedVend {
    /// The pool the identity belongs to, which is the `aud` the assume is made for.
    identity_pool_id: String,
    /// The authentication methods the exchange was made under, which the assume publishes
    /// as its `...:amr` condition values.
    methods: Vec<String>,
    /// The ARN of the role this vend assumes.
    role_arn: String,
    /// The session tags the assume asks to attach, in tag-key order: the principal tags the
    /// pool maps for the providers the presented logins were made under.
    requested_tags: Vec<(String, String)>,
    /// Whether the exchange presented NO login at all, which is what makes it the
    /// unauthenticated — guest — flow: the one this service scopes the session it vends
    /// down ([`crate::state::vended_session_scope`]).
    unauthenticated: bool,
}
