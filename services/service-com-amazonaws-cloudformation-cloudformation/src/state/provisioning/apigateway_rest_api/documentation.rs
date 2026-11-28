// SPDX-License-Identifier: CC0-1.0

//! The two `AWS::ApiGateway::*` types that carry an api's DOCUMENTATION — the part that
//! documents one entity of the api, and the version that snapshots every part of it — created
//! in Amazon API Gateway itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls and the same helpers (a declared property is read with
//! [`super::stated`], a property a call cannot be addressed without with [`super::required`],
//! and a property no call here carries is refused with [`super::refuse_unpublished`]), so no two
//! arms of this control plane word a refusal or place a member differently.
//!
//! ## Why they are really created
//!
//! An api's documentation is held by API Gateway itself: the parts are what
//! `GetDocumentationParts` lists and what an OpenAPI export of a stage carries, and a VERSION is
//! the snapshot of them a stage publishes (`Stage$documentationVersion`). A stack whose parts
//! and versions exist nowhere deploys an api that documents nothing — every read of them answers
//! an empty collection, a stage naming the version is refused, and an export carries no
//! documentation — however complete the stack's own records look.
//!
//! ## The calls
//!
//! Each type is the calls that owning service publishes for it, on the URI its own operation's
//! `@http` binding declares:
//!
//! * `AWS::ApiGateway::DocumentationPart` is `CreateDocumentationPart`
//!   (`POST /restapis/{restApiId}/documentation/parts`), which takes the `location` of the
//!   entity being documented and the `properties` document, and ASSIGNS the part's `id`;
//! * `AWS::ApiGateway::DocumentationVersion` is `CreateDocumentationVersion`
//!   (`POST /restapis/{restApiId}/documentation/versions`), which takes the
//!   `documentationVersion` the declaration names and answers that same version — the service
//!   assigns nothing for one.

use super::*;

use crate::state::template::{
    apigateway_documentation_version_identity, apigateway_identity_part,
    APIGATEWAY_DOCUMENTATION_PART, APIGATEWAY_DOCUMENTATION_VERSION,
    APIGATEWAY_DOCUMENTATION_VERSION_PROPERTY, APIGATEWAY_DOCUMENTATION_VERSION_SEPARATOR,
};

/// Which of these two types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Kind`], which travels with [`super::super::Owner`]: the algorithm resolves a type's
/// owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ApiGateway::DocumentationPart`.
    Part,
    /// `AWS::ApiGateway::DocumentationVersion`.
    Version,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        APIGATEWAY_DOCUMENTATION_PART => Some(Kind::Part),
        APIGATEWAY_DOCUMENTATION_VERSION => Some(Kind::Version),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::DocumentationPart
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::DocumentationPart` publishes.
///
/// Source: the type's own property set, against `CreateDocumentationPart`'s members
/// (`smithy_query(query="operation",operation="CreateDocumentationPart")`): the `@required`
/// `location` and `properties`, with the api addressing the call.
const PART_PROPERTIES: &[&str] = &[PART_LOCATION, PART_CONTENT, REST_API_ID];

/// The property naming the api ENTITY the part documents, which that create declares
/// `@required` ("specifies the target API entity to which the documentation applies").
const PART_LOCATION: &str = "Location";

/// The property carrying the documentation CONTENT, which that create declares `@required` as
/// its `properties` member ("the new documentation content map of the targeted API entity").
const PART_CONTENT: &str = "Properties";

/// The properties a part's own read reports, under this type's property names.
///
/// The LOCATION alone: the content is left out for the reason
/// [`super::request_validation`]'s model leaves its schema out — a declaration may state it as
/// a JSON document while the read answers the text it was stored as, and a property reported in
/// a different shape would be evidence of a change nobody made.
const PART_REPORTED: &[(&str, &str)] = &[(PART_LOCATION, "location")];

/// Create the part an `AWS::ApiGateway::DocumentationPart` declares on the api it names, so a
/// client that deployed the stack reads it back through `GetDocumentationPart` — and so an
/// export of a stage of that api really carries the documentation the template declared.
///
/// "Ref returns the documentation part ID", which API Gateway assigns: the create takes no
/// identifier and answers the `id` it chose, so that id is this resource's physical id.
async fn create_part(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, PART_PROPERTIES)?;
    let api_id = required(resource, REST_API_ID)?;
    let mut request = Map::new();
    let location = stated(resource, PART_LOCATION)
        .ok_or_else(|| missing_property(resource, PART_LOCATION))?;
    request.insert("location".to_string(), member_value(PART_LOCATION, location)?);
    let content = stated(resource, PART_CONTENT)
        .ok_or_else(|| missing_property(resource, PART_CONTENT))?;
    request.insert(
        "properties".to_string(),
        Value::String(document_text(content)),
    );
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &parts_uri(&api_id),
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateDocumentationPart")?;
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: BTreeMap::new(),
    })
}

/// One declared document as the TEXT the call takes it as: a document as its own
/// serialization, a string exactly as written — the same reading a declared model schema
/// travels through ([`super::request_validation`]), since both properties are a JSON document
/// the owning model types as a String.
fn document_text(declared: &Value) -> String {
    match declared {
        Value::String(text) => text.clone(),
        document => document.to_string(),
    }
}

/// The collection the documentation parts of ONE api live in, which is what
/// `CreateDocumentationPart` is bound to.
fn parts_uri(api_id: &str) -> String {
    format!("{}/documentation/parts", rest_api_uri(api_id))
}

/// The URI that addresses ONE part: the api that holds it and the id API Gateway assigned it.
fn part_uri(api_id: &str, part_id: &str) -> String {
    format!("{}/{}", parts_uri(api_id), percent_encoded(part_id))
}

// ---------------------------------------------------------------------------
// AWS::ApiGateway::DocumentationVersion
// ---------------------------------------------------------------------------

/// Every property `AWS::ApiGateway::DocumentationVersion` publishes.
///
/// Source: the type's own property set, against `CreateDocumentationVersion`'s members
/// (`smithy_query(query="operation",operation="CreateDocumentationVersion")`): the `@required`
/// `documentationVersion`, the `description`, and the `stageName` the snapshot is published to,
/// with the api addressing the call.
const VERSION_PROPERTIES: &[&str] = &[
    VERSION_DESCRIPTION,
    VERSION_NAME,
    REST_API_ID,
    VERSION_STAGE_NAME,
];

/// The property naming the version itself, which that create declares `@required` and which
/// every call on one takes as its `@httpLabel` (`GetDocumentationVersion$documentationVersion`).
///
/// The one spelling of it this crate has: the naming rules read the same property off a
/// declaration to compute the identifier the snapshot is published under
/// ([`apigateway_documentation_version_identity`]).
const VERSION_NAME: &str = APIGATEWAY_DOCUMENTATION_VERSION_PROPERTY;

/// The property describing the snapshot, which that create takes as its `description`.
const VERSION_DESCRIPTION: &str = "Description";

/// The property naming the stage the snapshot is published to, which that create takes as its
/// `stageName` ("the stage name to be associated with the new documentation snapshot").
const VERSION_STAGE_NAME: &str = "StageName";

/// The properties that ARE `CreateDocumentationVersion`'s own body members, as
/// `(property, member)` pairs: each is the property with a lower-cased initial, the api
/// addressing the call instead.
const VERSION_MEMBERS: &[(&str, &str)] = &[
    (VERSION_NAME, "documentationVersion"),
    (VERSION_DESCRIPTION, "description"),
    (VERSION_STAGE_NAME, "stageName"),
];

/// The properties a version's own read reports, under this type's property names.
///
/// The version and its description: the STAGE is left out because the read answers the
/// snapshot rather than the stage it was published to, so there is nothing of that property to
/// compare.
const VERSION_REPORTED: &[(&str, &str)] = &[
    (VERSION_NAME, "version"),
    (VERSION_DESCRIPTION, "description"),
];

/// Create the snapshot an `AWS::ApiGateway::DocumentationVersion` declares on the api it names,
/// so a client that deployed the stack reads it back through `GetDocumentationVersion` — and so
/// a stage naming that version really serves the documentation it snapshotted.
///
/// The VERSION is the identity: API Gateway keys a snapshot by the version under the api that
/// holds it, and `CreateDocumentationVersion$documentationVersion` is the identifier every call
/// on one takes — the service assigns nothing for one — so the resource is published under that
/// version beside the api ([`apigateway_documentation_version_identity`]), which is what `Ref`
/// resolves to.
async fn create_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, VERSION_PROPERTIES)?;
    let api_id = required(resource, REST_API_ID)?;
    let version = required(resource, VERSION_NAME)?;
    let mut request = Map::new();
    for (property, member) in VERSION_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &versions_uri(&api_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(apigateway_documentation_version_identity(
            &version, &api_id,
        )),
        attributes: BTreeMap::new(),
    })
}

/// The collection the documentation versions of ONE api live in, which is what
/// `CreateDocumentationVersion` is bound to.
fn versions_uri(api_id: &str) -> String {
    format!("{}/documentation/versions", rest_api_uri(api_id))
}

/// The URI that addresses ONE version: the api that holds it and the version's own name.
fn version_uri(api_id: &str, version: &str) -> String {
    format!("{}/{}", versions_uri(api_id), percent_encoded(version))
}

// ---------------------------------------------------------------------------
// The seam [`super`] reaches these arms through
// ---------------------------------------------------------------------------

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
        Kind::Part => create_part(ctx, region, resource).await,
        Kind::Version => create_version(ctx, region, resource).await,
    }
}

/// The URI that addresses one of these resources, which is the one its own read and its own
/// delete share — built from this resource's identity and the api that holds it.
///
/// A SNAPSHOT is addressed by the version half of its identifier ([`addressed_version`]), the
/// api half of it being what the URI the snapshot hangs off is built from.
fn addressed(kind: Kind, resource: &StoredResource) -> Result<String, Refusal> {
    let api_id = required(resource, REST_API_ID)?;
    let id = own_id(resource)?;
    Ok(match kind {
        Kind::Part => part_uri(&api_id, id),
        Kind::Version => version_uri(&api_id, addressed_version(id)),
    })
}

/// The VERSION half of a snapshot's identifier: the `documentationVersion` every call on one
/// takes, read through the one reading of such an identifier this crate has
/// ([`apigateway_identity_part`]).
fn addressed_version(identifier: &str) -> &str {
    apigateway_identity_part(identifier, APIGATEWAY_DOCUMENTATION_VERSION_SEPARATOR)
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
        Kind::Part => PART_REPORTED,
        Kind::Version => VERSION_REPORTED,
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
                APIGATEWAY_DOCUMENTATION_PART,
                PART_PROPERTIES,
                vec![PART_LOCATION, PART_CONTENT, REST_API_ID],
            ),
            (
                APIGATEWAY_DOCUMENTATION_VERSION,
                VERSION_PROPERTIES,
                VERSION_MEMBERS
                    .iter()
                    .map(|(property, _)| *property)
                    .chain([REST_API_ID])
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
    /// holds it and its own identity — the id API Gateway assigned a part, the version a
    /// snapshot is named by.
    #[test]
    fn a_resource_is_addressed_by_the_uri_its_operations_bind() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                "a documentation part",
                APIGATEWAY_DOCUMENTATION_PART,
                "p9k2m1",
                "/restapis/a1b2c3/documentation/parts/p9k2m1",
            ),
            (
                "a documentation version",
                APIGATEWAY_DOCUMENTATION_VERSION,
                "2024-05-01|a1b2c3",
                "/restapis/a1b2c3/documentation/versions/2024-05-01",
            ),
            (
                // A version is a caller's own text, so one carrying URI structure is still ONE
                // path segment: an unescaped separator would address another resource.
                "a version carrying a separator",
                APIGATEWAY_DOCUMENTATION_VERSION,
                "v1/draft|a1b2c3",
                "/restapis/a1b2c3/documentation/versions/v1%2Fdraft",
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

    /// A declared documentation content travels as TEXT: a document as its own serialization, a
    /// string exactly as written.
    #[test]
    fn declared_content_travels_as_the_text_the_create_takes() {
        let rows: &[(&str, Value, &str)] = &[
            (
                "a declared document",
                json!({ "description": "the orders collection" }),
                r#"{"description":"the orders collection"}"#,
            ),
            (
                "a declared string",
                json!("{\"description\":\"written as text\"}"),
                r#"{"description":"written as text"}"#,
            ),
        ];
        for (case, content, expected) in rows {
            let text = document_text(content);
            assert_eq!(
                text, *expected,
                "case {case:?}: expected the content text {expected:?}, got {text:?}"
            );
        }
    }

    /// A declaration that states none of a member its create declares `@required` is refused
    /// naming the PROPERTY, rather than a part being created with less than it asked for.
    #[test]
    fn a_part_declaring_no_location_or_content_is_refused() {
        let rows: &[(&str, Value, &str)] = &[
            (
                "no location",
                json!({ "RestApiId": "a1b2c3", PART_CONTENT: "{}" }),
                PART_LOCATION,
            ),
            (
                "no content",
                json!({ "RestApiId": "a1b2c3", PART_LOCATION: { "Type": "API" } }),
                PART_CONTENT,
            ),
        ];
        for (case, properties, named) in rows {
            let resource = declared(
                APIGATEWAY_DOCUMENTATION_PART,
                "stack-Part-7QF3",
                properties.clone(),
            );
            let missing = PART_PROPERTIES
                .iter()
                .filter(|property| **property != REST_API_ID)
                .find(|property| stated(&resource, property).is_none())
                .map(|property| missing_property(&resource, property));
            let Some(refusal) = missing else {
                panic!("case {case:?}: expected a refusal naming {named:?}, got none");
            };
            assert!(
                refusal.message.contains(named),
                "case {case:?}: expected a refusal naming {named:?}, got {refusal:?}"
            );
        }
    }
}
