// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::Alias`: the alias a stack declares, created in AWS Lambda itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records,
//! and is reached through the same seam ([`super::Owner`]).
//!
//! The type's properties are `CreateAlias`'s own members under the same names — `Name`,
//! `FunctionVersion`, `Description` and `RoutingConfig` — beside the `FunctionName` that
//! ADDRESSES the call rather than travelling in its body, and
//! `ProvisionedConcurrencyConfig`, which is no member of the create at all: it is the
//! `PutProvisionedConcurrencyConfig` a client makes next on the alias just created, so an
//! alias declared with one is configured with it rather than created without it.
//!
//! "Ref returns the alias ARN", which is the function's ARN qualified by the alias name, and
//! that is the ARN `CreateAlias` answers: the create reports it as the resource's physical
//! id, so the handle the stack publishes is the one `lambda get-alias` resolves.
//!
//! ## Lambda decides whether the alias can exist
//!
//! "The function version that the alias invokes" has to be a version the function HAS:
//! Lambda refuses a `CreateAlias` and an `UpdateAlias` naming one it does not hold with
//! `ResourceNotFoundException`. That refusal is the resource's failure — the alias was not
//! created, and was not changed — so a deployment pointing an alias at a version that does
//! not exist fails the resource and the stack rather than reporting a deploy nothing carried
//! out. Which is why this module exists at all: it is Lambda, not CloudFormation, that knows
//! which versions a function has.
//!
//! The alias's NAME and the FUNCTION it belongs to are its identity — neither can change on
//! an alias that exists — so a declaration that states either differently is a different
//! resource; everything else about an alias is applied in place by `UpdateAlias`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Refusal};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_ALIAS: &str = "AWS::Lambda::Alias";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The collection the aliases of ONE function bind:
/// `POST /2015-03-31/functions/{FunctionName}/aliases`.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The error code Lambda answers for an alias it does not hold (`ResourceNotFoundException`,
/// httpError 404), which is what says a delete has nothing left to remove.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The property naming the function the alias belongs to, which addresses the call
/// (`CreateAlias`'s `FunctionName` is an `@httpLabel`) rather than travelling in its body.
const FUNCTION_NAME_PROPERTY: &str = "FunctionName";

/// The property naming the alias itself, which is a body member of the create and an
/// `@httpLabel` of every later call on it.
const NAME_PROPERTY: &str = "Name";

/// The property that is no member of the create: the call of its own the module
/// documentation names.
const PROVISIONED_CONCURRENCY_PROPERTY: &str = "ProvisionedConcurrencyConfig";

/// The member the create and the update answer the alias's own ARN under, which is this
/// resource's physical id and the `AliasArn` attribute the type publishes.
pub(super) const ALIAS_ARN_MEMBER: &str = "AliasArn";

/// Every property `AWS::Lambda::Alias` publishes. A declared property this list does not
/// carry is REFUSED rather than dropped, so an alias is never reported created with less
/// than its declaration asked for.
///
/// Source: the type's own property set, which is `CreateAlias`'s member list
/// (`smithy_query(query="operation",operation="CreateAlias")`) plus the one property that is
/// another call's member.
const ALIAS_PROPERTIES: &[&str] = &[
    "Description",
    "FunctionName",
    "FunctionVersion",
    "Name",
    "ProvisionedConcurrencyConfig",
    "RoutingConfig",
];

/// The properties that ARE `CreateAlias`'s own body members under the same name, so the
/// declared value travels into the request as written.
const CREATE_MEMBERS: &[&str] = &["Description", "FunctionVersion", "Name", "RoutingConfig"];

/// The members `UpdateAlias` takes for an alias that exists: everything the create takes
/// but the alias's own name, which addresses that call instead.
const UPDATE_MEMBERS: &[&str] = &["Description", "FunctionVersion", "RoutingConfig"];

/// Create the alias an `AWS::Lambda::Alias` declares, in AWS Lambda itself — so a client
/// that deployed the stack invokes the function through the alias the template published.
///
/// `Err` is Lambda's own refusal of the create, which is what the resource fails with: a
/// function it does not hold, a `FunctionVersion` that function never published, an alias
/// name already taken.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let function = declared_function(resource)?;
    let mut request = Map::new();
    for member in CREATE_MEMBERS {
        if let Some(value) = declared(&resource.properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    // "Ref returns the alias name" is NOT what this type publishes — the physical id is the
    // ARN — but the alias's name is still CloudFormation's to decide where the declaration
    // states none, and the identifier computed for the resource is that name.
    request
        .entry(NAME_PROPERTY.to_string())
        .or_insert_with(|| Value::String(computed_alias_name(resource)));
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        &aliases_uri(&function),
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = reported_text(&answered, ALIAS_ARN_MEMBER).ok_or_else(missing_arn)?;
    let name = reported_text(&answered, NAME_PROPERTY)
        .unwrap_or_else(|| computed_alias_name(resource));

    // The property that is a call of its own, made on the alias that now exists. A refusal
    // of it leaves the alias configured differently from its declaration, which is not a
    // resource the stack may report as created — so the alias is removed again before the
    // refusal is reported, exactly as a function whose declared concurrency was refused is
    // ([`super::lambda_function`]).
    if let Err(refusal) = configure(ctx, region, &resource.properties, &function, &name).await {
        let _ = delete_alias(ctx, region, &function, &name).await;
        return Err(refusal);
    }
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes: attributes_of(&resource.resource_type, &arn),
    })
}

/// Apply a CHANGED declaration to the alias the stack already has, through Lambda's own
/// `UpdateAlias` — the call a client makes for the same change — and re-apply the property
/// that is a call of its own ([`configure`]).
///
/// The alias's NAME and the function it belongs to are its identity, and a declaration that
/// changes either is a different resource: [`super::super::template`] computes a different
/// physical id for it, so the deployment replaces the alias rather than reaching here.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared_properties: &Value,
) -> Result<Created, Refusal> {
    let asked = StoredResource {
        properties: declared_properties.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(&asked)?;
    let (function, name) = addressed(resource, declared_properties)?;
    let mut request = Map::new();
    for member in UPDATE_MEMBERS {
        if let Some(value) = declared(declared_properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    let answered = call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "PUT",
        &alias_uri(&function, &name),
        Some(&Value::Object(request)),
    )
    .await?;
    configure(ctx, region, declared_properties, &function, &name).await?;
    let arn = reported_text(&answered, ALIAS_ARN_MEMBER)
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    Ok(Created::attributes(attributes_of(
        &resource.resource_type,
        &arn,
    )))
}

/// Delete the alias a stack created, addressed by the function and the alias name its ARN —
/// this resource's physical id — carries.
///
/// An alias Lambda no longer holds is already in the state the delete asks for, judged on
/// the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (function, name) = addressed(resource, &resource.properties)?;
    delete_alias(ctx, region, &function, &name).await
}

/// The alias's configuration as Lambda reports it, for drift detection: the members
/// `GetAlias` answers under the same names the type publishes as properties. An alias that
/// service no longer holds was deleted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (function, name) = addressed(resource, &resource.properties)?;
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &alias_uri(&function, &name),
        None,
    )
    .await
    {
        Ok(answered) => {
            let mut reported = Map::new();
            for member in ["Name", "FunctionVersion", "Description", "RoutingConfig"] {
                if let Some(value) = declared(&answered, member) {
                    reported.insert(member.to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply the declared property no member of the create carries, through the Lambda call that
/// owns it, on the alias itself: "Qualifier … the version number or alias name". A
/// declaration stating none makes no call.
async fn configure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    function: &str,
    name: &str,
) -> Result<(), Refusal> {
    let Some(Value::Object(config)) = declared(properties, PROVISIONED_CONCURRENCY_PROPERTY) else {
        return Ok(());
    };
    call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "PUT",
        &format!(
            "/2019-09-30/functions/{}/provisioned-concurrency?Qualifier={}",
            percent_encoded(function),
            percent_encoded(name)
        ),
        Some(&Value::Object(config.clone())),
    )
    .await?;
    Ok(())
}

/// `DeleteAlias` on one alias, with an alias Lambda no longer holds read as the deletion
/// this asked for.
async fn delete_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    function: &str,
    name: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "DELETE",
        &alias_uri(function, name),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The function and the alias name every call after the create is addressed by.
///
/// Read from the ARN the create ANSWERED where the physical id is one — that ARN is Lambda's
/// own identifier for the alias, so the two can never disagree about which alias one names —
/// and otherwise from the declaration, which is all a resource whose create never answered
/// has.
fn addressed(
    resource: &StoredResource,
    declared_properties: &Value,
) -> Result<(String, String), Refusal> {
    if let Some(reference) =
        ::service_com_amazonaws_lambda_awsgirapiservice::state::parse_function_reference(
            &resource.physical_resource_id,
        )
        .filter(|reference| reference.was_arn)
    {
        if let Some(name) = reference.qualifier {
            return Ok((reference.name, name));
        }
    }
    let function = declared(declared_properties, FUNCTION_NAME_PROPERTY)
        .and_then(Value::as_str)
        .filter(|function| !function.is_empty())
        .map(ToString::to_string);
    let function = match function {
        Some(function) => function,
        None => declared_function(resource)?,
    };
    let name = declared(declared_properties, NAME_PROPERTY)
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map_or_else(|| computed_alias_name(resource), ToString::to_string);
    Ok((function, name))
}

/// The function the alias belongs to, as the declaration names it.
///
/// `FunctionName` is a REQUIRED property of the type ("the name or ARN of the Lambda
/// function"), so a declaration carrying none names no function and is refused rather than
/// sent to a call that cannot be addressed.
fn declared_function(resource: &StoredResource) -> Result<String, Refusal> {
    match declared(&resource.properties, FUNCTION_NAME_PROPERTY).and_then(Value::as_str) {
        Some(function) if !function.is_empty() => Ok(function.to_string()),
        _ => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!("{LAMBDA_ALIAS} requires the {FUNCTION_NAME_PROPERTY} property"),
        }),
    }
}

/// The alias name CloudFormation computed for a declaration that states none: the physical
/// NAME of the resource, which the naming rules took from the declared `Name` where there
/// was one and generated otherwise.
fn computed_alias_name(resource: &StoredResource) -> String {
    ::service_com_amazonaws_lambda_awsgirapiservice::state::parse_function_reference(
        &resource.physical_resource_id,
    )
    .and_then(|reference| reference.qualifier)
    .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// The attributes an alias publishes: its own ARN, under the `AliasArn` the type names and
/// the `Arn`/`<Type>Arn` every type answers ([`super::super::template::arn_attributes`]).
fn attributes_of(resource_type: &str, arn: &str) -> BTreeMap<String, String> {
    let mut attributes: BTreeMap<String, String> =
        super::super::template::arn_attributes(resource_type, arn)
            .into_iter()
            .collect();
    attributes.insert(ALIAS_ARN_MEMBER.to_string(), arn.to_string());
    attributes
}

/// The URI the aliases of ONE function bind.
fn aliases_uri(function: &str) -> String {
    format!("{FUNCTIONS_URI}/{}/aliases", percent_encoded(function))
}

/// The URI that addresses ONE alias of one function.
fn alias_uri(function: &str, name: &str) -> String {
    format!("{}/{}", aliases_uri(function), percent_encoded(name))
}

/// One declared property, as the template evaluated it.
fn declared<'a>(properties: &'a Value, name: &str) -> Option<&'a Value> {
    properties.get(name).filter(|value| !value.is_null())
}

/// One member of an answer read as TEXT.
fn reported_text(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The refusal for a create that answered without the ARN this resource's identity is: a
/// reference to the resource would have nothing to resolve to.
fn missing_arn() -> Refusal {
    Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "AWS Lambda reported no {ALIAS_ARN_MEMBER} for the alias it created, which is what a \
             reference to this resource resolves to"
        ),
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps an alias from being reported as created with less than its
/// declaration asked for.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if ALIAS_PROPERTIES.contains(&property.as_str()) {
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

    /// A resource record of this type, as the naming rules and a create leave one.
    fn alias(properties: Value, physical_resource_id: &str) -> StoredResource {
        StoredResource {
            logical_id: "FnAlias".to_string(),
            resource_type: LAMBDA_ALIAS.to_string(),
            physical_resource_id: physical_resource_id.to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every member the create sends is a published property of the type, every member the
    /// update sends is one the create sends, and every property that is not a body member is
    /// either the one that addresses the call or the one that is a call of its own.
    #[test]
    fn every_published_property_has_a_place() {
        for member in CREATE_MEMBERS {
            assert!(
                ALIAS_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {LAMBDA_ALIAS}, got none"
            );
        }
        for member in UPDATE_MEMBERS {
            assert!(
                CREATE_MEMBERS.contains(member),
                "case {member:?}: expected the updated member to be one the create takes, got \
                 none"
            );
        }
        for property in ALIAS_PROPERTIES {
            let placed = CREATE_MEMBERS.contains(property)
                || *property == FUNCTION_NAME_PROPERTY
                || *property == PROVISIONED_CONCURRENCY_PROPERTY;
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member, the one that \
                 addresses the call or a call of its own, got none of them"
            );
        }
    }

    /// Every call after the create is addressed by the function and the alias name the ARN
    /// the create answered carries; a resource with no such ARN falls back to what its
    /// declaration states.
    #[test]
    fn the_answered_arn_addresses_every_later_call() {
        let declaration = serde_json::json!({
            "FunctionName": "orders",
            "Name": "live",
            "FunctionVersion": "3",
        });
        let cases: &[(&str, &str, (&str, &str))] = &[
            (
                "the ARN the create answered",
                "arn:aws:lambda:us-east-1:000000000000:function:orders-fn:blue",
                ("orders-fn", "blue"),
            ),
            (
                "a record whose create never answered",
                "stack-FnAlias-ABC123",
                ("orders", "live"),
            ),
        ];
        for (case, physical_resource_id, (function, name)) in cases {
            let resource = alias(declaration.clone(), physical_resource_id);
            let addressed = addressed(&resource, &resource.properties);
            assert_eq!(
                addressed.as_ref().map(|(f, n)| (f.as_str(), n.as_str())).ok(),
                Some((*function, *name)),
                "case {case:?}: expected the call to address {function:?}/{name:?}, got \
                 {addressed:?}"
            );
        }
    }

    /// A property the type does not publish is refused rather than dropped.
    #[test]
    fn an_unpublished_property_is_refused() {
        let resource = alias(
            serde_json::json!({ "FunctionName": "orders", "Name": "live", "Weighted": true }),
            "stack-FnAlias-ABC123",
        );
        let refused = refuse_unmapped_properties(&resource);
        assert!(
            refused.is_err(),
            "case {:?}: expected a refusal naming the unpublished property, got {refused:?}",
            "Weighted"
        );
    }
}
