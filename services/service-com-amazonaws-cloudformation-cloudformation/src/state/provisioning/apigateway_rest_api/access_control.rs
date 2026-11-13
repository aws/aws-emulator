// SPDX-License-Identifier: CC0-1.0

//! The four `AWS::ApiGateway::*` types that govern ACCESS to a REST API — the authorizer
//! that decides a request, the api key a caller presents, the usage plan that meters a key
//! and the association that puts a key in a plan — created in Amazon API Gateway itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls, the same helpers (a declared property is read with
//! [`super::stated`], a property a call cannot be addressed without with [`super::required`],
//! and a property no call here carries is refused with [`super::refuse_unpublished`]), so the
//! two halves of this control plane cannot word a refusal or place a member differently.
//!
//! ## Why they are really created
//!
//! Each of these resources is what DECIDES whether a caller reaches a deployed stage at all:
//! API Gateway refuses an invoke that presents a key no usage plan meters, or one whose
//! method is authorized by an authorizer the api does not hold, and it answers every read of
//! one by the identifier that service assigned. A stack whose key, plan or authorizer exists
//! nowhere therefore publishes an identifier that resolves to nothing — `GetApiKey` and
//! `GetAuthorizer` answer `NotFoundException` for it — and the stage it deployed enforces
//! none of the access control the template declared, however complete the stack's own
//! records look.
//!
//! The identifier is also that service's to assign: `CreateApiKey`, `CreateAuthorizer` and
//! `CreateUsagePlan` each take no identifier and answer the `id` they chose, and a
//! `UsagePlanKey`'s own id is the id of the key it associates. So the only way a stack can
//! publish a value a client can call API Gateway with is for the resource to exist there and
//! for that create's own answer to be the resource's physical id
//! ([`crate::state::template::identity_is_service_assigned`] names the types whose identity
//! that is).
//!
//! ## The calls
//!
//! Each type is the call that owning service publishes for it, on the URI that operation's
//! own `@http` binding declares:
//!
//! * `AWS::ApiGateway::ApiKey` is `CreateApiKey` (`POST /apikeys`);
//! * `AWS::ApiGateway::Authorizer` is `CreateAuthorizer`
//!   (`POST /restapis/{restApiId}/authorizers`);
//! * `AWS::ApiGateway::UsagePlan` is `CreateUsagePlan` (`POST /usageplans`);
//! * `AWS::ApiGateway::UsagePlanKey` is `CreateUsagePlanKey`
//!   (`POST /usageplans/{usagePlanId}/keys`).

use super::*;

use crate::state::template::{
    APIGATEWAY_API_KEY, APIGATEWAY_AUTHORIZER, APIGATEWAY_USAGE_PLAN, APIGATEWAY_USAGE_PLAN_KEY,
};

/// Which of these four types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Kind`], which travels with [`super::super::Owner`]: the algorithm resolves a
/// type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ApiGateway::ApiKey`.
    ApiKey,
    /// `AWS::ApiGateway::Authorizer`.
    Authorizer,
    /// `AWS::ApiGateway::UsagePlan`.
    UsagePlan,
    /// `AWS::ApiGateway::UsagePlanKey`.
    UsagePlanKey,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        APIGATEWAY_API_KEY => Some(Kind::ApiKey),
        APIGATEWAY_AUTHORIZER => Some(Kind::Authorizer),
        APIGATEWAY_USAGE_PLAN => Some(Kind::UsagePlan),
        APIGATEWAY_USAGE_PLAN_KEY => Some(Kind::UsagePlanKey),
        _ => None,
    }
}

/// The property naming the usage plan a `UsagePlanKey` belongs to, which addresses the call
/// rather than travelling in its body.
const USAGE_PLAN_ID: &str = "UsagePlanId";

/// The collection every API key of the account lives in, which is what `CreateApiKey` is
/// bound to.
const API_KEYS_URI: &str = "/apikeys";

/// The collection every usage plan of the account lives in, which is what `CreateUsagePlan`
/// is bound to.
const USAGE_PLANS_URI: &str = "/usageplans";

/// The members of one of these creates the model declares `@required`, as `(property, member)`
/// pairs: a declaration stating none of one asks for a resource that call will not make, so it
/// is refused here naming the PROPERTY — in the one wording of that refusal this control plane
/// has ([`super::missing_property`]) — rather than relaying the request for the owning service
/// to refuse by its own member name.
fn refuse_missing(
    resource: &StoredResource,
    request: &Map<String, Value>,
    members: &[(&str, &str)],
) -> Result<(), Refusal> {
    for (property, member) in members {
        if !request.contains_key(*member) {
            return Err(missing_property(resource, property));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::ApiKey
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::ApiKey` publishes. A declared property this list does
/// not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `CreateApiKey`'s members
/// (`smithy_query(query="operation",operation="CreateApiKey")`).
const API_KEY_PROPERTIES: &[&str] = &[
    "CustomerId",
    "Description",
    "Enabled",
    "GenerateDistinctId",
    "Name",
    "StageKeys",
    "Tags",
    "Value",
];

/// The properties that ARE `CreateApiKey`'s own body members, as `(property, member)` pairs:
/// every one of them is the property with a lower-cased initial, and a `StageKeys` entry's
/// own members (`RestApiId`, `StageName`) re-case the same way into `StageKey`'s.
const API_KEY_MEMBERS: &[(&str, &str)] = &[
    ("CustomerId", "customerId"),
    ("Description", "description"),
    ("Enabled", "enabled"),
    ("GenerateDistinctId", "generateDistinctId"),
    ("Name", "name"),
    ("StageKeys", "stageKeys"),
    ("Value", "value"),
];

/// The properties API Gateway's own read of a key reports, as `(property, member)` pairs.
///
/// The SCALAR configuration alone, for the reason [`super::REST_API_REPORTED`] records: the
/// tags a key carries change independently of its declaration, the read reports a key's
/// stage keys as `{restApiId}/{stageName}` TEXT while a declaration states them as objects,
/// and neither the key's `Value` (which the read withholds unless asked for it) nor
/// `GenerateDistinctId` (an instruction to the create, not a property of the key) is a
/// value a later read could find changed.
const API_KEY_REPORTED: &[(&str, &str)] = &[
    ("CustomerId", "customerId"),
    ("Description", "description"),
    ("Enabled", "enabled"),
    ("Name", "name"),
];

/// Create the key an `AWS::ApiGateway::ApiKey` declares, so a client that deployed the stack
/// and then calls `apigateway GetApiKey` with the id the stack publishes reads the key back
/// — and so a usage plan can meter it and an invoke presenting it is admitted — rather than
/// a `NotFoundException` for a key nothing created.
///
/// "Ref returns the API key ID", which only that service produces, so the `id` it answers
/// replaces the stand-in the naming rules computed and is also the `APIKeyId` attribute the
/// type publishes.
async fn create_api_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, API_KEY_PROPERTIES)?;
    let mut request = Map::new();
    for (property, member) in API_KEY_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    let tags = gateway_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        API_KEYS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateApiKey")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("APIKeyId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The URI that addresses ONE api key: the `@httpLabel` every call on it binds.
fn api_key_uri(api_key_id: &str) -> String {
    format!("{API_KEYS_URI}/{}", percent_encoded(api_key_id))
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Authorizer
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Authorizer` publishes.
///
/// Source: the type's own property set, against `CreateAuthorizer`'s members
/// (`smithy_query(query="operation",operation="CreateAuthorizer")`).
const AUTHORIZER_PROPERTIES: &[&str] = &[
    "AuthType",
    "AuthorizerCredentials",
    "AuthorizerResultTtlInSeconds",
    "AuthorizerUri",
    "IdentitySource",
    "IdentityValidationExpression",
    "Name",
    "ProviderARNs",
    REST_API_ID,
    "Type",
];

/// The properties that ARE `CreateAuthorizer`'s own body members, as `(property, member)`
/// pairs: every one of them is the property with a lower-cased initial, the api it belongs to
/// addressing the call instead.
const AUTHORIZER_MEMBERS: &[(&str, &str)] = &[
    ("AuthType", "authType"),
    ("AuthorizerCredentials", "authorizerCredentials"),
    ("AuthorizerResultTtlInSeconds", "authorizerResultTtlInSeconds"),
    ("AuthorizerUri", "authorizerUri"),
    ("IdentitySource", "identitySource"),
    ("IdentityValidationExpression", "identityValidationExpression"),
    ("Name", "name"),
    ("ProviderARNs", "providerARNs"),
    ("Type", "type"),
];

/// The properties an authorizer's own read reports: every member of it, each under this
/// type's property name — an authorizer has no structured property and carries no tags, so
/// there is none left out.
const AUTHORIZER_REPORTED: &[(&str, &str)] = AUTHORIZER_MEMBERS;

/// The members `CreateAuthorizer` declares `@required`: "the authorizer's name" and "the
/// authorizer type", neither of which anything but the declaration can supply.
const AUTHORIZER_REQUIRED: &[(&str, &str)] = &[("Name", "name"), ("Type", "type")];

/// Create the authorizer an `AWS::ApiGateway::Authorizer` declares on the api it names, so a
/// client that deployed the stack reads it back through `GetAuthorizer` — and so a method
/// authorized by it really has its identity source enforced on the data plane — rather than
/// an api that holds no authorizer at all.
///
/// "Ref returns the authorizer ID", which API Gateway assigns: `CreateAuthorizer` takes no
/// identifier and answers the `id` it chose, so that id is this resource's physical id and
/// the `AuthorizerId` attribute the type publishes.
async fn create_authorizer(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, AUTHORIZER_PROPERTIES)?;
    let mut request = Map::new();
    for (property, member) in AUTHORIZER_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    refuse_missing(resource, &request, AUTHORIZER_REQUIRED)?;
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &authorizers_uri(&required(resource, REST_API_ID)?),
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateAuthorizer")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("AuthorizerId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The collection the authorizers of ONE api live in, which is what `CreateAuthorizer` is
/// bound to.
fn authorizers_uri(api_id: &str) -> String {
    format!("{}/authorizers", rest_api_uri(api_id))
}

/// The URI that addresses ONE authorizer: the api that holds it and its own id.
fn authorizer_uri(api_id: &str, authorizer_id: &str) -> String {
    format!(
        "{}/{}",
        authorizers_uri(api_id),
        percent_encoded(authorizer_id)
    )
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::UsagePlan
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::UsagePlan` publishes.
///
/// Source: the type's own property set, against `CreateUsagePlan`'s members
/// (`smithy_query(query="operation",operation="CreateUsagePlan")`).
const USAGE_PLAN_PROPERTIES: &[&str] = &[
    USAGE_PLAN_API_STAGES,
    "Description",
    "Quota",
    "Tags",
    "Throttle",
    USAGE_PLAN_NAME,
];

/// The property carrying the api stages the plan's limits apply to, whose entries are mapped
/// member by member ([`api_stages`]) because one of their members is a map the caller keys.
const USAGE_PLAN_API_STAGES: &str = "ApiStages";

/// The property carrying the plan's name, which `CreateUsagePlan` takes as its `@required`
/// `name` — the one member of these types that is not the property with a lower-cased
/// initial.
const USAGE_PLAN_NAME: &str = "UsagePlanName";

/// The properties that ARE `CreateUsagePlan`'s own body members, as `(property, member)`
/// pairs. The quota and the throttle re-case member for member (`Limit`/`Offset`/`Period`
/// and `BurstLimit`/`RateLimit`), and the api stages are mapped by [`api_stages`].
const USAGE_PLAN_MEMBERS: &[(&str, &str)] = &[
    ("Description", "description"),
    ("Quota", "quota"),
    ("Throttle", "throttle"),
    (USAGE_PLAN_NAME, "name"),
];

/// The properties a plan's own read reports, under this type's property names: its scalar
/// configuration alone, for the reason [`API_KEY_REPORTED`] records — the quota, the
/// throttle and the api stages are reported in the model's own case while a declaration
/// spells them in CloudFormation's, and the tags change independently of the declaration.
const USAGE_PLAN_REPORTED: &[(&str, &str)] =
    &[("Description", "description"), (USAGE_PLAN_NAME, "name")];

/// The members an entry of `ApiStages` travels as.
///
/// Source: `ApiStage`'s own members (`smithy_query(query="shape",shape="ApiStage")`:
/// `apiId`, `stage`, `throttle`), each the property with a lower-cased initial.
const API_STAGE_MEMBERS: &[(&str, &str)] = &[("ApiId", "apiId"), ("Stage", "stage")];

/// The member of an `ApiStage` that is a MAP THE CALLER KEYS: "Map containing method level
/// throttling information for API stage in a usage plan", whose keys are one method's
/// `{resource_path}/{http_method}` rather than modelled member names. Re-casing such a key
/// would change the method the declaration asked to throttle, so each key travels as
/// written and only the `ThrottleSettings` behind it is re-cased.
const API_STAGE_THROTTLE: &str = "Throttle";

/// The one member `CreateUsagePlan` declares `@required`: the plan's name.
const USAGE_PLAN_REQUIRED: &[(&str, &str)] = &[(USAGE_PLAN_NAME, "name")];

/// Create the plan an `AWS::ApiGateway::UsagePlan` declares, so a client that deployed the
/// stack reads it back through `GetUsagePlan` — and so the throttle and quota it declared
/// are the ones API Gateway meters a key's calls against — rather than a plan that exists
/// nowhere.
///
/// "Ref returns the usage plan ID", which API Gateway assigns: `CreateUsagePlan` takes no
/// identifier and answers the `id` it chose, so that id is this resource's physical id and
/// the `Id` attribute the type publishes.
async fn create_usage_plan(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, USAGE_PLAN_PROPERTIES)?;
    let mut request = Map::new();
    for (property, member) in USAGE_PLAN_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    if let Some(declared) = stated(resource, USAGE_PLAN_API_STAGES) {
        request.insert(
            "apiStages".to_string(),
            api_stages(&resource.resource_type, declared)?,
        );
    }
    // `CreateUsagePlanRequest$name` is required, and a plan has nowhere else to take its
    // name from.
    refuse_missing(resource, &request, USAGE_PLAN_REQUIRED)?;
    let tags = gateway_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        USAGE_PLANS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateUsagePlan")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The declared `ApiStages` as the `apiStages` list `CreateUsagePlan` takes, mapped entry by
/// entry: the api and the stage re-case, and the per-method throttle map keeps the keys the
/// declaration wrote ([`API_STAGE_THROTTLE`]).
fn api_stages(resource_type: &str, declared: &Value) -> Result<Value, Refusal> {
    let mut stages = Vec::new();
    for entry in entries(Some(declared)) {
        refuse_unpublished_members(
            resource_type,
            USAGE_PLAN_API_STAGES,
            entry,
            API_STAGE_MEMBERS,
            &[API_STAGE_THROTTLE],
        )?;
        let mut mapped = Map::new();
        for (property, member) in API_STAGE_MEMBERS {
            if let Some(value) = entry.get(*property).filter(|value| !value.is_null()) {
                mapped.insert((*member).to_string(), lower_camel_members(value, 0)?);
            }
        }
        if let Some(stated) = entry.get(API_STAGE_THROTTLE).filter(|value| !value.is_null()) {
            // The MAP's keys travel as written; a declaration whose throttle is no map at all
            // has no keys to re-case and travels as it stands, for that call's own validation
            // of the member to refuse — never dropped, which would create a plan that meters
            // calls against nothing the declaration asked for.
            let throttle = match stated {
                Value::Object(per_method) => {
                    let mut throttles = Map::new();
                    for (method, settings) in per_method {
                        throttles.insert(method.clone(), lower_camel_members(settings, 0)?);
                    }
                    Value::Object(throttles)
                }
                malformed => malformed.clone(),
            };
            mapped.insert("throttle".to_string(), throttle);
        }
        stages.push(Value::Object(mapped));
    }
    Ok(Value::Array(stages))
}

/// The URI that addresses ONE usage plan: the `@httpLabel` every call on it binds.
fn usage_plan_uri(usage_plan_id: &str) -> String {
    format!("{USAGE_PLANS_URI}/{}", percent_encoded(usage_plan_id))
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::UsagePlanKey
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::UsagePlanKey` publishes: the key, the kind of key it is
/// and the plan that meters it.
const USAGE_PLAN_KEY_PROPERTIES: &[&str] =
    &[USAGE_PLAN_KEY_ID, USAGE_PLAN_KEY_TYPE, USAGE_PLAN_ID];

/// The property naming the API key the association puts in the plan, which is
/// `CreateUsagePlanKey`'s `@required` `keyId`.
const USAGE_PLAN_KEY_ID: &str = "KeyId";

/// The property naming the kind of key the association is of, which is that call's
/// `@required` `keyType`.
const USAGE_PLAN_KEY_TYPE: &str = "KeyType";

/// The properties a `UsagePlanKey`'s own read reports, under this type's property names: the
/// `UsagePlanKey` body reports the key's own id and type beside the key's name and value, so
/// those two are what a declaration can be compared against. The plan is not among them: it
/// addresses the read rather than being reported by it.
const USAGE_PLAN_KEY_REPORTED: &[(&str, &str)] =
    &[(USAGE_PLAN_KEY_ID, "id"), (USAGE_PLAN_KEY_TYPE, "type")];

/// Associate the key an `AWS::ApiGateway::UsagePlanKey` declares with the plan it names, so
/// the plan really meters that key — which is what makes API Gateway admit an invoke
/// presenting it and refuse one presenting a key no plan holds.
///
/// "Ref returns the usage plan key ID": `CreateUsagePlanKey` answers the `id` of the
/// association, which is the id of the key it put in the plan, so that answer is this
/// resource's physical id and the `Id` attribute the type publishes.
async fn create_usage_plan_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, USAGE_PLAN_KEY_PROPERTIES)?;
    let mut request = Map::new();
    request.insert(
        "keyId".to_string(),
        Value::String(required(resource, USAGE_PLAN_KEY_ID)?),
    );
    request.insert(
        "keyType".to_string(),
        Value::String(required(resource, USAGE_PLAN_KEY_TYPE)?),
    );
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &usage_plan_keys_uri(&required(resource, USAGE_PLAN_ID)?),
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateUsagePlanKey")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The collection the keys of ONE usage plan live in, which is what `CreateUsagePlanKey` is
/// bound to.
fn usage_plan_keys_uri(usage_plan_id: &str) -> String {
    format!("{}/keys", usage_plan_uri(usage_plan_id))
}

/// The URI that addresses ONE association: the plan that holds it and the key's own id.
fn usage_plan_key_uri(usage_plan_id: &str, key_id: &str) -> String {
    format!(
        "{}/{}",
        usage_plan_keys_uri(usage_plan_id),
        percent_encoded(key_id)
    )
}

// ---------------------------------------------------------------------------
// The seam [`super`] reaches these arms through
// ---------------------------------------------------------------------------

/// Create one of these resources in API Gateway; see [`super::create`].
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::ApiKey => create_api_key(ctx, region, resource, propagated).await,
        // An authorizer and an association publish no `Tags` property, so CloudFormation
        // propagates no tag to either: there is no call that could carry one.
        Kind::Authorizer => create_authorizer(ctx, region, resource).await,
        Kind::UsagePlan => create_usage_plan(ctx, region, resource, propagated).await,
        Kind::UsagePlanKey => create_usage_plan_key(ctx, region, resource).await,
    }
}

/// The URI that addresses one of these resources, which is the one its own read and its own
/// delete share — built from this resource's physical id and the properties that name the
/// resource holding it.
fn addressed(kind: Kind, resource: &StoredResource) -> Result<String, Refusal> {
    let id = own_id(resource)?;
    Ok(match kind {
        Kind::ApiKey => api_key_uri(id),
        Kind::Authorizer => authorizer_uri(&required(resource, REST_API_ID)?, id),
        Kind::UsagePlan => usage_plan_uri(id),
        Kind::UsagePlanKey => usage_plan_key_uri(&required(resource, USAGE_PLAN_ID)?, id),
    })
}

/// Delete one of these resources from API Gateway; see [`super::delete`].
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    api_gateway_deleted(ctx, region, &addressed(kind, resource)?).await
}

/// The configuration one of these resources has as API Gateway reports it, for drift
/// detection; see [`super::actual_configuration`].
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let reported = match kind {
        Kind::ApiKey => API_KEY_REPORTED,
        Kind::Authorizer => AUTHORIZER_REPORTED,
        Kind::UsagePlan => USAGE_PLAN_REPORTED,
        Kind::UsagePlanKey => USAGE_PLAN_KEY_REPORTED,
    };
    let uri = addressed(kind, resource)?;
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answered) => {
            let mut held = Map::new();
            for (property, member) in reported {
                if let Some(value) = answered.get(*member) {
                    held.insert((*property).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(held)))
        }
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// A resource record as a deployment hands one to these arms.
    fn declared(resource_type: &str, physical: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Declared".to_string(),
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

    /// Every member one of these arms sends is a published property of its type, and every
    /// published property is placed somewhere: in the create's body, on the call's URI or as
    /// the resource's tags.
    #[test]
    fn every_published_property_is_placed() {
        let rows: &[(&str, &[&str], Vec<&str>)] = &[
            (
                "AWS::ApiGateway::ApiKey",
                API_KEY_PROPERTIES,
                API_KEY_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain(["Tags"])
                    .collect(),
            ),
            (
                "AWS::ApiGateway::Authorizer",
                AUTHORIZER_PROPERTIES,
                AUTHORIZER_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain([REST_API_ID])
                    .collect(),
            ),
            (
                "AWS::ApiGateway::UsagePlan",
                USAGE_PLAN_PROPERTIES,
                USAGE_PLAN_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain([USAGE_PLAN_API_STAGES, "Tags"])
                    .collect(),
            ),
            (
                "AWS::ApiGateway::UsagePlanKey",
                USAGE_PLAN_KEY_PROPERTIES,
                vec![USAGE_PLAN_KEY_ID, USAGE_PLAN_KEY_TYPE, USAGE_PLAN_ID],
            ),
        ];
        for (case, published, placed) in rows {
            for property in *published {
                assert!(
                    placed.contains(property),
                    "case {case:?}: expected the property {property:?} to be placed by some \
                     arm, got {placed:?}"
                );
            }
        }
    }

    /// Each type's resource is addressed by the URI its own operations bind, built from this
    /// resource's id and the properties that name the resource holding it.
    #[test]
    fn a_resource_is_addressed_by_the_uri_its_operations_bind() {
        let rows: &[(&str, Kind, &str, &str, Value, &str)] = &[
            (
                "an api key",
                Kind::ApiKey,
                "AWS::ApiGateway::ApiKey",
                "k1234abcde",
                json!({ "Name": "key" }),
                "/apikeys/k1234abcde",
            ),
            (
                "an authorizer of an api",
                Kind::Authorizer,
                "AWS::ApiGateway::Authorizer",
                "au12345678",
                json!({ "RestApiId": "a1b2c3d4e5" }),
                "/restapis/a1b2c3d4e5/authorizers/au12345678",
            ),
            (
                "a usage plan",
                Kind::UsagePlan,
                "AWS::ApiGateway::UsagePlan",
                "up12345678",
                json!({ "UsagePlanName": "plan" }),
                "/usageplans/up12345678",
            ),
            (
                "a key in a usage plan",
                Kind::UsagePlanKey,
                "AWS::ApiGateway::UsagePlanKey",
                "k1234abcde",
                json!({ "UsagePlanId": "up12345678", "KeyId": "k1234abcde", "KeyType": "API_KEY" }),
                "/usageplans/up12345678/keys/k1234abcde",
            ),
        ];
        for (case, kind, resource_type, physical, properties, expected) in rows {
            let resource = declared(resource_type, physical, properties.clone());
            let uri = addressed(*kind, &resource).expect("a resource naming what holds it");
            assert_eq!(
                uri, *expected,
                "case {case:?}: expected the URI {expected:?}, got {uri:?}"
            );
        }
    }

    /// A declaration that states none of a property a call cannot be addressed without is
    /// refused, rather than a resource being created somewhere else — and so is a record that
    /// carries no identifier of its own, which would address the account's own collection.
    #[test]
    fn a_declaration_naming_no_holder_is_refused() {
        let rows: &[(&str, Kind, &str, &str, Value)] = &[
            (
                "an authorizer naming no api",
                Kind::Authorizer,
                "AWS::ApiGateway::Authorizer",
                "au12345678",
                json!({ "Name": "auth" }),
            ),
            (
                "an authorizer whose api is empty",
                Kind::Authorizer,
                "AWS::ApiGateway::Authorizer",
                "au12345678",
                json!({ "RestApiId": "" }),
            ),
            (
                "an association naming no plan",
                Kind::UsagePlanKey,
                "AWS::ApiGateway::UsagePlanKey",
                "k1234abcde",
                json!({ "KeyId": "k1234abcde", "KeyType": "API_KEY" }),
            ),
            (
                "a key with no identifier of its own",
                Kind::ApiKey,
                "AWS::ApiGateway::ApiKey",
                "",
                json!({ "Name": "key" }),
            ),
            (
                "a plan with no identifier of its own",
                Kind::UsagePlan,
                "AWS::ApiGateway::UsagePlan",
                "",
                json!({ "UsagePlanName": "plan" }),
            ),
        ];
        for (case, kind, resource_type, physical, properties) in rows {
            let resource = declared(resource_type, physical, properties.clone());
            let refused = addressed(*kind, &resource);
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got \
                 {refused:?}"
            );
        }
    }

    /// One row of [`a_declaration_missing_a_required_member_is_refused`]: the case, the type
    /// declared, the required members of its create, and the request body built so far.
    type MissingMemberRow = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
        Value,
    );

    /// A declaration stating none of a member its create declares `@required` is refused
    /// naming that PROPERTY, rather than sent for the owning service to refuse by member.
    #[test]
    fn a_declaration_missing_a_required_member_is_refused() {
        let rows: Vec<MissingMemberRow> = vec![
            (
                "an authorizer stating neither name nor type",
                "AWS::ApiGateway::Authorizer",
                AUTHORIZER_REQUIRED,
                json!({}),
            ),
            (
                "an authorizer stating a name and no type",
                "AWS::ApiGateway::Authorizer",
                AUTHORIZER_REQUIRED,
                json!({ "name": "auth" }),
            ),
            (
                "a plan stating no name",
                "AWS::ApiGateway::UsagePlan",
                USAGE_PLAN_REQUIRED,
                json!({ "description": "a plan with no name" }),
            ),
        ];
        for (case, resource_type, members, request) in &rows {
            let resource = declared(resource_type, "physical", Value::Null);
            let built = request.as_object().cloned().unwrap_or_default();
            let refused = refuse_missing(&resource, &built, members);
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got \
                 {refused:?}"
            );
        }
    }

    /// An api stage's own members re-case, and its per-method throttle map keeps the method
    /// paths the declaration wrote — re-casing one of those keys would throttle a different
    /// method. A throttle that is no map at all travels as written, so the call that takes it
    /// refuses the declaration rather than this arm dropping it and making an unthrottled plan.
    #[test]
    fn an_api_stage_keeps_the_method_paths_it_throttles() {
        let declared = json!([
            { "ApiId": "a1b2c3d4e5", "Stage": "prod" },
            {
                "ApiId": "a1b2c3d4e5",
                "Stage": "beta",
                "Throttle": { "/items/GET": { "BurstLimit": 5, "RateLimit": 1.0 } }
            },
            { "ApiId": "a1b2c3d4e5", "Stage": "gamma", "Throttle": "not a map" },
        ]);
        let mapped = api_stages("AWS::ApiGateway::UsagePlan", &declared)
            .expect("a declaration of three api stages");
        assert_eq!(
            mapped,
            json!([
                { "apiId": "a1b2c3d4e5", "stage": "prod" },
                {
                    "apiId": "a1b2c3d4e5",
                    "stage": "beta",
                    "throttle": { "/items/GET": { "burstLimit": 5, "rateLimit": 1.0 } }
                },
                { "apiId": "a1b2c3d4e5", "stage": "gamma", "throttle": "not a map" },
            ]),
            "case \"three api stages\": expected the members CreateUsagePlan takes, got \
             {mapped:?}"
        );
    }
}
