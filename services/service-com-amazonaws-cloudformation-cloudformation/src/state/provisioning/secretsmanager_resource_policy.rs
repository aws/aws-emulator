// SPDX-License-Identifier: CC0-1.0

//! `AWS::SecretsManager::ResourcePolicy`: the resource-based policy a stack attaches to a
//! secret, attached in AWS Secrets Manager itself.
//!
//! The type is no entity of Secrets Manager's own: it is the RESOURCE POLICY of the secret
//! its `SecretId` names, which `PutResourcePolicy` attaches ("Attaches a resource-based
//! permission policy to a secret", replacing whatever policy the secret carried) and
//! `DeleteResourcePolicy` takes off ("Deletes the resource-based permission policy attached
//! to the secret"). Its properties are those calls' own members under the names the type
//! publishes, so the declaration reaches the owning service as written.
//!
//! ## Why the policy is really attached
//!
//! A resource policy is how a template fences a secret off from everything but the
//! principals it names, and Secrets Manager AUTHORIZES every later call against it: a
//! `Deny` in it refuses the read, and an `Allow` in it is what admits a principal holding no
//! identity-based grant. A stack that reported this resource created while nothing attached
//! the document leaves the secret with no policy at all — `GetResourcePolicy` on the
//! stack's secret reports none, and every call the policy was written to refuse is served.
//! So the deployment makes the call, and the stack fails the resource when that service
//! refuses it (a secret it does not hold, a document with syntax errors, a policy granting
//! public access under `BlockPublicPolicy`).
//!
//! It lives beside [`super::secretsmanager_secret`] and is reached through the same seam
//! ([`super::Owner`]), for the reason that module records.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{call_secretsmanager, Created, Readiness, Refusal, Removal};
use crate::state::StoredResource;

/// The resource type this module provisions, as [`crate::state::template`] names it: the
/// identifier of a resource of this type is the one the owning service answered
/// (`identity_is_service_assigned`), and its `SecretId` is create-only
/// (`replacement_properties`), so both halves of this module's contract are read from there.
pub(super) use crate::state::template::SECRETSMANAGER_RESOURCE_POLICY;

/// The error code Secrets Manager answers for a secret it does not hold
/// (`ResourceNotFoundException`, the modelled refusal of `PutResourcePolicy`,
/// `DeleteResourcePolicy` and `GetResourcePolicy`), which is what says a removal has nothing
/// left to take off and a read nothing to report.
const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration that describes no
/// policy this emulator can attach: the same one the catalogue's arms use.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that leaves out a property the type
/// requires.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The code a refusal carries where the owning service answered without the one value this
/// resource's identity is.
const INTERNAL_FAILURE: &str = "InternalFailure";

/// The property naming the secret this policy is attached to, which is
/// `PutResourcePolicy`'s own `SecretId` member and the resource's identity.
const SECRET_ID: &str = "SecretId";

/// The property carrying the policy document, which is `PutResourcePolicy`'s own
/// `ResourcePolicy` member — "a JSON-formatted string for an Amazon Web Services
/// resource-based policy", while the template states it as the JSON document it is
/// ([`declared_document`]).
const RESOURCE_POLICY: &str = "ResourcePolicy";

/// The property asking for policies granting broad access to be blocked, which is
/// `PutResourcePolicy`'s own `BlockPublicPolicy` member.
const BLOCK_PUBLIC_POLICY: &str = "BlockPublicPolicy";

/// Every property `AWS::SecretsManager::ResourcePolicy` publishes. A declared property this
/// module does not apply is REFUSED rather than dropped, so a policy is never reported
/// attached with less than its declaration asked for.
///
/// Source: the type's own property set; each one is `PutResourcePolicy`'s own member of the
/// same name (`smithy_query(query="operation",operation="PutResourcePolicy")`).
const POLICY_PROPERTIES: &[&str] = &[BLOCK_PUBLIC_POLICY, RESOURCE_POLICY, SECRET_ID];

/// The attribute the type publishes, which is the ARN of the secret the policy was attached
/// to — the same value `Ref` returns for the resource.
const ID_ATTRIBUTE: &str = "Id";

/// Attach the policy an `AWS::SecretsManager::ResourcePolicy` declares, in AWS Secrets
/// Manager itself — so the document the stack published really governs the secret, and a
/// later `GetResourcePolicy` on it answers that document.
///
/// `Err` is that service's own refusal of the call: a secret it does not hold, a document
/// with syntax errors, a policy granting public access where the declaration asked for those
/// to be blocked.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let answered = attach(ctx, region, &resource.properties, resource).await?;
    attached_identity(&answered)
}

/// Apply a CHANGED declaration to the policy the stack already attached, through the same
/// `PutResourcePolicy` — the one call that writes a secret's resource policy, and one that
/// replaces whatever document the secret carried.
///
/// A changed `SecretId` is not one of them: the secret IS this resource's identity, so a
/// declaration naming another secret is a different resource, which the algorithm above
/// replaces rather than bringing here — that property is one of the type's create-only ones
/// (`crate::state::template::replacement_properties`).
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let answered = attach(ctx, region, declared, resource).await?;
    // The policy stays on the same secret, so the handle the stack published — and the one
    // attribute read off it — is the same one the attach answered again.
    attached_identity(&answered).map(Some)
}

/// The identity the attach answered, as the resource's: the ARN of the secret the policy is
/// attached to, which is the physical id every later call on the policy takes and the one
/// attribute the type publishes.
fn attached_identity(answered: &Value) -> Result<Created, Refusal> {
    let arn = answered
        .get("ARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: INTERNAL_FAILURE.to_string(),
            message: "AWS Secrets Manager answered PutResourcePolicy without an ARN".to_string(),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert(ID_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// `PutResourcePolicy` has attached the document as it answers — the secret's every later
/// call is authorized against it from then on — so there is nothing for a later read to wait
/// on.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// `DeleteResourcePolicy` takes the document off as it answers, so there is nothing for a
/// later read to wait on either.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Take the policy a stack attached off the secret, addressed by the ARN it was attached to.
///
/// `DeleteResourcePolicy` is what CloudFormation makes of this type's removal: it "deletes
/// the resource-based permission policy attached to the secret", which is exactly this
/// resource ceasing to exist while the SECRET — a resource of its own, with its own deletion
/// policy — stays. A secret that service no longer holds carries no policy either, so it is
/// already in the state the removal asks for; judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    // A resource whose attach never answered holds no identity, so there is no secret to
    // address the call to and nothing attached to take off.
    if resource.physical_resource_id.is_empty() {
        return Ok(());
    }
    let request = serde_json::json!({ SECRET_ID: resource.physical_resource_id });
    match call_secretsmanager(ctx, region, "DeleteResourcePolicy", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The attached policy as the owning service reports it, for drift detection:
/// `GetResourcePolicy`'s own answer, read by the ARN the delete uses.
///
/// Only the document reads back under the same name as the property that declared it; the
/// `BlockPublicPolicy` the attach asked for is a property of the CALL rather than of the
/// secret, and no read of that service reports it, so it is not reported here — a property
/// the read does not report is not evidence of anything
/// ([`super::super::drift::differs`]). The document is reported as the JSON it parses as,
/// which is the form a template declares it in, and as the text it came in otherwise. A
/// secret that service no longer holds, or one carrying no policy, reports nothing.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    // A resource holding no identity addresses no secret, so there is nothing to report.
    if resource.physical_resource_id.is_empty() {
        return Ok(None);
    }
    let request = serde_json::json!({ SECRET_ID: resource.physical_resource_id });
    let answered = match call_secretsmanager(ctx, region, "GetResourcePolicy", &request).await {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(attached) = answered.get(RESOURCE_POLICY).and_then(Value::as_str) else {
        return Ok(None);
    };
    let reported = serde_json::from_str::<Value>(attached)
        .unwrap_or_else(|_| Value::String(attached.to_string()));
    let mut configuration = Map::new();
    configuration.insert(RESOURCE_POLICY.to_string(), reported);
    Ok(Some(Value::Object(configuration)))
}

/// Attach `properties` as the secret's resource policy, through `PutResourcePolicy`.
async fn attach(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    properties: &Value,
    resource: &StoredResource,
) -> Result<Value, Refusal> {
    refuse_unapplied_properties(properties, &resource.resource_type)?;
    let mut request = Map::new();
    request.insert(SECRET_ID.to_string(), Value::String(secret_of(properties)?));
    request.insert(
        RESOURCE_POLICY.to_string(),
        Value::String(declared_document(properties)?),
    );
    if let Some(value) = stated(properties, BLOCK_PUBLIC_POLICY) {
        request.insert(BLOCK_PUBLIC_POLICY.to_string(), value.clone());
    }
    call_secretsmanager(ctx, region, "PutResourcePolicy", &Value::Object(request)).await
}

/// The secret the declaration names, which the type requires: "SecretId — the ARN or name of
/// the secret to attach the resource-based policy" (`PutResourcePolicy`, this service's
/// Smithy model).
fn secret_of(properties: &Value) -> Result<String, Refusal> {
    match stated(properties, SECRET_ID).map(::service_common::json_wire_values::value_text) {
        Some(secret) if !secret.is_empty() => Ok(secret),
        _ => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!("{SECRETSMANAGER_RESOURCE_POLICY} requires the {SECRET_ID} property"),
        }),
    }
}

/// The declared document, as the text the attach carries it as: a template states it as a
/// JSON object (the common form) or as a string, and both reach the owning service as the
/// document itself — the same reading the policy-document property of every other type gets
/// ([`super::iam_policy`]).
fn declared_document(properties: &Value) -> Result<String, Refusal> {
    let rendered = match stated(properties, RESOURCE_POLICY) {
        Some(Value::String(document)) => Some(document.clone()),
        Some(document @ (Value::Object(_) | Value::Array(_))) => {
            serde_json::to_string(document).ok()
        }
        _ => None,
    };
    match rendered.filter(|document| !document.is_empty()) {
        Some(document) => Ok(document),
        None => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{SECRETSMANAGER_RESOURCE_POLICY} requires the {RESOURCE_POLICY} property"
            ),
        }),
    }
}

/// The value a declaration STATES for `property`, or `None` for one it leaves out or states
/// as null.
fn stated<'a>(properties: &'a Value, property: &str) -> Option<&'a Value> {
    properties.get(property).filter(|value| !value.is_null())
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a policy from being reported as attached with less than its
/// declaration asked for.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if POLICY_PROPERTIES.contains(&property.as_str()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the document reaches the owning service as the JSON text it is, whether the
    /// declaration states it as an object or as a string.
    #[test]
    fn a_declared_document_reaches_the_service_as_text() {
        // (case, the declared properties, the text the attach must carry)
        let rows: &[(&str, Value, &str)] = &[
            (
                "a document declared as the JSON object it is",
                serde_json::json!({
                    "SecretId": "db-AbCdEf",
                    "ResourcePolicy": {"Version": "2012-10-17"}
                }),
                r#"{"Version":"2012-10-17"}"#,
            ),
            (
                "a document declared as a string",
                serde_json::json!({
                    "SecretId": "db-AbCdEf",
                    "ResourcePolicy": "{\"Version\":\"2012-10-17\"}"
                }),
                r#"{"Version":"2012-10-17"}"#,
            ),
        ];
        for (case, properties, expected) in rows {
            let reached = declared_document(properties);
            assert_eq!(
                reached.as_deref().ok(),
                Some(*expected),
                "case {case:?}: expected the attach to carry {expected:?}, got {reached:?}"
            );
        }
    }

    /// RULE: a declaration leaving out a property the type requires, or stating one this
    /// module does not apply, is refused rather than deployed with less than it asked for.
    #[test]
    fn a_declaration_this_module_cannot_apply_is_refused() {
        // (case, the declared properties, whether the properties are refused)
        let rows: &[(&str, Value, bool)] = &[
            (
                "the secret, the document and the flag",
                serde_json::json!({
                    "SecretId": "arn:aws:secretsmanager:us-west-2:000000000000:secret:db-AbCdEf",
                    "ResourcePolicy": {"Version": "2012-10-17", "Statement": []},
                    "BlockPublicPolicy": true
                }),
                false,
            ),
            (
                "a property the type does not publish",
                serde_json::json!({"SecretId": "db-AbCdEf", "Elsewhere": "x"}),
                true,
            ),
        ];
        for (case, properties, refused) in rows {
            let reached =
                refuse_unapplied_properties(properties, SECRETSMANAGER_RESOURCE_POLICY).is_err();
            assert_eq!(
                reached, *refused,
                "case {case:?}: expected refused={refused}, got {reached} for {properties}"
            );
        }
        for (case, properties) in [
            ("no properties at all", Value::Null),
            (
                "properties without the secret",
                serde_json::json!({"ResourcePolicy": {"Version": "2012-10-17"}}),
            ),
            ("an empty secret id", serde_json::json!({"SecretId": ""})),
        ] {
            let reached = secret_of(&properties);
            assert!(
                reached.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {reached:?}"
            );
        }
        for (case, properties) in [
            ("no properties at all", Value::Null),
            (
                "properties without the document",
                serde_json::json!({"SecretId": "db-AbCdEf"}),
            ),
            (
                "an empty document",
                serde_json::json!({"SecretId": "db-AbCdEf", "ResourcePolicy": ""}),
            ),
        ] {
            let reached = declared_document(&properties);
            assert!(
                reached.is_err(),
                "case {case:?}: expected a refusal naming the required property, got {reached:?}"
            );
        }
    }
}
