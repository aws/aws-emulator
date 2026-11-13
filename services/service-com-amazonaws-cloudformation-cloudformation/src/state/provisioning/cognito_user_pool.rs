// SPDX-License-Identifier: CC0-1.0

//! `AWS::Cognito::UserPool`, `AWS::Cognito::UserPoolClient`,
//! `AWS::Cognito::UserPoolIdentityProvider` and `AWS::Cognito::UserPoolResourceServer`: the
//! user pool a stack declares, the app client that signs in against it, the third-party
//! identity provider it federates with and the resource server whose scopes that client is
//! permitted, created in Amazon Cognito user pools itself.
//!
//! This is the per-type half of provisioning for four resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::cognito_identity_pool`] for the reason that module
//! records — that file is far past the length a change to this repository may touch — and is
//! reached through the same seam ([`super::Owner`]).
//!
//! ## Why the pool and the client are really created
//!
//! The handle every operation of the owning service takes for a pool is the `UserPoolId` that
//! service MINTED: `UserPoolIdType` is `@pattern ^[\w-]+_[0-9a-zA-Z]+$` (the `<region>_<id>`
//! form), which no name CloudFormation computes satisfies, and the same holds for an app
//! client, whose `ClientIdType` is `@pattern ^[\w+]+$` and whose id `CreateUserPoolClient`
//! assigns. So the only way a stack can publish a value a client can call `DescribeUserPool`,
//! `DescribeUserPoolClient`, `GetIdentityProviderByIdentifier` or `AdminCreateUser` with is
//! for the resources to exist there and for each create's own answer to be the resource's
//! physical id ([`super::super::template::identity_is_service_assigned`] names both types, so
//! every reference to one is re-pointed at the minted identifier once its create has
//! answered). The pool's NAME and the client's NAME stay CloudFormation's own: that is what
//! each create is made under.
//!
//! An IDENTITY PROVIDER and a RESOURCE SERVER are the other case: a pool holds at most one
//! provider per `ProviderName` and at most one resource server per `Identifier`, which is
//! what each declaration states and what "Ref returns" for it, so the physical id is that
//! value ([`super::super::template::declared_name_property`] names the property it is stated
//! in) and nothing is adopted from the create. A resource server is created for the same
//! reason the pool is: the custom scopes it defines are the only scopes an app client of the
//! pool may be permitted, so a client declaring one cannot be created unless the resource
//! server really exists there.
//!
//! ## The properties
//!
//! Each type's properties ARE its create's own members under the same names, with three
//! exceptions, all on the pool:
//!
//! * `UserPoolName` is the name the pool is created UNDER rather than its identity, and the
//!   owning API spells that member `PoolName`;
//! * `UserPoolTags` is a MAP in the template and a map in the API too, so it travels as
//!   written — merged with the tags CloudFormation propagates to the resource
//!   ([`super::super::provisioner::tags_of`], the one place that rule is written);
//! * the five properties that configure MULTI-FACTOR AUTHENTICATION ([`MFA_PROPERTIES`]) are
//!   members of `SetUserPoolMfaConfig` rather than of the create, so they are applied with
//!   that one call after the pool exists — a declaration carrying one must not be reported as
//!   created with less than it asked for.
//!
//! A declared property none of those applies is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]), which is what keeps a resource from being reported as
//! created with less than its declaration asked for.
//!
//! A pool's `Schema` is the one property an UPDATE reaches through a call of its own: the
//! schema is append-only on a pool that exists, and `AddCustomAttributes` is what adds to it
//! ([`schema_additions`]).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_json_rpc, Created, Refusal, AWS_JSON_1_1};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource types this module provisions live in [`crate::state::template`]'s catalogue,
/// beside the return values each of them publishes.
use crate::state::template::{
    COGNITO_USER_POOL, COGNITO_USER_POOL_CLIENT, COGNITO_USER_POOL_IDENTITY_PROVIDER,
    COGNITO_USER_POOL_RESOURCE_SERVER,
};

/// Amazon Cognito user pools' signing name, which is what routes the in-process call to its
/// own handlers, and the service shape an awsJson `X-Amz-Target` names an operation under
/// (that model's `sigv4Name: cognito-idp` and service shape
/// `AWSCognitoIdentityProviderService`, `smithy_query(query="service_info")`). It speaks
/// `awsJson1_1`.
const COGNITO_IDP_SIGNING_NAME: &str = "cognito-idp";
const COGNITO_IDP_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The error code Amazon Cognito user pools answers for a pool, client or provider it does
/// not hold (`ResourceNotFoundException`, the modelled 404 of its reads, writes and deletes),
/// which is what says a delete has nothing left to remove.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property, or
/// for an answer that identifies nothing.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// Which of the three types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::Cognito::UserPool`.
    UserPool,
    /// `AWS::Cognito::UserPoolClient`.
    Client,
    /// `AWS::Cognito::UserPoolIdentityProvider`.
    IdentityProvider,
    /// `AWS::Cognito::UserPoolResourceServer`.
    ResourceServer,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        COGNITO_USER_POOL => Some(Kind::UserPool),
        COGNITO_USER_POOL_CLIENT => Some(Kind::Client),
        COGNITO_USER_POOL_IDENTITY_PROVIDER => Some(Kind::IdentityProvider),
        COGNITO_USER_POOL_RESOURCE_SERVER => Some(Kind::ResourceServer),
        _ => None,
    }
}

/// The property naming the pool, which is the name the create is made under rather than part
/// of the pool's identity, and the member the owning API takes that name as.
const USER_POOL_NAME: &str = "UserPoolName";
const POOL_NAME_MEMBER: &str = "PoolName";

/// The property carrying the pool's own tag map, spelled identically on both sides.
const USER_POOL_TAGS: &str = "UserPoolTags";

/// The member every call on a pool addresses it by, and the property a client and an identity
/// provider name their pool in.
const USER_POOL_ID: &str = "UserPoolId";

/// The member `CreateUserPoolClient` and `DescribeUserPoolClient` carry a client's id under,
/// and the property name a declaration states a client's name in.
const CLIENT_ID_MEMBER: &str = "ClientId";
const CLIENT_NAME: &str = "ClientName";

/// The property an identity provider's own name is stated in, which is its physical id.
const PROVIDER_NAME: &str = "ProviderName";

/// The property a resource server's own identifier is stated in, which is its physical id
/// and the member every call on one addresses it by beside its pool.
const RESOURCE_SERVER_IDENTIFIER: &str = "Identifier";

/// The properties of `AWS::Cognito::UserPool` that are `CreateUserPool`'s own members under
/// the same name, so the declared value travels into the request as written.
///
/// Source: `CreateUserPool`'s member list
/// (`smithy_query(query="operation",operation="CreateUserPool")`), which is also the type's
/// published property vocabulary beside [`USER_POOL_NAME`], [`USER_POOL_TAGS`] and
/// [`MFA_PROPERTIES`].
const USER_POOL_MEMBERS: &[&str] = &[
    "AccountRecoverySetting",
    "AcrConfiguration",
    "AdminCreateUserConfig",
    "AliasAttributes",
    "AutoVerifiedAttributes",
    "DeletionProtection",
    "DeviceConfiguration",
    "EmailConfiguration",
    "EmailVerificationMessage",
    "EmailVerificationSubject",
    "IssuerConfiguration",
    "KeyConfiguration",
    "LambdaConfig",
    "MfaConfiguration",
    "Policies",
    "Schema",
    "SmsAuthenticationMessage",
    "SmsConfiguration",
    "SmsVerificationMessage",
    "UserAttributeUpdateSettings",
    "UserPoolAddOns",
    "UserPoolTier",
    "UsernameAttributes",
    "UsernameConfiguration",
    "VerificationMessageTemplate",
];

/// Those of [`USER_POOL_MEMBERS`] `UpdateUserPool` also takes, which is what a CHANGED
/// declaration can be applied to the pool that exists with.
///
/// Source: `UpdateUserPool`'s member list. The four it does not carry — the sign-in aliases,
/// the username attributes and their case sensitivity, and the attribute schema — are
/// create-only, which is why [`crate::state::template::replacement_properties`] names them:
/// a declaration restating one asks for a different pool.
const USER_POOL_UPDATE_MEMBERS: &[&str] = &[
    "AccountRecoverySetting",
    "AcrConfiguration",
    "AdminCreateUserConfig",
    "AutoVerifiedAttributes",
    "DeletionProtection",
    "DeviceConfiguration",
    "EmailConfiguration",
    "EmailVerificationMessage",
    "EmailVerificationSubject",
    "IssuerConfiguration",
    "KeyConfiguration",
    "LambdaConfig",
    "MfaConfiguration",
    "Policies",
    "SmsAuthenticationMessage",
    "SmsConfiguration",
    "SmsVerificationMessage",
    "UserAttributeUpdateSettings",
    "UserPoolAddOns",
    "UserPoolTier",
    "VerificationMessageTemplate",
];

/// The properties of `AWS::Cognito::UserPool` that configure MULTI-FACTOR AUTHENTICATION:
/// members of `SetUserPoolMfaConfig` rather than of the pool's own create, so they are
/// applied with that call ([`apply_mfa_configuration`]) once the pool exists.
const MFA_PROPERTIES: &[&str] = &[
    "EmailAuthenticationMessage",
    "EmailAuthenticationSubject",
    "EnabledMfas",
    "WebAuthnRelyingPartyID",
    "WebAuthnUserVerification",
];

/// The properties of `AWS::Cognito::UserPoolClient`, every one of them a member of
/// `CreateUserPoolClient` under the same name — including the pool the client belongs to and
/// the client's own name.
///
/// Source: `CreateUserPoolClient`'s member list
/// (`smithy_query(query="operation",operation="CreateUserPoolClient")`).
const CLIENT_MEMBERS: &[&str] = &[
    "AccessTokenValidity",
    "AllowedOAuthFlows",
    "AllowedOAuthFlowsUserPoolClient",
    "AllowedOAuthScopes",
    "AnalyticsConfiguration",
    "AuthSessionValidity",
    "CallbackURLs",
    "ClientName",
    "DefaultRedirectURI",
    "EnablePropagateAdditionalUserContextData",
    "EnableTokenRevocation",
    "ExplicitAuthFlows",
    "GenerateSecret",
    "IdTokenValidity",
    "LogoutURLs",
    "PreventUserExistenceErrors",
    "ReadAttributes",
    "RefreshTokenRotation",
    "RefreshTokenValidity",
    "SupportedIdentityProviders",
    "TokenValidityUnits",
    "UserPoolId",
    "WriteAttributes",
];

/// Those of [`CLIENT_MEMBERS`] `UpdateUserPoolClient` also takes: every one but the secret
/// generation, which is fixed when the client is made (and is why
/// [`crate::state::template::replacement_properties`] names it).
const CLIENT_UPDATE_MEMBERS: &[&str] = &[
    "AccessTokenValidity",
    "AllowedOAuthFlows",
    "AllowedOAuthFlowsUserPoolClient",
    "AllowedOAuthScopes",
    "AnalyticsConfiguration",
    "AuthSessionValidity",
    "CallbackURLs",
    "ClientName",
    "DefaultRedirectURI",
    "EnablePropagateAdditionalUserContextData",
    "EnableTokenRevocation",
    "ExplicitAuthFlows",
    "IdTokenValidity",
    "LogoutURLs",
    "PreventUserExistenceErrors",
    "ReadAttributes",
    "RefreshTokenRotation",
    "RefreshTokenValidity",
    "SupportedIdentityProviders",
    "TokenValidityUnits",
    "UserPoolId",
    "WriteAttributes",
];

/// The properties of `AWS::Cognito::UserPoolIdentityProvider`, every one of them a member of
/// `CreateIdentityProvider` under the same name.
const IDENTITY_PROVIDER_MEMBERS: &[&str] = &[
    "AcrMapping",
    "AttributeMapping",
    "IdpIdentifiers",
    "ProviderDetails",
    "ProviderName",
    "ProviderType",
    "UserPoolId",
];

/// Those of [`IDENTITY_PROVIDER_MEMBERS`] `UpdateIdentityProvider` also takes: a provider's
/// TYPE and the pool it is in are fixed when it is made.
const IDENTITY_PROVIDER_UPDATE_MEMBERS: &[&str] = &[
    "AcrMapping",
    "AttributeMapping",
    "IdpIdentifiers",
    "ProviderDetails",
    "ProviderName",
    "UserPoolId",
];

/// The properties of `AWS::Cognito::UserPoolResourceServer`, every one of them a member of
/// `CreateResourceServer` — and of `UpdateResourceServer`, which takes the same four — under
/// the same name.
const RESOURCE_SERVER_MEMBERS: &[&str] = &["Identifier", "Name", "Scopes", "UserPoolId"];

/// The members these APIs model as BOOLEANS, which a template may write as a quoted scalar or
/// feed through a `Ref` to a `String` parameter — every value of one arrives as text.
const FLAG_MEMBERS: &[&str] = &[
    "AllowedOAuthFlowsUserPoolClient",
    "EnablePropagateAdditionalUserContextData",
    "EnableTokenRevocation",
    "GenerateSecret",
];

/// The members these APIs model as INTEGERS, in the same position as [`FLAG_MEMBERS`].
const NUMBER_MEMBERS: &[&str] = &[
    "AccessTokenValidity",
    "AuthSessionValidity",
    "IdTokenValidity",
    "RefreshTokenValidity",
];

/// Create the resource one of these types declares, in Amazon Cognito user pools itself — so
/// a client that deployed the stack can pass `!Ref <pool>` straight to `DescribeUserPool` or
/// `AdminCreateUser`, and `!Ref <client>` to `DescribeUserPoolClient`.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::UserPool => create_pool(ctx, region, resource, propagated).await,
        // Neither an app client nor an identity provider is taggable: every tagging call of
        // the owning service addresses a POOL, so there is no call that could carry a tag to
        // one.
        Kind::Client => create_client(ctx, region, resource).await,
        Kind::IdentityProvider => create_identity_provider(ctx, region, resource).await,
        Kind::ResourceServer => create_resource_server(ctx, region, resource).await,
    }
}

/// Create the pool an `AWS::Cognito::UserPool` declares, under the name the declaration asked
/// for — or under the identifier CloudFormation computed for the resource where it names none
/// — and report the id the owning service minted as the resource's identity.
///
/// `Err` is the owning service's own refusal of the create, which is what the resource fails
/// with: a password policy it will not take, a schema past the attribute quota, a pool count
/// past the account's limit.
async fn create_pool(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, Kind::UserPool)?;
    let mut request = declared_members(&resource.properties, USER_POOL_MEMBERS);
    request.insert(
        POOL_NAME_MEMBER.to_string(),
        Value::String(pool_name(resource)),
    );
    if let Some(tags) = pool_tags(&resource.properties, propagated) {
        request.insert(USER_POOL_TAGS.to_string(), tags);
    }
    let answered = call_cognito_idp(ctx, region, "CreateUserPool", &Value::Object(request)).await?;
    let pool = answered.get("UserPool").unwrap_or(&Value::Null);
    // The identity in force is the one the create ANSWERED with: the `<region>_<id>` the
    // owning service minted is no value CloudFormation could have computed.
    let user_pool_id = answered_text(pool, "Id").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: "Amazon Cognito reported no UserPool.Id for the pool it created, which is \
                  what a reference to this resource resolves to"
            .to_string(),
    })?;
    // The configuration the create cannot carry: applied to the pool that now exists, so a
    // declaration asking for it is not reported as created without it.
    apply_mfa_configuration(ctx, region, &resource.properties, &user_pool_id).await?;
    let mut attributes: BTreeMap<String, String> =
        crate::state::template::user_pool_attributes(region, &user_pool_id)
            .into_iter()
            .collect();
    // The ARN the owning service really holds the pool under, where it reported one.
    if let Some(arn) = answered_text(pool, "Arn") {
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created {
        physical_resource_id: Some(user_pool_id),
        attributes,
    })
}

/// Create the app client an `AWS::Cognito::UserPoolClient` declares, in the pool its
/// declaration names, and report the `ClientId` the owning service minted as the resource's
/// identity.
async fn create_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, Kind::Client)?;
    let mut request = declared_members(&resource.properties, CLIENT_MEMBERS);
    request.insert(
        USER_POOL_ID.to_string(),
        Value::String(declared_pool(resource)?),
    );
    // The client is created under the name the declaration asked for, or under the identifier
    // CloudFormation computed for the resource where it names none: `ClientName` is required
    // by the owning API and optional on the type.
    request
        .entry(CLIENT_NAME.to_string())
        .or_insert_with(|| Value::String(resource.physical_resource_id.clone()));
    let answered =
        call_cognito_idp(ctx, region, "CreateUserPoolClient", &Value::Object(request)).await?;
    client_created(answered.get("UserPoolClient").unwrap_or(&Value::Null))
}

/// What a client's create or update answered, as the resource's identity and attributes: the
/// `ClientId` that service minted, the client's name, and the secret it generated where the
/// declaration asked for one ("Fn::GetAtt … ClientSecret").
fn client_created(client: &Value) -> Result<Created, Refusal> {
    let client_id = answered_text(client, CLIENT_ID_MEMBER).ok_or_else(|| Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: "Amazon Cognito reported no UserPoolClient.ClientId for the app client it \
                  created, which is what a reference to this resource resolves to"
            .to_string(),
    })?;
    let mut attributes = BTreeMap::new();
    if let Some(name) = answered_text(client, CLIENT_NAME) {
        attributes.insert("Name".to_string(), name);
    }
    if let Some(secret) = answered_text(client, "ClientSecret") {
        attributes.insert("ClientSecret".to_string(), secret);
    }
    Ok(Created {
        physical_resource_id: Some(client_id),
        attributes,
    })
}

/// Create the third-party identity provider an `AWS::Cognito::UserPoolIdentityProvider`
/// declares, in the pool its declaration names.
///
/// The provider's identity is its `ProviderName`, which the resource already carries as its
/// physical id, so nothing is adopted from the answer and the type publishes no attribute of
/// its own.
async fn create_identity_provider(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, Kind::IdentityProvider)?;
    let mut request = declared_members(&resource.properties, IDENTITY_PROVIDER_MEMBERS);
    request.insert(
        USER_POOL_ID.to_string(),
        Value::String(declared_pool(resource)?),
    );
    request.insert(
        PROVIDER_NAME.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_cognito_idp(
        ctx,
        region,
        "CreateIdentityProvider",
        &Value::Object(request),
    )
    .await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Create the resource server an `AWS::Cognito::UserPoolResourceServer` declares, in the pool
/// its declaration names — so an app client of that pool may be permitted the custom scopes
/// the resource server defines, which is the one thing that makes them exist
/// (`CreateUserPoolClient` refuses a scope no resource server of the pool defines).
///
/// Its identity is the `Identifier` it is created under, which the resource already carries as
/// its physical id, so nothing is adopted from the answer.
async fn create_resource_server(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, resource, Kind::ResourceServer)?;
    let request = resource_server_request(resource, &resource.properties)?;
    call_cognito_idp(ctx, region, "CreateResourceServer", &request).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// The `CreateResourceServer`/`UpdateResourceServer` request one declaration states: the four
/// members both calls take, with the pool the declaration names and the identifier the
/// resource carries.
///
/// `Name` is required by both calls and by the type; a declaration stating none is created
/// under the identifier CloudFormation computed for the resource, as every other name here is.
fn resource_server_request(
    resource: &StoredResource,
    declared: &Value,
) -> Result<Value, Refusal> {
    let mut request = declared_members(declared, RESOURCE_SERVER_MEMBERS);
    request.insert(
        USER_POOL_ID.to_string(),
        Value::String(declared_pool(resource)?),
    );
    request.insert(
        RESOURCE_SERVER_IDENTIFIER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request
        .entry("Name".to_string())
        .or_insert_with(|| Value::String(resource.physical_resource_id.clone()));
    Ok(Value::Object(request))
}

/// Whether the owning service reports the resource ready.
///
/// Nothing is asked, and nothing waits: a pool EXISTS from the moment `CreateUserPool`
/// answers — `DescribeUserPool` resolves it and `AdminCreateUser` puts users in it — and so do
/// an app client, an identity provider and a resource server. None of those creates reports a
/// non-terminal state for the resource to settle out of.
pub(super) fn ready() -> super::Readiness {
    super::Readiness::Ready
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own update call: `UpdateUserPool`, `UpdateUserPoolClient`,
/// `UpdateIdentityProvider` or `UpdateResourceServer`, each addressed by the identity the
/// resource carries, so the resource keeps it and nothing is replaced. A pool whose schema
/// GREW takes `AddCustomAttributes` beside its update, the one call that adds an attribute to
/// a pool that exists ([`schema_additions`]).
///
/// `Ok(None)` is a change none of those calls can make — a property that is fixed when the
/// resource is created, or a schema change that is not an addition — so the deployment reports
/// it as not applied rather than as a success that dropped the caller's change. Such a
/// declaration is normally REPLACED before it reaches here
/// ([`crate::state::template::replacement_properties`] names those properties); this is the
/// same rule read from the other side.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<Option<Created>, Refusal> {
    // The DECLARED document, not the record's: by the time this is reached the resource
    // carries the properties it was CREATED with, so a property the new declaration
    // introduces is only in `declared` — and an unapplied one has to be refused there too, or
    // the deployment would report a change it never made.
    refuse_unapplied_properties(declared, resource, kind)?;
    if create_only_change(kind, &resource.properties, declared) {
        return Ok(None);
    }
    match kind {
        Kind::UserPool => {
            // What the declaration asks of the pool's SCHEMA, decided before anything is
            // sent: a change that is no addition cannot be applied at all.
            let Some(additions) = schema_additions(&resource.properties, declared) else {
                return Ok(None);
            };
            let mut request = declared_members(declared, USER_POOL_UPDATE_MEMBERS);
            request.insert(
                USER_POOL_ID.to_string(),
                Value::String(resource.physical_resource_id.clone()),
            );
            // The pool's name is sent whether or not the declaration changed it: the call
            // REPLACES the pool's configuration, so a member left out would unset what the
            // declaration still asks for.
            request.insert(
                POOL_NAME_MEMBER.to_string(),
                Value::String(declared_text(declared, USER_POOL_NAME).unwrap_or_else(|| {
                    declared_text(&resource.properties, USER_POOL_NAME)
                        .unwrap_or_else(|| resource.physical_resource_id.clone())
                })),
            );
            if let Some(tags) = pool_tags(declared, propagated) {
                request.insert(USER_POOL_TAGS.to_string(), tags);
            }
            call_cognito_idp(ctx, region, "UpdateUserPool", &Value::Object(request)).await?;
            apply_mfa_configuration(ctx, region, declared, &resource.physical_resource_id)
                .await?;
            // The attributes the declaration ADDS to the pool's schema, applied with the one
            // call that adds one to a pool that exists.
            if !additions.is_empty() {
                call_cognito_idp(
                    ctx,
                    region,
                    "AddCustomAttributes",
                    &serde_json::json!({
                        USER_POOL_ID: resource.physical_resource_id.clone(),
                        "CustomAttributes": Value::Array(additions),
                    }),
                )
                .await?;
            }
            // Every attribute the type publishes is read off the pool's id, which this call
            // leaves exactly as it was.
            Ok(Some(Created::attributes(BTreeMap::new())))
        }
        Kind::Client => {
            let mut request = declared_members(declared, CLIENT_UPDATE_MEMBERS);
            request.insert(
                USER_POOL_ID.to_string(),
                Value::String(declared_pool(resource)?),
            );
            request.insert(
                CLIENT_ID_MEMBER.to_string(),
                Value::String(resource.physical_resource_id.clone()),
            );
            let answered =
                call_cognito_idp(ctx, region, "UpdateUserPoolClient", &Value::Object(request))
                    .await?;
            // The client keeps the id the create minted, so only its attributes are re-read.
            let mut created =
                client_created(answered.get("UserPoolClient").unwrap_or(&Value::Null))?;
            created.physical_resource_id = None;
            Ok(Some(created))
        }
        Kind::IdentityProvider => {
            let mut request = declared_members(declared, IDENTITY_PROVIDER_UPDATE_MEMBERS);
            request.insert(
                USER_POOL_ID.to_string(),
                Value::String(declared_pool(resource)?),
            );
            request.insert(
                PROVIDER_NAME.to_string(),
                Value::String(resource.physical_resource_id.clone()),
            );
            call_cognito_idp(
                ctx,
                region,
                "UpdateIdentityProvider",
                &Value::Object(request),
            )
            .await?;
            Ok(Some(Created::attributes(BTreeMap::new())))
        }
        Kind::ResourceServer => {
            let request = resource_server_request(resource, declared)?;
            call_cognito_idp(ctx, region, "UpdateResourceServer", &request).await?;
            Ok(Some(Created::attributes(BTreeMap::new())))
        }
    }
}

/// Delete the resource a stack created, addressed by the identity the stack records for it: a
/// pool and a client by the id the owning service minted, a provider by the pair that
/// identifies it (its pool and its `ProviderName`).
///
/// A resource that service no longer holds — and a pool it no longer holds, which can hold
/// neither a client nor a provider either — is already in the state the delete asks for; both
/// are its `ResourceNotFoundException`, judged on the CODE and never on a message. A pool
/// whose `DeletionProtection` is active is NOT: that refusal is the owning service's own, and
/// the resource fails with it.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (operation, request) = match kind {
        Kind::UserPool => (
            "DeleteUserPool",
            serde_json::json!({USER_POOL_ID: resource.physical_resource_id.clone()}),
        ),
        Kind::Client => (
            "DeleteUserPoolClient",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                CLIENT_ID_MEMBER: resource.physical_resource_id.clone(),
            }),
        ),
        Kind::IdentityProvider => (
            "DeleteIdentityProvider",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                PROVIDER_NAME: resource.physical_resource_id.clone(),
            }),
        ),
        Kind::ResourceServer => (
            "DeleteResourceServer",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                RESOURCE_SERVER_IDENTIFIER: resource.physical_resource_id.clone(),
            }),
        ),
    };
    match call_cognito_idp(ctx, region, operation, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's configuration as the owning service reports it, for drift detection.
///
/// Each read answers the resource's own structure, whose members carry the very names the
/// type declares its properties with — except the pool's name and its attribute schema, which
/// `UserPoolType` reports as `Name` and `SchemaAttributes` ([`REPORTED_POOL_MEMBERS`]) — so
/// the answer maps onto the property vocabulary one for one. A resource the owning service no
/// longer holds was deleted ([`NOT_FOUND`]).
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (operation, request, reported, held) = match kind {
        Kind::UserPool => (
            "DescribeUserPool",
            serde_json::json!({USER_POOL_ID: resource.physical_resource_id.clone()}),
            REPORTED_POOL_MEMBERS,
            "UserPool",
        ),
        Kind::Client => (
            "DescribeUserPoolClient",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                CLIENT_ID_MEMBER: resource.physical_resource_id.clone(),
            }),
            REPORTED_CLIENT_MEMBERS,
            "UserPoolClient",
        ),
        Kind::IdentityProvider => (
            "DescribeIdentityProvider",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                PROVIDER_NAME: resource.physical_resource_id.clone(),
            }),
            REPORTED_PROVIDER_MEMBERS,
            "IdentityProvider",
        ),
        Kind::ResourceServer => (
            "DescribeResourceServer",
            serde_json::json!({
                USER_POOL_ID: declared_pool(resource)?,
                RESOURCE_SERVER_IDENTIFIER: resource.physical_resource_id.clone(),
            }),
            REPORTED_RESOURCE_SERVER_MEMBERS,
            "ResourceServer",
        ),
    };
    match call_cognito_idp(ctx, region, operation, &request).await {
        Ok(answer) => {
            let Some(detail) = answer.get(held) else {
                return Ok(None);
            };
            let mut configuration = Map::new();
            for (member, property) in reported {
                if let Some(value) = detail.get(*member) {
                    configuration.insert((*property).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(configuration)))
        }
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The members `UserPoolType` reports a pool's declared configuration under, paired with the
/// property each one is the value of. Every pair but the two that are spelled differently on
/// the two sides is the same name twice.
const REPORTED_POOL_MEMBERS: &[(&str, &str)] = &[
    ("AccountRecoverySetting", "AccountRecoverySetting"),
    ("AdminCreateUserConfig", "AdminCreateUserConfig"),
    ("AliasAttributes", "AliasAttributes"),
    ("AutoVerifiedAttributes", "AutoVerifiedAttributes"),
    ("DeletionProtection", "DeletionProtection"),
    ("DeviceConfiguration", "DeviceConfiguration"),
    ("EmailConfiguration", "EmailConfiguration"),
    ("LambdaConfig", "LambdaConfig"),
    ("MfaConfiguration", "MfaConfiguration"),
    ("Name", USER_POOL_NAME),
    ("Policies", "Policies"),
    ("SchemaAttributes", "Schema"),
    ("SmsConfiguration", "SmsConfiguration"),
    ("UserAttributeUpdateSettings", "UserAttributeUpdateSettings"),
    ("UserPoolAddOns", "UserPoolAddOns"),
    ("UserPoolTags", USER_POOL_TAGS),
    ("UserPoolTier", "UserPoolTier"),
    ("UsernameAttributes", "UsernameAttributes"),
    ("UsernameConfiguration", "UsernameConfiguration"),
    ("VerificationMessageTemplate", "VerificationMessageTemplate"),
];

/// The members `UserPoolClientType` reports an app client's declared configuration under,
/// each spelled identically to the property it is the value of.
const REPORTED_CLIENT_MEMBERS: &[(&str, &str)] = &[
    ("AccessTokenValidity", "AccessTokenValidity"),
    ("AllowedOAuthFlows", "AllowedOAuthFlows"),
    (
        "AllowedOAuthFlowsUserPoolClient",
        "AllowedOAuthFlowsUserPoolClient",
    ),
    ("AllowedOAuthScopes", "AllowedOAuthScopes"),
    ("AnalyticsConfiguration", "AnalyticsConfiguration"),
    ("AuthSessionValidity", "AuthSessionValidity"),
    ("CallbackURLs", "CallbackURLs"),
    ("ClientName", "ClientName"),
    ("DefaultRedirectURI", "DefaultRedirectURI"),
    (
        "EnablePropagateAdditionalUserContextData",
        "EnablePropagateAdditionalUserContextData",
    ),
    ("EnableTokenRevocation", "EnableTokenRevocation"),
    ("ExplicitAuthFlows", "ExplicitAuthFlows"),
    ("IdTokenValidity", "IdTokenValidity"),
    ("LogoutURLs", "LogoutURLs"),
    ("PreventUserExistenceErrors", "PreventUserExistenceErrors"),
    ("ReadAttributes", "ReadAttributes"),
    ("RefreshTokenRotation", "RefreshTokenRotation"),
    ("RefreshTokenValidity", "RefreshTokenValidity"),
    ("SupportedIdentityProviders", "SupportedIdentityProviders"),
    ("TokenValidityUnits", "TokenValidityUnits"),
    ("UserPoolId", "UserPoolId"),
    ("WriteAttributes", "WriteAttributes"),
];

/// The members `IdentityProviderType` reports a provider's declared configuration under, each
/// spelled identically to the property it is the value of.
const REPORTED_PROVIDER_MEMBERS: &[(&str, &str)] = &[
    ("AttributeMapping", "AttributeMapping"),
    ("IdpIdentifiers", "IdpIdentifiers"),
    ("ProviderDetails", "ProviderDetails"),
    ("ProviderName", "ProviderName"),
    ("ProviderType", "ProviderType"),
    ("UserPoolId", "UserPoolId"),
];

/// The members `ResourceServerType` reports a resource server's declared configuration under,
/// each spelled identically to the property it is the value of.
const REPORTED_RESOURCE_SERVER_MEMBERS: &[(&str, &str)] = &[
    ("Identifier", "Identifier"),
    ("Name", "Name"),
    ("Scopes", "Scopes"),
    ("UserPoolId", "UserPoolId"),
];

/// Apply the MFA configuration a pool's declaration states, with the one call that states it.
///
/// `SetUserPoolMfaConfig` REPLACES the whole configuration in force, so the call is made only
/// where the declaration carries one of [`MFA_PROPERTIES`] — a pool whose declaration states
/// none is left with the configuration its create produced rather than having it cleared —
/// and it carries the pool's `MfaConfiguration` beside the factors, since leaving that member
/// out of a replacing call would unset what the declaration asks for.
///
/// `EnabledMfas` is "the MFA methods you want users to be able to use": each entry names the
/// factor whose own configuration the call activates, which is how that list reaches an API
/// that takes one member per factor.
async fn apply_mfa_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    user_pool_id: &str,
) -> Result<(), Refusal> {
    if !MFA_PROPERTIES
        .iter()
        .any(|property| properties.get(*property).is_some_and(|value| !value.is_null()))
    {
        return Ok(());
    }
    let mut request = Map::new();
    request.insert(
        USER_POOL_ID.to_string(),
        Value::String(user_pool_id.to_string()),
    );
    if let Some(configuration) = properties.get("MfaConfiguration").cloned() {
        request.insert("MfaConfiguration".to_string(), configuration);
    }
    let enabled: Vec<String> = properties
        .get("EnabledMfas")
        .and_then(Value::as_array)
        .map(|factors| {
            factors
                .iter()
                .filter_map(|factor| factor.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    // `SMS_MFA`, `SOFTWARE_TOKEN_MFA` and `EMAIL_OTP` are the factors `EnabledMfas` names
    // (the `EnabledMfas` property's own published values), one per member of the call.
    if enabled.iter().any(|factor| factor == "SMS_MFA") {
        let mut sms = Map::new();
        for member in ["SmsAuthenticationMessage", "SmsConfiguration"] {
            if let Some(value) = properties.get(member) {
                sms.insert(member.to_string(), value.clone());
            }
        }
        request.insert("SmsMfaConfiguration".to_string(), Value::Object(sms));
    }
    if enabled.iter().any(|factor| factor == "SOFTWARE_TOKEN_MFA") {
        request.insert(
            "SoftwareTokenMfaConfiguration".to_string(),
            serde_json::json!({"Enabled": true}),
        );
    }
    if enabled.iter().any(|factor| factor == "EMAIL_OTP") {
        let mut email = Map::new();
        for (property, member) in [
            ("EmailAuthenticationMessage", "Message"),
            ("EmailAuthenticationSubject", "Subject"),
        ] {
            if let Some(value) = properties.get(property) {
                email.insert(member.to_string(), value.clone());
            }
        }
        request.insert("EmailMfaConfiguration".to_string(), Value::Object(email));
    }
    let mut passkeys = Map::new();
    for (property, member) in [
        ("WebAuthnRelyingPartyID", "RelyingPartyId"),
        ("WebAuthnUserVerification", "UserVerification"),
    ] {
        if let Some(value) = properties.get(property) {
            passkeys.insert(member.to_string(), value.clone());
        }
    }
    if !passkeys.is_empty() {
        request.insert(
            "WebAuthnConfiguration".to_string(),
            Value::Object(passkeys),
        );
    }
    call_cognito_idp(
        ctx,
        region,
        "SetUserPoolMfaConfig",
        &Value::Object(request),
    )
    .await
    .map(|_| ())
}

/// Whether a declaration states a property of this kind that is fixed when the resource is
/// created DIFFERENTLY from the resource that exists ([`create_only_properties`]).
fn create_only_change(kind: Kind, existing: &Value, declared: &Value) -> bool {
    create_only_properties(kind)
        .iter()
        .any(|property| existing.get(*property) != declared.get(*property))
}

/// The properties of each type that are members of its CREATE and of no member of its update,
/// so the resource that exists cannot take a new value for one.
///
/// A pool's sign-in aliases and its username attributes and their case sensitivity; a client's
/// secret generation; a provider's type; and — for all three of a pool's children — the POOL
/// each of them belongs to, which every call on one addresses it inside. A declaration
/// restating one of these asks for a resource no call this module makes could produce, so the
/// deployment reports it as not applied.
///
/// [`crate::state::template::replacement_properties`] is the other half of that reading: it
/// names the ones a deployment REPLACES the resource over, which is every one of these but
/// the pool — a declaration re-pointed at another pool is resolved against the identities in
/// force only once the pool's own identity is decided, so it is judged here, where both
/// documents name the pool by the id in force.
///
/// (A provider's `ProviderName`, a resource server's `Identifier` and a pool's own name are
/// the resources' identities, which that same reading covers. A pool's `Schema` is NOT one of
/// them: an attribute ADDED to it reaches the pool that exists through `AddCustomAttributes`,
/// which is [`schema_additions`].)
fn create_only_properties(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::UserPool => &["AliasAttributes", "UsernameAttributes", "UsernameConfiguration"],
        Kind::Client => &["GenerateSecret", "UserPoolId"],
        Kind::IdentityProvider => &["ProviderType", "UserPoolId"],
        Kind::ResourceServer => &["UserPoolId"],
    }
}

/// What a new declaration asks of a pool's `Schema`, as the attributes `AddCustomAttributes`
/// is to add: the entries the declaration ADDS, in declaration order.
///
/// The rule: a pool's attribute schema is APPEND-ONLY on the pool that exists. Amazon Cognito
/// publishes one call that changes it — `AddCustomAttributes`, which "adds additional user
/// attributes to the user pool schema" — and `UpdateUserPool` takes no schema at all, so a
/// declaration that adds custom attributes is applied to the pool it already has, while one
/// that DROPS an entry, CHANGES one it already declared, or adds a STANDARD attribute
/// ([`STANDARD_ATTRIBUTES`], none of which that call can add) asks for a schema no call can
/// produce: `None`, which the deployment reports as not applied.
fn schema_additions(existing: &Value, declared: &Value) -> Option<Vec<Value>> {
    let held = schema_entries(existing);
    let wanted = schema_entries(declared);
    // An entry the pool was created with that the new declaration no longer states as it
    // was: no call takes an attribute away or rewrites one.
    for (name, entry) in &held {
        if wanted.get(name) != Some(entry) {
            return None;
        }
    }
    // In DECLARATION order, which is the order the attributes are added in and therefore the
    // order the grown schema reads in.
    let mut additions = Vec::new();
    for entry in schema_list(declared) {
        let Some(name) = entry.get("Name").and_then(Value::as_str) else {
            continue;
        };
        if held.contains_key(name) {
            continue;
        }
        if STANDARD_ATTRIBUTES.contains(&name) {
            return None;
        }
        additions.push(entry.clone());
    }
    Some(additions)
}

/// One `Schema` declaration's entries, in the order the declaration writes them.
fn schema_list(properties: &Value) -> &[Value] {
    properties
        .get("Schema")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// One `Schema` declaration's entries, keyed by the attribute `Name` each states. An entry
/// that names no attribute is no attribute at all and is left out, so the owning service's own
/// validation is what judges it.
fn schema_entries(properties: &Value) -> BTreeMap<String, &Value> {
    schema_list(properties)
        .iter()
        .filter_map(|entry| {
            let name = entry.get("Name").and_then(Value::as_str)?;
            Some((name.to_string(), entry))
        })
        .collect()
}

/// The attribute names a user pool's schema carries of its own accord, which
/// `AddCustomAttributes` cannot add: it adds CUSTOM attributes, and Amazon Cognito stores each
/// one under a `custom:` prefix of the name the request gives.
///
/// Source: the standard attributes Amazon Cognito user pools publish (the OpenID Connect
/// standard claims, plus the `sub` it mints and the three it maintains itself —
/// `email_verified`, `phone_number_verified` and `updated_at`).
const STANDARD_ATTRIBUTES: &[&str] = &[
    "address",
    "birthdate",
    "email",
    "email_verified",
    "family_name",
    "gender",
    "given_name",
    "locale",
    "middle_name",
    "name",
    "nickname",
    "phone_number",
    "phone_number_verified",
    "picture",
    "preferred_username",
    "profile",
    "sub",
    "updated_at",
    "website",
    "zoneinfo",
];

/// The pool a client's, a provider's or a resource server's declaration names — the other
/// half of the resource's identity.
///
/// Read off the RECORD, which states the pool the resource was created in: an update is
/// addressed to the resource that exists, and a declaration naming another pool is a change
/// [`create_only_change`] has already reported as not applied.
///
/// `UserPoolId` is a required property of all three types, so a declaration carrying none
/// describes no resource that could be created: it is refused here, naming that property,
/// rather than sent as a request the owning service would reject.
fn declared_pool(resource: &StoredResource) -> Result<String, Refusal> {
    declared_text(&resource.properties, USER_POOL_ID).ok_or_else(|| Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: format!("{} declares no {USER_POOL_ID}", resource.resource_type),
    })
}

/// The name the pool is created under: the one the declaration states, or the identifier
/// CloudFormation computed for the resource where it states none.
fn pool_name(resource: &StoredResource) -> String {
    declared_text(&resource.properties, USER_POOL_NAME)
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// The text a declaration states a property as, where it states a non-empty one at all.
fn declared_text(properties: &Value, property: &str) -> Option<String> {
    properties
        .get(property)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

/// The text the owning service reported a member of its answer as, where it reported a
/// non-empty one.
fn answered_text(detail: &Value, member: &str) -> Option<String> {
    detail
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

/// The members of `members` a declaration states, as the request carries them: the type and
/// the API spell each one identically, so every value goes through as the template wrote it —
/// except a member the API models as a BOOLEAN or an INTEGER, which a template may write as a
/// quoted scalar ([`FLAG_MEMBERS`], [`NUMBER_MEMBERS`]). A value that is neither is left
/// exactly as written, so the owning service's own validation judges the value the template
/// really declared rather than a silently changed one.
fn declared_members(properties: &Value, members: &[&str]) -> Map<String, Value> {
    let mut request = Map::new();
    for member in members {
        let Some(value) = properties.get(*member).filter(|value| !value.is_null()) else {
            continue;
        };
        let carried = if FLAG_MEMBERS.contains(member) {
            declared_flag(value).map_or_else(|| value.clone(), Value::Bool)
        } else if NUMBER_MEMBERS.contains(member) {
            declared_number(value).map_or_else(|| value.clone(), Value::from)
        } else {
            value.clone()
        };
        request.insert((*member).to_string(), carried);
    }
    request
}

/// A declared boolean in either spelling a template gives one: JSON's boolean, or the text a
/// template spells one as, read case-insensitively — the same reading
/// [`crate::state::template`] makes of a template boolean. `None` is a value that is no
/// boolean at all.
fn declared_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// A declared whole number in either spelling a template gives one: JSON's number, or the
/// text a quoted scalar or a `String` parameter arrives as. `None` is a value that is no whole
/// number at all.
fn declared_number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// The tag map the pool is created or updated with: the map its own `UserPoolTags` states,
/// plus the tags CloudFormation propagates to the resource under every key that map does not
/// use, or `None` where there is no tag at all to send.
///
/// The merge itself is [`tags_of`], the one place that rule is written; it reads both
/// spellings a type's tags are declared in, so this pool's MAP is handed to it as the `Tags`
/// property and comes back as the pairs the API's own map takes.
fn pool_tags(properties: &Value, propagated: &[(String, String)]) -> Option<Value> {
    let declared = properties.get(USER_POOL_TAGS).cloned();
    let tags = tags_of(&serde_json::json!({"Tags": declared}), propagated);
    if tags.is_empty() {
        return None;
    }
    Some(Value::Object(
        tags.into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    ))
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a resource from being reported as created — or as updated —
/// with less than its declaration asked for. `properties` is the document the deployment
/// DECLARES, which on an update is not the one the record carries.
fn refuse_unapplied_properties(
    properties: &Value,
    resource: &StoredResource,
    kind: Kind,
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if applied_properties(kind).contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

/// Every property of one of these types this module APPLIES: its create's own members, and —
/// for a pool — the name it is created under, its tag map and the MFA configuration
/// [`apply_mfa_configuration`] sets.
fn applied_properties(kind: Kind) -> Vec<&'static str> {
    match kind {
        Kind::UserPool => USER_POOL_MEMBERS
            .iter()
            .chain(MFA_PROPERTIES)
            .copied()
            .chain([USER_POOL_NAME, USER_POOL_TAGS])
            .collect(),
        Kind::Client => CLIENT_MEMBERS.to_vec(),
        Kind::IdentityProvider => IDENTITY_PROVIDER_MEMBERS.to_vec(),
        Kind::ResourceServer => RESOURCE_SERVER_MEMBERS.to_vec(),
    }
}

/// Call one Amazon Cognito user-pools operation on the awsJson1_1 wire its model declares.
async fn call_cognito_idp(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        COGNITO_IDP_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COGNITO_IDP_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of one of these types, as the naming rules leave one.
    fn declared(resource_type: &str, properties: Value, physical: &str) -> StoredResource {
        StoredResource {
            logical_id: "Pool".to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: physical.to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// RULE: every member each arm sends is a property the type publishes, and every property
    /// it publishes is one this module applies — a property in neither position would be
    /// dropped silently.
    #[test]
    fn every_sent_member_is_an_applied_property() {
        for (kind, members) in [
            (Kind::UserPool, USER_POOL_MEMBERS),
            (Kind::Client, CLIENT_MEMBERS),
            (Kind::IdentityProvider, IDENTITY_PROVIDER_MEMBERS),
            (Kind::ResourceServer, RESOURCE_SERVER_MEMBERS),
        ] {
            let applied = applied_properties(kind);
            for member in members {
                assert!(
                    applied.contains(member),
                    "case {kind:?}: expected {member} to be an applied property, got {applied:?}"
                );
            }
        }
        for member in USER_POOL_UPDATE_MEMBERS {
            assert!(
                USER_POOL_MEMBERS.contains(member),
                "case {member:?}: expected an update member to be a create member too"
            );
        }
        for member in CLIENT_UPDATE_MEMBERS {
            assert!(
                CLIENT_MEMBERS.contains(member),
                "case {member:?}: expected an update member to be a create member too"
            );
        }
        for member in IDENTITY_PROVIDER_UPDATE_MEMBERS {
            assert!(
                IDENTITY_PROVIDER_MEMBERS.contains(member),
                "case {member:?}: expected an update member to be a create member too"
            );
        }
    }

    /// RULE: a property the create takes and the update does not is create-only, and is the
    /// one thing the update arm reports as not applied.
    #[test]
    fn a_property_no_update_call_takes_is_create_only() {
        for (kind, members, update_members) in [
            (Kind::UserPool, USER_POOL_MEMBERS, USER_POOL_UPDATE_MEMBERS),
            (Kind::Client, CLIENT_MEMBERS, CLIENT_UPDATE_MEMBERS),
            (
                Kind::IdentityProvider,
                IDENTITY_PROVIDER_MEMBERS,
                IDENTITY_PROVIDER_UPDATE_MEMBERS,
            ),
            (
                Kind::ResourceServer,
                RESOURCE_SERVER_MEMBERS,
                RESOURCE_SERVER_MEMBERS,
            ),
        ] {
            let create_only = create_only_properties(kind);
            for member in members {
                if update_members.contains(member) {
                    continue;
                }
                // A pool's `Schema` is the one create member no update call takes that is
                // still applied to the resource that exists, through the call that adds to it
                // ([`schema_additions`]).
                if kind == Kind::UserPool && *member == "Schema" {
                    continue;
                }
                assert!(
                    create_only.contains(member),
                    "case {kind:?}: expected {member} to be create-only, got {create_only:?}"
                );
            }
        }
    }

    /// RULE: a schema change that only ADDS attributes is applied to the pool that exists,
    /// and any other schema change is not applied at all.
    #[test]
    fn only_an_added_schema_attribute_reaches_a_pool_that_exists() {
        let held = serde_json::json!({
            "Schema": [
                {"Name": "tenant", "AttributeDataType": "String", "Mutable": true},
            ]
        });
        let rows: &[(&str, Value, Option<&[&str]>)] = &[
            (
                "the same schema",
                serde_json::json!({"Schema": [
                    {"Name": "tenant", "AttributeDataType": "String", "Mutable": true},
                ]}),
                Some(&[]),
            ),
            (
                "one custom attribute added",
                serde_json::json!({"Schema": [
                    {"Name": "tenant", "AttributeDataType": "String", "Mutable": true},
                    {"Name": "tier", "AttributeDataType": "Number"},
                ]}),
                Some(&["tier"]),
            ),
            (
                "two custom attributes added",
                serde_json::json!({"Schema": [
                    {"Name": "tenant", "AttributeDataType": "String", "Mutable": true},
                    {"Name": "tier", "AttributeDataType": "Number"},
                    {"Name": "region", "AttributeDataType": "String"},
                ]}),
                Some(&["tier", "region"]),
            ),
            (
                "an attribute dropped",
                serde_json::json!({"Schema": []}),
                None,
            ),
            (
                "no schema declared at all",
                serde_json::json!({}),
                None,
            ),
            (
                "an attribute changed",
                serde_json::json!({"Schema": [
                    {"Name": "tenant", "AttributeDataType": "Number", "Mutable": true},
                ]}),
                None,
            ),
            (
                "a standard attribute added",
                serde_json::json!({"Schema": [
                    {"Name": "tenant", "AttributeDataType": "String", "Mutable": true},
                    {"Name": "email", "Required": true},
                ]}),
                None,
            ),
        ];
        for (case, declared, expected) in rows {
            let additions = schema_additions(&held, declared);
            match expected {
                Some(names) => {
                    let added: Vec<String> = additions
                        .clone()
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: expected the change to be applied, got \
                                 {additions:?}"
                            )
                        })
                        .iter()
                        .filter_map(|entry| {
                            entry.get("Name").and_then(Value::as_str).map(ToString::to_string)
                        })
                        .collect();
                    assert_eq!(
                        added, *names,
                        "case {case:?}: expected the attributes {names:?} to be added, got \
                         {added:?}"
                    );
                }
                None => assert!(
                    additions.is_none(),
                    "case {case:?}: expected a change no call can make, got {additions:?}"
                ),
            }
        }
    }

    /// RULE: the pool is created under the name the declaration states, and under the
    /// identifier CloudFormation computed for it where it states none.
    #[test]
    fn the_pool_is_created_under_the_declared_name() {
        let rows: &[(&str, Value, &str)] = &[
            (
                "a declared name",
                serde_json::json!({"UserPoolName": "shop-directory"}),
                "shop-directory",
            ),
            (
                "no name at all",
                serde_json::json!({}),
                "stack-Pool-ABCDEF123456",
            ),
            (
                "an empty name",
                serde_json::json!({"UserPoolName": ""}),
                "stack-Pool-ABCDEF123456",
            ),
        ];
        for (case, properties, expected) in rows {
            let resource = declared(
                COGNITO_USER_POOL,
                properties.clone(),
                "stack-Pool-ABCDEF123456",
            );
            let name = pool_name(&resource);
            assert_eq!(
                &name, expected,
                "case {case:?}: expected the pool to be created as {expected:?}, got {name:?}"
            );
        }
    }

    /// RULE: a member the API models as a boolean or an integer travels as that JSON type
    /// whichever spelling the template wrote it in, and a value that is neither is sent as
    /// written.
    #[test]
    fn a_flag_or_a_number_travels_as_the_api_models_it() {
        let properties = serde_json::json!({
            "GenerateSecret": "true",
            "EnableTokenRevocation": false,
            "RefreshTokenValidity": "30",
            "AccessTokenValidity": 60,
            "AuthSessionValidity": "not a number",
            "ClientName": "shop-client",
        });
        let request = declared_members(&properties, CLIENT_MEMBERS);
        let rows: &[(&str, &str, Value)] = &[
            ("a quoted true", "GenerateSecret", Value::Bool(true)),
            ("a JSON false", "EnableTokenRevocation", Value::Bool(false)),
            ("a quoted number", "RefreshTokenValidity", Value::from(30)),
            ("a JSON number", "AccessTokenValidity", Value::from(60)),
            (
                "no number at all",
                "AuthSessionValidity",
                Value::String("not a number".to_string()),
            ),
            (
                "text",
                "ClientName",
                Value::String("shop-client".to_string()),
            ),
        ];
        for (case, member, expected) in rows {
            let carried = request.get(*member);
            assert_eq!(
                carried,
                Some(expected),
                "case {case:?}: expected {member} to travel as {expected:?}, got {carried:?}"
            );
        }
    }

    /// RULE: a declared property no call this module makes can apply is refused, naming the
    /// property, rather than dropped.
    #[test]
    fn an_unapplied_property_is_refused() {
        let resource = declared(
            COGNITO_USER_POOL,
            serde_json::json!({"UserPoolName": "shop", "NoSuchProperty": true}),
            "shop",
        );
        let refusal = refuse_unapplied_properties(&resource.properties, &resource, Kind::UserPool)
            .expect_err("a property nothing applies is refused");
        assert!(
            refusal.message.contains("NoSuchProperty"),
            "case {:?}: expected the refusal to name the property, got {:?}",
            "an unapplied property",
            refusal.message
        );
    }
}
