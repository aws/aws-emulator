// SPDX-License-Identifier: CC0-1.0

//! `AWS::Cognito::IdentityPool` and `AWS::Cognito::IdentityPoolRoleAttachment`: the identity
//! pool a stack declares and the IAM roles bound to it, created in Amazon Cognito Identity
//! itself.
//!
//! This is the per-type half of provisioning for two resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records —
//! that file is far past the length a change to this repository may touch — and is reached
//! through the same seam ([`super::Owner`]).
//!
//! ## Why the pool is really created
//!
//! "Ref returns the Amazon Cognito identity pool ID", and that id is the handle EVERY
//! operation of the owning service takes: `GetId`, `DescribeIdentityPool`,
//! `GetIdentityPoolRoles` and `SetIdentityPoolRoles` each take an `IdentityPoolId` whose
//! modelled `@pattern ^[\w-]+:[0-9a-f-]+$` the pool's NAME does not satisfy. The id is a
//! `REGION:GUID` the owning service mints — no naming rule computes that GUID — so the only
//! way a stack can publish a value a client can call Amazon Cognito Identity with is for the
//! pool to exist there and for `CreateIdentityPool`'s own answer to be the resource's
//! physical id ([`super::super::template::identity_is_service_assigned`] names the type, so
//! every reference to the resource is re-pointed at it once the create has answered).
//!
//! ## The properties
//!
//! `AWS::Cognito::IdentityPool`'s properties are `CreateIdentityPool`'s own members under the
//! same names, with two exceptions:
//!
//! * `IdentityPoolName` is the name the pool is created UNDER rather than its identity: a
//!   declaration that states none is created under the identifier CloudFormation computed for
//!   the resource, which is the name a generated one takes;
//! * `IdentityPoolTags` is a MAP in the template and a map in the API too, so it travels as
//!   written — merged with the tags CloudFormation propagates to the resource
//!   ([`super::super::provisioner::tags_of`], the one place that rule is written).
//!
//! The type's three remaining properties are each a different Amazon Cognito API: `PushSync`
//! and `CognitoStreams` are `cognito-sync` configurations and `CognitoEvents` is that
//! service's event stream. None of them is applied here, so a declaration carrying one is
//! REFUSED rather than dropped ([`refuse_unapplied_properties`]): a pool reported created
//! with less than its declaration asked for is the shallow success this arm exists to avoid.
//!
//! `AWS::Cognito::IdentityPoolRoleAttachment` is `SetIdentityPoolRoles` in template form: its
//! `Roles` and `RoleMappings` are that call's own members, and its `IdentityPoolId` is the
//! pool they are set on. The attachment is not a resource of its own in the owning API — a
//! pool has at most one role configuration, which each call REPLACES — so its physical id
//! stays CloudFormation's own, its update is the same call again, and its deletion is the
//! call that leaves the pool with no roles at all.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_json_rpc, Created, Refusal, AWS_JSON_1_1};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource types this module provisions.
pub(super) const COGNITO_IDENTITY_POOL: &str = "AWS::Cognito::IdentityPool";
pub(super) const COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT: &str =
    "AWS::Cognito::IdentityPoolRoleAttachment";

/// Amazon Cognito Identity's signing name, which is what routes the in-process call to its
/// own handlers, and the service shape an awsJson `X-Amz-Target` names an operation under
/// (that model's `sigv4Name: cognito-identity` and service shape
/// `AWSCognitoIdentityService`, `smithy_query(query="service_info")`). It speaks
/// `awsJson1_1`.
const COGNITO_IDENTITY_SIGNING_NAME: &str = "cognito-identity";
const COGNITO_IDENTITY_TARGET_PREFIX: &str = "AWSCognitoIdentityService";

/// The error code Amazon Cognito Identity answers for a pool it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its reads, writes and deletes), which is
/// what says a delete has nothing left to remove.
const POOL_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// Which of the two types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::Cognito::IdentityPool`.
    IdentityPool,
    /// `AWS::Cognito::IdentityPoolRoleAttachment`.
    RoleAttachment,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        COGNITO_IDENTITY_POOL => Some(Kind::IdentityPool),
        COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT => Some(Kind::RoleAttachment),
        _ => None,
    }
}

/// The property naming the pool, which is the name the create is made under rather than part
/// of the pool's identity.
const IDENTITY_POOL_NAME: &str = "IdentityPoolName";

/// The property carrying the pool's own tag map.
const IDENTITY_POOL_TAGS: &str = "IdentityPoolTags";

/// The property naming the pool a role attachment configures, and the member every call on a
/// pool is addressed by.
const IDENTITY_POOL_ID: &str = "IdentityPoolId";

/// The member `CreateIdentityPool` and `DescribeIdentityPool` answer the pool's id under.
const IDENTITY_POOL_ID_MEMBER: &str = "IdentityPoolId";

/// The attribute `AWS::Cognito::IdentityPool` publishes through `Fn::GetAtt` ("Name: the name
/// of the Amazon Cognito identity pool"), read off the member the create answers it under.
const NAME_ATTRIBUTE: &str = "Name";

/// The properties that ARE `CreateIdentityPool`'s own members under the same name, so the
/// declared value travels into the request as written.
///
/// Source: `CreateIdentityPool`'s member list
/// (`smithy_query(query="operation",operation="CreateIdentityPool")`), intersected with the
/// properties `AWS::Cognito::IdentityPool` publishes. `AllowUnauthenticatedIdentities` is
/// REQUIRED by both.
const POOL_MEMBERS: &[&str] = &[
    "AllowClassicFlow",
    "AllowUnauthenticatedIdentities",
    "CognitoIdentityProviders",
    "DeveloperProviderName",
    "OpenIdConnectProviderARNs",
    "SamlProviderARNs",
    "SupportedLoginProviders",
];

/// Every property `AWS::Cognito::IdentityPool` publishes. A declared property this list does
/// not carry is a property of a DIFFERENT Amazon Cognito API (see the module docs) and is
/// refused rather than dropped.
const POOL_PROPERTIES: &[&str] = &[
    "AllowClassicFlow",
    "AllowUnauthenticatedIdentities",
    "CognitoIdentityProviders",
    "DeveloperProviderName",
    "IdentityPoolName",
    "IdentityPoolTags",
    "OpenIdConnectProviderARNs",
    "SamlProviderARNs",
    "SupportedLoginProviders",
];

/// Every property `AWS::Cognito::IdentityPoolRoleAttachment` publishes. Two of them are
/// `SetIdentityPoolRoles`' own members under the same name (`Roles`, which that model makes
/// required, and `RoleMappings`); the third names the pool the call is addressed to.
const ROLE_ATTACHMENT_PROPERTIES: &[&str] = &["IdentityPoolId", "RoleMappings", "Roles"];

/// The member of a template's `RoleMapping` that names the identity provider the mapping is
/// for — "identifier for the identity provider for which the role is mapped", which the
/// API's own `RoleMappings` map carries as its KEY instead.
const IDENTITY_PROVIDER_MEMBER: &str = "IdentityProvider";

/// The `RoleMappings` member of the request, built from the declaration's own map.
///
/// The two sides key the same thing differently: `SetIdentityPoolRoles` takes a map FROM the
/// identity provider's identifier (`<provider name>` or `<provider name>:<client id>`) to its
/// mapping, while a template's entry carries that identifier in the mapping's own
/// `IdentityProvider` member and may be held under any key at all (the console writes
/// `userpool`). So the key the request uses is the declared `IdentityProvider` where the entry
/// states one, and the entry's own key otherwise — and the member itself does not travel, not
/// being a member of the API's `RoleMapping` at all. Every other member of the mapping is
/// spelled identically on both sides and goes through as the template wrote it.
fn declared_role_mappings(properties: &Value) -> Option<Value> {
    let declared = properties.get("RoleMappings")?.as_object()?;
    let mut mappings = Map::new();
    for (key, mapping) in declared {
        let provider = mapping
            .get(IDENTITY_PROVIDER_MEMBER)
            .and_then(Value::as_str)
            .filter(|provider| !provider.is_empty())
            .unwrap_or(key.as_str())
            .to_string();
        let carried = match mapping.as_object() {
            Some(members) => Value::Object(
                members
                    .iter()
                    .filter(|(member, _)| *member != IDENTITY_PROVIDER_MEMBER)
                    .map(|(member, value)| (member.clone(), value.clone()))
                    .collect(),
            ),
            None => mapping.clone(),
        };
        mappings.insert(provider, carried);
    }
    Some(Value::Object(mappings))
}

/// The two properties of `AWS::Cognito::IdentityPool` the owning API models as BOOLEANS,
/// which a template may write as a quoted scalar or feed through a `Ref` to a `String`
/// parameter — every value of one arrives as text.
const POOL_FLAGS: &[&str] = &["AllowClassicFlow", "AllowUnauthenticatedIdentities"];

/// A declared boolean in either spelling a template gives one: JSON's boolean, or the text a
/// template spells one as, read case-insensitively — the same reading
/// [`super::super::template`] makes of a template boolean. `None` is a value that is no
/// boolean at all, which the owning service's own validation is what judges, so it is left
/// exactly as written.
fn declared_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// Create the resource one of these types declares, in Amazon Cognito Identity itself — so a
/// client that deployed the stack can pass `!Ref <pool>` straight to `GetId` or
/// `DescribeIdentityPool`, and an identity of the pool assumes the role the stack attached.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::IdentityPool => create_pool(ctx, region, resource, propagated).await,
        // A role attachment is a configuration of a pool rather than a resource of its own,
        // so CloudFormation propagates no tag to it: there is no call that could carry one.
        Kind::RoleAttachment => set_roles(ctx, region, &attachment_request(resource)?).await,
    }
}

/// Create the pool an `AWS::Cognito::IdentityPool` declares.
///
/// `Err` is the owning service's own refusal of the create, which is what the resource fails
/// with: a name outside the modelled bounds, a provider configuration it will not take, a
/// pool count past the account's limit.
async fn create_pool(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, &resource.resource_type, POOL_PROPERTIES)?;
    let mut request = declared_members(&resource.properties, POOL_MEMBERS);
    // The pool is created under the name the declaration asked for, or under the identifier
    // CloudFormation computed for the resource where it names none.
    request.insert(
        IDENTITY_POOL_NAME.to_string(),
        Value::String(pool_name(resource)),
    );
    // `AllowUnauthenticatedIdentities` is required by the model and by the type; a
    // declaration that states none is created with the member's own documented default
    // (`@default false`), which is what the owning service would store for it.
    request
        .entry(POOL_MEMBERS[1].to_string())
        .or_insert_with(|| Value::Bool(false));
    let tags = pool_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert(
            IDENTITY_POOL_TAGS.to_string(),
            Value::Object(tags.into_iter().map(|(key, value)| (key, Value::String(value))).collect()),
        );
    }
    let answered = call_cognito_identity(
        ctx,
        region,
        "CreateIdentityPool",
        &Value::Object(request),
    )
    .await?;
    // The identity in force is the one the create ANSWERED with: the `REGION:GUID` id the
    // owning service minted is no value CloudFormation could have computed.
    let identity_pool_id = answered
        .get(IDENTITY_POOL_ID_MEMBER)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "Amazon Cognito Identity reported no {IDENTITY_POOL_ID_MEMBER} for the pool it \
                 created, which is what a reference to this resource resolves to"
            ),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    if let Some(name) = answered.get(IDENTITY_POOL_NAME).and_then(Value::as_str) {
        attributes.insert(NAME_ATTRIBUTE.to_string(), name.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(identity_pool_id),
        attributes,
    })
}

/// Whether the owning service reports the resource ready.
///
/// Nothing is asked, and nothing waits: a pool EXISTS from the moment `CreateIdentityPool`
/// answers — `DescribeIdentityPool` resolves it and `GetId` mints identities in it — and a
/// role configuration is in force from the moment `SetIdentityPoolRoles` answers. Neither
/// call reports a non-terminal state for the resource to settle out of.
pub(super) fn ready() -> super::Readiness {
    super::Readiness::Ready
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own update call.
///
/// For a pool that is `UpdateIdentityPool`, which "updates the configuration of an identity
/// pool" — the call takes the pool's id, so the pool keeps its identity and nothing is
/// replaced. For a role attachment it is `SetIdentityPoolRoles` again, the one call that
/// states a pool's roles, which replaces the configuration in force.
///
/// `Ok(None)` is a change neither call can make: a role attachment moved to a DIFFERENT pool
/// is a different attachment, and the pool it was on would keep the roles this resource put
/// there, so the deployment reports the change as not applied rather than as a success that
/// left the old pool configured.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    match kind {
        Kind::IdentityPool => {
            // The DECLARED document, not the record's: by the time this is reached the
            // resource carries the properties it was CREATED with, so a property the new
            // declaration introduces is only in `declared` — and an unapplied one has to be
            // refused there too, or the deployment would report a change it never made.
            refuse_unapplied_properties(declared, &resource.resource_type, POOL_PROPERTIES)?;
            let mut request = declared_members(declared, POOL_MEMBERS);
            request.insert(
                IDENTITY_POOL_ID.to_string(),
                Value::String(resource.physical_resource_id.clone()),
            );
            // Both members `UpdateIdentityPool` requires are sent whether or not the
            // declaration changed them: the call REPLACES the pool's configuration, so a
            // member left out would unset what the declaration still asks for.
            request.insert(
                IDENTITY_POOL_NAME.to_string(),
                Value::String(declared_pool_name(declared).unwrap_or_else(|| pool_name(resource))),
            );
            request
                .entry(POOL_MEMBERS[1].to_string())
                .or_insert_with(|| Value::Bool(false));
            let answered = call_cognito_identity(
                ctx,
                region,
                "UpdateIdentityPool",
                &Value::Object(request),
            )
            .await?;
            let mut attributes = BTreeMap::new();
            if let Some(name) = answered.get(IDENTITY_POOL_NAME).and_then(Value::as_str) {
                attributes.insert(NAME_ATTRIBUTE.to_string(), name.to_string());
            }
            Ok(Some(Created::attributes(attributes)))
        }
        Kind::RoleAttachment => {
            refuse_unapplied_properties(
                declared,
                &resource.resource_type,
                ROLE_ATTACHMENT_PROPERTIES,
            )?;
            // The pool this resource CONFIGURED, read off the record. A declaration that
            // names another one is a different attachment: the pool this one is on would
            // keep the roles it put there, so the change is reported as not applied.
            let configured = attachment_pool(resource)?;
            let moved = declared
                .get(IDENTITY_POOL_ID)
                .and_then(Value::as_str)
                .is_some_and(|pool| pool != configured);
            if moved {
                return Ok(None);
            }
            let mut request = attachment_members(declared);
            request.insert(IDENTITY_POOL_ID.to_string(), Value::String(configured));
            set_roles(ctx, region, &Value::Object(request))
                .await
                .map(Some)
        }
    }
}

/// Delete the resource a stack created.
///
/// A pool is deleted by `DeleteIdentityPool`, addressed by the id its create answered — this
/// resource's physical id. A role attachment has no delete of its own: what it created is the
/// pool's role configuration, so its deletion is the call that leaves the pool with none
/// ("Sets the roles for an identity pool", `Roles` being the whole configuration in force).
/// Either way a pool the owning service no longer holds is already in the state the delete
/// asks for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (operation, request) = match kind {
        Kind::IdentityPool => (
            "DeleteIdentityPool",
            serde_json::json!({IDENTITY_POOL_ID: resource.physical_resource_id.clone()}),
        ),
        Kind::RoleAttachment => (
            "SetIdentityPoolRoles",
            serde_json::json!({
                IDENTITY_POOL_ID: attachment_pool(resource)?,
                "Roles": Map::new(),
            }),
        ),
    };
    match call_cognito_identity(ctx, region, operation, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == POOL_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's configuration as the owning service reports it, for drift detection.
///
/// A pool is read back with `DescribeIdentityPool`, whose answer carries the very members the
/// type declares its properties with, so the answer maps onto the property vocabulary one for
/// one; a role attachment is read with `GetIdentityPoolRoles`, the one call that reports a
/// pool's role configuration. A pool the owning service no longer holds was deleted
/// ([`POOL_NOT_FOUND`]).
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (operation, request, reported): (&str, Value, &[&str]) = match kind {
        Kind::IdentityPool => (
            "DescribeIdentityPool",
            serde_json::json!({IDENTITY_POOL_ID: resource.physical_resource_id.clone()}),
            POOL_PROPERTIES,
        ),
        Kind::RoleAttachment => (
            "GetIdentityPoolRoles",
            serde_json::json!({IDENTITY_POOL_ID: attachment_pool(resource)?}),
            ROLE_ATTACHMENT_PROPERTIES,
        ),
    };
    match call_cognito_identity(ctx, region, operation, &request).await {
        Ok(answer) => {
            let mut configuration = Map::new();
            for property in reported {
                if let Some(value) = answer.get(*property) {
                    configuration.insert((*property).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(configuration)))
        }
        Err(refusal) if refusal.code == POOL_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `SetIdentityPoolRoles` request one role attachment declares: the pool it configures
/// and the two members that call takes, `Roles` sent as the empty map a declaration that
/// states none means (the member is required, and "Roles" is the whole configuration).
fn attachment_request(resource: &StoredResource) -> Result<Value, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        &resource.resource_type,
        ROLE_ATTACHMENT_PROPERTIES,
    )?;
    let mut request = attachment_members(&resource.properties);
    request.insert(
        IDENTITY_POOL_ID.to_string(),
        Value::String(attachment_pool(resource)?),
    );
    Ok(Value::Object(request))
}

/// The two members `SetIdentityPoolRoles` takes beside the pool, as one declaration states
/// them: its `Roles` — sent as the empty map a declaration that states none means, the member
/// being required and the call replacing the whole configuration in force — and its
/// `RoleMappings`, keyed the way that call keys them ([`declared_role_mappings`]).
fn attachment_members(properties: &Value) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(roles) = properties.get("Roles").filter(|roles| !roles.is_null()) {
        request.insert("Roles".to_string(), roles.clone());
    }
    request
        .entry("Roles".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(mappings) = declared_role_mappings(properties) {
        request.insert("RoleMappings".to_string(), mappings);
    }
    request
}

/// Set the roles one request names, and report the resource as created with no attribute of
/// its own: the type publishes only its own id, which the stack already records.
async fn set_roles(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    request: &Value,
) -> Result<Created, Refusal> {
    call_cognito_identity(ctx, region, "SetIdentityPoolRoles", request).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// The pool a role attachment configures.
///
/// `IdentityPoolId` is a required property of the type, so a declaration carrying none
/// describes no attachment that could be made: it is refused here, naming that property,
/// rather than sent as a request the owning service would reject.
fn attachment_pool(resource: &StoredResource) -> Result<String, Refusal> {
    resource
        .properties
        .get(IDENTITY_POOL_ID)
        .and_then(Value::as_str)
        .filter(|pool| !pool.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "{} declares no {IDENTITY_POOL_ID}",
                resource.resource_type
            ),
        })
}

/// The name the pool is created under: the one the declaration states, or the identifier
/// CloudFormation computed for the resource where it states none.
fn pool_name(resource: &StoredResource) -> String {
    declared_pool_name(&resource.properties)
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// The `IdentityPoolName` a declaration states, if it states one at all.
fn declared_pool_name(properties: &Value) -> Option<String> {
    properties
        .get(IDENTITY_POOL_NAME)
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

/// The members of `members` a declaration states, as the request carries them: the type and
/// the API spell each one identically, so every value goes through as the template wrote it —
/// except a member the API models as a BOOLEAN, which is read through [`declared_flag`], this
/// module's one spelling rule for a property carrying a flag.
fn declared_members(properties: &Value, members: &[&str]) -> Map<String, Value> {
    let mut request = Map::new();
    for member in members {
        if let Some(value) = properties.get(*member).filter(|value| !value.is_null()) {
            let carried = if POOL_FLAGS.contains(member) {
                declared_flag(value).map_or_else(|| value.clone(), Value::Bool)
            } else {
                value.clone()
            };
            request.insert((*member).to_string(), carried);
        }
    }
    request
}

/// The tag set the pool is created with: the map its own `IdentityPoolTags` states, plus the
/// tags CloudFormation propagates to the resource under every key that map does not use.
///
/// The merge itself is [`tags_of`], the one place that rule is written — it reads a `Tags`
/// LIST, which is the form every other taggable type declares, so this pool's MAP is handed
/// to it in that form and comes back as the pairs the API's own map takes.
fn pool_tags(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    let declared: Vec<Value> = properties
        .get(IDENTITY_POOL_TAGS)
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| serde_json::json!({"Key": key, "Value": value}))
                .collect()
        })
        .unwrap_or_default();
    tags_of(&serde_json::json!({"Tags": declared}), propagated)
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a pool from being reported as created — or as updated — with
/// less than its declaration asked for. `properties` is the document the deployment
/// DECLARES, which on an update is not the one the record carries.
fn refuse_unapplied_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

/// Call one Amazon Cognito Identity operation on the awsJson1_1 wire its model declares.
async fn call_cognito_identity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        COGNITO_IDENTITY_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COGNITO_IDENTITY_TARGET_PREFIX}.{operation}"),
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

    /// Every member the pool arm sends is a published property of the type, and the one
    /// property that is not a create member is the tag map the merge above carries.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in POOL_MEMBERS {
            assert!(
                POOL_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {COGNITO_IDENTITY_POOL}, got none"
            );
        }
        for property in POOL_PROPERTIES {
            let placed = POOL_MEMBERS.contains(property)
                || *property == IDENTITY_POOL_NAME
                || *property == IDENTITY_POOL_TAGS;
            assert!(
                placed,
                "case {property:?}: expected the property to be a create member, the pool's \
                 name or its tag map, got none of them"
            );
        }
        for member in attachment_members(&serde_json::json!({
            "Roles": {"authenticated": "arn:aws:iam::000000000000:role/r"},
            "RoleMappings": {"userpool": {"Type": "Token"}},
        }))
        .keys()
        {
            assert!(
                ROLE_ATTACHMENT_PROPERTIES.contains(&member.as_str()),
                "case {member:?}: expected the sent member to be a published property of \
                 {COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT}, got none"
            );
        }
    }

    /// The provider a role mapping is keyed by in the request is the one the declaration
    /// NAMES, whichever key the template held the mapping under, and the member that named it
    /// does not travel — the API's own `RoleMapping` has no such member.
    #[test]
    fn a_role_mapping_is_keyed_by_the_identity_provider_it_names() {
        // (case, the declared RoleMappings, the key the request must carry)
        let cases: &[(&str, Value, &str)] = &[
            (
                "a mapping naming its provider under a key of its own",
                serde_json::json!({"userpool": {
                    "IdentityProvider": "cognito-idp.us-east-1.amazonaws.com/us-east-1_a:client1",
                    "Type": "Token",
                    "AmbiguousRoleResolution": "AuthenticatedRole",
                }}),
                "cognito-idp.us-east-1.amazonaws.com/us-east-1_a:client1",
            ),
            (
                "a mapping held under the provider itself",
                serde_json::json!({"graph.facebook.com": {
                    "Type": "Token",
                    "AmbiguousRoleResolution": "Deny",
                }}),
                "graph.facebook.com",
            ),
            (
                "a mapping whose named provider is empty",
                serde_json::json!({"accounts.google.com": {
                    "IdentityProvider": "",
                    "Type": "Token",
                    "AmbiguousRoleResolution": "Deny",
                }}),
                "accounts.google.com",
            ),
        ];
        for (case, declared, expected) in cases {
            let mapped = declared_role_mappings(&serde_json::json!({
                "RoleMappings": declared.clone(),
            }))
            .unwrap_or_else(|| panic!("case {case:?}: expected a mapped RoleMappings member"));
            let keys: Vec<&String> = mapped
                .as_object()
                .map(|mappings| mappings.keys().collect())
                .unwrap_or_default();
            assert_eq!(
                keys,
                vec![&(*expected).to_string()],
                "case {case:?}: expected the request to key the mapping by {expected:?}, got \
                 {keys:?}"
            );
            let entry = mapped
                .get(*expected)
                .unwrap_or_else(|| panic!("case {case:?}: no mapping under {expected:?}"));
            assert!(
                entry.get(IDENTITY_PROVIDER_MEMBER).is_none(),
                "case {case:?}: the member naming the provider must not travel, got {entry}"
            );
            assert!(
                entry.get("Type").is_some(),
                "case {case:?}: every other member travels as written, got {entry}"
            );
        }
    }

    /// A boolean property is read in either spelling a template gives one.
    #[test]
    fn a_declared_flag_is_read_in_either_spelling() {
        // (case, the declared value, the flag the request must carry)
        let cases: &[(&str, Value, Option<bool>)] = &[
            ("a JSON boolean", serde_json::json!(true), Some(true)),
            ("a quoted scalar", serde_json::json!("true"), Some(true)),
            ("a quoted false", serde_json::json!("false"), Some(false)),
            ("a mixed-case scalar", serde_json::json!("True"), Some(true)),
            ("a value that is no boolean", serde_json::json!("yes"), None),
        ];
        for (case, declared, expected) in cases {
            let request = declared_members(
                &serde_json::json!({"AllowUnauthenticatedIdentities": declared.clone()}),
                POOL_MEMBERS,
            );
            let carried = request.get("AllowUnauthenticatedIdentities");
            match expected {
                Some(flag) => assert_eq!(
                    carried,
                    Some(&Value::Bool(*flag)),
                    "case {case:?}: expected the flag {flag}, got {carried:?}"
                ),
                // A value that is no boolean is left exactly as written, for the owning
                // service's own validation to judge.
                None => assert_eq!(
                    carried,
                    Some(declared),
                    "case {case:?}: expected the value as written, got {carried:?}"
                ),
            }
        }
    }

    /// The pool is created under the declared name where there is one, and under the
    /// identifier CloudFormation computed where the declaration names none.
    #[test]
    fn the_declared_name_is_the_name_the_pool_is_created_under() {
        let cases: &[(&str, Value, &str)] = &[
            (
                "a declared name",
                serde_json::json!({"IdentityPoolName": "stack-pool"}),
                "stack-pool",
            ),
            (
                "no name at all",
                serde_json::json!({"AllowUnauthenticatedIdentities": true}),
                "stack-Pool-ABC123",
            ),
            (
                "an empty name",
                serde_json::json!({"IdentityPoolName": ""}),
                "stack-Pool-ABC123",
            ),
        ];
        for (case, properties, expected) in cases {
            let resource =
                declared(COGNITO_IDENTITY_POOL, properties.clone(), "stack-Pool-ABC123");
            let named = pool_name(&resource);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected the create to be made under {expected:?}, got {named:?}"
            );
        }
    }

    /// A property of a different Amazon Cognito API is refused rather than dropped.
    #[test]
    fn an_unapplied_property_is_refused() {
        let cases: &[(&str, &str, Value)] = &[
            (
                "a cognito-sync push configuration",
                COGNITO_IDENTITY_POOL,
                serde_json::json!({"IdentityPoolName": "p", "PushSync": {"ApplicationArns": []}}),
            ),
            (
                "a cognito-sync event stream",
                COGNITO_IDENTITY_POOL,
                serde_json::json!({"IdentityPoolName": "p", "CognitoEvents": {}}),
            ),
            (
                "a property of no Amazon Cognito API at all",
                COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT,
                serde_json::json!({"IdentityPoolId": "us-east-1:a", "Invented": true}),
            ),
        ];
        for (case, resource_type, properties) in cases {
            let published = if *resource_type == COGNITO_IDENTITY_POOL {
                POOL_PROPERTIES
            } else {
                ROLE_ATTACHMENT_PROPERTIES
            };
            let refused = refuse_unapplied_properties(properties, resource_type, published);
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the unapplied property, got {refused:?}"
            );
        }
    }

    /// The pool's own tag map and the tags CloudFormation propagates end up in one map, with
    /// the declaration winning for a key it states itself.
    #[test]
    fn the_declared_tag_map_is_merged_with_the_propagated_tags() {
        let properties = serde_json::json!({
            "IdentityPoolTags": {"Owner": "declared", "Team": "identity"},
        });
        let propagated = vec![
            ("Owner".to_string(), "propagated".to_string()),
            ("Stage".to_string(), "test".to_string()),
        ];
        let merged: BTreeMap<String, String> =
            pool_tags(&properties, &propagated).into_iter().collect();
        for (case, key, expected) in [
            ("a key the declaration states", "Owner", "declared"),
            ("a key only the stack states", "Stage", "test"),
            ("a key only the declaration states", "Team", "identity"),
        ] {
            assert_eq!(
                merged.get(key).map(String::as_str),
                Some(expected),
                "case {case:?}: expected {expected:?} for {key:?}, got {merged:?}"
            );
        }
    }

    /// A role attachment that declares no pool is refused naming that property.
    #[test]
    fn a_role_attachment_with_no_pool_is_refused() {
        let resource = declared(
            COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT,
            serde_json::json!({"Roles": {}}),
            "stack-Attachment-ABC123",
        );
        let refused = attachment_pool(&resource);
        assert!(
            refused.is_err(),
            "case \"no IdentityPoolId\": expected a refusal naming the property, got {refused:?}"
        );
    }
}
