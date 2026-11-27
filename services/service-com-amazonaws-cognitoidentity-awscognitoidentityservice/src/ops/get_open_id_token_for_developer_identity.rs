// SPDX-License-Identifier: CC0-1.0

//! @op GetOpenIdTokenForDeveloperIdentity
//! Registers (or retrieves) the identity a BACKEND-authenticated user has in a stored
//! pool, and mints an OpenID Connect token for it.
//!
//! The `Logins` map names the pool's own `DeveloperProviderName` (the "domain" the pool
//! was created with) and, as its value, the developer user identifier the backend
//! authenticated. That pair is one login of the pool, held in the same login index
//! `GetId` keys a federated login by, so the operation is idempotent per developer user
//! identifier: the identifier already linked to an identity resolves to that identity, and
//! a fresh one mints a new identity in the pool.
//!
//! The logins one call presents name ONE account, and the call links every one of them to
//! it. So a presented login the pool has already PROVEN for another identity is no login of
//! this account, and such a call is refused. WHICH refusal it is follows what the call has
//! proven and how many of the link's two sides the pool already holds. A call that has
//! PROVEN the identity it names — one of the logins it presents is a login of that identity
//! — has shown the authority this operation asks over it, so where it ALSO presents a
//! developer user identifier the pool has already registered, both sides of the link are
//! identities the pool holds and what it asks for is a MERGE of the two, which only
//! `MergeDeveloperIdentities` performs: the conflict between those identities. A call that
//! NAMES an identity, presents no login of it and presents a developer user identifier the
//! pool has registered against another identity has proven nothing about the identity it
//! named, so it is that failure of the caller's authority over it. Where one side of the
//! link does not exist yet — no identity named, or an identifier the pool has registered
//! nothing against — there are no two stored identities to merge and what cannot be done is
//! the link of the logins presented, a condition of what the POOL holds that no login the
//! caller could send would clear. No refusal reports what the pool holds, so none names an
//! identity the caller did not send. An identity an identity PROVIDER's login
//! already proves is that provider's user, so a developer user is joined into it only by a
//! call that has itself PROVEN that account: one of the logins it presents is a login of that
//! identity, which is the documented way to associate a new login with an existing identity;
//! a call that presents none of its logins cannot link a developer user into it. An
//! unauthenticated (guest) identity and an identity of the same backend's other developer
//! user identifiers are what the `IdentityId` member links a new one to. An identity no login
//! has ever been linked to is named by its id alone, so nothing a request carries binds the
//! call to it: a developer user identifier the pool has already registered keeps naming the
//! identity it is registered under, which is the identity such a call resolves to and
//! answers, and the unauthenticated identity the call named is ABSORBED into it — the two are
//! one account from there on, so the absorbed id is left in the state a join leaves the side
//! it absorbed in and no later call reaches it as the guest it was.
//!
//! One call registers ONE of the backend's users, so the identity it registers a developer
//! user identifier in is one that can still become that user's: a new identity of the pool,
//! an unauthenticated one, an identity only the backend's own users reach, or one only an
//! identity provider's login proves. An identity that ALREADY stands for a provider's user
//! AND one of the backend's users is the joined account an implicit link made of the two, and
//! another of the backend's users comes onto it only by a JOIN of the two identities their
//! registrations stand for — the merge that leaves one record carrying the developer provider
//! once per user it stands for — so a fresh identifier presented for it is refused as the
//! link it cannot make. That JOIN is what a call presenting a developer user identifier the
//! pool has ALREADY registered elsewhere asks for, and this operation performs it where the
//! identity that registration stands for is an account the BACKEND's own logins alone reach
//! and the call has PROVEN the identity it names: the named identity survives, answering its
//! own id, and the joined-away one is left as a join leaves the side it absorbed. Where that
//! other registration is an account an identity PROVIDER's login also proves, the join is of
//! two federated accounts, which only `MergeDeveloperIdentities` performs: the conflict
//! between those identities.
//!
//! A MERGE makes the two identities it joins ONE account: the merged-away side's logins all
//! resolve to the identity that survives it, so a login of that survivor is a login of the
//! merged-away identity's own account
//! ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::canonical_identity`]).
//! An `IdentityId` naming such an identity, presented with a login of that account, is
//! therefore a call the caller has PROVEN, and it is served on the identity still standing for
//! the account — answering that identity's id, which is what the ordinary re-login of a merged
//! developer user is, the caller replaying the id it was answered before the merge. What is
//! NOT served is a call naming an identity no login of its own reaches and presenting no login
//! of its account at all: an identity an unlink or a merge left linked to nothing is
//! inaccessible, so there is nothing for such a call to link its logins into.
//!
//! The `Logins` map names ONE developer provider — "you can only specify one developer
//! provider as part of the `Logins` map, which is linked to the identity pool" — and WHICH of
//! its entries name one is the classification every reading of a presented map makes
//! ([`crate::state::developer_provider_logins`]): a map naming two is refused before the
//! pool's configuration is consulted at all.
//!
//! The token is minted from the stored identity (see [`crate::state::open_id_token`]) and
//! recorded in `open_id_tokens`, so the service can later say which identity a token it
//! issued names and until when.
//!
//! A pool whose stored roles configuration holds a ROLE MAPPING does not serve this mint at
//! all. A mapping has the service pick a session's role from the CLAIMS of the token the
//! exchange presents, and a token minted for a user the caller's own backend authenticated
//! carries no such claims, so the operation is refused on the pool's configuration before any
//! identity is resolved or any token minted.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetOpenIdTokenForDeveloperIdentity::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `GetOpenIdTokenForDeveloperIdentity`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json")
//! ```
//!
//! Registers (or retrieves) a Cognito IdentityId and an OpenID Connect token for a user
//! authenticated by your backend authentication process. Supplying multiple logins will
//! create an implicit linked account. You can only specify one developer provider as part
//! of the Logins map, which is linked to the identity pool. The developer provider is the
//! "domain" by which Cognito will refer to your users. You can use
//! GetOpenIdTokenForDeveloperIdentity to create a new identity and to link new logins (that
//! is, user credentials issued by a public provider or developer provider) to an existing
//! identity. When you want to create a new identity, the IdentityId should be null. When
//! you want to associate a new login with an existing authenticated/unauthenticated
//! identity, you can do so by providing the existing IdentityId. This API will create the
//! identity in the specified IdentityPoolId. You must use Amazon Web Services developer
//! credentials to call this operation.
//!
//! ## Input
//!
//! * `IdentityId` (IdentityId, len 1..55, pattern ^[\w-]+:[0-9a-f-]+$) — A unique identifier
//!   in the format REGION:GUID.
//! * `IdentityPoolId` (IdentityPoolId, len 1..55, pattern ^[\w-]+:[0-9a-f-]+$) **required** —
//!   An identity pool ID in the format REGION:GUID.
//! * `Logins` (LoginsMap, len 0..10) **required** — A set of optional name-value pairs that
//!   map provider names to provider tokens. Each name-value pair represents a user from a
//!   public provider or developer provider. If the user is from a developer provider, the
//!   name-value pair will follow the syntax "developer_provider_name":
//!   "developer_user_identifier". The developer provider is the "domain" by which Cognito
//!   will refer to your users; you provided this domain while creating/updating the identity
//!   pool. The developer user identifier is an identifier from your backend that uniquely
//!   identifies a user. When you create an identity pool, you can specify the supported
//!   logins.
//! * `PrincipalTags` (PrincipalTags, len 0..50) — Use this operation to configure attribute
//!   mappings for custom providers.
//! * `TokenDuration` (TokenDuration, range 1..86400) — The expiration time of the token, in
//!   seconds. You can specify a custom expiration time for the token so that you can cache
//!   it. If you don't provide an expiration time, the token is valid for 15 minutes. You can
//!   exchange the token with Amazon STS for temporary Amazon Web Services credentials, which
//!   are valid for a maximum of one hour. The maximum token duration you can set is 24 hours.
//!   You should take care in setting the expiration time for a token, as there are
//!   significant security implications: an attacker could use a leaked token to access your
//!   Amazon Web Services resources for the token's duration. Please provide for a small grace
//!   period, usually no more than 5 minutes, to account for …
//!
//! ## Output
//!
//! * `IdentityId` (IdentityId, len 1..55, pattern ^[\w-]+:[0-9a-f-]+$) — A unique identifier
//!   in the format REGION:GUID.
//! * `Token` (OIDCToken) — An OpenID token.
//!
//! ## Errors
//!
//! * `DeveloperUserAlreadyRegisteredException` (HTTP 400) — The provided developer user
//!   identifier is already registered with Cognito under a different identity ID.
//! * `InternalErrorException` (HTTP 500) — Thrown when the service encounters an error during
//!   processing the request.
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

/// The sentence a `Logins` map naming more than one DEVELOPER provider is refused with,
/// named here so the refusal below reads as one line.
use crate::state::developer_provider_logins::MULTIPLE_DEVELOPER_PROVIDERS_MESSAGE;

/// How long the token this operation mints stays valid when the request names no
/// `TokenDuration`. From that member's own documentation: "The expiration time of the
/// token, in seconds. [...] If you don't provide an expiration time, the token is valid
/// for 15 minutes."
/// (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentityInput",member="TokenDuration")`).
const DEFAULT_TOKEN_DURATION: ::std::time::Duration = ::std::time::Duration::from_secs(900);

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetOpenIdTokenForDeveloperIdentityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetOpenIdTokenForDeveloperIdentityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cognitoidentity#GetOpenIdTokenForDeveloperIdentity")]
#[derive(Debug)]
pub enum GetOpenIdTokenForDeveloperIdentityErrorModeled {
    DeveloperUserAlreadyRegisteredException(DeveloperUserAlreadyRegisteredException),
    InternalErrorException(InternalErrorException),
    InvalidParameterException(InvalidParameterException),
    NotAuthorizedException(NotAuthorizedException),
    ResourceConflictException(ResourceConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_open_id_token_for_developer_identity::GetOpenIdTokenForDeveloperIdentityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetOpenIdTokenForDeveloperIdentityError = crate::scaffolded::Scaffolded<GetOpenIdTokenForDeveloperIdentityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetOpenIdTokenForDeveloperIdentityErrorModeled> for GetOpenIdTokenForDeveloperIdentityError {
    fn from(modeled: GetOpenIdTokenForDeveloperIdentityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetOpenIdTokenForDeveloperIdentity` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetOpenIdTokenForDeveloperIdentity<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetOpenIdTokenForDeveloperIdentity<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetOpenIdTokenForDeveloperIdentity<Eng>
{
    type Input = GetOpenIdTokenForDeveloperIdentityInput;
    type Output = GetOpenIdTokenForDeveloperIdentityOutput;
    type Error = GetOpenIdTokenForDeveloperIdentityError;

    const NAME: &'static str = "GetOpenIdTokenForDeveloperIdentity";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OPEN_ID_TOKEN_FOR_DEVELOPER_IDENTITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OPEN_ID_TOKEN_FOR_DEVELOPER_IDENTITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The call AWS audits is this one, whatever it answers: the record is built from the
        // outcome, so a refused call is recorded with the error code its caller was answered
        // with. It is a DATA event of the pool it acts on, not a management event
        // ([`crate::state::data_events`]) — the call exchanges a developer login for an
        // identity in the pool's contents — so it reaches the bucket of every trail whose
        // selectors admit identity-pool data events and never event history. The token the
        // served call answers is a credential (`OIDCToken` is `@sensitive`), so the record
        // names the identity and not the token.
        let request_parameters = ::service_common::management_events::recorded_members([
            (
                "identityPoolId",
                ::serde_json::json!(input.identity_pool_id),
            ),
            ("identityId", ::serde_json::json!(input.identity_id)),
            (
                "logins",
                ::serde_json::json!(crate::state::data_events::recorded_login_values(Some(
                    &input.logins
                ))),
            ),
            ("principalTags", ::serde_json::json!(input.principal_tags)),
            ("tokenDuration", ::serde_json::json!(input.token_duration)),
        ]);
        let region = crate::state::request_region(ctx.parts());
        let identity_pool_id = Some(input.identity_pool_id.clone());
        let outcome = self.exchange(input, ctx).await;
        crate::state::data_events::hold_data_event(
            &*self.engine,
            ctx.parts(),
            &crate::state::data_events::ServedDataCall {
                operation: Self::NAME,
                region: &region,
                read_only: false,
                identity_pool_id,
                request_parameters,
                response_elements: outcome.as_ref().ok().map(|output| {
                    ::service_common::management_events::recorded_members([(
                        "identityId",
                        ::serde_json::json!(output.identity_id),
                    )])
                }),
            },
            &outcome,
        );
        outcome
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>
    GetOpenIdTokenForDeveloperIdentity<Eng>
{
    /// The exchange itself: authorize, resolve or mint the identity, and mint its token.
    async fn exchange(
        &self,
        input: GetOpenIdTokenForDeveloperIdentityInput,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<
        GetOpenIdTokenForDeveloperIdentityOutput,
        GetOpenIdTokenForDeveloperIdentityError,
    > {
        // Authorization comes first, before any identity is minted: this is a
        // developer-credentials operation ("You must use Amazon Web Services developer
        // credentials to call this operation"), so it is served only to a caller whose
        // identity-based IAM policies allow its own action on the pool the request names.
        if let Some(denied) = crate::state::authorization::denied(
            &*self.engine,
            ctx,
            <Self as smithy_server_runtime::Operation>::NAME,
            Some(&crate::state::identity_pool_arn(&input.identity_pool_id)),
        ) {
            return Err(denied);
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::InternalErrorException(
                    InternalErrorException {
                        message: Some("the identity-pool store is unavailable".to_string()),
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();

        // "This API will create the identity in the specified IdentityPoolId", so the pool
        // has to be one this service holds.
        let Some(pool) = store.identity_pools.get(&input.identity_pool_id) else {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "IdentityPool '{}' not found.",
                            input.identity_pool_id
                        )),
                    },
                )
                .into(),
            );
        };

        // `Logins` is modelled REQUIRED and `len 0..10`, so an EMPTY map reaches this
        // handler: a request that presents no login at all names no user to register or
        // resolve, which is the missing input parameter it is. That is a different refusal
        // from a map that DOES name a provider and names one the pool's own developer
        // provider is not, and it is decided before the pool's configuration is consulted at
        // all, so the two are never collapsed into one answer.
        if input.logins.is_empty() {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(crate::state::NO_LOGIN_PROVIDER_MESSAGE.to_string()),
                    },
                )
                .into(),
            );
        }

        // An identity pool is linked to ONE developer provider — "you can only specify one
        // developer provider as part of the `Logins` map, which is linked to the identity
        // pool" (`smithy_query(query="doc",shape="GetOpenIdTokenForDeveloperIdentity")`) — so
        // a map naming TWO of them asks for one account of two back ends' users at once,
        // which no pool can serve whichever domain it was created with. WHICH entries name a
        // developer provider is the classification a `Logins` map is read with
        // ([`crate::state::IdentityPool::names_a_developer_provider`]): every name this
        // service cannot say names an identity PROVIDER is a "domain" of some back end. The
        // map the caller sent is what is wrong, so this is decided on the request alone,
        // before the pool's own configuration is consulted and before any identity is
        // resolved, linked or minted and any token issued
        // ([`crate::state::developer_provider_logins::MULTIPLE_DEVELOPER_PROVIDERS_MESSAGE`]).
        if pool.logins_name_several_developer_providers(&input.logins) {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(MULTIPLE_DEVELOPER_PROVIDERS_MESSAGE.to_string()),
                    },
                )
                .into(),
            );
        }

        // A ROLE MAPPING has the SERVICE pick the role a session gets from the CLAIMS of the
        // token the exchange presents, and a token minted here stands for a user the
        // CALLER's own backend authenticated, so it carries no provider claims for a mapping
        // to resolve a role from. A pool whose stored roles configuration holds a role
        // mapping therefore does not serve this mint at all: the whole operation is closed
        // on such a pool, whichever logins the request presents and whichever identity it
        // names, so the pool's configuration is the bad input parameter the call is answered
        // with ([`crate::state::ROLE_MAPPINGS_FORBID_DEVELOPER_TOKEN_MESSAGE`]). Decided
        // here, before any identity is resolved, linked or minted and before any token is
        // issued, so the refusal leaves the pool and its identities exactly as they were.
        if store
            .identity_pool_roles
            .get(&input.identity_pool_id)
            .is_some_and(crate::state::IdentityPoolRoles::has_role_mappings)
        {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(
                            crate::state::ROLE_MAPPINGS_FORBID_DEVELOPER_TOKEN_MESSAGE
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        }

        // The developer provider is "the 'domain' by which Cognito will refer to your
        // users; you provided this domain while creating/updating the identity pool", so
        // the developer login this call carries is the `Logins` entry keyed by the POOL's
        // own `DeveloperProviderName`, and its value is the developer user identifier. A map
        // naming a SECOND developer provider beside it was refused above, so what is left
        // here is the one developer login a pool can serve. A pool that names no developer
        // provider, and a `Logins` naming none of the pool's own, are the SAME refusal to a
        // caller — the developer provider this call is about is not one this pool supports —
        // so both answer the one fixed sentence every operation of this service that resolves
        // a developer provider answers
        // ([`crate::state::UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE`]), which names neither the
        // provider the request carried nor the pool's own.
        let Some(developer_provider) = pool.developer_provider_name.clone() else {
            return Err(unsupported_developer_provider());
        };
        let Some(developer_user_identifier) = input.logins.get(&developer_provider).cloned() else {
            return Err(unsupported_developer_provider());
        };

        // Every login the call presents — the developer one and any public provider beside
        // it — in a stable order.
        let mut presented: Vec<(String, String)> = input
            .logins
            .iter()
            .map(|(provider, login)| (provider.clone(), login.clone()))
            .collect();
        presented.sort();
        let mut providers: Vec<String> = input.logins.keys().cloned().collect();
        providers.sort();

        // "When you want to associate a new login with an existing authenticated/
        // unauthenticated identity, you can do so by providing the existing IdentityId" —
        // an identity of THIS pool, since an identity never moves between pools. Which pool
        // holds it is settled FIRST ([`crate::state::ScopedIdentityRefusal`]), so an identity
        // of another pool is one this request may not reach rather than a resource this
        // service does not hold.
        //
        // WHICH identity the id it carries names is a second question: a MERGE makes the two
        // identities it joins ONE account, giving the merged-away side's logins to the
        // identity that survives it, so from then on the merged-away id is another name for
        // that one account and not an account of its own
        // ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::canonical_identity`]).
        // A developer user of a merged account keeps replaying the id it was answered before
        // the merge, and the identity that id names is the one still standing for that
        // account, so the call is served as the call on that identity it is — the id the
        // request carried is what the call RESOLVES through, not a second account the
        // presented login fails to prove.
        let mut named_identity_is_idp_proven = false;
        let mut named_identity_has_been_linked = false;
        let mut named_identity_is_inaccessible = false;
        let mut named_identity = input.identity_id.clone();
        if let Some(identity_id) = &input.identity_id {
            match store.scoped_identity(&input.identity_pool_id, identity_id) {
                Err(crate::state::ScopedIdentityRefusal::NotStored) => {
                    return Err(
                        GetOpenIdTokenForDeveloperIdentityErrorModeled::ResourceNotFoundException(
                            ResourceNotFoundException {
                                message: Some(crate::state::identity_not_found_message(
                                    identity_id,
                                )),
                            },
                        )
                        .into(),
                    );
                }
                Err(crate::state::ScopedIdentityRefusal::AnotherPool) => {
                    return Err(
                        GetOpenIdTokenForDeveloperIdentityErrorModeled::NotAuthorizedException(
                            NotAuthorizedException {
                                message: Some(crate::state::forbidden_identity_message(
                                    identity_id,
                                )),
                            },
                        )
                        .into(),
                    );
                }
                Ok(identity) => {
                    // Whether the identity the request NAMED is one no login of its own
                    // reaches: an identity an unlink or a merge left linked to nothing
                    // ([`crate::state::Identity::is_disabled`]).
                    named_identity_is_inaccessible = identity.is_disabled();
                    // The identity still standing for the account that id names, which every
                    // question below is asked of.
                    let standing_for = store.canonical_identity(identity_id);
                    if let Some(identity) = store.identities.get(&standing_for) {
                        // Whether that identity is one an IDENTITY PROVIDER's login already
                        // proves: a provider it is linked to that is not the pool's own
                        // developer provider is a provider whose own user it is.
                        named_identity_is_idp_proven = identity
                            .logins
                            .iter()
                            .any(|provider| *provider != developer_provider);
                        named_identity_has_been_linked = identity.has_been_linked();
                    }
                    named_identity = Some(standing_for);
                }
            }
        }

        let registered_to = store
            .linked_identity(
                &input.identity_pool_id,
                &developer_provider,
                &developer_user_identifier,
            )
            .cloned();

        // An identity the request names that NO login has ever been linked to — the
        // UNAUTHENTICATED (guest) identity `GetId` mints for a call presenting none — is
        // named by its id alone: there is no login of it for a call to present, so nothing
        // the request carries binds the call to it. A developer user identifier the pool has
        // already registered therefore keeps naming the identity it is registered under, and
        // that identity is the one this call is about; the identity the request named takes
        // no part in it and is left untouched. An identity logins HAVE been linked to is a
        // federated user's, so it stays the identity the call is about and a presented login
        // proving another one is the conflict decided below.
        let ignores_named_identity = named_identity.is_some()
            && !named_identity_has_been_linked
            && registered_to.is_some()
            && registered_to != named_identity;

        // The identity this call is about: the one the request names, else the one the
        // developer user identifier is already registered under (which is what makes the
        // operation idempotent per identifier), else none yet — a new identity.
        let resolved = if ignores_named_identity {
            registered_to.clone()
        } else {
            named_identity.clone().or_else(|| registered_to.clone())
        };

        // Every login the call presents belongs to the ONE account this call is about, and
        // the call links every one of them to it. Two things break that, and the pool's own
        // index is what says so ([`identities_proven_by`]: the logins it has indexed against
        // an identity, the developer user identifiers registered to one, and the identity a
        // token this service minted was minted for):
        //
        // * a presented login the pool has already PROVEN for another identity is no login
        //   of this account;
        // * the identity the request NAMES is one an identity PROVIDER's login already
        //   proves, and the developer user identifier presented is not one already registered
        //   against it — so the call would JOIN a developer user to an account that is already
        //   that provider's user, which this operation does not do: it registers a developer
        //   user identifier in an identity no provider's login proves. A login of the named
        //   identity presented beside the developer one shows the caller may act on that
        //   identity, which is not the same as the two users being one account, so it does not
        //   make the join one this call can have. An UNAUTHENTICATED (guest) identity no login
        //   proves is the case the member exists for, another developer user identifier of a
        //   developer identity is one more login of the same backend's user, and a developer
        //   login the pool has already registered against the named identity is that
        //   identity's own re-login, so none of those is refused here.
        let proven = store.identities_proven_by(&input.identity_pool_id, &input.logins, now);
        // Whether one of the presented logins is a login OF the identity the request names —
        // the same question every login-authorized call of this service is authorized by
        // (`logins_name_identity`), answered from the identities just resolved.
        let proves_the_named_identity = named_identity
            .as_ref()
            .is_some_and(|named| proven.contains(named));
        // The identity the presented developer user identifier is already registered under,
        // JOINED into the identity this call has proven. An identity the pool reaches through
        // the BACKEND's own logins alone stands for that backend's users and for no provider's
        // user, so a call that names the identity it registers the user in and PROVES it — one
        // of the logins it presents is a login of that identity — makes the two one account:
        // the merge this operation performs implicitly, leaving the surviving record carrying
        // the developer provider once per user it now stands for and the joined-away one
        // reached by no login at all
        // ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::merge_identity_into`]).
        // An account an identity PROVIDER's login ALSO proves is that provider's user, and
        // joining two such accounts is what `MergeDeveloperIdentities` is for, so that one is
        // refused below instead.
        let joined_developer_identity = match (named_identity.as_deref(), registered_to.as_deref())
        {
            (Some(named), Some(registered))
                if registered != named
                    && resolved.as_deref() == Some(named)
                    && proves_the_named_identity =>
            {
                let logins = store.pool_logins_of_identity(&input.identity_pool_id, registered);
                (!logins.is_empty()
                    && logins
                        .iter()
                        .all(|(provider, _)| *provider == developer_provider))
                .then(|| registered.to_string())
            }
            _ => None,
        };
        let proves_another_account = proven.iter().any(|named| {
            Some(named) != resolved.as_ref()
                && Some(named.as_str()) != joined_developer_identity.as_deref()
        });
        let would_join_a_proven_identity = named_identity.as_deref().is_some_and(|named| {
            named_identity_is_idp_proven
                && !proves_the_named_identity
                && registered_to.as_deref() != Some(named)
        });
        // Whether the call would register a developer user identifier the pool has registered
        // against NOTHING in an identity that ALREADY stands for a provider's user AND one of
        // the backend's users at once — the account an implicit link makes of the two
        // ("supplying multiple logins will create an implicit linked account"). One call
        // registers ONE of the backend's users, and such an identity is already the joined
        // account of a provider's user and one of them, so another of the backend's users
        // comes onto it only by a JOIN of the two identities their registrations stand for —
        // the merge that leaves one record carrying the developer provider once per user it
        // stands for. The link itself is therefore not one this call can make. An identity
        // only the backend's own users reach, one only an identity provider's login proves,
        // an unauthenticated one and a new identity of the pool each still take the
        // registration, so none of them is refused here.
        let adds_to_a_joined_account = registered_to.is_none()
            && resolved.as_deref().is_some_and(|identity_id| {
                let logins =
                    store.pool_logins_of_identity(&input.identity_pool_id, identity_id);
                logins
                    .iter()
                    .any(|(provider, _)| *provider == developer_provider)
                    && logins
                        .iter()
                        .any(|(provider, _)| *provider != developer_provider)
            });

        // An identity reached through NO login of its own — a DISABLED one, which an unlink
        // or a merge left linked to nothing ("removing the last linked login will make this
        // identity inaccessible", `UnlinkIdentity`,
        // `smithy_query(query="doc",shape="UnlinkIdentity")`) — is an identity no caller can
        // show a login of, and linking the logins presented INTO the identity named is the
        // one thing this operation does with an identity a call has not proven. So a call
        // naming such an identity is served only where a presented login DOES prove the
        // account it belongs to: for one a merge merged away, a login of the surviving
        // identity, the two being one account from the merge on. Presented anything else, the
        // call has shown no authority over the identity it named, which is the authorization
        // failure every login-authorized call of this service answers in one sentence
        // ([`crate::state::UNMATCHED_LOGINS_MESSAGE`]) — fixed text, so it tells a caller
        // nothing about what the pool holds for that identity.
        if named_identity_is_inaccessible && !proves_the_named_identity {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::NotAuthorizedException(
                    NotAuthorizedException {
                        message: Some(crate::state::UNMATCHED_LOGINS_MESSAGE.to_string()),
                    },
                )
                .into(),
            );
        }
        if proves_another_account || would_join_a_proven_identity || adds_to_a_joined_account {
            // Which refusal this is follows WHOSE ownership the request got wrong, and how
            // many of the link's two sides are identities the pool already HOLDS.
            //
            // A call that has PROVEN the identity it names — one of the logins it presents is
            // a login OF that identity — has shown the authority this operation asks of it
            // over that identity, so its fault is not one of authority at all. Presented
            // beside that proof is a developer user identifier the pool has already
            // registered, so BOTH sides of the link are identities the pool holds and what
            // the call asks for is a MERGE of the two. Joining two stored identities is what
            // `MergeDeveloperIdentities` is for, so the implicit link is refused as the
            // conflict between those identities
            // ([`crate::state::CANNOT_MERGE_IDENTITIES_MESSAGE`], fixed text naming neither
            // of them).
            //
            // A request that NAMES an identity, presents NO login of it, and presents a
            // developer user identifier the pool has already registered against another
            // identity presents logins that are not all logins OF the identity it named: the
            // call is not one the caller has shown it may make on that identity, which is
            // the authorization failure every login-authorized call of this service answers
            // in one sentence ([`crate::state::UNMATCHED_LOGINS_MESSAGE`]) — "include at
            // least one valid login for this identity". Fixed text, so it tells a caller
            // nothing about which other identity holds the login it presented.
            //
            // Where one side of the link does not exist yet — the request names NO identity,
            // or presents a developer user identifier the pool has registered nothing
            // against — there are not two stored identities to merge and the fact is about
            // the POOL's state instead: a login already linked to another account, which is
            // precisely what the modelled `ResourceConflictException` states ("Thrown when a
            // user tries to use a login which is already linked to another account") — a
            // condition of what the pool HOLDS rather than of what the caller may do, so no
            // login the caller could add would clear it
            // ([`crate::state::CANNOT_LINK_LOGINS_MESSAGE`], also fixed text). A fresh
            // developer user identifier the call would register in the joined account of a
            // provider's user and one of the backend's users is the same fact about the pool:
            // that identity already stands for both, and only a JOIN of two identities puts
            // another of the backend's users on it.
            if registered_to.is_some() && proves_the_named_identity {
                return Err(
                    GetOpenIdTokenForDeveloperIdentityErrorModeled::ResourceConflictException(
                        ResourceConflictException {
                            message: Some(
                                crate::state::CANNOT_MERGE_IDENTITIES_MESSAGE.to_string(),
                            ),
                        },
                    )
                    .into(),
                );
            }
            if named_identity.is_some() && registered_to.is_some() {
                return Err(
                    GetOpenIdTokenForDeveloperIdentityErrorModeled::NotAuthorizedException(
                        NotAuthorizedException {
                            message: Some(crate::state::UNMATCHED_LOGINS_MESSAGE.to_string()),
                        },
                    )
                    .into(),
                );
            }
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::ResourceConflictException(
                    ResourceConflictException {
                        message: Some(crate::state::CANNOT_LINK_LOGINS_MESSAGE.to_string()),
                    },
                )
                .into(),
            );
        }

        // The stores this call grows carry THIS emulator's own ceilings (`crate::state`),
        // checked before anything is written. The operation models no limit error, so a
        // request the emulator cannot store is refused with the modelled throttle rather
        // than written past the ceiling.
        if store.pool_login_index_would_overflow(&input.identity_pool_id, &presented) {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::TooManyRequestsException(
                    TooManyRequestsException {
                        message: Some(format!(
                            "The maximum number of logins ({}) for the identity pool '{}' has \
                             been reached.",
                            crate::state::MAX_LOGINS_PER_POOL, input.identity_pool_id
                        )),
                    },
                )
                .into(),
            );
        }
        if let Some(resolved) = &resolved {
            if store.identity_providers_would_overflow(resolved, &providers) {
                return Err(
                    GetOpenIdTokenForDeveloperIdentityErrorModeled::TooManyRequestsException(
                        TooManyRequestsException {
                            message: Some(format!(
                                "The maximum number of linked login providers ({}) for the \
                                 identity '{resolved}' has been reached.",
                                crate::state::MAX_LOGIN_PROVIDERS_PER_IDENTITY
                            )),
                        },
                    )
                    .into(),
                );
            }
        }

        let identity_id = match &resolved {
            Some(identity_id) => identity_id.clone(),
            None => {
                if store.pool_identity_store_is_full(&input.identity_pool_id) {
                    return Err(
                        GetOpenIdTokenForDeveloperIdentityErrorModeled::TooManyRequestsException(
                            TooManyRequestsException {
                                message: Some(format!(
                                    "The maximum number of identities ({}) for the identity pool \
                                     '{}' has been reached.",
                                    crate::state::MAX_IDENTITIES_PER_POOL,
                                    input.identity_pool_id
                                )),
                            },
                        )
                        .into(),
                    );
                }
                // "When you want to create a new identity, the IdentityId should be null":
                // the identity lives in its pool's region, which the pool's id names.
                let region = crate::state::region_of(&input.identity_pool_id).to_string();
                let identity_id = store.new_identity_id(&region);
                store.identities.insert(
                    identity_id.clone(),
                    crate::state::Identity {
                        identity_id: identity_id.clone(),
                        identity_pool_id: input.identity_pool_id.clone(),
                        logins: providers.clone(),
                        // A freshly minted identity has had no unlink, so it is not a
                        // disabled one.
                        disabled: false,
                        merged_away: false,
                        merged_into: None,
                        creation_date: now,
                        last_modified_date: now,
                    },
                );
                store
                    .identities_by_pool
                    .entry(input.identity_pool_id.clone())
                    .or_default()
                    .push(identity_id.clone());
                identity_id
            }
        };

        // The identity the presented developer user identifier was registered in, where this
        // call JOINS it into the identity it proved: the two are one account from here on,
        // reached through the identity this call answers, so the joined-away one is left in
        // the state that join leaves the side it absorbed in — a DISABLED identity whose
        // record stays stored, its developer users resolving to the surviving identity, which
        // carries the developer provider once per user it now stands for. Joined BEFORE the
        // presented logins are linked, so the user this call presents is counted on the
        // survivor once: the join brings that registration over, and linking the login again
        // adds a provider the identity now carries no second time.
        if let Some(joined) = &joined_developer_identity {
            store.merge_identity_into(&input.identity_pool_id, joined, &identity_id, now);
        }

        // Every presented login now resolves to this identity, however it was reached —
        // "supplying multiple logins will create an implicit linked account".
        for (provider, login) in &presented {
            store.link_login(&input.identity_pool_id, provider, login, &identity_id);
        }

        // A provider this identity had not been linked to before is a change to the
        // identity, so it is recorded with the instant it happened.
        if resolved.is_some() {
            if let Some(identity) = store.identities.get_mut(&identity_id) {
                let linked_before = identity.logins.len();
                for provider in &providers {
                    if !identity.logins.contains(provider) {
                        identity.logins.push(provider.clone());
                    }
                }
                if identity.logins.len() != linked_before {
                    identity.logins.sort();
                    identity.last_modified_date = now;
                }
            }
        }

        // The UNAUTHENTICATED identity a call resolved AWAY from is one side of the account
        // this call joins: it carries no login of its own, so there is nothing of it to link
        // to the identity the developer user identifier is registered under, and what the
        // implicit link joins is the IDENTITY itself — "supplying multiple logins will create
        // an implicit linked account". The two are one account from here on, reached through
        // the identity this call answers, so the guest identity is ABSORBED into it: the state
        // a join leaves the side it absorbed in ([`crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::merge_identity_into`]),
        // a DISABLED identity whose record stays stored and whose read projects the sentinel
        // that join leaves behind. So no later call reaches the absorbed id as the
        // unauthenticated identity it was: what names an identity a login has been linked to
        // is a login OF it, and the absorbed one has none.
        if ignores_named_identity {
            if let Some(absorbed) = &named_identity {
                store.merge_identity_into(
                    &input.identity_pool_id,
                    absorbed,
                    &identity_id,
                    now,
                );
            }
        }

        // `TokenDuration` is modelled `range 1..86400`, so a value outside it was refused
        // before this handler ran; its absence is the documented 15 minutes.
        let validity = input
            .token_duration
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(DEFAULT_TOKEN_DURATION, ::std::time::Duration::from_secs);
        let expiration = now + validity;
        // What AUTHENTICATED this identity, as the token's `amr` claim names it: each provider
        // the call presented, and — for the developer one — the user that provider
        // authenticated, named as `<developerProviderName>:<identityPoolId>:
        // <developerUserIdentifier>`. A developer-authenticated identity is the user of the
        // developer's OWN directory, so the method that stands for it has to name that user:
        // the provider name alone says only that the directory authenticated somebody, which
        // is why a redeemed token publishes both.
        let mut authenticated_by = providers.clone();
        authenticated_by.push(format!(
            "{developer_provider}:{}:{developer_user_identifier}",
            input.identity_pool_id
        ));
        // A token is either SIGNED or not issued: a claim set nothing could sign is a server
        // failure of this service, not a token.
        let Some(token) = crate::state::open_id_token(
            &input.identity_pool_id,
            &identity_id,
            &authenticated_by,
            now,
            expiration,
        ) else {
            return Err(
                GetOpenIdTokenForDeveloperIdentityErrorModeled::InternalErrorException(
                    InternalErrorException {
                        message: Some(
                            "the OpenID Connect token could not be signed".to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        // What the clock has already expired is forgotten here, and the store is kept
        // inside this emulator's own ceiling on outstanding tokens; then this issuance is
        // recorded, which is what lets the service say later which identity the token it
        // handed out names.
        let issued = crate::state::IssuedOpenIdToken {
            identity_id: identity_id.clone(),
            identity_pool_id: input.identity_pool_id.clone(),
            issued_at: now,
            expiration,
            authenticated_by: authenticated_by.clone(),
            principal_tags: input
                .principal_tags
                .map(|tags| tags.into_iter().collect()),
        };
        store.record_open_id_token(token.clone(), issued.clone(), now);
        // The same issuance, in the registry the service that REDEEMS this token reads
        // (`sts:AssumeRoleWithWebIdentity`), which is how a token this emulator issued is
        // told apart from one it did not. Recorded after this service's own store and with
        // its lock released, so the two registries are never held at once.
        drop(store);
        crate::state::record_issued_web_identity(&*self.engine, &token, &issued, now);

        Ok(GetOpenIdTokenForDeveloperIdentityOutput {
            identity_id: Some(identity_id),
            token: Some(token),
        })
    }
}

/// The modelled refusal of a request whose developer provider is not the one its identity
/// pool is configured with (HTTP 400): a bad input parameter, which is what the caller has
/// to fix, carrying the one fixed sentence this service states for it.
fn unsupported_developer_provider() -> GetOpenIdTokenForDeveloperIdentityError {
    GetOpenIdTokenForDeveloperIdentityErrorModeled::InvalidParameterException(
        InvalidParameterException {
            message: Some(crate::state::UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE.to_string()),
        },
    )
    .into()
}
