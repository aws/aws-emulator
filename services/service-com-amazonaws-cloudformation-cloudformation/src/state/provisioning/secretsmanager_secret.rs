// SPDX-License-Identifier: CC0-1.0

//! `AWS::SecretsManager::Secret`: the secret a stack declares, created in AWS Secrets
//! Manager itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::step_functions_state_machine`] for the reason that
//! module records — the catalogue file is far past the length a change to this repository
//! may touch — and is reached through the same seam ([`super::Owner`]).
//!
//! ## Why the secret is really created
//!
//! "Ref returns the ARN of the secret", and that ARN is the handle every Secrets Manager
//! call takes: a template passes it into a function's environment, a task definition's
//! `valueFrom` or a database's credentials, and the application's first request is a
//! `GetSecretValue` on it. A stack that reported the secret created while nothing created it
//! publishes an ARN that resolves to nothing, so that first request is
//! `ResourceNotFoundException` — the stack completed and the application is broken. Creating
//! the secret where it belongs is what makes the ARN the stack publishes one the owning
//! service answers for, and what makes a second declaration of the same name meet that
//! service's own `ResourceExistsException`.
//!
//! ## The identifier is the service's
//!
//! A secret's ARN ends in six characters Secrets Manager draws per secret
//! (`arn:aws:secretsmanager:<region>:<account>:secret:<name>-<suffix>`), so no naming rule
//! can compute it: the ARN the create answers with IS the resource's physical id
//! ([`super::super::template::identity_is_service_assigned`] names the type), and the
//! stand-in the naming rules computed is only what a reference was resolved against until
//! then. The NAME the secret is created under is still CloudFormation's — the `Name` the
//! declaration states, or the generated `<stack>-<logical id>-<suffix>` name — and travels
//! as the resource's `Name` attribute, because the physical id is the ARN
//! ([`secret_name`]).
//!
//! ## The version id
//!
//! `CreateSecret` and `UpdateSecret` stake the secret's new version under a
//! `ClientRequestToken`, and that member is documented "If you generate a raw HTTP request
//! to the Secrets Manager service endpoint, then you must generate a ClientRequestToken and
//! include it in the request" — the SDKs "generate a random UUID for you". This provider is
//! such a raw request, so it mints one per call ([`client_request_token`]).
//!
//! ## The value
//!
//! The type states the secret's value two ways and they are alternatives: `SecretString` IS
//! the value, and `GenerateSecretString` asks Secrets Manager to generate one — which is
//! that service's own `GetRandomPassword`, with the generation parameters the property
//! publishes under the very member names that call takes. Where the property also states a
//! `SecretStringTemplate` and a `GenerateStringKey`, the generated password is put into that
//! JSON document under that key, which is how a template declares a secret holding a whole
//! credential document with only the password generated. A declaration that states both
//! forms, or one half of that pair, describes no single value and is refused rather than
//! deployed with whichever this module happened to prefer.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    applicable_tags, call_secretsmanager, key_value_tag_list, Created, Readiness, Refusal, Removal,
};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const SECRETSMANAGER_SECRET: &str = "AWS::SecretsManager::Secret";

/// The error code Secrets Manager answers for a secret it does not hold
/// (`ResourceNotFoundException`, the modelled refusal of its reads, updates and deletes),
/// which is what says a delete has nothing left to remove and a read nothing to report.
const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";

/// The read that says whether Secrets Manager still holds a secret, and what it holds of it
/// ([`removal`], [`actual_configuration`]).
const DESCRIBE_SECRET: &str = "DescribeSecret";

/// `DeleteSecret`'s member asking for the secret to be removed with no recovery window,
/// which is what this type's removal states (see [`delete`]).
const FORCE_DELETE_WITHOUT_RECOVERY: &str = "ForceDeleteWithoutRecovery";

/// The code a refusal of this module's own carries for a declaration that describes no
/// resource this emulator can create: the same one the catalogue's arms use.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration whose own properties contradict each other
/// or leave out one the type requires.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The code a refusal carries where the owning service answered a create without the one
/// value the resource's identity is.
const INTERNAL_FAILURE: &str = "InternalFailure";

/// The property carrying the secret's value verbatim, which is `CreateSecret`'s own
/// `SecretString` member.
const SECRET_STRING: &str = "SecretString";

/// The property asking the owning service to GENERATE the value
/// ([`generated_secret_string`]).
const GENERATE_SECRET_STRING: &str = "GenerateSecretString";

/// The property naming the secret, which is the name it is created under rather than part of
/// its identity here ([`secret_name`]).
const NAME: &str = "Name";

/// The property declaring the Regions the secret is replicated to, which is
/// `CreateSecret`'s own `AddReplicaRegions` member — the list members are spelled
/// identically on both sides (`Region`, `KmsKeyId`), so the declared list travels as
/// written.
const REPLICA_REGIONS: &str = "ReplicaRegions";
/// See [`REPLICA_REGIONS`].
const ADD_REPLICA_REGIONS: &str = "AddReplicaRegions";

/// The member both writes stake the new version under, and how many characters this
/// provider mints one with: the modelled `ClientRequestTokenType`'s own shortest length
/// (32..64, `smithy_query(query="operation",operation="CreateSecret")`).
const CLIENT_REQUEST_TOKEN: &str = "ClientRequestToken";
/// See [`CLIENT_REQUEST_TOKEN`].
const CLIENT_REQUEST_TOKEN_CHARACTERS: usize = 32;

/// The two members of the generation property that are CLOUDFORMATION's rather than
/// `GetRandomPassword`'s: the JSON document the generated password is put INTO, and the key
/// it is put under.
const SECRET_STRING_TEMPLATE: &str = "SecretStringTemplate";
/// See [`SECRET_STRING_TEMPLATE`].
const GENERATE_STRING_KEY: &str = "GenerateStringKey";

/// Every property `AWS::SecretsManager::Secret` publishes. A declared property this list
/// does not carry is REFUSED rather than dropped, so a secret is never reported created with
/// less than its declaration asked for.
///
/// Source: the type's own property set; each one beside the two value forms is
/// `CreateSecret`'s own member of the same name
/// (`smithy_query(query="operation",operation="CreateSecret")`).
const SECRET_PROPERTIES: &[&str] = &[
    "Description",
    GENERATE_SECRET_STRING,
    "KmsKeyId",
    NAME,
    REPLICA_REGIONS,
    SECRET_STRING,
    "Tags",
];

/// The properties that reach `CreateSecret` under their own names, each being that call's
/// member of the same spelling.
const CREATE_MEMBERS: &[&str] = &["Description", "KmsKeyId"];

/// The members of `GenerateSecretString` that are `GetRandomPassword`'s own, spelled
/// identically on both sides.
///
/// Source: `GetRandomPassword`'s member list
/// (`smithy_query(query="operation",operation="GetRandomPassword")`), intersected with the
/// members the `GenerateSecretString` property publishes.
const PASSWORD_MEMBERS: &[&str] = &[
    "ExcludeCharacters",
    "ExcludeLowercase",
    "ExcludeNumbers",
    "ExcludePunctuation",
    "ExcludeUppercase",
    "IncludeSpace",
    "PasswordLength",
    "RequireEachIncludedType",
];

/// Create the secret an `AWS::SecretsManager::Secret` declares, in AWS Secrets Manager
/// itself — so a client (or a function) that reads the ARN the stack published calls
/// `GetSecretValue` on it and gets the value the template declared.
///
/// `Err` is that service's own refusal of the create, which is what the resource fails with:
/// a name the account already holds, a value past the modelled length, a KMS key it will not
/// encrypt with.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(&resource.properties, &resource.resource_type)?;
    let name = secret_name(resource);
    let mut request = Map::new();
    request.insert(NAME.to_string(), Value::String(name.clone()));
    request.insert(
        CLIENT_REQUEST_TOKEN.to_string(),
        Value::String(client_request_token(resource)),
    );
    for member in CREATE_MEMBERS {
        if let Some(value) = stated(&resource.properties, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let Some(form) = value_form(&resource.properties)? {
        let value = resolved_value(ctx, region, form).await?;
        request.insert(SECRET_STRING.to_string(), Value::String(value));
    }
    if let Some(replicas) = stated(&resource.properties, REPLICA_REGIONS) {
        request.insert(ADD_REPLICA_REGIONS.to_string(), replicas.clone());
    }
    // The secret carries the tags its declaration asks for plus the ones CloudFormation
    // gives the resource, as `CreateSecret`'s own `Tags` list — less the keys in AWS's
    // reserved namespace, which this service refuses on a caller's request ("the `aws:`
    // prefix ... is reserved for AWS use") and which no public call of it writes.
    let tags = applicable_tags(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("Tags".to_string(), key_value_tag_list(&tags));
    }
    let answered = call_secretsmanager(ctx, region, "CreateSecret", &Value::Object(request)).await?;
    let arn = answered
        .get("ARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: INTERNAL_FAILURE.to_string(),
            message: "AWS Secrets Manager answered CreateSecret without an ARN".to_string(),
        })?
        .to_string();
    // The ARN that service minted is the resource's identity from here on: the physical id
    // its `Ref` resolves to, the `Id` attribute the type publishes, and the ARN the uniform
    // convention reports for the resource.
    let mut attributes: BTreeMap<String, String> =
        crate::state::template::arn_attributes(&resource.resource_type, &arn)
            .into_iter()
            .collect();
    attributes.insert(
        NAME.to_string(),
        answered
            .get(NAME)
            .and_then(Value::as_str)
            .filter(|reported| !reported.is_empty())
            .map_or(name, ToString::to_string),
    );
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// A created secret is ready the moment the create answers: `GetSecretValue` reads the
/// version that create stored, so nothing about the secret is still coming up.
pub(super) fn readiness() -> Readiness {
    Readiness::Ready
}

/// Whether AWS Secrets Manager has finished removing the secret the stack deleted.
///
/// The removal skips the recovery window ([`delete`]), but it is not instantaneous:
/// "Secrets Manager performs the actual deletion with an asynchronous background process, so
/// there might be a short delay before the secret is permanently deleted."
/// (`DeleteSecret$ForceDeleteWithoutRecovery`). So the secret is gone only once that service
/// no longer answers for the ARN — its own read, never a span this crate assumed — and the
/// stack's deletion is held open until then, because CloudFormation does not report an
/// operation complete while a resource it deleted is still there.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    let request = serde_json::json!({ "SecretId": physical_resource_id });
    match call_secretsmanager(ctx, region, DESCRIBE_SECRET, &request).await {
        // Secrets Manager still holds the secret, marked for deletion: it reports no status
        // of its own for one, so what the resource is still in is CloudFormation's own
        // deleting state.
        Ok(_) => Ok(Removal::Pending(
            crate::state::resource_status::DELETE_IN_PROGRESS.to_string(),
        )),
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => Ok(Removal::Gone),
        Err(refusal) => Err(refusal),
    }
}

/// Apply a CHANGED declaration to the secret the stack already has, through `UpdateSecret` —
/// the call that takes the secret's own id, so it keeps its ARN and nothing is replaced.
///
/// `UpdateSecret` takes the description, the encryption key and the value, which are exactly
/// the properties a deployment can change on a secret that exists. A changed `Name` is not
/// one of them: the name is the secret's own identity in that service, so a renamed
/// declaration is a different resource and is deployed as one by the algorithm above rather
/// than reaching here.
///
/// The VALUE is written only where the declaration changes it ([`value_is_changed`]):
/// `GenerateSecretString` asks the owning service for a fresh password every time it is read,
/// so re-sending it on a deployment that changed only the description would store a new
/// version under consumers already holding the old one — rotating a value no declaration
/// changed.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it: a changed `Tags` set, which `UpdateSecret` does
/// not carry, and a changed `ReplicaRegions` list, which is `ReplicateSecretToRegions` and
/// `RemoveRegionsFromReplication` rather than a member of the update.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type)?;
    // Refused whatever else changed: a declaration stating both value forms describes no
    // single value, so it is no more deployable as an update than as a create.
    let declared_form = value_form(declared)?;
    if declared.get("Tags") != resource.properties.get("Tags")
        || declared.get(REPLICA_REGIONS) != resource.properties.get(REPLICA_REGIONS)
    {
        return Ok(None);
    }
    let mut request = Map::new();
    request.insert(
        "SecretId".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert(
        CLIENT_REQUEST_TOKEN.to_string(),
        Value::String(client_request_token(resource)),
    );
    for member in CREATE_MEMBERS {
        if let Some(value) = stated(declared, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    if let (true, Some(form)) = (
        value_is_changed(declared, &resource.properties),
        declared_form,
    ) {
        let value = resolved_value(ctx, region, form).await?;
        request.insert(SECRET_STRING.to_string(), Value::String(value));
    }
    call_secretsmanager(ctx, region, "UpdateSecret", &Value::Object(request)).await?;
    // The secret keeps its ARN across an update, so the handle the stack publishes — and
    // every attribute read off it — is unchanged.
    Ok(Some(Created::default()))
}

/// Delete the secret a stack created, addressed by the ARN it is known by.
///
/// The removal of this type takes the secret AWAY rather than leaving it recoverable: a
/// stack's deletion removes what the stack created, and a secret left scheduled for
/// deletion is still a secret this account holds — it still answers `DescribeSecret`, it
/// still occupies its name, and a Parameter Store reference to it still names something.
/// So the delete states `ForceDeleteWithoutRecovery` — "Specifies whether to delete the
/// secret without any recovery window" (`DeleteSecret$ForceDeleteWithoutRecovery`) — and
/// the secret is gone once that service has carried it out ([`removal`]). A secret it no
/// longer holds is already in the state the delete asks for, judged on the CODE and never
/// on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "SecretId": resource.physical_resource_id,
        FORCE_DELETE_WITHOUT_RECOVERY: true,
    });
    match call_secretsmanager(ctx, region, "DeleteSecret", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The secret's configuration as the owning service reports it, for drift detection:
/// `DescribeSecret`'s own record, read by the ARN the delete uses.
///
/// Only the properties that read answers under the same names are reported — the secret's
/// name, its description and the key it is encrypted with — and a property the read does not
/// report is not evidence of anything ([`super::super::drift::differs`]). The VALUE is not
/// among them: `DescribeSecret` reports no `SecretString` at all, which is what keeps a
/// drift read from publishing a secret's contents. A secret that service no longer holds was
/// deleted.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "SecretId": resource.physical_resource_id });
    let answered = match call_secretsmanager(ctx, region, DESCRIBE_SECRET, &request).await {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == SECRET_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let mut configuration = Map::new();
    for property in [NAME, "Description", "KmsKeyId"] {
        if let Some(value) = answered.get(property).filter(|value| !value.is_null()) {
            configuration.insert(property.to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(configuration)))
}

/// A fresh [`CLIENT_REQUEST_TOKEN`] for one write of this secret.
///
/// A raw HTTP request has to carry one (see the module docs), and it is what the version the
/// write stakes is identified by — so it is MINTED per call rather than derived from the
/// value being written, whose digest would otherwise travel as a version id a reader of the
/// secret's metadata can see.
fn client_request_token(resource: &StoredResource) -> String {
    crate::state::new_identifier_digits(
        &format!("{SECRETSMANAGER_SECRET}/{}", resource.logical_id),
        CLIENT_REQUEST_TOKEN_CHARACTERS,
    )
}

/// The NAME the secret is created under: the `Name` the template asked for, or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated for a declaration that
/// names none.
///
/// The generated name travels as the resource's `Name` attribute, because this type's
/// physical id is the ARN the owning service mints; a record with no such attribute (one
/// whose attribute budget was spent) falls back to the name inside the stand-in ARN, which
/// is its last colon-separated segment without the six-character suffix that ARN form ends
/// in.
fn secret_name(resource: &StoredResource) -> String {
    if let Some(declared) = stated(&resource.properties, NAME).and_then(Value::as_str) {
        return declared.to_string();
    }
    if let Some(name) = resource.attributes.get(NAME) {
        return name.clone();
    }
    let addressed = resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default();
    match addressed.rsplit_once('-') {
        Some((name, _suffix)) => name.to_string(),
        None => addressed.to_string(),
    }
}

/// Which of the two forms a declaration states the secret's VALUE in.
#[derive(Debug, Clone)]
enum ValueForm<'a> {
    /// The value itself, as `SecretString` states it.
    Verbatim(String),
    /// The generation `GenerateSecretString` asks the owning service for.
    Generated(&'a Value),
}

/// The form the declaration states the secret's value in, or `None` for a declaration that
/// states neither (the type publishes both as optional, and a secret may be created empty
/// and written later by `PutSecretValue`).
///
/// The two forms are ALTERNATIVES, so a declaration stating both describes no single value
/// and is refused rather than deployed with one of them dropped.
fn value_form(properties: &Value) -> Result<Option<ValueForm<'_>>, Refusal> {
    match (
        stated(properties, SECRET_STRING),
        stated(properties, GENERATE_SECRET_STRING),
    ) {
        (Some(_), Some(_)) => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{SECRETSMANAGER_SECRET} states both {SECRET_STRING} and \
                 {GENERATE_SECRET_STRING}, which are alternatives; the secret was not created"
            ),
        }),
        (Some(value), None) => Ok(Some(ValueForm::Verbatim(attribute_text(value)))),
        (None, Some(generation)) => Ok(Some(ValueForm::Generated(generation))),
        (None, None) => Ok(None),
    }
}

/// Whether `declared` states a DIFFERENT value for the secret than the one in force: either
/// value form written differently is a changed value, and two declarations stating the same
/// form identically ask for the same value however that form is spelled.
///
/// It is what keeps an update that changed something else from writing the secret again (see
/// [`update`]).
fn value_is_changed(declared: &Value, in_force: &Value) -> bool {
    [SECRET_STRING, GENERATE_SECRET_STRING]
        .iter()
        .any(|property| stated(declared, property) != stated(in_force, property))
}

/// The value one [`ValueForm`] names: the one the declaration stated, or the one the owning
/// service generated for it.
async fn resolved_value(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    form: ValueForm<'_>,
) -> Result<String, Refusal> {
    match form {
        ValueForm::Verbatim(value) => Ok(value),
        ValueForm::Generated(generation) => generated_secret_string(ctx, region, generation).await,
    }
}

/// The value a `GenerateSecretString` declaration asks the owning service for.
///
/// The generation parameters are `GetRandomPassword`'s own members
/// ([`PASSWORD_MEMBERS`]), so the password is the one THAT service produced rather than
/// anything this module composed. Where the declaration also states a `SecretStringTemplate`
/// and a `GenerateStringKey`, the password is put into that JSON document under that key —
/// "SecretStringTemplate: a string template that is a JSON structure" the generated string is
/// merged into — and the merged document is the value. The two are a PAIR: a declaration
/// stating one without the other describes no document and is refused.
async fn generated_secret_string(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    generation: &Value,
) -> Result<String, Refusal> {
    let mut request = Map::new();
    for member in PASSWORD_MEMBERS {
        if let Some(value) = stated(generation, member) {
            request.insert((*member).to_string(), value.clone());
        }
    }
    let answered =
        call_secretsmanager(ctx, region, "GetRandomPassword", &Value::Object(request)).await?;
    let password = answered
        .get("RandomPassword")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal {
            code: INTERNAL_FAILURE.to_string(),
            message: "AWS Secrets Manager answered GetRandomPassword without a RandomPassword"
                .to_string(),
        })?
        .to_string();
    let template = stated(generation, SECRET_STRING_TEMPLATE).map(attribute_text);
    let key = stated(generation, GENERATE_STRING_KEY).map(attribute_text);
    match (template, key) {
        (None, None) => Ok(password),
        (Some(template), Some(key)) => merged_secret_document(&template, &key, &password),
        _ => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "{GENERATE_SECRET_STRING} states one of {SECRET_STRING_TEMPLATE} and \
                 {GENERATE_STRING_KEY} without the other, which together name where the \
                 generated value goes; the secret was not created"
            ),
        }),
    }
}

/// `template` with `password` put in under `key`: the document a `SecretStringTemplate` plus
/// a `GenerateStringKey` declares the secret's value to be.
///
/// The template has to BE a JSON object for a key to be added to it, so a template that is
/// anything else is refused rather than replaced by a document this module invented.
fn merged_secret_document(template: &str, key: &str, password: &str) -> Result<String, Refusal> {
    let mut document = match serde_json::from_str::<Value>(template) {
        Ok(Value::Object(members)) => members,
        _ => {
            return Err(Refusal {
                code: INVALID_REQUEST.to_string(),
                message: format!(
                    "{SECRET_STRING_TEMPLATE} is not a JSON structure, so the generated value \
                     has nowhere to go; the secret was not created"
                ),
            })
        }
    };
    document.insert(key.to_string(), Value::String(password.to_string()));
    serde_json::to_string(&Value::Object(document)).map_err(|error| Refusal {
        code: INTERNAL_FAILURE.to_string(),
        message: error.to_string(),
    })
}

/// The value a declaration STATES for `property`, or `None` for one it leaves out or states
/// as null.
fn stated<'a>(properties: &'a Value, property: &str) -> Option<&'a Value> {
    properties.get(property).filter(|value| !value.is_null())
}

/// One declared value as the TEXT a string member carries: a JSON string as it is, and any
/// other scalar as the template spelled it — a template may write a value through a `Ref` to
/// a parameter of another type, and the owning service's own validation is what judges it.
///
/// The rule is stated once in the shared crate
/// ([`::service_common::json_wire_values::value_text`]); this names it for the declarations
/// this module reads.
fn attribute_text(value: &Value) -> String {
    ::service_common::json_wire_values::value_text(value)
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a secret from being reported as created — or as updated —
/// with less than its declaration asked for.
fn refuse_unapplied_properties(properties: &Value, resource_type: &str) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if SECRET_PROPERTIES.contains(&property.as_str()) {
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

    /// A record as the deployment hands one to this module: the stand-in ARN the naming
    /// rules computed as its physical id, and the computed name among its attributes.
    fn declared(properties: Value, name: &str) -> StoredResource {
        let mut attributes = BTreeMap::new();
        attributes.insert(NAME.to_string(), name.to_string());
        StoredResource {
            logical_id: "Secret".to_string(),
            resource_type: SECRETSMANAGER_SECRET.to_string(),
            physical_resource_id: format!(
                "arn:aws:secretsmanager:us-west-2:000000000000:secret:{name}-AbCdEf"
            ),
            attributes,
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// RULE: the secret is created under the name CloudFormation decided — the one the
    /// declaration states, the generated one it carries as an attribute, or the name inside
    /// the stand-in ARN — never under the ARN that is this type's physical id.
    #[test]
    fn the_secret_is_created_under_the_name_cloudformation_decided() {
        let rows: &[(&str, Value, bool, &str)] = &[
            (
                "a declared name",
                serde_json::json!({"Name": "db-credentials"}),
                true,
                "db-credentials",
            ),
            (
                "a generated name",
                serde_json::json!({}),
                true,
                "stack-Secret-ABC123",
            ),
            (
                "a record with no Name attribute",
                serde_json::json!({}),
                false,
                "stack-Secret-ABC123",
            ),
        ];
        for (case, properties, attributed, expected) in rows {
            let mut resource = declared(properties.clone(), "stack-Secret-ABC123");
            if !attributed {
                resource.attributes.remove(NAME);
            }
            let actual = secret_name(&resource);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected the name {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the two value forms are alternatives — a declaration stating both describes no
    /// single value and is refused, naming both — and each on its own is the form it states.
    #[test]
    fn the_two_value_forms_are_alternatives() {
        let both = serde_json::json!({
            "SecretString": "a-value",
            "GenerateSecretString": {"PasswordLength": 20}
        });
        let refusal =
            value_form(&both).expect_err("case \"both value forms\": expected a refusal");
        assert!(
            refusal.message.contains(SECRET_STRING)
                && refusal.message.contains(GENERATE_SECRET_STRING),
            "case \"both value forms\": expected the refusal to name both forms, got {:?}",
            refusal.message
        );
        let rows: &[(&str, Value, Option<&str>)] = &[
            (
                "a stated value",
                serde_json::json!({"SecretString": "a-value"}),
                Some("a-value"),
            ),
            (
                "a value stated as a document",
                serde_json::json!({"SecretString": {"username": "admin"}}),
                Some("{\"username\":\"admin\"}"),
            ),
            ("neither form", serde_json::json!({"Name": "a-secret"}), None),
        ];
        for (case, properties, expected) in rows {
            let form = value_form(properties)
                .unwrap_or_else(|refusal| panic!("case {case:?}: expected a form, got {refusal:?}"));
            let actual = match form {
                Some(ValueForm::Verbatim(value)) => Some(value),
                Some(ValueForm::Generated(_)) => Some("a generation".to_string()),
                None => None,
            };
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
        let generated = serde_json::json!({"GenerateSecretString": {"PasswordLength": 20}});
        let form = value_form(&generated).expect("case \"a generation\": expected a form");
        assert!(
            matches!(form, Some(ValueForm::Generated(_))),
            "case \"a generation\": expected the generated form, got {form:?}"
        );
    }

    /// RULE: a generated password goes into the declared JSON document under the declared
    /// key, and a template that is no JSON structure is refused rather than replaced.
    #[test]
    fn the_generated_password_is_merged_into_the_declared_document() {
        let merged = merged_secret_document("{\"username\":\"admin\"}", "password", "s3cret")
            .expect("case \"a JSON structure\": expected the merged document");
        let parsed: Value =
            serde_json::from_str(&merged).expect("case \"a JSON structure\": expected JSON");
        assert_eq!(
            parsed,
            serde_json::json!({"username": "admin", "password": "s3cret"}),
            "case \"a JSON structure\": expected the password under the declared key, got \
             {merged:?}"
        );
        for (case, template) in [
            ("a JSON array", "[1, 2]"),
            ("a bare scalar", "\"text\""),
            ("not JSON at all", "{username"),
        ] {
            let refusal = merged_secret_document(template, "password", "s3cret")
                .expect_err(&format!("case {case:?}: expected a refusal"));
            assert!(
                refusal.message.contains(SECRET_STRING_TEMPLATE),
                "case {case:?}: expected the refusal to name the template property, got {:?}",
                refusal.message
            );
        }
    }

    /// RULE: an update writes the value only where the declaration CHANGES it, so a
    /// deployment that changed something else does not rotate a value no declaration changed.
    #[test]
    fn the_value_is_written_only_where_the_declaration_changes_it() {
        let rows: &[(&str, Value, Value, bool)] = &[
            (
                "the same generation stated twice",
                serde_json::json!({"GenerateSecretString": {"PasswordLength": 20}}),
                serde_json::json!({"GenerateSecretString": {"PasswordLength": 20}}),
                false,
            ),
            (
                "only the description changed",
                serde_json::json!({"Description": "now", "SecretString": "a-value"}),
                serde_json::json!({"Description": "before", "SecretString": "a-value"}),
                false,
            ),
            (
                "the stated value changed",
                serde_json::json!({"SecretString": "a-new-value"}),
                serde_json::json!({"SecretString": "a-value"}),
                true,
            ),
            (
                "the generation parameters changed",
                serde_json::json!({"GenerateSecretString": {"PasswordLength": 32}}),
                serde_json::json!({"GenerateSecretString": {"PasswordLength": 20}}),
                true,
            ),
            (
                "the form changed",
                serde_json::json!({"SecretString": "a-value"}),
                serde_json::json!({"GenerateSecretString": {"PasswordLength": 20}}),
                true,
            ),
            (
                "a value added where there was none",
                serde_json::json!({"SecretString": "a-value"}),
                serde_json::json!({"Description": "before"}),
                true,
            ),
        ];
        for (case, declared, in_force, expected) in rows {
            let actual = value_is_changed(declared, in_force);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected changed={expected}, got {actual}"
            );
        }
    }

    /// RULE: a declared property no call here applies is refused, naming it; every property
    /// the type publishes is accepted.
    #[test]
    fn an_unapplied_property_is_refused() {
        let published = serde_json::json!({
            "Description": "the application's credentials",
            "GenerateSecretString": {"PasswordLength": 32},
            "KmsKeyId": "alias/aws/secretsmanager",
            "Name": "db-credentials",
            "ReplicaRegions": [{"Region": "us-east-1"}],
            "SecretString": "a-value",
            "Tags": []
        });
        assert!(
            refuse_unapplied_properties(&published, SECRETSMANAGER_SECRET).is_ok(),
            "case \"every published property\": expected it to be applied, got a refusal"
        );
        let unknown = serde_json::json!({"Name": "db-credentials", "Unheard": 1});
        let refusal = refuse_unapplied_properties(&unknown, SECRETSMANAGER_SECRET)
            .expect_err("case \"a property no call carries\": expected a refusal");
        assert!(
            refusal.message.contains("Unheard"),
            "case \"a property no call carries\": expected the refusal to name it, got {:?}",
            refusal.message
        );
    }

    /// RULE: the tag list carries the declaration's own tags and the ones CloudFormation
    /// propagates, under the member names the modelled `Tag` spells — and never a key in the
    /// reserved namespace, which this service refuses on a caller's request.
    #[test]
    fn the_tag_list_leaves_the_reserved_namespace_alone() {
        let properties = serde_json::json!({"Tags": [{"Key": "Owner", "Value": "platform"}]});
        let propagated = vec![
            ("Stack".to_string(), "secrets-suite".to_string()),
            (
                "aws:cloudformation:logical-id".to_string(),
                "Secret".to_string(),
            ),
        ];
        let tags = applicable_tags(&tags_of(&properties, &propagated));
        let rendered = key_value_tag_list(&tags);
        assert_eq!(
            rendered,
            serde_json::json!([
                {"Key": "Owner", "Value": "platform"},
                {"Key": "Stack", "Value": "secrets-suite"}
            ]),
            "case \"a declaration beside the propagated tags\": expected the declared and \
             stack keys alone, got {rendered}"
        );
    }
}
