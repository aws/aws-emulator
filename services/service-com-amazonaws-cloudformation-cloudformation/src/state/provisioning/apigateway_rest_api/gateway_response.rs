// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::GatewayResponse`: the customization of one of the responses API GATEWAY
//! ITSELF returns for an api — the status code, the parameters and the templates a refusal of
//! that class answers with — put on the api in Amazon API Gateway itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls and the same helpers (a declared property is read with
//! [`super::stated`], a property a call cannot be addressed without with [`super::required`],
//! and a property no call here carries is refused with [`super::refuse_unpublished`]), so no
//! two arms of this control plane word a refusal or place a member differently.
//!
//! ## Why it is really created
//!
//! A gateway response is what API Gateway answers a caller with when the api itself refuses the
//! request — an unknown resource, a missing authentication token, a throttled call — and the
//! customization is held BY THAT SERVICE: until one is put on the api, every read of the type
//! answers the response API Gateway generates (`defaultResponse: true`, carrying no status code
//! at all for the two class-wide defaults), and the api keeps answering its own body and status
//! code. So a stack whose gateway response exists nowhere publishes a customization no client
//! can read and the deployed api applies none of it, however complete the stack's own record
//! looks.
//!
//! ## The calls
//!
//! The type is the calls that owning service publishes for a gateway response, each on the URI
//! its own operation's `@http` binding declares — all three addressed by the api and the
//! response TYPE, which are the two `@httpLabel`s of every one of them:
//!
//! * the create is `PutGatewayResponse`
//!   (`PUT /restapis/{restApiId}/gatewayresponses/{responseType}`), whose `statusCode`,
//!   `responseParameters` and `responseTemplates` the declaration's properties are;
//! * the delete is `DeleteGatewayResponse`, which is what makes the api answer its own
//!   generated response again once the stack is gone;
//! * the drift read is `GetGatewayResponse`.
//!
//! The put is the whole customization in one call, so a CHANGED declaration is applied by the
//! same call rather than reported as a change this emulator does not make.

use super::*;

use crate::state::template::{
    apigateway_gateway_response_identity, APIGATEWAY_RESPONSE_TYPE_PROPERTY,
};

/// Every property `AWS::ApiGateway::GatewayResponse` publishes. A declared property this list
/// does not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `PutGatewayResponse`'s members
/// (`smithy_query(query="operation",operation="PutGatewayResponse")`): the two `@httpLabel`s
/// that address the call and the three members that travel in its body.
const GATEWAY_RESPONSE_PROPERTIES: &[&str] = &[
    RESPONSE_PARAMETERS,
    RESPONSE_TEMPLATES,
    APIGATEWAY_RESPONSE_TYPE_PROPERTY,
    REST_API_ID,
    GATEWAY_STATUS_CODE,
];

/// The property carrying the HTTP status code the customized response answers with, which is
/// `PutGatewayResponse$statusCode`.
const GATEWAY_STATUS_CODE: &str = "StatusCode";

/// The two properties carrying a gateway response's MAPS: the parameters its headers, query
/// strings and paths are mapped from, and the templates its body is rendered with.
const RESPONSE_PARAMETERS: &str = "ResponseParameters";
const RESPONSE_TEMPLATES: &str = "ResponseTemplates";

/// The properties that ARE `PutGatewayResponse`'s own body members, as `(property, member)`
/// pairs: each is the property with a lower-cased initial, the api and the response type
/// addressing the call instead.
const GATEWAY_RESPONSE_MEMBERS: &[(&str, &str)] = &[
    (GATEWAY_STATUS_CODE, "statusCode"),
    (RESPONSE_PARAMETERS, "responseParameters"),
    (RESPONSE_TEMPLATES, "responseTemplates"),
];

/// The properties a gateway response's own read reports, under this type's property names.
///
/// The STATUS CODE alone. The two maps are left out for the reason
/// [`super::REST_API_REPORTED`] leaves an api's resource policy out: every representation of a
/// gateway response carries both of them filled in — the parameters as an empty map while
/// nothing is mapped, the templates as API Gateway's own body template — so comparing them
/// against the declaration would report a difference nobody made.
const GATEWAY_RESPONSE_REPORTED: &[(&str, &str)] = &[(GATEWAY_STATUS_CODE, "statusCode")];

/// Put the customization an `AWS::ApiGateway::GatewayResponse` declares on the api it names, so
/// a client that deployed the stack reads it back through `GetGatewayResponse` — and so the
/// deployed api really answers that status code, those parameters and those templates when it
/// refuses a request — rather than an api that carries no customization at all.
///
/// The resource is addressed by the api and the response type it customizes, both of them
/// properties the declaration states, so what `Ref` resolves to is that pair
/// ([`apigateway_gateway_response_identity`]) and not an identifier API Gateway assigns: this
/// service assigns none for a gateway response.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, GATEWAY_RESPONSE_PROPERTIES)?;
    let api_id = required(resource, REST_API_ID)?;
    let response_type = required(resource, APIGATEWAY_RESPONSE_TYPE_PROPERTY)?;
    let mut request = Map::new();
    for (property, member) in GATEWAY_RESPONSE_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), gateway_member(value));
        }
    }
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PUT",
        &gateway_response_uri(&api_id, &response_type),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(apigateway_gateway_response_identity(
            &api_id,
            &response_type,
        )),
        attributes: BTreeMap::new(),
    })
}

/// One declared property as the member it travels as.
///
/// Every member of this type travels as TEXT: the status code is a `StatusCode`
/// (`PutGatewayResponse$statusCode`, a string the model constrains to an HTTP status code), and
/// both maps are keyed by the caller's own data — a mapping expression, a content type — with
/// `MapOfStringToString` values (`GatewayResponse$responseParameters`,
/// `GatewayResponse$responseTemplates`). A template may state a NUMBER or a BOOLEAN literal
/// where a property takes a string ("all property values in a template are strings"), so each
/// value travels as the text that literal spells, through this crate's one reading of a member
/// the owning model types as text throughout — and the keys of either map exactly as the
/// declaration writes them, since re-casing one would change what it asked for.
fn gateway_member(value: &Value) -> Value {
    crate::state::provisioning::modelled_declaration::text_throughout(value)
}

/// Apply a CHANGED declaration of the customization; see [`super::update`].
///
/// `PutGatewayResponse` is the whole customization in one call — a put, not a patch — so the
/// same call that created it applies the new declaration, under the identifier the resource
/// already carries.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Created>, Refusal> {
    create(ctx, region, resource).await.map(Some)
}

/// The URI that addresses ONE gateway response: the api that holds it and the response type it
/// customizes, which are the two `@httpLabel`s every call on one binds.
fn gateway_response_uri(api_id: &str, response_type: &str) -> String {
    format!(
        "{}/gatewayresponses/{}",
        rest_api_uri(api_id),
        percent_encoded(response_type)
    )
}

/// That URI for one declared resource: built from the properties that ADDRESS the call, the way
/// every other record inside an api is addressed here — the resource's own identifier carries
/// the same pair ([`apigateway_gateway_response_identity`]).
fn addressed(resource: &StoredResource) -> Result<String, Refusal> {
    Ok(gateway_response_uri(
        &required(resource, REST_API_ID)?,
        &required(resource, APIGATEWAY_RESPONSE_TYPE_PROPERTY)?,
    ))
}

/// Take the customization off the api; see [`super::delete`].
///
/// What a deleted customization leaves is the response API Gateway GENERATES for the type,
/// which every read then answers — so the delete is what makes the api stop applying what the
/// stack put there. An api that service no longer holds answers `NotFoundException` and is
/// already in the state the delete asks for, as it is for every other record inside one.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    api_gateway_deleted(ctx, region, &addressed(resource)?).await
}

/// The customization the api has as API Gateway reports it, for drift detection; see
/// [`super::actual_configuration`].
///
/// A response the api holds NO customization of is reported as absent (`Ok(None)`): that read
/// answers the response API Gateway generates for the type, flagged `defaultResponse`, which is
/// the service saying the resource this stack put there is not on the api — never a
/// customization to compare a declaration against.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = addressed(resource)?;
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answered) => {
            if answered
                .get(DEFAULT_RESPONSE_MEMBER)
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            let mut held = Map::new();
            for (property, member) in GATEWAY_RESPONSE_REPORTED {
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

/// The member every read of a gateway response says whether it is API Gateway's own generated
/// response under: "a Boolean flag to indicate whether this GatewayResponse is the default
/// gateway response (`true`) or not (`false`)" (`GatewayResponse$defaultResponse`).
const DEFAULT_RESPONSE_MEMBER: &str = "defaultResponse";

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// A resource record as a deployment hands one to these arms.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Declared".to_string(),
            resource_type: APIGATEWAY_GATEWAY_RESPONSE.to_string(),
            physical_resource_id: "a1b2c3:DEFAULT_4XX".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every published property of the type is placed somewhere: in the put's body or on the
    /// call's URI. A property no arm places would be dropped from a create reported as a
    /// success.
    #[test]
    fn every_published_property_is_placed() {
        let placed: Vec<&str> = GATEWAY_RESPONSE_MEMBERS
            .iter()
            .map(|(property, _)| *property)
            .chain([REST_API_ID, APIGATEWAY_RESPONSE_TYPE_PROPERTY])
            .collect();
        for property in GATEWAY_RESPONSE_PROPERTIES {
            assert!(
                placed.contains(property),
                "case {property:?}: expected the property of {APIGATEWAY_GATEWAY_RESPONSE} to \
                 be placed by some arm, got {placed:?}"
            );
        }
    }

    /// The resource is addressed by the URI its own operations bind, built from the api and the
    /// response type the declaration states — and the identifier it is published under carries
    /// that same pair.
    #[test]
    fn a_gateway_response_is_addressed_by_the_uri_its_operations_bind() {
        let rows: &[(&str, Value, &str)] = &[
            (
                "a class-wide default",
                json!({ "RestApiId": "a1b2c3", "ResponseType": "DEFAULT_4XX" }),
                "/restapis/a1b2c3/gatewayresponses/DEFAULT_4XX",
            ),
            (
                "one condition's own response",
                json!({ "RestApiId": "zz99yy", "ResponseType": "MISSING_AUTHENTICATION_TOKEN" }),
                "/restapis/zz99yy/gatewayresponses/MISSING_AUTHENTICATION_TOKEN",
            ),
        ];
        for (case, properties, expected) in rows {
            let resource = declared(properties.clone());
            let uri = addressed(&resource).expect("a resource naming its api and its type");
            assert_eq!(
                uri, *expected,
                "case {case:?}: expected the URI {expected:?}, got {uri:?}"
            );
        }
    }

    /// A declaration states its properties as `PutGatewayResponse`'s own members: each with a
    /// lower-cased initial, with the keys of both maps exactly as written and their values as
    /// the text a literal spells.
    #[test]
    fn a_declaration_travels_as_the_members_the_put_takes() {
        let rows: &[(&str, Value, Value)] = &[
            (
                "a status code and a header parameter",
                json!({
                    "RestApiId": "a1b2c3",
                    "ResponseType": "DEFAULT_4XX",
                    "StatusCode": "404",
                    "ResponseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'*'",
                    },
                }),
                json!({
                    "statusCode": "404",
                    "responseParameters": {
                        "gatewayresponse.header.Access-Control-Allow-Origin": "'*'",
                    },
                }),
            ),
            (
                "a status code a template spells as a number",
                json!({
                    "RestApiId": "a1b2c3",
                    "ResponseType": "DEFAULT_5XX",
                    "StatusCode": 500,
                    "ResponseTemplates": { "application/json": "{\"message\":\"gone\"}" },
                }),
                json!({
                    "statusCode": "500",
                    "responseTemplates": { "application/json": "{\"message\":\"gone\"}" },
                }),
            ),
            (
                "no customization but the response type",
                json!({ "RestApiId": "a1b2c3", "ResponseType": "DEFAULT_4XX" }),
                json!({}),
            ),
        ];
        for (case, properties, expected) in rows {
            let resource = declared(properties.clone());
            let mut request = Map::new();
            for (property, member) in GATEWAY_RESPONSE_MEMBERS {
                if let Some(value) = stated(&resource, property) {
                    request.insert((*member).to_string(), gateway_member(value));
                }
            }
            assert_eq!(
                Value::Object(request.clone()),
                *expected,
                "case {case:?}: expected the members PutGatewayResponse takes, got {request:?}"
            );
        }
    }

    /// A declaration that states none of a property the call cannot be addressed without is
    /// refused naming that property, rather than a call being made against the api's whole
    /// collection of gateway responses.
    #[test]
    fn a_declaration_missing_an_addressing_property_is_refused() {
        let rows: &[(&str, Value, &str)] = &[
            (
                "no response type",
                json!({ "RestApiId": "a1b2c3" }),
                APIGATEWAY_RESPONSE_TYPE_PROPERTY,
            ),
            (
                "no api",
                json!({ "ResponseType": "DEFAULT_4XX" }),
                REST_API_ID,
            ),
        ];
        for (case, properties, named) in rows {
            let refused = addressed(&declared(properties.clone()));
            let Err(refusal) = refused else {
                panic!("case {case:?}: expected a refusal, got {refused:?}");
            };
            assert!(
                refusal.message.contains(named)
                    && refusal.message.contains(APIGATEWAY_GATEWAY_RESPONSE),
                "case {case:?}: expected the refusal to name {named:?} and the type, got \
                 {refusal:?}"
            );
        }
    }
}
