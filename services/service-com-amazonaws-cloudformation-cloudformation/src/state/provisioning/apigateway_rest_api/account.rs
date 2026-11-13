// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::Account`: the ACCOUNT-level settings of that service's REST control
//! plane — the CloudWatch Logs role API Gateway writes an account's logs under — which a
//! stack really sets in Amazon API Gateway itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls and the same helpers (a patch operation is written with
//! [`super::patch`], a declared value is read as the wire carries it with
//! [`super::attribute_text`], a property no call here carries is refused with
//! [`super::refuse_unpublished`]), so no two arms of this control plane word a refusal or
//! place a member differently.
//!
//! ## Why it is really set
//!
//! The account's CloudWatch role is what makes API Gateway able to write a stage's access
//! logs and a method's execution logs at all: a stage whose logging is turned on while the
//! account carries no role is refused by that service, and `GetAccount` is where a client
//! reads which role is in force. API Gateway holds that setting, so a stack that recorded
//! the resource and asked that service nothing leaves an account whose role is unset however
//! complete the stack's own records look — and the logging a template deployed the resource
//! in order to enable can never be turned on.
//!
//! ## The calls
//!
//! The settings are a SINGLETON of the account and Region, so there is no create call and no
//! identifier for either side to assign: the one call API Gateway publishes for them is
//! `UpdateAccount` (`PATCH /account`), whose `patchOperations` carry a `replace` of
//! `/cloudwatchRoleArn`. So every arm here is that one call —
//!
//! * CREATE sets the role the declaration states;
//! * UPDATE sets the role the CHANGED declaration states, and clears the setting where that
//!   declaration no longer states one;
//! * DELETE clears the setting this resource put there, which is what makes a rolled-back
//!   deployment leave the account as it found it;
//!
//! — and the drift read is that service's own `GetAccount` (`GET /account`).
//!
//! A declaration stating no role at all asks for no setting, so nothing is patched for it:
//! the account keeps whatever is in force, rather than this resource clearing a role it never
//! set.

use super::*;

/// The account settings of the REST control plane, which is what `GetAccount` and
/// `UpdateAccount` are bound to (`GET`/`PATCH /account`): one singleton per account and
/// Region, with no identifier in its URI.
const ACCOUNT_URI: &str = "/account";

/// Every property `AWS::ApiGateway::Account` publishes. A declared property this list does
/// not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `UpdateAccount`'s one patchable member
/// (`smithy_query(query="shape",shape="Account")`: `cloudwatchRoleArn` is the member a caller
/// sets, beside the throttle settings, the features and the API key version that service
/// reports for itself).
const ACCOUNT_PROPERTIES: &[&str] = &[CLOUDWATCH_ROLE_ARN];

/// The property carrying the role API Gateway writes the account's logs under.
const CLOUDWATCH_ROLE_ARN: &str = "CloudWatchRoleArn";

/// The JSON Pointer that member is addressed by in a patch: `Account$cloudwatchRoleArn` as a
/// pointer from the record's root.
const CLOUDWATCH_ROLE_ARN_PATH: &str = "/cloudwatchRoleArn";

/// The member `GetAccount` reports the role as, which the drift read takes it from.
const CLOUDWATCH_ROLE_ARN_MEMBER: &str = "cloudwatchRoleArn";

/// The `op` that sets a single-valued member: the account's role takes `replace` and neither
/// an `add` nor a `remove`, so clearing it is a `replace` carrying no value.
const REPLACE: &str = "replace";

/// The value a `replace` carries to CLEAR the setting: an empty value names no role, which is
/// how a caller takes the account's CloudWatch role off a member whose path takes no `remove`.
const NO_ROLE: &str = "";

/// The properties API Gateway's own read of the account reports, as `(property, member)`
/// pairs, for drift detection.
///
/// The role alone: everything else `GetAccount` answers is the service's own report about the
/// account (its request limits, its features, its API key version), which no declaration
/// states, so a difference in one would be a change nobody made.
const ACCOUNT_REPORTED: &[(&str, &str)] = &[(CLOUDWATCH_ROLE_ARN, CLOUDWATCH_ROLE_ARN_MEMBER)];

/// Set the account's CloudWatch role to the one an `AWS::ApiGateway::Account` declares, so a
/// client that deployed the stack reads it back through `GetAccount` — and so the stage access
/// logging and method execution logging that role exists to permit can really be turned on.
///
/// The resource keeps the identifier the naming rules computed: the settings are a singleton,
/// so that call answers no id of its own ([`super::super::super::template::return_values`]
/// mints the one this type publishes).
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unpublished(resource, ACCOUNT_PROPERTIES)?;
    if let Some(role_arn) = declared_role(&resource.properties) {
        set_role(ctx, region, &role_arn).await?;
    }
    Ok(Created::default())
}

/// Apply a CHANGED declaration to the account settings that are already in force, through the
/// same one call: `UpdateAccount` states the role a client wants whether or not one is set, so
/// the settings keep their identity and nothing is replaced.
///
/// A declaration that no longer states a role CLEARS the setting this resource put there,
/// rather than leaving a role the template no longer asks for in force — decided the way
/// [`delete`] decides it, from that service's own read, so a role another caller re-pointed the
/// singleton at since the create is left alone.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unpublished_properties(&resource.resource_type, declared, ACCOUNT_PROPERTIES)?;
    match declared_role(declared) {
        Some(role_arn) => set_role(ctx, region, &role_arn).await?,
        None => clear_own_role(ctx, region, resource).await?,
    }
    Ok(Some(Created::default()))
}

/// Take the account's CloudWatch role off again, where this resource is what PUT it there:
/// the setting is the only thing the resource ever was, so a stack that removes the resource —
/// or rolls its creation back — leaves the account as it found it.
///
/// Whether this resource put it there is decided from that service's own read, not from the
/// record alone: the role in force has to be the one this resource declares. So a create that
/// was refused before it patched anything takes nothing off on the way back out, and a role
/// another caller set is left alone — the clear undoes this resource's own write and no other.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    clear_own_role(ctx, region, resource).await
}

/// Take the role THIS resource's record declares off the account, and nothing else: the ONE
/// clearing this module does, shared by the delete and by an update whose declaration no longer
/// states a role, so neither can take off a setting this resource did not write.
///
/// A record stating no role wrote none, so there is nothing to take off for it.
async fn clear_own_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let Some(declared) = declared_role(&resource.properties) else {
        return Ok(());
    };
    if reported_role(ctx, region).await?.as_deref() == Some(declared.as_str()) {
        set_role(ctx, region, NO_ROLE).await?;
    }
    Ok(())
}

/// The account settings as API Gateway reports them, under this type's property names
/// ([`ACCOUNT_REPORTED`]), for drift detection.
///
/// `Ok(Some(..))` always: the settings are a singleton every account and Region has, so there
/// is no answer that could say the resource is GONE.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = read(ctx, region).await?;
    let mut held = Map::new();
    for (property, member) in ACCOUNT_REPORTED {
        if let Some(value) = answered.get(*member) {
            held.insert((*property).to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(held)))
}

/// The role API Gateway reports in force for the account, or `None` where it reports none.
async fn reported_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Option<String>, Refusal> {
    Ok(read(ctx, region)
        .await?
        .get(CLOUDWATCH_ROLE_ARN_MEMBER)
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// What `GetAccount` answers about the account settings: the ONE read this module makes,
/// shared by the drift report and the delete that has to know what is in force.
async fn read(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Value, Refusal> {
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "GET",
        ACCOUNT_URI,
        None,
    )
    .await
}

/// The role one declaration states, or `None` where it states none — a property declared
/// `null`, and one carrying no text, each name no role.
fn declared_role(properties: &Value) -> Option<String> {
    properties
        .get(CLOUDWATCH_ROLE_ARN)
        .filter(|value| !value.is_null())
        .map(attribute_text)
        .filter(|role_arn| !role_arn.is_empty())
}

/// The one call this module makes: `UpdateAccount` with a `replace` of the role's pointer,
/// carrying the ARN to set or [`NO_ROLE`] to clear it.
async fn set_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert(
        "patchOperations".to_string(),
        Value::Array(vec![patch(REPLACE, CLOUDWATCH_ROLE_ARN_PATH, role_arn)]),
    );
    call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PATCH",
        ACCOUNT_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// An IAM role ARN of the form a `CloudWatchRoleArn` names.
    const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/apigateway-logs";

    /// A resource record as a deployment hands one to these arms.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Declared".to_string(),
            resource_type: APIGATEWAY_ACCOUNT.to_string(),
            physical_resource_id: "ab12cd34ef".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
        }
    }

    /// Every published property of the type reaches the one call this module makes: a
    /// declaration stating it is read as the value the patch carries, so a property no arm
    /// places would be dropped from a create reported as a success.
    #[test]
    fn every_published_property_reaches_the_patch() {
        for property in ACCOUNT_PROPERTIES {
            let properties = json!({ *property: ROLE_ARN });
            let carried = declared_role(&properties);
            assert_eq!(
                carried.as_deref(),
                Some(ROLE_ARN),
                "case {property:?}: expected the declared {property} of {APIGATEWAY_ACCOUNT} to \
                 reach the patch, got {carried:?}"
            );
        }
    }

    /// WHICH role a declaration states, over the forms a property takes: a stated ARN, a
    /// property declared `null`, one carrying no text, and no property at all.
    #[test]
    fn a_declaration_states_the_role_the_patch_carries() {
        let rows: &[(&str, Value, Option<&str>)] = &[
            ("a stated role", json!({ CLOUDWATCH_ROLE_ARN: ROLE_ARN }), Some(ROLE_ARN)),
            ("no property at all", json!({}), None),
            ("a null property", json!({ CLOUDWATCH_ROLE_ARN: Value::Null }), None),
            ("an empty property", json!({ CLOUDWATCH_ROLE_ARN: "" }), None),
        ];
        for (case, properties, expected) in rows {
            let stated = declared_role(properties);
            assert_eq!(
                stated.as_deref(),
                *expected,
                "case {case:?}: expected the declaration to state {expected:?}, got {stated:?}"
            );
        }
    }

    /// A declaration stating a property no call here carries is refused naming that property,
    /// rather than the account being reported set with less than the declaration asked for.
    #[test]
    fn a_property_no_call_here_carries_is_refused() {
        let refused = refuse_unpublished(
            &declared(json!({ CLOUDWATCH_ROLE_ARN: ROLE_ARN, "Unpublished": "x" })),
            ACCOUNT_PROPERTIES,
        );
        let Err(refusal) = refused else {
            panic!("case \"an unpublished property\": expected a refusal, got {refused:?}");
        };
        assert_eq!(
            refusal.code, NOT_IMPLEMENTED,
            "case \"an unpublished property\": expected the refusal to carry \
             {NOT_IMPLEMENTED:?}, got {refusal:?}"
        );
        assert!(
            refusal.message.contains("Unpublished")
                && refusal.message.contains(APIGATEWAY_ACCOUNT),
            "case \"an unpublished property\": expected the refusal to name the property and \
             the type, got {refusal:?}"
        );
    }

    /// The patch the one call carries: a `replace` of the role's own pointer, with the ARN to
    /// set or the empty value that clears it.
    #[test]
    fn the_patch_replaces_the_roles_own_pointer() {
        let rows: &[(&str, &str)] = &[("a role to set", ROLE_ARN), ("the setting cleared", NO_ROLE)];
        for (case, value) in rows {
            let operation = patch(REPLACE, CLOUDWATCH_ROLE_ARN_PATH, value);
            assert_eq!(
                operation,
                json!({ "op": REPLACE, "path": CLOUDWATCH_ROLE_ARN_PATH, "value": value }),
                "case {case:?}: expected the patch UpdateAccount takes, got {operation:?}"
            );
        }
    }
}
