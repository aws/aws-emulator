// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentity-awscognitoidentityservice service.
//!
//! The identity-pool lifecycle and the identity/credential flow are stored here, in the
//! single shared engine, so every handler in this crate observes the same store across
//! requests. The records below are the SERVICE's own view of a resource; the modelled
//! wire shapes live in `crate::ops::types`, and a shape several operations answer with is
//! projected from its record once, there, so those operations cannot report it differently
//! (`ReportedIdentityPool`, `IdentityPoolShortDescription`).

/// Whether IAM lets a request to this service do what it asks — the authorization gate
/// every developer-credentials operation of this service consults before it reads or
/// writes anything.
pub mod authorization;

/// The DATA events of this service's identity data plane: what a record of one carries, and
/// where the account's trails deliver it.
pub mod data_events;

/// What an identity pool's own configuration has to satisfy before it is stored, and how a
/// configuration this service will not store is refused.
pub mod pool_configuration;

use crate::ops::types::{CognitoIdentityProvider, MappingRule, RoleMapping};
use emulator_server::CredentialStore;
/// What an integrated Amazon Cognito user pool says about a token presented as the login of
/// one of its users, re-exported from the service that OWNS that record so this service
/// names it rather than keeping a second copy of it
/// ([`IntegratedUserPools`]).
pub use ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state::PresentedTokenUser;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// The region an id is minted in when the request carries no SigV4 credential scope to
/// read one from (`GetId` and `GetCredentialsForIdentity` are modelled `@optionalAuth`,
/// so an unsigned request is legitimate).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most pools this emulator stores. Not an AWS quota: it is THIS emulator's own
/// ceiling on a store that grows by one record per `CreateIdentityPool` call and lives
/// for the process, so an unbounded caller cannot grow it without limit. A call past it
/// is refused with the modelled `LimitExceededException`.
pub const MAX_IDENTITY_POOLS: usize = 1_000;

/// The most identities this emulator stores in one pool, for the same reason: `GetId`
/// mints one per unauthenticated call. Also the emulator's own ceiling, refused with the
/// modelled `LimitExceededException`.
pub const MAX_IDENTITIES_PER_POOL: usize = 10_000;

/// The most federated logins this emulator indexes for one pool. `GetId` adds one entry
/// per `Logins` entry it has not seen before, whether or not it mints an identity, so
/// this store needs its own ceiling: twice [`MAX_IDENTITIES_PER_POOL`], which leaves room
/// for identities reached through more than one provider. The emulator's own bound,
/// refused with the modelled `LimitExceededException`.
pub const MAX_LOGINS_PER_POOL: usize = 2 * MAX_IDENTITIES_PER_POOL;

/// The most providers one identity is linked to. THIS EMULATOR's own ceiling on a record
/// a caller can grow — `GetId` links up to ten providers per call (`LoginsMap` is modelled
/// `len 0..10`) and may be called again — not an AWS quota, so it is set well above what a
/// single call can reach while still bounding the stored record and the `Logins` list
/// every `ListIdentities` page carries. Refused with the modelled `LimitExceededException`.
pub const MAX_LOGIN_PROVIDERS_PER_IDENTITY: usize = 50;

/// The most identity RECORDS this emulator keeps for identities whose pool has been
/// deleted. An identity belongs to the pool that minted it and the record of it outlives
/// that pool's delete, so an identity-scoped call made afterwards can report the POOL as
/// the resource that is missing; but the pool's delete also releases the per-pool ceiling
/// those records were counted against ([`MAX_IDENTITIES_PER_POOL`]), so without a ceiling
/// of their own a caller repeating create-pool / mint / delete-pool would grow the store
/// without limit. THIS EMULATOR's own bound, like the ceilings above, and it is enforced by
/// EVICTION rather than by a refusal: the delete is a call AWS serves, so the oldest
/// orphaned records are given up instead — an identity-scoped call naming one this
/// emulator no longer keeps reports that identity missing, which is what it is.
pub const MAX_ORPHANED_IDENTITY_RECORDS: usize = MAX_IDENTITIES_PER_POOL;

/// The most logins AMAZON COGNITO links to one identity — not this emulator's own
/// ceiling, but the service's documented limit: "The number of linked logins is limited to
/// 20. So, the number of linked logins for the source user, SourceUserIdentifier, and the
/// destination user, DestinationUserIdentifier, together should not be larger than 20.
/// Otherwise, an exception will be thrown" (`MergeDeveloperIdentities`,
/// `smithy_query(query="doc",shape="MergeDeveloperIdentities")`). A linked login is one
/// indexed `(provider, value)` pair of the pool — a developer user identifier as much as a
/// federated token's login — which is what [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::pool_logins_of_identity`]
/// reports. The limit is enforced where that documentation states it: on a merge, which is
/// the one call that adds another user's whole login set to an identity at once.
pub const MAX_LINKED_LOGINS_PER_IDENTITY: usize = 20;

/// A stored identity pool, as `CreateIdentityPool` wrote it and
/// `DescribeIdentityPool` reports it. Every optional member is held as an `Option` so a
/// read returns exactly what was stored, rather than inventing an empty map or list for
/// a member the caller never sent.
#[derive(Debug, Clone)]
pub struct IdentityPool {
    pub identity_pool_id: String,
    pub identity_pool_name: String,
    pub allow_unauthenticated_identities: bool,
    pub allow_classic_flow: Option<bool>,
    pub supported_login_providers: Option<BTreeMap<String, String>>,
    pub developer_provider_name: Option<String>,
    pub open_id_connect_provider_arns: Option<Vec<String>>,
    pub cognito_identity_providers: Option<Vec<CognitoIdentityProvider>>,
    pub saml_provider_arns: Option<Vec<String>>,
    pub identity_pool_tags: Option<BTreeMap<String, String>>,
    /// The instant the pool was created, as every reader of the pool reports it. Fixed when
    /// the record is first written and never touched again, so the id, the ARN and this
    /// instant name the same pool for as long as it is stored.
    pub creation_date: SystemTime,
    /// The instant the stored record last CHANGED — its configuration (`UpdateIdentityPool`)
    /// or its tag set (`TagResource`/`UntagResource`). Equal to [`Self::creation_date`] on a
    /// pool nothing has changed since it was created, and never moved by a read.
    pub last_modified_date: SystemTime,
}

impl IdentityPool {
    /// What this pool's stored configuration trusts a presented login's ISSUER through,
    /// with `registered` — the IAM OpenID Connect providers the account holds
    /// ([`registered_oidc_providers`]) — resolving the provider ARNs the pool names.
    ///
    /// `GetCredentialsForIdentity` is documented "Any provided logins will be validated
    /// against supported login providers" (this service's model), and a `Logins` entry
    /// carries its issuer in the provider name itself
    /// (`cognito-idp.<region>.amazonaws.com/<UserPoolId>` for an Amazon Cognito user pool,
    /// the issuer's host and path for any other OpenID Connect provider — `GetId.Logins`).
    /// Two stored members state a trust for such an issuer, and this collects both:
    ///
    /// * a `CognitoIdentityProviders` entry, which names an Amazon Cognito user pool. The
    ///   trust is the issuer; what the user pool's own token says it was issued for is part
    ///   of validating that token against the user pool that minted it, which this does not
    ///   do, so the entry states no audience constraint here.
    /// * an `OpenIdConnectProviderARNs` entry, whose ARN names the issuer
    ///   ([`oidc_provider_arn_issuer`]) and whose IAM registration holds the client ids —
    ///   "a list of client IDs (also known as audiences)" — that trust accepts. The
    ///   registration is read at the instant the login is presented, so an ARN of THIS
    ///   account that resolves to no provider states the trust and accepts no audience at
    ///   all; an ARN naming another account's provider resolves to a registration this
    ///   emulator does not hold, so what it accepts is not knowable here and the trust
    ///   states no audience constraint.
    ///
    /// The pool's other provider members name no issuer this emulator can decide a token
    /// against: a public login provider, a SAML provider and the pool's own developer
    /// provider are each decided elsewhere, so they state no trust here and no login of
    /// theirs is refused on this configuration.
    pub fn login_trust(&self, registered: &RegisteredOidcProviders) -> PoolLoginTrust {
        let mut issuers: BTreeMap<String, TrustedAudiences> = BTreeMap::new();
        for provider in self.cognito_identity_providers.iter().flatten() {
            let issuer = provider
                .provider_name
                .as_deref()
                .map(login_provider_issuer)
                .unwrap_or_default();
            if issuer.is_empty() {
                continue;
            }
            issuers
                .entry(issuer.to_string())
                .or_default()
                .accept(false, Vec::new());
        }
        for arn in self.open_id_connect_provider_arns.iter().flatten() {
            // The ARN itself names the issuer, so the pool states the trust whether or not
            // the account still holds the provider; the registration the ARN RESOLVES to is
            // where the client ids it accepts come from, and one of this account that
            // resolves to no provider accepts none of them.
            let Some(issuer) = oidc_provider_arn_issuer(arn) else {
                continue;
            };
            let resolvable = oidc_provider_arn_is_of_this_account(arn);
            let client_ids = registered
                .registered(arn)
                .map(|provider| provider.client_ids.clone())
                .unwrap_or_default();
            issuers
                .entry(issuer.to_string())
                .or_default()
                .accept(resolvable, client_ids);
        }
        PoolLoginTrust {
            issuers,
            // WHY the trust holds no entry for an issuer is part of what a refusal says, so
            // the pool's whole provider configuration travels with the trust: a pool that
            // names no identity provider AT ALL is in a different state from one whose
            // providers simply do not include the issuer presented
            // ([`Self::configures_no_identity_provider`]).
            configures_no_provider: self.configures_no_identity_provider(),
        }
    }

    /// Whether a `Logins` entry made in the name of `provider_name` carries an OpenID
    /// Connect IDENTITY TOKEN, so the value presented for it is decidable from the token
    /// itself.
    ///
    /// Two of this pool's configured provider kinds issue one: an Amazon Cognito user pool
    /// of its `CognitoIdentityProviders`, whose login is the user pool's id token, and an
    /// OpenID Connect issuer of its `OpenIdConnectProviderARNs` ("A list of OpendID Connect
    /// provider ARNs", this service's model), whose login is that issuer's id token. A login
    /// of any other kind carries a provider-specific credential — an OAuth access token for
    /// a public provider, a developer user identifier for the pool's own developer provider
    /// — which is no JSON Web Token and is decided by the provider that issued it.
    pub fn issues_open_id_connect_tokens(&self, provider_name: &str) -> bool {
        self.names_cognito_identity_provider(provider_name)
            || self.names_open_id_connect_provider(provider_name)
    }

    /// Whether one of this pool's `CognitoIdentityProviders` names `provider_name`: the
    /// Amazon Cognito user pool whose id tokens this pool asked for.
    fn names_cognito_identity_provider(&self, provider_name: &str) -> bool {
        let issuer = login_provider_issuer(provider_name);
        self.cognito_identity_providers
            .iter()
            .flatten()
            .filter_map(|provider| provider.provider_name.as_deref())
            .any(|named| login_provider_issuer(named) == issuer)
    }

    /// Whether `provider_name` is the issuer of one of this pool's
    /// `OpenIdConnectProviderARNs`: an IAM OpenID Connect provider is named
    /// `arn:aws:iam::<account>:oidc-provider/<url without its scheme>`, and that url is the
    /// name a `Logins` entry of that issuer is made under
    /// ([`oidc_provider_arn_issuer`]).
    fn names_open_id_connect_provider(&self, provider_name: &str) -> bool {
        let issuer = login_provider_issuer(provider_name);
        self.open_id_connect_provider_arns
            .iter()
            .flatten()
            .filter_map(|arn| oidc_provider_arn_issuer(arn))
            .any(|named| named == issuer)
    }

    /// Whether `provider_name` is this pool's own developer provider — "the 'domain' by
    /// which Cognito will refer to your users" — rather than a PUBLIC provider. A pool that
    /// names no developer provider has no such entry at all.
    pub fn names_developer_provider(&self, provider_name: &str) -> bool {
        self.developer_provider_name.as_deref() == Some(provider_name)
    }

    /// Whether `provider_name` is one of this pool's `SamlProviderARNs`: a SAML identity
    /// provider, whose `Logins` entry is keyed by the provider's own ARN ("A list of the
    /// SAML provider ARNs", this service's model) and whose assertion is what carries the
    /// roles the user may assume.
    pub fn names_saml_provider(&self, provider_name: &str) -> bool {
        self.saml_provider_arns
            .iter()
            .flatten()
            .any(|arn| arn == provider_name)
    }

    /// Whether this pool's FEDERATED-PROVIDER configuration declares `provider_name`: an
    /// identity provider that can authenticate a user of this pool and present a login for
    /// it.
    ///
    /// Four stored members hold such a provider, and every one of them is one a `Logins`
    /// entry can be keyed by: `SupportedLoginProviders`, "the supported login providers"
    /// keyed by provider name; `CognitoIdentityProviders`, each naming an Amazon Cognito
    /// user pool; `SamlProviderARNs` and `OpenIdConnectProviderARNs`, each naming the
    /// provider by the ARN of its IAM registration — whose issuer is the name a login of it
    /// is made under, so either spelling resolves ([`oidc_provider_arn_issuer`]).
    ///
    /// The pool's own `DeveloperProviderName` is NOT one of them: a developer provider is
    /// this account's own back end rather than a federated provider, and it issues no token
    /// a provider's claims could be read from.
    pub fn names_federated_provider(&self, provider_name: &str) -> bool {
        self.supported_login_providers
            .iter()
            .flatten()
            .any(|(named, _)| named == provider_name)
            || self.names_cognito_identity_provider(provider_name)
            || self.names_open_id_connect_provider(provider_name)
            || self.names_saml_provider(provider_name)
            || self
                .open_id_connect_provider_arns
                .iter()
                .flatten()
                .any(|arn| arn == provider_name)
    }

    /// Whether this pool's stored configuration names NO identity provider at all: none of
    /// the four federated-provider members a `Logins` entry can be keyed by
    /// ([`Self::names_federated_provider`]) and no developer provider of its own either.
    ///
    /// A pool in that state federates NOBODY — there is no issuer whose users it asked for,
    /// so no login presented to it could have been one of a provider it is configured with.
    /// That is a different thing from a login whose provider is not among the ones the pool
    /// does name ([`LoginProviderRefusal::UnsupportedProvider`]), and AWS words it
    /// differently ([`no_identity_providers_message`]): what the caller has to act on is the
    /// POOL's configuration rather than the token it presented.
    pub fn configures_no_identity_provider(&self) -> bool {
        let no_public_providers = self
            .supported_login_providers
            .as_ref()
            .is_none_or(BTreeMap::is_empty);
        let no_user_pools = self
            .cognito_identity_providers
            .as_ref()
            .is_none_or(Vec::is_empty);
        let no_oidc_providers = self
            .open_id_connect_provider_arns
            .as_ref()
            .is_none_or(Vec::is_empty);
        let no_saml_providers = self.saml_provider_arns.as_ref().is_none_or(Vec::is_empty);
        let no_developer_provider = self
            .developer_provider_name
            .as_deref()
            .is_none_or(str::is_empty);
        no_public_providers
            && no_user_pools
            && no_oidc_providers
            && no_saml_providers
            && no_developer_provider
    }

    /// Whether `logins` carries at least one entry made in the name of a PUBLIC provider —
    /// any provider that is not this pool's own developer one.
    pub fn logins_name_a_public_provider(&self, logins: &HashMap<String, String>) -> bool {
        logins
            .keys()
            .any(|provider| !self.names_developer_provider(provider))
    }

    /// Whether a `Logins` entry made in the name of `provider_name` carries an OpenID
    /// Connect IDENTITY TOKEN whose own claims this service reads before anything else is
    /// decided about it.
    ///
    /// Two kinds of provider name say so: one of this pool's configured OpenID Connect
    /// providers ([`IdentityPool::issues_open_id_connect_tokens`]), and ANY name that names
    /// an Amazon Cognito user pool ([`names_cognito_user_pool`]) — the
    /// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` form is a user pool's issuer
    /// whether or not this pool was configured with it, and a user pool's login is always
    /// that pool's id token. Every other provider name carries a provider-specific
    /// credential this service cannot read.
    fn presents_identity_token(&self, provider_name: &str) -> bool {
        self.issues_open_id_connect_tokens(provider_name) || names_cognito_user_pool(provider_name)
    }

    /// Why the `Logins` map of a request authorizes nothing, if so: what the PRESENTED
    /// TOKENS themselves say, then what this pool's stored configuration trusts
    /// ([`PoolLoginTrust::refuse`]), and last what the user pool a token was issued by says
    /// about the user it stands for ([`Self::refuse_server_side_token_check`]).
    ///
    /// The entries are decided in a stable order, so a map that breaks the rule in two
    /// places is refused the same way every time, and the token's own form and issuer are
    /// decided BEFORE the pool's configuration: a value that is not the kind of token its
    /// provider issues, or that was issued by someone other than the provider it is
    /// presented under, authorizes nothing whatever the pool trusts.
    ///
    /// The reading is [`::service_common::jwt::read`], the tree's one bounded reading of a
    /// compact JWS, through the reader a service presented ANOTHER party's token calls: a
    /// value that is no JSON Web Token, or one nothing signed, is no identity token however
    /// the provider would have decided it. The signature itself is not verified — that needs
    /// the issuer's published verification key, which this emulator makes no outbound
    /// connection to fetch — so what is decided here is decided from the presented value
    /// alone, and only ever to REFUSE it.
    pub fn refuse_logins(
        &self,
        registered: &RegisteredOidcProviders,
        integrated: &IntegratedUserPools,
        logins: &HashMap<String, String>,
    ) -> Option<LoginProviderRefusal> {
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        presented
            .iter()
            .find_map(|(provider_name, token)| self.refuse_presented_token(provider_name, token))
            .or_else(|| self.login_trust(registered).refuse(logins))
            .or_else(|| {
                presented.iter().find_map(|(provider_name, _)| {
                    self.refuse_server_side_token_check(integrated, provider_name)
                })
            })
    }

    /// Why the login presented under `provider_name` is refused by the USER POOL that
    /// provider names, if so.
    ///
    /// "Once you set `ServerSideTokenCheck` to TRUE for an identity pool, that identity pool
    /// will check with the integrated user pools to make sure that the user has not been
    /// globally signed out or deleted before the identity pool provides an OIDC token or
    /// Amazon Web Services credentials for the user. If the user is signed out or deleted,
    /// the identity pool will return a 400 Not Authorized error"
    /// (`smithy_query(query="doc",shape="CognitoIdentityProvider",member="ServerSideTokenCheck")`).
    /// So for a provider entry that asks for the check, the token is only as good as the
    /// user pool still says it is ([`IntegratedUserPools`]): a user that pool has signed
    /// out, deactivated or deleted leaves the token it minted standing for nobody, and the
    /// login is refused however well formed and however trusted its issuer is.
    ///
    /// An entry that does not ask for the check is decided by its issuer alone, and so is
    /// one whose user pool this emulator does not hold: there is no integrated pool to check
    /// WITH, so nothing about the token is decided here.
    fn refuse_server_side_token_check(
        &self,
        integrated: &IntegratedUserPools,
        provider_name: &str,
    ) -> Option<LoginProviderRefusal> {
        if !self.checks_tokens_server_side(provider_name) {
            return None;
        }
        matches!(
            integrated.presented_token_user(provider_name),
            Some(PresentedTokenUser::Inactive)
        )
        .then_some(LoginProviderRefusal::InactiveToken)
    }

    /// Whether this pool's `CognitoIdentityProviders` entry for the issuer `provider_name`
    /// names asks for server-side token validation — `ServerSideTokenCheck` is "TRUE if
    /// server-side token validation is enabled for the identity provider's token" (this
    /// service's model), and a pool may name one user pool in more than one entry, so the
    /// check is on where ANY entry for that issuer asks for it.
    ///
    /// The member is optional and the validation is what a caller opts IN to, so an entry
    /// that stored no value for it asks for none.
    pub fn checks_tokens_server_side(&self, provider_name: &str) -> bool {
        let issuer = login_provider_issuer(provider_name);
        self.cognito_identity_providers
            .iter()
            .flatten()
            .filter(|provider| {
                provider
                    .provider_name
                    .as_deref()
                    .map(login_provider_issuer)
                    .is_some_and(|named| named == issuer)
            })
            .any(|provider| provider.server_side_token_check == Some(true))
    }

    /// Why the value presented for ONE login is not the token its provider issues, if so.
    ///
    /// A provider whose logins are OpenID Connect identity tokens
    /// ([`IdentityPool::presents_identity_token`]) has to be presented with one: a value
    /// whose `iss` claim — "the principal that issued the JWT" (RFC 7519 §4.1.1), which an
    /// OpenID Connect provider's URL is — names an issuer other than the provider the entry
    /// is made under, and a value that is no such token at all, are each refused. The two
    /// names are compared like for like, as the issuer form a provider name carries
    /// ([`login_provider_issuer`]).
    ///
    /// The ISSUER is decided first, and from the claim set alone
    /// ([`::service_common::jwt::read_unsecured`], the reading that leaves the signature
    /// judgement out): a value that says who issued it is answered for the issuer it names
    /// whatever else is wrong with it. What is left — a value carrying no readable claim set,
    /// or one nothing signed ([`::service_common::jwt::read`], the reading a service
    /// presented ANOTHER party's token makes) — is no identity token. A claim set carrying
    /// no `iss` at all is not refused for the issuer: there is none to compare, and what
    /// such a token says is the issuing provider's to decide.
    ///
    /// The signature itself is never verified — that needs the issuer's published
    /// verification key, which this emulator makes no outbound connection to fetch — so what
    /// is decided here is decided from the presented value alone, and only ever to REFUSE
    /// it.
    fn refuse_presented_token(
        &self,
        provider_name: &str,
        token: &str,
    ) -> Option<LoginProviderRefusal> {
        if !self.presents_identity_token(provider_name) {
            return None;
        }
        let claimed_issuer = ::service_common::jwt::read_unsecured(token)
            .ok()
            .and_then(|read| read.claim_str(ISSUER_CLAIM).map(ToString::to_string));
        if let Some(issuer) = claimed_issuer {
            if !names_the_same_issuer(provider_name, &issuer) {
                return Some(LoginProviderRefusal::IssuerMismatch);
            }
        }
        ::service_common::jwt::read(token)
            .err()
            .map(|_| LoginProviderRefusal::NotAnIdentityToken)
    }
}

/// The ISSUER an identity pool's `OpenIdConnectProviderARNs` entry names: the resource
/// portion of an IAM OpenID Connect provider ARN
/// (`arn:<partition>:iam::<account>:oidc-provider/<url without its scheme>`) IS the
/// provider's URL without its scheme, which is the issuer form a `Logins` provider name
/// carries.
///
/// Read from the ARN rather than from the provider it resolves to, because the pool stores
/// the ARN and nothing else: an ARN whose provider the account no longer holds still names
/// the issuer the pool trusts, and what it resolves to decides only which audiences that
/// trust accepts. `None` for a value that names no OIDC provider at all.
///
/// The ARN is read through [`::service_common::arn`], the tree's one parse of an ARN.
pub fn oidc_provider_arn_issuer(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != IAM_ARN_NAMESPACE {
        return None;
    }
    let resource = parsed
        .resource
        .strip_prefix(OIDC_PROVIDER_ARN_RESOURCE_PREFIX)?;
    let issuer = login_provider_issuer(resource);
    (!issuer.is_empty()).then_some(issuer)
}

/// Whether an OIDC provider ARN names a provider of the ACCOUNT this emulator serves, and
/// so one whose IAM registration — the client ids it accepts — this emulator can resolve at
/// all. An ARN of any other account names a registration held somewhere this emulator does
/// not reach.
pub fn oidc_provider_arn_is_of_this_account(arn: &str) -> bool {
    ::service_common::arn::parse(arn)
        .is_some_and(|parsed| parsed.account == ::engine::emulator_account_id())
}

/// The ARN namespace of IAM, whose resources an OIDC provider ARN names.
const IAM_ARN_NAMESPACE: &str = "iam";

/// The ISSUER a login provider name or an identity provider's URL names: the host-and-path
/// form both carry — a `Logins` provider name is written that way (`GetId.Logins`) and an
/// IAM OIDC provider's URL "should correspond to the iss claim in the provider's OpenID
/// Connect ID tokens" and is stored without its scheme — so the two are compared like for
/// like.
///
/// The derivation is STS's own ([`::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::web_identity_provider_name`]),
/// called rather than copied: it is this tree's one dropping of a token issuer's scheme,
/// and the service that redeems a web identity has to read an issuer the same way the
/// service that federates one does.
pub fn login_provider_issuer(name: &str) -> &str {
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::web_identity_provider_name(
        name,
    )
}

/// One IAM OpenID Connect identity provider the account holds, reduced to what deciding a
/// login against it needs.
#[derive(Debug, Clone)]
pub struct RegisteredOidcProvider {
    /// The client ids it was registered with — "a list of client IDs (also known as
    /// audiences) that are associated with the specified IAM OIDC provider resource object"
    /// (`iam:GetOpenIDConnectProvider$ClientIDList`).
    pub client_ids: Vec<String>,
}

/// The IAM OpenID Connect identity providers the ACCOUNT holds, keyed by ARN, as a snapshot
/// taken before this service's own store is locked ([`registered_oidc_providers`]).
///
/// An identity pool names such a provider by ARN and nothing else, so what the ARN resolves
/// to is IAM's record rather than a second copy of it kept here: registering, deleting or
/// re-registering a provider, or adding a client id to one, changes what the pools naming it
/// admit, with no write to this service's state at all.
#[derive(Debug, Default, Clone)]
pub struct RegisteredOidcProviders {
    providers: BTreeMap<String, RegisteredOidcProvider>,
}

impl RegisteredOidcProviders {
    /// The provider `arn` names, or `None` when the account holds none for it — which is
    /// the answer for an ARN whose provider was deleted, and for one that never named a
    /// provider at all.
    pub fn registered(&self, arn: &str) -> Option<&RegisteredOidcProvider> {
        self.providers.get(arn)
    }
}

/// The IAM OpenID Connect providers the account holds, read through IAM's OWN state
/// accessor in the same engine, so a provider `iam:CreateOpenIDConnectProvider` wrote is the
/// very record a login is decided against.
///
/// A SNAPSHOT, read before a handler locks this service's store, so no request holds two
/// services' stores at once. Empty for an engine carrying no IAM store, and for an account
/// that has registered no provider: either way no identity pool's OIDC provider ARN resolves.
pub fn registered_oidc_providers<Eng: Send + Sync + 'static>(engine: &Eng) -> RegisteredOidcProviders {
    let Some(iam) = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(
        engine,
    ) else {
        return RegisteredOidcProviders::default();
    };
    let iam = iam.lock();
    RegisteredOidcProviders {
        providers: iam
            .open_id_connect_providers
            .iter()
            .map(|(arn, record)| {
                (
                    arn.clone(),
                    RegisteredOidcProvider {
                        client_ids: record.client_id_list.clone(),
                    },
                )
            })
            .collect(),
    }
}

/// What an INTEGRATED Amazon Cognito user pool says about a user-pool token one request
/// presents: the answer that pool gives for each `Logins` entry made under a user pool's
/// provider name, keyed by that provider name (the `Logins` member is a map, so one
/// provider carries one login per request).
///
/// A SNAPSHOT, read through the user-pools service's own state accessor in the same engine
/// before a handler locks this service's store, so no request holds two services' stores at
/// once and the user a token stands for is the one that service holds rather than a second
/// copy of it kept here. Empty for an engine carrying no user-pool store, and for a request
/// presenting no user-pool login: either way no entry is decided by an integrated pool.
#[derive(Debug, Default, Clone)]
pub struct IntegratedUserPools {
    presented: BTreeMap<String, PresentedTokenUser>,
}

impl IntegratedUserPools {
    /// What the user pool `provider_name` names says about the token presented under it, or
    /// `None` when no integrated pool answered for that entry at all — an entry of some
    /// other kind of provider, and one naming a user pool this emulator does not hold.
    pub fn presented_token_user(&self, provider_name: &str) -> Option<PresentedTokenUser> {
        self.presented.get(provider_name).copied()
    }
}

/// What the account's Amazon Cognito user pools say about the user-pool tokens `logins`
/// presents ([`IntegratedUserPools`]), read through the user-pools service's OWN state
/// accessor in the same engine: the user a token stands for, and whether that pool has
/// signed it out, is that service's record.
///
/// Only an entry whose provider name names a user pool
/// ([`names_cognito_user_pool`]) is asked about — every other provider is decided by the
/// issuer that issued its credential — and the pool asked is the one the provider name
/// names, which carries the pool's id ([`cognito_user_pool_named`]).
pub fn user_pool_logins<Eng: Send + Sync + 'static>(
    engine: &Eng,
    logins: &HashMap<String, String>,
) -> IntegratedUserPools {
    let Some(user_pools) =
        ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state::service_state(
            engine,
        )
    else {
        return IntegratedUserPools::default();
    };
    let user_pools = user_pools.lock();
    let mut presented = BTreeMap::new();
    for (provider_name, token) in logins {
        let Some((_, user_pool_id)) = cognito_user_pool_named(login_provider_issuer(provider_name))
        else {
            continue;
        };
        presented.insert(
            provider_name.clone(),
            user_pools.presented_token_user(user_pool_id, token),
        );
    }
    IntegratedUserPools { presented }
}

/// The audiences the trusts an identity pool states for ONE issuer accept.
#[derive(Debug, Default, Clone)]
struct TrustedAudiences {
    /// Whether some trust for this issuer states no audience constraint at all. Such a
    /// trust names no audience to compare a token against, so it accepts a token issued for
    /// any.
    any: bool,
    /// The client ids the audience-constrained trusts for this issuer accept.
    recorded: Vec<String>,
}

impl TrustedAudiences {
    /// Add one more trust for this issuer: `constrains` says whether it states an audience
    /// constraint at all, and `client_ids` are the audiences it accepts — which an
    /// audience-constrained trust may hold none of, in which case it accepts none.
    fn accept<I: IntoIterator<Item = String>>(&mut self, constrains: bool, client_ids: I) {
        for client_id in client_ids {
            if client_id.is_empty() {
                continue;
            }
            if !self.recorded.contains(&client_id) {
                self.recorded.push(client_id);
            }
        }
        self.any |= !constrains;
    }
}

/// What an identity pool's stored configuration decides about the `Logins` of one request:
/// the issuers it trusts, with the audiences each of those trusts accepts
/// ([`IdentityPool::login_trust`]).
#[derive(Debug, Default, Clone)]
pub struct PoolLoginTrust {
    issuers: BTreeMap<String, TrustedAudiences>,
    /// Whether the pool names no identity provider of any kind
    /// ([`IdentityPool::configures_no_identity_provider`]) — which is what an issuer the
    /// trust holds no entry for is refused on, where it is refused at all.
    configures_no_provider: bool,
}

impl PoolLoginTrust {
    /// Why the `Logins` map of a request authorizes nothing against this trust, if so.
    ///
    /// The entries are decided in a stable order, so a map that breaks the rule in two
    /// places is refused the same way every time.
    pub fn refuse(&self, logins: &HashMap<String, String>) -> Option<LoginProviderRefusal> {
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        presented
            .into_iter()
            .find_map(|(provider_name, token)| self.refuse_login(provider_name, token))
    }

    /// Why one presented login authorizes nothing, if so.
    ///
    /// An issuer the pool states a trust for admits the login only where the token was
    /// issued for an audience that trust accepts: the `aud` claim of an OpenID Connect ID
    /// token names the client it was issued for, and a client id the trust does not hold is
    /// one the pool never asked for tokens of — so a trust that accepts no client id at all
    /// (an `OpenIdConnectProviderARNs` entry whose provider the account no longer holds,
    /// or one registered with no client id) admits no audience either. A token carrying no
    /// readable audience is not refused here — its audience is decided by the provider that
    /// issued it, which this emulator does not reach.
    ///
    /// An issuer the pool states NO trust for is refused only where the issuer is one this
    /// emulator itself hosts (an Amazon Cognito user pool, whose provider name names it):
    /// any other issuer's token is decided by that issuer, so the pool's configuration
    /// alone does not refuse it.
    fn refuse_login(&self, provider_name: &str, token: &str) -> Option<LoginProviderRefusal> {
        let issuer = login_provider_issuer(provider_name);
        let Some(trusted) = self.issuers.get(issuer) else {
            if !names_cognito_user_pool(issuer) {
                return None;
            }
            // The pool holds no trust for this issuer, and WHY decides the wording: a pool
            // configured with no identity provider at all federates nobody, so what the
            // caller has to act on is that pool's configuration, while a pool whose
            // configured providers simply do not include the issuer is refused on the token
            // it presented.
            return Some(if self.configures_no_provider {
                LoginProviderRefusal::NoIdentityProviders
            } else {
                LoginProviderRefusal::UnsupportedProvider
            });
        };
        if trusted.any {
            return None;
        }
        let presented = presented_token_audiences(token);
        if presented.is_empty() {
            return None;
        }
        let accepted = presented
            .iter()
            .any(|audience| trusted.recorded.contains(audience));
        (!accepted).then_some(LoginProviderRefusal::IncorrectAudience)
    }
}

/// Why a presented `Logins` entry authorizes nothing: what the token itself says, and what
/// the identity pool's trust for its issuer says. Each is answered with the operation's
/// modelled `NotAuthorizedException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginProviderRefusal {
    /// The value presented for a provider whose logins are OpenID Connect identity tokens
    /// is not such a token at all.
    NotAnIdentityToken,
    /// It is one, and it was issued by another issuer than the provider name it is
    /// presented under.
    IssuerMismatch,
    /// The pool states no trust for the token's issuer.
    UnsupportedProvider,
    /// The pool states no trust for ANY issuer: it names no identity provider at all.
    NoIdentityProviders,
    /// It states one, and the token names an audience that trust does not accept.
    IncorrectAudience,
    /// The trust is a `CognitoIdentityProviders` entry asking for server-side token
    /// validation, and the user pool it names says the token stands for no user that still
    /// signs in: one it has signed out, deactivated or deleted.
    InactiveToken,
}

impl LoginProviderRefusal {
    /// The message the refusal carries, for a login presented to the pool
    /// `identity_pool_id` names.
    ///
    /// Only the refusal that is about the POOL's own configuration states that pool
    /// ([`no_identity_providers_message`]); every other one is about the TOKEN, and is the
    /// fixed sentence its own constant records.
    pub fn message(self, identity_pool_id: &str) -> Cow<'static, str> {
        match self {
            Self::NotAnIdentityToken => Cow::Borrowed(INVALID_OPEN_ID_CONNECT_TOKEN_MESSAGE),
            Self::IssuerMismatch => Cow::Borrowed(MISMATCHED_TOKEN_ISSUER_MESSAGE),
            Self::UnsupportedProvider => Cow::Borrowed(UNSUPPORTED_LOGIN_PROVIDER_MESSAGE),
            Self::NoIdentityProviders => {
                Cow::Owned(no_identity_providers_message(identity_pool_id))
            }
            Self::IncorrectAudience => Cow::Borrowed(INCORRECT_TOKEN_AUDIENCE_MESSAGE),
            Self::InactiveToken => Cow::Borrowed(INACTIVE_LOGIN_TOKEN_MESSAGE),
        }
    }
}

/// The claim naming "the principal that issued the JWT" (RFC 7519 §4.1.1), which an OpenID
/// Connect ID token carries the issuing provider's URL in — and which [`open_id_token`]
/// writes this service's own issuer into.
const ISSUER_CLAIM: &str = "iss";

/// The claim naming who a token was issued FOR (RFC 7519 §4.1.3), which an OpenID Connect
/// ID token carries the client id in — for a provider that issues tokens to an app client,
/// that client.
const AUDIENCE_CLAIM: &str = "aud";

/// The claim naming "the principal that is the subject of the JWT" (RFC 7519 §4.1.2), which
/// an OpenID Connect ID token carries the authenticated user's identifier in.
const SUBJECT_CLAIM: &str = "sub";

/// The provider's USER IDENTIFIER a presented login names — what an identity is indexed
/// under in a pool.
///
/// `Logins` maps a provider name to a "provider_user_identifier"
/// (`GetCredentialsForIdentity$Logins`, this service's model), and `GetId` "generates (or
/// retrieves)" the one identity of that federated user. For a provider whose login is an
/// OpenID Connect ID token, the identifier that token NAMES is its `sub` claim rather than
/// the token's own bytes: the same user signing in again — through another app client of the
/// same issuer, or after a refresh — presents another token naming the same subject, and
/// both are one federated user. A login no subject can be read out of is indexed by the
/// value as presented, which is the only identifier it carries (a developer user identifier
/// among them).
///
/// The claims are read through [`presented_token_claims`], the one reading this service
/// makes of a login ANOTHER party issued, so the identifier a login is indexed under and
/// the audience it is admitted on come from the same claim set.
pub fn login_user_identifier(login: &str) -> String {
    presented_token_claims(login)
        .and_then(|claims| {
            claims
                .get(SUBJECT_CLAIM)
                .and_then(::serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .filter(|subject| !subject.is_empty())
        .unwrap_or_else(|| login.to_string())
}

/// The audiences a presented login token names: its `aud` claim, which "MAY be a single
/// case-sensitive string" or an array of them (RFC 7519 §4.1.3), read through the same
/// [`presented_token_claims`] / [`claim_values`] pair a role mapping reads a claim with.
///
/// Empty for a value that is no identity token and for a token carrying no `aud`: there is
/// no audience to decide the login on, and nothing is granted on what is read either way.
fn presented_token_audiences(token: &str) -> Vec<String> {
    presented_token_claims(token)
        .map(|claims| claim_values(&claims, AUDIENCE_CLAIM))
        .unwrap_or_default()
}

/// The resource type of an IAM OpenID Connect provider ARN, which the issuer url it names
/// follows (`arn:aws:iam::<account>:oidc-provider/<url without its scheme>`).
const OIDC_PROVIDER_ARN_RESOURCE_PREFIX: &str = "oidc-provider/";

/// How AWS words the refusal of a login whose provider issues OpenID Connect identity
/// tokens and whose presented value is not one.
///
/// Fixed text with no request- or state-derived part: the caller of an unsigned,
/// token-authorized call holds no credential of this service, so the refusal says that the
/// value presented is no identity token and nothing about the identity, the pool or the
/// providers it is configured with.
pub const INVALID_OPEN_ID_CONNECT_TOKEN_MESSAGE: &str =
    "Invalid login token. Not a valid OpenId Connect identity token.";

/// How AWS words the refusal of an identity token whose `iss` claim names an issuer other
/// than the provider name the `Logins` entry presents it under: the entry says who issued
/// the token, and the token says so too, so a request whose two answers differ presents a
/// token of someone else's.
///
/// Fixed text, for the same reason as [`INVALID_OPEN_ID_CONNECT_TOKEN_MESSAGE`]: it names
/// neither the issuer the token claimed nor the provider it was presented under.
pub const MISMATCHED_TOKEN_ISSUER_MESSAGE: &str =
    "Invalid login token. Issuer doesn't match providerName";

/// How AWS words the refusal of an UNAUTHENTICATED call — one presenting no login at all —
/// to a pool whose `AllowUnauthenticatedIdentities` does not admit one: "identities from
/// this identity pool and the associated IAM roles can only be used with authenticated
/// identity providers" (`CreateIdentityPoolInput$AllowUnauthenticatedIdentities`, this
/// service's model).
///
/// Fixed text with no request- or state-derived part, so it names neither the pool the call
/// addressed nor the identity it was made for.
pub const UNAUTHENTICATED_ACCESS_MESSAGE: &str =
    "Unauthenticated access is not supported for this identity pool.";

/// How AWS words the refusal of an unlink of an identity no login has ever been linked to —
/// an UNAUTHENTICATED (guest) identity, which `GetId` mints for a call presenting no login
/// at all. Such an identity is reached through its id alone and has no linked login to
/// detach, so there is nothing this operation can take off it.
pub const CANNOT_UNLINK_UNAUTHENTICATED_MESSAGE: &str = "Cannot unlink unauthenticated user.";

/// How AWS words the refusal of a request that would link logins naming MORE THAN ONE
/// account: a developer user identifier and a federated login that another identity has
/// already been proven by cannot become one user by being presented together.
///
/// Fixed text, naming neither the logins presented nor the identities they resolve to: the
/// caller is told that these logins cannot be linked, which is what it has to act on.
pub const CANNOT_LINK_LOGINS_MESSAGE: &str = "Cannot link these logins.";

/// How AWS words the refusal of a merge of two developer users that cannot become one
/// user — they are reached through one public provider as two different users, so no
/// single identity can hold both.
///
/// Fixed text, naming neither the provider the two users share nor either user: what the
/// caller is told is that this pair cannot be merged, which is the whole of what it can
/// act on. Worded as [`CANNOT_LINK_LOGINS_MESSAGE`] is, the two being the same kind of
/// refusal of one combination of logins.
pub const CANNOT_MERGE_IDENTITIES_MESSAGE: &str = "Cannot merge these identities.";

/// How AWS reports a DEVELOPER USER the identity pool has registered against no identity —
/// the resource a request naming a developer user identifier addresses.
///
/// The one part taken from the request is the identifier it named, which the caller sent
/// itself, so the refusal discloses nothing the caller did not already hold; the rest is
/// fixed text. Shared, so every operation that reports a developer user missing words it
/// alike.
pub fn developer_user_not_found_message(developer_user_identifier: &str) -> String {
    format!("User '{developer_user_identifier}' not found.")
}

/// How AWS reports an IDENTITY this service holds no record of — the resource a request
/// naming an `IdentityId` addresses.
///
/// The one part taken from the request is the id it named, which the caller sent itself, so
/// the refusal discloses nothing the caller did not already hold; the rest is fixed text,
/// naming neither the identity pool the call addressed nor anything else of this account's
/// state. Shared, so every operation that reports an identity missing words it alike.
pub fn identity_not_found_message(identity_id: &str) -> String {
    format!("Identity '{identity_id}' not found.")
}

/// How AWS words the refusal of a call that names an identity the caller has not proven it
/// may act for: one already linked to a login, reached with no login of it at all.
///
/// The one part taken from the request is the `IdentityId` it named, which the caller sent
/// itself, so the refusal discloses nothing the caller did not already hold. Shared, so
/// every operation that decides access to an identity answers alike.
pub fn forbidden_identity_message(identity_id: &str) -> String {
    format!("Access to Identity '{identity_id}' is forbidden.")
}

/// How AWS words the refusal of a request naming an identity provider the identity pool's
/// own federated-provider configuration does not declare.
///
/// The one part taken from the request is the provider name it carried, which the caller
/// sent itself; the rest is fixed text, naming none of the providers the pool really holds.
pub fn provider_not_configured_message(identity_provider_name: &str) -> String {
    format!("Provider {identity_provider_name} not configured for Identity Pool")
}

/// How AWS words the refusal of a `Logins` map that names no PUBLIC provider at all — one
/// carrying only the pool's own developer provider, or nothing. Public providers are what
/// the public-provider unlink operates on; a developer login is detached through
/// `UnlinkDeveloperIdentity` instead.
pub const NO_PUBLIC_PROVIDER_MESSAGE: &str = "Please provide a valid public provider";

/// How AWS words the refusal of a request naming a developer provider that is not the one
/// its identity pool is configured with: a bad input parameter, which is what the caller
/// has to fix, rather than an access decision. Fixed text, naming neither the provider the
/// request carried nor the pool's own.
pub const UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE: &str =
    "This identity pool does not support the specified developer provider.";

/// How AWS words the refusal of a `Logins` map that carries NO entry at all where the
/// operation takes the map as the whole of who the user is: no login was supplied, which is
/// a different missing input from a map that does name a provider and names one the pool
/// does not support ([`UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE`]). Fixed text, naming
/// nothing of the request or of the pool.
pub const NO_LOGIN_PROVIDER_MESSAGE: &str = "Please specify at least one login provider";

/// How AWS words the refusal of a developer-identity lookup that names NEITHER of the two
/// members it can resolve a mapping from. Both are optional in the model
/// (`LookupDeveloperIdentityInput`), so which combinations the operation can act on is its
/// own rule; the sentence names the two members and nothing of the request.
pub const NO_IDENTITY_OR_DEVELOPER_USER_MESSAGE: &str =
    "Please specify either an identity id or developer user identifier";

/// How AWS words the refusal of a developer-user-identifier / `IdentityId` PAIR the pool's
/// mappings do not link — "If you supply both, DeveloperUserIdentifier will be matched
/// against IdentityID [...] Otherwise, a ResourceConflictException is thrown". Fixed text,
/// naming neither value, so a caller is told the pair does not match without being told what
/// either side of it does hold.
pub const MISMATCHED_DEVELOPER_USER_PAIR_MESSAGE: &str =
    "The provided user identifier and identity id do not match.";

/// Why a pool does not serve the basic (classic) authentication flow, whose entry point is
/// `GetOpenIdToken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicFlowRefusal {
    /// The pool's `AllowClassicFlow` disables the flow.
    Disabled,
    /// The pool's roles are chosen by a ROLE MAPPING, which only the enhanced flow
    /// resolves.
    RoleMappings,
}

impl ClassicFlowRefusal {
    /// How AWS words this refusal: a fixed sentence per reason, naming what about the pool
    /// closes the flow and pointing the caller at the flow that pool does serve. Neither
    /// carries a part taken from the request or from stored state — what is wrong is the
    /// flow the call is in, not any value it carried.
    pub fn message(self) -> &'static str {
        match self {
            Self::Disabled => "Basic (classic) flow is not enabled, please use enhanced flow.",
            Self::RoleMappings => {
                "Basic (classic) flow is not supported with RoleMappings, please use enhanced \
                 flow."
            }
        }
    }
}

/// Why `pool`, configured with `roles`, does not serve the basic (classic) authentication
/// flow — and `None` when it does.
///
/// Two things close that flow, and both are the pool's own stored configuration:
///
/// * `AllowClassicFlow` "enables or disables the Basic (Classic) authentication flow"
///   (`smithy_query(query="doc",shape="CreateIdentityPoolInput",member="AllowClassicFlow")`),
///   so a pool that stored `false` for it forbids the flow outright;
/// * a ROLE MAPPING picks the role for an identity from the claims of the token the exchange
///   presents, which the SERVICE does as it vends — the enhanced flow. In the basic flow the
///   client assumes a role of its own choosing, so a token minted for it would bypass the
///   mapping altogether, and a pool whose roles are mapped does not serve that flow.
///
/// A pool that stored no value for the flag has made no such choice, and this service
/// reports exactly what was stored, so the absent flag alone does not close the flow.
pub fn classic_flow_refusal(
    pool: &IdentityPool,
    roles: Option<&IdentityPoolRoles>,
) -> Option<ClassicFlowRefusal> {
    if pool.allow_classic_flow == Some(false) {
        return Some(ClassicFlowRefusal::Disabled);
    }
    if roles.is_some_and(IdentityPoolRoles::has_role_mappings) {
        return Some(ClassicFlowRefusal::RoleMappings);
    }
    None
}

/// Whether the issuer a presented token CLAIMS and the `Logins` provider name it was
/// presented under name the same issuer.
///
/// The two are compared as the issuer form both carry ([`login_provider_issuer`]). A
/// provider name that names an Amazon Cognito USER POOL
/// ([`cognito_user_pool_named`]) is matched on the POOL instead of on one spelling of
/// its endpoint: the name is the published
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` form, while a pool stamps into `iss`
/// the endpoint the sign-in that minted the token reached it on, so which issuers a pool
/// publishes is the user-pool service's own reading
/// (`UserPool`'s `is_pool_issuer`) rather than a second copy of the issuer form here.
fn names_the_same_issuer(provider_name: &str, claimed: &str) -> bool {
    let named = login_provider_issuer(provider_name);
    if login_provider_issuer(claimed) == named {
        return true;
    }
    cognito_user_pool_named(named).is_some_and(|(region, user_pool_id)| {
        ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
            ::state::is_pool_issuer(region, user_pool_id, claimed)
    })
}

/// The region and user-pool id a user-pool provider name states, read out of the
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` form `GetId.Logins` publishes for one
/// ([`names_cognito_user_pool`]), and `None` for a name of any other shape.
fn cognito_user_pool_named(provider_name: &str) -> Option<(&str, &str)> {
    let rest = provider_name.strip_prefix(COGNITO_USER_POOL_PROVIDER_PREFIX)?;
    let (host_tail, user_pool_id) = rest.split_once('/')?;
    let region = host_tail.strip_suffix(COGNITO_USER_POOL_PROVIDER_SUFFIX)?;
    (!region.is_empty() && !user_pool_id.is_empty() && !user_pool_id.contains('/'))
        .then_some((region, user_pool_id))
}

/// Whether `provider_name` names an Amazon Cognito user pool: the
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` form `GetId.Logins` publishes for
/// one ("Amazon Cognito user pool: `cognito-idp.<region>.amazonaws.com/<YOUR_USER_POOL_ID>`",
/// this service's model). The name itself carries the issuer, which is what makes the
/// pool's stored configuration able to decide whether the login is one it accepts.
pub fn names_cognito_user_pool(provider_name: &str) -> bool {
    provider_name
        .strip_prefix(COGNITO_USER_POOL_PROVIDER_PREFIX)
        .is_some_and(|rest| rest.contains('/'))
}

/// The prefix of every Amazon Cognito user-pool provider name: the user-pools service's
/// own endpoint prefix, which every such provider name starts with.
const COGNITO_USER_POOL_PROVIDER_PREFIX: &str = "cognito-idp.";

/// What follows the region in a user-pool provider name's host, so that the region itself
/// is what lies between this and [`COGNITO_USER_POOL_PROVIDER_PREFIX`].
const COGNITO_USER_POOL_PROVIDER_SUFFIX: &str = ".amazonaws.com";

/// The message a `NotAuthorizedException` carries for a `Logins` entry whose provider is
/// not one the identity pool accepts.
///
/// One fixed sentence, naming neither the provider the request presented nor the
/// providers the pool is configured with: the caller of this refusal is an
/// UNAUTHENTICATED one — the credential-exchange operations are the calls a client makes
/// before it holds any credential — so a message built from the pool's configuration
/// would report that configuration to anyone who asked with a token the pool rejects.
/// It states that the token's provider is not supported here, which is what a client has
/// to act on.
pub const UNSUPPORTED_LOGIN_PROVIDER_MESSAGE: &str =
    "Token is not from a supported provider of this identity pool.";

/// How AWS words the refusal of a login presented to an identity pool that is configured
/// with no identity provider at all ([`IdentityPool::configures_no_identity_provider`]).
///
/// Its one variable part is the pool the call addressed, which the caller itself named (the
/// `IdentityPoolId` of the request, or the pool the `IdentityId` it presented belongs to) —
/// so unlike [`UNSUPPORTED_LOGIN_PROVIDER_MESSAGE`], which withholds the pool's provider
/// configuration from an unauthenticated caller, this one discloses nothing the caller did
/// not already have: WHICH pool is unconfigured is the one thing it has to act on, and a
/// pool with no providers has no configuration to report.
pub fn no_identity_providers_message(identity_pool_id: &str) -> String {
    format!("Identity pool - {identity_pool_id} does not have identity providers configured.")
}

/// The message a `NotAuthorizedException` carries for a `Logins` entry whose token names an
/// audience the pool's trust for that token's issuer does not accept — a client id that is
/// in none of the trust's registrations.
///
/// One fixed sentence, as [`UNSUPPORTED_LOGIN_PROVIDER_MESSAGE`] is and for the same reason:
/// the caller holds no credential, so the refusal names neither the audience the token
/// carried nor the client ids the pool's providers are registered with. It states that the
/// token was issued for the wrong audience, which is what a client has to act on — the one
/// thing it can fix, by presenting a token of a client the provider is registered for or by
/// registering that client with the provider.
pub const INCORRECT_TOKEN_AUDIENCE_MESSAGE: &str =
    "Invalid login token. Incorrect token audience.";

/// The message a `NotAuthorizedException` carries for a login the INTEGRATED user pool no
/// longer stands behind: a pool entry asking for server-side token validation, whose user
/// pool reports the token's user signed out, deactivated or deleted
/// ([`IdentityPool::refuse_server_side_token_check`]).
///
/// One fixed sentence, as the refusals above are and for the same reason — the caller holds
/// no credential — naming neither the user pool consulted nor what it said about the user:
/// what a client has to act on is that the token it presented is no longer live, which is
/// the one thing it can fix, by signing in again.
pub const INACTIVE_LOGIN_TOKEN_MESSAGE: &str = "Token is inactive";

/// The message a `NotAuthorizedException` carries for a request naming a developer-identity
/// mapping the identity pool does not hold: a developer user identifier it has indexed
/// against no identity, under a developer provider that is not the pool's own, or in a pool
/// that names no developer provider at all.
///
/// Resolving a developer user identifier is an authorization question rather than the
/// lookup of a resource the caller already names: the identifier is a LOGIN of the pool, so
/// a request carrying one the pool holds no mapping for is answered as access the caller
/// does not have. One fixed sentence, naming neither the identifier nor any identity, so
/// nothing is disclosed about what the pool holds for it — and shared, so every operation
/// that resolves one answers alike.
pub const DEVELOPER_IDENTIFIER_FORBIDDEN_MESSAGE: &str = "You do not have access to this identity";

/// The most tags one identity pool can carry, from the model's own doc for `TagResource`:
/// "An identity pool can have as many as 50 tags"
/// (`smithy_query(query="doc",shape="TagResource")`). An AWS-published ceiling on the
/// resource, so it bounds the tag set a pool ends up with however the tags got there, and
/// a call that would take a pool past it is refused with the modelled
/// `InvalidParameterException` — the only client-input error the tagging operations model
/// for a quota of the resource.
pub const MAX_TAGS_PER_IDENTITY_POOL: usize = 50;

/// How AWS words the refusal of a tag write that would take a pool past that ceiling: it
/// names the ceiling ([`MAX_TAGS_PER_IDENTITY_POOL`], the published limit) and says that the
/// RESULTING tag set is what breaks it, which is what makes re-assigning keys the pool
/// already holds admissible however many the call carries.
///
/// Its one variable part is the limit itself; it names neither the pool the call addressed
/// nor how many tags that pool would have ended up with, so nothing of this account's state
/// is in it.
pub fn too_many_user_tags_message() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_IDENTITY_POOL} user tags.")
}

/// The tag-key prefix AWS reserves for its own tags. AWS's tagging rules state that the
/// `aws:` prefix is reserved for AWS use and that a user cannot create, edit or delete a
/// tag whose key carries it, so a request assigning such a key is a bad input parameter
/// rather than a tag this service stores.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How AWS words the refusal of a request that would MUTATE a tag lying in the namespace it
/// reserves for its own ([`RESERVED_TAG_KEY_PREFIX`]) — assigning one, or removing one.
///
/// Fixed text with no request- or state-derived part: the reservation is about WHO the
/// caller is (an end user rather than AWS itself), so the refusal says that and names
/// neither the key the request carried nor the resource it addressed. Shared by every
/// operation of this service that writes a resource's tag set, so they answer alike.
pub const SYSTEM_TAG_MUTATION_MESSAGE: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// How AWS words the refusal of a `ResourceArn` that is not an ARN of a resource of this
/// service: the ARN is read by the authorization layer in front of the operation, before
/// any resource is looked up, and that layer reports that the ARN it was handed failed its
/// validation.
///
/// Fixed text, naming nothing of the value it was handed — the ARN is the caller's own
/// input and the refusal adds nothing by echoing it. Shared by every operation of this
/// service that takes a `ResourceArn`, because the layer that reads it is the same one.
pub const ARN_VALIDATION_FAILED_MESSAGE: &str = "ARN in SecureServiceAuthStrategy failed validation:";

/// Whether `tag_key` lies in the AWS-reserved tag namespace ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// The prefix is matched case-insensitively — the reservation is on the namespace, not on
/// one spelling of it — and on BYTES, so a key whose first characters are multi-byte is
/// compared without slicing inside a character.
pub fn is_reserved_tag_key(tag_key: &str) -> bool {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    let key = tag_key.as_bytes();
    key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The MODELLED length bounds of an identity-pool tag KEY, `TagKeysType` (`len 1..128`):
/// the shape both the tag map's key (`IdentityPoolTagsType`) and the untag list's member
/// (`IdentityPoolTagsListType`) target
/// (`smithy_query(query="shape",shape="IdentityPoolTagsType,IdentityPoolTagsListType")`).
/// Held here so every operation that reads a tag key checks the one bound the model states.
pub const TAG_KEY_LENGTH: ::std::ops::RangeInclusive<usize> = 1..=128;

/// The MODELLED length bounds of a tag VALUE, `TagValueType` (`len 0..256`), from the same
/// query.
pub const TAG_VALUE_LENGTH: ::std::ops::RangeInclusive<usize> = 0..=256;

/// Whether `value` lies inside `bounds`, counted in CHARACTERS, as the `length` trait
/// measures a string.
pub fn length_is_within(value: &str, bounds: &::std::ops::RangeInclusive<usize>) -> bool {
    bounds.contains(&value.chars().count())
}

/// The one message a constraint refusal carries: the envelope AWS wraps modelled-constraint
/// violations in, stating how many it DETECTED and naming the ones in `violations`. Shared
/// by every operation of this service that refuses a value the model bounds, so the refusals
/// read alike however many violations one request carries.
///
/// `detected` is the number of violations the request has, which is what AWS's envelope
/// counts; `violations` are the clauses this refusal names, so a caller reading them against
/// the count can see when a refusal names fewer clauses than the request has violations.
pub fn validation_errors_detected(detected: usize, violations: &[String]) -> String {
    format!(
        "{detected} validation error{} detected: {}",
        if detected == 1 { "" } else { "s" },
        violations.join("; ")
    )
}

/// The refusal of a MAP member whose KEY is outside the constraints its key shape declares:
/// the framework's envelope and clause over the map MEMBER, quoting the map the request
/// sent and listing what a key of it has to satisfy.
///
/// A map's key is the one modelled constraint the validating walk ahead of a handler does
/// not report — it treats a key as a path segment rather than as a constrained leaf — so the
/// HANDLER applies the key shape's own bounds and words the refusal with this. Both halves
/// are the shared renderings (`service_common::validation_message::map_key_refusal_quoting`
/// for the sentence and `service_common::validation::rendered_map` for the value), so a
/// clause a handler of this service words and one restored from the walk state one map the
/// same way.
///
/// `key_length` is the key shape's declared `@length` and `key_pattern` its declared
/// `@pattern`, unanchored as a refusal of this service quotes an expression
/// ([`crate::service::validation`]) — and `None` for a key shape that declares none.
pub fn map_key_constraint_refusal(
    member_path: &str,
    submitted: &BTreeMap<String, String>,
    key_length: &::std::ops::RangeInclusive<usize>,
    key_pattern: Option<&str>,
) -> String {
    let quoted = rendered_map(submitted);
    ::service_common::validation_message::map_key_refusal_quoting(
        member_path,
        Some(&quoted),
        *key_length.start() as i64,
        *key_length.end() as i64,
        key_pattern,
    )
}

/// `submitted` as a constraint refusal quotes a map: `{<key>=<value>, …}`, through the one
/// renderer of that form (`service_common::validation::rendered_map`). The entries are in
/// the order the table holding them is walked
/// (`service_common::submitted_collections::hashed_entry_order`), which is the order every
/// clause of this service states a map in ([`crate::service::validation`]).
///
/// Two keys that share a bucket are stated in KEY order, which is what the entries of one
/// bucket carry wherever this order is stated: the rule reads nothing but the keys, so a
/// clause a handler words about the map it holds and one the boundary walk's refusal states
/// about the same map read back from the request are the same text.
pub fn rendered_map(submitted: &BTreeMap<String, String>) -> String {
    let entries: Vec<(&str, &str)> = submitted
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    ::service_common::validation::rendered_map(
        &::service_common::submitted_collections::hashed_entry_order(&entries),
    )
}

/// The punctuation an AWS tag key admits beside letters, digits and separators. AWS's
/// tagging rules publish the character set of a tag key as letters, numbers, spaces
/// representable in UTF-8 and `_ . : / = + - @`; a principal tag is referenced from an IAM
/// policy as a session tag of the federated principal, so a key outside that set names a
/// tag no policy could ever reference.
pub const TAG_KEY_PUNCTUATION: &str = "_.:/=+-@";

/// Whether `tag_key` is spelled with the characters a tag key admits
/// ([`TAG_KEY_PUNCTUATION`] beside letters, digits and separator whitespace, in any
/// script). The emptiness and the length of a key are the MODELLED bounds of the shape it
/// is written through, checked where it is read; this is the character rule alone.
pub fn is_well_formed_tag_key(tag_key: &str) -> bool {
    is_well_formed_tag_text(tag_key)
}

/// Whether `tag_text` — a tag KEY or a tag VALUE — is spelled with the characters a tag
/// admits ([`TAG_KEY_PUNCTUATION`] beside letters, digits and separator whitespace, in any
/// script).
///
/// AWS's tagging rules publish ONE character set for a tag, which both halves of it are
/// written in, so the key rule ([`is_well_formed_tag_key`]) and the value rule
/// ([`pool_configuration::tag_charset_refusal`]) are this one function rather than two
/// readings that could drift. The emptiness and the length of either half are the MODELLED
/// bounds of the shape it is written through, checked where it is read; this is the
/// character rule alone.
pub fn is_well_formed_tag_text(tag_text: &str) -> bool {
    tag_text.chars().all(|character| {
        character.is_alphanumeric()
            // A separator, which is what AWS's rule admits as a "space": a control
            // character is whitespace to Rust but is not a separator.
            || (character.is_whitespace() && !character.is_control())
            || TAG_KEY_PUNCTUATION.contains(character)
    })
}

/// The most providers of ONE pool this emulator holds a principal-tag attribute map for.
/// Not an AWS quota: THIS emulator's own ceiling on a store that grows by one record per
/// `SetPrincipalTagAttributeMap` call naming a provider it has not seen before — the
/// provider name is a free-form caller string — and whose records live for the process. Set
/// to the same order as the other per-request ceilings here, well above the providers one
/// pool can configure, so a legitimate caller never meets it, while a caller varying the
/// name cannot grow the store without limit. A call that would create a record past it is
/// refused with the modelled `InvalidParameterException`; replacing the mapping of a
/// provider already stored is never refused for it.
pub const MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL: usize = 50;

/// The principal-tag attribute map of ONE identity provider of one identity pool, as
/// `SetPrincipalTagAttributeMap` wrote it and `GetPrincipalTagAttributeMap` reports it.
/// A set call REPLACES the provider's whole mapping, so one record holds everything that
/// mapping consists of.
#[derive(Debug, Clone, Default)]
pub struct PrincipalTagAttributeMap {
    /// Whether the call asked for the default (username and clientID) attribute mappings.
    pub use_defaults: bool,
    /// Principal tag key -> the user attribute (claim) it is taken from, ordered so a read
    /// reports the same map twice. Empty when the call carried no `PrincipalTags`.
    pub principal_tags: BTreeMap<String, String>,
}

/// The DEFAULT principal-tag attribute map, which a call asking for the default mappings
/// configures: the two tags a provider's users are mapped by when the call names no
/// attributes of its own — `username` and `client` — each taken from the claim an issued
/// OpenID Connect token carries it in, the subject (`sub`), which names the identity, and the
/// audience (`aud`), which names what the token was issued to (see [`open_id_token`]).
///
/// Those two tag KEYS are the ones AWS's own documentation of attributes for access control
/// publishes for the default mappings — a policy references the vended session tags as
/// `aws:PrincipalTag/username` and `aws:PrincipalTag/client` — and they are what a read of a
/// default-mapped provider reports. `SetPrincipalTagAttributeMap`'s own doc names the same
/// pair in prose, as the "default (username and clientID) attribute mappings"
/// (`smithy_query(query="doc",shape="SetPrincipalTagAttributeMap")`); that sentence describes
/// the two attributes rather than spelling the tag keys, so the keys are taken from the
/// published mapping above.
///
/// Choosing the defaults is a choice INSTEAD of a custom mapping, so this map is the
/// provider's whole mapping rather than something merged into one.
pub const DEFAULT_PRINCIPAL_TAGS: [(&str, &str); 2] = [("client", "aud"), ("username", "sub")];

/// [`DEFAULT_PRINCIPAL_TAGS`] as a stored mapping.
pub fn default_principal_tags() -> BTreeMap<String, String> {
    DEFAULT_PRINCIPAL_TAGS
        .iter()
        .map(|(tag_key, attribute)| (tag_key.to_string(), attribute.to_string()))
        .collect()
}

/// The most mapping rules one identity provider may carry, from the model's own doc for
/// `RulesConfigurationType$Rules`: "An array of rules. You can specify up to 25 rules per
/// identity provider." (`smithy_query(query="shape",shape="RulesConfigurationType")`).
/// The modelled `length 1..400` on the list is wider, so the documented per-provider
/// ceiling is the service's own rule and is enforced where the rules are written.
pub const MAX_RULES_PER_IDENTITY_PROVIDER: usize = 25;

/// The role configuration of one identity pool, as `SetIdentityPoolRoles` wrote it and
/// `GetIdentityPoolRoles` reports it. `SetIdentityPoolRoles` replaces the whole
/// configuration, so one record holds everything a pool's roles consist of.
#[derive(Debug, Clone, Default)]
pub struct IdentityPoolRoles {
    /// `authenticated` / `unauthenticated` -> role ARN, keyed by the modelled `RoleType`.
    /// Ordered, so a read reports the same map twice.
    pub roles: BTreeMap<String, String>,
    /// Identity-provider name -> its role mapping. `None` when the call that wrote this
    /// record carried no `RoleMappings`, so a read reports exactly what was stored rather
    /// than an empty map the caller never sent.
    pub role_mappings: Option<BTreeMap<String, RoleMapping>>,
}

impl IdentityPoolRoles {
    /// Whether this configuration maps any identity provider to a role.
    ///
    /// A role MAPPING is resolved by the service as it vends — it reads the claims of the
    /// token the exchange presents and picks the role its rules name — so it is a
    /// configuration only the enhanced flow can honour.
    pub fn has_role_mappings(&self) -> bool {
        self.role_mappings
            .as_ref()
            .is_some_and(|role_mappings| !role_mappings.is_empty())
    }

    /// The role a vend assumes when the request names no `CustomRoleArn`: this pool's
    /// `authenticated` role for a request that presented a login, its `unauthenticated`
    /// role for one that did not. `None` when the pool configured no role for that flow,
    /// which is a pool that can vend no session at all for it.
    pub fn role_for(&self, authenticated: bool) -> Option<&str> {
        let role_type = if authenticated {
            AUTHENTICATED_ROLE_TYPE
        } else {
            UNAUTHENTICATED_ROLE_TYPE
        };
        self.roles.get(role_type).map(String::as_str)
    }

    /// What the IDENTITY PROVIDER's own mapping decides for an exchange presenting `logins`
    /// against `pool`.
    ///
    /// A `RoleMappings` entry is how a pool has the SERVICE pick the role from the token
    /// the exchange presents, so it decides the vend before the pool-wide
    /// `authenticated`/`unauthenticated` role does ([`IdentityPoolRoles::role_for`], which
    /// is the fallback). The entry that decides is the one keyed by the identity provider
    /// the login was made under ([`mapping_key_names_provider`]), and what it resolves is
    /// read from that token's claims ([`mapping_resolved_role_arn`]).
    ///
    /// A mapping that decides the exchange and resolves NO role is the case its
    /// `AmbiguousRoleResolution` states the outcome of — "Specifies the action to be taken
    /// if either no rules match the claim value for the `Rules` type, or there is no
    /// `cognito:preferred_role` claim and there are multiple `cognito:roles` matches for
    /// the `Token` type" (`RoleMapping$AmbiguousRoleResolution`, this service's model) — so
    /// that value is read here and answered as [`MappedRole`] rather than silently falling
    /// through to the pool-wide role.
    ///
    /// The logins are considered in provider-name order and the mappings in key order, so
    /// which mapping decides an exchange presenting several logins does not depend on the
    /// iteration order of the request's map.
    pub fn mapped_role(
        &self,
        pool: &IdentityPool,
        logins: &HashMap<String, String>,
    ) -> MappedRole {
        let Some(role_mappings) = self.role_mappings.as_ref() else {
            return MappedRole::Unmapped;
        };
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        // What the FIRST mapping that decided the exchange without resolving a role states
        // for that case, kept until a later mapping resolves one: a resolved role is what
        // the exchange assumes, whichever mapping resolved it.
        let mut unresolved: Option<MappedRole> = None;
        for (provider, token) in presented {
            // A login whose presented value carries no claim set states nothing a mapping
            // rule reads, so no mapping of it decides anything here: what such a token says
            // is decided by the provider that issued it, which this emulator does not reach.
            let Some(claims) = presented_token_claims(token) else {
                continue;
            };
            for mapping in role_mappings
                .iter()
                .filter(|(key, _)| mapping_key_names_provider(key, provider, pool, &claims))
                .map(|(_, mapping)| mapping)
            {
                match mapping_resolved_role_arn(mapping, &claims) {
                    Some(role_arn) => return MappedRole::Resolved(role_arn),
                    None if unresolved.is_none() => {
                        unresolved = Some(ambiguous_role_resolution(mapping, provider));
                    }
                    None => {}
                }
            }
        }
        unresolved.unwrap_or(MappedRole::Unmapped)
    }

    /// Whether this configuration holds a `RoleMappings` entry for the identity provider
    /// `provider_name`.
    ///
    /// A mapping is keyed by the identity provider's own identifier, which for a provider
    /// that issues its tokens to an APP CLIENT carries that client after the provider's name
    /// (`<provider name>:<client id>`, [`mapping_key_names_provider`]), so a key naming this
    /// provider for any of its clients is one of the provider's.
    pub fn maps_provider(&self, provider_name: &str) -> bool {
        self.role_mappings.iter().flatten().any(|(key, _)| {
            key == provider_name
                || key
                    .strip_prefix(provider_name)
                    .is_some_and(|rest| rest.starts_with(PROVIDER_CLIENT_SEPARATOR))
        })
    }

    /// Whether this configuration names `role_arn` — as one of the pool's two roles, or
    /// as the role a mapping rule resolves to. This is the set of roles the pool can vend
    /// credentials for, which is what a `CustomRoleArn` has to be drawn from.
    pub fn names_role_arn(&self, role_arn: &str) -> bool {
        self.roles.values().any(|arn| arn == role_arn)
            || self.role_mappings.iter().flatten().any(|(_, mapping)| {
                mapping.rules_configuration.iter().any(|configuration| {
                    configuration
                        .rules
                        .iter()
                        .any(|rule| rule.role_arn == role_arn)
                })
            })
    }

    /// Whether an exchange presenting `logins` may be told to assume `role_arn` through
    /// `CustomRoleArn`: "The Amazon Resource Name (ARN) of the role to be assumed when
    /// MULTIPLE ROLES WERE RECEIVED IN THE TOKEN from the identity provider"
    /// (`GetCredentialsForIdentityInput$CustomRoleArn`, this service's model).
    ///
    /// So the roles such a request may pick from are the roles the pool's own configuration
    /// names ([`IdentityPoolRoles::names_role_arn`]) TOGETHER WITH the roles the presented
    /// tokens themselves carry ([`token_claimed_role_arns`]) — the latter being the very
    /// several-roles-in-one-token case the member exists for, which no stored rule of the
    /// pool need name. Anything else is a role this exchange could not have resolved, so it
    /// is not one a request may substitute.
    pub fn admits_custom_role_arn(
        &self,
        logins: &HashMap<String, String>,
        role_arn: &str,
    ) -> bool {
        self.names_role_arn(role_arn)
            || token_claimed_role_arns(logins)
                .iter()
                .any(|claimed| claimed == role_arn)
    }
}

/// Whether an exchange presenting `logins` against `pool`, configured with `roles`, may name
/// a `CustomRoleArn` AT ALL — "Only SAML providers and providers with RoleMappings support
/// custom role ARN", which is how the service words the refusal of one that may not.
///
/// The member substitutes one of the SEVERAL roles the identity provider's own answer
/// offered ("the role to be assumed when multiple roles were received in the token from the
/// identity provider", `GetCredentialsForIdentityInput$CustomRoleArn`), and two kinds of
/// provider offer such a choice: a SAML provider, whose assertion carries the roles the user
/// may assume ([`IdentityPool::names_saml_provider`]), and a provider the pool holds a
/// `RoleMappings` entry for, which is what picks a role from the token's claims
/// ([`IdentityPoolRoles::maps_provider`]). An exchange whose presented logins are of neither
/// kind — a pool with no role mapping for the provider the login was made under, and an
/// exchange presenting no login at all — had one role to assume and nothing to choose
/// between, so naming another is a bad input parameter rather than a choice.
pub fn custom_role_arn_is_supported(
    pool: &IdentityPool,
    roles: Option<&IdentityPoolRoles>,
    logins: &HashMap<String, String>,
) -> bool {
    logins.keys().any(|provider| {
        pool.names_saml_provider(provider)
            || roles.is_some_and(|roles| roles.maps_provider(provider))
    })
}

/// How AWS words the refusal of a `CustomRoleArn` an exchange may not name at all
/// ([`custom_role_arn_is_supported`]). Fixed text: it states which providers support the
/// member, naming neither the role the request asked for nor the providers the pool is
/// configured with.
pub const CUSTOM_ROLE_ARN_UNSUPPORTED_MESSAGE: &str =
    "Only SAML providers and providers with RoleMappings support custom role ARN.";

/// The roles the tokens `logins` presents received from the identity provider that issued
/// them: the values of the two claims a Cognito identity-provider token carries roles in
/// ([`ROLES_CLAIM`] and [`PREFERRED_ROLE_CLAIM`], the claims `RoleMapping$Type` names for the
/// `Token` type), each once, in the provider-name order the logins are considered in.
///
/// A login whose presented value carries no claim set at all states no role here: what such
/// a token says is decided by the provider that issued it, which this emulator does not
/// reach.
fn token_claimed_role_arns(logins: &HashMap<String, String>) -> Vec<String> {
    let mut presented: Vec<(&String, &String)> = logins.iter().collect();
    presented.sort();
    let mut claimed: Vec<String> = Vec::new();
    for (_, token) in presented {
        let Some(claims) = presented_token_claims(token) else {
            continue;
        };
        for claim in [ROLES_CLAIM, PREFERRED_ROLE_CLAIM] {
            for role_arn in claim_values(&claims, claim) {
                if !claimed.contains(&role_arn) {
                    claimed.push(role_arn);
                }
            }
        }
    }
    claimed
}

/// The `RoleMappingType` values this emulator resolves a role for, from `RoleMapping$Type`
/// (this service's model): "Token will use `cognito:roles` and `cognito:preferred_role`
/// claims from the Cognito identity provider token to map groups to roles. Rules will
/// attempt to match claims from the token to map to a role."
const RULES_ROLE_MAPPING_TYPE: &str = "Rules";
const TOKEN_ROLE_MAPPING_TYPE: &str = "Token";

/// The claims a `Token` mapping maps a user's GROUPS to a role through, as
/// `RoleMapping$Type` names them: the roles the user's groups grant, and the one of them
/// the user prefers.
const ROLES_CLAIM: &str = "cognito:roles";
const PREFERRED_ROLE_CLAIM: &str = "cognito:preferred_role";

/// What a pool's `RoleMappings` decide about one exchange
/// ([`IdentityPoolRoles::mapped_role`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappedRole {
    /// No mapping of the configuration decides the exchange: none is keyed by an identity
    /// provider a presented login was made under, or the login presenting one carries no
    /// claim set for a mapping to read. The pool-wide role for the flow the request is in
    /// is what such an exchange assumes.
    Unmapped,
    /// The ARN of the role a mapping resolved from the presented token's claims.
    Resolved(String),
    /// A mapping decides the exchange, resolves no role from the presented token, and
    /// states the `AuthenticatedRole` resolution for that case: the pool's own
    /// `authenticated` role is what the exchange assumes.
    AuthenticatedRole,
    /// The same, with the `Deny` resolution: the exchange is refused and nothing is vended.
    /// Carries the identity provider that mapping decided the login under, which the refusal
    /// names ([`denied_role_resolution_message`]).
    Denied(String),
}

/// What ONE mapping states for the case its rules resolve no role: its
/// `AmbiguousRoleResolution` value, which `SetIdentityPoolRoles` stores only as one of the
/// two values the modelled `AmbiguousRoleResolutionType` enum admits
/// ([`crate::ops::types::AMBIGUOUS_ROLE_RESOLUTION_TYPE_VALUES`]).
///
/// A mapping stored with no resolution at all — which `SetIdentityPoolRoles` refuses for
/// the two types that require one — states no `Deny`, so it leaves the exchange to the
/// pool-wide role rather than refusing it.
fn ambiguous_role_resolution(mapping: &RoleMapping, provider: &str) -> MappedRole {
    match mapping.ambiguous_role_resolution.as_deref() {
        Some(DENY_ROLE_RESOLUTION) => MappedRole::Denied(provider.to_string()),
        Some(AUTHENTICATED_ROLE_RESOLUTION) => MappedRole::AuthenticatedRole,
        _ => MappedRole::Unmapped,
    }
}

/// The `AmbiguousRoleResolutionType` values, from the model
/// ([`crate::ops::types::AMBIGUOUS_ROLE_RESOLUTION_TYPE_VALUES`]).
const AUTHENTICATED_ROLE_RESOLUTION: &str = "AuthenticatedRole";
const DENY_ROLE_RESOLUTION: &str = "Deny";

/// The role ONE mapping resolves for a token carrying `claims`, and `None` when it
/// resolves none — the case the mapping's `AmbiguousRoleResolution` decides
/// ([`ambiguous_role_resolution`]).
///
/// For a `Rules` mapping that is the first rule to match: "Rules are evaluated in order.
/// The first one to match specifies the role" (`RulesConfigurationType$Rules`, this
/// service's model), in the DECLARED order the list was stored in. For a `Token` mapping it
/// is the role the token's own group claims name ([`token_mapped_role_arn`]). A type the
/// model does not define resolves nothing.
fn mapping_resolved_role_arn(
    mapping: &RoleMapping,
    claims: &::serde_json::Map<String, ::serde_json::Value>,
) -> Option<String> {
    match mapping.r#type.as_str() {
        RULES_ROLE_MAPPING_TYPE => mapping
            .rules_configuration
            .as_ref()?
            .rules
            .iter()
            .find(|rule| rule_matches_claims(rule, claims))
            .map(|rule| rule.role_arn.clone()),
        TOKEN_ROLE_MAPPING_TYPE => token_mapped_role_arn(claims),
        _ => None,
    }
}

/// The role a `Token` mapping resolves from a token's own group claims: the type that
/// "will use `cognito:roles` and `cognito:preferred_role` claims from the Cognito identity
/// provider token to map groups to roles" (`RoleMapping$Type`, this service's model).
///
/// The preferred role is what the user's token asks for, so it decides where it names one;
/// otherwise the single role the user's groups grant is the one mapped. A token naming no
/// role, and one naming several with no preference among them, resolve NONE — "there is no
/// `cognito:preferred_role` claim and there are multiple `cognito:roles` matches"
/// (`RoleMapping$AmbiguousRoleResolution`) is the very case that mapping's resolution
/// states the outcome of.
fn token_mapped_role_arn(
    claims: &::serde_json::Map<String, ::serde_json::Value>,
) -> Option<String> {
    if let [preferred] = claim_values(claims, PREFERRED_ROLE_CLAIM).as_slice() {
        return Some(preferred.clone());
    }
    match claim_values(claims, ROLES_CLAIM).as_slice() {
        [role_arn] => Some(role_arn.clone()),
        _ => None,
    }
}

/// Whether `rule`'s match condition holds for `claims`: "the match condition that specifies
/// how closely the claim value in the IdP token must match `Value`"
/// (`MappingRule$MatchType`, this service's model), over the values the token carries for
/// the claim the rule names.
///
/// A claim the token does not carry satisfies no condition at all, `NotEqual` included —
/// the rule's claim is one "that must be present in the token" (`MappingRule$Claim`) — and
/// a match type the model does not define is one no claim value satisfies.
fn rule_matches_claims(
    rule: &MappingRule,
    claims: &::serde_json::Map<String, ::serde_json::Value>,
) -> bool {
    let values = claim_values(claims, &rule.claim);
    if values.is_empty() {
        return false;
    }
    // The `MappingRuleMatchType` values, from the model
    // (`crate::ops::types::MAPPING_RULE_MATCH_TYPE_VALUES`).
    match rule.match_type.as_str() {
        "Equals" => values.contains(&rule.value),
        "Contains" => values.iter().any(|value| value.contains(&rule.value)),
        "StartsWith" => values.iter().any(|value| value.starts_with(&rule.value)),
        "NotEqual" => !values.contains(&rule.value),
        _ => false,
    }
}

/// The values a token carries for the claim `name`, as the strings a mapping rule compares
/// against.
///
/// A claim is ONE value where the token carries a string, a number or a boolean, and every
/// element where it carries a list of those — a group or role membership claim is a list,
/// and an audience may be one (RFC 7519 §4.1.3) — so a rule decides a multi-valued claim on
/// the values it actually carries rather than on a rendering of the list. A claim the token
/// does not carry, or carries as a structure no claim value is, has no values at all.
fn claim_values(
    claims: &::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Vec<String> {
    fn value_of(claim: &::serde_json::Value) -> Option<String> {
        match claim {
            ::serde_json::Value::String(text) => Some(text.clone()),
            ::serde_json::Value::Number(number) => Some(number.to_string()),
            ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }
    match claims.get(name) {
        Some(::serde_json::Value::Array(elements)) => {
            elements.iter().filter_map(value_of).collect()
        }
        Some(claim) => value_of(claim).into_iter().collect(),
        None => Vec::new(),
    }
}

/// Whether the `RoleMappings` key `key` names the identity provider a `Logins` entry made
/// under `provider` was authenticated by.
///
/// A role mapping is keyed by the identity provider's own identifier. A provider that
/// issues its tokens to an APP CLIENT — an Amazon Cognito user pool of the pool's
/// `CognitoIdentityProviders`, an OpenID Connect issuer — is identified by that client as
/// well as by its name, as `<provider name>:<client id>`; every other provider is
/// identified by its name alone.
///
/// The client a presented token was issued to is the one the token's own audience claim
/// names (RFC 7519 §4.1.3), so that claim is what decides such a key: a pool fronted by two
/// app clients of one provider holds a mapping per client, and a token issued to one of them
/// is not the other's. A token carrying no audience at all names no client, and for it the
/// clients the pool itself stores for that provider are what the key can be matched against.
fn mapping_key_names_provider(
    key: &str,
    provider: &str,
    pool: &IdentityPool,
    claims: &::serde_json::Map<String, ::serde_json::Value>,
) -> bool {
    if key == provider {
        return true;
    }
    let Some(client_id) = key
        .strip_prefix(provider)
        .and_then(|rest| rest.strip_prefix(PROVIDER_CLIENT_SEPARATOR))
    else {
        return false;
    };
    let audiences = claim_values(claims, AUDIENCE_CLAIM);
    if !audiences.is_empty() {
        return audiences.iter().any(|audience| audience.as_str() == client_id);
    }
    pool.cognito_identity_providers
        .iter()
        .flatten()
        .filter(|configured| configured.provider_name.as_deref() == Some(provider))
        .any(|configured| configured.client_id.as_deref() == Some(client_id))
}

/// What separates an identity provider's name from the app client it issues tokens to in
/// the identifier a `RoleMappings` entry is keyed by.
const PROVIDER_CLIENT_SEPARATOR: char = ':';

/// The claim set of the identity token a `Logins` entry presents, when the value presented
/// is a token at all.
///
/// The reading is [`::service_common::jwt::read`], the tree's one bounded reading of a
/// compact JWS: a value that is no JSON Web Token, or one nothing signed, carries no claim
/// a role mapping could resolve a role from. The signature is not VERIFIED here — that needs
/// the issuing provider's published verification key, which this emulator makes no outbound
/// connection to fetch — so what the claims can decide is which of the roles the POOL's own
/// stored configuration names is assumed, and never a role the configuration does not name.
fn presented_token_claims(token: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    ::service_common::jwt::read(token)
        .ok()
        .map(|read| read.claims)
}

/// A stored identity, as `GetId` minted it. `logins` holds the PROVIDER NAMES the
/// identity is linked to, which is what the modelled `IdentityDescription$Logins` list
/// carries.
#[derive(Debug, Clone)]
pub struct Identity {
    pub identity_id: String,
    pub identity_pool_id: String,
    pub logins: Vec<String>,
    /// Whether every login provider has already been taken off this identity by a call that
    /// takes logins away — an unlink of the last one, or a merge that moved them onto the
    /// identity it merged this one into — which is what makes it a DISABLED one: the record
    /// survives, reachable through no login at all
    /// ("removing the last linked login will make this identity inaccessible"), and that is
    /// a different state from an identity that never carried a login — see
    /// [`Identity::is_disabled`].
    pub disabled: bool,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Identity {
    /// Whether this identity is a DISABLED one: an unlink or a merge took its logins off it
    /// and none has been linked since, so nothing resolves to it.
    pub fn is_disabled(&self) -> bool {
        self.disabled && self.logins.is_empty()
    }

    /// The `Logins` list every read of this identity reports — `DescribeIdentity` and the
    /// descriptions `ListIdentities` pages — which is the provider names it is linked to,
    /// and for a DISABLED one the single [`DISABLED_LOGIN_PROJECTION`] sentinel standing in
    /// place of the providers that were taken off it.
    pub fn projected_logins(&self) -> Vec<String> {
        if self.is_disabled() {
            return vec![DISABLED_LOGIN_PROJECTION.to_string()];
        }
        self.logins.clone()
    }

    /// Whether a login has ever been linked to this identity: one it still carries, or one
    /// an unlink or a merge has since taken off it (which is what leaves it DISABLED).
    ///
    /// Such an identity is a FEDERATED user's, so what authorizes a call naming it is a
    /// login of it. An identity this is false for is an UNAUTHENTICATED (guest) one — the
    /// identity `GetId` mints for a call presenting no login at all — which no login reaches
    /// and whose id is the whole of what names it.
    pub fn has_been_linked(&self) -> bool {
        self.disabled || !self.logins.is_empty()
    }
}

/// The one element the `Logins` list of a DISABLED identity carries: a fixed sentinel,
/// not a provider name, which is how a read tells an identity an unlink orphaned from one
/// that was never linked to anything (whose list is empty). It is the same value whatever
/// providers were removed, so nothing of the request or of the stored record goes into it.
pub const DISABLED_LOGIN_PROJECTION: &str = "DISABLED";

/// How AWS words the refusal of a call that has to be authorized by a LOGIN of the identity
/// it names when none of the logins it presents is one of that identity's — which is every
/// such call for an identity an unlink left linked to nothing, since no login resolves to it
/// any more ("removing the last linked login will make this identity inaccessible",
/// `UnlinkIdentity`, this service's model).
///
/// Entirely fixed text, with no request- or state-derived part: the caller of these
/// unsigned, login-authorized operations holds no credential of this service, so the refusal
/// states what it has to send and nothing about the identity or what the pool holds for it.
pub const UNMATCHED_LOGINS_MESSAGE: &str = "Logins don't match. Please include at least one \
                                            valid login for this identity or identity pool.";

/// Why an identity a POOL-SCOPED request names beside its `IdentityPoolId` is not one that
/// request may reach
/// ([`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::scoped_identity`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedIdentityRefusal {
    /// This service holds no identity of that id at all, so the identity is a resource the
    /// request named and the service does not have.
    NotStored,
    /// It holds one, and that identity belongs to ANOTHER identity pool, so reaching it
    /// through the pool this request names is not something the caller may do.
    AnotherPool,
}

/// Why a call naming an identity is not one its caller has proven it may make
/// ([`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::refuse_identity_access`]).
/// Each is answered with the modelled `NotAuthorizedException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityAccessRefusal {
    /// The call presented no login at all for an identity a login has been linked to.
    NoLoginPresented,
    /// It presented logins, and none of them is one of that identity's.
    LoginsDontMatch,
}

impl IdentityAccessRefusal {
    /// The message the refusal carries for a call that named `identity_id`.
    pub fn message(self, identity_id: &str) -> String {
        match self {
            Self::NoLoginPresented => forbidden_identity_message(identity_id),
            Self::LoginsDontMatch => UNMATCHED_LOGINS_MESSAGE.to_string(),
        }
    }
}

/// The most OpenID Connect tokens this emulator holds outstanding at once. Not an AWS
/// quota: THIS emulator's own ceiling on a store that grows by one record per successful
/// `GetOpenIdToken` / `GetOpenIdTokenForDeveloperIdentity` call — including the calls that
/// resolve an identity the pool already has — and whose records live as long as the window
/// the caller asked for, up to the modelled `TokenDuration` maximum of 24 hours. Expiry
/// alone would bound only how long a record lives, not how many exist, so the count is
/// capped too: see
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::record_open_id_token`],
/// which forgets the oldest issuances to stay inside it.
pub const MAX_OUTSTANDING_OPEN_ID_TOKENS: usize = 10_000;

/// The issuer every OpenID Connect token this service mints names in its `iss` claim.
/// Amazon Cognito Identity's OIDC tokens are issued by
/// `https://cognito-identity.amazonaws.com`, which is also the principal an IAM role's
/// trust policy names to let an identity pool's users assume it
/// (`AssumeRoleWithWebIdentity$RoleArn`: "The trust policies of these roles must accept
/// the cognito-identity.amazonaws.com service principal").
pub const OIDC_TOKEN_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// The Cognito Identity service principal, which an issued token names among the
/// authentication methods (`amr`) that produced it, beside the providers that
/// authenticated the user.
pub const COGNITO_IDENTITY_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// An OpenID Connect token this service issued, keyed by the token itself in
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::open_id_tokens`]: the
/// identity it names, the pool that issued it, the window it is valid in and the
/// principal tags the issuing call asked for.
#[derive(Debug, Clone)]
pub struct IssuedOpenIdToken {
    pub identity_id: String,
    pub identity_pool_id: String,
    pub issued_at: SystemTime,
    pub expiration: SystemTime,
    /// The `Logins` provider names the ISSUING call presented: what authenticated the
    /// identity this token names, which is what the token's own `amr` claim is built from
    /// ([`authentication_methods`]). Empty for an issuance made with no login at all — an
    /// unauthenticated one — so the methods of such a token are the unauthenticated method
    /// alone.
    pub authenticated_by: Vec<String>,
    /// The `PrincipalTags` the issuing call carried, ordered so a read reports the same
    /// map twice. `None` when the caller sent none, so nothing invents an empty map.
    pub principal_tags: Option<BTreeMap<String, String>>,
}

/// The `Logins` provider name a token THIS service minted is presented under — the Cognito
/// Identity principal: "If the token is for cognito-identity.amazonaws.com, it will be
/// passed through to Security Token Service with the appropriate role for the token"
/// (`GetCredentialsForIdentity`, this service's model). A `Logins` entry under any other
/// provider name carries a token an EXTERNAL provider issued, which this emulator never saw
/// minted and so cannot decide; an entry under THIS name names an issuance this service
/// recorded itself, which is what makes its validity window readable here.
pub const COGNITO_IDENTITY_LOGIN_PROVIDER: &str = COGNITO_IDENTITY_PRINCIPAL;

/// How AWS words the refusal of a request presenting a token of this service whose validity
/// window has closed — the window `GetOpenIdToken` / `GetOpenIdTokenForDeveloperIdentity`
/// recorded when they minted it.
///
/// Fixed text with no request- or state-derived part: the caller of a credential exchange
/// holds no credential yet, so the refusal says that the token's time is up and nothing
/// about the identity, the pool or the token it was made with.
pub const EXPIRED_LOGIN_TOKEN_MESSAGE: &str = "Invalid login token. Token is expired.";

/// How AWS words the refusal of a request presenting a token under this service's own
/// provider name that this service cannot resolve at all: one it never issued, or one it
/// has forgotten. Nothing in it is vouched for, so nothing in it authorizes anything.
pub const UNVERIFIABLE_LOGIN_TOKEN_MESSAGE: &str =
    "Invalid login token. Couldn't verify signed token.";

/// How AWS words the refusal of a token presented under this service's own provider name
/// that is a token of ANOTHER identity pool: the token is read against the pool that owns
/// the identity the call names — each identity belongs to the pool that minted it — and a
/// token some other pool minted is no Cognito token of this one, whatever it claims.
///
/// Fixed text with no request- or state-derived part, as the other refusals of a presented
/// token are: the caller of a credential exchange holds no credential yet, so the refusal
/// names neither pool, neither identity, nor the token it was made with.
pub const NOT_A_COGNITO_TOKEN_MESSAGE: &str = "Invalid login token. Not a Cognito token.";

/// Why a token presented under [`COGNITO_IDENTITY_LOGIN_PROVIDER`] authorizes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTokenRefusal {
    /// The validity window the issuance recorded has closed.
    Expired,
    /// This service holds no issuance for the token.
    Unverifiable,
    /// The issuance was made by ANOTHER identity pool than the one that owns the identity
    /// the request was made for, so the token is no token of that pool at all — which is
    /// what keeps a token one identity pool minted out of another pool's identities.
    AnotherPool,
    /// The issuance is one of the SAME pool, naming another of its identities: a token of
    /// that pool, but no login of the identity the request named.
    AnotherIdentity,
}

impl LoginTokenRefusal {
    /// The message the refusal carries.
    pub fn message(self) -> &'static str {
        match self {
            Self::Expired => EXPIRED_LOGIN_TOKEN_MESSAGE,
            Self::Unverifiable => UNVERIFIABLE_LOGIN_TOKEN_MESSAGE,
            Self::AnotherPool => NOT_A_COGNITO_TOKEN_MESSAGE,
            Self::AnotherIdentity => UNMATCHED_LOGINS_MESSAGE,
        }
    }
}

/// The credentials `GetCredentialsForIdentity` issued for one identity. The id and the
/// secrets are the emulator's own credential store's: a handler never constructs them.
#[derive(Debug, Clone)]
pub struct IssuedCredentials {
    pub access_key_id: String,
    pub secret_key: String,
    pub session_token: String,
    pub expiration: SystemTime,
    /// The ARN of the pool role this session was assumed of, so a later vend hands the
    /// same session back only while it still stands for the role THAT call resolves — a
    /// vend for another role is another session.
    pub role_arn: String,
}

/// Per-service state for com-amazonaws-cognitoidentity-awscognitoidentityservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// Pools by `IdentityPoolId`.
    pub identity_pools: HashMap<String, IdentityPool>,
    /// The roles of a pool, by `IdentityPoolId`, as `SetIdentityPoolRoles` wrote them. A
    /// pool with no entry has had no `SetIdentityPoolRoles` call, which is an EMPTY role
    /// configuration, not a missing pool.
    pub identity_pool_roles: HashMap<String, IdentityPoolRoles>,
    /// The principal-tag attribute map of one `(IdentityPoolId, IdentityProviderName)`
    /// pair, as `SetPrincipalTagAttributeMap` wrote it. A pair with no entry has had no
    /// such call, so there is no mapping of that provider's attributes to report. The two
    /// parts are kept as separate key fields rather than joined into one string: the
    /// provider name comes from the caller and any separator it may also contain would
    /// make two distinct providers share a key. Bounded per pool by
    /// [`MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL`].
    pub principal_tag_attribute_maps: HashMap<(String, String), PrincipalTagAttributeMap>,
    /// Identities by `IdentityId`.
    pub identities: HashMap<String, Identity>,
    /// The identities whose pool has been DELETED, in the order the deletes orphaned them.
    /// Their records stay in [`Self::identities`] so an identity-scoped call made after the
    /// pool went reports the POOL as the resource that is missing; this is the queue that
    /// keeps that retention bounded by [`MAX_ORPHANED_IDENTITY_RECORDS`], the oldest
    /// orphaned record being given up first. An entry whose record a later
    /// `DeleteIdentities` removed is simply nothing left to evict.
    pub orphaned_identities: VecDeque<String>,
    /// The identities of one pool, in the order `GetId` minted them, so a paginated
    /// `ListIdentities` resumes at a stable position.
    pub identities_by_pool: HashMap<String, Vec<String>>,
    /// `(IdentityPoolId, provider name, login value)` -> `IdentityId`, so `GetId` is
    /// idempotent for a federated login. The three parts are kept as separate key fields
    /// rather than joined into one string: both the provider name and the login come from
    /// the caller, and any separator they may also contain would make two distinct logins
    /// share a key.
    pub login_index: HashMap<(String, String, String), String>,
    /// The credentials outstanding for an identity, by `IdentityId`.
    pub credentials: HashMap<String, IssuedCredentials>,
    /// The OpenID Connect tokens outstanding, by the token itself, as `GetOpenIdToken`
    /// and `GetOpenIdTokenForDeveloperIdentity` minted them. A token is a bearer of the
    /// identity it names, so the record is what tells this service which identity and
    /// pool a token presented later belongs to, and whether its window is still open.
    pub open_id_tokens: HashMap<String, IssuedOpenIdToken>,
    /// The pools a request is WRITING at this instant, by `IdentityPoolId` — the claims
    /// [`claimed_pool_write`] hands out and the dropped [`PoolWriteClaim`] gives back.
    ///
    /// A pool is reconfigured as a whole, by its configuration (`UpdateIdentityPool`) or by
    /// its role configuration (`SetIdentityPoolRoles`), so two requests writing one pool at
    /// the same time are the parallel modification `ConcurrentModificationException` is
    /// thrown for ("Thrown if there are parallel requests to modify a resource",
    /// `smithy_query(query="shape",shape="ConcurrentModificationException")`): the writer
    /// that finds the pool already claimed is the one that loses the race, and what it did
    /// not get to write is not stored. Nothing is claimed by a READ, and a claim never
    /// outlives the request that took it.
    pub pool_writes_in_flight: ::std::collections::BTreeSet<String>,
}

/// The claim one request holds on a pool while it writes it.
///
/// Held for the whole of the write — taken before the pool is read and given back when this
/// value is dropped, which happens on the way out of the handler whatever the handler
/// answers — so a second writer of the same pool meets the claim rather than a half-written
/// record, and a refused or panicking write leaves no claim behind.
pub struct PoolWriteClaim {
    state: ::engine::ServiceState<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>,
    identity_pool_id: String,
}

impl ::std::fmt::Debug for PoolWriteClaim {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PoolWriteClaim")
            .field("identity_pool_id", &self.identity_pool_id)
            .finish()
    }
}

impl Drop for PoolWriteClaim {
    fn drop(&mut self) {
        self.state
            .lock()
            .pool_writes_in_flight
            .remove(&self.identity_pool_id);
    }
}

/// The claim on `identity_pool_id`, or `None` when another request is writing that pool
/// right now — which is the parallel modification the modelled
/// `ConcurrentModificationException` reports.
///
/// `state` is this service's store handle (what [`service_state`] hands back); the claim
/// holds a handle of its own so it can give the claim back when it is dropped.
pub fn claimed_pool_write(
    state: &::engine::ServiceState<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>,
    identity_pool_id: &str,
) -> Option<PoolWriteClaim> {
    if !state
        .lock()
        .pool_writes_in_flight
        .insert(identity_pool_id.to_string())
    {
        return None;
    }
    Some(PoolWriteClaim {
        state: state.clone(),
        identity_pool_id: identity_pool_id.to_string(),
    })
}

/// The text `ConcurrentModificationException` (HTTP 400) carries, in the model's own words
/// for the shape: "Thrown if there are parallel requests to modify a resource"
/// (`smithy_query(query="shape",shape="ConcurrentModificationException")`).
///
/// Fixed text with no request- or state-derived part: what the caller acts on is that the
/// write lost a race, which is the same fact whichever pool it was for.
pub const PARALLEL_MODIFICATION_MESSAGE: &str =
    "There are parallel requests to modify this resource.";

impl ComAmazonawsCognitoidentityAwscognitoidentityserviceState {
    /// Whether another pool would take this emulator past [`MAX_IDENTITY_POOLS`].
    pub fn identity_pool_store_is_full(&self) -> bool {
        self.identity_pools.len() >= MAX_IDENTITY_POOLS
    }

    /// Whether this pool already holds as many principal-tag attribute maps as
    /// [`MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL`] allows, so a mapping for a provider it
    /// does not hold one for yet would take it past that ceiling. Replacing the mapping of
    /// a provider it already holds grows nothing, so the caller asks only about a NEW one.
    pub fn principal_tag_attribute_map_store_is_full(&self, identity_pool_id: &str) -> bool {
        self.principal_tag_attribute_maps
            .keys()
            .filter(|(pool, _)| pool == identity_pool_id)
            .count()
            >= MAX_PRINCIPAL_TAG_ATTRIBUTE_MAPS_PER_POOL
    }

    /// Every stored `IdentityPoolId`, in the order a listing of the account's pools
    /// reports them: sorted, so two successive pages of one listing neither overlap nor
    /// skip a pool. The pools are held in a hash map, whose iteration order is not
    /// stable from one call to the next, so the order has to come from the ids
    /// themselves — as it does for the login values of an identity.
    pub fn identity_pool_ids_in_listing_order(&self) -> Vec<String> {
        let mut identity_pool_ids: Vec<String> = self.identity_pools.keys().cloned().collect();
        identity_pool_ids.sort();
        identity_pool_ids
    }

    /// Whether another identity in this pool would take it past
    /// [`MAX_IDENTITIES_PER_POOL`].
    pub fn pool_identity_store_is_full(&self, identity_pool_id: &str) -> bool {
        self.identities_by_pool
            .get(identity_pool_id)
            .is_some_and(|identities| identities.len() >= MAX_IDENTITIES_PER_POOL)
    }

    /// The `IdentityId` the pool has linked to this `(provider, login)`, if any.
    ///
    /// The login is keyed by the provider's USER IDENTIFIER it names
    /// ([`login_user_identifier`]), so every login of one federated user resolves to the one
    /// identity.
    pub fn linked_identity(
        &self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
    ) -> Option<&String> {
        self.login_index.get(&(
            identity_pool_id.to_string(),
            provider.to_string(),
            login_user_identifier(login),
        ))
    }

    /// Every login value this pool has indexed for `provider` against `identity_id`, in
    /// a stable (sorted) order — for the pool's own `DeveloperProviderName`, the developer
    /// user identifiers one identity is registered under. One provider's share of
    /// [`Self::pool_logins_of_identity`], so the index is scanned in one place; that scan
    /// orders the pairs, which leaves these values sorted too — and the order has to come
    /// from the values themselves, the index being a hash map, for two successive pages of a
    /// listing to neither overlap nor skip one.
    pub fn logins_of_identity(
        &self,
        identity_pool_id: &str,
        provider: &str,
        identity_id: &str,
    ) -> Vec<String> {
        self.pool_logins_of_identity(identity_pool_id, identity_id)
            .into_iter()
            .filter(|(login_provider, _)| login_provider == provider)
            .map(|(_, login)| login)
            .collect()
    }

    /// Every `(provider name, login value)` pair this pool has indexed against
    /// `identity_id`, whatever the provider — the identity's LINKED LOGINS, the developer
    /// user identifiers among them — in a stable (sorted) order: the index is a hash map, so
    /// the order has to come from the pairs themselves.
    pub fn pool_logins_of_identity(
        &self,
        identity_pool_id: &str,
        identity_id: &str,
    ) -> Vec<(String, String)> {
        let mut logins: Vec<(String, String)> = self
            .login_index
            .iter()
            .filter(|((pool_id, _, _), linked)| {
                pool_id == identity_pool_id && linked.as_str() == identity_id
            })
            .map(|((_, provider, login), _)| (provider.clone(), login.clone()))
            .collect();
        logins.sort();
        logins
    }

    /// Link `(provider, login)` in this pool to `identity_id`, so a later `GetId`
    /// presenting it resolves to that identity.
    ///
    /// Indexed under the provider's USER IDENTIFIER the login names
    /// ([`login_user_identifier`]), which is what makes the link one per federated USER
    /// rather than one per presented value.
    pub fn link_login(
        &mut self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
        identity_id: &str,
    ) {
        self.login_index.insert(
            (
                identity_pool_id.to_string(),
                provider.to_string(),
                login_user_identifier(login),
            ),
            identity_id.to_string(),
        );
    }

    /// Forget the link between `(provider, login)` in this pool and whatever identity it
    /// resolved to, and report that identity — the inverse of [`Self::link_login`], so a
    /// later call presenting the same login resolves nothing and is seen as new. `None`
    /// when this pool indexed no such login at all.
    pub fn unlink_login(
        &mut self,
        identity_pool_id: &str,
        provider: &str,
        login: &str,
    ) -> Option<String> {
        self.login_index.remove(&(
            identity_pool_id.to_string(),
            provider.to_string(),
            login_user_identifier(login),
        ))
    }

    /// Merge the identity `merged_away` of this pool INTO `survivor`, and report the access
    /// key ids whose credentials the caller must now forget (`None` when there was no such
    /// pair of this pool's identities to merge, so nothing was written).
    ///
    /// Every login `merged_away` was reached through — its developer user identifiers and
    /// its federated logins alike — now resolves to `survivor`, which is left linked to the
    /// union of the two provider sets and stamped with `now` as the instant it changed. The
    /// two users are one user afterwards, so nothing reaches `merged_away` any more: the
    /// credentials outstanding for it and the OpenID Connect tokens issued for it spoke for
    /// a user that is now reached through `survivor` alone, so they are given up.
    ///
    /// The RECORD of `merged_away` nevertheless stays stored, in `identities` and in the
    /// pool's listing order, with its logins emptied and `disabled` set: a merge takes the
    /// logins off an identity, it does not remove the identity, so what is left is exactly
    /// the DISABLED identity an unlink of the last login leaves ([`Identity::is_disabled`])
    /// — still described by a read of its id, still reported by a listing that hides no
    /// disabled identity, and hidden from one that hides them. Removing the record is
    /// [`Self::delete_identity`]'s own effect, which no other transition borrows.
    ///
    /// Both identities have to be the named pool's own, which is what the caller resolved
    /// them through: an identity belongs to the pool that minted it and never moves, so a
    /// pair this pool does not hold is merged nowhere and answered `None` — rather than one
    /// pool's records being torn down while another pool's listing still names the identity.
    pub fn merge_identity_into(
        &mut self,
        identity_pool_id: &str,
        merged_away: &str,
        survivor: &str,
        now: SystemTime,
    ) -> Option<Vec<String>> {
        let both_belong_to_the_pool = [merged_away, survivor].iter().all(|identity_id| {
            self.identities
                .get(*identity_id)
                .is_some_and(|identity| identity.identity_pool_id == identity_pool_id)
        });
        if !both_belong_to_the_pool {
            return None;
        }
        for (provider, login) in self.pool_logins_of_identity(identity_pool_id, merged_away) {
            self.link_login(identity_pool_id, &provider, &login, survivor);
        }
        // The merged-away identity keeps its place in the store and in the pool's listing
        // order; what the merge takes off it is its logins, which the survivor carries from
        // here on. An identity left linked to nothing is a DISABLED one, so that is the
        // state the record is stamped with, at the instant the merge changed it.
        let absorbed = match self.identities.get_mut(merged_away) {
            Some(identity) => {
                let absorbed = ::std::mem::take(&mut identity.logins);
                identity.disabled = true;
                identity.last_modified_date = now;
                absorbed
            }
            None => Vec::new(),
        };
        if let Some(identity) = self.identities.get_mut(survivor) {
            for provider in absorbed {
                if !identity.logins.contains(&provider) {
                    identity.logins.push(provider);
                }
            }
            identity.logins.sort();
            identity.last_modified_date = now;
        }
        self.open_id_tokens
            .retain(|_, issued| issued.identity_id != merged_away);
        Some(
            self.credentials
                .remove(merged_away)
                .map(|issued| issued.access_key_id)
                .into_iter()
                .collect(),
        )
    }

    /// Whether indexing `logins` for this pool would take it past
    /// [`MAX_LOGINS_PER_POOL`]. Only the logins not already indexed count:
    /// re-presenting a login this pool already knows grows nothing.
    pub fn pool_login_index_would_overflow(
        &self,
        identity_pool_id: &str,
        logins: &[(String, String)],
    ) -> bool {
        let indexed = self
            .login_index
            .keys()
            .filter(|(pool, _, _)| pool == identity_pool_id)
            .count();
        let fresh = logins
            .iter()
            .filter(|(provider, login)| {
                self.linked_identity(identity_pool_id, provider, login)
                    .is_none()
            })
            .count();
        indexed + fresh > MAX_LOGINS_PER_POOL
    }

    /// Whether linking `providers` to this identity would take it past
    /// [`MAX_LOGIN_PROVIDERS_PER_IDENTITY`]. Only providers it is not linked to yet count.
    pub fn identity_providers_would_overflow(
        &self,
        identity_id: &str,
        providers: &[String],
    ) -> bool {
        self.identities.get(identity_id).is_some_and(|identity| {
            let fresh = providers
                .iter()
                .filter(|provider| !identity.logins.contains(provider))
                .count();
            identity.logins.len() + fresh > MAX_LOGIN_PROVIDERS_PER_IDENTITY
        })
    }

    /// Forget every credential whose validity window has closed, and report the access
    /// key ids the caller must now delete from the emulator's credential store. The clock
    /// made these due: they are swept whenever credentials are issued, so an expired
    /// session is not held for the life of the process.
    pub fn purge_expired_credentials(&mut self, now: SystemTime) -> Vec<String> {
        let expired: Vec<(String, String)> = self
            .credentials
            .iter()
            .filter(|(_, issued)| issued.expiration <= now)
            .map(|(identity_id, issued)| (identity_id.clone(), issued.access_key_id.clone()))
            .collect();
        let mut revoked = Vec::with_capacity(expired.len());
        for (identity_id, access_key_id) in expired {
            self.credentials.remove(&identity_id);
            revoked.push(access_key_id);
        }
        revoked
    }

    /// Forget every OpenID Connect token whose validity window has closed. The clock made
    /// these due: an expired token names nothing, so the sweep runs whenever a token is
    /// issued and no expired token is held for the life of the process.
    pub fn purge_expired_open_id_tokens(&mut self, now: SystemTime) {
        self.open_id_tokens
            .retain(|_, issued| issued.expiration > now);
    }

    /// Record an issued OpenID Connect token, keeping the store within
    /// [`MAX_OUTSTANDING_OPEN_ID_TOKENS`]: what the clock has already expired is swept
    /// first, and if the ceiling is still reached the OLDEST issuances are forgotten to
    /// make room. Eviction rather than refusal, because every call mints a token and a
    /// caller must not be locked out of its own next token by its own history; a token
    /// this emulator has forgotten is simply one it can no longer resolve.
    pub fn record_open_id_token(
        &mut self,
        token: String,
        issued: IssuedOpenIdToken,
        now: SystemTime,
    ) {
        self.purge_expired_open_id_tokens(now);
        while self.open_id_tokens.len() >= MAX_OUTSTANDING_OPEN_ID_TOKENS {
            // Oldest first, with the token itself breaking a tie, so the eviction order is
            // the same whatever order the map iterates in.
            let oldest = self
                .open_id_tokens
                .iter()
                .min_by(|(left, left_issued), (right, right_issued)| {
                    left_issued
                        .issued_at
                        .cmp(&right_issued.issued_at)
                        .then_with(|| left.cmp(right))
                })
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            self.open_id_tokens.remove(&oldest);
        }
        self.open_id_tokens.insert(token, issued);
    }

    /// The record of a token this service issued, while its window is still open. `None`
    /// for a token this service did not issue, and for one whose window has closed.
    pub fn open_id_token_record(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Option<&IssuedOpenIdToken> {
        self.open_id_tokens
            .get(token)
            .filter(|issued| issued.expiration > now)
    }

    /// The issuance a token presented under [`COGNITO_IDENTITY_LOGIN_PROVIDER`] resolves
    /// to, or why it resolves to none: a token this service minted is honoured only INSIDE
    /// the validity window its issuance recorded, and only when this service issued it at
    /// all.
    ///
    /// The window is read from the token's OWN claims as well as from the record, because
    /// the two are the same window ([`open_id_token`] writes the recorded instants into the
    /// claims): a token whose record a sweep has already forgotten is still answered as the
    /// expired token it is rather than as one this service never issued.
    pub fn resolve_login_token(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Result<&IssuedOpenIdToken, LoginTokenRefusal> {
        if claimed_token_expiration(token).is_some_and(|expiration| expiration <= now) {
            return Err(LoginTokenRefusal::Expired);
        }
        match self.open_id_tokens.get(token) {
            Some(issued) if issued.expiration > now => Ok(issued),
            Some(_) => Err(LoginTokenRefusal::Expired),
            None => Err(LoginTokenRefusal::Unverifiable),
        }
    }

    /// Why the `Logins` map of a request made for `identity_id` authorizes nothing, if so.
    ///
    /// The one entry the map can carry under this service's own provider name has to resolve
    /// to an issuance whose window is still open ([`Self::resolve_login_token`]) AND whose
    /// identity is the one the request was made for: such a token was minted FOR one
    /// identity, so it is a login of that identity alone and of no other.
    ///
    /// Which refusal an issuance of ANOTHER identity carries is decided by the POOL that
    /// minted it, because the token is read against the pool that owns the identity the
    /// request names — each identity belonging to the pool that minted it. A token of that
    /// same pool is one of its tokens and simply not a login of the identity named
    /// ([`LoginTokenRefusal::AnotherIdentity`]); a token ANOTHER pool minted is no token of
    /// this pool at all ([`LoginTokenRefusal::AnotherPool`]), which is the refusal that keeps
    /// one pool's tokens out of another pool's identities.
    ///
    /// A map presenting no token of this service's is decided by the providers that issued
    /// its tokens, which this emulator does not reach, so it is not refused here.
    pub fn refuse_presented_login_tokens(
        &self,
        identity_id: &str,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> Option<LoginTokenRefusal> {
        let token = logins.get(COGNITO_IDENTITY_LOGIN_PROVIDER)?;
        let issued = match self.resolve_login_token(token, now) {
            Err(refusal) => return Some(refusal),
            Ok(issued) => issued,
        };
        if issued.identity_id == identity_id {
            return None;
        }
        // The pool the named identity belongs to, which is the pool the token is read
        // against. An identity this service holds no record of is reported as the missing
        // resource it is before this is reached, so there is no pool to compare for one and
        // the token is answered as the login of another identity that it is.
        let addressed_pool = self
            .identities
            .get(identity_id)
            .map(|identity| identity.identity_pool_id.as_str());
        if addressed_pool.is_some_and(|pool| pool != issued.identity_pool_id) {
            return Some(LoginTokenRefusal::AnotherPool);
        }
        Some(LoginTokenRefusal::AnotherIdentity)
    }

    /// Every identity a presented `Logins` map PROVES, each once, in the provider-name order
    /// the entries are considered in.
    ///
    /// A login proves the identity it names: a `(provider, login)` this pool has indexed
    /// against one ([`Self::linked_identity`]), and — for the entry made under this service's
    /// own provider name — the identity the token it carries was minted FOR, while that
    /// token's window is still open ([`Self::resolve_login_token`]). A login of any other
    /// kind names some identity of the provider that issued it and none of this pool's, so it
    /// proves nothing here.
    pub fn identities_proven_by(
        &self,
        identity_pool_id: &str,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> Vec<String> {
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        let mut proven: Vec<String> = Vec::new();
        for (provider, login) in presented {
            let named = if provider.as_str() == COGNITO_IDENTITY_LOGIN_PROVIDER {
                self.resolve_login_token(login, now)
                    .ok()
                    .map(|issued| issued.identity_id.clone())
            } else {
                self.linked_identity(identity_pool_id, provider, login)
                    .cloned()
            };
            if let Some(named) = named.filter(|named| !proven.contains(named)) {
                proven.push(named);
            }
        }
        proven
    }

    /// Whether `logins` presents at least one login OF `identity_id`: one of the identities
    /// the map proves ([`Self::identities_proven_by`]) is that identity. A login of any other
    /// kind names some other identity, or none at all, so it is not one of this identity's.
    pub fn logins_name_identity(
        &self,
        identity_pool_id: &str,
        identity_id: &str,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> bool {
        self.identities_proven_by(identity_pool_id, logins, now)
            .iter()
            .any(|proven| proven == identity_id)
    }

    /// The identity a POOL-SCOPED request names, or why it is not one that request may
    /// reach.
    ///
    /// An identity belongs to the pool that minted it and never moves between pools, so
    /// which pool holds it is an OWNERSHIP question that is settled before the identity is
    /// resolved inside the scope of the pool the request named: an identity of another pool
    /// is one this request may not reach, while an id no call ever minted is a resource this
    /// service does not hold. Deciding them in that order is what keeps an identity of
    /// another pool from being reported as a resource missing from the pool the request
    /// named. Each operation words the two outcomes as it words its own refusals; the
    /// identity itself comes back so a caller that goes on to read it needs no second lookup.
    pub fn scoped_identity(
        &self,
        identity_pool_id: &str,
        identity_id: &str,
    ) -> Result<&Identity, ScopedIdentityRefusal> {
        match self.identities.get(identity_id) {
            None => Err(ScopedIdentityRefusal::NotStored),
            Some(identity) if identity.identity_pool_id != identity_pool_id => {
                Err(ScopedIdentityRefusal::AnotherPool)
            }
            Some(identity) => Ok(identity),
        }
    }

    /// Why a call naming `identity` is not one its caller has proven it may make, if so.
    ///
    /// An identity a login has ever been linked to ([`Identity::has_been_linked`]) is a
    /// federated user's, and what authorizes a call for it is a login OF it
    /// ([`Self::logins_name_identity`]): a call presenting none at all has proven nothing
    /// about who it is for, and one presenting logins none of which is that identity's has
    /// proven only that it holds some OTHER user's login. An UNAUTHENTICATED (guest)
    /// identity has no login to present, so its id alone is what names it and no call for it
    /// is refused here.
    pub fn refuse_identity_access(
        &self,
        identity: &Identity,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> Option<IdentityAccessRefusal> {
        if !identity.has_been_linked() {
            return None;
        }
        if logins.is_empty() {
            return Some(IdentityAccessRefusal::NoLoginPresented);
        }
        (!self.logins_name_identity(
            &identity.identity_pool_id,
            &identity.identity_id,
            logins,
            now,
        ))
        .then_some(IdentityAccessRefusal::LoginsDontMatch)
    }

    /// The authentication methods an exchange presenting `logins` is made under: the values
    /// the `...:amr` condition key publishes to the trust policy the exchange is decided
    /// against ([`web_identity_condition_context`]).
    ///
    /// What authenticated the user is what the LOGIN says, not the name it is handed over
    /// under. An entry made under this service's own provider name carries a token this
    /// service MINTED, so the providers that authenticated the identity are the ones that
    /// token's issuance recorded — the very values its `amr` claim carries
    /// ([`open_id_token`]) — which is what tells a developer-authenticated login apart from
    /// a user-pool one at the exchange. An entry of any other provider is itself the
    /// provider that authenticated the user, and a token this service cannot resolve adds no
    /// method beyond the name it was presented under.
    pub fn exchange_authentication_methods(
        &self,
        logins: &HashMap<String, String>,
        now: SystemTime,
    ) -> Vec<String> {
        let mut presented: Vec<(&String, &String)> = logins.iter().collect();
        presented.sort();
        let mut providers: Vec<String> = Vec::new();
        for (provider, login) in presented {
            let recorded = (provider.as_str() == COGNITO_IDENTITY_LOGIN_PROVIDER)
                .then(|| self.resolve_login_token(login, now).ok())
                .flatten()
                .map(|issued| issued.authenticated_by.clone())
                .unwrap_or_default();
            if recorded.is_empty() {
                providers.push(provider.clone());
            } else {
                providers.extend(recorded);
            }
        }
        providers.sort();
        providers.dedup();
        let mut methods = authentication_methods(&providers);
        methods.sort();
        methods.dedup();
        methods
    }

    /// Mint an `IdentityPoolId` in `region` that no stored pool already uses.
    pub fn new_identity_pool_id(&self, region: &str) -> String {
        std::iter::repeat_with(|| cognito_id(region))
            .find(|id| !self.identity_pools.contains_key(id))
            .expect("the id generator is infinite")
    }

    /// Mint an `IdentityId` in `region` that no stored identity already uses.
    pub fn new_identity_id(&self, region: &str) -> String {
        std::iter::repeat_with(|| cognito_id(region))
            .find(|id| !self.identities.contains_key(id))
            .expect("the id generator is infinite")
    }

    /// Remove ONE identity with everything that named it — the logins its pool indexed
    /// against it, its place in the pool's listing order, the credentials outstanding for
    /// it and the OpenID Connect tokens issued for it — and report the access key ids whose
    /// credentials the caller must now forget. `None` when this service holds no such
    /// identity, so nothing was written.
    ///
    /// The teardown [`Self::delete_identity_pool`] does for the logins, credentials and
    /// tokens of every identity of a pool, plus the RECORD itself, which only this
    /// transition removes: once the identity is gone, a login that resolved to it resolves
    /// nothing (so presenting it again is a NEW user), a listing of the pool no longer names
    /// it, and the credentials and tokens that spoke for it speak for nobody. Removing the
    /// record is this transition's own: a merge gives an identity's LOGINS up, not the
    /// record, and leaves it stored as a disabled one ([`Self::merge_identity_into`]), and a
    /// pool delete leaves the records of the identities it minted standing.
    pub fn delete_identity(&mut self, identity_id: &str) -> Option<Vec<String>> {
        let identity = self.identities.remove(identity_id)?;
        if let Some(identities) = self.identities_by_pool.get_mut(&identity.identity_pool_id) {
            identities.retain(|listed| listed != identity_id);
        }
        self.login_index
            .retain(|(pool, _, _), linked| !(*pool == identity.identity_pool_id && linked == identity_id));
        self.open_id_tokens
            .retain(|_, issued| issued.identity_id != identity_id);
        Some(
            self.credentials
                .remove(identity_id)
                .map(|issued| issued.access_key_id)
                .into_iter()
                .collect(),
        )
    }

    /// Remove a pool with everything that belongs to it — its login index entries, its
    /// identities' outstanding credentials, the OIDC tokens it issued, the listing order of
    /// its identities and the principal-tag attribute maps of its providers — and
    /// report the access key ids whose credentials the caller must now forget. `None` when
    /// no such pool is stored.
    ///
    /// What the pool does NOT take with it is the RECORD of each identity it minted: an
    /// identity belongs to the pool that minted it and names it for as long as the record
    /// stands, so a call naming such an identity still resolves it and then finds the pool
    /// it names gone — which is why an identity-scoped call made after the pool went reports
    /// the POOL as the missing resource and not the identity. Nothing resolves to those
    /// identities any more (every login of them was indexed per pool, and the index entries
    /// go here), and no listing reports them, the pool whose listing held them being gone.
    ///
    /// That retention is BOUNDED: the delete releases the per-pool ceiling those records
    /// were counted against, so they are queued in [`Self::orphaned_identities`] and the
    /// oldest are given up as soon as the queue would pass
    /// [`MAX_ORPHANED_IDENTITY_RECORDS`], which keeps repeated create-pool / mint /
    /// delete-pool cycles from growing the store without limit. A record this eviction
    /// removes is one this emulator no longer holds at all, so a call naming it reports that
    /// identity missing.
    pub fn delete_identity_pool(&mut self, identity_pool_id: &str) -> Option<Vec<String>> {
        self.identity_pools.remove(identity_pool_id)?;
        self.identity_pool_roles.remove(identity_pool_id);
        self.principal_tag_attribute_maps
            .retain(|(pool, _), _| pool != identity_pool_id);
        let identity_ids = self
            .identities_by_pool
            .remove(identity_pool_id)
            .unwrap_or_default();
        let mut revoked = Vec::new();
        for identity_id in &identity_ids {
            if let Some(issued) = self.credentials.remove(identity_id) {
                revoked.push(issued.access_key_id);
            }
        }
        self.orphaned_identities.extend(identity_ids);
        while self.orphaned_identities.len() > MAX_ORPHANED_IDENTITY_RECORDS {
            if let Some(evicted) = self.orphaned_identities.pop_front() {
                self.identities.remove(&evicted);
            }
        }
        self.login_index
            .retain(|(pool, _, _), _| pool != identity_pool_id);
        // A token names an identity of this pool, and neither the pool nor the identity
        // exists any more, so the token names nothing.
        self.open_id_tokens
            .retain(|_, issued| issued.identity_pool_id != identity_pool_id);
        Some(revoked)
    }
}

/// An identity-pool / identity id: `REGION:GUID`, the form the modelled
/// `^[\w-]+:[0-9a-f-]+$` pattern (and real Cognito Identity) uses.
fn cognito_id(region: &str) -> String {
    format!("{region}:{}", guid())
}

/// How many ids this process has minted, so two ids minted in the same nanosecond still
/// differ in what is mixed into them.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// The GUID half of a Cognito id: 32 lowercase hex digits in RFC 4122's 8-4-4-4-12
/// layout with the version-4 and variant bits set, which is what AWS's `REGION:GUID` ids
/// carry and what the modelled `[0-9a-f-]+` half admits.
///
/// The bytes come from the standard library's `RandomState`, whose SipHash keys the host
/// seeds per process and which varies per instance, mixed with the wall clock and a
/// process-unique counter through SHA-256 (the `sha2` crate) and rendered with `hex`, so
/// no input is recoverable from an id and two ids never coincide by construction. An id
/// is a NAME, not a secret: every secret this service returns is minted by the emulator's
/// own credential store, and [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::new_identity_pool_id`]
/// / [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::new_identity_id`]
/// additionally reject a value the store already holds, so no id can alias two resources.
fn guid() -> String {
    use sha2::Digest as _;
    use std::hash::{BuildHasher as _, Hasher as _};

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut entropy = std::collections::hash_map::RandomState::new().build_hasher();
    entropy.write_u64(minted);
    entropy.write_u128(nanos);

    let mut digest = sha2::Sha256::new();
    digest.update(entropy.finish().to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    let hashed = digest.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    // RFC 4122 §4.4: the version in the high nibble of octet 6, the variant in the two
    // high bits of octet 8. Every other bit is the random material above.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&bytes[0..4]),
        hex::encode(&bytes[4..6]),
        hex::encode(&bytes[6..8]),
        hex::encode(&bytes[8..10]),
        hex::encode(&bytes[10..16])
    )
}

/// The region part of an `IdentityPoolId` / `IdentityId` (`REGION:GUID`), so an id
/// minted for a resource inside a pool lands in the pool's own region.
pub fn region_of(cognito_id: &str) -> &str {
    match cognito_id.split_once(':') {
        Some((region, _)) if is_region_like(region) => region,
        _ => DEFAULT_REGION,
    }
}

/// The region this request is for: the SigV4 credential scope's third field, which every
/// signed AWS SDK request carries. Falls back to [`DEFAULT_REGION`] for an unsigned
/// request, or one whose scope region could not appear in an id.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` can stand in the `^[\w-]+` half of an id's modelled pattern.
fn is_region_like(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `role_arn` names an IAM role OF THE ACCOUNT this emulator serves — the one
/// question this service asks of a role it is handed for an identity pool to assume.
///
/// A role an identity pool is configured with is a role the caller PASSES to Amazon
/// Cognito, and a service can only be passed a role of the account it is acting for: a
/// value naming another account's role, and a value naming no role at all, are the same
/// answer here, because the pass is decided about the value as the role of some account
/// that is not this one rather than parsed as a parameter. What the refusal then says is
/// [`CROSS_ACCOUNT_PASS_ROLE_MESSAGE`].
///
/// The reading is `service_common::iam_role::role_name`, the tree's one resolution of a
/// caller-supplied IAM ARN onto an entity of this account (the `iam` vendor, no Region, this
/// account, a `role/<path><name>` resource), so this service and every other one handed a
/// role ARN read it the same way.
pub fn names_an_assumable_role(role_arn: &str) -> bool {
    ::service_common::iam_role::role_name(role_arn).is_some()
}

/// How AWS words the refusal of a role a service may not be passed: a value that names no
/// role of the caller's own account.
///
/// Fixed text with no request- or state-derived part — the value is the caller's own input
/// and the refusal adds nothing by echoing it — and the sentence every emulated service
/// that authorizes a pass of a caller-supplied role answers with, so a client reads one
/// wording for one rule.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// This service's ARN namespace, from the model's own `aws.api#service` trait
/// (`smithy_query(query="service_info")`: `arnNamespace: cognito-identity`). It is the
/// third field of every ARN that names one of this service's resources.
pub const ARN_NAMESPACE: &str = "cognito-identity";

/// The resource type an identity pool's ARN names. An Amazon Cognito identity pool is
/// named by `arn:<partition>:cognito-identity:<region>:<account>:identitypool/<IdentityPoolId>`,
/// which is the ARN form the tagging operations' `ResourceArn` carries.
pub const IDENTITY_POOL_RESOURCE_TYPE: &str = "identitypool";

/// The ARN that names the identity pool `identity_pool_id`: this service's namespace, the
/// region the pool's own id names, the emulator's canonical account id
/// ([`::engine::emulator_account_id`]) and the pool as the resource.
pub fn identity_pool_arn(identity_pool_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region_of(identity_pool_id),
        &format!("{IDENTITY_POOL_RESOURCE_TYPE}/{identity_pool_id}"),
    )
}

/// The `IdentityPoolId` an identity-pool ARN names, or `None` when `arn` does not name an
/// identity pool of this service at all (a different ARN namespace or resource type, a
/// malformed account or partition, or no pool id after the resource type).
///
/// Only the ARN's FORM is checked here, and only the pool id is returned: an
/// `IdentityPoolId` is unique across this service, so it is the id — not the ARN's region
/// or account, which are the naming context around it — that selects the stored pool.
/// Whether that pool exists is the caller's lookup, which is what distinguishes a
/// malformed `ResourceArn` (an input error) from one naming a resource that is not stored
/// (a missing resource).
pub fn identity_pool_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(namespace), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let partition_is_named = !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    // An AWS account id is twelve digits.
    let account_is_an_id = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    if namespace != ARN_NAMESPACE
        || !partition_is_named
        || !is_region_like(region)
        || !account_is_an_id
    {
        return None;
    }
    resource
        .strip_prefix(IDENTITY_POOL_RESOURCE_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|identity_pool_id| !identity_pool_id.is_empty())
}

/// The pagination scope of the account-wide identity-pool listing, for a listing that
/// pages the account's POOLS rather than the contents of one pool. No `IdentityPoolId`
/// can take this value — a modelled id is `REGION:GUID` (`^[\w-]+:[0-9a-f-]+$`), so it
/// always carries a colon — which keeps a token of this listing from resuming a listing
/// inside a pool, and the other way round.
pub const IDENTITY_POOL_LISTING_SCOPE: &str = "identitypools";

/// An opaque pagination token naming the listing it was minted for (`scope`: the pool a
/// listing pages the contents of, or [`IDENTITY_POOL_LISTING_SCOPE`] for the account's
/// pools) and the item a page resumes at. Hex, so the token is opaque to a caller and
/// still matches the modelled `^[\S]+$` pattern.
pub fn encode_pagination_token(scope: &str, cursor: &str) -> String {
    hex::encode(format!("{scope}#{cursor}"))
}

/// The cursor inside a token from [`encode_pagination_token`], or `None` when the token
/// was not produced by this service for THIS listing — a token minted for one listing
/// cannot resume another.
pub fn decode_pagination_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (minted_for, cursor) = decoded.split_once('#')?;
    (minted_for == scope).then(|| cursor.to_string())
}

/// The OpenID Connect token for `identity_id` in `identity_pool_id`, valid from
/// `issued_at` until `expiration` and authenticated by `authenticated_by` (the provider
/// names the issuing call presented).
///
/// The claims are the OIDC registered ones (RFC 7519 §4.1) an Amazon Cognito Identity
/// token carries: the service's own `iss`, the identity pool as the audience, the
/// identity as the subject, the validity window, the `amr` authentication-methods
/// reference ([`authentication_methods`]) and a `jti` that makes every issued token
/// distinct, so two calls in the same second never produce one token.
pub fn open_id_token(
    identity_pool_id: &str,
    identity_id: &str,
    authenticated_by: &[String],
    issued_at: SystemTime,
    expiration: SystemTime,
) -> String {
    let amr: Vec<::serde_json::Value> = authentication_methods(authenticated_by)
        .iter()
        .map(|method| ::serde_json::Value::from(method.as_str()))
        .collect();
    let claims = ::serde_json::json!({
        "iss": OIDC_TOKEN_ISSUER,
        "aud": identity_pool_id,
        "sub": identity_id,
        "amr": amr,
        "iat": unix_seconds(issued_at),
        "exp": unix_seconds(expiration),
        "jti": guid(),
    });
    unsecured_jws(&claims)
}

/// `claims` as an UNSECURED JWS in compact serialization (RFC 7515 §6, "Using the
/// Algorithm none"): `base64url(header) '.' base64url(claims) '.'`, with an empty
/// signature segment.
///
/// Amazon Cognito Identity signs its OIDC tokens with a key pair whose public half it
/// publishes. This emulator holds no such key pair, so it issues the form RFC 7515
/// defines for a JWS that carries no signature rather than filling the third segment with
/// bytes that would only look like one. The token is still this emulator's own: its claims
/// come from stored state and every token issued is recorded in
/// [`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::open_id_tokens`], so the
/// service can say which identity a token it handed out names.
fn unsecured_jws(claims: &::serde_json::Value) -> String {
    let header = ::serde_json::json!({ "alg": "none", "typ": "JWT" });
    format!(
        "{}.{}.",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes())
    )
}

/// `bytes` in base64url without padding, as a JWS segment is encoded (RFC 7515 §2): the
/// standard-alphabet codec this workspace already uses for every Smithy blob
/// (`aws_smithy_types::base64`), with `+` and `/` replaced by `-` and `_` and the padding
/// removed.
pub fn base64_url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// The claim set a token this service minted carries, read back out of the token itself:
/// the payload segment of the unsecured JWS [`open_id_token`] produced. `None` for a value
/// that is not one of this service's tokens — one that is no compact JWS, one whose payload
/// is not a JSON claim set, or one naming another issuer.
///
/// The reading is [`::service_common::jwt`], the tree's ONE bounded reading of a compact
/// JWS, through the reader a service that MINTED the token it is reading calls: the tokens
/// [`unsecured_jws`] produces carry no signature, which is the form that reader admits.
/// Nothing is VERIFIED here and nothing is granted on what it reads — an unsecured token is
/// one nothing signed, so the claim set is only ever read to REFUSE a token the service's
/// own issuance registry also refuses.
fn minted_token_claims(token: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let read = ::service_common::jwt::read_unsecured(token).ok()?;
    (read.claim_str(ISSUER_CLAIM) == Some(OIDC_TOKEN_ISSUER)).then_some(read.claims)
}

/// The claim naming the instant a token's validity window closes (RFC 7519 §4.1.4).
const EXPIRATION_CLAIM: &str = "exp";

/// The instant a token this service minted says its validity window closes at, from the
/// `exp` claim [`open_id_token`] writes it into. `None` for a value that is not one of this
/// service's tokens, or whose claim set carries no instant there.
pub fn claimed_token_expiration(token: &str) -> Option<SystemTime> {
    let seconds = minted_token_claims(token)?
        .get(EXPIRATION_CLAIM)?
        .as_u64()?;
    // An instant past what this platform can represent is no instant the clock has
    // reached, so it is answered as no window rather than overflowing.
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

/// `time` as whole seconds since the epoch, which is how a token claim carries an instant
/// (RFC 7519 §2's NumericDate).
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitoidentityAwscognitoidentityserviceState>)
}

// ---------------------------------------------------------------------------
// The role session a pool vends: the web-identity assume behind
// `GetCredentialsForIdentity`.
// ---------------------------------------------------------------------------

/// The `RoleSessionName` every role session an identity pool vends carries, and so the
/// last field of the assumed-role ARN a vended session speaks for.
///
/// The caller of `GetCredentialsForIdentity` names no session: the pool's own exchange
/// does, with one name for every identity it vends for.
pub const VENDED_ROLE_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The key of the pool role a vend assumes for a request that PRESENTED a login, from the
/// modelled `RoleType` pattern `^(un)?authenticated$` and the `Roles` doc ("the key will
/// be either \"authenticated\" or \"unauthenticated\"").
pub const AUTHENTICATED_ROLE_TYPE: &str = "authenticated";

/// The key of the pool role a vend assumes for a request that presented NO login; see
/// [`AUTHENTICATED_ROLE_TYPE`].
pub const UNAUTHENTICATED_ROLE_TYPE: &str = "unauthenticated";

/// The IAM action a role's trust policy has to grant the Cognito Identity principal for a
/// pool to be able to vend a session of it: "Your role trust policy must grant
/// AssumeRoleWithWebIdentity permissions to cognito-identity.amazonaws.com"
/// (`InvalidIdentityPoolConfigurationException`, this service's model).
pub const ASSUME_ROLE_WITH_WEB_IDENTITY_ACTION: &str = "sts:AssumeRoleWithWebIdentity";

/// The condition key carrying the identity pool a vended session is being assumed FOR —
/// `<the Cognito Identity principal>:aud`. A role's trust policy conditions the assume on
/// it, which is what keeps one identity pool from federating into another pool's role
/// (`AssumeRoleWithWebIdentityRequest$RoleArn`: the trust policies of these roles "must
/// contain the cognito-identity.amazonaws.com:aud condition key to restrict role
/// assumption to users from your intended identity pools").
pub const AUDIENCE_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:aud";

/// The condition key carrying the identity the session is vended for: the `sub` claim of
/// the token the exchange is made with.
pub const SUBJECT_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:sub";

/// The condition key carrying the authentication methods the exchange was made under —
/// `authenticated` or `unauthenticated`, beside the providers that authenticated the user.
/// Multi-valued, so a trust policy decides it under one of IAM's set qualifiers.
pub const AUTHENTICATION_METHODS_CONDITION_KEY: &str = "cognito-identity.amazonaws.com:amr";

/// The condition keys a vend publishes about the assume it makes, so a trust-policy clause
/// over one of them is read as "the request carried no value" rather than as a key this
/// emulator does not model; every other key stays undecided and withholds the grant.
pub const WEB_IDENTITY_PUBLISHED_CONDITION_KEYS: &[&str] = &[
    AUDIENCE_CONDITION_KEY,
    SUBJECT_CONDITION_KEY,
    AUTHENTICATION_METHODS_CONDITION_KEY,
];

/// How AWS words the refusal of a vend the pool's role configuration cannot serve: the
/// pool names no role for the request, the role it names is not one the account holds, or
/// that role's trust policy does not admit an assume by this pool's users.
///
/// One fixed sentence for all three, naming neither the role nor the pool: the caller of
/// `GetCredentialsForIdentity` is an unauthenticated one, so the refusal reports nothing
/// about the pool's configuration beyond the fact that it is the configuration at fault.
pub const INVALID_POOL_CONFIGURATION_MESSAGE: &str =
    "Invalid identity pool configuration. Check assigned IAM roles for this pool.";

/// How AWS words the refusal of a vend a role MAPPING decided against: the mapping keyed by
/// the provider the login was made under resolved no role from the presented token and states
/// the `Deny` resolution for that case, so the exchange is not authorized to receive any
/// credential.
///
/// This is not the pool-configuration refusal above: the pool may well configure a role for
/// the flow the request is in, and `InvalidIdentityPoolConfigurationException` is the error
/// for a pool that "has no authenticated role configured, or STS returned an error response"
/// (that error's own doc, this service's model). A mapping that denies the exchange is an
/// AUTHORIZATION decision about the token presented, which is `NotAuthorizedException` —
/// "thrown when a user is not authorized to access the requested resource".
///
/// The refusal names WHICH mapped identity provider decided against the exchange —
/// `provider` is the identity provider the login being resolved was made under, the name the
/// pool's `RoleMappings` entry is keyed by (for a user-pool provider,
/// `cognito-idp.<region>.amazonaws.com/<user pool id>`) — around text that is otherwise
/// fixed for every mapping type and every reason a mapping resolved no role. Nothing else of
/// the pool's configuration is reported: the caller of `GetCredentialsForIdentity` is an
/// unauthenticated one.
pub fn denied_role_resolution_message(provider: &str) -> String {
    format!("The ambiguous role mapping rules for: {provider} denied this request.")
}

/// A role as the IAM store in the same engine holds it, reduced to the facts an assume
/// decides on.
///
/// STS's OWN type, read through STS's own reader, so a role this service assumes is the
/// very record `sts:AssumeRole` would resolve for the same ARN.
pub type ResolvedRole =
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ResolvedRole;

/// The STS session registry in the SAME shared engine: where the sessions
/// `sts:GetCallerIdentity` resolves live.
///
/// STS's own accessor is called rather than copied, so a session this service mints is
/// recorded in the very registry that service reports from. `None` means the engine
/// carries no such store.
pub fn sts_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>>
{
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::service_state(engine)
}

/// The role `role_arn` names, as the IAM store in the same engine holds it, or `None` when
/// the account holds no such role — which is also the answer for an ARN of another account,
/// and for an engine that carries no IAM store at all.
pub fn resolve_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<ResolvedRole> {
    let iam = ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::iam_state(
        engine,
    )?;
    let iam = iam.lock();
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::resolve_role(
        &iam, role_arn,
    )
}

/// The condition keys a vend publishes about the assume it is making, for a trust policy's
/// `Condition` to be decided against: the pool the session is vended for, the identity it
/// is vended to, and the authentication methods the exchange was made under.
///
/// `methods` are those authentication methods
/// ([`ComAmazonawsCognitoidentityAwscognitoidentityserviceState::exchange_authentication_methods`]),
/// which is what makes the exchange an authenticated or an unauthenticated one. Nothing is
/// invented: every pair comes from the request, from the stored identity it names and from
/// the issuance of the token it presents.
pub fn web_identity_condition_context(
    identity_pool_id: &str,
    identity_id: &str,
    methods: &[String],
) -> Vec<(String, String)> {
    let mut context = vec![
        (
            AUDIENCE_CONDITION_KEY.to_string(),
            identity_pool_id.to_string(),
        ),
        (SUBJECT_CONDITION_KEY.to_string(), identity_id.to_string()),
    ];
    context.extend(
        methods
            .iter()
            .map(|method| (AUTHENTICATION_METHODS_CONDITION_KEY.to_string(), method.clone())),
    );
    context
}

/// The authentication methods an exchange made with `providers` — the `Logins` provider
/// names the call presented — was performed under.
///
/// A call that presented at least one login is an AUTHENTICATED one, and names
/// `authenticated`, this service (every token is minted by it, whatever provider
/// authenticated the user) and each of those providers; a call that presented none is
/// UNAUTHENTICATED and names that alone. One function, because these are the values a
/// minted token carries in its `amr` claim ([`open_id_token`]) AND the ones the
/// `...:amr` condition key publishes to a trust policy
/// ([`web_identity_condition_context`]): a policy clause written against the token a
/// caller holds has to decide the same way as the exchange that token is made with.
pub fn authentication_methods(providers: &[String]) -> Vec<String> {
    if providers.is_empty() {
        return vec![UNAUTHENTICATED_ROLE_TYPE.to_string()];
    }
    let mut methods = vec![
        AUTHENTICATED_ROLE_TYPE.to_string(),
        COGNITO_IDENTITY_PRINCIPAL.to_string(),
    ];
    methods.extend(providers.iter().cloned());
    methods
}

/// Whether `role`'s trust policy admits the web-identity assume `context` describes.
///
/// The DECISION is [`::service_common::iam_authorization`], the tree's one evaluation of
/// IAM policy documents, read as the RESOURCE-based policy a trust policy is: its
/// statements name the principals they grant to — here the Cognito Identity service
/// principal, the `Principal.Federated` spelling an identity pool's users are admitted by
/// — and a statement naming no resource covers the role itself. A role carrying no trust
/// policy, or one this cannot read as a policy, admits nobody: nothing is assumed on
/// evidence the account does not hold.
pub fn role_admits_web_identity(role: &ResolvedRole, context: &[(String, String)]) -> bool {
    let Some(document) = role.trust_policy.as_deref() else {
        return false;
    };
    let principals = [COGNITO_IDENTITY_PRINCIPAL.to_string()];
    let request = ::service_common::iam_authorization::Request::new(
        ASSUME_ROLE_WITH_WEB_IDENTITY_ACTION,
        &role.arn,
        &principals,
    )
    .with_context(context)
    .with_published_keys(WEB_IDENTITY_PUBLISHED_CONDITION_KEYS);
    ::service_common::iam_authorization::resource_decision(document, &request)
        == ::service_common::iam_authorization::Decision::Allowed
}

/// The session a vend hands back: the credential the caller signs with, and the
/// assumed-role identity it speaks for.
#[derive(Debug, Clone)]
pub struct VendedSession {
    pub access_key_id: String,
    pub secret_key: String,
    pub session_token: String,
    pub expiration: SystemTime,
    /// `arn:<partition>:sts::<account>:assumed-role/<role>/<session>`, what
    /// `sts:GetCallerIdentity` answers for a request signed with this credential.
    pub assumed_role_arn: String,
    /// `<RoleId>:<RoleSessionName>`, the `AssumedRoleUser$AssumedRoleId` of the assume.
    pub assumed_role_id: String,
}

/// Mint the role session a vend hands back: an assumed-role session of `role`, recorded in
/// the STS session registry in the same engine ([`sts_state`]) by STS's own mint, so
/// `sts:GetCallerIdentity` under the credential reports the assumed-role ARN and anything
/// correlating the session back to its role matches. That same mint records the session
/// where a service resolves a temporary credential's principal, so a call the vended
/// credential signs is judged on the policies in force on the role it is a session of.
///
/// The session lasts `validity`, capped by the role's own `MaxSessionDuration` — a session
/// is never minted for longer than the role admits.
///
/// `federated_claims` are the condition keys the exchange this vend makes publishes about
/// the token it stands for ([`web_identity_condition_context`]); they travel with the
/// session, so a request the vended credential signs is decided against the same keys the
/// assume was.
///
/// `Err` says why no session could be minted — this engine carries no STS registry, or the
/// registry is full of sessions that have not expired — so the caller refuses the request
/// rather than answering with a credential that stands for nothing.
pub fn mint_vended_role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    role: &ResolvedRole,
    now: SystemTime,
    validity: Duration,
    federated_claims: &[(String, String)],
) -> Result<VendedSession, String> {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

    let Some(registry) = sts_state(engine) else {
        return Err(
            "the session registry the vended credentials are recorded in is unavailable"
                .to_string(),
        );
    };
    let duration_seconds = i32::try_from(validity.as_secs())
        .unwrap_or(i32::MAX)
        .min(role.max_session_duration);
    let assumed_role_id = format!("{}:{VENDED_ROLE_SESSION_NAME}", role.role_id);
    let assumed_role_arn = sts::sts_arn(
        &role.account,
        &format!("assumed-role/{}/{VENDED_ROLE_SESSION_NAME}", role.role_name),
    );
    let identity = sts::Identity {
        user_id: assumed_role_id.clone(),
        account: role.account.clone(),
        arn: assumed_role_arn.clone(),
    };
    // Minted through the token service's shared role-session mint, which also records the
    // session where a service resolves a temporary credential's principal: a call the vended
    // credential signs is then judged on the policies in force on the role it is a session
    // of.
    let minted = sts::mint_role_session(
        engine,
        store,
        &registry,
        now,
        sts::NewSession {
            role_arn: Some(role.arn.clone()),
            // The identity the exchanged token stands for travels with the session, so a
            // policy in force on the role can scope what the session may do to its own
            // federated subject.
            federated_claims: federated_claims.to_vec(),
            ..sts::NewSession::new(identity, sts::SessionKind::AssumedRole, duration_seconds)
        },
    )
    .map_err(|why| why.message())?;
    Ok(VendedSession {
        access_key_id: minted.credentials.access_key_id,
        secret_key: minted.credentials.secret_access_key,
        session_token: minted.credentials.session_token,
        expiration: minted.session.expires_at,
        assumed_role_arn,
        assumed_role_id,
    })
}

/// Forget a session this service vended, in both stores that hold it: the credential store
/// (so a request still presenting it no longer verifies) and the STS session registry (so
/// it names no identity either), and in the account's session directory (so no record of a
/// later call names a credential nobody holds).
pub fn discard_vended_role_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    access_key_id: &str,
) {
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::forget_audited_session(
        engine,
        access_key_id,
    );
    match sts_state(engine) {
        Some(registry) => registry.lock().discard(access_key_id, store),
        None => {
            let _ = store.delete(access_key_id);
        }
    }
}

/// What the audit record of one vended assume reports.
pub struct VendedAssume<'a> {
    /// The role the session was assumed of.
    pub role: &'a ResolvedRole,
    /// The session that was minted.
    pub session: &'a VendedSession,
    /// The identity pool the assume was made for: the `aud` of the exchanged token.
    pub identity_pool_id: &'a str,
    /// The identity it was made for: the `sub` of the exchanged token.
    pub identity_id: &'a str,
    /// The Region the vend was served in.
    pub region: &'a str,
    /// How long the minted session lasts.
    pub duration_seconds: i32,
}

/// `userIdentity.type` of a call made by a user a web identity provider authenticated,
/// which is who an identity pool's exchange assumes the role on behalf of.
const WEB_IDENTITY_PRINCIPAL_TYPE: &str = "WebIdentityUser";

/// The resource type an assume's audit record names the role by.
const ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";

/// Record the web-identity assume a vend made in the account's management-event log, so
/// CloudTrail's `LookupEvents` reports it — the event is STS's, because the assume is: the
/// pool exchanges the token for a role session through `sts:AssumeRoleWithWebIdentity`.
///
/// Only a vend that was SERVED is recorded: a vend the role's trust policy refused assumed
/// nothing, so there is no assume to report.
///
/// Recording is best-effort by design, as every other recorder in this tree: an engine
/// carrying no log (a unit test over a bare engine) still serves the call, because failing
/// a caller because the audit log is unreachable would be a worse answer than serving it
/// unrecorded. No secret is written — the record carries the session's access key id and
/// expiry, never its secret key or session token.
pub fn record_vended_assume<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
    assume: &VendedAssume<'_>,
) {
    let Some(log) = management_event_log(engine) else {
        ::tracing::warn!(
            "the engine carries no management-event log: the vended assume was served \
             unrecorded"
        );
        return;
    };
    let expiration = ::aws_smithy_types::DateTime::from(assume.session.expiration)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    log.lock().record(
        ::service_common::management_events::ServedApiCall {
            event_name: "AssumeRoleWithWebIdentity".to_string(),
            event_source: STS_EVENT_SOURCE.to_string(),
            aws_region: assume.region.to_string(),
            read_only: false,
            // The assume is made for a federated user, which holds no access key of its
            // own: what names it is the identity the token carries.
            access_key_id: None,
            user_name: Some(assume.identity_id.to_string()),
            principal_type: WEB_IDENTITY_PRINCIPAL_TYPE.to_string(),
            // What identifies a federated user is the token that was exchanged: the
            // provider that issued it, the audience it was issued for (the identity pool)
            // and the subject it names (the identity), which is the principal the
            // assumed session speaks for.
            principal_id: Some(format!(
                "{COGNITO_IDENTITY_PRINCIPAL}:{}:{}",
                assume.identity_pool_id, assume.identity_id
            )),
            // The provider that authenticated the user the assume was made for is the
            // identity pool's own service principal: the token the assume exchanged is one
            // Amazon Cognito Identity issued, and `userIdentity.identityProvider` is the
            // external provider's principal name.
            identity_provider: Some(COGNITO_IDENTITY_PRINCIPAL.to_string()),
            // The assume itself is made ON BEHALF of a federated user, not BY a session:
            // the session it mints is what LATER calls carry, and each of those records
            // names it through the account's session directory.
            session: None,
            account_id: ::engine::emulator_account_id().to_string(),
            user_agent: ::service_common::management_events::recorded_user_agent(parts),
            resources: vec![::service_common::management_events::AuditedResource::new(
                ROLE_RESOURCE_TYPE,
                assume.role.arn.clone(),
            )],
            request_parameters: ::serde_json::json!({
                "roleArn": assume.role.arn,
                "roleSessionName": VENDED_ROLE_SESSION_NAME,
                "durationSeconds": assume.duration_seconds,
            }),
            response_elements: Some(::serde_json::json!({
                "credentials": {
                    "accessKeyId": assume.session.access_key_id,
                    "expiration": expiration,
                },
                "assumedRoleUser": {
                    "assumedRoleId": assume.session.assumed_role_id,
                    "arn": assume.session.assumed_role_arn,
                },
                "provider": COGNITO_IDENTITY_PRINCIPAL,
                "audience": assume.identity_pool_id,
                "subjectFromWebIdentityToken": assume.identity_id,
            })),
            error_code: None,
            error_message: None,
        },
        SystemTime::now(),
    );
}

/// The `eventSource` an assume's audit record carries: STS's endpoint prefix in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event, because the assume is a
/// call to STS however it was reached.
pub const STS_EVENT_SOURCE: &str = "sts.amazonaws.com";

/// This service's own `eventSource`: its endpoint prefix (`smithy_query(query="service_info")`
/// -> `endpointPrefix: cognito-identity`) in the same form.
pub const EVENT_SOURCE: &str = "cognito-identity.amazonaws.com";

/// What a vend reports about itself for its own audit record.
/// The CloudFormation-style type name of the resource an audit record of this service refers
/// to.
const IDENTITY_POOL_RESOURCE_TYPE_NAME: &str = "AWS::Cognito::IdentityPool";

/// What a handler of this service reports about the call it has just served, for that
/// call's own audit record.
pub struct ServedCall<'a> {
    /// The operation's modelled name (`Self::NAME`), as the event's `eventName`.
    pub operation: &'a str,
    /// The Region the call was served in.
    pub region: &'a str,
    /// Whether the call only REPORTED state. CloudTrail records it as the event's
    /// `readOnly`, which a lookup can filter on.
    pub read_only: bool,
    /// The identity pool the call addressed, when it addressed one: the resource the
    /// event names. A request naming a pool this service does not hold still addressed
    /// that pool, but one that names no pool at all records no resource rather than one
    /// built from an id nothing resolves.
    pub identity_pool_id: Option<String>,
    /// The request's parameters, named as the event document names them (see
    /// [`service_common::management_events::recorded_members`]).
    pub request_parameters: ::serde_json::Value,
    /// The values the response carried, for a response whose members are not secrets;
    /// `None` for a call that answers nothing, or one whose answer is a credential.
    pub response_elements: Option<::serde_json::Value>,
}

/// Record a call this service served in the account's management-event log, whatever it
/// answered, so CloudTrail's `LookupEvents` reports it — with the error code and message
/// its caller was answered with when it was refused.
///
/// What is recorded is this service's ADMINISTRATION of identity pools: the calls a client
/// makes with AWS developer credentials. The IDENTITY DATA PLANE — `GetId`,
/// `GetOpenIdToken`, `GetCredentialsForIdentity`, `UnlinkIdentity` and
/// `GetOpenIdTokenForDeveloperIdentity`, the five calls that mint or exchange an identity in
/// a pool's CONTENTS — is not: those calls are data events, which CloudTrail delivers to a
/// trail's bucket and never to event history, so their handlers record through
/// [`data_events::hold_data_event`] instead. The `sts:AssumeRoleWithWebIdentity` a served
/// vend makes IS a management event of STS and is recorded as that
/// ([`record_vended_assume`]).
///
/// Of the calls that ARE recorded, every one is, a reporting one included, because the log
/// is the account's record of the API calls that were MADE and not a projection of this
/// service's resources: what a reporting call reports is untouched by its own record, and a
/// client auditing identity-pool administration reads the reads as well as the writes (an
/// event's `readOnly` is what tells the two apart, and what a lookup filters on).
///
/// Recording is best-effort by design, as every other recorder in this tree: an engine
/// carrying no log (a unit test over a bare engine) still serves the call, because
/// failing a caller because the audit log is unreachable would be a worse answer than
/// serving it unrecorded.
///
/// No secret is written: a provider token, an OpenID token and a vended session are the
/// caller's own and are left out of both `requestParameters` and `responseElements` by
/// the handler that fills them.
///
/// A call this service stopped at AUTHORIZATION records neither the parameters nor the
/// response elements, as every recorder in this tree does
/// ([`service_common::management_events::recorded_request_parameters`]): the record of such
/// a call says that a caller asked for something it may not do, not what the refused
/// request held.
pub fn record_served_call<Eng: Send + Sync + 'static, T, E>(
    engine: &Eng,
    parts: &http::request::Parts,
    call: &ServedCall<'_>,
    outcome: &Result<T, E>,
) where
    E: ::smithy_server_runtime::OperationError + ::std::fmt::Display,
{
    let Some(log) = management_event_log(engine) else {
        // Not silent: an unrecorded call leaves no trace in the log by definition, so it
        // leaves one here instead.
        ::tracing::warn!(
            operation = call.operation,
            "the engine carries no management-event log: the call was served unrecorded"
        );
        return;
    };
    let (error_code, error_message) =
        ::service_common::management_events::recorded_failure(outcome);
    // Whether this service's PERMISSION CHECK stopped the call: its gate
    // ([`authorization::denied`]) is placed before an implemented handler reads or writes
    // anything, and the refusal it answers is the only one carrying this code and status —
    // every other refusal of this service is one an operation reached while serving the
    // call, so that call was dispatched and its record keeps what it carried.
    let stopped_at_authorization = outcome.as_ref().err().is_some_and(|error| {
        error.shape_name() == crate::scaffolded::ACCESS_DENIED_CODE
            && error.http_status() == crate::scaffolded::ACCESS_DENIED_STATUS
    });
    let caller = ::service_common::management_events::recorded_caller(parts);
    let session = recorded_session(engine, caller.access_key_id.as_deref());
    log.lock().record(
        ::service_common::management_events::ServedApiCall {
            event_name: call.operation.to_string(),
            event_source: EVENT_SOURCE.to_string(),
            aws_region: call.region.to_string(),
            read_only: call.read_only,
            access_key_id: caller.access_key_id,
            user_name: session
                .as_ref()
                .map_or(caller.user_name, |session| session.user_name.clone()),
            principal_type: session.as_ref().map_or_else(
                || caller.principal_type.to_string(),
                |session| session.principal_type.clone(),
            ),
            // The unique id of the IAM entity a LONG-TERM credential speaks for is IAM's
            // to mint, and this service resolves none, so such a record leaves it out
            // rather than naming a principal nothing resolves. A session's own principal
            // id is the session's, and the directory holds it.
            principal_id: session.as_ref().map(|session| session.principal_id.clone()),
            // Every operation of this service is called either with an AWS credential or
            // with none at all, so no caller of one is a federated user an external
            // provider authenticated.
            identity_provider: None,
            // The session the call was made BY, when it was made with one — the shared
            // directory the minting service wrote it to, so a call this service serves
            // under a role session is recorded as that session.
            session: session.clone(),
            account_id: ::engine::emulator_account_id().to_string(),
            user_agent: ::service_common::management_events::recorded_user_agent(parts),
            resources: call
                .identity_pool_id
                .as_deref()
                .map(|identity_pool_id| {
                    ::service_common::management_events::AuditedResource::new(
                        IDENTITY_POOL_RESOURCE_TYPE_NAME,
                        identity_pool_arn(identity_pool_id),
                    )
                })
                .into_iter()
                .collect(),
            request_parameters: ::service_common::management_events::recorded_request_parameters(
                call.request_parameters.clone(),
                stopped_at_authorization,
            ),
            response_elements: call.response_elements.clone(),
            error_code,
            error_message,
        },
        SystemTime::now(),
    );
}

/// The pool an identity belongs to, for a call that names only the identity. `None` for
/// an identity this service does not hold.
pub fn identity_pool_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity_id: &str,
) -> Option<String> {
    service_state(engine).and_then(|state| {
        state
            .lock()
            .identities
            .get(identity_id)
            .map(|identity| identity.identity_pool_id.clone())
    })
}

/// What a request carrying a whole identity-pool configuration sent, for its audit
/// record.
///
/// `CreateIdentityPool` and `UpdateIdentityPool` carry the SAME members — an update also
/// names the pool it rewrites — so one writer serves both and the two records cannot
/// drift apart. `SupportedLoginProviders` maps a provider name to the application id the
/// DEVELOPER registered with that provider, which is the pool's configuration rather than
/// a caller's secret, so it is recorded as sent.
pub struct RecordedPoolRequest<'a> {
    /// The pool the request rewrites, for an update; `None` for a create, which names
    /// none.
    pub identity_pool_id: Option<&'a str>,
    pub identity_pool_name: &'a str,
    pub allow_unauthenticated_identities: bool,
    pub allow_classic_flow: Option<bool>,
    pub supported_login_providers: Option<&'a ::std::collections::HashMap<String, String>>,
    pub developer_provider_name: Option<&'a str>,
    pub open_id_connect_provider_arns: Option<&'a [String]>,
    pub cognito_identity_providers: Option<&'a [CognitoIdentityProvider]>,
    pub saml_provider_arns: Option<&'a [String]>,
    pub identity_pool_tags: Option<&'a ::std::collections::HashMap<String, String>>,
}

impl RecordedPoolRequest<'_> {
    /// These members as the event document's `requestParameters`.
    pub fn recorded(&self) -> ::serde_json::Value {
        let providers = self.cognito_identity_providers.map(|providers| {
            providers
                .iter()
                .map(|provider| {
                    ::service_common::management_events::recorded_members([
                        ("providerName", ::serde_json::json!(provider.provider_name)),
                        ("clientId", ::serde_json::json!(provider.client_id)),
                        (
                            "serverSideTokenCheck",
                            ::serde_json::json!(provider.server_side_token_check),
                        ),
                    ])
                })
                .collect::<Vec<::serde_json::Value>>()
        });
        ::service_common::management_events::recorded_members([
            ("identityPoolId", ::serde_json::json!(self.identity_pool_id)),
            (
                "identityPoolName",
                ::serde_json::json!(self.identity_pool_name),
            ),
            (
                "allowUnauthenticatedIdentities",
                ::serde_json::json!(self.allow_unauthenticated_identities),
            ),
            (
                "allowClassicFlow",
                ::serde_json::json!(self.allow_classic_flow),
            ),
            (
                "supportedLoginProviders",
                ::serde_json::json!(self.supported_login_providers),
            ),
            (
                "developerProviderName",
                ::serde_json::json!(self.developer_provider_name),
            ),
            (
                "openIdConnectProviderARNs",
                ::serde_json::json!(self.open_id_connect_provider_arns),
            ),
            ("cognitoIdentityProviders", ::serde_json::json!(providers)),
            (
                "samlProviderARNs",
                ::serde_json::json!(self.saml_provider_arns),
            ),
            (
                "identityPoolTags",
                ::serde_json::json!(self.identity_pool_tags),
            ),
        ])
    }
}

/// The account's management-event (audit) log in the SAME shared engine, the one store
/// CloudTrail's `LookupEvents` reads. `None` means the engine carries no such log.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::management_events::ManagementEventLog>)
}

/// The account's SESSION directory in that same shared engine: the sessions STS minted,
/// each keyed by the access key id it was minted under.
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

/// The session the credential a request was signed with belongs to, for a request signed
/// with one: what the record of that call carries as its `userIdentity.sessionContext`.
///
/// `None` for a long-term credential, for an unsigned request — every vend leg is one —
/// and for an engine that carries no directory.
pub fn recorded_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: Option<&str>,
) -> Option<::service_common::management_events::RecordedSession> {
    let access_key_id = access_key_id?;
    session_audit_directory(engine)
        .and_then(|directory| directory.lock().session(access_key_id).cloned())
}

/// The registry of web identity tokens THIS emulator issued, in the same shared engine:
/// the one store the service that redeems such a token reads
/// (`sts:AssumeRoleWithWebIdentity`). `None` means the engine carries no such registry.
pub fn issued_web_identity_tokens<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::web_identity_tokens::IssuedWebIdentityTokens>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::web_identity_tokens::IssuedWebIdentityTokens>)
}

/// Record, in the registry the REDEEMER of a web identity token reads, that this emulator
/// issued `token` for the identity `issued` names.
///
/// An OpenID Connect token this service mints is a bearer of an identity that another
/// service is handed back: "Calling `AssumeRoleWithWebIdentity` does not require the use of
/// Amazon Web Services security credentials ... the identity of the caller is validated by
/// using a token from the web identity provider" (`AssumeRoleWithWebIdentity`, that
/// service's model). The redeemer cannot verify a signature against an issuer's published
/// key set — this emulator fetches nothing from outside itself — so what it CAN establish
/// is that the token is one this emulator issued, and that is what this record says. The
/// claims recorded are exactly the ones the token carries ([`open_id_token`]).
///
/// Best-effort, as every recorder in this tree is: an engine carrying no registry (a unit
/// test over a bare engine) still gets its token, because failing an issuance because the
/// redemption registry is unreachable would be a worse answer than issuing it unrecorded.
pub fn record_issued_web_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    token: &str,
    issued: &IssuedOpenIdToken,
    now: SystemTime,
) {
    let Some(registry) = issued_web_identity_tokens(engine) else {
        ::tracing::warn!(
            "the engine carries no issued-web-identity registry: the token was issued \
             unrecorded, so no redeemer can establish that this emulator issued it"
        );
        return;
    };
    registry.lock().record(
        token.to_string(),
        ::service_common::web_identity_tokens::IssuedWebIdentity {
            issuer: OIDC_TOKEN_ISSUER.to_string(),
            audience: issued.identity_pool_id.clone(),
            subject: issued.identity_id.clone(),
            authentication_methods: authentication_methods(&issued.authenticated_by),
            expires_at: issued.expiration,
        },
        now,
    );
}

/// Give up every web identity token this emulator issued FOR `identity_pool_id`, in the
/// registry the redeemer of one reads.
///
/// Called by this service's delete path: a token names an identity of a pool, and once
/// neither the pool nor the identity exists the token names nothing — so the record that
/// would still resolve it to a role goes with the pool, exactly as this service's own
/// record of the issuance does.
pub fn forget_issued_web_identities<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity_pool_id: &str,
) {
    if let Some(registry) = issued_web_identity_tokens(engine) {
        registry.lock().forget_audience(identity_pool_id);
    }
}

/// Give up every web identity token this emulator issued FOR one identity, in the registry
/// the redeemer of one reads.
///
/// The same judgement [`forget_issued_web_identities`] makes, one identity rather than a
/// whole pool: a merge leaves the identity it merged away existing no longer, so a token
/// naming it names nobody and must stop resolving to a role.
pub fn forget_issued_web_identities_of_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identity_id: &str,
) {
    if let Some(registry) = issued_web_identity_tokens(engine) {
        registry.lock().forget_subject(identity_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored pool carrying only what a delete reads, under `identity_pool_id`.
    fn stored_pool(identity_pool_id: &str) -> IdentityPool {
        IdentityPool {
            identity_pool_id: identity_pool_id.to_string(),
            identity_pool_name: "bounded".to_string(),
            allow_unauthenticated_identities: true,
            allow_classic_flow: None,
            supported_login_providers: None,
            developer_provider_name: None,
            open_id_connect_provider_arns: None,
            cognito_identity_providers: None,
            saml_provider_arns: None,
            identity_pool_tags: None,
            creation_date: SystemTime::UNIX_EPOCH,
            last_modified_date: SystemTime::UNIX_EPOCH,
        }
    }

    /// Store `count` identities of `identity_pool_id`, as a mint of each would.
    fn mint_identities(
        store: &mut ComAmazonawsCognitoidentityAwscognitoidentityserviceState,
        identity_pool_id: &str,
        count: usize,
    ) -> Vec<String> {
        let now = SystemTime::UNIX_EPOCH;
        let mut minted = Vec::new();
        for index in 0..count {
            let identity_id = format!("us-east-1:{identity_pool_id}-{index}");
            store.identities.insert(
                identity_id.clone(),
                Identity {
                    identity_id: identity_id.clone(),
                    identity_pool_id: identity_pool_id.to_string(),
                    logins: Vec::new(),
                    disabled: false,
                    creation_date: now,
                    last_modified_date: now,
                },
            );
            store
                .identities_by_pool
                .entry(identity_pool_id.to_string())
                .or_default()
                .push(identity_id.clone());
            minted.push(identity_id);
        }
        minted
    }

    /// The records a pool delete orphans are kept so an identity-scoped call can report the
    /// POOL that is missing, and that retention is BOUNDED: the delete releases the per-pool
    /// ceiling the records were counted against, so the queue never holds more than
    /// [`MAX_ORPHANED_IDENTITY_RECORDS`] of them and the oldest go first — however many
    /// pools a caller creates, fills and deletes.
    #[test]
    fn the_records_a_pool_delete_orphans_are_kept_within_one_ceiling() {
        // Rows differ along how the orphans arrive: in one delete at the ceiling, in one
        // past it, and spread over several deletes that pass it together.
        // (case, the identities each successive pool is filled with)
        let cases: [(&str, &[usize]); 4] = [
            ("one pool exactly at the ceiling", &[MAX_ORPHANED_IDENTITY_RECORDS]),
            (
                "one pool one past the ceiling",
                &[MAX_ORPHANED_IDENTITY_RECORDS + 1],
            ),
            (
                "two pools passing the ceiling together",
                &[MAX_ORPHANED_IDENTITY_RECORDS - 1, 2],
            ),
            (
                "many small pools, each created, filled and deleted",
                &[7; 16],
            ),
        ];

        for (case, pool_sizes) in cases {
            let mut store = ComAmazonawsCognitoidentityAwscognitoidentityserviceState::default();
            let mut orphaned_in_order: Vec<String> = Vec::new();
            for (index, count) in pool_sizes.iter().enumerate() {
                let identity_pool_id = format!("pool{index}");
                store
                    .identity_pools
                    .insert(identity_pool_id.clone(), stored_pool(&identity_pool_id));
                orphaned_in_order.extend(mint_identities(&mut store, &identity_pool_id, *count));
                assert!(
                    store.delete_identity_pool(&identity_pool_id).is_some(),
                    "case {case:?}: the pool must be deleted, not reported missing"
                );
            }

            let kept = orphaned_in_order.len().min(MAX_ORPHANED_IDENTITY_RECORDS);
            assert_eq!(
                store.identities.len(),
                kept,
                "case {case:?}: expected {kept} identity records kept, got {}",
                store.identities.len()
            );
            assert_eq!(
                store.orphaned_identities.len(),
                kept,
                "case {case:?}: expected the queue to hold {kept} entries, got {}",
                store.orphaned_identities.len()
            );
            // The oldest orphans are the ones given up, so the records kept are the LAST
            // `kept` to be orphaned — which is what makes a call about a recently deleted
            // pool report that pool.
            for identity_id in &orphaned_in_order[orphaned_in_order.len() - kept..] {
                assert!(
                    store.identities.contains_key(identity_id),
                    "case {case:?}: {identity_id} was orphaned last and must be kept"
                );
            }
            for identity_id in &orphaned_in_order[..orphaned_in_order.len() - kept] {
                assert!(
                    !store.identities.contains_key(identity_id),
                    "case {case:?}: {identity_id} was orphaned first and must be given up"
                );
            }
        }
    }
}
