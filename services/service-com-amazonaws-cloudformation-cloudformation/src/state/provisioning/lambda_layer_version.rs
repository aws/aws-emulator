// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::LayerVersion`: the layer version a stack declares, published in AWS Lambda
//! itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records,
//! and is reached through the same seam ([`super::Owner`]).
//!
//! The type's properties are `PublishLayerVersion`'s own members under the same names —
//! `Content`, `Description`, `CompatibleRuntimes`, `CompatibleArchitectures` and
//! `LicenseInfo` — beside the `LayerName` that ADDRESSES the call rather than travelling in
//! its body. A declaration that names no layer is published under the identifier
//! CloudFormation computed for the resource, which is the name a generated one takes.
//!
//! ## The identifier is Lambda's
//!
//! "Ref returns the ARN of the layer version", and that ARN carries a VERSION NUMBER: "each
//! time you call PublishLayerVersion with the same layer name, a new version is created", so
//! the number — and therefore the ARN — is Lambda's to assign and exists only once the
//! publish has answered. The create reports it as the resource's physical id, which is what
//! [`super::reconcile`] re-points every reference to this resource at
//! ([`super::repoint_references`]), so a function whose `Layers` names the layer reaches
//! Lambda carrying the ARN that service published rather than a stand-in nothing resolves.
//!
//! A layer version cannot be CHANGED: Lambda publishes no call that edits one, and every
//! property of the type requires the resource to be replaced. So there is no update arm —
//! the declaration a deployment changes is reported as not applied rather than as a success
//! nothing carried out — and nothing of a published version can have drifted from its
//! declaration either.

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Refusal};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_LAYER_VERSION: &str = "AWS::Lambda::LayerVersion";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The error code Lambda answers for a layer version it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says a delete has nothing
/// left to remove.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The property naming the layer the version is published under, which addresses the call
/// (`PublishLayerVersion`'s `LayerName` is an `@httpLabel`) rather than travelling in its
/// body.
const LAYER_NAME_PROPERTY: &str = "LayerName";

/// The member the publish answers the version's own ARN under.
const LAYER_VERSION_ARN_MEMBER: &str = "LayerVersionArn";

/// Every property `AWS::Lambda::LayerVersion` publishes. A declared property this list does
/// not carry is REFUSED rather than dropped, so a layer version is never reported published
/// with less than its declaration asked for.
///
/// Source: the type's own property set, which is `PublishLayerVersion`'s member list
/// (`smithy_query(query="operation",operation="PublishLayerVersion")`).
const LAYER_PROPERTIES: &[&str] = &[
    "CompatibleArchitectures",
    "CompatibleRuntimes",
    "Content",
    "Description",
    "LayerName",
    "LicenseInfo",
];

/// The properties that ARE `PublishLayerVersion`'s own body members under the same name, so
/// the declared value travels into the request as written.
const PUBLISH_MEMBERS: &[&str] = &[
    "CompatibleArchitectures",
    "CompatibleRuntimes",
    "Content",
    "Description",
    "LicenseInfo",
];

/// Publish the version an `AWS::Lambda::LayerVersion` declares, in AWS Lambda itself — so a
/// function the same stack declares really has the layer in its execution environment, and a
/// client that deployed the stack reads the version back through `lambda get-layer-version`.
///
/// `Err` is Lambda's own refusal of the publish, which is what the resource fails with: a
/// `Content` that names no archive, a layer name it will not take, a quota the archive would
/// take the account past.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let layer = layer_name(resource);
    let mut request = Map::new();
    for member in PUBLISH_MEMBERS {
        if let Some(value) = resource
            .properties
            .get(*member)
            .filter(|value| !value.is_null())
        {
            request.insert((*member).to_string(), value.clone());
        }
    }
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        &versions_uri(&layer),
        Some(&Value::Object(request)),
    )
    .await?;
    // The identity in force is the one the publish ANSWERED with: the version number it
    // allocated is no value CloudFormation could have computed.
    let arn = answered
        .get(LAYER_VERSION_ARN_MEMBER)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InvalidRequest".to_string(),
            message: format!(
                "AWS Lambda reported no {LAYER_VERSION_ARN_MEMBER} for the version it published \
                 under {layer}, which is what a reference to this resource resolves to"
            ),
        })?;
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        ..Created::default()
    })
}

/// Delete the version a stack published, addressed by the layer and the version number its
/// ARN — this resource's physical id — carries.
///
/// A version Lambda no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    // The id is read the way Lambda itself reads a layer version ARN, so the two can never
    // disagree about which version one names. An id that is no published version's ARN
    // names nothing this delete could remove — a create that never answered, or a layer
    // Lambda itself provides, which belongs to no account.
    let Some(::service_com_amazonaws_lambda_awsgirapiservice::state::LayerVersionReference::Published {
        name,
        version,
        ..
    }) = ::service_com_amazonaws_lambda_awsgirapiservice::state::parse_layer_version_arn(
        &resource.physical_resource_id,
    ) else {
        return Ok(());
    };
    let uri = format!("{}/{version}", versions_uri(&name));
    match call_rest_json(ctx, LAMBDA_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The layer the version is published under: the one the declaration names, or the
/// identifier CloudFormation computed for the resource where it names none — "LayerName:
/// the name or Amazon Resource Name (ARN) of the layer" is not a required property, and a
/// resource the template does not name is named by CloudFormation.
fn layer_name(resource: &StoredResource) -> String {
    match resource
        .properties
        .get(LAYER_NAME_PROPERTY)
        .and_then(Value::as_str)
    {
        Some(declared) if !declared.is_empty() => declared.to_string(),
        _ => resource.physical_resource_id.clone(),
    }
}

/// The URI the versions of ONE layer bind: the `@httpLabel` carrying the layer,
/// percent-encoded as a non-greedy label is sent.
fn versions_uri(layer: &str) -> String {
    format!("/2018-10-31/layers/{}/versions", percent_encoded(layer))
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a version from being reported as published with less than its
/// declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if LAYER_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource record of this type, as the naming rules leave one.
    fn declared(properties: Value, physical_resource_id: &str) -> StoredResource {
        StoredResource {
            logical_id: "Shared".to_string(),
            resource_type: LAMBDA_LAYER_VERSION.to_string(),
            physical_resource_id: physical_resource_id.to_string(),
            attributes: std::collections::BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every member the arm sends is a published property of the type, and the one property
    /// that is not a body member is the one that addresses the call.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in PUBLISH_MEMBERS {
            assert!(
                LAYER_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {LAMBDA_LAYER_VERSION}, got none"
            );
        }
        for property in LAYER_PROPERTIES {
            let placed =
                PUBLISH_MEMBERS.contains(property) || *property == LAYER_NAME_PROPERTY;
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member or the one that \
                 addresses the call, got neither"
            );
        }
    }

    /// The layer the publish addresses is the declared name where there is one, and the
    /// identifier CloudFormation computed where the declaration names none.
    #[test]
    fn the_declared_layer_name_addresses_the_publish() {
        let cases: &[(&str, Value, &str)] = &[
            (
                "a declared name",
                serde_json::json!({ "LayerName": "shared-deps" }),
                "shared-deps",
            ),
            (
                "no name at all",
                serde_json::json!({ "Content": { "S3Bucket": "b", "S3Key": "k" } }),
                "stack-Shared-ABC123",
            ),
            (
                "an empty name",
                serde_json::json!({ "LayerName": "" }),
                "stack-Shared-ABC123",
            ),
        ];
        for (case, properties, expected) in cases {
            let resource = declared(properties.clone(), "stack-Shared-ABC123");
            let named = layer_name(&resource);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected the publish to address {expected:?}, got {named:?}"
            );
        }
    }

    /// A property the type does not publish is refused rather than dropped.
    #[test]
    fn an_unpublished_property_is_refused() {
        let resource = declared(
            serde_json::json!({ "LayerName": "shared", "RetentionPolicy": "Retain" }),
            "stack-Shared-ABC123",
        );
        let refused = refuse_unmapped_properties(&resource);
        assert!(
            refused.is_err(),
            "case {:?}: expected a refusal naming the unpublished property, got {refused:?}",
            "RetentionPolicy"
        );
    }
}
