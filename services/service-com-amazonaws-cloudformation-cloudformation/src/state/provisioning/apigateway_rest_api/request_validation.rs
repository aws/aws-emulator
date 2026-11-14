// SPDX-License-Identifier: CC0-1.0

//! The two `AWS::ApiGateway::*` types that decide what a deployed METHOD validates a request
//! against — the request validator a method names in its `requestValidatorId`, and the model
//! whose schema its `requestModels` maps a content type to — created in Amazon API Gateway
//! itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls, the same helpers (a declared property is read with
//! [`super::stated`], a property a call cannot be addressed without with [`super::required`],
//! and a property no call here carries is refused with [`super::refuse_unpublished`]), so the
//! two halves of this control plane cannot word a refusal or place a member differently.
//!
//! ## Why they are really created
//!
//! A method's request validation is enforced by API Gateway only through the records these
//! two types ARE: the validator the method names decides whether the call's parameters and
//! its payload are checked before the integration is reached, and the model the method maps
//! its content type to is the schema that payload is checked against. A stack whose validator
//! and model exist nowhere therefore deploys a method whose `requestValidatorId` resolves to
//! nothing — `GetRequestValidator` answers `NotFoundException` for it — so the stage serves a
//! request the template declared invalid, and the integration behind the method runs for a
//! call that should never have reached it, however complete the stack's own records look.
//!
//! The validator's identifier is also that service's to assign: `CreateRequestValidator`
//! takes no identifier and answers the `id` it chose, and "Ref returns the request validator
//! ID" ([`crate::state::template::identity_is_service_assigned`] names the type, so a
//! method's `RequestValidatorId: !Ref Validator` is re-pointed at that id once the create has
//! answered). A MODEL is keyed by its NAME instead — "Ref returns the model name", and
//! `GetModel`, `DeleteModel` and a method's `requestModels` all take that name — so the name
//! the declaration states is the identity, and the create answers it rather than an id.
//!
//! ## The calls
//!
//! Each type is the call that owning service publishes for it, on the URI that operation's
//! own `@http` binding declares:
//!
//! * `AWS::ApiGateway::RequestValidator` is `CreateRequestValidator`
//!   (`POST /restapis/{restApiId}/requestvalidators`);
//! * `AWS::ApiGateway::Model` is `CreateModel` (`POST /restapis/{restApiId}/models`).

use super::*;

use crate::state::template::{APIGATEWAY_MODEL, APIGATEWAY_REQUEST_VALIDATOR};

/// Which of these two types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Kind`], which travels with [`super::super::Owner`]: the algorithm resolves a
/// type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ApiGateway::RequestValidator`.
    RequestValidator,
    /// `AWS::ApiGateway::Model`.
    Model,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        APIGATEWAY_REQUEST_VALIDATOR => Some(Kind::RequestValidator),
        APIGATEWAY_MODEL => Some(Kind::Model),
        _ => None,
    }
}

/// The property both types state the record's own name in, which is `CreateRequestValidator`'s
/// and `CreateModel`'s own `name` member.
const NAME: &str = "Name";

// ---------------------------------------------------------------------------
// AWS::ApiGateway::RequestValidator
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::RequestValidator` publishes. A declared property this
/// list does not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `CreateRequestValidator`'s members
/// (`smithy_query(query="operation",operation="CreateRequestValidator")`).
const VALIDATOR_PROPERTIES: &[&str] = &[
    NAME,
    REST_API_ID,
    "ValidateRequestBody",
    "ValidateRequestParameters",
];

/// The properties that ARE `CreateRequestValidator`'s own body members, as
/// `(property, member)` pairs: every one of them is the property with a lower-cased initial,
/// the api it belongs to addressing the call instead.
const VALIDATOR_MEMBERS: &[(&str, &str)] = &[
    (NAME, "name"),
    ("ValidateRequestBody", "validateRequestBody"),
    ("ValidateRequestParameters", "validateRequestParameters"),
];

/// The properties a validator's own read reports: every member of it, each under this type's
/// property name — a validator has no structured property and carries no tags, so there is
/// none left out.
const VALIDATOR_REPORTED: &[(&str, &str)] = VALIDATOR_MEMBERS;

/// Create the validator an `AWS::ApiGateway::RequestValidator` declares on the api it names,
/// so a client that deployed the stack reads it back through `GetRequestValidator` — and so
/// the method naming it really has its parameters and its payload checked before the
/// integration runs — rather than an api that holds no validator at all.
///
/// "Ref returns the request validator ID", which API Gateway assigns: the create takes no
/// identifier and answers the `id` it chose, so that id is this resource's physical id and the
/// `RequestValidatorId` attribute the type publishes.
async fn create_request_validator(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, VALIDATOR_PROPERTIES)?;
    let mut request = Map::new();
    for (property, member) in VALIDATOR_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &validators_uri(&required(resource, REST_API_ID)?),
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateRequestValidator")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("RequestValidatorId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The collection the request validators of ONE api live in, which is what
/// `CreateRequestValidator` is bound to.
fn validators_uri(api_id: &str) -> String {
    format!("{}/requestvalidators", rest_api_uri(api_id))
}

/// The URI that addresses ONE request validator: the api that holds it and its own id.
fn validator_uri(api_id: &str, validator_id: &str) -> String {
    format!(
        "{}/{}",
        validators_uri(api_id),
        percent_encoded(validator_id)
    )
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::Model
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::Model` publishes.
///
/// Source: the type's own property set, against `CreateModel`'s members
/// (`smithy_query(query="operation",operation="CreateModel")`).
const MODEL_PROPERTIES: &[&str] = &[
    MODEL_CONTENT_TYPE,
    "Description",
    NAME,
    REST_API_ID,
    MODEL_SCHEMA,
];

/// The property naming the content type the model is the schema for, which `CreateModel`
/// declares `@required`.
const MODEL_CONTENT_TYPE: &str = "ContentType";

/// The property carrying the model's SCHEMA, which travels as text rather than as the
/// document the declaration states ([`model_schema`]).
const MODEL_SCHEMA: &str = "Schema";

/// The properties that ARE `CreateModel`'s own body members, as `(property, member)` pairs:
/// each is the property with a lower-cased initial, the api it belongs to addressing the call
/// instead.
const MODEL_MEMBERS: &[(&str, &str)] = &[
    (MODEL_CONTENT_TYPE, "contentType"),
    ("Description", "description"),
    (NAME, "name"),
];

/// The properties a model's own read reports, under this type's property names.
///
/// The SCHEMA is left out for the reason [`super::REST_API_REPORTED`] records of an api's
/// resource policy: a declaration states it as a JSON DOCUMENT while the read answers the
/// text that document was stored as, and a property reported in a different shape would be
/// evidence of a change nobody made.
const MODEL_REPORTED: &[(&str, &str)] = MODEL_MEMBERS;

/// The members `CreateModel` declares `@required`: "the name of the model" and "the content
/// type for the model", neither of which anything but the declaration can supply.
const MODEL_REQUIRED: &[(&str, &str)] = &[(NAME, "name"), (MODEL_CONTENT_TYPE, "contentType")];

/// Create the model an `AWS::ApiGateway::Model` declares on the api it names, so a client that
/// deployed the stack reads it back through `GetModel` — and so a method mapping its content
/// type to the model really has its payload validated against that schema — rather than an api
/// that holds no model at all.
///
/// "Ref returns the model name": API Gateway keys a model by its name under the api that holds
/// it, so the name the create was made under is this resource's physical id.
async fn create_model(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, MODEL_PROPERTIES)?;
    let name = model_name(resource);
    let mut request = Map::new();
    for (property, member) in MODEL_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    if !name.is_empty() {
        request.insert("name".to_string(), Value::String(name.clone()));
    }
    if let Some(schema) = stated(resource, MODEL_SCHEMA) {
        request.insert("schema".to_string(), Value::String(model_schema(schema)));
    }
    refuse_missing(resource, &request, MODEL_REQUIRED)?;
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &models_uri(&required(resource, REST_API_ID)?),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(name),
        attributes: BTreeMap::new(),
    })
}

/// The name the model is created under, which is also its identity.
///
/// The declaration's own `Name` where it states one. Where it states none, CloudFormation
/// names the model itself — and `CreateModel$name` "must be alphanumeric", so the generated
/// identifier contributes only the alphanumerics it is built from: a name carrying the
/// separators of a generated id is one that service refuses, which would fail a stack
/// CloudFormation deploys.
fn model_name(resource: &StoredResource) -> String {
    if let Some(declared) = stated(resource, NAME)
        .map(attribute_text)
        .filter(|name| !name.is_empty())
    {
        return declared;
    }
    resource
        .physical_resource_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// One declared `Schema` as the text `CreateModel` takes it as.
///
/// That property is a JSON document ("the schema for the model. For application/json models,
/// this should be JSON schema draft 4 model") while the call's `schema` member is a String, so
/// a declared document travels as its own serialization and a declared string travels exactly
/// as written — the text a client stated is the text the model is stored with, so nothing of
/// the schema is re-cased or re-shaped on the way.
fn model_schema(declared: &Value) -> String {
    match declared {
        Value::String(text) => text.clone(),
        document => document.to_string(),
    }
}

/// The collection the models of ONE api live in, which is what `CreateModel` is bound to.
fn models_uri(api_id: &str) -> String {
    format!("{}/models", rest_api_uri(api_id))
}

/// The URI that addresses ONE model: the api that holds it and its own name.
fn model_uri(api_id: &str, model_name: &str) -> String {
    format!("{}/{}", models_uri(api_id), percent_encoded(model_name))
}

// ---------------------------------------------------------------------------
// The seam [`super`] reaches these arms through
// ---------------------------------------------------------------------------

/// The members of one of these creates the model declares `@required`: a declaration stating
/// none of one asks for a resource that call will not make, so it is refused here naming the
/// PROPERTY — in the one wording of that refusal this control plane has
/// ([`super::missing_property`]) — rather than relaying the request for the owning service to
/// refuse by its own member name.
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

/// Create one of these resources in API Gateway; see [`super::create`].
///
/// Neither type publishes a `Tags` property, so CloudFormation propagates no tag to either:
/// there is no call that could carry one.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    match kind {
        Kind::RequestValidator => create_request_validator(ctx, region, resource).await,
        Kind::Model => create_model(ctx, region, resource).await,
    }
}

/// The URI that addresses one of these resources, which is the one its own read and its own
/// delete share — built from this resource's identity and the api that holds it.
fn addressed(kind: Kind, resource: &StoredResource) -> Result<String, Refusal> {
    let api_id = required(resource, REST_API_ID)?;
    let id = own_id(resource)?;
    Ok(match kind {
        Kind::RequestValidator => validator_uri(&api_id, id),
        Kind::Model => model_uri(&api_id, id),
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
        Kind::RequestValidator => VALIDATOR_REPORTED,
        Kind::Model => MODEL_REPORTED,
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
    /// published property is placed somewhere: in the create's body or on the call's URI.
    #[test]
    fn every_published_property_is_placed() {
        let rows: &[(&str, &[&str], Vec<&str>)] = &[
            (
                APIGATEWAY_REQUEST_VALIDATOR,
                VALIDATOR_PROPERTIES,
                VALIDATOR_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain([REST_API_ID])
                    .collect(),
            ),
            (
                APIGATEWAY_MODEL,
                MODEL_PROPERTIES,
                MODEL_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain([REST_API_ID, MODEL_SCHEMA])
                    .collect(),
            ),
        ];
        for (resource_type, published, placed) in rows {
            for property in *published {
                assert!(
                    placed.contains(property),
                    "case {resource_type:?}/{property:?}: expected the property to be placed \
                     by some arm, got {placed:?}"
                );
            }
        }
    }

    /// Each resource is addressed by the URI its own operations bind, built from the api that
    /// holds it and its own identity.
    #[test]
    fn a_resource_is_addressed_by_the_uri_its_operations_bind() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                "a request validator",
                APIGATEWAY_REQUEST_VALIDATOR,
                "v9k2m1",
                "/restapis/a1b2c3/requestvalidators/v9k2m1",
            ),
            (
                "a model",
                APIGATEWAY_MODEL,
                "EchoRequest",
                "/restapis/a1b2c3/models/EchoRequest",
            ),
        ];
        for (case, resource_type, physical, expected) in cases {
            let resource = declared(resource_type, physical, json!({ "RestApiId": "a1b2c3" }));
            let kind = kind_of(resource_type).expect("a type this module owns");
            let uri = addressed(kind, &resource).expect("a resource naming its api");
            assert_eq!(
                uri, *expected,
                "case {case:?}: expected the URI {expected:?}, got {uri:?}"
            );
        }
    }

    /// A model's name is the one its declaration states; a declaration that states none is
    /// named by the alphanumerics of the identifier CloudFormation generated, which is a name
    /// `CreateModel` accepts.
    #[test]
    fn a_model_is_created_under_the_name_it_is_identified_by() {
        let cases: &[(&str, &str, Value, &str)] = &[
            (
                "a declared name",
                "stack-Model-7QF3",
                json!({ "RestApiId": "a1b2c3", "Name": "EchoRequest" }),
                "EchoRequest",
            ),
            (
                "no declared name",
                "stack-Model-7QF3",
                json!({ "RestApiId": "a1b2c3" }),
                "stackModel7QF3",
            ),
        ];
        for (case, physical, properties, expected) in cases {
            let resource = declared(APIGATEWAY_MODEL, physical, properties.clone());
            let name = model_name(&resource);
            assert_eq!(
                name, *expected,
                "case {case:?}: expected the model name {expected:?}, got {name:?}"
            );
        }
    }

    /// A declared schema travels as TEXT: a document as its own serialization, a string
    /// exactly as written.
    #[test]
    fn a_declared_schema_travels_as_the_text_the_create_takes() {
        let cases: &[(&str, Value, &str)] = &[
            (
                "a declared document",
                json!({ "type": "object", "required": ["marker"] }),
                r#"{"type":"object","required":["marker"]}"#,
            ),
            (
                "a declared string",
                json!("{\"type\":\"object\"}"),
                r#"{"type":"object"}"#,
            ),
        ];
        for (case, declared, expected) in cases {
            let text = model_schema(declared);
            assert_eq!(
                text, *expected,
                "case {case:?}: expected the schema text {expected:?}, got {text:?}"
            );
        }
    }

    /// A declaration that states none of a member its create declares `@required` is refused
    /// naming the PROPERTY, rather than a model being created with less than it asked for.
    #[test]
    fn a_model_declaring_no_content_type_is_refused() {
        let resource = declared(
            APIGATEWAY_MODEL,
            "stack-Model-7QF3",
            json!({ "RestApiId": "a1b2c3", "Name": "EchoRequest" }),
        );
        let mut request = Map::new();
        request.insert("name".to_string(), Value::String("EchoRequest".to_string()));
        let refused = refuse_missing(&resource, &request, MODEL_REQUIRED);
        let message = refused.expect_err("a refusal naming the missing property").message;
        assert!(
            message.contains(MODEL_CONTENT_TYPE),
            "case \"no content type\": expected a refusal naming {MODEL_CONTENT_TYPE:?}, got \
             {message:?}"
        );
    }
}
