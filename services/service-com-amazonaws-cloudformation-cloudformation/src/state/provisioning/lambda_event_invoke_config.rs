// SPDX-License-Identifier: CC0-1.0

//! `AWS::Lambda::EventInvokeConfig`: the asynchronous-invocation configuration a stack
//! declares for one qualifier of a function, put in AWS Lambda itself — so the destinations
//! a template wires up really receive the invocation records of that function's events.
//!
//! The type is no entity of Lambda's own: it is the configuration HELD AGAINST a version or
//! an alias, which `PutFunctionEventInvokeConfig` writes ("Configures options for
//! asynchronous invocation on a function, version, or alias") and
//! `DeleteFunctionEventInvokeConfig` takes away. Its properties are that call's own members
//! under the same names, with the two that ADDRESS the call — the function and the qualifier
//! — travelling in the URI rather than in the body.
//!
//! This is the per-type half of provisioning for one resource type, reached through the same
//! seam as every other ([`super::Owner`]) and living in its own module for the reason
//! [`super::lambda_function`] records.
//!
//! The configuration is addressed by the function and the qualifier together: every call
//! about one takes both, so this resource's physical id is the pair, which is what the
//! removal and the drift read address it by.

use serde_json::{Map, Value};

use super::{call_rest_json, percent_encoded, Created, Refusal};
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const LAMBDA_EVENT_INVOKE_CONFIG: &str = "AWS::Lambda::EventInvokeConfig";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS Lambda's own handlers.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The error code Lambda answers for a function, or a configuration, it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says a delete has nothing
/// left to remove and a read has no configuration to report.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The property naming the function the configuration is held on, which addresses the call
/// rather than travelling in its body.
const FUNCTION_NAME_PROPERTY: &str = "FunctionName";

/// The property naming the version or alias it is held against, which addresses the call the
/// same way — `PutFunctionEventInvokeConfig$Qualifier` is an `@httpQuery` member.
const QUALIFIER_PROPERTY: &str = "Qualifier";

/// Every property `AWS::Lambda::EventInvokeConfig` publishes. A declared property this list
/// does not carry is REFUSED rather than dropped, so a destination is never reported as
/// configured with less than its declaration asked for.
///
/// Source: the type's own property set, which is `PutFunctionEventInvokeConfig`'s member list
/// (`smithy_query(query="operation",operation="PutFunctionEventInvokeConfig")`).
const CONFIG_PROPERTIES: &[&str] = &[
    "DestinationConfig",
    "FunctionName",
    "MaximumEventAgeInSeconds",
    "MaximumRetryAttempts",
    "Qualifier",
];

/// The properties that ARE `PutFunctionEventInvokeConfig`'s own body members under the same
/// name, so the declared value travels into the request as written.
const CONFIG_MEMBERS: &[&str] = &[
    "DestinationConfig",
    "MaximumEventAgeInSeconds",
    "MaximumRetryAttempts",
];

/// Put the configuration an `AWS::Lambda::EventInvokeConfig` declares in AWS Lambda itself —
/// so a client that deployed the stack and then calls `lambda get-function-event-invoke-config`
/// reads it back, and the destinations it names really receive the invocation records of the
/// function's asynchronous events.
///
/// `Err` is Lambda's own refusal of the put: a function or qualifier it does not hold, a
/// destination it will not accept, a retry count or event age outside the range the members
/// admit. The resource is never passed off as created on a call that did not answer 2xx, so a
/// stack whose configuration Lambda refused rolls back instead of reporting destinations that
/// nothing would ever write to.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let addressed = Addressed::of(&resource.properties)?;
    put(ctx, region, resource, &addressed).await?;
    Ok(Created {
        // What every call about this configuration addresses it by, which is therefore the
        // identity the stack publishes for the resource.
        physical_resource_id: Some(addressed.physical_id()),
        attributes: ::std::collections::BTreeMap::new(),
    })
}

/// Whether Lambda reports the configuration this create made ready.
///
/// Nothing is asked, and nothing waits: the configuration is in force the moment
/// `PutFunctionEventInvokeConfig` answers — the next asynchronous event of that qualifier is
/// delivered under it — so there is no transition to hold the resource's creation open on.
pub(super) fn readiness() -> super::Readiness {
    super::Readiness::Ready
}

/// Apply a CHANGED declaration to the configuration the stack already has.
///
/// `PutFunctionEventInvokeConfig` writes the WHOLE configuration of the qualifier it
/// addresses, so a changed declaration is applied by putting it: the configuration in force
/// ends up being exactly what the declaration states. A declaration that moves it to another
/// function or another qualifier addresses a DIFFERENT configuration, so the one in force is
/// taken off its own qualifier first and the resource publishes the new pair as its id.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let asked = Addressed::of(declared)?;
    let in_force = Addressed::of(&resource.properties).ok();
    let declaration = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    put(ctx, region, &declaration, &asked).await?;
    // The configuration this resource used to be is removed only once the declared one is in
    // force, and only where the declaration really moved it: a qualifier the stack no longer
    // configures would otherwise keep delivering to destinations nothing declares.
    if let Some(previous) = in_force.filter(|previous| previous.physical_id() != asked.physical_id())
    {
        remove(ctx, region, &previous).await?;
    }
    Ok(Created {
        physical_resource_id: Some(asked.physical_id()),
        attributes: ::std::collections::BTreeMap::new(),
    })
}

/// Delete the configuration a stack put, addressed by the function and qualifier its
/// declaration names.
///
/// A configuration — or a function — Lambda no longer holds is already in the state the
/// delete asks for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let Ok(addressed) = Addressed::of(&resource.properties) else {
        // A declaration that named no function or qualifier configured nothing, so there is
        // nothing for this delete to take away.
        return Ok(());
    };
    remove(ctx, region, &addressed).await
}

/// The configuration as Lambda reports it, for drift detection: the members
/// `GetFunctionEventInvokeConfig` answers under the same names the type publishes as
/// properties, beside the two that address the call and are the resource's own identity. A
/// configuration that service no longer holds was deleted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Ok(addressed) = Addressed::of(&resource.properties) else {
        return Ok(None);
    };
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "GET",
        &addressed.uri(),
        None,
    )
    .await
    {
        Ok(answered) => {
            let mut reported = Map::new();
            for member in CONFIG_MEMBERS {
                if let Some(value) = answered.get(*member).filter(|value| !value.is_null()) {
                    reported.insert((*member).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// `PutFunctionEventInvokeConfig` with the members `resource`'s declaration states, on the
/// qualifier `addressed` names.
async fn put(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    addressed: &Addressed,
) -> Result<(), Refusal> {
    refuse_unmapped_properties(resource)?;
    let mut request = Map::new();
    for member in CONFIG_MEMBERS {
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
        "PUT",
        &addressed.uri(),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(())
}

/// `DeleteFunctionEventInvokeConfig` on the qualifier `addressed` names.
async fn remove(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    addressed: &Addressed,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        LAMBDA_SIGNING_NAME,
        region,
        "DELETE",
        &addressed.uri(),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// WHICH configuration a declaration is about: the function it is held on and the qualifier
/// it is held against, which together address every call about it.
struct Addressed {
    /// `FunctionName`: "the name or ARN of the Lambda function, version, or alias".
    function: String,
    /// `Qualifier`: "a version number or alias name".
    qualifier: String,
}

impl Addressed {
    /// The pair a declaration names, or the refusal for one that names neither — a
    /// configuration with no function and no qualifier addresses nothing, so nothing is put
    /// anywhere for it.
    fn of(properties: &Value) -> Result<Self, Refusal> {
        let text = |property: &str| {
            properties
                .get(property)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        };
        let required = |property: &str| {
            text(property).ok_or_else(|| Refusal {
                code: "InvalidRequest".to_string(),
                message: format!(
                    "{LAMBDA_EVENT_INVOKE_CONFIG} requires the {property} property"
                ),
            })
        };
        Ok(Self {
            function: required(FUNCTION_NAME_PROPERTY)?,
            qualifier: required(QUALIFIER_PROPERTY)?,
        })
    }

    /// The URI the configuration operations of ONE qualifier bind: the `@httpLabel` carrying
    /// the function and the `@httpQuery` carrying the qualifier, each percent-encoded as a
    /// single value is sent.
    fn uri(&self) -> String {
        format!(
            "/2019-09-25/functions/{}/event-invoke-config?Qualifier={}",
            percent_encoded(&self.function),
            percent_encoded(&self.qualifier)
        )
    }

    /// The pair as one value, which is what this resource's physical id is.
    fn physical_id(&self) -> String {
        format!("{}:{}", self.function, self.qualifier)
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was.
fn refuse_unmapped_properties(resource: &StoredResource) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if CONFIG_PROPERTIES.contains(&property.as_str()) {
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

    /// One declaration as a stored resource, for the rows below.
    fn resource(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "EventInvokeConfig".to_string(),
            resource_type: LAMBDA_EVENT_INVOKE_CONFIG.to_string(),
            physical_resource_id: String::new(),
            attributes: std::collections::BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every member the arm sends is a published property of the type, and the properties
    /// that are not body members are the two that address the call.
    #[test]
    fn every_sent_member_is_a_published_property() {
        for member in CONFIG_MEMBERS {
            assert!(
                CONFIG_PROPERTIES.contains(member),
                "case {member:?}: expected the sent member to be a published property of \
                 {LAMBDA_EVENT_INVOKE_CONFIG}, got none"
            );
        }
        for property in CONFIG_PROPERTIES {
            let placed = CONFIG_MEMBERS.contains(property)
                || *property == FUNCTION_NAME_PROPERTY
                || *property == QUALIFIER_PROPERTY;
            assert!(
                placed,
                "case {property:?}: expected the property to be a body member or one of the \
                 two that address the call, got neither"
            );
        }
    }

    /// RULE: a configuration is addressed by the function and the qualifier together, so a
    /// declaration that leaves either out is refused rather than put somewhere else. The rows
    /// differ along which of the two is missing and how.
    #[test]
    fn a_configuration_naming_no_function_or_qualifier_is_refused() {
        let rows: &[(&str, Value)] = &[
            ("no properties at all", Value::Null),
            ("neither of the two", serde_json::json!({ "MaximumRetryAttempts": 0 })),
            ("no qualifier", serde_json::json!({ "FunctionName": "orders" })),
            ("no function", serde_json::json!({ "Qualifier": "$LATEST" })),
            (
                "an empty function name",
                serde_json::json!({ "FunctionName": "", "Qualifier": "$LATEST" }),
            ),
            (
                "an empty qualifier",
                serde_json::json!({ "FunctionName": "orders", "Qualifier": "" }),
            ),
        ];
        for (case, properties) in rows {
            let addressed = Addressed::of(properties);
            assert!(
                addressed.is_err(),
                "case {case:?}: expected a refusal naming the required property, got an \
                 addressed configuration"
            );
        }
    }

    /// The URI the calls bind, and the id the resource publishes, are the function and the
    /// qualifier the declaration names — the qualifier travelling as a single query value
    /// however it is spelled. The rows differ along the form of each.
    #[test]
    fn the_call_is_addressed_by_the_declared_function_and_qualifier() {
        let rows: &[(&str, &str, &str, &str, &str)] = &[
            (
                "a name and the unpublished version",
                "orders",
                "$LATEST",
                "/2019-09-25/functions/orders/event-invoke-config?Qualifier=%24LATEST",
                "orders:$LATEST",
            ),
            (
                "a name and a published version",
                "orders",
                "7",
                "/2019-09-25/functions/orders/event-invoke-config?Qualifier=7",
                "orders:7",
            ),
            (
                "a name and an alias",
                "orders",
                "live",
                "/2019-09-25/functions/orders/event-invoke-config?Qualifier=live",
                "orders:live",
            ),
            (
                "a full function ARN",
                "arn:aws:lambda:us-east-1:000000000000:function:orders",
                "$LATEST",
                "/2019-09-25/functions/arn%3Aaws%3Alambda%3Aus-east-1%3A000000000000%3Afunction%3Aorders/event-invoke-config?Qualifier=%24LATEST",
                "arn:aws:lambda:us-east-1:000000000000:function:orders:$LATEST",
            ),
        ];
        for (case, function, qualifier, uri, physical_id) in rows {
            let addressed = Addressed::of(&serde_json::json!({
                "FunctionName": function,
                "Qualifier": qualifier,
            }))
            .expect("the declaration names both");
            let answered = addressed.uri();
            assert_eq!(
                answered, *uri,
                "case {case:?}: expected the call to bind {uri}, got {answered}"
            );
            let answered = addressed.physical_id();
            assert_eq!(
                answered, *physical_id,
                "case {case:?}: expected the id {physical_id}, got {answered}"
            );
        }
    }

    /// A declared property this module does not apply is refused, naming it, rather than
    /// dropped — and every property the type publishes is accepted.
    #[test]
    fn a_property_this_arm_does_not_apply_is_refused() {
        let accepted = resource(serde_json::json!({
            "FunctionName": "orders",
            "Qualifier": "$LATEST",
            "MaximumRetryAttempts": 0,
            "MaximumEventAgeInSeconds": 60,
            "DestinationConfig": { "OnFailure": { "Destination": "arn:aws:sqs:us-east-1:000000000000:dlq" } },
        }));
        assert!(
            refuse_unmapped_properties(&accepted).is_ok(),
            "case \"every published property\": expected the declaration to be applied, got a \
             refusal"
        );
        let refused = resource(serde_json::json!({
            "FunctionName": "orders",
            "Qualifier": "$LATEST",
            "SomethingElse": true,
        }));
        assert!(
            refuse_unmapped_properties(&refused).is_err(),
            "case \"a property the type does not publish\": expected a refusal naming it, got \
             none"
        );
    }
}
