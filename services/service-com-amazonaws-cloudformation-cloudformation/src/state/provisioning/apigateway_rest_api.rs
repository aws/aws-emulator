// SPDX-License-Identifier: CC0-1.0

//! The `AWS::ApiGateway::*` types of the REST control plane this emulator creates in Amazon
//! API Gateway itself: the five a REST API is built out of — the api, its resources, its
//! methods, a deployment of them and the stage that serves it — in [`access_control`], the
//! four that govern who reaches that stage — the authorizer that decides a request, the api
//! key a caller presents, the usage plan that meters a key and the association that puts a
//! key in a plan — and, in [`vpc_link`], the VPC link a private integration reaches a VPC
//! through.
//!
//! This is the per-type half of provisioning for those types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records —
//! that file is far past the length a change to this repository may touch — and is reached
//! through the same seam ([`super::Owner`]).
//!
//! ## Why they are really created
//!
//! A REST API is reached on its own DATA PLANE: a client invokes a deployed stage at
//! `{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}`, and what answers there is the
//! integration behind the method the request's path and verb resolve to. API Gateway is the
//! service that holds all of it — the api, the resource tree, each method's integration, the
//! deployment snapshot a stage points at — so a stack whose api exists nowhere publishes an
//! invoke URL nothing serves, however complete the stack's own records look. The id is also
//! that service's to assign (`CreateRestApi` takes no identifier and answers the `id` it
//! chose, and "Ref returns the RestApi ID"), so the only way a stack can publish a value a
//! client can call API Gateway — or the stage — with is for the api to exist there and for
//! that create's own answer to be the resource's physical id
//! ([`super::super::template::identity_is_service_assigned`] names the types whose identity
//! that is, so every reference to one is re-pointed at it once its create has answered).
//!
//! ## The calls
//!
//! Each type is the call that owning service publishes for it, on the URI that operation's
//! own `@http` binding declares:
//!
//! * `AWS::ApiGateway::RestApi` is `CreateRestApi` (`POST /restapis`) — or, where the
//!   declaration carries an OpenAPI definition in `Body`, `ImportRestApi`
//!   (`POST /restapis?mode=import`), the call that reads a definition: the document IS the
//!   api, so its paths become the api's resources and each operation's integration extension
//!   the method's integration. The properties that call cannot carry are then applied with
//!   `UpdateRestApi`, the patch call API Gateway publishes for an api that exists
//!   ([`REST_API_PATCHED`]), and the tags with `TagResource`, since neither the import nor
//!   the patch takes a tag;
//! * `AWS::ApiGateway::Resource` is `CreateResource`
//!   (`POST /restapis/{restApiId}/resources/{parentId}`);
//! * `AWS::ApiGateway::Method` is `PutMethod` plus the calls its nested properties are:
//!   `PutIntegration` for its `Integration`, `PutIntegrationResponse` for each of that
//!   integration's responses and `PutMethodResponse` for each `MethodResponses` entry —
//!   API Gateway holds a method's integration and its responses as resources of their own,
//!   each with a put of its own;
//! * `AWS::ApiGateway::Deployment` is `CreateDeployment`
//!   (`POST /restapis/{restApiId}/deployments`), whose `StageDescription` configures the
//!   stage that call creates: the members it takes travel in it, and the rest are applied to
//!   that stage with `UpdateStage` ([`stage_patches`]);
//! * `AWS::ApiGateway::Stage` is `CreateStage` (`POST /restapis/{restApiId}/stages`), with
//!   the same patch tail for the settings that create does not take;
//! * `AWS::ApiGateway::VpcLink` is `CreateVpcLink` (`POST /vpclinks`), the one create here
//!   that answers a resource still being provisioned ([`vpc_link`]).
//!
//! A declared property no call here carries is REFUSED rather than dropped: an api reported
//! created with less than its declaration asked for is the shallow success these arms exist
//! to avoid.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    applicable_tags, call_rest_json, call_rest_payload, percent_encoded, Created, Readiness,
    Refusal,
};
use crate::state::provisioner::{
    api_gateway_answered, api_gateway_deleted, attribute_text, lower_camel_members, tags_as_map,
    tags_of, API_GATEWAY_NOT_FOUND, API_GATEWAY_SIGNING_NAME,
};
use crate::state::template::{
    APIGATEWAY_ACCOUNT, APIGATEWAY_DEPLOYMENT, APIGATEWAY_RESOURCE, APIGATEWAY_REST_API,
    APIGATEWAY_VPC_LINK,
};
use crate::state::StoredResource;

/// The four types that govern ACCESS to a REST API rather than making one up, in a module of
/// their own so that neither file grows past the length a change to this repository may
/// touch. They are reached through the same [`Kind`] every arm here is.
mod access_control;

/// The VPC link a private integration reaches a VPC through, in a module of its own for the
/// same reason: it is the one type here whose create leaves the resource still coming up, so
/// its arms are the only ones that read a status back. Reached through the same [`Kind`].
mod vpc_link;

/// The ACCOUNT-level settings of this control plane, in a module of its own for the same
/// reason: they are the one record here that is a SINGLETON rather than a resource either side
/// names, so its arms are the only ones whose calls carry no identifier at all. Reached
/// through the same [`Kind`].
mod account;

/// The two resource types this module provisions that the template module does not name: it
/// names the three whose `Ref` and `Fn::GetAtt` return an identifier API Gateway assigns
/// ([`APIGATEWAY_REST_API`], [`APIGATEWAY_RESOURCE`], [`APIGATEWAY_DEPLOYMENT`]), while a
/// stage's `Ref` is the name its own declaration states and a method publishes no return
/// value of its own.
const APIGATEWAY_METHOD: &str = "AWS::ApiGateway::Method";
const APIGATEWAY_STAGE: &str = "AWS::ApiGateway::Stage";

// The error code API Gateway answers for a resource it does not hold
// (`API_GATEWAY_NOT_FOUND`) is what says a delete has nothing left to remove, and its signing
// name (`API_GATEWAY_SIGNING_NAME`) is what routes every call below to its own handlers; both
// come from the module that already states them for this service's other control plane. Each
// call rides `call_rest_json` on the URI its own operation's `@http` binding declares, since
// that model speaks `restJson1`.

/// The code a refusal of this module's own carries for a declaration it cannot act on.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// The media type an OpenAPI definition is imported as: `ImportRestApi`'s `body` is an
/// `@httpPayload` blob, so the document travels as the request's whole body rather than
/// inside a JSON envelope.
const DEFINITION_MEDIA_TYPE: &str = "application/json";

/// Which of these types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ApiGateway::RestApi`.
    RestApi,
    /// `AWS::ApiGateway::Resource`.
    Resource,
    /// `AWS::ApiGateway::Method`.
    Method,
    /// `AWS::ApiGateway::Deployment`.
    Deployment,
    /// `AWS::ApiGateway::Stage`.
    Stage,
    /// `AWS::ApiGateway::VpcLink`, whose arms are [`vpc_link`]'s.
    VpcLink,
    /// `AWS::ApiGateway::Account`, whose arms are [`account`]'s: the account-level settings
    /// of this control plane rather than a resource of it.
    Account,
    /// One of the four types that govern ACCESS to a REST API, whose arms are
    /// [`access_control`]'s. Which of them it is travels with the kind, as the kind itself
    /// travels with [`super::Owner`].
    AccessControl(access_control::Kind),
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    let kind = match resource_type {
        APIGATEWAY_REST_API => Kind::RestApi,
        APIGATEWAY_RESOURCE => Kind::Resource,
        APIGATEWAY_METHOD => Kind::Method,
        APIGATEWAY_DEPLOYMENT => Kind::Deployment,
        APIGATEWAY_STAGE => Kind::Stage,
        APIGATEWAY_VPC_LINK => Kind::VpcLink,
        APIGATEWAY_ACCOUNT => Kind::Account,
        _ => return access_control::kind_of(resource_type).map(Kind::AccessControl),
    };
    Some(kind)
}

/// The property naming the api a resource, a method, a deployment or a stage belongs to,
/// which addresses the call rather than travelling in its body.
const REST_API_ID: &str = "RestApiId";

/// Create one of these resources in API Gateway, so a client that deployed the stack reads
/// it back through that service's own read — and so the stage the stack published really
/// serves the methods the template declared.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::RestApi => create_rest_api(ctx, region, resource, propagated).await,
        Kind::Resource => create_resource(ctx, region, resource).await,
        Kind::Method => put_method(ctx, region, resource).await,
        Kind::Deployment => create_deployment(ctx, region, resource).await,
        Kind::Stage => create_stage(ctx, region, resource, propagated).await,
        Kind::VpcLink => vpc_link::create(ctx, region, resource, propagated).await,
        // The account settings carry no tags: API Gateway's tagging calls address a resource
        // ARN, and the account is no resource of that service, so there is no call that could
        // carry one.
        Kind::Account => account::create(ctx, region, resource).await,
        Kind::AccessControl(kind) => {
            access_control::create(ctx, region, kind, resource, propagated).await
        }
    }
}

/// Whether API Gateway reports one of these resources ready for a client to use — see
/// [`super::Owner::readiness`].
///
/// Every type here but one EXISTS from the moment its create answers: that service reports no
/// state an api, a resource, a method, a deployment, a stage, an authorizer, a key, a plan or
/// an association is still coming up in. A VPC LINK is the exception — creating one is "an
/// asynchronous operation" whose answer carries the status the link is still in — so that arm
/// reads the status back ([`vpc_link::readiness`]).
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    match kind {
        Kind::VpcLink => vpc_link::readiness(ctx, region, physical_resource_id).await,
        Kind::RestApi
        | Kind::Resource
        | Kind::Method
        | Kind::Deployment
        | Kind::Stage
        | Kind::Account
        | Kind::AccessControl(_) => Ok(Readiness::Ready),
    }
}

/// Apply a changed declaration to one of these records that already exists — see
/// [`super::Owner::update`].
///
/// A changed declaration of an api, a resource, a method, a deployment, a stage or one of the
/// four types that govern access to a stage is applied by no arm here yet: API Gateway changes
/// those through its own PATCH calls, which this module makes only for the settings a create
/// could not carry — so such a change is reported as NOT applied (`Ok(None)`) rather than as a
/// success that dropped the caller's change. The ACCOUNT settings are the one record here
/// whose only call IS that patch, so a changed declaration of them is applied
/// ([`account::update`]).
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    match kind {
        Kind::Account => account::update(ctx, region, resource, declared).await,
        Kind::RestApi
        | Kind::Resource
        | Kind::Method
        | Kind::Deployment
        | Kind::Stage
        | Kind::VpcLink
        | Kind::AccessControl(_) => Ok(None),
    }
}

/// Delete one of these resources from API Gateway, addressed exactly as its own read is.
///
/// One that service no longer holds (`NotFoundException`, judged on the CODE and never on a
/// message, which carries the request's own identifier) is already in the state the delete
/// asks for — which is also what deleting the API that held it leaves behind, since an api
/// owns its resources, methods, deployments and stages. The delete itself is the one both
/// API Gateway control planes share ([`api_gateway_deleted`]).
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = match kind {
        Kind::RestApi => rest_api_uri(&resource.physical_resource_id),
        Kind::Resource => format!(
            "{}/resources/{}",
            rest_api_uri(&required(resource, REST_API_ID)?),
            percent_encoded(&resource.physical_resource_id)
        ),
        Kind::Method => method_uri(resource)?,
        Kind::Deployment => format!(
            "{}/deployments/{}",
            rest_api_uri(&required(resource, REST_API_ID)?),
            percent_encoded(&resource.physical_resource_id)
        ),
        Kind::Stage => stage_uri(
            &required(resource, REST_API_ID)?,
            &resource.physical_resource_id,
        ),
        Kind::VpcLink => vpc_link::vpc_link_uri(own_id(resource)?),
        // The account settings are taken away by no DELETE at all: that service publishes
        // none for a singleton, so the setting this resource put there is CLEARED through the
        // one call it has ([`account::delete`]).
        Kind::Account => return account::delete(ctx, region, resource).await,
        Kind::AccessControl(kind) => {
            return access_control::delete(ctx, region, kind, resource).await
        }
    };
    api_gateway_deleted(ctx, region, &uri).await
}

/// The configuration one of these resources has as API Gateway reports it, for drift
/// detection: that resource's own read, under this type's property names
/// ([`REPORTED_PROPERTIES`]). One that service no longer holds (`NotFoundException`) was
/// deleted.
///
/// A METHOD reports nothing here: its own read answers the integration and the responses as
/// nested records, which a declaration states in a different shape, so the check is reported
/// as one this emulator does not make rather than a difference nobody made.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (uri, reported) = match kind {
        Kind::RestApi => (
            rest_api_uri(&resource.physical_resource_id),
            REST_API_REPORTED,
        ),
        Kind::Resource => (
            format!(
                "{}/resources/{}",
                rest_api_uri(&required(resource, REST_API_ID)?),
                percent_encoded(&resource.physical_resource_id)
            ),
            RESOURCE_REPORTED,
        ),
        Kind::Deployment => (
            format!(
                "{}/deployments/{}",
                rest_api_uri(&required(resource, REST_API_ID)?),
                percent_encoded(&resource.physical_resource_id)
            ),
            DEPLOYMENT_REPORTED,
        ),
        Kind::Stage => (
            stage_uri(
                &required(resource, REST_API_ID)?,
                &resource.physical_resource_id,
            ),
            STAGE_REPORTED,
        ),
        // A METHOD's own read answers its integration and its responses as nested records,
        // which a declaration states in a different shape, so nothing of it is COMPARED.
        // That is reported as a check this emulator does not make — `Ok(None)` would say
        // the resource is gone, which is a status nothing established.
        Kind::Method => {
            return Err(Refusal {
                code: NOT_IMPLEMENTED.to_string(),
                message: format!(
                    "this emulator does not read the configuration of a {APIGATEWAY_METHOD} \
                     back for drift detection"
                ),
            })
        }
        Kind::VpcLink => (
            vpc_link::vpc_link_uri(own_id(resource)?),
            vpc_link::VPC_LINK_REPORTED,
        ),
        // The account settings are read by a call carrying no identifier, so that arm reads
        // them itself rather than through the address a resource of this control plane has.
        Kind::Account => return account::actual_configuration(ctx, region).await,
        Kind::AccessControl(kind) => {
            return access_control::actual_configuration(ctx, region, kind, resource).await
        }
    };
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

/// The properties API Gateway's own read of an api reports, as `(property, member)` pairs.
///
/// The SCALAR configuration alone: `Tags` are left out because the tags a resource carries
/// change independently of its declaration, the structured properties are left out because
/// the read spells their members in the model's own case while a declaration spells them in
/// CloudFormation's, the definition is left out because the read reports the api's resource
/// tree rather than the document it was imported from, and the resource POLICY is left out
/// because a template declares it as an object while the read answers its text — a property
/// reported in a different SHAPE would be evidence of a change nobody made.
const REST_API_REPORTED: &[(&str, &str)] = &[
    ("ApiKeySourceType", "apiKeySource"),
    ("BinaryMediaTypes", "binaryMediaTypes"),
    ("Description", "description"),
    ("DisableExecuteApiEndpoint", "disableExecuteApiEndpoint"),
    ("MinimumCompressionSize", "minimumCompressionSize"),
    ("Name", "name"),
];

/// The properties a resource's own read reports: its place in the tree, which is every
/// property the type publishes beside the api that holds it.
const RESOURCE_REPORTED: &[(&str, &str)] = &[("ParentId", "parentId"), ("PathPart", "pathPart")];

/// The properties a deployment's own read reports. A deployment is a SNAPSHOT — its
/// `StageDescription` configures the stage it created, not the deployment — so its
/// description is all it reports of its declaration.
const DEPLOYMENT_REPORTED: &[(&str, &str)] = &[("Description", "description")];

/// The properties a stage's own read reports, under this type's property names.
const STAGE_REPORTED: &[(&str, &str)] = &[
    ("CacheClusterEnabled", "cacheClusterEnabled"),
    ("CacheClusterSize", "cacheClusterSize"),
    ("ClientCertificateId", "clientCertificateId"),
    ("DeploymentId", "deploymentId"),
    ("Description", "description"),
    ("DocumentationVersion", "documentationVersion"),
    ("StageName", "stageName"),
    ("TracingEnabled", "tracingEnabled"),
    ("Variables", "variables"),
];

// ---------------------------------------------------------------------------
// AWS::ApiGateway::RestApi
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::RestApi` publishes. A declared property this list does
/// not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `CreateRestApi`'s and `ImportRestApi`'s
/// members (`smithy_query(query="operation",operation="CreateRestApi,ImportRestApi")`).
const REST_API_PROPERTIES: &[&str] = &[
    "ApiKeySourceType",
    "BinaryMediaTypes",
    REST_API_BODY,
    "BodyS3Location",
    "CloneFrom",
    "Description",
    "DisableExecuteApiEndpoint",
    "EndpointConfiguration",
    FAIL_ON_WARNINGS.0,
    "MinimumCompressionSize",
    REST_API_MODE,
    "Name",
    "Parameters",
    "Policy",
    "Tags",
];

/// The property carrying the OpenAPI definition the api is IMPORTED from, inline.
const REST_API_BODY: &str = "Body";

/// The property carrying that definition as an Amazon S3 OBJECT instead. The object has to
/// be FETCHED from S3 before any api can be imported from it, which nothing here does, so a
/// declaration stating it is refused rather than created without its definition.
const REST_API_BODY_S3_LOCATION: &str = "BodyS3Location";

/// The property that governs how a CHANGED definition is applied to an api that already has
/// one ("overwrite" / "merge"), which is `PutRestApi`'s own `mode`. A create has no
/// definition to merge into, so it is read by the update path alone and carried by no call
/// here — as are the import knobs `Parameters` and `FailOnWarnings` for a declaration that
/// carries no definition to import, which have nothing to govern either.
const REST_API_MODE: &str = "Mode";

/// The property that names the api an import CLONES. `CreateRestApi` takes it as
/// `cloneFrom`, and an import takes no such member — the document is the api — so a
/// declaration stating both is refused rather than importing one and silently not cloning
/// the other.
const REST_API_CLONE_FROM: &str = "CloneFrom";

/// The properties that ARE `CreateRestApi`'s own body members, as `(property, member)`
/// pairs. Every one of them is the property with a lower-cased initial except the key
/// source, which the type publishes as `ApiKeySourceType` and that call takes as
/// `apiKeySource`.
const REST_API_MEMBERS: &[(&str, &str)] = &[
    ("ApiKeySourceType", "apiKeySource"),
    ("BinaryMediaTypes", "binaryMediaTypes"),
    (REST_API_CLONE_FROM, "cloneFrom"),
    ("Description", "description"),
    ("DisableExecuteApiEndpoint", "disableExecuteApiEndpoint"),
    ("EndpointConfiguration", "endpointConfiguration"),
    ("MinimumCompressionSize", "minimumCompressionSize"),
    ("Name", "name"),
];

/// The property whose member takes the TEXT of a document a template declares as an object:
/// the keys inside a resource policy are the policy language's own, not modelled member
/// names.
const REST_API_POLICY: &str = "Policy";

/// The properties an IMPORT cannot carry, beside the patch path `UpdateRestApi` applies each
/// of them on — the call API Gateway publishes for an api that exists.
///
/// `ImportRestApi` takes the definition, the import `parameters` and `failOnWarnings` and
/// nothing else, so an api imported from a document is brought to the configuration its
/// declaration states by patching it, which is what AWS's own provider does.
const REST_API_PATCHED: &[(&str, &str)] = &[
    ("ApiKeySourceType", "/apiKeySource"),
    ("Description", "/description"),
    ("DisableExecuteApiEndpoint", "/disableExecuteApiEndpoint"),
    ("MinimumCompressionSize", "/minimumCompressionSize"),
    ("Name", "/name"),
    (REST_API_POLICY, "/policy"),
];

/// The import's own query parameters: `failOnWarnings` ("rollback the API creation when a
/// warning is encountered") under the name that operation's `@httpQuery` trait gives it.
const FAIL_ON_WARNINGS: (&str, &str) = ("FailOnWarnings", "failonwarnings");

/// The import parameter the endpoint TYPES travel as: "to configure the endpoint type, set
/// parameters as endpointConfigurationTypes=EDGE, endpointConfigurationTypes=REGIONAL, or
/// endpointConfigurationTypes=PRIVATE" (`ImportRestApiRequest$parameters`).
const ENDPOINT_TYPES_PARAMETER: &str = "endpointConfigurationTypes";

/// Create the api an `AWS::ApiGateway::RestApi` declares, so a client that deployed the
/// stack and then calls `apigateway GetRestApi` with the id the stack publishes reads the
/// api back — and so the stage deployed from it is served on the execute-api data plane —
/// rather than a `NotFoundException` for an api nothing created.
///
/// A declaration carrying a `Body` is IMPORTED from that definition
/// ([`import_rest_api`]), which is the call that creates an api WITH its resources, its
/// methods and their integrations; one that carries none is `CreateRestApi` with the
/// declared properties as that call's own members and the tags CloudFormation gives the
/// resource as its tag map.
///
/// "Ref returns the RestApi ID", which only that service produces, so the `id` it answers
/// replaces the stand-in the naming rules computed, and the `rootResourceId` it reports is
/// the `RootResourceId` attribute the type publishes — the resource every other resource of
/// the api hangs off, which no naming rule could compute either.
async fn create_rest_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, REST_API_PROPERTIES)?;
    if resource.properties.get(REST_API_BODY_S3_LOCATION).is_some() {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not read the definition a {APIGATEWAY_REST_API} holds in \
                 Amazon S3, so the api was not created from its {REST_API_BODY_S3_LOCATION} \
                 property"
            ),
        });
    }
    if let Some(definition) = resource.properties.get(REST_API_BODY) {
        return import_rest_api(ctx, region, resource, definition, propagated).await;
    }
    let mut request = Map::new();
    for (property, member) in REST_API_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    if let Some(policy) = stated(resource, REST_API_POLICY) {
        request.insert("policy".to_string(), Value::String(attribute_text(policy)));
    }
    if !request.contains_key("name") {
        // `CreateRestApi$name` is required, and an api that is not created from a definition
        // has nowhere else to take its name from — a declaration stating none asks for an api
        // that call will not make.
        return Err(Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "{APIGATEWAY_REST_API} requires the Name property when it declares no \
                 {REST_API_BODY}"
            ),
        });
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
        REST_APIS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    rest_api_created(&answered, "CreateRestApi")
}

/// Import the api an `AWS::ApiGateway::RestApi` declares a DEFINITION for, so a client that
/// deployed such a stack reads the api back — with the resource for every path of that
/// document, the method for every operation on a path and the integration behind each
/// method — rather than a `NotFoundException` for an api nothing created.
///
/// The definition IS the api, so it travels as `ImportRestApi`'s `body` payload, with the
/// declared import `Parameters` and `FailOnWarnings` on the query that operation binds them
/// to and the endpoint types the declaration states as the import parameter published for
/// them ([`ENDPOINT_TYPES_PARAMETER`]). What the import takes no member for is applied after
/// it, through the calls API Gateway publishes for an api that exists: the scalar
/// configuration with `UpdateRestApi` ([`REST_API_PATCHED`]) and the tags with
/// `TagResource`.
async fn import_rest_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    definition: &Value,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    if resource.properties.get(REST_API_CLONE_FROM).is_some() {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "{APIGATEWAY_REST_API} declares a {REST_API_BODY}, so the api is imported from \
                 that definition, and {REST_API_CLONE_FROM} is no member of that import; the \
                 resource was not created"
            ),
        });
    }
    // `mode=import` is part of the operation's own URI, so the parameters a declaration
    // states are appended to it.
    let mut uri = format!("{REST_APIS_URI}?mode=import");
    if let Some(true) = stated(resource, FAIL_ON_WARNINGS.0).and_then(Value::as_bool) {
        uri.push_str(&format!("&{}=true", FAIL_ON_WARNINGS.1));
    }
    if let Some(parameters) = stated(resource, "Parameters").and_then(Value::as_object) {
        for (parameter, value) in parameters {
            uri.push('&');
            uri.push_str(&percent_encoded(parameter));
            uri.push('=');
            uri.push_str(&percent_encoded(&attribute_text(value)));
        }
    }
    for endpoint_type in endpoint_types(resource) {
        uri.push('&');
        uri.push_str(ENDPOINT_TYPES_PARAMETER);
        uri.push('=');
        uri.push_str(&percent_encoded(&endpoint_type));
    }
    let answered = call_rest_payload(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &uri,
        DEFINITION_MEDIA_TYPE,
        attribute_text(definition).into_bytes(),
    )
    .await?;
    let created = rest_api_created(&answered, "ImportRestApi")?;
    let api_id = created
        .physical_resource_id
        .clone()
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    let patches = rest_api_patches(resource)?;
    if !patches.is_empty() {
        let mut request = Map::new();
        request.insert("patchOperations".to_string(), Value::Array(patches));
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PATCH",
            &rest_api_uri(&api_id),
            Some(&Value::Object(request)),
        )
        .await?;
    }
    let tags = gateway_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        let mut request = Map::new();
        request.insert("tags".to_string(), Value::Object(tags));
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PUT",
            &format!("/tags/{}", percent_encoded(&rest_api_arn(region, &api_id))),
            Some(&Value::Object(request)),
        )
        .await?;
    }
    Ok(created)
}

/// The patches that bring an IMPORTED api to the configuration its declaration states: one
/// `replace` per declared scalar ([`REST_API_PATCHED`]), one `add` per declared binary media
/// type and per declared VPC endpoint id, and the endpoint's own address type.
///
/// The endpoint TYPES are not here: they travel as the import's own parameter, which is the
/// member API Gateway publishes for them on that call.
fn rest_api_patches(resource: &StoredResource) -> Result<Vec<Value>, Refusal> {
    let mut patches = Vec::new();
    for (property, path) in REST_API_PATCHED {
        let Some(value) = stated(resource, property) else {
            continue;
        };
        patches.push(patch("replace", path, &attribute_text(value)));
    }
    for media_type in text_list(stated(resource, "BinaryMediaTypes")) {
        patches.push(patch("add", "/binaryMediaTypes", &media_type));
    }
    let endpoint = stated(resource, "EndpointConfiguration");
    for vpc_endpoint_id in text_list(endpoint.and_then(|value| value.get("VpcEndpointIds"))) {
        patches.push(patch(
            "add",
            "/endpointConfiguration/vpcEndpointIds",
            &vpc_endpoint_id,
        ));
    }
    if let Some(address_type) = endpoint
        .and_then(|value| value.get("IpAddressType"))
        .map(attribute_text)
    {
        patches.push(patch(
            "replace",
            "/endpointConfiguration/ipAddressType",
            &address_type,
        ));
    }
    Ok(patches)
}

/// The endpoint types a declaration's `EndpointConfiguration` states, which the import takes
/// as a parameter of its own.
fn endpoint_types(resource: &StoredResource) -> Vec<String> {
    text_list(
        stated(resource, "EndpointConfiguration").and_then(|configuration| {
            configuration.get("Types")
        }),
    )
}

/// The api an answered create or import reports: the `id` that service assigned, which is
/// this resource's identity, and the `rootResourceId` it reports, which is the attribute the
/// type publishes. One spelling for both calls, which answer the same api.
fn rest_api_created(answered: &Value, operation: &str) -> Result<Created, Refusal> {
    let api_id = api_gateway_answered(answered, "id", operation)?;
    let mut attributes = BTreeMap::new();
    attributes.insert("RestApiId".to_string(), api_id.clone());
    if let Some(root) = answered.get("rootResourceId").and_then(Value::as_str) {
        attributes.insert("RootResourceId".to_string(), root.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(api_id),
        attributes,
    })
}

/// The ARN API Gateway names a REST API by, which is what its tagging calls address: that
/// service's own REST path under its ARN namespace, as the account-less form its model
/// declares (`arn:aws:apigateway:{region}::/restapis/{id}`).
fn rest_api_arn(region: &str, api_id: &str) -> String {
    format!("arn:aws:{API_GATEWAY_SIGNING_NAME}:{region}::/restapis/{api_id}")
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Resource
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Resource` publishes: the api it belongs to, the resource
/// it hangs off and its own path segment.
const RESOURCE_PROPERTIES: &[&str] = &[REST_API_ID, RESOURCE_PARENT_ID, RESOURCE_PATH_PART];

/// The property naming the resource this one hangs off, which addresses `CreateResource`.
const RESOURCE_PARENT_ID: &str = "ParentId";

/// The property carrying the path segment the resource adds, which is that call's one body
/// member.
const RESOURCE_PATH_PART: &str = "PathPart";

/// Create the resource an `AWS::ApiGateway::Resource` declares, under the resource its
/// `ParentId` names — so the api really holds the path the template declared and a method
/// can be put on it.
///
/// "Ref returns the resource ID", which API Gateway assigns: `CreateResource` takes the
/// PARENT's id and answers the `id` it chose for the new one, so that id is this resource's
/// physical id.
async fn create_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, RESOURCE_PROPERTIES)?;
    let uri = format!(
        "{}/resources/{}",
        rest_api_uri(&required(resource, REST_API_ID)?),
        percent_encoded(&required(resource, RESOURCE_PARENT_ID)?)
    );
    let mut request = Map::new();
    request.insert(
        "pathPart".to_string(),
        Value::String(required(resource, RESOURCE_PATH_PART)?),
    );
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &uri,
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateResource")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("ResourceId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Method
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Method` publishes.
const METHOD_PROPERTIES: &[&str] = &[
    "ApiKeyRequired",
    "AuthorizationScopes",
    "AuthorizationType",
    "AuthorizerId",
    METHOD_HTTP_METHOD,
    METHOD_INTEGRATION,
    METHOD_RESPONSES,
    "OperationName",
    "RequestModels",
    "RequestParameters",
    "RequestValidatorId",
    METHOD_RESOURCE_ID,
    REST_API_ID,
];

/// The properties that address `PutMethod` rather than travelling in its body: the resource
/// the method is on and the verb it answers.
const METHOD_RESOURCE_ID: &str = "ResourceId";
const METHOD_HTTP_METHOD: &str = "HttpMethod";

/// The property carrying the method's integration, which is `PutIntegration`'s own call.
const METHOD_INTEGRATION: &str = "Integration";

/// The property carrying the method's responses, each of which is a `PutMethodResponse`.
const METHOD_RESPONSES: &str = "MethodResponses";

/// The properties that ARE `PutMethod`'s own body members — each the property with a
/// lower-cased initial.
const METHOD_MEMBERS: &[(&str, &str)] = &[
    ("ApiKeyRequired", "apiKeyRequired"),
    ("AuthorizationScopes", "authorizationScopes"),
    ("AuthorizationType", "authorizationType"),
    ("AuthorizerId", "authorizerId"),
    ("OperationName", "operationName"),
    ("RequestModels", "requestModels"),
    ("RequestParameters", "requestParameters"),
    ("RequestValidatorId", "requestValidatorId"),
];

/// The `authorizationType` of a method whose declaration states none: `PutMethod` requires
/// the member, and a method with no authorizer on it is open — which is the value API Gateway
/// reports such a method's authorization as.
const NO_AUTHORIZATION: &str = "NONE";

/// The properties of a method's `Integration` that ARE `PutIntegration`'s own body members,
/// each under the name that operation puts it on the WIRE.
///
/// `IntegrationHttpMethod` is the one the model spells differently: that call's
/// `integrationHttpMethod` is "the HTTP method of the integration" and travels as
/// `httpMethod` (its `jsonName`), while the member called `httpMethod` is the METHOD's own
/// verb, which addresses the call and never travels in its body.
const INTEGRATION_MEMBERS: &[(&str, &str)] = &[
    ("CacheKeyParameters", "cacheKeyParameters"),
    ("CacheNamespace", "cacheNamespace"),
    ("ConnectionId", "connectionId"),
    ("ConnectionType", "connectionType"),
    ("ContentHandling", "contentHandling"),
    ("Credentials", "credentials"),
    ("IntegrationHttpMethod", "httpMethod"),
    ("PassthroughBehavior", "passthroughBehavior"),
    ("RequestParameters", "requestParameters"),
    ("RequestTemplates", "requestTemplates"),
    ("TimeoutInMillis", "timeoutInMillis"),
    ("TlsConfig", "tlsConfig"),
    ("Type", "type"),
    ("Uri", "uri"),
];

/// The property of an `Integration` carrying the responses it maps, each of which is a
/// `PutIntegrationResponse` of its own.
const INTEGRATION_RESPONSES: &str = "IntegrationResponses";

/// The properties of one integration response that are `PutIntegrationResponse`'s own body
/// members. Its `StatusCode` addresses the call.
const INTEGRATION_RESPONSE_MEMBERS: &[(&str, &str)] = &[
    ("ContentHandling", "contentHandling"),
    ("ResponseParameters", "responseParameters"),
    ("ResponseTemplates", "responseTemplates"),
    ("SelectionPattern", "selectionPattern"),
];

/// The properties of one method response that are `PutMethodResponse`'s own body members.
/// Its `StatusCode` addresses the call.
const METHOD_RESPONSE_MEMBERS: &[(&str, &str)] = &[
    ("ResponseModels", "responseModels"),
    ("ResponseParameters", "responseParameters"),
];

/// The member of a response that names the status it is for, which is the `@httpLabel` both
/// response puts bind.
const STATUS_CODE: &str = "StatusCode";

/// Put the method an `AWS::ApiGateway::Method` declares on the resource it names, with the
/// integration behind it and the responses either side of it — so an invoke of the deployed
/// stage reaches the backend the template declared rather than a path the api has no method
/// for.
///
/// Four calls, because API Gateway holds four resources: `PutMethod` for the method itself,
/// `PutIntegration` for its `Integration`, `PutIntegrationResponse` for each response that
/// integration maps and `PutMethodResponse` for each `MethodResponses` entry. The method's
/// own identity stays CloudFormation's: API Gateway addresses a method by the api, the
/// resource and the verb rather than by an id of its own, so there is nothing to adopt.
///
/// How many calls that is, is bounded by the DECLARATION — one per declared response — which
/// the template reader has already bounded ([`crate::state::template::MAX_TEMPLATE_NODES`]),
/// so no request can make this walk run longer than the template it came in.
async fn put_method(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, METHOD_PROPERTIES)?;
    // Every nested member is checked BEFORE the first put, so a declaration this module
    // cannot apply leaves no half-built method behind.
    if let Some(integration) = stated(resource, METHOD_INTEGRATION) {
        refuse_unpublished_members(
            APIGATEWAY_METHOD,
            METHOD_INTEGRATION,
            integration,
            INTEGRATION_MEMBERS,
            &[INTEGRATION_RESPONSES],
        )?;
        for response in entries(integration.get(INTEGRATION_RESPONSES)) {
            required_member(APIGATEWAY_METHOD, INTEGRATION_RESPONSES, response)?;
            refuse_unpublished_members(
                APIGATEWAY_METHOD,
                INTEGRATION_RESPONSES,
                response,
                INTEGRATION_RESPONSE_MEMBERS,
                &[STATUS_CODE],
            )?;
        }
    }
    for response in entries(stated(resource, METHOD_RESPONSES)) {
        required_member(APIGATEWAY_METHOD, METHOD_RESPONSES, response)?;
        refuse_unpublished_members(
            APIGATEWAY_METHOD,
            METHOD_RESPONSES,
            response,
            METHOD_RESPONSE_MEMBERS,
            &[STATUS_CODE],
        )?;
    }
    let uri = method_uri(resource)?;
    let mut request = Map::new();
    for (property, member) in METHOD_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    request
        .entry("authorizationType".to_string())
        .or_insert_with(|| Value::String(NO_AUTHORIZATION.to_string()));
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PUT",
        &uri,
        Some(&Value::Object(request)),
    )
    .await?;

    if let Some(integration) = stated(resource, METHOD_INTEGRATION) {
        let mut request = Map::new();
        for (property, member) in INTEGRATION_MEMBERS {
            if let Some(value) = integration.get(*property).filter(|value| !value.is_null()) {
                request.insert((*member).to_string(), member_value(property, value)?);
            }
        }
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PUT",
            &format!("{uri}/integration"),
            Some(&Value::Object(request)),
        )
        .await?;
        for response in entries(integration.get(INTEGRATION_RESPONSES)) {
            let status = required_member(APIGATEWAY_METHOD, INTEGRATION_RESPONSES, response)?;
            let request = response_request(response, INTEGRATION_RESPONSE_MEMBERS)?;
            call_rest_json(
                ctx,
                API_GATEWAY_SIGNING_NAME,
                region,
                "PUT",
                &format!(
                    "{uri}/integration/responses/{}",
                    percent_encoded(&status)
                ),
                Some(&request),
            )
            .await?;
        }
    }

    for response in entries(stated(resource, METHOD_RESPONSES)) {
        let status = required_member(APIGATEWAY_METHOD, METHOD_RESPONSES, response)?;
        let request = response_request(response, METHOD_RESPONSE_MEMBERS)?;
        call_rest_json(
            ctx,
            API_GATEWAY_SIGNING_NAME,
            region,
            "PUT",
            &format!("{uri}/responses/{}", percent_encoded(&status)),
            Some(&request),
        )
        .await?;
    }
    Ok(Created::default())
}

/// One response's own members as the request that puts it, every member the property with a
/// lower-cased initial.
fn response_request(response: &Value, members: &[(&str, &str)]) -> Result<Value, Refusal> {
    let mut request = Map::new();
    for (property, member) in members {
        if let Some(value) = response.get(*property).filter(|value| !value.is_null()) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    Ok(Value::Object(request))
}

/// The URI that addresses ONE method: the api, the resource and the verb, which are the three
/// `@httpLabel`s every call on a method binds.
fn method_uri(resource: &StoredResource) -> Result<String, Refusal> {
    Ok(format!(
        "{}/resources/{}/methods/{}",
        rest_api_uri(&required(resource, REST_API_ID)?),
        percent_encoded(&required(resource, METHOD_RESOURCE_ID)?),
        percent_encoded(&required(resource, METHOD_HTTP_METHOD)?)
    ))
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Deployment
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Deployment` publishes.
const DEPLOYMENT_PROPERTIES: &[&str] = &[
    DEPLOYMENT_CANARY_SETTINGS,
    "Description",
    REST_API_ID,
    DEPLOYMENT_STAGE_DESCRIPTION,
    "StageName",
];

/// The property carrying how the deployment is rolled out to the stage's CANARY, which is
/// `CreateDeployment`'s own `canarySettings`.
const DEPLOYMENT_CANARY_SETTINGS: &str = "DeploymentCanarySettings";

/// The property carrying the configuration of the STAGE this deployment creates.
const DEPLOYMENT_STAGE_DESCRIPTION: &str = "StageDescription";

/// The properties that ARE `CreateDeployment`'s own body members.
const DEPLOYMENT_MEMBERS: &[(&str, &str)] =
    &[("Description", "description"), ("StageName", "stageName")];

/// The members of a `StageDescription` that are `CreateDeployment`'s own: the settings of the
/// stage it creates that call takes. `Description` is that call's `stageDescription` — the
/// stage's description, as against the deployment's own — and `CanarySetting` its
/// `canarySettings`.
const STAGE_DESCRIPTION_MEMBERS: &[(&str, &str)] = &[
    ("CacheClusterEnabled", "cacheClusterEnabled"),
    ("CacheClusterSize", "cacheClusterSize"),
    ("CanarySetting", "canarySettings"),
    ("Description", "stageDescription"),
    ("TracingEnabled", "tracingEnabled"),
    ("Variables", "variables"),
];

/// Create the deployment an `AWS::ApiGateway::Deployment` declares: the snapshot of the api's
/// methods a stage serves, so the api the stack published answers requests at all.
///
/// "Ref returns the deployment ID", which API Gateway assigns — a template declares the api
/// and the stage, never the deployment's own id — so the `id` the create answers is this
/// resource's physical id. A declared `StageDescription` configures the stage the call
/// creates: the members that create takes travel in it, and the rest are applied to that
/// stage afterwards ([`stage_patches`]), which is the one call API Gateway publishes for
/// them.
async fn create_deployment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, DEPLOYMENT_PROPERTIES)?;
    let api_id = required(resource, REST_API_ID)?;
    let description = stated(resource, DEPLOYMENT_STAGE_DESCRIPTION);
    // The settings of the stage this call creates that `CreateDeployment` takes no member
    // for, built BEFORE the create so a `StageDescription` carrying one this module cannot
    // apply is refused with no deployment left behind. It is read whether or not the
    // declaration names a stage, since a member nothing applies is a member nothing applies.
    let stage_patches = match description {
        // `CreateDeployment` takes no tag of its own, which is what `tagged` is false for.
        Some(description) => stage_patches(description, STAGE_DESCRIPTION_MEMBERS, false)?,
        None => Vec::new(),
    };
    let mut request = Map::new();
    for (property, member) in DEPLOYMENT_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    if let Some(description) = description {
        for (property, member) in STAGE_DESCRIPTION_MEMBERS {
            if let Some(value) = description.get(*property).filter(|value| !value.is_null()) {
                request.insert((*member).to_string(), member_value(property, value)?);
            }
        }
    }
    // The deployment's OWN canary settings, which are the same member and the deployment's
    // rather than the stage description's, so they are what travels where both are declared.
    if let Some(canary) = stated(resource, DEPLOYMENT_CANARY_SETTINGS) {
        request.insert("canarySettings".to_string(), canary_settings(canary)?);
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &format!("{}/deployments", rest_api_uri(&api_id)),
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateDeployment")?;

    // Those settings reach the stage the deployment created, through the patch call API
    // Gateway publishes for a stage that exists. A `StageDescription` with no `StageName`
    // beside it configures no stage, since the deployment created none.
    if let Some(stage_name) = stated(resource, "StageName").map(attribute_text) {
        if !stage_patches.is_empty() {
            patch_stage(ctx, region, &api_id, &stage_name, stage_patches).await?;
        }
    }
    let mut attributes = BTreeMap::new();
    attributes.insert("DeploymentId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Stage
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Stage` publishes.
const STAGE_PROPERTIES: &[&str] = &[
    "AccessLogSetting",
    "CacheClusterEnabled",
    "CacheClusterSize",
    "CanarySetting",
    "ClientCertificateId",
    "DeploymentId",
    "Description",
    "DocumentationVersion",
    STAGE_METHOD_SETTINGS,
    REST_API_ID,
    STAGE_NAME,
    "Tags",
    "TracingEnabled",
    "Variables",
];

/// The property naming the stage, which is both its own name and this resource's physical
/// id.
const STAGE_NAME: &str = "StageName";

/// The properties that ARE `CreateStage`'s own body members. `CanarySetting` is the one the
/// model spells differently (`canarySettings`).
const STAGE_MEMBERS: &[(&str, &str)] = &[
    ("CacheClusterEnabled", "cacheClusterEnabled"),
    ("CacheClusterSize", "cacheClusterSize"),
    ("CanarySetting", "canarySettings"),
    ("DeploymentId", "deploymentId"),
    ("Description", "description"),
    ("DocumentationVersion", "documentationVersion"),
    (STAGE_NAME, "stageName"),
    ("TracingEnabled", "tracingEnabled"),
    ("Variables", "variables"),
];

/// Create the stage an `AWS::ApiGateway::Stage` declares, pointing at the deployment it
/// names — so the invoke URL the stack published is served by the methods that deployment
/// snapshot carries.
///
/// "Ref returns the stage name", which is the name the declaration states and the physical id
/// the naming rules already computed, so nothing is adopted. The settings `CreateStage` takes
/// no member for are applied with `UpdateStage` ([`stage_patches`]), which is built BEFORE the
/// create: a declaration carrying a setting this module cannot apply is then refused with no
/// stage left behind.
async fn create_stage(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, STAGE_PROPERTIES)?;
    let api_id = required(resource, REST_API_ID)?;
    let stage_name = required(resource, STAGE_NAME)?;
    // `CreateStage` takes the declaration's tags as its own tag map ([`gateway_tags`]).
    let patches = stage_patches(&resource.properties, STAGE_MEMBERS, true)?;
    let mut request = Map::new();
    for (property, member) in STAGE_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    let tags = gateway_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &format!("{}/stages", rest_api_uri(&api_id)),
        Some(&Value::Object(request)),
    )
    .await?;
    if !patches.is_empty() {
        patch_stage(ctx, region, &api_id, &stage_name, patches).await?;
    }
    Ok(Created::default())
}

/// The property carrying the per-method settings of a stage, each of which is a patch of its
/// own.
const STAGE_METHOD_SETTINGS: &str = "MethodSettings";

/// The declared stage settings `CreateStage` takes no member for, beside the patch path
/// `UpdateStage` applies each of them on.
///
/// Source: that operation's own patch surface — "/accessLogSettings/destinationArn",
/// "/accessLogSettings/format", "/clientCertificateId" and "/documentationVersion" —
/// against the type's property set. A setting the create a declaration travelled in DID
/// take is not patched again ([`stage_patches`] skips it), so which of these are applied
/// here depends on which call carried the declaration.
const STAGE_PATCHED: &[(&str, &str)] = &[
    ("ClientCertificateId", "/clientCertificateId"),
    ("DocumentationVersion", "/documentationVersion"),
];

/// The members of an `AccessLogSetting`, beside the patch path each one is written on.
const ACCESS_LOG_PATCHED: &[(&str, &str)] = &[
    ("DestinationArn", "/accessLogSettings/destinationArn"),
    ("Format", "/accessLogSettings/format"),
];

/// The settings of one method, as `(property, setting)` pairs: the property a
/// `MethodSettings` entry (or a `StageDescription`) states it under, and the name
/// `UpdateStage`'s patch surface spells that setting with.
const METHOD_SETTING_PATCHED: &[(&str, &str)] = &[
    ("CacheDataEncrypted", "caching/dataEncrypted"),
    ("CacheTtlInSeconds", "caching/ttlInSeconds"),
    ("CachingEnabled", "caching/enabled"),
    ("DataTraceEnabled", "logging/dataTrace"),
    ("LoggingLevel", "logging/loglevel"),
    ("MetricsEnabled", "metrics/enabled"),
    (
        "RequireAuthorizationForCacheControl",
        "caching/requireAuthorizationForCacheControl",
    ),
    ("ThrottlingBurstLimit", "throttling/burstLimit"),
    ("ThrottlingRateLimit", "throttling/rateLimit"),
    (
        "UnauthorizedCacheControlHeaderStrategy",
        "caching/unauthorizedCacheControlHeaderStrategy",
    ),
];

/// The method path that stands for every method of the stage: "method paths defined as
/// {resource_path}/{http_method} for an individual method override, or /\*/\* for overriding
/// all methods in the stage" (`Stage$methodSettings`). A setting stated beside the stage
/// itself rather than inside a `MethodSettings` entry is the stage-wide one, so it is
/// written on that path.
const EVERY_METHOD: &str = "*/*";

/// The members of a `MethodSettings` entry that name the method it is FOR rather than a
/// setting of it.
const METHOD_SETTING_RESOURCE_PATH: &str = "ResourcePath";
const METHOD_SETTING_HTTP_METHOD: &str = "HttpMethod";

/// The patches that bring a created stage to the configuration its declaration states:
/// everything of `declared` that is no member of the create it travelled in.
///
/// `carried` names the members that create DID take, so one document serves both callers —
/// a stage's own properties and a deployment's `StageDescription`, which state the same
/// settings under the same names — and `tagged` says whether that create took the
/// declaration's tags as well.
fn stage_patches(
    declared: &Value,
    carried: &[(&str, &str)],
    tagged: bool,
) -> Result<Vec<Value>, Refusal> {
    let mut patches = Vec::new();
    for (property, path) in STAGE_PATCHED {
        if carried.iter().any(|(member, _)| member == property) {
            continue;
        }
        if let Some(value) = declared.get(*property).filter(|value| !value.is_null()) {
            patches.push(patch("replace", path, &attribute_text(value)));
        }
    }
    if let Some(access_log) = declared
        .get("AccessLogSetting")
        .filter(|value| !value.is_null())
    {
        for (member, path) in ACCESS_LOG_PATCHED {
            if let Some(value) = access_log.get(*member).filter(|value| !value.is_null()) {
                patches.push(patch("replace", path, &attribute_text(value)));
            }
        }
    }
    // A setting stated beside the stage itself applies to every method of it; one stated
    // inside a `MethodSettings` entry applies to the method that entry names.
    for (property, setting) in METHOD_SETTING_PATCHED {
        if let Some(value) = declared.get(*property).filter(|value| !value.is_null()) {
            patches.push(patch(
                "replace",
                &format!("/{EVERY_METHOD}/{setting}"),
                &attribute_text(value),
            ));
        }
    }
    for entry in entries(declared.get(STAGE_METHOD_SETTINGS)) {
        let resource_path = entry
            .get(METHOD_SETTING_RESOURCE_PATH)
            .map(attribute_text)
            .unwrap_or_default();
        let http_method = entry
            .get(METHOD_SETTING_HTTP_METHOD)
            .map(attribute_text)
            .unwrap_or_default();
        let method_path = method_setting_path(&resource_path, &http_method);
        for (property, setting) in METHOD_SETTING_PATCHED {
            if let Some(value) = entry.get(*property).filter(|value| !value.is_null()) {
                patches.push(patch(
                    "replace",
                    &format!("/{method_path}/{setting}"),
                    &attribute_text(value),
                ));
            }
        }
        refuse_unpublished_members(
            APIGATEWAY_STAGE,
            STAGE_METHOD_SETTINGS,
            entry,
            METHOD_SETTING_PATCHED,
            &[METHOD_SETTING_RESOURCE_PATH, METHOD_SETTING_HTTP_METHOD],
        )?;
    }
    // Every other member is one the create carried or a member this walk has placed; one
    // that is none of them is refused rather than dropped. The TAGS are one the caller
    // decides: a stage's own create takes a tag map, while `CreateDeployment` takes none and
    // API Gateway publishes no call that tags the stage it created from this declaration, so
    // a `StageDescription` stating tags is refused rather than created without them.
    let placed: Vec<&str> = carried
        .iter()
        .map(|(property, _)| *property)
        .chain(STAGE_PATCHED.iter().map(|(property, _)| *property))
        .chain(METHOD_SETTING_PATCHED.iter().map(|(property, _)| *property))
        .chain(["AccessLogSetting", STAGE_METHOD_SETTINGS, REST_API_ID])
        .chain(tagged.then_some("Tags"))
        .collect();
    if let Value::Object(members) = declared {
        for member in members.keys() {
            if !placed.contains(&member.as_str()) {
                return Err(Refusal {
                    code: NOT_IMPLEMENTED.to_string(),
                    message: format!(
                        "this emulator does not apply the {member} setting of a stage; the \
                         resource was not created"
                    ),
                });
            }
        }
    }
    Ok(patches)
}

/// The `methodSettings` key one setting is written on: the resource path with every `/`
/// inside it escaped as a JSON Pointer requires, then the HTTP method — or the stage-wide
/// path where the entry names neither.
fn method_setting_path(resource_path: &str, http_method: &str) -> String {
    if resource_path.is_empty() || http_method.is_empty() {
        return EVERY_METHOD.to_string();
    }
    if resource_path == "*" {
        return format!("*/{http_method}");
    }
    format!("{}/{http_method}", resource_path.replace('/', "~1"))
}

/// Apply one patch set to a stage that exists, through the call API Gateway publishes for it.
async fn patch_stage(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    api_id: &str,
    stage_name: &str,
    patches: Vec<Value>,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert("patchOperations".to_string(), Value::Array(patches));
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PATCH",
        &stage_uri(api_id, stage_name),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// The collection every REST API of the account lives in, which is what `CreateRestApi` and
/// `ImportRestApi` are bound to.
const REST_APIS_URI: &str = "/restapis";

/// The URI that addresses ONE api: the `@httpLabel` every call on it binds.
fn rest_api_uri(api_id: &str) -> String {
    format!("{REST_APIS_URI}/{}", percent_encoded(api_id))
}

/// The URI that addresses ONE stage: the api and the stage's own name.
fn stage_uri(api_id: &str, stage_name: &str) -> String {
    format!(
        "{}/stages/{}",
        rest_api_uri(api_id),
        percent_encoded(stage_name)
    )
}

/// One patch operation as `PatchOperation` spells it: the op, the JSON Pointer it writes and
/// the value it writes there.
fn patch(op: &str, path: &str, value: &str) -> Value {
    let mut operation = Map::new();
    operation.insert("op".to_string(), Value::String(op.to_string()));
    operation.insert("path".to_string(), Value::String(path.to_string()));
    operation.insert("value".to_string(), Value::String(value.to_string()));
    Value::Object(operation)
}

/// Those of the tags CloudFormation gives one of these resources that API Gateway's own
/// calls will TAKE.
///
/// That service refuses a tag key in AWS's reserved namespace on every call that takes tags —
/// the `tags` member of a create and `TagResource` alike, in the one check its own crate's tag
/// validation makes ("Tag keys cannot start with 'aws:'") — so the three tags CloudFormation
/// records of its own accord are left off rather than sent to be refused: a stack must not
/// fail over tags CloudFormation adds. The stack's own tags, and the declaration's, are
/// ordinary keys and are applied either way ([`applicable_tags`], the one place that rule is
/// written).
fn gateway_tags(properties: &Value, propagated: &[(String, String)]) -> Map<String, Value> {
    tags_as_map(&tags_of(properties, &applicable_tags(propagated)))
}

/// One declared property as the member it travels as.
///
/// Every member of these types is the property with its initial letter lower-cased, all the
/// way down ([`lower_camel_members`]), with two exceptions this reading carries: a property
/// whose value is a map KEYED BY THE CALLER's own data ([`DATA_KEYED`]) travels exactly as
/// the declaration spells it, since those keys are a stage variable's name, a mapping
/// expression or a content type and not modelled member names; and a canary setting is
/// mapped member by member ([`canary_settings`]), because one of ITS members is such a map.
fn member_value(property: &str, value: &Value) -> Result<Value, Refusal> {
    if DATA_KEYED.contains(&property) {
        return Ok(value.clone());
    }
    if CANARY_PROPERTIES.contains(&property) {
        return canary_settings(value);
    }
    lower_camel_members(value, 0)
}

/// The properties whose value is a MAP the caller keys: a stage variable by its name, a
/// request or response parameter by its mapping expression, a template or a model by the
/// content type it is for. Re-casing one of those keys would change what the declaration
/// asked for, so such a map travels as written.
const DATA_KEYED: &[&str] = &[
    "RequestModels",
    "RequestParameters",
    "RequestTemplates",
    "ResponseModels",
    "ResponseParameters",
    "ResponseTemplates",
    "StageVariableOverrides",
    "Variables",
];

/// The properties carrying a canary setting: a deployment's own, and the one a stage (or a
/// `StageDescription`) states.
const CANARY_PROPERTIES: &[&str] = &["CanarySetting", DEPLOYMENT_CANARY_SETTINGS];

/// The members of a canary setting, as the model spells them (`CanarySettings` and
/// `DeploymentCanarySettings`, whose shared members these are).
const CANARY_MEMBERS: &[(&str, &str)] = &[
    ("DeploymentId", "deploymentId"),
    ("PercentTraffic", "percentTraffic"),
    ("StageVariableOverrides", "stageVariableOverrides"),
    ("UseStageCache", "useStageCache"),
];

/// One declared canary setting as the member it travels as, mapped member by member rather
/// than walked: its `StageVariableOverrides` is a map the caller keys ([`DATA_KEYED`]).
fn canary_settings(declared: &Value) -> Result<Value, Refusal> {
    let mut mapped = Map::new();
    for (property, member) in CANARY_MEMBERS {
        if let Some(value) = declared.get(*property).filter(|value| !value.is_null()) {
            mapped.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    Ok(Value::Object(mapped))
}

/// One declared property, or `None` where the declaration states none — a property declared
/// `null` states nothing.
fn stated<'r>(resource: &'r StoredResource, property: &str) -> Option<&'r Value> {
    resource
        .properties
        .get(property)
        .filter(|value| !value.is_null())
}

/// One property a call cannot be addressed without, as the text it carries, or the refusal
/// for a declaration that states none.
fn required(resource: &StoredResource, property: &str) -> Result<String, Refusal> {
    match stated(resource, property).map(attribute_text) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(missing_property(resource, property)),
    }
}

/// The refusal for a declaration that states none of a property a call here cannot be made
/// without, naming the PROPERTY rather than relaying the request for API Gateway to refuse by
/// its own member name.
///
/// The ONE wording of that refusal for this module and both its children, so a declaration
/// missing a property that ADDRESSES a call ([`required`]) and one missing a member its create
/// declares `@required` ([`access_control::refuse_missing`], [`vpc_link`]) are refused alike.
fn missing_property(resource: &StoredResource, property: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: format!(
            "{} requires the {property} property",
            resource.resource_type
        ),
    }
}

/// This resource's own identifier, which addresses every call on it, or the refusal for a
/// record carrying none: an empty id would address the account's own COLLECTION rather than
/// one resource, so a read of it would answer a page and a delete would address nothing.
///
/// Shared by the arms of this module and of both its children, so none of them can word that
/// refusal differently.
fn own_id(resource: &StoredResource) -> Result<&str, Refusal> {
    let id = resource.physical_resource_id.as_str();
    if id.is_empty() {
        return Err(Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "{} cannot be addressed without the identifier API Gateway assigned it",
                resource.resource_type
            ),
        });
    }
    Ok(id)
}

/// The `StatusCode` of one response, which is the `@httpLabel` its put binds.
fn required_member(
    resource_type: &str,
    property: &str,
    response: &Value,
) -> Result<String, Refusal> {
    match response
        .get(STATUS_CODE)
        .filter(|value| !value.is_null())
        .map(attribute_text)
    {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "an entry of {resource_type}'s {property} requires the {STATUS_CODE} member"
            ),
        }),
    }
}

/// The entries of a declared LIST property, or nothing where the declaration states none.
fn entries(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

/// One declared list of strings, each value as the wire carries it.
fn text_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().map(attribute_text).collect())
        .unwrap_or_default()
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: a resource reported created with less than its declaration asked for would be a
/// success that dropped the caller's configuration.
fn refuse_unpublished(resource: &StoredResource, published: &[&str]) -> Result<(), Refusal> {
    refuse_unpublished_properties(&resource.resource_type, &resource.properties, published)
}

/// [`refuse_unpublished`] over a declaration's properties alone, for the arm that judges the
/// document a stack UPDATE states rather than the one the record was created with.
fn refuse_unpublished_properties(
    resource_type: &str,
    properties: &Value,
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

/// The same refusal for the members of a NESTED property: `placed` are the ones a table above
/// maps and `addressing` the ones that address the call rather than travelling in it.
fn refuse_unpublished_members(
    resource_type: &str,
    property: &str,
    declared: &Value,
    placed: &[(&str, &str)],
    addressing: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(members) = declared else {
        return Ok(());
    };
    for member in members.keys() {
        let carried = placed.iter().any(|(name, _)| name == member)
            || addressing.contains(&member.as_str());
        if carried {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {member} member of {resource_type}'s \
                 {property} property"
            ),
        });
    }
    Ok(())
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
    /// published property is placed somewhere: in the create's body, on the call's URI, as a
    /// patch of the resource, or as its tags.
    #[test]
    fn every_published_property_is_placed() {
        let rest_api: Vec<&str> = REST_API_MEMBERS
            .iter()
            .map(|(property, _)| *property)
            .chain(REST_API_PATCHED.iter().map(|(property, _)| *property))
            .chain([
                REST_API_BODY,
                REST_API_BODY_S3_LOCATION,
                REST_API_MODE,
                REST_API_POLICY,
                "Parameters",
                FAIL_ON_WARNINGS.0,
                "Tags",
            ])
            .collect();
        for property in REST_API_PROPERTIES {
            assert!(
                rest_api.contains(property),
                "case {property:?}: expected the property of {APIGATEWAY_REST_API} to be placed \
                 by some arm, got none"
            );
        }
        let stage: Vec<&str> = STAGE_MEMBERS
            .iter()
            .map(|(property, _)| *property)
            .chain(STAGE_PATCHED.iter().map(|(property, _)| *property))
            .chain([
                "AccessLogSetting",
                STAGE_METHOD_SETTINGS,
                REST_API_ID,
                "Tags",
            ])
            .collect();
        for property in STAGE_PROPERTIES {
            assert!(
                stage.contains(property),
                "case {property:?}: expected the property of {APIGATEWAY_STAGE} to be placed by \
                 some arm, got none"
            );
        }
        let method: Vec<&str> = METHOD_MEMBERS
            .iter()
            .map(|(property, _)| *property)
            .chain([
                METHOD_INTEGRATION,
                METHOD_RESPONSES,
                METHOD_RESOURCE_ID,
                METHOD_HTTP_METHOD,
                REST_API_ID,
            ])
            .collect();
        for property in METHOD_PROPERTIES {
            assert!(
                method.contains(property),
                "case {property:?}: expected the property of {APIGATEWAY_METHOD} to be placed by \
                 some arm, got none"
            );
        }
    }

    /// Each type's resource is addressed by the URI its own operations bind, built from the
    /// properties that name it.
    #[test]
    fn a_resource_is_addressed_by_the_uri_its_operations_bind() {
        let method = declared(
            APIGATEWAY_METHOD,
            "stack-Method-ABC",
            json!({ "RestApiId": "a1b2c3", "ResourceId": "r9", "HttpMethod": "GET" }),
        );
        assert_eq!(
            method_uri(&method).expect("a method naming its api, resource and verb"),
            "/restapis/a1b2c3/resources/r9/methods/GET",
            "case {:?}: expected the method's own URI",
            method.logical_id
        );
        assert_eq!(
            stage_uri("a1b2c3", "Prod"),
            "/restapis/a1b2c3/stages/Prod",
            "case \"stage\": expected the stage's own URI"
        );
    }

    /// A declaration that states none of a property a call cannot be addressed without is
    /// refused, rather than a resource being created somewhere else.
    #[test]
    fn a_declaration_naming_no_api_is_refused() {
        let cases: &[(&str, Value)] = &[
            ("no properties at all", Value::Null),
            ("properties without the api", json!({ "StageName": "Prod" })),
            ("an empty api id", json!({ "RestApiId": "" })),
        ];
        for (case, properties) in cases {
            let resource = declared(APIGATEWAY_STAGE, "Prod", properties.clone());
            let refused = required(&resource, REST_API_ID);
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {refused:?}"
            );
        }
    }

    /// A method setting is written on the path `UpdateStage` reads a method by: the resource
    /// path escaped as a JSON Pointer requires and then the verb, or the stage-wide path
    /// where the entry names no one method.
    #[test]
    fn a_method_setting_is_written_on_the_method_path_it_names() {
        let cases: &[(&str, &str, &str, &str)] = &[
            ("one method", "/items", "GET", "~1items/GET"),
            ("a nested path", "/items/{id}", "PUT", "~1items~1{id}/PUT"),
            ("every method of a resource", "*", "*", "*/*"),
            ("the whole stage", "", "", "*/*"),
            ("a verb with no path", "", "GET", "*/*"),
        ];
        for (case, resource_path, http_method, expected) in cases {
            let path = method_setting_path(resource_path, http_method);
            assert_eq!(
                path, *expected,
                "case {case:?}: expected the method path {expected:?}, got {path:?}"
            );
        }
    }

    /// The settings a declaration states beside the stage itself are written on the
    /// stage-wide method path, and the ones a `MethodSettings` entry states on the method it
    /// names.
    #[test]
    fn stage_settings_become_the_patches_the_update_takes() {
        let declared = json!({
            "StageName": "Prod",
            "ClientCertificateId": "cert-1",
            "AccessLogSetting": { "DestinationArn": "arn:aws:logs:us-east-1:1:log-group:g", "Format": "$context.requestId" },
            "MethodSettings": [{ "ResourcePath": "/items", "HttpMethod": "GET", "MetricsEnabled": true }]
        });
        let patches =
            stage_patches(&declared, STAGE_MEMBERS, true).expect("a mapped declaration");
        let paths: Vec<String> = patches
            .iter()
            .filter_map(|patch| patch.get("path").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        for expected in [
            "/clientCertificateId",
            "/accessLogSettings/destinationArn",
            "/accessLogSettings/format",
            "/~1items/GET/metrics/enabled",
        ] {
            assert!(
                paths.contains(&expected.to_string()),
                "case {expected:?}: expected a patch on that path, got {paths:?}"
            );
        }
    }
}
