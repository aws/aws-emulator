// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::Version`: the function version a stack declares, published in AWS Lambda
//! itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records,
//! and is reached through the same seam ([`super::Owner`]).
//!
//! The type's properties are `PublishVersion`'s own members under the same names —
//! `CodeSha256` and `Description` — beside the `FunctionName` that ADDRESSES the call rather
//! than travelling in its body, and the two properties that are calls of their own on the
//! version just published (`ProvisionedConcurrencyConfig` is
//! `PutProvisionedConcurrencyConfig`, `RuntimePolicy` is `PutRuntimeManagementConfig`), so a
//! version declared with one is configured with it rather than published without it.
//!
//! ## The identifier is Lambda's
//!
//! "Ref returns the ARN of the function version", and that ARN carries a VERSION NUMBER:
//! `PublishVersion` "creates a version from the current code and configuration of a
//! function", and the number it allocates is no value CloudFormation could have computed.
//! So the create reports the ARN Lambda answered as the resource's physical id and the
//! number as the `Version` attribute, and [`super::reconcile`] re-points every reference to
//! this resource at both ([`super::repoint_references`]) — which is how an
//! `AWS::Lambda::Alias` whose `FunctionVersion` is `Fn::GetAtt` of this resource reaches
//! Lambda carrying the version that service published rather than a stand-in nothing
//! resolves.
//!
//! A published version cannot be CHANGED: its code and configuration are the snapshot the
//! publish took, and Lambda publishes no call that edits one. So there is no update arm for
//! the members of the publish — the declaration a deployment changes is reported as not
//! applied rather than as a success nothing carried out — and nothing of the snapshot can
//! have drifted from its declaration either.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Refusal, Removal};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_VERSION: &str = "AWS::Lambda::Version";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The collection `PublishVersion` and `DeleteFunction` address a function's versions
/// through: `POST /2015-03-31/functions/{FunctionName}/versions`.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The error code Lambda answers for a function, or a version of one, it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says a delete has nothing
/// left to remove.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The property naming the function the version is published FROM, which addresses the call
/// (`PublishVersion`'s `FunctionName` is an `@httpLabel`) rather than travelling in its body.
const FUNCTION_NAME_PROPERTY: &str = "FunctionName";

/// The two properties no member of the publish carries, each applied by the Lambda call that
/// owns it once the version exists ([`configure`]).
const PROVISIONED_CONCURRENCY_PROPERTY: &str = "ProvisionedConcurrencyConfig";
const RUNTIME_POLICY_PROPERTY: &str = "RuntimePolicy";

/// The member Lambda answers the version NUMBER it allocated under, which is also the name
/// of the attribute the type publishes for it.
pub(super) const VERSION_MEMBER: &str = "Version";

/// The member Lambda answers the version's own ARN under — the function's ARN qualified by
/// that number — which is both this resource's physical id and the `FunctionArn` attribute
/// the type publishes.
pub(super) const FUNCTION_ARN_MEMBER: &str = "FunctionArn";

/// Every property `AWS::Lambda::Version` publishes. A declared property this list does not
/// carry is REFUSED rather than dropped, so a version is never reported published with less
/// than its declaration asked for.
///
/// Source: the type's own property set, which is `PublishVersion`'s member list
/// (`smithy_query(query="operation",operation="PublishVersion")`) plus the two properties
/// that are other calls' members.
const VERSION_PROPERTIES: &[&str] = &[
    "CodeSha256",
    "Description",
    "FunctionName",
    "ProvisionedConcurrencyConfig",
    "RuntimePolicy",
];

/// The properties that ARE `PublishVersion`'s own body members under the same name, so the
/// declared value travels into the request as written.
const PUBLISH_MEMBERS: &[&str] = &["CodeSha256", "Description"];

/// Publish the version an `AWS::Lambda::Version` declares, in AWS Lambda itself — so an
/// alias the same stack declares really invokes a version that exists, and a client that
/// deployed the stack reads it back through `lambda list-versions-by-function`.
///
/// `Err` is Lambda's own refusal of the publish, which is what the resource fails with: a
/// function it does not hold, a `CodeSha256` that is not the function's current one, a
/// version quota the publish would take the function past.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let function = declared_function(resource)?;
    let mut request = Map::new();
    for member in PUBLISH_MEMBERS {
        if let Some(value) = declared(resource, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        &format!("{}/versions", function_uri(&function)),
        Some(&Value::Object(request)),
    )
    .await?;
    // The identity in force is the one the publish ANSWERED with: the number it allocated is
    // no value CloudFormation could have computed, and the ARN carries it.
    let version = reported_text(&answered, VERSION_MEMBER).ok_or_else(|| missing(VERSION_MEMBER))?;
    let arn =
        reported_text(&answered, FUNCTION_ARN_MEMBER).ok_or_else(|| missing(FUNCTION_ARN_MEMBER))?;

    // The two properties that are calls of their own, made on the version that now exists. A
    // refusal of one of them leaves the version configured differently from its declaration,
    // which is not a resource the stack may report as created — so the version is removed
    // again before the refusal is reported, exactly as a function whose declared concurrency
    // was refused is ([`super::lambda_function`]).
    if let Err(refusal) = configure(ctx, region, resource, &function, &version).await {
        let _ = delete_version(ctx, region, &arn).await;
        return Err(refusal);
    }

    let mut attributes: BTreeMap<String, String> = super::super::template::arn_attributes(
        &resource.resource_type,
        &arn,
    )
    .into_iter()
    .collect();
    attributes.insert(VERSION_MEMBER.to_string(), version);
    attributes.insert(FUNCTION_ARN_MEMBER.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the version a stack published, addressed by the ARN its publish answered — this
/// resource's physical id, which carries both the function and the version number.
///
/// A version Lambda no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_version(ctx, region, &resource.physical_resource_id).await
}

/// Whether Lambda has finished removing the version a delete was accepted for.
///
/// `DeleteFunction` with the version's own qualifier takes the version away, so there is
/// nothing to wait for; the read is what says so rather than an assumption about it.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &format!("{}/configuration", function_uri(physical_resource_id)),
        None,
    )
    .await
    {
        Ok(answered) => Ok(Removal::Pending(
            reported_text(&answered, "State").unwrap_or_default(),
        )),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(Removal::Gone),
        Err(refusal) => Err(refusal),
    }
}

/// Apply the declared properties no member of the publish carries — the two the module
/// documentation names — each through the Lambda call that owns it, on the version just
/// published. A declaration that states neither makes no call.
async fn configure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    function: &str,
    version: &str,
) -> Result<(), Refusal> {
    // "ProvisionedConcurrencyConfig … specifies a provisioned concurrency configuration for
    // a function's version" (`PutProvisionedConcurrencyConfig`), whose `Qualifier` is the
    // version this resource published.
    if let Some(Value::Object(declared)) = declared(resource, PROVISIONED_CONCURRENCY_PROPERTY) {
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "PUT",
            &format!(
                "/2019-09-30/functions/{}/provisioned-concurrency?Qualifier={}",
                percent_encoded(function),
                percent_encoded(version)
            ),
            Some(&Value::Object(declared.clone())),
        )
        .await?;
    }
    // "RuntimePolicy … the runtime management configuration of a function's version"
    // (`PutRuntimeManagementConfig`), whose members the property states.
    if let Some(Value::Object(declared)) = declared(resource, RUNTIME_POLICY_PROPERTY) {
        call_rest_json(
            ctx,
            LAMBDA_SIGNING_NAME,
            region,
            "PUT",
            &format!(
                "/2021-07-20/functions/{}/runtime-management-config?Qualifier={}",
                percent_encoded(function),
                percent_encoded(version)
            ),
            Some(&Value::Object(declared.clone())),
        )
        .await?;
    }
    Ok(())
}

/// `DeleteFunction` on ONE version, addressed by the qualified ARN the publish answered,
/// with a version Lambda no longer holds read as the deletion this asked for.
async fn delete_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    qualified: &str,
) -> Result<(), Refusal> {
    // An id that is no published version's ARN names nothing this delete could remove — a
    // publish that never answered, say — so there is nothing to ask Lambda for.
    let names_a_version = ::service_com_amazonaws_lambda_awsgirapiservice::state::parse_function_reference(
        qualified,
    )
    .is_some_and(|reference| reference.qualifier.is_some());
    if !names_a_version {
        return Ok(());
    }
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "DELETE",
        &function_uri(qualified),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The function the version is published from, as the declaration names it.
///
/// `FunctionName` is a REQUIRED property of the type ("the name or ARN of the Lambda
/// function"), so a declaration carrying none names no function to publish from and is
/// refused rather than sent to a call that cannot be addressed.
fn declared_function(resource: &StoredResource) -> Result<String, Refusal> {
    match declared(resource, FUNCTION_NAME_PROPERTY).and_then(Value::as_str) {
        Some(function) if !function.is_empty() => Ok(function.to_string()),
        _ => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!("{LAMBDA_VERSION} requires the {FUNCTION_NAME_PROPERTY} property"),
        }),
    }
}

/// The URI that addresses ONE function: the `@httpLabel` every function operation binds the
/// name to, percent-encoded as a non-greedy label is sent.
fn function_uri(function: &str) -> String {
    format!("{FUNCTIONS_URI}/{}", percent_encoded(function))
}

/// One declared property of the resource, as the template evaluated it.
fn declared<'a>(resource: &'a StoredResource, name: &str) -> Option<&'a Value> {
    resource
        .properties
        .get(name)
        .filter(|value| !value.is_null())
}

/// One member of an answer read as TEXT.
fn reported_text(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The refusal for a publish that answered without the member this resource's identity is
/// read from: without it there is no value a reference to the resource could resolve to.
fn missing(member: &str) -> Refusal {
    Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "AWS Lambda reported no {member} for the version it published, which is what a \
             reference to this resource resolves to"
        ),
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a version from being reported as published with less than its
/// declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if VERSION_PROPERTIES.contains(&property.as_str()) {
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
    fn version(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "FnVersion".to_string(),
            resource_type: LAMBDA_VERSION.to_string(),
            physical_resource_id: "arn:aws:lambda:us-east-1:000000000000:function:fn:7"
                .to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every member the publish sends is a published property of the type, and every
    /// property that is not a body member is either the one that addresses the call or one
    /// of the two that are calls of their own.
    #[test]
    fn every_published_property_has_a_place() {
        for member in PUBLISH_MEMBERS {
            assert!(
                VERSION_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {LAMBDA_VERSION}, got none"
            );
        }
        for property in VERSION_PROPERTIES {
            let placed = PUBLISH_MEMBERS.contains(property)
                || *property == FUNCTION_NAME_PROPERTY
                || *property == PROVISIONED_CONCURRENCY_PROPERTY
                || *property == RUNTIME_POLICY_PROPERTY;
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member, the one that \
                 addresses the call or a call of its own, got none of them"
            );
        }
    }

    /// A declaration naming no function is refused rather than sent to a call that cannot be
    /// addressed, and one naming a function is addressed by it.
    #[test]
    fn the_declared_function_addresses_the_publish() {
        let cases: &[(&str, Value, Option<&str>)] = &[
            (
                "a function named by name",
                serde_json::json!({ "FunctionName": "orders" }),
                Some("orders"),
            ),
            (
                "a function named by ARN",
                serde_json::json!({
                    "FunctionName": "arn:aws:lambda:us-east-1:000000000000:function:orders"
                }),
                Some("arn:aws:lambda:us-east-1:000000000000:function:orders"),
            ),
            (
                "no function at all",
                serde_json::json!({ "Description": "the first cut" }),
                None,
            ),
            ("an empty name", serde_json::json!({ "FunctionName": "" }), None),
        ];
        for (case, properties, expected) in cases {
            let named = declared_function(&version(properties.clone()));
            match expected {
                Some(expected) => assert_eq!(
                    named.as_deref().ok(),
                    Some(*expected),
                    "case {case:?}: expected the publish to address {expected:?}, got {named:?}"
                ),
                None => assert!(
                    named.is_err(),
                    "case {case:?}: expected a refusal naming the required property, got {named:?}"
                ),
            }
        }
    }

    /// A property the type does not publish is refused rather than dropped.
    #[test]
    fn an_unpublished_property_is_refused() {
        let resource = version(serde_json::json!({
            "FunctionName": "orders",
            "RetentionPolicy": "Retain",
        }));
        let refused = refuse_unmapped_properties(&resource);
        assert!(
            refused.is_err(),
            "case {:?}: expected a refusal naming the unpublished property, got {refused:?}",
            "RetentionPolicy"
        );
    }
}
