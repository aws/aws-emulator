// SPDX-License-Identifier: CC0-1.0

//! Applying a CHANGED declaration to a record of this control plane that API Gateway
//! publishes a PATCH call for: the five a REST API is built out of — the api itself, a
//! resource of it, a method on that resource, a deployment of them and the stage that serves
//! one — and the client certificate a stage presents to its backend.
//!
//! ## The rule this module implements
//!
//! A deployment that changes a resource's declaration applies that change to the resource
//! that is already there, through the owning service's own update call. API Gateway publishes
//! one for each of these records — `UpdateRestApi`, `UpdateResource`, `UpdateMethod`,
//! `UpdateDeployment`, `UpdateStage` and `UpdateClientCertificate`, each a `PATCH` whose
//! `patchOperations` name the
//! pointers that service lets a caller write — so a changed declaration of one of them is a
//! change CloudFormation CARRIES OUT, and only a change no pointer of that call can express
//! leaves the resource as it is.
//!
//! That is what a redeployment of a REST API depends on: adding a method to an api creates a
//! new deployment (a deployment is an immutable SNAPSHOT of the api's methods, so a template
//! that deploys again declares another one) and re-points the stage at it, which is one
//! `replace` of the stage's `/deploymentId`. A stack that reported such an update as a change
//! it does not make fails the whole update and rolls it back, and the stage goes on serving
//! the snapshot taken before the method existed.
//!
//! ## How a change becomes patches
//!
//! Each type has a table of the properties that service's patch call writes, beside the
//! pointer each one is written on and the `op` that CLEARS it where the new declaration no
//! longer states it ([`Patchable`]). A property whose stated value is unchanged is not
//! patched at all; one the table does not carry — a property fixed when the resource was
//! created, or a collection with a shape of its own — is answered as a change this emulator
//! does not apply (`Ok(None)`), which the deployment reports rather than recording a
//! configuration nothing wrote.
//!
//! The nested properties each type publishes are applied as their own calls: a map is one
//! patch per key (`/variables/{name}`), a list one patch per entry
//! (`/binaryMediaTypes`), and a method's integration and responses are the PUTs that own
//! them, which a put of the changed declaration overwrites exactly as the create made them.

use super::*;

use super::client_certificate::{client_certificate_uri, CLIENT_CERTIFICATE_PROPERTIES};

/// One property a patch call writes: the property a declaration states it under, the JSON
/// Pointer that call writes it on, and how a declaration that no longer states it is
/// CLEARED — the `op` that pointer takes for that ([`REMOVE`] where the member may be
/// absent, [`REPLACE`] with no value where the pointer takes no `remove`), or `None` for a
/// member that always carries a value, which a declaration cannot drop.
type Patchable = (&'static str, &'static str, Option<&'static str>);

/// The `op` that writes a value on a pointer.
const REPLACE: &str = "replace";

/// The `op` that takes a value off a pointer, and the one that puts an entry into a
/// collection keyed by name.
const REMOVE: &str = "remove";
const ADD: &str = "add";

/// Apply a changed declaration to one of these records, through the update call API Gateway
/// publishes for it — see [`super::update`], which dispatches here.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    match kind {
        Kind::RestApi => update_rest_api(ctx, region, resource, declared).await,
        Kind::Resource => update_resource(ctx, region, resource, declared).await,
        Kind::Method => update_method(ctx, region, resource, declared).await,
        Kind::Deployment => update_deployment(ctx, region, resource, declared).await,
        Kind::Stage => update_stage(ctx, region, resource, declared).await,
        // The client certificate's own patch call, which is the one record here that is not
        // part of an api: `UpdateClientCertificate` re-describes the certificate the stack
        // minted ([`update_client_certificate`]).
        Kind::ClientCertificate => update_client_certificate(ctx, region, resource, declared).await,
        // Every other kind is answered by its own module, which [`super::update`] reaches
        // directly; nothing dispatches them here.
        Kind::VpcLink
        | Kind::Account
        | Kind::AccessControl(_)
        | Kind::RequestValidation(_)
        | Kind::GatewayResponse
        | Kind::Documentation(_) => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::RestApi
// ---------------------------------------------------------------------------

/// The properties `UpdateRestApi` writes, each on the pointer that call names it by.
///
/// Source: that operation's own patch surface — the members of the `RestApi` shape a patch
/// may name — against the type's property set. `Name` and `DisableExecuteApiEndpoint` cannot
/// be cleared: an api always reports a name, and the flag always carries one of its two
/// values.
const REST_API_UPDATED: &[Patchable] = &[
    ("ApiKeySourceType", "/apiKeySource", None),
    ("Description", "/description", Some(REMOVE)),
    (
        "DisableExecuteApiEndpoint",
        "/disableExecuteApiEndpoint",
        None,
    ),
    ("MinimumCompressionSize", "/minimumCompressionSize", Some(REMOVE)),
    ("Name", "/name", None),
    (REST_API_POLICY, "/policy", Some(REMOVE)),
];

/// The list-valued property of an api that same call writes one ENTRY at a time: "add" puts a
/// media type there and "remove" takes one away.
const REST_API_MEDIA_TYPES: (&str, &str) = ("BinaryMediaTypes", "/binaryMediaTypes");

/// The members of an `EndpointConfiguration` that call writes, as
/// `(member, pointer, keyed-by-entry)` — the two lists are written one entry at a time, and
/// the address type is a single value.
const ENDPOINT_MEMBERS: &[(&str, &str, bool)] = &[
    ("Types", "/endpointConfiguration/types", true),
    ("VpcEndpointIds", "/endpointConfiguration/vpcEndpointIds", true),
    ("IpAddressType", "/endpointConfiguration/ipAddressType", false),
];

/// Apply a changed `AWS::ApiGateway::RestApi` declaration to the api that exists.
///
/// The scalar configuration and the endpoint configuration are `UpdateRestApi` patches; a
/// changed DEFINITION is `PutRestApi`, the call that puts a definition on an api that already
/// has one, under the `Mode` the declaration states; and the tags are that service's own
/// tagging calls, which address the api by ARN. A definition the declaration holds in Amazon
/// S3 is put on the api the same way, from the document the object it addresses holds.
async fn update_rest_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, REST_API_PROPERTIES)?;
    let handled: Vec<&str> = REST_API_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .chain([
            REST_API_MEDIA_TYPES.0,
            "EndpointConfiguration",
            REST_API_BODY,
            REST_API_BODY_S3_LOCATION,
            REST_API_MODE,
            "Tags",
        ])
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    let Some(mut patches) = scalar_patches(&resource.properties, declared, REST_API_UPDATED)
    else {
        return Ok(None);
    };
    patches.extend(list_patches(
        &resource.properties,
        declared,
        REST_API_MEDIA_TYPES.0,
        REST_API_MEDIA_TYPES.1,
    ));
    let endpoint_before = stated_property(&resource.properties, "EndpointConfiguration");
    let endpoint_now = stated_property(declared, "EndpointConfiguration");
    for (member, pointer, keyed) in ENDPOINT_MEMBERS {
        let before = endpoint_before.and_then(|configuration| configuration.get(*member));
        let now = endpoint_now.and_then(|configuration| configuration.get(*member));
        if *keyed {
            patches.extend(entry_patches(before, now, pointer));
            continue;
        }
        match (before, now) {
            (before, Some(value)) if before != Some(value) => {
                patches.push(patch(REPLACE, pointer, &attribute_text(value)));
            }
            // An address type the declaration no longer states cannot be cleared: the member
            // takes one of its documented values, and that call publishes no `remove` of it.
            (Some(_), None) => return Ok(None),
            _ => {}
        }
    }
    let api_id = own_id(resource)?.to_string();
    // The definition a declaration holding it in Amazon S3 states, read from the object its
    // reference addresses. The read is made for a reference the declaration CHANGED, which is
    // the only change to such a definition a template states: the document is the object's,
    // and the api already has the one the stored reference addressed.
    let reference_before = stated_property(&resource.properties, REST_API_BODY_S3_LOCATION);
    let reference_now = stated_property(declared, REST_API_BODY_S3_LOCATION);
    let read = match reference_now.filter(|now| reference_before != Some(*now)) {
        Some(location) if stated_property(declared, REST_API_BODY).is_none() => Some(Value::String(
            super::super::apigateway_definitions::read_definition(ctx, region, location).await?,
        )),
        _ => None,
    };
    // Everything this change CANNOT express is decided BEFORE the first call: a declaration
    // that drops the definition takes nothing off the api (that call puts a document on one,
    // and there is none to put), so it is reported as not applied with the api untouched
    // rather than after half of the change has landed.
    let definition_before = stated_property(&resource.properties, REST_API_BODY);
    let definition_now = read
        .as_ref()
        .or_else(|| stated_property(declared, REST_API_BODY));
    // The declaration no longer states the definition it was deployed with: an inline one it
    // dropped, or a reference it dropped.
    let dropped_inline = definition_before.is_some();
    let dropped_reference = reference_before.is_some() && reference_now.is_none();
    if definition_now.is_none() && (dropped_inline || dropped_reference) {
        return Ok(None);
    }
    if !patches.is_empty() {
        let mut request = Map::new();
        request.insert("patchOperations".to_string(), Value::Array(patches));
        call_rest_json_named(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            UPDATE_REST_API,
            "PATCH",
            &rest_api_uri(&api_id),
            Some(&Value::Object(request)),
        )
        .await?;
    }
    // A CHANGED definition is put on the api with `PutRestApi`, "a feature of the API Gateway
    // control service for updating an existing API with an input of external API
    // definitions", whose `mode` is what the type's `Mode` property states — "the Mode
    // determines how API Gateway handles resource updates", whose documented default for an
    // api is to MERGE the definition into what the api already has.
    if let Some(definition) = definition_now.filter(|now| definition_before != Some(*now)) {
        let mode = stated_property(declared, REST_API_MODE)
            .map(attribute_text)
            .unwrap_or_else(|| DEFINITION_MERGE.to_string());
        call_rest_payload(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PUT",
            &format!(
                "{}?mode={}",
                rest_api_uri(&api_id),
                percent_encoded(&mode.to_ascii_lowercase())
            ),
            DEFINITION_MEDIA_TYPE,
            attribute_text(definition).into_bytes(),
        )
        .await?;
    }
    retag(
        ctx,
        region,
        &rest_api_arn(region, &api_id),
        &resource.properties,
        declared,
    )
    .await?;
    Ok(Some(Created::default()))
}

/// The `mode` a definition is put on an api under where the declaration states none: "the new
/// API definition is merged into the existing one", which is the documented default of the
/// type's `Mode` property.
const DEFINITION_MERGE: &str = "merge";

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Resource
// ---------------------------------------------------------------------------

/// The properties `UpdateResource` writes: where the resource hangs and the path segment it
/// adds, the two pointers that call publishes. Neither can be cleared — a resource always
/// reports both — so a declaration that drops one asks for a resource this call cannot make.
const RESOURCE_UPDATED: &[Patchable] = &[
    (RESOURCE_PARENT_ID, "/parentId", None),
    (RESOURCE_PATH_PART, "/pathPart", None),
];

/// Apply a changed `AWS::ApiGateway::Resource` declaration to the resource that exists: the
/// segment it adds and the resource it hangs off are what `UpdateResource` moves.
async fn update_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, RESOURCE_PROPERTIES)?;
    let handled: Vec<&str> = RESOURCE_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    let Some(patches) = scalar_patches(&resource.properties, declared, RESOURCE_UPDATED) else {
        return Ok(None);
    };
    patched(
        ctx,
        region,
        &format!(
            "{}/resources/{}",
            rest_api_uri(&required(resource, REST_API_ID)?),
            percent_encoded(own_id(resource)?)
        ),
        patches,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Method
// ---------------------------------------------------------------------------

/// The properties `UpdateMethod` writes, each on the pointer that call names it by.
///
/// Source: that operation's own patch surface — one pointer per updateable member of the
/// `Method` shape — against the type's property set. `AuthorizationType` cannot be cleared
/// (a method always reports how it is authorized) and neither can `ApiKeyRequired`, which
/// always carries one of its two values.
const METHOD_UPDATED: &[Patchable] = &[
    ("ApiKeyRequired", "/apiKeyRequired", None),
    ("AuthorizationType", "/authorizationType", None),
    ("AuthorizerId", "/authorizerId", Some(REMOVE)),
    ("OperationName", "/operationName", Some(REMOVE)),
    ("RequestValidatorId", "/requestValidatorId", Some(REMOVE)),
];

/// The collection-valued properties of a method that call writes one ENTRY at a time: the
/// scopes a list of values, the parameters and the models maps the caller keys.
const METHOD_SCOPES: (&str, &str) = ("AuthorizationScopes", "/authorizationScopes");
const METHOD_KEYED: &[(&str, &str)] = &[
    ("RequestModels", "/requestModels"),
    ("RequestParameters", "/requestParameters"),
];

/// Apply a changed `AWS::ApiGateway::Method` declaration to the method that exists.
///
/// The method's own members are `UpdateMethod` patches; its `Integration` and its
/// `MethodResponses` are sub-resources with puts of their own, and a put is what writes one
/// whether or not it is there already — so a changed integration, a changed integration
/// response and a changed method response are applied by the very calls the create made.
async fn update_method(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, METHOD_PROPERTIES)?;
    let handled: Vec<&str> = METHOD_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .chain(METHOD_KEYED.iter().map(|(property, _)| *property))
        .chain([METHOD_SCOPES.0, METHOD_INTEGRATION, METHOD_RESPONSES])
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    let Some(mut patches) = scalar_patches(&resource.properties, declared, METHOD_UPDATED) else {
        return Ok(None);
    };
    patches.extend(entry_patches(
        stated_property(&resource.properties, METHOD_SCOPES.0),
        stated_property(declared, METHOD_SCOPES.0),
        METHOD_SCOPES.1,
    ));
    for (property, pointer) in METHOD_KEYED {
        patches.extend(map_patches(
            &resource.properties,
            declared,
            property,
            pointer,
        ));
    }
    let uri = method_uri(resource)?;
    // Everything this change CANNOT express is decided BEFORE the first call, so a method is
    // never left half changed by a deployment that then reports the change as not applied: a
    // response the declaration no longer states cannot be taken off (a put is what writes
    // one), and neither can an integration it no longer states — a method always has one.
    let integration_before = stated_property(&resource.properties, METHOD_INTEGRATION);
    let integration_now = stated_property(declared, METHOD_INTEGRATION);
    if dropped_responses(&resource.properties, declared)
        || (integration_before.is_some() && integration_now.is_none())
    {
        return Ok(None);
    }
    if !patches.is_empty() {
        let mut request = Map::new();
        request.insert("patchOperations".to_string(), Value::Array(patches));
        call_rest_json_named(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            UPDATE_METHOD,
            "PATCH",
            &uri,
            Some(&Value::Object(request)),
        )
        .await?;
    }
    // The sub-resources, each put as the create puts it: a put is what writes one whether or
    // not it is there already, so a changed integration and a changed response are applied by
    // the very calls the create made.
    if let Some(integration) = integration_now.filter(|now| integration_before != Some(*now)) {
        put_integration(ctx, region, &uri, integration).await?;
    }
    let responses_before = stated_property(&resource.properties, METHOD_RESPONSES);
    let responses_now = stated_property(declared, METHOD_RESPONSES);
    if responses_before != responses_now {
        for response in entries(responses_now) {
            let status = required_member(APIGATEWAY_METHOD, METHOD_RESPONSES, response)?;
            refuse_unpublished_members(
                APIGATEWAY_METHOD,
                METHOD_RESPONSES,
                response,
                METHOD_RESPONSE_MEMBERS,
                &[STATUS_CODE],
            )?;
            let request = response_request(response, METHOD_RESPONSE_MEMBERS)?;
            call_rest_json_named(
                ctx,
                API_GATEWAY_SIGNING_NAME,
                region,
                PUT_METHOD_RESPONSE,
                "PUT",
                &format!("{uri}/responses/{}", percent_encoded(&status)),
                Some(&request),
            )
            .await?;
        }
    }
    Ok(Some(Created::default()))
}

/// Put the integration a changed declaration states behind the method, with the responses it
/// maps: the same three calls the create makes, each of which overwrites what is there.
async fn put_integration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    integration: &Value,
) -> Result<(), Refusal> {
    refuse_unpublished_members(
        APIGATEWAY_METHOD,
        METHOD_INTEGRATION,
        integration,
        INTEGRATION_MEMBERS,
        &[INTEGRATION_RESPONSES],
    )?;
    let mut request = Map::new();
    for (property, member) in INTEGRATION_MEMBERS {
        if let Some(value) = integration.get(*property).filter(|value| !value.is_null()) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    call_rest_json_named(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        PUT_INTEGRATION,
        "PUT",
        &format!("{uri}/integration"),
        Some(&Value::Object(request)),
    )
    .await?;
    for response in entries(integration.get(INTEGRATION_RESPONSES)) {
        let status = required_member(APIGATEWAY_METHOD, INTEGRATION_RESPONSES, response)?;
        refuse_unpublished_members(
            APIGATEWAY_METHOD,
            INTEGRATION_RESPONSES,
            response,
            INTEGRATION_RESPONSE_MEMBERS,
            &[STATUS_CODE],
        )?;
        let request = response_request(response, INTEGRATION_RESPONSE_MEMBERS)?;
        call_rest_json_named(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            PUT_INTEGRATION_RESPONSE,
            "PUT",
            &format!("{uri}/integration/responses/{}", percent_encoded(&status)),
            Some(&request),
        )
        .await?;
    }
    Ok(())
}

/// Whether the changed declaration states fewer responses than the record carries — a
/// response of the method itself or of its integration that the puts above would leave
/// behind.
fn dropped_responses(stored: &Value, declared: &Value) -> bool {
    let dropped = |before: Option<&Value>, now: Option<&Value>| {
        let held: Vec<String> = entries(before)
            .iter()
            .filter_map(|response| response.get(STATUS_CODE).map(attribute_text))
            .collect();
        let stated: Vec<String> = entries(now)
            .iter()
            .filter_map(|response| response.get(STATUS_CODE).map(attribute_text))
            .collect();
        held.iter().any(|status| !stated.contains(status))
    };
    if dropped(
        stated_property(stored, METHOD_RESPONSES),
        stated_property(declared, METHOD_RESPONSES),
    ) {
        return true;
    }
    dropped(
        stated_property(stored, METHOD_INTEGRATION)
            .and_then(|integration| integration.get(INTEGRATION_RESPONSES)),
        stated_property(declared, METHOD_INTEGRATION)
            .and_then(|integration| integration.get(INTEGRATION_RESPONSES)),
    )
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Deployment
// ---------------------------------------------------------------------------

/// The one property `UpdateDeployment` writes, on the one pointer that call publishes. The
/// pointer takes no `remove`, so a declaration that no longer states a description clears it
/// with a `replace` carrying no value.
const DEPLOYMENT_UPDATED: &[Patchable] = &[("Description", "/description", Some(REPLACE))];

/// Apply a changed `AWS::ApiGateway::Deployment` declaration to the deployment that exists.
///
/// A deployment is an immutable SNAPSHOT of the api's methods: its own description is the one
/// thing that call writes, so a changed `StageName`, `StageDescription` or
/// `DeploymentCanarySettings` asks for another deployment rather than a change to this one
/// and is reported as not applied.
async fn update_deployment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, DEPLOYMENT_PROPERTIES)?;
    let handled: Vec<&str> = DEPLOYMENT_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    let Some(patches) = scalar_patches(&resource.properties, declared, DEPLOYMENT_UPDATED) else {
        return Ok(None);
    };
    patched(
        ctx,
        region,
        &format!(
            "{}/deployments/{}",
            rest_api_uri(&required(resource, REST_API_ID)?),
            percent_encoded(own_id(resource)?)
        ),
        patches,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Stage
// ---------------------------------------------------------------------------

/// The properties `UpdateStage` writes, each on the pointer that call names it by.
///
/// Source: that operation's own patch surface (`/deploymentId`, `/description`,
/// `/cacheClusterEnabled`, `/cacheClusterSize`, `/clientCertificateId`,
/// `/documentationVersion`, `/tracingEnabled`, beside the collections and nested structures
/// below) against the type's property set. A stage always points at a deployment and always
/// reports its cache and tracing flags, so none of those three can be cleared.
const STAGE_UPDATED: &[Patchable] = &[
    ("CacheClusterEnabled", "/cacheClusterEnabled", None),
    ("CacheClusterSize", "/cacheClusterSize", Some(REMOVE)),
    ("ClientCertificateId", "/clientCertificateId", Some(REMOVE)),
    ("DeploymentId", "/deploymentId", None),
    ("Description", "/description", Some(REMOVE)),
    ("DocumentationVersion", "/documentationVersion", Some(REMOVE)),
    ("TracingEnabled", "/tracingEnabled", None),
];

/// The stage variables, which that call writes one ENTRY at a time
/// (`/variables/{variable_name}`).
const STAGE_VARIABLES: (&str, &str) = ("Variables", "/variables");

/// The members of a stage's `CanarySetting` that call writes, beside the pointer each one is
/// written on. `StageVariableOverrides` is the one keyed by name, so it is written one entry
/// at a time.
const CANARY_UPDATED: &[(&str, &str)] = &[
    ("DeploymentId", "/canarySettings/deploymentId"),
    ("PercentTraffic", "/canarySettings/percentTraffic"),
    ("UseStageCache", "/canarySettings/useStageCache"),
];
const CANARY_OVERRIDES: (&str, &str) = (
    "StageVariableOverrides",
    "/canarySettings/stageVariableOverrides",
);

/// The properties of a stage whose value this call can WRITE but not take away: the nested
/// settings a create applies as patches of their own ([`stage_patches`]) and the stage-wide
/// per-method settings. A declaration that no longer states one asks for a stage that call
/// cannot make, so such a change is reported as not applied.
///
/// Read off the tables the patches are built from, so a setting added to one of them is
/// covered here without a second list to keep in step.
const STAGE_DROPPABLE: &[&str] = &["AccessLogSetting", STAGE_METHOD_SETTINGS];

/// Apply a changed `AWS::ApiGateway::Stage` declaration to the stage that exists.
///
/// This is the call a REDEPLOYED api turns on: a template that deploys an api again declares
/// a new deployment and states it as the stage's `DeploymentId`, which is one `replace` of
/// `/deploymentId` — after which the stage serves the snapshot that deployment took.
async fn update_stage(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, STAGE_PROPERTIES)?;
    let handled: Vec<&str> = STAGE_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .chain(STAGE_PATCHED.iter().map(|(property, _)| *property))
        .chain(METHOD_SETTING_PATCHED.iter().map(|(property, _)| *property))
        .chain([
            STAGE_VARIABLES.0,
            "AccessLogSetting",
            "CanarySetting",
            STAGE_METHOD_SETTINGS,
            "Tags",
        ])
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    // A nested SETTING the declaration no longer states is a change this call cannot express:
    // the patches below are the `replace` ops a create applies, and that surface publishes no
    // pointer that takes a whole access-log setting, a per-method setting or the stage-wide
    // settings off again — so such a change is reported as not applied rather than as a
    // success that left the setting in force. Decided BEFORE any call, so the stage is left as
    // it is.
    for property in STAGE_DROPPABLE
        .iter()
        .copied()
        .chain(METHOD_SETTING_PATCHED.iter().map(|(property, _)| *property))
    {
        let before = stated_property(&resource.properties, property);
        let now = stated_property(declared, property);
        if before.is_some() && now.is_none() {
            return Ok(None);
        }
    }
    let Some(mut patches) = scalar_patches(&resource.properties, declared, STAGE_UPDATED) else {
        return Ok(None);
    };
    patches.extend(map_patches(
        &resource.properties,
        declared,
        STAGE_VARIABLES.0,
        STAGE_VARIABLES.1,
    ));
    // The settings a create could not carry — the access log settings and the per-method
    // settings — are the very patches a create applies, so they are rebuilt from the changed
    // declaration and re-applied: each is a `replace` of the pointer it is written on, which
    // writes the same value again where it did not change.
    patches.extend(stage_patches(declared, STAGE_MEMBERS, true)?);
    let canary_before = stated_property(&resource.properties, "CanarySetting");
    let canary_now = stated_property(declared, "CanarySetting");
    if canary_before != canary_now {
        // A canary the declaration no longer states cannot be taken off: that call writes
        // the canary's own members and publishes no pointer that removes the canary itself.
        let Some(canary) = canary_now else {
            return Ok(None);
        };
        for (member, pointer) in CANARY_UPDATED {
            match canary.get(*member).filter(|value| !value.is_null()) {
                Some(value) => patches.push(patch(REPLACE, pointer, &attribute_text(value))),
                None if canary_before
                    .and_then(|held| held.get(*member))
                    .is_some() =>
                {
                    return Ok(None)
                }
                None => {}
            }
        }
        patches.extend(map_patches(
            canary_before.unwrap_or(&Value::Null),
            canary,
            CANARY_OVERRIDES.0,
            CANARY_OVERRIDES.1,
        ));
    }
    let api_id = required(resource, REST_API_ID)?;
    let stage_name = own_id(resource)?.to_string();
    if !patches.is_empty() {
        patch_stage(ctx, region, &api_id, &stage_name, patches).await?;
    }
    retag(
        ctx,
        region,
        &stage_arn(region, &api_id, &stage_name),
        &resource.properties,
        declared,
    )
    .await?;
    Ok(Some(Created::default()))
}

/// The ARN API Gateway names a STAGE by, which is what its tagging calls address: the api's
/// own REST path with the stage's name under it, in the account-less form that model declares.
fn stage_arn(region: &str, api_id: &str, stage_name: &str) -> String {
    format!(
        "arn:aws:{API_GATEWAY_SIGNING_NAME}:{region}::/restapis/{api_id}/stages/{stage_name}"
    )
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::ClientCertificate
// ---------------------------------------------------------------------------

/// The one property `UpdateClientCertificate` writes, on the pointer that call names it by.
///
/// Source: that operation's own patch surface — the member of the `ClientCertificate` shape a
/// patch may name — against the type's property set. The material API Gateway issued and the
/// window it is valid over are no part of a declaration, and the identifier is read-only, so
/// the description is the whole of it; it is cleared with `remove`, the op a member that may be
/// absent takes.
const CLIENT_CERTIFICATE_UPDATED: &[Patchable] = &[("Description", "/description", Some(REMOVE))];

/// Apply a changed `AWS::ApiGateway::ClientCertificate` declaration to the certificate that
/// exists: the description is `UpdateClientCertificate`'s one patch, and the tags are that
/// service's own tagging calls, which address the certificate by ARN.
///
/// The certificate's MATERIAL is untouched by either call — a changed declaration re-describes
/// the certificate the stack minted rather than issuing another one — so the identifier the
/// stack published goes on naming the same record.
async fn update_client_certificate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(
        &resource.resource_type,
        declared,
        CLIENT_CERTIFICATE_PROPERTIES,
    )?;
    let handled: Vec<&str> = CLIENT_CERTIFICATE_UPDATED
        .iter()
        .map(|(property, _, _)| *property)
        .chain(["Tags"])
        .collect();
    if changed_elsewhere(&resource.properties, declared, &handled) {
        return Ok(None);
    }
    let Some(patches) =
        scalar_patches(&resource.properties, declared, CLIENT_CERTIFICATE_UPDATED)
    else {
        return Ok(None);
    };
    let client_certificate_id = own_id(resource)?.to_string();
    if !patches.is_empty() {
        patched(
            ctx,
            region,
            &client_certificate_uri(&client_certificate_id),
            patches,
        )
        .await?;
    }
    retag(
        ctx,
        region,
        &client_certificate_arn(region, &client_certificate_id),
        &resource.properties,
        declared,
    )
    .await?;
    Ok(Some(Created::default()))
}

/// The ARN API Gateway names a CLIENT CERTIFICATE by, which is what its tagging calls address:
/// that service's own REST path for the certificate, in the account-less form its model
/// declares (`arn:aws:apigateway:{region}::/clientcertificates/{id}`).
fn client_certificate_arn(region: &str, client_certificate_id: &str) -> String {
    format!(
        "arn:aws:{API_GATEWAY_SIGNING_NAME}:{region}::/clientcertificates/\
         {client_certificate_id}"
    )
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Send one patch set to a record that exists, and report the change as applied. An empty set
/// is no call at all: the declaration states exactly what the record already carries.
async fn patched(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    patches: Vec<Value>,
) -> Result<Option<Created>, Refusal> {
    if patches.is_empty() {
        return Ok(Some(Created::default()));
    }
    let mut request = Map::new();
    request.insert("patchOperations".to_string(), Value::Array(patches));
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PATCH",
        uri,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::default()))
}

/// The patches that take the single-valued properties of a record from the declaration it was
/// deployed with to `declared`: one `replace` per property whose stated value CHANGED, and the
/// clearing op for one the new declaration no longer states.
///
/// `None` where a property that cannot be cleared was dropped: the member always carries a
/// value, so there is no patch that states what the declaration now asks for.
fn scalar_patches(
    stored: &Value,
    declared: &Value,
    table: &[Patchable],
) -> Option<Vec<Value>> {
    let mut patches = Vec::new();
    for (property, pointer, clearing) in table {
        let before = stated_property(stored, property);
        let now = stated_property(declared, property);
        if before == now {
            continue;
        }
        match now {
            Some(value) => patches.push(patch(REPLACE, pointer, &attribute_text(value))),
            None => patches.push(patch(clearing.as_ref()?, pointer, "")),
        }
    }
    Some(patches)
}

/// The patches that bring a collection the caller KEYS into step: a `replace` for every key
/// the changed declaration states differently, and a `remove` for every key it no longer
/// states.
///
/// The key is written into the pointer as a JSON Pointer requires, so a key carrying `/` or
/// `~` — a content type, a mapping expression — addresses the entry it names rather than a
/// path into it.
fn map_patches(
    stored: &Value,
    declared: &Value,
    property: &str,
    pointer: &str,
) -> Vec<Value> {
    let empty = Map::new();
    let before = stated_property(stored, property)
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let now = stated_property(declared, property)
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut patches = Vec::new();
    for (key, value) in now {
        if before.get(key) == Some(value) {
            continue;
        }
        patches.push(patch(
            REPLACE,
            &format!(
                "{pointer}/{}",
                ::service_common::json_pointer::escaped_segment(key)
            ),
            &attribute_text(value),
        ));
    }
    for key in before.keys() {
        if now.contains_key(key) {
            continue;
        }
        patches.push(patch(
            REMOVE,
            &format!(
                "{pointer}/{}",
                ::service_common::json_pointer::escaped_segment(key)
            ),
            "",
        ));
    }
    patches
}

/// The patches that bring a LIST the service holds as a set into step: an `add` for every
/// entry the changed declaration states anew and a `remove` for every entry it no longer
/// states, which are the two ops that call writes one entry of a collection with.
fn list_patches(
    stored: &Value,
    declared: &Value,
    property: &str,
    pointer: &str,
) -> Vec<Value> {
    entry_patches(
        stated_property(stored, property),
        stated_property(declared, property),
        pointer,
    )
}

/// [`list_patches`] over two already-read declarations, for a list nested inside a property.
fn entry_patches(before: Option<&Value>, now: Option<&Value>, pointer: &str) -> Vec<Value> {
    let held = text_list(before);
    let stated = text_list(now);
    let mut patches = Vec::new();
    for entry in &stated {
        if !held.contains(entry) {
            patches.push(patch(ADD, pointer, entry));
        }
    }
    for entry in &held {
        if !stated.contains(entry) {
            patches.push(patch(REMOVE, pointer, entry));
        }
    }
    patches
}

/// Whether the changed declaration states a property DIFFERENTLY that no call here writes —
/// one fixed when the resource was created, or one whose change asks for a different resource
/// altogether.
///
/// Read over both declarations, so a property the new one adds and one it drops are each a
/// change: a deployment reported as applied has to have applied every property the template
/// states.
fn changed_elsewhere(stored: &Value, declared: &Value, handled: &[&str]) -> bool {
    let keys = |properties: &Value| -> Vec<String> {
        properties
            .as_object()
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut properties = keys(stored);
    properties.extend(keys(declared));
    properties.sort();
    properties.dedup();
    properties.into_iter().any(|property| {
        !handled.contains(&property.as_str())
            && stated_property(stored, &property) != stated_property(declared, &property)
    })
}

/// One property of a declaration, or `None` where it states none — a property declared `null`
/// states nothing, exactly as [`stated`] reads one off a record.
fn stated_property<'d>(properties: &'d Value, property: &str) -> Option<&'d Value> {
    properties
        .get(property)
        .filter(|value| !value.is_null())
    }

/// Bring the TAGS of a record whose declaration states them into step, through the tagging
/// calls API Gateway publishes for a resource of its own: the tags the changed declaration
/// states are put with `TagResource`, and the keys it no longer states are taken off with
/// `UntagResource`.
async fn retag(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_arn: &str,
    stored: &Value,
    declared: &Value,
) -> Result<(), Refusal> {
    let held = gateway_tags(stored, &[]);
    let stated = gateway_tags(declared, &[]);
    if held == stated {
        return Ok(());
    }
    let dropped: Vec<String> = held
        .keys()
        .filter(|key| !stated.contains_key(*key))
        .cloned()
        .collect();
    if !stated.is_empty() {
        let mut request = Map::new();
        request.insert("tags".to_string(), Value::Object(stated));
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PUT",
            &format!("/tags/{}", percent_encoded(resource_arn)),
            Some(&Value::Object(request)),
        )
        .await?;
    }
    if !dropped.is_empty() {
        let keys: Vec<String> = dropped
            .iter()
            .map(|key| format!("tagKeys={}", percent_encoded(key)))
            .collect();
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "DELETE",
            &format!(
                "/tags/{}?{}",
                percent_encoded(resource_arn),
                keys.join("&")
            ),
            None,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// One row of the table below: the case, the declaration the record was deployed with,
    /// the one a deployment now states, and the patches that change asks for — `None` for a
    /// change this module reports as not applied.
    type Row = (
        &'static str,
        Value,
        Value,
        Option<Vec<(&'static str, &'static str, &'static str)>>,
    );

    /// The patches a changed declaration becomes, over the forms a change takes: a value
    /// replaced, one stated anew, one dropped from a pointer that can be cleared and one
    /// dropped from a pointer that cannot.
    #[test]
    fn a_changed_declaration_becomes_the_patches_the_update_takes() {
        let rows: &[Row] = &[
            (
                "a stage re-pointed at another deployment",
                json!({ "DeploymentId": "d1" }),
                json!({ "DeploymentId": "d2" }),
                Some(vec![("replace", "/deploymentId", "d2")]),
            ),
            (
                "a description stated anew",
                json!({ "DeploymentId": "d1" }),
                json!({ "DeploymentId": "d1", "Description": "live" }),
                Some(vec![("replace", "/description", "live")]),
            ),
            (
                "a description the declaration no longer states",
                json!({ "DeploymentId": "d1", "Description": "live" }),
                json!({ "DeploymentId": "d1" }),
                Some(vec![("remove", "/description", "")]),
            ),
            (
                "nothing changed at all",
                json!({ "DeploymentId": "d1", "Description": "live" }),
                json!({ "DeploymentId": "d1", "Description": "live" }),
                Some(Vec::new()),
            ),
            (
                "a deployment id the declaration drops, which cannot be cleared",
                json!({ "DeploymentId": "d1" }),
                json!({}),
                None,
            ),
        ];
        for (case, stored, declared, expected) in rows {
            let patches = scalar_patches(stored, declared, STAGE_UPDATED);
            let read: Option<Vec<(String, String, String)>> = patches.as_ref().map(|patches| {
                patches
                    .iter()
                    .map(|patch| {
                        let text = |member: &str| {
                            patch
                                .get(member)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (text("op"), text("path"), text("value"))
                    })
                    .collect()
            });
            let wanted: Option<Vec<(String, String, String)>> = expected.as_ref().map(|rows| {
                rows.iter()
                    .map(|(op, path, value)| {
                        ((*op).to_string(), (*path).to_string(), (*value).to_string())
                    })
                    .collect()
            });
            assert_eq!(
                read, wanted,
                "case {case:?}: expected the patches {wanted:?}, got {read:?}"
            );
        }
    }

    /// A collection the caller keys is brought into step one entry at a time, with the key
    /// written into the pointer as a JSON Pointer requires.
    #[test]
    fn a_keyed_collection_is_patched_one_entry_at_a_time() {
        let stored = json!({ "Variables": { "Backend": "primary", "Gone": "x" } });
        let declared = json!({ "Variables": { "Backend": "standby", "Added": "y" } });
        let patches = map_patches(&stored, &declared, "Variables", "/variables");
        let mut read: Vec<(String, String, String)> = patches
            .iter()
            .map(|patch| {
                let text = |member: &str| {
                    patch
                        .get(member)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                (text("op"), text("path"), text("value"))
            })
            .collect();
        // Compared as a SET: which patches the change asks for is the rule, and the order a
        // map's entries are walked in is the document's.
        read.sort();
        let mut wanted = vec![
            ("remove".to_string(), "/variables/Gone".to_string(), String::new()),
            ("replace".to_string(), "/variables/Added".to_string(), "y".to_string()),
            (
                "replace".to_string(),
                "/variables/Backend".to_string(),
                "standby".to_string(),
            ),
        ];
        wanted.sort();
        assert_eq!(
            read, wanted,
            "case \"changed, added and dropped variables\": expected one patch per entry, got \
             {read:?}"
        );
        let keyed = map_patches(
            &json!({ "RequestModels": {} }),
            &json!({ "RequestModels": { "application/json": "Model" } }),
            "RequestModels",
            "/requestModels",
        );
        assert_eq!(
            keyed
                .first()
                .and_then(|patch| patch.get("path"))
                .and_then(Value::as_str),
            Some("/requestModels/application~1json"),
            "case \"a content-type key\": expected the key escaped as a JSON Pointer requires, \
             got {keyed:?}"
        );
    }

    /// A property no table here carries is a change this module does not apply, whichever
    /// side states it — so a resource is never reported changed with less than the
    /// declaration asked for.
    #[test]
    fn a_property_no_pointer_writes_is_reported_as_not_applied() {
        let rows: &[(&str, Value, Value, bool)] = &[
            (
                "the stage renamed, which is its identity",
                json!({ "StageName": "live", "DeploymentId": "d1" }),
                json!({ "StageName": "beta", "DeploymentId": "d1" }),
                true,
            ),
            (
                "only the deployment re-pointed",
                json!({ "StageName": "live", "DeploymentId": "d1" }),
                json!({ "StageName": "live", "DeploymentId": "d2" }),
                false,
            ),
            (
                "a property stated null on both sides",
                json!({ "StageName": "live", "Description": Value::Null }),
                json!({ "StageName": "live" }),
                false,
            ),
        ];
        let handled: Vec<&str> = STAGE_UPDATED
            .iter()
            .map(|(property, _, _)| *property)
            .collect();
        for (case, stored, declared, expected) in rows {
            let elsewhere = changed_elsewhere(stored, declared, &handled);
            assert_eq!(
                elsewhere, *expected,
                "case {case:?}: expected changed_elsewhere to be {expected}, got {elsewhere}"
            );
        }
    }
}
