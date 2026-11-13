// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::Permission`: the statement a stack declares on a function's
//! resource-based policy, added in AWS Lambda itself.
//!
//! The type is no entity of Lambda's own: it is ONE statement of the policy attached to the
//! function it names, which `AddPermission` adds ("Grants a principal permission to use a
//! function") and `RemovePermission` removes. Its properties are that call's own members
//! under the same names, beside the `StatementId` CloudFormation decides — this resource's
//! physical id, which is what `Ref` returns for the type and what the removal addresses the
//! statement by.
//!
//! It lives beside [`super::lambda_function`] and is reached through the same seam
//! ([`super::Owner`]), for the reason that module records.

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Refusal};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_PERMISSION: &str = "AWS::Lambda::Permission";

/// The SigV4 signing name of the service that owns it.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The error code Lambda answers for a function or a statement it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says a removal has nothing
/// left to remove.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The property naming the function the statement is added to, which addresses the call
/// rather than travelling in its body.
const FUNCTION_NAME_PROPERTY: &str = "FunctionName";

/// Every property `AWS::Lambda::Permission` publishes. A declared property this list does
/// not carry is REFUSED rather than dropped, so a grant is never reported as made with less
/// than its declaration asked for.
///
/// Source: the type's own property set, which is `AddPermission`'s member list
/// (`smithy_query(query="operation",operation="AddPermission")`) less the `StatementId`
/// CloudFormation decides and the `RevisionId` a stack states for no statement.
const PERMISSION_PROPERTIES: &[&str] = &[
    "Action",
    "EventSourceToken",
    "FunctionName",
    "FunctionUrlAuthType",
    "Principal",
    "PrincipalOrgID",
    "SourceAccount",
    "SourceArn",
];

/// The properties that ARE `AddPermission`'s own body members under the same name.
const PERMISSION_MEMBERS: &[&str] = &[
    "Action",
    "EventSourceToken",
    "FunctionUrlAuthType",
    "Principal",
    "PrincipalOrgID",
    "SourceAccount",
    "SourceArn",
];

/// Add the statement an `AWS::Lambda::Permission` declares to the policy of the function it
/// names — so a client that deployed the stack and then calls `lambda get-policy` reads the
/// grant back, and a service the grant was made for is really allowed to invoke the
/// function.
///
/// `Err` is Lambda's own refusal: a function it does not hold, a statement id already taken,
/// a principal it will not accept.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource)?;
    let function = function_of(resource)?;
    let mut request = Map::new();
    // The statement id is this resource's physical id: "Ref returns the physical ID of the
    // resource", and that id is what `RemovePermission` addresses the statement by.
    request.insert(
        "StatementId".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    for member in PERMISSION_MEMBERS {
        if let Some(value) = resource
            .properties
            .get(*member)
            .filter(|value| !value.is_null())
        {
            request.insert((*member).to_string(), value.clone());
        }
    }
    call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "POST",
        &policy_uri(&function),
        Some(&Value::Object(request)),
    )
    .await?;
    // The statement publishes no attribute of its own: the type's only return value is the
    // physical id the naming rules computed, which this create carried out as the statement
    // id.
    Ok(Created::default())
}

/// Apply a CHANGED declaration to the statement a stack added, through the two calls Lambda
/// publishes for a policy statement: the statement in force is removed and the DECLARED one
/// is added, so the function's policy ends up holding exactly the grant the declaration
/// states rather than the stack failing while the caller's change went nowhere.
///
/// There is no call that edits a statement in place — `AddPermission` adds one and
/// `RemovePermission` removes one — which is why every property of this type publishes
/// "Update requires: Replacement". The statement is replaced under the same statement id,
/// this resource's physical id: that is the id `Ref` returns for the type and the one the
/// removal addresses the statement by, so the resource keeps the identity the stack
/// published for it.
///
/// The OLD statement is removed from the function the record names, which is the function it
/// was added to; the new one is added to the function the DECLARATION names, so a declaration
/// that moves the grant to another function leaves none behind on the first.
///
/// The removal precedes the add, because both statements are the same statement id and
/// Lambda holds one statement per id. A declared grant the ADD is refused for therefore
/// leaves the function without that statement for the length of this call, so the statement in
/// force is PUT BACK before the refusal is reported: the resource is reported failed with
/// Lambda's own refusal and the function keeps the grant it had, rather than the stack rolling
/// back over a function whose policy quietly lost a statement (a rollback re-declares the
/// configuration in force, which this walk then has nothing to apply for). The two checks that
/// can be made without calling anything — the function the declaration names, and a property
/// this module does not apply — are made before the removal for the same reason.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let declaration = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(&declaration)?;
    // The function the declaration names is resolved BEFORE the statement in force is
    // removed, so a declaration that names none leaves the grant it asked to change alone.
    function_of(&declaration)?;
    delete(ctx, region, resource).await?;
    match create(ctx, region, &declaration).await {
        Ok(created) => Ok(created),
        Err(refusal) => {
            if let Err(restoring) = create(ctx, region, resource).await {
                // Lambda refused to put the statement in force back. Nothing else can
                // restore it, so the resource's own refusal is still what the deployment
                // reports, and this is recorded so the lost statement is diagnosable.
                tracing::warn!(
                    logical_id = %resource.logical_id,
                    statement_id = %resource.physical_resource_id,
                    code = %restoring.code,
                    "the statement in force could not be put back after the declared grant \
                     was refused"
                );
            }
            Err(refusal)
        }
    }
}

/// Remove the statement a stack added, addressed by the function it was added to and the
/// statement id that is this resource's physical id.
///
/// A statement — or a function — Lambda no longer holds is already in the state the removal
/// asks for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let function = function_of(resource)?;
    let uri = format!(
        "{}/{}",
        policy_uri(&function),
        percent_encoded(&resource.physical_resource_id)
    );
    match call_rest_json(ctx, LAMBDA_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The function the statement belongs to, as the declaration names it: a name, a partial
/// ARN or a full ARN, each of which `AddPermission`'s `FunctionName` label admits.
fn function_of(resource: &StoredResource) -> Result<String, Refusal> {
    match resource
        .properties
        .get(FUNCTION_NAME_PROPERTY)
        .and_then(Value::as_str)
    {
        Some(function) if !function.is_empty() => Ok(function.to_string()),
        _ => Err(Refusal {
            code: "InvalidRequest".to_string(),
            message: format!("{LAMBDA_PERMISSION} requires the {FUNCTION_NAME_PROPERTY} property"),
        }),
    }
}

/// The URI the policy operations of ONE function bind: the `@httpLabel` carrying the
/// function, percent-encoded as a non-greedy label is sent.
fn policy_uri(function: &str) -> String {
    format!(
        "/2015-03-31/functions/{}/policy",
        percent_encoded(function)
    )
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if PERMISSION_PROPERTIES.contains(&property.as_str()) {
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

    /// Every member the arm sends is a published property of the type, and the one property
    /// that is not a body member is the one that addresses the call.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in PERMISSION_MEMBERS {
            assert!(
                PERMISSION_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {LAMBDA_PERMISSION}, got none"
            );
        }
        for property in PERMISSION_PROPERTIES {
            let placed =
                PERMISSION_MEMBERS.contains(property) || *property == FUNCTION_NAME_PROPERTY;
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member or the one that \
                 addresses the call, got neither"
            );
        }
    }

    /// The function a declaration names is required, and a declaration that names none is
    /// refused rather than a grant being made somewhere else.
    #[test]
    fn a_statement_naming_no_function_is_refused() {
        let cases: &[(&str, Value)] = &[
            ("no properties at all", Value::Null),
            ("properties without the function", serde_json::json!({ "Action": "lambda:InvokeFunction" })),
            ("an empty function name", serde_json::json!({ "FunctionName": "" })),
        ];
        for (case, properties) in cases {
            let resource = StoredResource {
                logical_id: "Permission".to_string(),
                resource_type: LAMBDA_PERMISSION.to_string(),
                physical_resource_id: "stack-Permission-ABC".to_string(),
                attributes: std::collections::BTreeMap::new(),
                metadata: None,
                properties: properties.clone(),
                materialised: false,
                deletion_policy: None,
                update_replace_policy: None,
            };
            let refused = function_of(&resource);
            assert!(
                refused.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {refused:?}"
            );
        }
    }
}
